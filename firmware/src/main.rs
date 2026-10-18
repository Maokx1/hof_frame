//! Display bring-up with SW7 gesture handling and scheduled deep sleep.

mod epd;
mod frame_flash;
#[cfg(feature = "debug-upload")]
mod image_decode;
#[cfg(feature = "debug-upload")]
mod psram;
mod safety_nvs;
#[cfg(feature = "debug-upload")]
mod upload_wifi;

use std::time::{Duration, Instant};

use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::gpio::Pull;
use esp_idf_svc::hal::gpio::{PinDriver, RtcInput};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::reset::ResetReason;
#[cfg(not(feature = "debug-upload"))]
use esp_idf_svc::hal::reset::WakeupReason;
#[cfg(not(feature = "debug-upload"))]
use esp_idf_svc::hal::sleep::{rtc::RtcWakeLevel, DeepSleep};
use esp_idf_svc::sys::EspError;

use firmware::gesture::{Gesture, GestureClassifier, BUTTON_DEBOUNCE};
#[cfg(feature = "debug-upload")]
use firmware::safety::SafetyGuard;
use firmware::safety::SafetyState;
#[cfg(not(feature = "debug-upload"))]
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

fn refresh_panel_with(color_at: impl Fn(usize, usize) -> u8) -> Result<(), EspError> {
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

    let power_off = epd.power_off();
    let result = match (refresh, power_off) {
        (Err(refresh_error), Err(power_off_error)) => {
            log::error!("panel power-off also failed: {power_off_error:?}");
            Err(refresh_error)
        }
        (Err(refresh_error), Ok(())) => Err(refresh_error),
        (Ok(()), Err(power_off_error)) => Err(power_off_error),
        (Ok(()), Ok(())) => Ok(()),
    };
    let cleanup = epd.close();
    match (result, cleanup) {
        (Err(error), Err(cleanup_error)) => {
            log::error!("panel SPI cleanup also failed: {cleanup_error:?}");
            Err(error)
        }
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn refresh_saved_frame() -> Result<(), EspError> {
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
    let percentage = if cfg!(feature = "debug-battery-preview") {
        log::warn!("battery overlay: SIMULATED 100% for size preview; not a battery measurement");
        Some(100)
    } else {
        log::info!("battery overlay: unknown (?%); calibrated measurement unavailable");
        None
    };
    let overlay = firmware::overlay::StatusOverlay::new(metadata, percentage).map_err(|error| {
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
        refresh_panel_with(|x, y| {
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
        refresh_panel_with(|x, y| {
            overlay
                .pixel(x, y)
                .unwrap_or_else(|| firmware::frame::default_pixel(x, y))
        })
    }
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

#[cfg(not(feature = "debug-upload"))]
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
            log::warn!("SW7 remained pressed for 60 s; disabling button wake until timer wake");
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
            .expect("failed to configure SW7 wake");
    }

    log::info!(
        "entering deep sleep; next refresh in {} s{}",
        remaining_us / 1_000_000,
        if button_released {
            " with SW7 wake enabled"
        } else {
            " with timer wake only"
        }
    );
    deep_sleep.enter()
}

#[cfg(feature = "debug-upload")]
const MOCK_REFRESH_INTERVAL_US: u64 = 120 * 1_000_000;
#[cfg(feature = "debug-upload")]
const MOCK_MANUAL_LIMIT_US: u64 = 30 * 1_000_000;

#[cfg(feature = "debug-upload")]
fn mock_refresh(
    schedule: &mut RefreshSchedule,
    safety: &mut SafetyGuard<safety_nvs::NvsSafetyStore>,
    manual: bool,
) {
    if safety.state() != SafetyState::Clean {
        log::warn!("mock Refresh: BLOCKED by persistent safety latch");
        if !manual {
            schedule.defer_retry(rtc_time_us(), MOCK_REFRESH_INTERVAL_US);
        }
        return;
    }
    if let Err(error) = safety.begin_work() {
        safety_storage_fault(error);
    }
    log::info!("mock safety: WORKING marker committed to NVS before work");
    let mut storage = match frame_flash::FlashFrames::open() {
        Ok(storage) => storage,
        Err(error) => safety_storage_fault(error),
    };
    match storage.load() {
        Ok(scan) => log::info!(
            "mock LoadFrame: selected={:?}; panel remains OFF",
            scan.current
        ),
        Err(error) => safety_storage_fault(error),
    }
    log::info!(
        "mock Refresh: {} started; panel OFF, Wi-Fi OFF",
        if manual { "manual" } else { "timer" }
    );
    FreeRtos::delay_ms(1_000);
    if let Err(error) = safety.finish_work() {
        safety_storage_fault(error);
    }
    let completed_at_us = rtc_time_us();
    schedule.refresh_succeeded(completed_at_us, manual);
    log::info!(
        "mock Refresh: SUCCESS; NVS clean; daily deadline restarted; next refresh in 120 s, deadline={} us",
        schedule.deadline_us(MOCK_REFRESH_INTERVAL_US)
    );
}

fn safety_storage_fault(error: impl std::fmt::Debug) -> ! {
    loop {
        log::error!("FIRMWARE SAFETY FAULT: {error:?}; work blocked; staying awake, no retry");
        FreeRtos::delay_ms(5_000);
    }
}

#[cfg(feature = "debug-upload")]
fn main() {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    epd::Epd::disable_panel_power().expect("gesture test failed to disable panel power");
    let peripherals = Peripherals::take().expect("peripherals already taken");
    let button = PinDriver::rtc_input(peripherals.pins.gpio13, Pull::Up)
        .expect("failed to configure SW7/GPIO13 as an RTC input");
    let mut safety = match safety_nvs::open("hof_mock", ResetReason::get() == ResetReason::Brownout)
    {
        Ok(safety) => safety,
        Err(error) => safety_storage_fault(error),
    };
    let mut upload_wifi = {
        if let Err(error) = psram::verify() {
            log::error!("PSRAM verification failed: {error:?}");
            safety_storage_fault(error);
        }
        let setup = (|| -> Result<_, EspError> {
            use esp_idf_svc::ipv4;
            use esp_idf_svc::netif::{EspNetif, NetifConfiguration, NetifStack};
            use esp_idf_svc::wifi::{EspWifi, WifiDriver};

            let event_loop = esp_idf_svc::eventloop::EspSystemEventLoop::take()?;
            let ap_configuration = NetifConfiguration {
                ip_configuration: Some(ipv4::Configuration::Router(ipv4::RouterConfiguration {
                    subnet: ipv4::Subnet {
                        gateway: upload_wifi::AP_ADDRESS,
                        mask: "24".parse().expect("valid fixed subnet mask"),
                    },
                    dhcp_enabled: true,
                    dns: None,
                    secondary_dns: None,
                })),
                ..NetifConfiguration::wifi_default_router()
            };
            let wifi = EspWifi::wrap_all(
                WifiDriver::new(peripherals.modem, event_loop.clone(), None)?,
                EspNetif::new(NetifStack::Sta)?,
                EspNetif::new_with_conf(&ap_configuration)?,
            )?;
            let mut wifi = esp_idf_svc::wifi::BlockingWifi::wrap(wifi, event_loop)?;
            upload_wifi::configure(&mut wifi)?;
            Ok(wifi)
        })();
        match setup {
            Ok(wifi) => wifi,
            Err(error) => {
                log::error!("upload Wi-Fi configuration failed: {error:?}");
                safety_storage_fault(error);
            }
        }
    };

    log::warn!(
        "upload development mode: MOCK deep sleep; USB stays connected; panel OFF, Wi-Fi OFF"
    );
    #[cfg(feature = "debug-upload-display")]
    log::warn!("display test ENABLED: successful upload/save triggers REAL panel refresh; timer/manual refresh remain mocked; battery guard SIMULATED; use bench power");
    log::warn!("mock safety: NVS namespace=hof_mock, state={:?}; acknowledgement uses SIMULATED safe recovery voltage, never a battery measurement", safety.state());
    log::warn!("mock schedule: daily interval=120 s, manual limit=30 s, refresh duration=1 s; production remains 24 h / 1 h");
    let mut schedule = RefreshSchedule::new(rtc_time_us());
    let mut button_armed = false;
    let mut released_since = None;
    let mut next_status_us = 0;
    log::info!("mock PrepareSleep: waiting for SW7 release; initial timer in 120 s");
    loop {
        let now_us = rtc_time_us();
        if schedule.due(now_us, MOCK_REFRESH_INTERVAL_US) {
            log::info!("mock Boot: TIMER wake; no button gesture required");
            mock_refresh(&mut schedule, &mut safety, false);
            continue;
        }

        let pressed = button.is_low();
        if !button_armed {
            if pressed {
                released_since = None;
            } else if let Some(released_at) = released_since {
                if Instant::now().duration_since(released_at) >= BUTTON_DEBOUNCE {
                    button_armed = true;
                    log::info!("mock DeepSleep: READY; press SW7 to start the next gesture");
                }
            } else {
                released_since = Some(Instant::now());
            }
        } else if pressed {
            button_armed = false;
            released_since = None;
            let wake_at = Instant::now();
            let deadline_before = schedule.deadline_us(MOCK_REFRESH_INTERVAL_US);
            log::info!("mock Boot: SW7 wake, press 1");
            let gesture = classify_gesture(&button, wake_at);
            log::info!(
                "SW7 gesture test: {gesture:?} after {:.3} s; panel OFF, Wi-Fi OFF",
                wake_at.elapsed().as_secs_f32()
            );
            match gesture {
                Gesture::Upload if safety.state() == SafetyState::Latched => {
                    if let Err(error) = safety.acknowledge(true, true) {
                        safety_storage_fault(error);
                    }
                    log::warn!("mock safety: acknowledgement committed; CLEAN; recovery voltage SIMULATED safe; no upload/refresh; daily deadline unchanged={deadline_before} us");
                }
                Gesture::ManualRefresh
                    if schedule.manual_allowed(rtc_time_us(), MOCK_MANUAL_LIMIT_US) =>
                {
                    mock_refresh(&mut schedule, &mut safety, true);
                }
                Gesture::ManualRefresh => {
                    log::info!("mock ManualRefresh: RATE LIMITED; daily deadline unchanged={deadline_before} us");
                }
                Gesture::Upload => {
                    if let Err(error) = safety.begin_work() {
                        safety_storage_fault(error);
                    }
                    let received = match upload_wifi::run(&mut upload_wifi, wake_at) {
                        Ok(received) => received,
                        Err(error) => {
                            log::error!("UploadWindow failed: {error:?}");
                            if let Err(latch_error) = safety.latch() {
                                safety_storage_fault(latch_error);
                            }
                            safety_storage_fault(error);
                        }
                    };
                    #[cfg(feature = "debug-upload-display")]
                    let mut displayed = false;
                    if let Some(mut received) = received {
                        let saved = image_decode::save(
                            &received.buffer.bytes_mut()[..received.len],
                            received.metadata,
                            wake_at + firmware::upload::AWAKE_TIMEOUT,
                        );
                        drop(received);
                        let saved = match saved {
                            Ok(()) => true,
                            Err(error) => {
                                match error {
                                    image_decode::PipelineError::Rejected(error) => {
                                        log::warn!("Upload rejected during decode: {error}; saved frame unchanged; panel OFF");
                                    }
                                    image_decode::PipelineError::Fault(error) => {
                                        log::error!("Upload pipeline fault: {error}; panel OFF");
                                        if let Err(latch_error) = safety.latch() {
                                            safety_storage_fault(latch_error);
                                        }
                                        safety_storage_fault(error);
                                    }
                                }
                                false
                            }
                        };
                        #[cfg(not(feature = "debug-upload-display"))]
                        let _ = saved;
                        #[cfg(feature = "debug-upload-display")]
                        if saved {
                            if wake_at.elapsed() >= firmware::upload::AWAKE_TIMEOUT {
                                log::error!("display test: awake deadline expired before refresh; saved frame retained; panel OFF");
                                if let Err(latch_error) = safety.latch() {
                                    safety_storage_fault(latch_error);
                                }
                                safety_storage_fault("display awake deadline expired");
                            }
                            log::info!("display test: verified save; Wi-Fi OFF; starting REAL panel refresh; USB stays connected");
                            if let Err(error) = refresh_saved_frame() {
                                log::error!("display test: refresh failed: {error:?}; saved frame retained; no automatic retry");
                                if let Err(power_error) = epd::Epd::disable_panel_power() {
                                    log::error!("display test: emergency power cutoff failed: {power_error:?}");
                                }
                                if let Err(latch_error) = safety.latch() {
                                    safety_storage_fault(latch_error);
                                }
                                safety_storage_fault(error);
                            }
                            displayed = true;
                        }
                    }
                    if let Err(error) = safety.finish_work() {
                        safety_storage_fault(error);
                    }
                    #[cfg(feature = "debug-upload-display")]
                    if displayed {
                        schedule.refresh_succeeded(rtc_time_us(), false);
                        log::info!("display test: SUCCESS; panel OFF; safety CLEAN; daily deadline restarted; USB connected");
                    }
                    log::info!(
                        "mock Upload: processing ended; panel OFF; daily deadline={} us",
                        schedule.deadline_us(MOCK_REFRESH_INTERVAL_US)
                    );
                }
                Gesture::Sleep => {
                    log::info!(
                        "mock Sleep: no refresh; daily deadline unchanged={deadline_before} us"
                    );
                }
            }
            log::info!("mock PrepareSleep: waiting for SW7 release; timer stays active");
            continue;
        }

        if now_us >= next_status_us {
            log::info!(
                "mock sleep: daily in {:.1} s; manual allowed={}; button armed={}; safety={:?}; USB connected",
                schedule
                    .deadline_us(MOCK_REFRESH_INTERVAL_US)
                    .saturating_sub(now_us) as f64
                    / 1_000_000.0,
                schedule.manual_allowed(now_us, MOCK_MANUAL_LIMIT_US) && safety.state() == SafetyState::Clean,
                button_armed,
                safety.state()
            );
            next_status_us = now_us.saturating_add(5_000_000);
        }
        FreeRtos::delay_ms(BUTTON_POLL_MS);
    }
}

#[cfg(not(feature = "debug-upload"))]
fn main() {
    let wake_at = Instant::now();

    // Required once so esp-idf-sys runtime patches link properly.
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take().expect("peripherals already taken");
    let button = PinDriver::rtc_input(peripherals.pins.gpio13, Pull::Up)
        .expect("failed to configure SW7/GPIO13 as an RTC input");

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

    let mut manual_refresh = false;
    let requested_refresh = match wake_reason {
        WakeupReason::Button if gesture.is_some() => match gesture.expect("button gesture sampled")
        {
            Gesture::Upload => {
                log::warn!("five-second SW7 hold detected; upload and calibrated battery recovery acknowledgement are not implemented");
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
                log::info!("short or incomplete SW7 gesture; returning to sleep");
                false
            }
        },
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
        match refresh_saved_frame() {
            Ok(()) => {
                if let Err(error) = safety.finish_work() {
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
