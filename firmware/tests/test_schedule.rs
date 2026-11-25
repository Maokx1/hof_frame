use firmware::schedule::*;

#[test]
fn production_intervals_are_twenty_four_hours_and_one_hour() {
    assert_eq!(DAILY_INTERVAL_US, 86_400_000_000);
    assert_eq!(MANUAL_LIMIT_US, 3_600_000_000);
}

#[test]
fn usb_timer_waits_sixty_seconds_after_completion() {
    assert_eq!(USB_INTERVAL_US, 60_000_000);
    let mut schedule = RefreshSchedule::new(0);
    assert!(!schedule.due(USB_INTERVAL_US - 1, USB_INTERVAL_US));
    assert!(schedule.due(USB_INTERVAL_US, USB_INTERVAL_US));
    let completed = 150_000_000;
    schedule.refresh_succeeded(completed, false);
    assert!(!schedule.due(completed + USB_INTERVAL_US - 1, USB_INTERVAL_US));
    assert!(schedule.due(completed + USB_INTERVAL_US, USB_INTERVAL_US));
    assert!(schedule.manual_allowed(completed, MANUAL_LIMIT_US));
    schedule.refresh_succeeded(completed + 1, true);
    schedule.refresh_succeeded(completed + 2, false);
    assert!(!schedule.manual_allowed(completed + MANUAL_LIMIT_US, MANUAL_LIMIT_US));
    assert!(schedule.manual_allowed(completed + 1 + MANUAL_LIMIT_US, MANUAL_LIMIT_US));
}
#[test]
fn early_wakes_leave_original_deadline_unchanged() {
    let schedule = RefreshSchedule::new(100);
    for now in [101, 150, 199] {
        assert!(!schedule.due(now, 100));
        assert_eq!(schedule.deadline_us(100), 200);
    }
    assert!(schedule.due(200, 100));
}

#[test]
fn successful_manual_refresh_restarts_daily_deadline_at_completion() {
    let mut schedule = RefreshSchedule::new(100);
    schedule.refresh_succeeded(175, true);
    assert_eq!(schedule.deadline_us(100), 275);
    assert!(!schedule.due(274, 100));
    assert!(schedule.due(275, 100));
}

#[test]
fn hourly_limit_includes_exact_boundary_and_zero_timestamp() {
    let mut schedule = RefreshSchedule::new(0);
    assert!(schedule.manual_allowed(0, MANUAL_LIMIT_US));
    schedule.refresh_succeeded(0, true);
    assert!(!schedule.manual_allowed(MANUAL_LIMIT_US - 1, MANUAL_LIMIT_US));
    assert!(schedule.manual_allowed(MANUAL_LIMIT_US, MANUAL_LIMIT_US));
}

#[test]
fn denied_manual_request_does_not_move_daily_deadline() {
    let mut schedule = RefreshSchedule::new(0);
    schedule.refresh_succeeded(10, true);
    assert!(!schedule.manual_allowed(20, 50));
    assert_eq!(schedule.deadline_us(100), 110);
}

#[test]
fn automatic_refresh_does_not_reset_manual_limit() {
    let mut schedule = RefreshSchedule::new(0);
    schedule.refresh_succeeded(10, true);
    schedule.refresh_succeeded(30, false);
    assert!(!schedule.manual_allowed(59, 50));
    assert!(schedule.manual_allowed(60, 50));
    assert_eq!(schedule.deadline_us(100), 130);
}

#[test]
fn early_failure_keeps_daily_deadline_and_does_not_consume_manual_limit() {
    let mut schedule = RefreshSchedule::new(0);
    schedule.refresh_failed(20, 100);
    assert_eq!(schedule.deadline_us(100), 100);
    assert!(schedule.manual_allowed(20, 50));
}

#[test]
fn overdue_failure_backs_off_without_updating_last_success() {
    let mut schedule = RefreshSchedule::new(0);
    schedule.refresh_failed(110, 100);
    assert_eq!(schedule.last_refresh_us, 0);
    assert_eq!(schedule.deadline_us(100), 210);
    assert!(!schedule.due(209, 100));
    assert!(schedule.due(210, 100));
}

#[test]
fn usb_display_failure_defers_a_full_interval_without_recording_success() {
    for failed_at in [20, USB_INTERVAL_US, USB_INTERVAL_US + 10] {
        let mut schedule = RefreshSchedule::new(0);
        schedule.defer_retry(failed_at, USB_INTERVAL_US);
        assert_eq!(schedule.last_refresh_us, 0);
        assert!(schedule.manual_allowed(failed_at, MANUAL_LIMIT_US));
        assert!(!schedule.due(failed_at + USB_INTERVAL_US - 1, USB_INTERVAL_US));
        assert!(schedule.due(failed_at + USB_INTERVAL_US, USB_INTERVAL_US));
        schedule.refresh_succeeded(failed_at + 1, false);
        assert_eq!(
            schedule.deadline_us(USB_INTERVAL_US),
            failed_at + 1 + USB_INTERVAL_US
        );
    }
}

#[test]
fn failed_refresh_keeps_the_previous_manual_limit() {
    let mut schedule = RefreshSchedule::new(0);
    schedule.refresh_succeeded(10, true);
    schedule.defer_retry(20, 100);
    assert!(!schedule.manual_allowed(59, 50));
    assert!(schedule.manual_allowed(60, 50));
}

#[test]
fn success_clears_recovery_cooldown() {
    let mut schedule = RefreshSchedule::new(0);
    schedule.defer_retry(50, 100);
    assert!(schedule.cooling_down(149));
    assert!(!schedule.cooling_down(150));
    schedule.refresh_succeeded(60, true);
    assert!(!schedule.cooling_down(60));
    assert_eq!(schedule.deadline_us(100), 160);
}

#[test]
fn clock_regression_does_not_bypass_manual_limit() {
    let mut schedule = RefreshSchedule::new(100);
    schedule.refresh_succeeded(100, true);
    assert!(!schedule.manual_allowed(50, 10));
}

#[test]
fn deadlines_saturate_instead_of_wrapping() {
    let schedule = RefreshSchedule::new(u64::MAX - 5);
    assert_eq!(schedule.deadline_us(100), u64::MAX);
    let mut retry = RefreshSchedule::new(0);
    retry.defer_retry(u64::MAX - 2, 100);
    assert_eq!(retry.deadline_us(100), u64::MAX);
    assert!(retry.cooling_down(u64::MAX - 1));
    assert!(!retry.cooling_down(u64::MAX));
}

#[test]
fn mock_acceptance_cycle_preserves_deadline_until_success() {
    let second = 1_000_000;
    let mut schedule = RefreshSchedule::new(0);
    let daily = 120 * second;
    let hourly = 30 * second;
    schedule.refresh_succeeded(2 * second, true);
    assert_eq!(schedule.deadline_us(daily), 122 * second);
    assert!(!schedule.manual_allowed(10 * second, hourly));
    assert!(!schedule.due(20 * second, daily));
    assert_eq!(schedule.deadline_us(daily), 122 * second);
    assert!(schedule.manual_allowed(32 * second, hourly));
    schedule.refresh_succeeded(33 * second, true);
    assert_eq!(schedule.deadline_us(daily), 153 * second);
    assert!(!schedule.due(153 * second - 1, daily));
    assert!(schedule.due(153 * second, daily));
    schedule.refresh_succeeded(154 * second, false);
    assert_eq!(schedule.deadline_us(daily), 274 * second);
    assert!(schedule.manual_allowed(154 * second, hourly));
}

#[test]
fn failed_manual_override_does_not_clear_recovery_cooldown() {
    let mut schedule = RefreshSchedule::new(0);
    schedule.defer_retry(10, 100);
    schedule.refresh_failed(20, 100);
    assert_eq!(schedule.deadline_us(100), 110);
    assert!(schedule.cooling_down(20));
    assert!(schedule.manual_allowed(20, 50));
}

#[test]
fn clear_retry_removes_cooldown_without_moving_schedule() {
    let mut schedule = RefreshSchedule::new(0);
    schedule.defer_retry(10, 100);
    assert!(schedule.cooling_down(50));
    schedule.clear_retry();
    assert!(!schedule.cooling_down(50));
    assert_eq!(schedule.deadline_us(1_000), 1_000);
}
