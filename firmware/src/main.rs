//! Milestone 2: minimal ESP-IDF-backed Rust project.
//!
//! Confirms the board boots, logs the reset/wake cause, and demonstrates a
//! full deep-sleep cycle waking on SW2 (provisionally GPIO12). Later
//! milestones replace the fixed delay below with BLE advertising and the
//! five-minute idle timer described in `docs/PLAN.md`.

use std::time::Duration;

use esp_idf_svc::hal::gpio::{PinDriver, Pull};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::reset::{ResetReason, WakeupReason};
use esp_idf_svc::hal::sleep::{rtc::RtcWakeLevel, DeepSleep};

/// SW2 wake button pin (active-high per Good Display documentation).
const WAKE_BUTTON_GPIO: &str = "GPIO12";

fn main() {
    // It is necessary to call this function once. Otherwise, some patches to the runtime
    // implemented by esp-idf-sys might not link properly. See https://github.com/esp-rs/esp-idf-template/issues/71
    esp_idf_svc::sys::link_patches();

    // Bind the log crate to the ESP Logging facilities
    esp_idf_svc::log::EspLogger::initialize_default();

    log::info!(
        "hof_frame firmware booting; reset reason: {:?}, wakeup reason: {:?}",
        ResetReason::get(),
        WakeupReason::get()
    );

    let peripherals = Peripherals::take().expect("peripherals already taken");

    // Good Display documents SW2 (IO12) as active-high, so enable the internal
    // pull-down and wake while the pin reads high. Confirm on the physical
    // board that it is the revision with SW2-SW4 fitted and whether an external
    // pull-down exists. GPIO12 is RTC-capable on the ESP32-S3.
    let wake_pin = PinDriver::rtc_input(peripherals.pins.gpio12, Pull::Down)
        .expect("failed to configure GPIO12 as an RTC input");

    log::info!("wake source configured on {WAKE_BUTTON_GPIO}; entering placeholder init window");

    // Placeholder for the future wake/init/BLE-advertising window described in
    // the plan. Kept short during bring-up so the sleep/wake cycle is easy to
    // observe on the serial monitor.
    std::thread::sleep(Duration::from_secs(3));

    log::info!("entering deep sleep; wake by pressing {WAKE_BUTTON_GPIO}");

    DeepSleep::new()
        .expect("failed to reset wakeup sources")
        .wakeup_on_rtc(&wake_pin, RtcWakeLevel::AnyHigh)
        .expect("failed to configure RTC GPIO wakeup")
        .enter();
}
