use firmware::gesture::{FaultSwitch, Gesture, GestureClassifier};
use std::time::{Duration, Instant};

fn at(wake_at: Instant, millis: u64) -> Instant {
    wake_at + Duration::from_millis(millis)
}

#[test]
fn fault_switch_debounces_press_and_requires_release_before_next_toggle() {
    let now = Instant::now();
    let mut switch = FaultSwitch::new(now, false);
    for (millis, pressed, toggled) in [
        (0, true, false),
        (10, false, false),
        (20, true, false),
        (69, true, false),
        (70, true, true),
        (5000, true, false),
        (6000, false, false),
        (6050, false, false),
        (7000, true, false),
        (7050, true, true),
    ] {
        assert_eq!(switch.sample(at(now, millis), pressed), toggled);
    }
}

#[test]
fn fault_switch_held_at_boot_does_not_toggle() {
    let now = Instant::now();
    let mut switch = FaultSwitch::new(now, true);
    for (millis, pressed, toggled) in [
        (5000, true, false),
        (6000, false, false),
        (6050, false, false),
        (7000, true, false),
        (7050, true, true),
    ] {
        assert_eq!(switch.sample(at(now, millis), pressed), toggled);
    }
}

#[test]
fn waking_press_held_for_three_seconds_requests_upload() {
    let wake_at = Instant::now();
    let mut classifier = GestureClassifier::new(wake_at, true);

    assert_eq!(classifier.sample(at(wake_at, 2_999), true), None);
    assert_eq!(
        classifier.sample(at(wake_at, 3_000), true),
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
fn waking_press_released_before_three_seconds_does_not_request_upload() {
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

    assert_eq!(classifier.sample(at(wake_at, 2_990), false), None);
    assert_eq!(classifier.sample(at(wake_at, 3_000), false), None);
    assert_eq!(
        classifier.sample(at(wake_at, 3_040), false),
        Some(Gesture::Sleep)
    );
}

#[test]
fn short_release_bounce_does_not_cancel_waking_hold() {
    let wake_at = Instant::now();
    let mut classifier = GestureClassifier::new(wake_at, true);

    assert_eq!(classifier.sample(at(wake_at, 2_990), false), None);
    assert_eq!(classifier.sample(at(wake_at, 3_000), false), None);
    assert_eq!(
        classifier.sample(at(wake_at, 3_010), true),
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
#[test]
fn release_gate_rejects_held_and_bouncing_presses_and_rearms_after_release() {
    use firmware::gesture::ReleaseGate;
    let start = std::time::Instant::now();
    let mut gate = ReleaseGate::default();
    assert!(!gate.sample(start, true));
    assert!(!gate.sample(start + std::time::Duration::from_secs(4), true));
    assert!(!gate.sample(start + std::time::Duration::from_secs(5), false));
    assert!(!gate.sample(start + std::time::Duration::from_millis(5049), true));
    assert!(!gate.sample(start + std::time::Duration::from_secs(6), false));
    assert!(!gate.sample(start + std::time::Duration::from_millis(6049), false));
    assert!(!gate.armed());
    assert!(!gate.sample(start + std::time::Duration::from_millis(6050), false));
    assert!(gate.armed());
    assert!(gate.sample(start + std::time::Duration::from_millis(6051), true));
    assert!(!gate.sample(start + std::time::Duration::from_secs(7), true));
    assert!(!gate.sample(start + std::time::Duration::from_secs(8), false));
    assert!(!gate.sample(start + std::time::Duration::from_millis(8050), false));
    assert!(gate.sample(start + std::time::Duration::from_millis(8051), true));
}
