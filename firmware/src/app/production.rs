//! One production wake cycle, followed by real deep sleep.

use super::{AppError, State};
use crate::{
    battery::BatterySensor,
    buttons::{self, Button},
    display::{self, DisplayOutcome},
    epd::Epd,
    psram, rtc, safety_nvs, upload_wifi,
    work::{self, Guard},
};
use crate::{
    dispatch::{self, Decision},
    gesture::Gesture,
    safety::{SafetyState, WorkPhase},
    schedule::DAILY_INTERVAL_US,
};
use esp_idf_svc::hal::{
    gpio::{PinDriver, Pull},
    peripherals::Peripherals,
    reset::{ResetReason, WakeupReason},
};
use std::time::Instant;

pub struct Application {
    button: Button,
    upload_hardware: Option<UploadHardware>,
    safety: Guard,
    wake_at: Instant,
    gesture: Option<Gesture>,
    interrupted: bool,
}

struct UploadHardware {
    modem: esp_idf_svc::hal::modem::Modem<'static>,
    adc: esp_idf_svc::hal::adc::ADC1<'static>,
    battery_pin: esp_idf_svc::hal::gpio::Gpio8<'static>,
}

impl Application {
    /// Boot the production wake, halting with a logged fault if setup fails.
    pub fn boot() -> Self {
        let wake_at = Instant::now();
        super::initialize_runtime();
        work::require(Self::try_boot(wake_at))
    }

    /// Read the wake gesture and recovery records before deciding whether work is safe.
    fn try_boot(wake_at: Instant) -> Result<Self, AppError> {
        let peripherals = Peripherals::take()?;
        let button = PinDriver::rtc_input(peripherals.pins.gpio2, Pull::Up)?;
        let reset = ResetReason::get();
        let wake = WakeupReason::get();
        Epd::disable_panel_power()?;
        let safety = safety_nvs::open("hof_safety", reset == ResetReason::Brownout)?;
        let gesture = if reset == ResetReason::DeepSleep && wake == WakeupReason::Button {
            Some(buttons::classify(
                &button,
                wake_at,
                safety.state() == SafetyState::Latched,
            ))
        } else {
            None
        };
        let now_us = rtc::now_us();
        rtc::initialize(now_us);
        if safety.state() != SafetyState::Clean {
            log::error!("persistent safety latch set; refresh/upload blocked; hold the waking KEY0 press for 10 s to acknowledge after confirming stable power");
            rtc::update(|state| state.schedule.defer_retry(now_us, DAILY_INTERVAL_US));
        }
        log::info!("firmware booting; reset reason: {reset:?}, wakeup reason: {wake:?}");
        let interrupted = rtc::recover_interrupted(now_us, reset);
        if interrupted {
            log::error!("previous panel refresh was interrupted or reset was caused by brownout; skipping refresh; persistent safety latch takes precedence over RTC cooldown");
        }
        let upload_hardware = UploadHardware {
            modem: peripherals.modem,
            adc: peripherals.adc1,
            battery_pin: peripherals.pins.gpio8,
        };
        Ok(Self {
            button,
            upload_hardware: Some(upload_hardware),
            safety,
            wake_at,
            gesture,
            interrupted,
        })
    }

    /// Production starts by selecting work for this wake.
    pub fn initial_state(&self) -> State {
        State::Dispatch
    }

    /// Execute one state handler and return the next state.
    pub fn advance(&mut self, state: State) -> State {
        match state {
            State::Dispatch => self.dispatch(),
            State::Upload(wake_at) => {
                self.upload(wake_at);
                State::PrepareSleep
            }
            State::Refresh { manual } => {
                self.refresh(manual);
                State::PrepareSleep
            }
            State::PrepareSleep => self.sleep(),
        }
    }

    /// Enter deep sleep; halting is the only safe response if wake sources cannot be armed.
    fn sleep(&self) -> State {
        let Err(error) = buttons::sleep(&self.button);
        log::error!("deep sleep could not be armed: {error}");
        work::halt(error)
    }

    /// Translate the pure work decision into a production state and diagnostic log.
    fn dispatch(&mut self) -> State {
        let now_us = rtc::now_us();
        let schedule = rtc::retained().schedule;
        match dispatch::decide(
            self.gesture,
            self.safety.state(),
            self.interrupted || schedule.cooling_down(now_us),
            schedule,
            now_us,
        ) {
            Decision::Upload => State::Upload(self.wake_at),
            Decision::Refresh { manual } => {
                log::info!(
                    "{}; refreshing the display",
                    if manual {
                        "triple press detected"
                    } else {
                        "timer or cold/reset wake"
                    }
                );
                State::Refresh { manual }
            }
            Decision::Blocked => {
                log::warn!("work suppressed by safety latch or interrupted-work recovery cooldown; hold the waking KEY0 press for 10 s to acknowledge a latch");
                State::PrepareSleep
            }
            Decision::ClearLatch => {
                // The long hold is the qualified manual acknowledgement; the
                // operator asserts stable power because automated
                // recovery-voltage measurement is not implemented.
                work::require(self.safety.acknowledge(true, true));
                rtc::update(|state| {
                    state.refresh_in_progress = 0;
                    state.schedule.clear_retry();
                });
                log::warn!("manual latch acknowledgement: KEY0 held for 10 s; hof_safety committed Clean; last recorded fault retained; returning to sleep, work resumes on the next wake");
                State::PrepareSleep
            }
            Decision::RateLimited => {
                log::info!("manual refresh rate-limited; returning to sleep");
                State::PrepareSleep
            }
            Decision::Sleep => {
                log::info!("short or incomplete KEY0 gesture; returning to sleep");
                State::PrepareSleep
            }
        }
    }

    /// Mark retained and durable work before a physical operation.
    fn begin_work(&mut self) {
        work::require(self.safety.begin_work());
        rtc::update(|state| state.refresh_in_progress = 1);
    }

    /// Run a bounded upload session, retaining success inside the pipeline.
    fn upload(&mut self, wake_at: Instant) {
        log::warn!("normal upload: REAL display and deep sleep; battery voltage guard is NOT qualified; stable bench power only");
        match self.run_upload(wake_at) {
            Ok(()) => {
                work::finish(&mut self.safety);
                rtc::update(|state| state.refresh_in_progress = 0);
            }
            Err(error) => {
                log::error!("normal upload failed: {error}; latching work without retry");
                self.record_failure(true);
            }
        }
    }

    /// Read the battery, mark work, then verify hardware and serve the upload window.
    fn run_upload(&mut self, wake_at: Instant) -> Result<(), AppError> {
        let hardware = self
            .upload_hardware
            .take()
            .ok_or(AppError::UploadHardwareUsed)?;
        let mut sensor = BatterySensor::try_new(hardware.adc, hardware.battery_pin);
        let percentage = BatterySensor::read_optional(sensor.as_mut());
        self.begin_work();
        let mut schedule = rtc::retained().schedule;
        work::set_phase(&mut self.safety, WorkPhase::Setup)?;
        psram::verify()?;
        let mut wifi = upload_wifi::setup(hardware.modem)?;
        upload_wifi::run(&mut wifi, wake_at, &mut self.safety, &mut schedule, percentage)?;
        Ok(())
    }

    /// Refresh with Wi-Fi off and retain completion only after durable cleanup.
    fn refresh(&mut self, manual: bool) {
        self.begin_work();
        match display::refresh_saved(&mut self.safety) {
            Ok(DisplayOutcome::Displayed) => {
                work::finish(&mut self.safety);
                let completed_at = rtc::now_us();
                rtc::update(|state| {
                    state.schedule.refresh_succeeded(completed_at, manual);
                    state.refresh_in_progress = 0;
                });
            }
            Err(error) => {
                log::error!("display refresh failed: {error}");
                self.record_failure(false);
            }
        }
    }

    /// Cut power and latch faults; upload failures always defer the next retry.
    fn record_failure(&mut self, upload: bool) {
        work::require(Epd::disable_panel_power());
        work::require(self.safety.latch());
        let failed_at = rtc::now_us();
        rtc::update(|state| {
            state.refresh_in_progress = 0;
            if upload {
                state.schedule.defer_retry(failed_at, DAILY_INTERVAL_US);
            } else {
                state.schedule.refresh_failed(failed_at, DAILY_INTERVAL_US);
            }
        });
    }
}
