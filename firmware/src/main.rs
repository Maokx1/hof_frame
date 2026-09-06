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

/// Provisional SW2 wake button pin; confirm against the board schematic.
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

    // SW2 is assumed active-low (pressed pulls the pin to GND), so enable the
    // internal pull-up and wake while the pin reads low. Confirm the actual
    // active level and pull arrangement on the physical board. The pin must
    // be RTC-capable to remain configured as a deep-sleep wakeup source.
    let wake_pin = PinDriver::rtc_input(peripherals.pins.gpio12, Pull::Up)
        .expect("failed to configure GPIO12 as an RTC input");

    log::info!("wake source configured on {WAKE_BUTTON_GPIO}; entering placeholder init window");

    // Placeholder for the future wake/init/BLE-advertising window described in
    // the plan. Kept short during bring-up so the sleep/wake cycle is easy to
    // observe on the serial monitor.
    std::thread::sleep(Duration::from_secs(3));

    log::info!("entering deep sleep; wake by pressing {WAKE_BUTTON_GPIO}");

    DeepSleep::new()
        .expect("failed to reset wakeup sources")
        .wakeup_on_rtc(&wake_pin, RtcWakeLevel::AllLow)
        .expect("failed to configure RTC GPIO wakeup")
        .enter();
}
