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

use std::thread::sleep;
use std::time::{Duration, Instant};

use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::gpio::Pull;
use esp_idf_svc::hal::gpio::{PinDriver, RtcInput};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::reset::ResetReason;
#[cfg(any(not(feature = "debug-gestures"), feature = "debug-button-sleep"))]
use esp_idf_svc::hal::reset::WakeupReason;
use esp_idf_svc::hal::sleep::{rtc::RtcWakeLevel, DeepSleep};
use esp_idf_svc::sys::EspError;

use epd::{BLACK, WHITE};
use firmware::gesture::{Gesture, GestureClassifier, BUTTON_DEBOUNCE};
#[cfg(feature = "debug-gestures")]
use firmware::safety::SafetyGuard;
#[cfg(any(not(feature = "debug-button-sleep"), feature = "debug-gestures"))]
use firmware::safety::SafetyState;
#[cfg(not(any(feature = "debug-button-sleep", feature = "debug-gestures")))]
use firmware::schedule::MANUAL_LIMIT_US;
use firmware::schedule::{RefreshSchedule, DAILY_INTERVAL_US};

const BUTTON_POLL_MS: u32 = 5;
const BUTTON_RELEASE_TIMEOUT: Duration = Duration::from_secs(60);

/// When true, runs `CLEAR_CYCLES` black/white full-screen refreshes instead of
/// the picture, to clear ghosting left by an interrupted refresh.
const CLEAR_TO_WHITE: bool = false;
const CLEAR_CYCLES: usize = 3;
/// Rest between refreshes; back-to-back Spectra 6 refreshes stress the panel.
const CLEAR_PAUSE: Duration = Duration::from_secs(180);

const PICTURE: &[u8] = include_bytes!("../assets/sunflower.bin");
const PICTURE_WIDTH: usize = epd::WIDTH;
const PICTURE_HEIGHT: usize = epd::HEIGHT;
const PICTURE_LEFT: usize = (epd::WIDTH - PICTURE_WIDTH) / 2;
const PICTURE_TOP: usize = (epd::HEIGHT - PICTURE_HEIGHT) / 2;

const _: () = assert!(PICTURE.len() == PICTURE_WIDTH * PICTURE_HEIGHT / 2);

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

fn color_at(x: usize, y: usize) -> u8 {
    if x < PICTURE_LEFT
        || x >= PICTURE_LEFT + PICTURE_WIDTH
        || y < PICTURE_TOP
        || y >= PICTURE_TOP + PICTURE_HEIGHT
    {
        return WHITE;
    }
    let i = (y - PICTURE_TOP) * PICTURE_WIDTH + (x - PICTURE_LEFT);
    let byte = PICTURE[i / 2];
    if i % 2 == 0 {
        byte >> 4
    } else {
        byte & 0x0F
    }
}

fn refresh_panel() -> Result<(), EspError> {
    refresh_panel_with(color_at)
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

        if CLEAR_TO_WHITE {
            for cycle in 1..=CLEAR_CYCLES {
                for (name, color) in [("black", BLACK), ("white", WHITE)] {
                    log::info!("clear cycle {cycle}/{CLEAR_CYCLES}: {name}");
                    epd.init()?;
                    let start = Instant::now();
                    epd.show(|_, _| color)?;
                    log::info!("refresh done in {:.1} s", start.elapsed().as_secs_f32());
                    if cycle < CLEAR_CYCLES || color != WHITE {
                        log::info!("resting {} s", CLEAR_PAUSE.as_secs());
                        sleep(CLEAR_PAUSE);
                    }
                }
            }
        } else {
            log::info!("streaming image pixels");
            let start = Instant::now();
            epd.show(color_at)?;
            log::info!(
                "panel show sequence completed in {:.1} s",
                start.elapsed().as_secs_f32()
            );
        }
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

#[cfg(not(feature = "debug-button-sleep"))]
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

#[cfg(feature = "debug-button-sleep")]
fn test_button_sleep(button: &PinDriver<'_, RtcInput>) -> Result<(), EspError> {
    let started = Instant::now();
    let mut released_since = None;
    loop {
        let now = Instant::now();
        if button.is_low() {
            released_since = None;
        } else if let Some(released_at) = released_since {
            if now.duration_since(released_at) >= BUTTON_DEBOUNCE {
                break;
            }
        } else {
            released_since = Some(now);
        }
        if started.elapsed() >= BUTTON_RELEASE_TIMEOUT {
            log::error!("SW7 test: button did not release in 60 s; staying awake");
            record_button_test_abort(ButtonTestAbort::ReleaseTimeout, None);
            return Err(EspError::from(esp_idf_svc::sys::ESP_ERR_TIMEOUT).unwrap());
        }
        FreeRtos::delay_ms(BUTTON_POLL_MS);
    }
    // The HAL calls low-level wake AllLow; with only GPIO13 selected this is low wake.
    let deep_sleep = DeepSleep::new()?.wakeup_on_rtc(button, RtcWakeLevel::AllLow)?;
    set_button_test_phase(BUTTON_TEST_WAITING);
    log::info!("SW7 test: panel off; entering deep sleep with GPIO13 wake only; press SW7 once");
    deep_sleep.enter()
}

#[cfg(feature = "debug-button-sleep")]
const BUTTON_TEST_UNSTARTED: u32 = 0;
#[cfg(feature = "debug-button-sleep")]
const BUTTON_TEST_WAITING: u32 = 1;
#[cfg(feature = "debug-button-sleep")]
const BUTTON_TEST_REFRESHING: u32 = 2;
#[cfg(feature = "debug-button-sleep")]
const BUTTON_TEST_ABORTED: u32 = 4;

#[cfg(feature = "debug-button-sleep")]
#[link_section = ".rtc.data"]
static mut BUTTON_TEST_PHASE: u32 = BUTTON_TEST_UNSTARTED;

#[cfg(feature = "debug-button-sleep")]
#[derive(Clone, Copy, Debug)]
enum ButtonTestAbort {
    None,
    PanelPowerOff,
    Setup,
    ReleaseTimeout,
    UnexpectedWakeMask,
    Refresh,
    UnexpectedReset,
    InterruptedRefresh,
    InvalidPhase,
}

#[cfg(feature = "debug-button-sleep")]
#[derive(Clone, Copy, Debug)]
struct ButtonTestDiagnostic {
    wake_mask: u64,
    abort: ButtonTestAbort,
    error_code: i32,
}

#[cfg(feature = "debug-button-sleep")]
#[link_section = ".rtc.data"]
static mut BUTTON_TEST_DIAGNOSTIC: ButtonTestDiagnostic = ButtonTestDiagnostic {
    wake_mask: 0,
    abort: ButtonTestAbort::None,
    error_code: 0,
};

#[cfg(feature = "debug-button-sleep")]
fn button_test_diagnostic() -> ButtonTestDiagnostic {
    unsafe { std::ptr::addr_of!(BUTTON_TEST_DIAGNOSTIC).read_volatile() }
}

#[cfg(feature = "debug-button-sleep")]
fn update_button_test_diagnostic(update: impl FnOnce(&mut ButtonTestDiagnostic)) {
    unsafe {
        let state = std::ptr::addr_of_mut!(BUTTON_TEST_DIAGNOSTIC);
        let mut value = state.read_volatile();
        update(&mut value);
        state.write_volatile(value);
    }
}

#[cfg(feature = "debug-button-sleep")]
fn record_button_test_abort(reason: ButtonTestAbort, error: Option<EspError>) {
    set_button_test_phase(BUTTON_TEST_ABORTED);
    update_button_test_diagnostic(|diagnostic| {
        diagnostic.abort = reason;
        diagnostic.error_code = error.map_or(0, |error| error.code());
    });
}

#[cfg(feature = "debug-button-sleep")]
fn button_test_phase() -> u32 {
    unsafe { std::ptr::addr_of!(BUTTON_TEST_PHASE).read_volatile() }
}

#[cfg(feature = "debug-button-sleep")]
fn set_button_test_phase(phase: u32) {
    unsafe {
        std::ptr::addr_of_mut!(BUTTON_TEST_PHASE).write_volatile(phase);
    }
}

#[cfg(feature = "debug-button-sleep")]
fn idle_awake() -> ! {
    loop {
        log::info!(
            "SW7 test idle: reset={:?}, wake={:?}, phase={}, diagnostic={:?}; no further refresh",
            ResetReason::get(),
            WakeupReason::get(),
            button_test_phase(),
            button_test_diagnostic()
        );
        sleep(Duration::from_secs(10));
    }
}

#[cfg(all(feature = "debug-button-sleep", not(feature = "debug-gestures")))]
fn main() {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let reset_reason = ResetReason::get();
    let wake_reason = WakeupReason::get();
    let wake_mask = unsafe { esp_idf_svc::sys::esp_sleep_get_ext1_wakeup_status() };
    update_button_test_diagnostic(|diagnostic| diagnostic.wake_mask = wake_mask);
    log::warn!(
        "SW7 cycle test boot; reset: {reset_reason:?}, wake: {wake_reason:?}, phase: {}",
        button_test_phase()
    );

    match button_test_phase() {
        BUTTON_TEST_UNSTARTED => {
            if let Err(error) = epd::Epd::disable_panel_power() {
                log::error!("failed to disable panel power before waiting for SW7: {error:?}");
                record_button_test_abort(ButtonTestAbort::PanelPowerOff, Some(error));
                idle_awake();
            }
            log::info!("startup refresh disabled; leaving image unchanged and waiting for SW7");
            let setup = Peripherals::take()
                .and_then(|peripherals| PinDriver::rtc_input(peripherals.pins.gpio13, Pull::Up))
                .and_then(|button| test_button_sleep(&button));
            if let Err(error) = setup {
                log::error!("SW7 sleep setup failed: {error:?}");
                if button_test_phase() != BUTTON_TEST_ABORTED {
                    record_button_test_abort(ButtonTestAbort::Setup, Some(error));
                }
            }
        }
        BUTTON_TEST_WAITING
            if reset_reason == ResetReason::DeepSleep && wake_reason == WakeupReason::Button =>
        {
            log::warn!(
                "SW7 diagnostic: EXT1 wake mask {wake_mask:#018x}; expected GPIO13 = 0x2000"
            );
            if let Err(error) = epd::Epd::disable_panel_power() {
                log::error!("failed to cut panel power on diagnostic wake: {error:?}");
                record_button_test_abort(ButtonTestAbort::PanelPowerOff, Some(error));
                idle_awake();
            }
            if wake_mask != 1_u64 << 13 {
                record_button_test_abort(ButtonTestAbort::UnexpectedWakeMask, None);
                log::error!("unexpected EXT1 mask; panel refresh suppressed");
                idle_awake();
            }
            match Peripherals::take()
                .and_then(|peripherals| PinDriver::rtc_input(peripherals.pins.gpio13, Pull::Up))
            {
                Ok(button) => {
                    log::info!(
                        "confirmed GPIO13 wake; current SW7 level={}",
                        button.is_high()
                    );
                    set_button_test_phase(BUTTON_TEST_REFRESHING);
                    match refresh_panel() {
                        Ok(()) => {
                            log::info!("SW7 wake refresh completed; waiting for release before sleeping again");
                            if let Err(error) = test_button_sleep(&button) {
                                if button_test_phase() != BUTTON_TEST_ABORTED {
                                    record_button_test_abort(ButtonTestAbort::Setup, Some(error));
                                }
                                log::error!("SW7 sleep setup failed after refresh: {error:?}; staying awake");
                            }
                        }
                        Err(error) => {
                            record_button_test_abort(ButtonTestAbort::Refresh, Some(error));
                            log::error!("SW7 wake refresh failed: {error:?}; no retry");
                        }
                    }
                }
                Err(error) => {
                    record_button_test_abort(ButtonTestAbort::Setup, Some(error));
                    log::error!("failed to configure GPIO13 after SW7 wake: {error:?}");
                }
            }
        }
        BUTTON_TEST_WAITING => {
            record_button_test_abort(ButtonTestAbort::UnexpectedReset, None);
            log::error!(
                "unexpected reset/wake while waiting for SW7 ({reset_reason:?}/{wake_reason:?}); \
                 aborting without refreshing"
            );
        }
        BUTTON_TEST_REFRESHING => {
            record_button_test_abort(ButtonTestAbort::InterruptedRefresh, None);
            log::error!(
                "reset interrupted the SW7 wake refresh ({reset_reason:?}/{wake_reason:?}); \
                 aborting without automatic retry"
            );
        }
        BUTTON_TEST_ABORTED => log::error!("SW7 cycle test aborted; no automatic refresh"),
        phase => {
            record_button_test_abort(ButtonTestAbort::InvalidPhase, None);
            log::error!("invalid SW7 test phase {phase}; aborting without refresh");
        }
    }

    idle_awake()
}

#[cfg(feature = "debug-gestures")]
const MOCK_REFRESH_INTERVAL_US: u64 = 120 * 1_000_000;
#[cfg(feature = "debug-gestures")]
const MOCK_MANUAL_LIMIT_US: u64 = 30 * 1_000_000;

#[cfg(feature = "debug-gestures")]
fn mock_refresh(
    schedule: &mut RefreshSchedule,
    safety: &mut SafetyGuard<safety_nvs::NvsSafetyStore>,
    manual: bool,
    inject_interruption: &mut bool,
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
    if *inject_interruption {
        *inject_interruption = false;
        log::error!("mock fault injection: interrupted work; reopening committed safety record without resetting USB");
        if let Err(error) = safety.recover_interrupted_work() {
            safety_storage_fault(error);
        }
        schedule.defer_retry(rtc_time_us(), MOCK_REFRESH_INTERVAL_US);
        log::warn!("mock safety: {:?}; refresh not completed; hold SW7 for 5 s to acknowledge with SIMULATED safe recovery voltage", safety.state());
        return;
    }
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

#[cfg(feature = "debug-frame-storage")]
fn test_frame_storage(safety: &mut SafetyGuard<safety_nvs::NvsSafetyStore>) {
    if safety.state() != SafetyState::Clean {
        log::warn!("frame storage test BLOCKED by safety latch; hold to acknowledge, then reflash this feature");
        return;
    }
    let mut storage = match frame_flash::FlashFrames::open() {
        Ok(storage) => storage,
        Err(error) => safety_storage_fault(error),
    };
    let before = match storage.load() {
        Ok(scan) => scan,
        Err(error) => safety_storage_fault(error),
    };
    log::warn!("frame storage test: committing bundled native portrait fixture to inactive slot; old={:?}; panel OFF", before.current);
    if let Err(error) = safety.begin_work() {
        safety_storage_fault(error);
    }
    let committed = match firmware::frame::commit(
        &mut storage,
        PICTURE,
        firmware::frame::Metadata::default(),
    ) {
        Ok(record) => record,
        Err(error) => safety_storage_fault(error),
    };
    let after = match storage.load() {
        Ok(scan) => scan,
        Err(error) => safety_storage_fault(error),
    };
    if after.current != Some(committed) {
        safety_storage_fault("frame commit/reload mismatch");
    }
    match storage.map(committed) {
        Ok(mapped) => {
            if mapped.bytes() != PICTURE {
                safety_storage_fault("mapped frame differs from fixture");
            }
        }
        Err(error) => safety_storage_fault(error),
    }
    if let Err(error) = safety.finish_work() {
        safety_storage_fault(error);
    }
    log::info!("frame storage test: PASS; committed/reloaded/mapped 960000 bytes; {committed:?}; NVS clean; panel OFF");
}

#[cfg(feature = "debug-gestures")]
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
    let mut inject_interruption = cfg!(feature = "debug-interrupted-work");
    #[cfg(feature = "debug-upload")]
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
    #[cfg(feature = "debug-frame-storage")]
    test_frame_storage(&mut safety);

    log::warn!("SW7 gesture test: MOCK deep sleep; USB stays connected; panel OFF, Wi-Fi OFF");
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
            mock_refresh(&mut schedule, &mut safety, false, &mut inject_interruption);
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
                    mock_refresh(&mut schedule, &mut safety, true, &mut inject_interruption);
                }
                Gesture::ManualRefresh => {
                    log::info!("mock ManualRefresh: RATE LIMITED; daily deadline unchanged={deadline_before} us");
                }
                Gesture::Upload => {
                    #[cfg(feature = "debug-upload")]
                    {
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
                                            log::error!(
                                                "Upload pipeline fault: {error}; panel OFF"
                                            );
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
                    #[cfg(not(feature = "debug-upload"))]
                    log::info!("mock Upload: no image received; daily deadline unchanged={deadline_before} us");
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

#[cfg(not(any(feature = "debug-button-sleep", feature = "debug-gestures")))]
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
