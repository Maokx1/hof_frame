//! Display bring-up with SW7 gesture handling and scheduled deep sleep.

mod epd;

use std::thread::sleep;
use std::time::{Duration, Instant};

use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::gpio::Pull;
use esp_idf_svc::hal::gpio::{PinDriver, RtcInput};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::reset::ResetReason;
use esp_idf_svc::hal::reset::WakeupReason;
use esp_idf_svc::hal::sleep::{rtc::RtcWakeLevel, DeepSleep};
use esp_idf_svc::sys::EspError;

use epd::{BLACK, WHITE};

const BUTTON_DEBOUNCE: Duration = Duration::from_millis(50);
const BUTTON_POLL_MS: u32 = 5;
const GESTURE_WINDOW: Duration = Duration::from_secs(3);
const LONG_HOLD: Duration = Duration::from_secs(5);
const BUTTON_RELEASE_TIMEOUT: Duration = Duration::from_secs(60);
const REFRESH_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const MANUAL_REFRESH_LIMIT: Duration = Duration::from_secs(60 * 60);

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

const RETAINED_MAGIC: u32 = 0x484F_4631;
const US_PER_DAY: u64 = REFRESH_INTERVAL.as_micros() as u64;
const US_PER_HOUR: u64 = MANUAL_REFRESH_LIMIT.as_micros() as u64;

#[repr(C)]
#[derive(Clone, Copy)]
struct RetainedState {
    magic: u32,
    last_refresh_us: u64,
    last_manual_refresh_us: u64,
    retry_after_us: u64,
    refresh_in_progress: u32,
}

#[link_section = ".rtc.data"]
static mut RETAINED_STATE: RetainedState = RetainedState {
    magic: 0,
    last_refresh_us: 0,
    last_manual_refresh_us: 0,
    retry_after_us: 0,
    refresh_in_progress: 0,
};

extern "C" {
    fn esp_rtc_get_time_us() -> i64;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Gesture {
    Sleep,
    Upload,
    ManualRefresh,
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
                last_refresh_us: now_us,
                last_manual_refresh_us: 0,
                retry_after_us: 0,
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
            state.retry_after_us = now_us.saturating_add(US_PER_DAY);
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
    log::info!("creating panel interface; panel load switch will turn on");
    let epd = epd::Epd::new()?;
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
    match (refresh, power_off) {
        (Err(refresh_error), Err(power_off_error)) => {
            log::error!("panel power-off also failed: {power_off_error:?}");
            Err(refresh_error)
        }
        (Err(refresh_error), Ok(())) => Err(refresh_error),
        (Ok(()), Err(power_off_error)) => Err(power_off_error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn classify_gesture(button: &PinDriver<'_, RtcInput>, wake_at: Instant) -> Gesture {
    let mut presses = 1;
    let mut stable_pressed = button.is_low();
    let mut candidate_pressed = stable_pressed;
    let mut candidate_since = Instant::now();
    let mut waking_press_held = stable_pressed;
    let gesture_deadline = wake_at + GESTURE_WINDOW;
    let hold_deadline = wake_at + LONG_HOLD;

    loop {
        let now = Instant::now();
        let raw_pressed = button.is_low();
        if raw_pressed != candidate_pressed {
            candidate_pressed = raw_pressed;
            candidate_since = now;
        } else if candidate_pressed != stable_pressed
            && now.duration_since(candidate_since) >= BUTTON_DEBOUNCE
        {
            stable_pressed = candidate_pressed;
            if stable_pressed {
                presses += 1;
                if presses == 3 && now <= gesture_deadline {
                    return Gesture::ManualRefresh;
                }
            } else {
                waking_press_held = false;
            }
        }

        if waking_press_held && stable_pressed && now >= hold_deadline {
            return Gesture::Upload;
        }
        if now >= gesture_deadline && !(waking_press_held && stable_pressed) {
            return Gesture::Sleep;
        }
        FreeRtos::delay_ms(BUTTON_POLL_MS);
    }
}

fn manual_refresh_allowed(now_us: u64, last_manual_refresh_us: u64) -> bool {
    last_manual_refresh_us == 0 || now_us.saturating_sub(last_manual_refresh_us) >= US_PER_HOUR
}

#[cfg(not(feature = "debug-button-sleep"))]
fn refresh_deadline_us(state: RetainedState) -> u64 {
    state
        .last_refresh_us
        .saturating_add(US_PER_DAY)
        .max(state.retry_after_us)
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
    let deadline_us = refresh_deadline_us(retained_state());
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
            "SW7 test idle: reset={:?}, wake={:?}, EXT1={:#018x}, phase={}; no further refresh",
            ResetReason::get(),
            WakeupReason::get(),
            unsafe { esp_idf_svc::sys::esp_sleep_get_ext1_wakeup_status() },
            button_test_phase()
        );
        sleep(Duration::from_secs(10));
    }
}

#[cfg(feature = "debug-button-sleep")]
fn main() {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let reset_reason = ResetReason::get();
    let wake_reason = WakeupReason::get();
    log::warn!(
        "SW7 cycle test boot; reset: {reset_reason:?}, wake: {wake_reason:?}, phase: {}",
        button_test_phase()
    );

    match button_test_phase() {
        BUTTON_TEST_UNSTARTED => {
            if let Err(error) = epd::Epd::disable_panel_power() {
                log::error!("failed to disable panel power before waiting for SW7: {error:?}");
                set_button_test_phase(BUTTON_TEST_ABORTED);
                idle_awake();
            }
            log::info!("startup refresh disabled; leaving image unchanged and waiting for SW7");
            let setup = Peripherals::take()
                .and_then(|peripherals| PinDriver::rtc_input(peripherals.pins.gpio13, Pull::Up))
                .and_then(|button| test_button_sleep(&button));
            if let Err(error) = setup {
                log::error!("SW7 sleep setup failed: {error:?}");
                set_button_test_phase(BUTTON_TEST_ABORTED);
            }
        }
        BUTTON_TEST_WAITING
            if reset_reason == ResetReason::DeepSleep && wake_reason == WakeupReason::Button =>
        {
            let wake_mask = unsafe { esp_idf_svc::sys::esp_sleep_get_ext1_wakeup_status() };
            log::warn!(
                "SW7 diagnostic: EXT1 wake mask {wake_mask:#018x}; expected GPIO13 = 0x2000"
            );
            if let Err(error) = epd::Epd::disable_panel_power() {
                log::error!("failed to cut panel power on diagnostic wake: {error:?}");
                set_button_test_phase(BUTTON_TEST_ABORTED);
                idle_awake();
            }
            if wake_mask != 1_u64 << 13 {
                set_button_test_phase(BUTTON_TEST_ABORTED);
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
                                set_button_test_phase(BUTTON_TEST_ABORTED);
                                log::error!("SW7 sleep setup failed after refresh: {error:?}; staying awake");
                            }
                        }
                        Err(error) => {
                            set_button_test_phase(BUTTON_TEST_ABORTED);
                            log::error!("SW7 wake refresh failed: {error:?}; no retry");
                        }
                    }
                }
                Err(error) => {
                    set_button_test_phase(BUTTON_TEST_ABORTED);
                    log::error!("failed to configure GPIO13 after SW7 wake: {error:?}");
                }
            }
        }
        BUTTON_TEST_WAITING => {
            set_button_test_phase(BUTTON_TEST_ABORTED);
            log::error!(
                "unexpected reset/wake while waiting for SW7 ({reset_reason:?}/{wake_reason:?}); \
                 aborting without refreshing"
            );
        }
        BUTTON_TEST_REFRESHING => {
            set_button_test_phase(BUTTON_TEST_ABORTED);
            log::error!(
                "reset interrupted the SW7 wake refresh ({reset_reason:?}/{wake_reason:?}); \
                 aborting without automatic retry"
            );
        }
        BUTTON_TEST_ABORTED => log::error!("SW7 cycle test aborted; no automatic refresh"),
        phase => {
            set_button_test_phase(BUTTON_TEST_ABORTED);
            log::error!("invalid SW7 test phase {phase}; aborting without refresh");
        }
    }

    idle_awake()
}

#[cfg(not(feature = "debug-button-sleep"))]
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
    let now_us = rtc_time_us();
    initialize_retained_state(now_us);
    log::info!("firmware booting; reset reason: {reset_reason:?}, wakeup reason: {wake_reason:?}");
    let interrupted_refresh = recover_interrupted_refresh(now_us, reset_reason);
    if interrupted_refresh {
        log::error!(
            "previous panel refresh was interrupted or reset was caused by brownout; \
             skipping refresh and retrying no sooner than 24 hours"
        );
    }

    let mut manual_refresh = false;
    let requested_refresh = match wake_reason {
        WakeupReason::Button => match classify_gesture(&button, wake_at) {
            Gesture::Upload => {
                log::warn!("five-second SW7 hold detected; upload window is not implemented yet");
                false
            }
            Gesture::ManualRefresh => {
                let state = retained_state();
                if manual_refresh_allowed(rtc_time_us(), state.last_manual_refresh_us) {
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

    let cooling_down = rtc_time_us() < retained_state().retry_after_us;
    let manual_recovery_override = manual_refresh;
    let should_refresh =
        requested_refresh && (manual_recovery_override || (!cooling_down && !interrupted_refresh));
    if requested_refresh && !should_refresh {
        log::warn!("refresh suppressed by interrupted-refresh recovery cooldown");
    } else if requested_refresh && manual_recovery_override && (cooling_down || interrupted_refresh)
    {
        log::warn!("explicit triple-press refresh overrides interrupted-refresh cooldown");
    }

    if should_refresh {
        update_retained_state(|state| state.refresh_in_progress = 1);
        match refresh_panel() {
            Ok(()) => {
                let refreshed_at_us = rtc_time_us();
                update_retained_state(|state| {
                    state.last_refresh_us = refreshed_at_us;
                    state.retry_after_us = 0;
                    state.refresh_in_progress = 0;
                    if manual_refresh {
                        state.last_manual_refresh_us = refreshed_at_us;
                    }
                });
            }
            Err(error) => {
                log::error!("display refresh failed: {error:?}");
                let failed_at_us = rtc_time_us();
                update_retained_state(|state| {
                    let scheduled_at_us = state.last_refresh_us.saturating_add(US_PER_DAY);
                    state.refresh_in_progress = 0;
                    state.retry_after_us = if scheduled_at_us <= failed_at_us {
                        failed_at_us.saturating_add(US_PER_DAY)
                    } else {
                        0
                    };
                });
            }
        }
    }

    sleep_until_refresh(&button)
}
