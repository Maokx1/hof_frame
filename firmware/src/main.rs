//! EE04 display bring-up with KEY0 gesture handling and scheduled deep sleep.

mod battery;
mod epd;
mod frame_flash;
mod image_decode;
mod psram;
mod safety_nvs;
mod upload_flash;
mod upload_wifi;

use std::time::{Duration, Instant};

use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::gpio::Pull;
use esp_idf_svc::hal::gpio::{PinDriver, RtcInput};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::reset::ResetReason;
#[cfg(not(feature = "usb"))]
use esp_idf_svc::hal::reset::WakeupReason;
#[cfg(not(feature = "usb"))]
use esp_idf_svc::hal::sleep::{rtc::RtcWakeLevel, DeepSleep};
use esp_idf_svc::sys::EspError;

use firmware::gesture::{Gesture, GestureClassifier, BUTTON_DEBOUNCE};
use firmware::safety::SafetyGuard;
use firmware::safety::SafetyState;
use firmware::safety::WorkPhase;
#[cfg(not(feature = "usb"))]
use firmware::schedule::MANUAL_LIMIT_US;
use firmware::schedule::{RefreshSchedule, DAILY_INTERVAL_US};

const BUTTON_POLL_MS: u32 = 5;
const BUTTON_RELEASE_TIMEOUT: Duration = Duration::from_secs(60);

const RETAINED_MAGIC: u32 = 0x484F_4632;

#[repr(C)]
#[derive(Clone, Copy)]
struct RetainedState {
    magic: u32,
    schedule: RefreshSchedule,
    refresh_in_progress: u32,
}

#[link_section = ".rtc.data"]
static mut RETAINED_STATE: RetainedState = RetainedState {
    magic: 0,
    schedule: RefreshSchedule::new(0),
    refresh_in_progress: 0,
};

extern "C" {
    fn esp_rtc_get_time_us() -> i64;
}

fn rtc_time_us() -> u64 {
    // ESP-IDF's RTC clock continues across deep sleep, unlike esp_timer.
    unsafe { esp_rtc_get_time_us().max(0) as u64 }
}

fn read_battery_percentage(sensor: Option<&mut battery::BatterySensor<'_>>) -> Option<u8> {
    let sensor = sensor?;
    match sensor.read_percentage() {
        Ok(Some(percentage)) => Some(percentage),
        Ok(None) => {
            log::warn!(
                "battery reading outside the expected pack-voltage range; website will show ?%"
            );
            None
        }
        Err(error) => {
            log::warn!("battery ADC read failed; website will show ?%: {error:?}");
            None
        }
    }
}

fn retained_state() -> RetainedState {
    unsafe { std::ptr::addr_of!(RETAINED_STATE).read_volatile() }
}

fn update_retained_state(update: impl FnOnce(&mut RetainedState)) {
    unsafe {
        let state = std::ptr::addr_of_mut!(RETAINED_STATE);
        let mut value = state.read_volatile();
        update(&mut value);
        state.write_volatile(value);
    }
}

fn initialize_retained_state(now_us: u64) {
    update_retained_state(|state| {
        if state.magic != RETAINED_MAGIC {
            *state = RetainedState {
                magic: RETAINED_MAGIC,
                schedule: RefreshSchedule::new(now_us),
                refresh_in_progress: 0,
            };
        }
    });
}

fn recover_interrupted_refresh(now_us: u64, reset_reason: ResetReason) -> bool {
    let state = retained_state();
    let interrupted = state.refresh_in_progress != 0 || reset_reason == ResetReason::Brownout;
    if interrupted {
        update_retained_state(|state| {
            state.refresh_in_progress = 0;
            state.schedule.defer_retry(now_us, DAILY_INTERVAL_US);
        });
    }
    interrupted
}

enum DisplayOutcome {
    Displayed,
    #[cfg(feature = "usb")]
    Failed(EspError),
}

fn refresh_panel_with(
    safety: &mut SafetyGuard<safety_nvs::NvsSafetyStore>,
    color_at: impl Fn(usize, usize) -> u8,
) -> Result<DisplayOutcome, EspError> {
    set_work_phase(safety, WorkPhase::Display)?;
    log::info!("creating panel interface; panel load switch will turn on");
    let epd = match epd::Epd::new() {
        Ok(epd) => epd,
        Err(error) => {
            if let Err(power_error) = epd::Epd::disable_panel_power() {
                log::error!("panel setup failure power cutoff also failed: {power_error:?}");
            }
            return Err(error);
        }
    };
    log::info!("panel interface ready");
    let refresh = (|| {
        log::info!("sending panel initialization sequence");
        epd.init()?;
        log::info!("panel initialization complete");

        log::info!("streaming image pixels");
        let start = Instant::now();
        epd.show(color_at)?;
        log::info!(
            "panel show sequence completed in {:.1} s",
            start.elapsed().as_secs_f32()
        );
        Ok(())
    })();

    if let Err(error) = refresh {
        log::error!("panel display operation failed: {error:?}");
    }
    let phase = set_work_phase(safety, WorkPhase::PanelCleanup);
    let power_off = if refresh.is_ok() {
        epd.power_off()
    } else {
        log::warn!("display refresh failed; cutting panel power without another BUSY wait");
        epd::Epd::disable_panel_power()
    };
    let cleanup = epd.close();
    // Only a display operation failure is recoverable, never failed cleanup.
    phase?;
    if let Err(error) = power_off {
        log::error!("panel power-off failed: {error:?}; work blocked");
        if let Err(cleanup_error) = cleanup {
            log::error!("panel SPI cleanup also failed: {cleanup_error:?}");
        }
        return Err(error);
    }
    cleanup?;
    #[cfg(feature = "usb")]
    if let Err(error) = refresh {
        log::error!(
            "USB display refresh failed: {error:?}; panel OFF, SPI released; continuing execution"
        );
        return Ok(DisplayOutcome::Failed(error));
    }
    #[cfg(not(feature = "usb"))]
    if refresh.is_err() {
        set_work_phase(safety, WorkPhase::Display)?;
    }
    refresh?;
    Ok(DisplayOutcome::Displayed)
}

fn refresh_saved_frame(
    safety: &mut SafetyGuard<safety_nvs::NvsSafetyStore>,
) -> Result<DisplayOutcome, EspError> {
    set_work_phase(safety, WorkPhase::LoadFrame)?;
    let mut storage = frame_flash::FlashFrames::open()?;
    let scan = storage.load().map_err(|error| {
        log::error!(
            "LoadFrame failed: {error:?}; storage I/O errors are not a missing-frame fallback"
        );
        match error {
            firmware::frame::FrameError::Storage(error) => error,
            _ => EspError::from(esp_idf_svc::sys::ESP_ERR_INVALID_STATE).unwrap(),
        }
    })?;
    let metadata = scan
        .current
        .map(|record| record.metadata)
        .unwrap_or_default();
    log::info!("battery overlay: unknown (?%); calibrated measurement unavailable");
    let overlay = firmware::overlay::StatusOverlay::new(metadata, None).map_err(|error| {
        log::error!("battery overlay rendering failed: {error}; panel remains OFF");
        EspError::from(esp_idf_svc::sys::ESP_ERR_INVALID_STATE).unwrap()
    })?;
    log::info!(
        "battery overlay: corner={:?}, orientation={:?}",
        metadata.battery_corner,
        metadata.orientation
    );
    if let Some(record) = scan.current {
        let mapped = storage.map(record)?;
        log::info!("rendering validated native-order frame: {record:?}; live status overlay");
        refresh_panel_with(safety, |x, y| {
            if let Some(pixel) = overlay.pixel(x, y) {
                return pixel;
            }
            let index = y * firmware::frame::WIDTH + x;
            let byte = mapped.bytes()[index / 2];
            if index % 2 == 0 {
                byte >> 4
            } else {
                byte & 15
            }
        })
    } else {
        refresh_panel_with(safety, |x, y| {
            overlay
                .pixel(x, y)
                .unwrap_or_else(|| firmware::frame::default_pixel(x, y))
        })
    }
}

fn set_work_phase(
    safety: &mut SafetyGuard<safety_nvs::NvsSafetyStore>,
    phase: WorkPhase,
) -> Result<(), EspError> {
    safety.set_phase(phase).map_err(|error| {
        log::error!("safety diagnostics write failed: {error:?}; work blocked");
        match error {
            firmware::safety::SafetyError::Storage(error) => error,
            firmware::safety::SafetyError::Blocked => {
                EspError::from(esp_idf_svc::sys::ESP_ERR_INVALID_STATE).unwrap()
            }
        }
    })
}

fn classify_gesture(button: &PinDriver<'_, RtcInput>, wake_at: Instant) -> Gesture {
    let mut classifier = GestureClassifier::new(wake_at, button.is_low());
    loop {
        let now = Instant::now();
        if let Some(gesture) = classifier.sample(now, button.is_low()) {
            return gesture;
        }
        FreeRtos::delay_ms(BUTTON_POLL_MS);
    }
}

#[cfg(not(feature = "usb"))]
fn sleep_until_refresh(button: &PinDriver<'_, RtcInput>) -> ! {
    let release_started = Instant::now();
    let mut released_since = None;
    let button_released = loop {
        let now = Instant::now();
        if button.is_low() {
            released_since = None;
        } else if released_since.is_none() {
            released_since = Some(now);
        } else if now.duration_since(released_since.expect("release start set")) >= BUTTON_DEBOUNCE
        {
            break true;
        }

        if now.duration_since(release_started) >= BUTTON_RELEASE_TIMEOUT {
            log::warn!("KEY0 remained pressed for 60 s; disabling button wake until timer wake");
            break false;
        }
        FreeRtos::delay_ms(BUTTON_POLL_MS);
    };

    let now_us = rtc_time_us();
    let deadline_us = retained_state().schedule.deadline_us(DAILY_INTERVAL_US);
    let remaining_us = deadline_us.saturating_sub(now_us).max(1_000_000);
    let mut deep_sleep = DeepSleep::new()
        .expect("failed to reset deep-sleep wakeup sources")
        .wakeup_on_timer(Duration::from_micros(remaining_us))
        .expect("failed to configure RTC timer wake");

    if button_released {
        deep_sleep = deep_sleep
            .wakeup_on_rtc(button, RtcWakeLevel::AllLow)
            .expect("failed to configure KEY0 wake");
    }

    log::info!(
        "entering deep sleep; next refresh in {} s{}",
        remaining_us / 1_000_000,
        if button_released {
            " with KEY0 wake enabled"
        } else {
            " with timer wake only"
        }
    );
    deep_sleep.enter()
}

#[cfg(feature = "usb")]
const USB_REFRESH_INTERVAL_US: u64 = firmware::schedule::USB_INTERVAL_US;
#[cfg(feature = "usb")]
const USB_MANUAL_LIMIT_US: u64 = firmware::schedule::MANUAL_LIMIT_US;

#[cfg(feature = "usb")]
fn usb_refresh(
    schedule: &mut RefreshSchedule,
    safety: &mut SafetyGuard<safety_nvs::NvsSafetyStore>,
    manual: bool,
) {
    if safety.state() != SafetyState::Clean {
        log::warn!("USB refresh: BLOCKED by persistent runtime-fault latch");
        if !manual {
            schedule.defer_retry(rtc_time_us(), USB_REFRESH_INTERVAL_US);
        }
        return;
    }
    if let Err(error) = safety.begin_work() {
        safety_storage_fault(error);
    }
    let source = if manual {
        "manual refresh"
    } else {
        "scheduled refresh"
    };
    log::info!("{source}: starting REAL saved-frame refresh; Wi-Fi OFF; USB connected");
    let outcome = refresh_saved_frame(safety);
    if let Err(error) = outcome {
        log::error!(
            "{source}: FAILED: {error:?}; daily deadline unchanged; no automatic physical retry"
        );
        if let Err(power_error) = epd::Epd::disable_panel_power() {
            log::error!("{source}: emergency panel power cutoff failed: {power_error:?}");
        }
        if let Err(latch_error) = safety.latch() {
            safety_storage_fault(latch_error);
        }
        safety_storage_fault(error);
    }
    if let Err(error) = set_work_phase(safety, WorkPhase::SafetyCleanup) {
        safety_storage_fault(error);
    }
    if let Err(error) = safety.finish_work() {
        safety_storage_fault(error);
    }
    if let Err(error) = set_work_phase(safety, WorkPhase::Idle) {
        safety_storage_fault(error);
    }
    let completed_at_us = rtc_time_us();
    if let Ok(DisplayOutcome::Failed(error)) = outcome {
        schedule.defer_retry(completed_at_us, USB_REFRESH_INTERVAL_US);
        log::error!("{source}: display FAILED: {error:?}; cleanup complete, NVS clean; continuing; next scheduled attempt in 60 s");
        return;
    }
    schedule.refresh_succeeded(completed_at_us, manual);
    log::info!(
        "{}: SUCCESS; panel OFF; Wi-Fi OFF; NVS clean; daily deadline restarted; next timer in {} s, deadline={} us",
        source,
        USB_REFRESH_INTERVAL_US / 1_000_000,
        schedule.deadline_us(USB_REFRESH_INTERVAL_US)
    );
}

fn safety_storage_fault(error: impl std::fmt::Debug) -> ! {
    loop {
        log::error!("FIRMWARE SAFETY FAULT: {error:?}; work blocked; staying awake, no retry");
        FreeRtos::delay_ms(5_000);
    }
}

fn process_upload(
    received: upload_wifi::Received,
    deadline: Instant,
    progress: &mut dyn FnMut(firmware::upload::ProgressStage),
    safety: &mut SafetyGuard<safety_nvs::NvsSafetyStore>,
    schedule: &mut RefreshSchedule,
) -> Result<&'static str, upload_wifi::ProcessingError> {
    use firmware::upload::ProgressStage;
    use upload_wifi::ProcessingError;

    let saved = image_decode::save(
        received.buffer.reader(deadline),
        received.len,
        received.metadata,
        deadline,
        safety,
    );
    match saved {
        Ok(()) => {}
        Err(image_decode::PipelineError::Rejected(error)) => {
            return Err(ProcessingError::Retry(format!(
                "Image could not be decoded: {error}. Saved picture and display are unchanged. Choose another image and retry."
            )));
        }
        Err(image_decode::PipelineError::Fault(error)) => {
            return Err(ProcessingError::Fatal(format!(
                "Image save failed: {error}. Display was not refreshed."
            )));
        }
    }
    progress(ProgressStage::Processed);
    if Instant::now() >= deadline {
        return Err(ProcessingError::Fatal(
            "Image saved, but awake deadline expired before display refresh.".into(),
        ));
    }
    log::info!("display: verified save; AP ON; starting REAL panel refresh");
    progress(ProgressStage::Displaying);
    let outcome = refresh_saved_frame(safety);
    if let Err(error) = outcome {
        if let Err(power_error) = epd::Epd::disable_panel_power() {
            log::error!("display: emergency panel power cutoff failed: {power_error:?}");
            return Err(ProcessingError::Fatal(format!(
                    "Image saved, but display refresh failed: {error}. Emergency panel power cutoff also failed: {power_error}. Disconnect power."
                )));
        }
        return Err(ProcessingError::Fatal(format!(
                "Image saved, but display refresh failed: {error}. Panel power is off; no automatic retry."
            )));
    }
    #[cfg(feature = "usb")]
    if let Ok(DisplayOutcome::Failed(error)) = outcome {
        set_work_phase(safety, WorkPhase::SafetyCleanup).map_err(|error| {
            ProcessingError::Fatal(format!("Safety diagnostic cleanup failed: {error}"))
        })?;
        safety
            .finish_work()
            .map_err(|error| ProcessingError::Fatal(format!("Safety cleanup failed: {error:?}")))?;
        schedule.defer_retry(rtc_time_us(), USB_REFRESH_INTERVAL_US);
        return Err(ProcessingError::DisplayFailed(format!(
            "Image saved, but display refresh failed: {error}. Panel power is off. USB operation continues; next scheduled attempt in 60 s."
        )));
    }
    progress(ProgressStage::Displayed);
    set_work_phase(safety, WorkPhase::SafetyCleanup).map_err(|error| {
        ProcessingError::Fatal(format!("Safety diagnostic cleanup failed: {error}"))
    })?;
    safety.finish_work().map_err(|error| {
        ProcessingError::Fatal(format!(
            "Image processing completed but safety cleanup failed: {error:?}."
        ))
    })?;
    schedule.refresh_succeeded(rtc_time_us(), false);
    log::info!("display: SUCCESS; panel OFF; safety CLEAN; final result returning to browser");
    Ok("Success: image saved and display refresh completed. Panel power is off.")
}

#[cfg(feature = "usb")]
fn main() {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    epd::Epd::disable_panel_power().expect("gesture test failed to disable panel power");
    let peripherals = Peripherals::take().expect("peripherals already taken");
    let button = PinDriver::rtc_input(peripherals.pins.gpio2, Pull::Up)
        .expect("failed to configure KEY0/GPIO2 as an RTC input");
    let fault_button = PinDriver::rtc_input(peripherals.pins.gpio3, Pull::Up)
        .expect("failed to configure KEY1/GPIO3 as an input");
    let mut fault_switch =
        firmware::gesture::FaultSwitch::new(Instant::now(), fault_button.is_low());
    let mut simulated_fault = false;
    log::warn!("USB session reset reason: {:?}; development reboot resets USB runtime guard, not production recovery", ResetReason::get());
    if let Err(error) = safety_nvs::log_production_diagnostics() {
        log::error!("could not read production safety NVS snapshot: {error:?}");
    }
    let mut safety = match safety_nvs::open_usb_session(ResetReason::get() == ResetReason::Brownout)
    {
        Ok(safety) => safety,
        Err(error) => safety_storage_fault(error),
    };
    let mut upload_wifi = {
        if let Err(error) = safety.begin_work() {
            safety_storage_fault(error);
        }
        if let Err(error) = set_work_phase(&mut safety, WorkPhase::Setup) {
            safety_storage_fault(error);
        }
        if let Err(error) = psram::verify() {
            log::error!("PSRAM verification failed: {error:?}");
            if let Err(latch_error) = safety.latch() {
                safety_storage_fault(latch_error);
            }
            safety_storage_fault(error);
        }
        match upload_wifi::setup(peripherals.modem) {
            Ok(wifi) => wifi,
            Err(error) => {
                log::error!("upload Wi-Fi configuration failed: {error:?}");
                if let Err(latch_error) = safety.latch() {
                    safety_storage_fault(latch_error);
                }
                safety_storage_fault(error);
            }
        }
    };
    let mut battery_sensor =
        match battery::BatterySensor::new(peripherals.adc1, peripherals.pins.gpio8) {
            Ok(sensor) => Some(sensor),
            Err(error) => {
                log::warn!("battery ADC setup failed; website will show ?%: {error:?}");
                None
            }
        };
    if let Err(error) = set_work_phase(&mut safety, WorkPhase::SafetyCleanup) {
        safety_storage_fault(error);
    }
    if let Err(error) = safety.finish_work() {
        safety_storage_fault(error);
    }
    if let Err(error) = set_work_phase(&mut safety, WorkPhase::Idle) {
        safety_storage_fault(error);
    }

    log::warn!(
        "upload development mode: MOCK deep sleep; USB stays connected; panel OFF, Wi-Fi OFF"
    );
    log::warn!("USB mode: REAL upload/manual/scheduled display; simulated deep sleep; timer=60 s from completion; manual limit=1 h; production remains 24 h / 1 h");
    log::warn!("USB safety: simulated fault initially OFF; KEY1 toggles while idle; runtime safety namespace=hof_usb, state={:?}; production hof_safety untouched; KEY1 cannot clear real faults; battery estimate is display-only; USB power only", safety.state());
    let mut schedule = RefreshSchedule::new(rtc_time_us());
    let mut button_armed = false;
    let mut released_since = None;
    let mut next_status_us = 0;
    log::info!(
        "mock PrepareSleep: waiting for KEY0 release; initial timer in {} s",
        USB_REFRESH_INTERVAL_US / 1_000_000
    );
    loop {
        let now_us = rtc_time_us();
        if fault_switch.sample(Instant::now(), fault_button.is_low()) {
            simulated_fault = !simulated_fault;
            log::warn!("USB KEY1: simulated fault={simulated_fault}; real safety={:?}; production latch unchanged", safety.state());
        }
        if !simulated_fault && schedule.due(now_us, USB_REFRESH_INTERVAL_US) {
            log::info!("USB: TIMER wake; no button gesture required");
            usb_refresh(&mut schedule, &mut safety, false);
            continue;
        }

        let pressed = button.is_low();
        if !button_armed {
            if pressed {
                released_since = None;
            } else if let Some(released_at) = released_since {
                if Instant::now().duration_since(released_at) >= BUTTON_DEBOUNCE {
                    button_armed = true;
                    log::info!("mock DeepSleep: READY; press KEY0 to start the next gesture");
                }
            } else {
                released_since = Some(Instant::now());
            }
        } else if pressed {
            button_armed = false;
            released_since = None;
            let wake_at = Instant::now();
            let deadline_before = schedule.deadline_us(USB_REFRESH_INTERVAL_US);
            log::info!("mock Boot: KEY0 wake, press 1");
            let gesture = classify_gesture(&button, wake_at);
            log::info!(
                "KEY0 gesture test: {gesture:?} after {:.3} s; panel OFF, Wi-Fi OFF",
                wake_at.elapsed().as_secs_f32()
            );
            match gesture {
                Gesture::Upload | Gesture::ManualRefresh
                    if simulated_fault || safety.state() != SafetyState::Clean =>
                {
                    log::warn!("USB: work BLOCKED; simulated fault={simulated_fault}; runtime safety={:?}; no acknowledgement performed", safety.state());
                }
                Gesture::ManualRefresh
                    if schedule.manual_allowed(rtc_time_us(), USB_MANUAL_LIMIT_US) =>
                {
                    usb_refresh(&mut schedule, &mut safety, true);
                }
                Gesture::ManualRefresh => {
                    log::info!("mock ManualRefresh: RATE LIMITED; daily deadline unchanged={deadline_before} us");
                }
                Gesture::Upload => {
                    if let Err(error) = safety.begin_work() {
                        safety_storage_fault(error);
                    }
                    let battery_percentage = read_battery_percentage(battery_sensor.as_mut());
                    let result = upload_wifi::run(
                        &mut upload_wifi,
                        wake_at,
                        &mut safety,
                        &mut schedule,
                        battery_percentage,
                    );
                    if let Err(error) = result {
                        log::error!("UploadWindow failed: {error:?}");
                        if let Err(latch_error) = safety.latch() {
                            safety_storage_fault(latch_error);
                        }
                        safety_storage_fault(error);
                    }
                    if safety.state() == SafetyState::Working {
                        if let Err(error) = set_work_phase(&mut safety, WorkPhase::SafetyCleanup) {
                            safety_storage_fault(error);
                        }
                        if let Err(error) = safety.finish_work() {
                            safety_storage_fault(error);
                        }
                        if let Err(error) = set_work_phase(&mut safety, WorkPhase::Idle) {
                            safety_storage_fault(error);
                        }
                    }
                    log::info!(
                        "mock Upload: processing ended; panel OFF; daily deadline={} us",
                        schedule.deadline_us(USB_REFRESH_INTERVAL_US)
                    );
                }
                Gesture::Sleep => {
                    log::info!(
                        "mock Sleep: no refresh; daily deadline unchanged={deadline_before} us"
                    );
                }
            }
            log::info!("mock PrepareSleep: waiting for KEY0 release; timer stays active");
            continue;
        }

        if now_us >= next_status_us {
            log::info!(
                "mock deep sleep: daily in {:.1} s; manual allowed={}; button armed={}; runtime safety={:?}; simulated fault={}; USB connected",
                schedule
                    .deadline_us(USB_REFRESH_INTERVAL_US)
                    .saturating_sub(now_us) as f64
                    / 1_000_000.0,
                schedule.manual_allowed(now_us, USB_MANUAL_LIMIT_US) && safety.state() == SafetyState::Clean && !simulated_fault,
                button_armed,
                safety.state(),
                simulated_fault
            );
            next_status_us = now_us.saturating_add(5_000_000);
        }
        FreeRtos::delay_ms(BUTTON_POLL_MS);
    }
}

#[cfg(not(feature = "usb"))]
fn main() {
    let wake_at = Instant::now();

    // Required once so esp-idf-sys runtime patches link properly.
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take().expect("peripherals already taken");
    let button = PinDriver::rtc_input(peripherals.pins.gpio2, Pull::Up)
        .expect("failed to configure KEY0/GPIO2 as an RTC input");

    let reset_reason = ResetReason::get();
    let wake_reason = WakeupReason::get();
    if let Err(error) = epd::Epd::disable_panel_power() {
        safety_storage_fault(error);
    }
    let gesture = if reset_reason == ResetReason::DeepSleep && wake_reason == WakeupReason::Button {
        Some(classify_gesture(&button, wake_at))
    } else {
        None
    };
    let now_us = rtc_time_us();
    initialize_retained_state(now_us);
    let mut safety = match safety_nvs::open("hof_safety", reset_reason == ResetReason::Brownout) {
        Ok(safety) => safety,
        Err(error) => safety_storage_fault(error),
    };
    if safety.state() != SafetyState::Clean {
        log::error!("persistent safety latch set; refresh/upload blocked; battery recovery measurement is not implemented, so acknowledgement is unavailable");
        update_retained_state(|state| state.schedule.defer_retry(now_us, DAILY_INTERVAL_US));
    }
    log::info!("firmware booting; reset reason: {reset_reason:?}, wakeup reason: {wake_reason:?}");
    let interrupted_refresh = recover_interrupted_refresh(now_us, reset_reason);
    if interrupted_refresh {
        log::error!(
            "previous panel refresh was interrupted or reset was caused by brownout; \
             skipping refresh; persistent safety latch takes precedence over RTC cooldown"
        );
    }

    if gesture == Some(Gesture::Upload) {
        let state = retained_state();
        if safety.state() != SafetyState::Clean
            || state.schedule.cooling_down(rtc_time_us())
            || interrupted_refresh
        {
            log::warn!("upload suppressed by safety latch or interrupted-work recovery cooldown; calibrated battery acknowledgement remains unavailable");
            sleep_until_refresh(&button);
        }
        log::warn!("normal upload: REAL display and deep sleep; battery voltage guard is NOT qualified; stable bench power only");
        let mut battery_sensor =
            match battery::BatterySensor::new(peripherals.adc1, peripherals.pins.gpio8) {
                Ok(sensor) => Some(sensor),
                Err(error) => {
                    log::warn!("battery ADC setup failed; website will show ?%: {error:?}");
                    None
                }
            };
        let battery_percentage = read_battery_percentage(battery_sensor.as_mut());
        if let Err(error) = safety.begin_work() {
            safety_storage_fault(error);
        }
        update_retained_state(|state| state.refresh_in_progress = 1);
        let mut schedule = retained_state().schedule;
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            set_work_phase(&mut safety, WorkPhase::Setup)?;
            psram::verify()?;
            let mut wifi = upload_wifi::setup(peripherals.modem)?;
            upload_wifi::run(
                &mut wifi,
                wake_at,
                &mut safety,
                &mut schedule,
                battery_percentage,
            )
        })();
        match result {
            Ok(()) => {
                if safety.state() == SafetyState::Working {
                    if let Err(error) = set_work_phase(&mut safety, WorkPhase::SafetyCleanup) {
                        safety_storage_fault(error);
                    }
                    if let Err(error) = safety.finish_work() {
                        safety_storage_fault(error);
                    }
                }
                if let Err(error) = set_work_phase(&mut safety, WorkPhase::Idle) {
                    safety_storage_fault(error);
                }
                update_retained_state(|state| state.refresh_in_progress = 0);
            }
            Err(error) => {
                log::error!("normal upload failed: {error}; latching work without retry");
                if let Err(power_error) = epd::Epd::disable_panel_power() {
                    safety_storage_fault(power_error);
                }
                if let Err(latch_error) = safety.latch() {
                    safety_storage_fault(latch_error);
                }
                update_retained_state(|state| {
                    state.refresh_in_progress = 0;
                    state.schedule.defer_retry(rtc_time_us(), DAILY_INTERVAL_US);
                });
            }
        }
        sleep_until_refresh(&button);
    }

    let mut manual_refresh = false;
    let requested_refresh = match wake_reason {
        WakeupReason::Button if gesture.is_some() => {
            match gesture.expect("button gesture sampled") {
                Gesture::Upload => {
                    log::error!("upload gesture reached refresh dispatch unexpectedly; no refresh requested");
                    false
                }
                Gesture::ManualRefresh => {
                    let state = retained_state();
                    if state
                        .schedule
                        .manual_allowed(rtc_time_us(), MANUAL_LIMIT_US)
                    {
                        log::info!("triple press detected; refreshing the display");
                        manual_refresh = true;
                        true
                    } else {
                        log::info!("manual refresh rate-limited; returning to sleep");
                        false
                    }
                }
                Gesture::Sleep => {
                    log::info!("short or incomplete KEY0 gesture; returning to sleep");
                    false
                }
            }
        }
        WakeupReason::Timer => {
            log::info!("24-hour RTC timer wake; refreshing the display");
            true
        }
        reason => {
            log::info!("non-button cold/reset wake ({reason:?}); refreshing the display");
            true
        }
    };

    let cooling_down = retained_state().schedule.cooling_down(rtc_time_us());
    let should_refresh = requested_refresh
        && safety.state() == SafetyState::Clean
        && !cooling_down
        && !interrupted_refresh;
    if requested_refresh && !should_refresh {
        log::warn!("refresh suppressed by safety latch or interrupted-refresh recovery cooldown");
    }

    if should_refresh {
        if let Err(error) = safety.begin_work() {
            safety_storage_fault(error);
        }
        update_retained_state(|state| state.refresh_in_progress = 1);
        match refresh_saved_frame(&mut safety) {
            Ok(DisplayOutcome::Displayed) => {
                if let Err(error) = set_work_phase(&mut safety, WorkPhase::SafetyCleanup) {
                    safety_storage_fault(error);
                }
                if let Err(error) = safety.finish_work() {
                    safety_storage_fault(error);
                }
                if let Err(error) = set_work_phase(&mut safety, WorkPhase::Idle) {
                    safety_storage_fault(error);
                }
                let refreshed_at_us = rtc_time_us();
                update_retained_state(|state| {
                    state
                        .schedule
                        .refresh_succeeded(refreshed_at_us, manual_refresh);
                    state.refresh_in_progress = 0;
                });
            }
            Err(error) => {
                log::error!("display refresh failed: {error:?}");
                if let Err(error) = epd::Epd::disable_panel_power() {
                    safety_storage_fault(error);
                }
                if let Err(error) = safety.latch() {
                    safety_storage_fault(error);
                }
                let failed_at_us = rtc_time_us();
                update_retained_state(|state| {
                    state.refresh_in_progress = 0;
                    state
                        .schedule
                        .refresh_failed(failed_at_us, DAILY_INTERVAL_US);
                });
            }
        }
    }

    sleep_until_refresh(&button)
}
