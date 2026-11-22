//! GPIO sampling and production sleep preparation.

use std::time::{Duration, Instant};

use crate::gesture::{Gesture, GestureClassifier};
use esp_idf_svc::hal::{
    delay::FreeRtos,
    gpio::{PinDriver, RtcInput},
};

pub const POLL_MS: u32 = 5;
const RELEASE_TIMEOUT: Duration = Duration::from_secs(60);
pub type Button = PinDriver<'static, RtcInput>;

/// Sample KEY0 until its bounded gesture classifier reaches a decision.
pub fn classify(button: &Button, wake_at: Instant) -> Gesture {
    let mut classifier = GestureClassifier::new(wake_at, button.is_low());
    loop {
        if let Some(gesture) = classifier.sample(Instant::now(), button.is_low()) {
            return gesture;
        }
        FreeRtos::delay_ms(POLL_MS);
    }
}

production_items! {
    use crate::gesture::BUTTON_DEBOUNCE;

    /// Wait for a stable release; a stuck button must not cause a wake loop.
    fn wait_for_release(button: &Button) -> bool {
        let started = Instant::now();
        let mut released_since = None;
        loop {
            let now = Instant::now();
            if button.is_low() {
                released_since = None;
            } else if let Some(released_at) = released_since {
                if now.duration_since(released_at) >= BUTTON_DEBOUNCE {
                    return true;
                }
            } else {
                released_since = Some(now);
            }
            if now.duration_since(started) >= RELEASE_TIMEOUT {
                log::warn!("KEY0 remained pressed for 60 s; disabling button wake until timer wake");
                return false;
            }
            FreeRtos::delay_ms(POLL_MS);
        }
    }

    /// Arm the remaining RTC deadline and enter deep sleep.
    pub fn sleep(button: &Button) -> ! {
        use crate::schedule::DAILY_INTERVAL_US;
        use esp_idf_svc::hal::sleep::{rtc::RtcWakeLevel, DeepSleep};

        let released = wait_for_release(button);
        let remaining_us = crate::rtc::retained()
            .schedule
            .deadline_us(DAILY_INTERVAL_US)
            .saturating_sub(crate::rtc::now_us())
            .max(1_000_000);
        let mut sleep = DeepSleep::new()
            .expect("failed to reset deep-sleep wakeup sources")
            .wakeup_on_timer(Duration::from_micros(remaining_us))
            .expect("failed to configure RTC timer wake");
        if released {
            sleep = sleep
                .wakeup_on_rtc(button, RtcWakeLevel::AllLow)
                .expect("failed to configure KEY0 wake");
        }
        log::info!(
            "entering deep sleep; next refresh in {} s{}",
            remaining_us / 1_000_000,
            if released {
                " with KEY0 wake enabled"
            } else {
                " with timer wake only"
            }
        );
        sleep.enter()
    }
}
