//! Awake USB development cycle with real work and simulated sleep/faults.

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
    gesture::{FaultSwitch, ReleaseGate},
    safety::{SafetyState, WorkPhase},
    schedule::{RefreshSchedule, MANUAL_LIMIT_US, USB_INTERVAL_US},
};
use esp_idf_svc::{
    hal::{
        delay::FreeRtos,
        gpio::{PinDriver, Pull},
        peripherals::Peripherals,
        reset::ResetReason,
    },
    wifi::{BlockingWifi, EspWifi},
};
use std::time::Instant;

pub struct Application {
    button: Button,
    fault_button: Button,
    fault_switch: FaultSwitch,
    release: ReleaseGate,
    simulated_fault: bool,
    safety: Guard,
    wifi: BlockingWifi<EspWifi<'static>>,
    battery: Option<BatterySensor<'static>>,
    schedule: RefreshSchedule,
    next_status_us: u64,
}

impl Application {
    /// Boot USB mode, halting with a logged fault if setup fails.
    pub fn boot() -> Self {
        super::initialize_runtime();
        work::require(Self::try_boot())
    }

    /// Initialize shared hardware under a USB-only durable safety session.
    fn try_boot() -> Result<Self, AppError> {
        Epd::disable_panel_power()?;
        let peripherals = Peripherals::take()?;
        let button = PinDriver::rtc_input(peripherals.pins.gpio2, Pull::Up)?;
        let fault_button = PinDriver::rtc_input(peripherals.pins.gpio3, Pull::Up)?;
        let fault_switch = FaultSwitch::new(Instant::now(), fault_button.is_low());
        let mut safety = Self::open_safety()?;
        work::require(safety.begin_work());
        work::require(work::set_phase(&mut safety, WorkPhase::Setup));
        let wifi = match Self::setup_work_hardware(peripherals.modem) {
            Ok(wifi) => wifi,
            Err(error) => {
                log::error!("USB work hardware setup failed: {error}");
                work::latch_and_halt(&mut safety, error);
            }
        };
        let battery = BatterySensor::try_new(peripherals.adc1, peripherals.pins.gpio8);
        work::finish(&mut safety);
        log::warn!("USB mode: REAL upload/manual/scheduled display; simulated deep sleep; timer=60 s from completion; manual limit=1 h; production remains 24 h / 1 h");
        log::warn!("USB safety: simulated fault initially OFF; KEY1 toggles while idle; runtime safety namespace=hof_usb, state={:?}; production hof_safety untouched; KEY1 cannot clear real faults; battery estimate is display-only; USB power only", safety.state());
        Ok(Self {
            button,
            fault_button,
            fault_switch,
            release: ReleaseGate::default(),
            simulated_fault: false,
            safety,
            wifi,
            battery,
            schedule: RefreshSchedule::new(rtc::now_us()),
            next_status_us: 0,
        })
    }

    /// Verify PSRAM and configure the upload hotspot inside the setup work boundary.
    fn setup_work_hardware(
        modem: esp_idf_svc::hal::modem::Modem<'static>,
    ) -> Result<BlockingWifi<EspWifi<'static>>, AppError> {
        psram::verify()?;
        Ok(upload_wifi::setup(modem)?)
    }

    /// Log production diagnostics read-only, then reset only the USB namespace.
    fn open_safety() -> Result<Guard, AppError> {
        let reset = ResetReason::get();
        log::warn!("USB session reset reason: {reset:?}; development reboot resets USB runtime guard, not production recovery");
        if let Err(error) = safety_nvs::log_production_diagnostics() {
            log::error!("could not read production safety NVS snapshot: {error}");
        }
        Ok(safety_nvs::open_usb_session(reset == ResetReason::Brownout)?)
    }

    /// USB waits for KEY0 release without an immediate boot refresh.
    pub fn initial_state(&self) -> State {
        log::info!("mock PrepareSleep: waiting for KEY0 release; initial timer in 60 s");
        State::Idle
    }

    /// Execute one state handler; synchronous work suspends idle KEY1 polling.
    pub fn advance(&mut self, state: State) -> State {
        match state {
            State::Idle => self.idle(),
            State::Classify(wake_at) => self.classify(wake_at),
            State::Upload(wake_at) => {
                self.upload(wake_at);
                State::PrepareSleep
            }
            State::Refresh { manual } => {
                self.refresh(manual);
                if manual {
                    State::PrepareSleep
                } else {
                    State::Idle
                }
            }
            State::PrepareSleep => {
                log::info!("mock PrepareSleep: waiting for KEY0 release; timer stays active");
                State::Idle
            }
        }
    }

    /// Poll fault injection and give overdue timer work priority over gestures.
    fn idle(&mut self) -> State {
        let now = Instant::now();
        let now_us = rtc::now_us();
        if self.fault_switch.sample(now, self.fault_button.is_low()) {
            self.simulated_fault = !self.simulated_fault;
            log::warn!(
                "USB KEY1: simulated fault={}; real safety={:?}; production latch unchanged",
                self.simulated_fault,
                self.safety.state()
            );
        }
        if !self.simulated_fault && self.schedule.due(now_us, USB_INTERVAL_US) {
            log::info!("USB: TIMER wake; no button gesture required");
            return State::Refresh { manual: false };
        }
        let armed_before = self.release.armed();
        if self.release.sample(now, self.button.is_low()) {
            return State::Classify(Instant::now());
        }
        if !armed_before && self.release.armed() {
            log::info!("mock DeepSleep: READY; press KEY0 to start the next gesture");
        }
        self.log_status(now_us);
        FreeRtos::delay_ms(buttons::POLL_MS);
        State::Idle
    }

    /// Classify a waking press and translate pure policy into the next state.
    fn classify(&mut self, wake_at: Instant) -> State {
        log::info!("mock Boot: KEY0 wake, press 1");
        let gesture = buttons::classify(&self.button, wake_at);
        log::info!(
            "KEY0 gesture test: {gesture:?} after {:.3} s; panel OFF, Wi-Fi OFF",
            wake_at.elapsed().as_secs_f32()
        );
        match dispatch::decide(
            Some(gesture),
            self.safety.state(),
            self.simulated_fault,
            self.schedule,
            rtc::now_us(),
        ) {
            Decision::Upload => State::Upload(wake_at),
            Decision::Refresh { manual } => State::Refresh { manual },
            Decision::Blocked => {
                log::warn!("USB: work BLOCKED; simulated fault={}; runtime safety={:?}; no acknowledgement performed",
                    self.simulated_fault, self.safety.state());
                State::PrepareSleep
            }
            Decision::RateLimited => {
                log::info!(
                    "mock ManualRefresh: RATE LIMITED; daily deadline unchanged={} us",
                    self.schedule.deadline_us(USB_INTERVAL_US)
                );
                State::PrepareSleep
            }
            Decision::Sleep => {
                log::info!(
                    "mock Sleep: no refresh; daily deadline unchanged={} us",
                    self.schedule.deadline_us(USB_INTERVAL_US)
                );
                State::PrepareSleep
            }
        }
    }

    /// Refresh with Wi-Fi off; only cleanly recovered display failures may continue.
    fn refresh(&mut self, manual: bool) {
        if self.safety.state() != SafetyState::Clean {
            log::warn!("USB refresh: BLOCKED by persistent runtime-fault latch");
            if !manual {
                self.schedule.defer_retry(rtc::now_us(), USB_INTERVAL_US);
            }
            return;
        }
        work::require(self.safety.begin_work());
        let source = if manual {
            "manual refresh"
        } else {
            "scheduled refresh"
        };
        log::info!("{source}: starting REAL saved-frame refresh; Wi-Fi OFF; USB connected");
        let outcome = match display::refresh_saved(&mut self.safety) {
            Ok(outcome) => outcome,
            Err(error) => {
                log::error!("{source}: FAILED: {error}; daily deadline unchanged; no automatic physical retry");
                if let Err(power_error) = Epd::disable_panel_power() {
                    log::error!("{source}: emergency panel power cutoff failed: {power_error}");
                }
                work::latch_and_halt(&mut self.safety, error);
            }
        };
        work::finish(&mut self.safety);
        let completed_at = rtc::now_us();
        match outcome {
            DisplayOutcome::Failed(error) => {
                self.schedule.defer_retry(completed_at, USB_INTERVAL_US);
                log::error!("{source}: display FAILED: {error}; cleanup complete, NVS clean; continuing; next scheduled attempt in 60 s");
            }
            DisplayOutcome::Displayed => {
                self.schedule.refresh_succeeded(completed_at, manual);
                log::info!("{source}: SUCCESS; panel OFF; Wi-Fi OFF; NVS clean; daily deadline restarted; next timer in 60 s, deadline={} us",
                    self.schedule.deadline_us(USB_INTERVAL_US));
            }
        }
    }

    /// Run the upload window and clean idle-expiry work without moving its deadline.
    fn upload(&mut self, wake_at: Instant) {
        work::require(self.safety.begin_work());
        let percentage = BatterySensor::read_optional(self.battery.as_mut());
        if let Err(error) = upload_wifi::run(
            &mut self.wifi,
            wake_at,
            &mut self.safety,
            &mut self.schedule,
            percentage,
        ) {
            log::error!("UploadWindow failed: {error}");
            work::latch_and_halt(&mut self.safety, error);
        }
        if self.safety.state() == SafetyState::Working {
            work::finish(&mut self.safety);
        }
        log::info!(
            "mock Upload: processing ended; panel OFF; daily deadline={} us",
            self.schedule.deadline_us(USB_INTERVAL_US)
        );
    }

    /// Emit bounded idle diagnostics without altering work or deadlines.
    fn log_status(&mut self, now_us: u64) {
        if now_us < self.next_status_us {
            return;
        }
        log::info!("mock deep sleep: daily in {:.1} s; manual allowed={}; button armed={}; runtime safety={:?}; simulated fault={}; USB connected",
            self.schedule.deadline_us(USB_INTERVAL_US).saturating_sub(now_us) as f64 / 1_000_000.0,
            self.schedule.manual_allowed(now_us, MANUAL_LIMIT_US)
                && self.safety.state() == SafetyState::Clean && !self.simulated_fault,
            self.release.armed(), self.safety.state(), self.simulated_fault);
        self.next_status_us = now_us.saturating_add(5_000_000);
    }
}
