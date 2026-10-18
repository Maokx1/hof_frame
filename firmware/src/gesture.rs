use std::time::{Duration, Instant};

pub const BUTTON_DEBOUNCE: Duration = Duration::from_millis(50);
pub const GESTURE_WINDOW: Duration = Duration::from_secs(3);
pub const LONG_HOLD: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    Sleep,
    Upload,
    ManualRefresh,
}

pub struct GestureClassifier {
    presses: u8,
    stable_pressed: bool,
    candidate_pressed: bool,
    candidate_since: Instant,
    waking_press_held: bool,
    gesture_deadline: Instant,
    hold_deadline: Instant,
    result: Option<Gesture>,
}

impl GestureClassifier {
    pub fn new(wake_at: Instant, initially_pressed: bool) -> Self {
        Self {
            presses: 1,
            stable_pressed: initially_pressed,
            candidate_pressed: initially_pressed,
            candidate_since: wake_at,
            waking_press_held: initially_pressed,
            gesture_deadline: wake_at + GESTURE_WINDOW,
            hold_deadline: wake_at + LONG_HOLD,
            result: None,
        }
    }

    pub fn sample(&mut self, now: Instant, raw_pressed: bool) -> Option<Gesture> {
        if self.result.is_some() {
            return self.result;
        }
        if raw_pressed != self.candidate_pressed {
            self.candidate_pressed = raw_pressed;
            self.candidate_since = now;
        } else if self.candidate_pressed != self.stable_pressed
            && now.duration_since(self.candidate_since) >= BUTTON_DEBOUNCE
        {
            self.stable_pressed = self.candidate_pressed;
            if self.stable_pressed {
                self.presses += 1;
                if self.presses == 3 && now <= self.gesture_deadline {
                    self.result = Some(Gesture::ManualRefresh);
                    return self.result;
                }
            } else {
                self.waking_press_held = false;
            }
        }

        // Resolve a pending release before accepting a hold at the deadline.
        if self.waking_press_held
            && self.stable_pressed
            && self.candidate_pressed
            && now >= self.hold_deadline
        {
            self.result = Some(Gesture::Upload);
        }
        if now >= self.gesture_deadline && !(self.waking_press_held && self.stable_pressed) {
            self.result = Some(Gesture::Sleep);
        }
        self.result
    }
}

#[cfg(test)]
mod tests {
    use super::{Gesture, GestureClassifier};
    use std::time::{Duration, Instant};

    fn at(wake_at: Instant, millis: u64) -> Instant {
        wake_at + Duration::from_millis(millis)
    }

    #[test]
    fn waking_press_held_for_five_seconds_requests_upload() {
        let wake_at = Instant::now();
        let mut classifier = GestureClassifier::new(wake_at, true);

        assert_eq!(classifier.sample(at(wake_at, 4_999), true), None);
        assert_eq!(
            classifier.sample(at(wake_at, 5_000), true),
            Some(Gesture::Upload)
        );
    }

    #[test]
    fn triple_press_within_window_requests_manual_refresh() {
        let wake_at = Instant::now();
        let mut classifier = GestureClassifier::new(wake_at, true);

        assert_eq!(classifier.sample(at(wake_at, 100), false), None);
        assert_eq!(classifier.sample(at(wake_at, 150), false), None);
        assert_eq!(classifier.sample(at(wake_at, 300), true), None);
        assert_eq!(classifier.sample(at(wake_at, 350), true), None);
        assert_eq!(classifier.sample(at(wake_at, 500), false), None);
        assert_eq!(classifier.sample(at(wake_at, 550), false), None);
        assert_eq!(classifier.sample(at(wake_at, 700), true), None);
        assert_eq!(
            classifier.sample(at(wake_at, 750), true),
            Some(Gesture::ManualRefresh)
        );
    }

    #[test]
    fn button_bounce_shorter_than_debounce_does_not_count_as_press() {
        let wake_at = Instant::now();
        let mut classifier = GestureClassifier::new(wake_at, false);

        assert_eq!(classifier.sample(at(wake_at, 10), true), None);
        assert_eq!(classifier.sample(at(wake_at, 40), false), None);
        assert_eq!(classifier.sample(at(wake_at, 100), false), None);
        assert_eq!(classifier.sample(at(wake_at, 200), true), None);
        assert_eq!(classifier.sample(at(wake_at, 250), true), None);
        assert_eq!(classifier.sample(at(wake_at, 300), false), None);
        assert_eq!(classifier.sample(at(wake_at, 350), false), None);
        assert_eq!(classifier.sample(at(wake_at, 500), true), None);
        assert_eq!(
            classifier.sample(at(wake_at, 550), true),
            Some(Gesture::ManualRefresh)
        );
    }

    #[test]
    fn incomplete_gesture_returns_to_sleep_after_window() {
        let wake_at = Instant::now();
        let mut classifier = GestureClassifier::new(wake_at, true);

        assert_eq!(classifier.sample(at(wake_at, 100), false), None);
        assert_eq!(classifier.sample(at(wake_at, 150), false), None);
        assert_eq!(classifier.sample(at(wake_at, 300), true), None);
        assert_eq!(classifier.sample(at(wake_at, 350), true), None);
        assert_eq!(classifier.sample(at(wake_at, 500), false), None);
        assert_eq!(classifier.sample(at(wake_at, 550), false), None);
        assert_eq!(
            classifier.sample(at(wake_at, 3_000), false),
            Some(Gesture::Sleep)
        );
    }

    #[test]
    fn third_press_not_debounced_by_window_deadline_returns_to_sleep() {
        let wake_at = Instant::now();
        let mut classifier = GestureClassifier::new(wake_at, true);

        assert_eq!(classifier.sample(at(wake_at, 100), false), None);
        assert_eq!(classifier.sample(at(wake_at, 150), false), None);
        assert_eq!(classifier.sample(at(wake_at, 300), true), None);
        assert_eq!(classifier.sample(at(wake_at, 350), true), None);
        assert_eq!(classifier.sample(at(wake_at, 500), false), None);
        assert_eq!(classifier.sample(at(wake_at, 550), false), None);
        assert_eq!(classifier.sample(at(wake_at, 2_960), true), None);
        assert_eq!(
            classifier.sample(at(wake_at, 3_000), true),
            Some(Gesture::Sleep)
        );
    }

    #[test]
    fn waking_press_released_before_five_seconds_does_not_request_upload() {
        let wake_at = Instant::now();
        let mut classifier = GestureClassifier::new(wake_at, true);

        assert_eq!(classifier.sample(at(wake_at, 1_000), false), None);
        assert_eq!(classifier.sample(at(wake_at, 1_050), false), None);
        assert_eq!(
            classifier.sample(at(wake_at, 3_000), false),
            Some(Gesture::Sleep)
        );
    }

    #[test]
    fn pending_release_at_hold_deadline_does_not_open_upload() {
        let wake_at = Instant::now();
        let mut classifier = GestureClassifier::new(wake_at, true);

        assert_eq!(classifier.sample(at(wake_at, 4_990), false), None);
        assert_eq!(classifier.sample(at(wake_at, 5_000), false), None);
        assert_eq!(
            classifier.sample(at(wake_at, 5_040), false),
            Some(Gesture::Sleep)
        );
    }

    #[test]
    fn short_release_bounce_does_not_cancel_waking_hold() {
        let wake_at = Instant::now();
        let mut classifier = GestureClassifier::new(wake_at, true);

        assert_eq!(classifier.sample(at(wake_at, 4_990), false), None);
        assert_eq!(classifier.sample(at(wake_at, 5_000), false), None);
        assert_eq!(
            classifier.sample(at(wake_at, 5_010), true),
            Some(Gesture::Upload)
        );
    }

    #[test]
    fn already_released_waking_click_still_counts_as_press_one() {
        let wake_at = Instant::now();
        let mut classifier = GestureClassifier::new(wake_at, false);

        assert_eq!(classifier.sample(at(wake_at, 100), true), None);
        assert_eq!(classifier.sample(at(wake_at, 150), true), None);
        assert_eq!(classifier.sample(at(wake_at, 300), false), None);
        assert_eq!(classifier.sample(at(wake_at, 350), false), None);
        assert_eq!(classifier.sample(at(wake_at, 500), true), None);
        assert_eq!(
            classifier.sample(at(wake_at, 550), true),
            Some(Gesture::ManualRefresh)
        );
    }

    #[test]
    fn holding_second_press_is_not_an_upload_gesture() {
        let wake_at = Instant::now();
        let mut classifier = GestureClassifier::new(wake_at, false);

        assert_eq!(classifier.sample(at(wake_at, 100), true), None);
        assert_eq!(classifier.sample(at(wake_at, 150), true), None);
        assert_eq!(
            classifier.sample(at(wake_at, 5_000), true),
            Some(Gesture::Sleep)
        );
    }

    #[test]
    fn third_press_debounced_exactly_at_deadline_is_accepted_once() {
        let wake_at = Instant::now();
        let mut classifier = GestureClassifier::new(wake_at, false);

        assert_eq!(classifier.sample(at(wake_at, 100), true), None);
        assert_eq!(classifier.sample(at(wake_at, 150), true), None);
        assert_eq!(classifier.sample(at(wake_at, 300), false), None);
        assert_eq!(classifier.sample(at(wake_at, 350), false), None);
        assert_eq!(classifier.sample(at(wake_at, 2_950), true), None);
        assert_eq!(
            classifier.sample(at(wake_at, 3_000), true),
            Some(Gesture::ManualRefresh)
        );
        assert_eq!(
            classifier.sample(at(wake_at, 5_000), false),
            Some(Gesture::ManualRefresh)
        );
    }

    #[test]
    fn timeout_is_terminal_even_if_further_presses_arrive() {
        let wake_at = Instant::now();
        let mut classifier = GestureClassifier::new(wake_at, false);

        assert_eq!(
            classifier.sample(at(wake_at, 3_000), false),
            Some(Gesture::Sleep)
        );
        assert_eq!(
            classifier.sample(at(wake_at, 3_100), true),
            Some(Gesture::Sleep)
        );
    }
}
