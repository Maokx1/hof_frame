pub const DAILY_INTERVAL_US: u64 = 24 * 60 * 60 * 1_000_000;
pub const MANUAL_LIMIT_US: u64 = 60 * 60 * 1_000_000;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RefreshSchedule {
    last_refresh_us: u64,
    last_manual_refresh_us: Option<u64>,
    retry_after_us: u64,
}

impl RefreshSchedule {
    pub const fn new(now_us: u64) -> Self {
        Self {
            last_refresh_us: now_us,
            last_manual_refresh_us: None,
            retry_after_us: 0,
        }
    }

    pub fn deadline_us(self, interval_us: u64) -> u64 {
        self.last_refresh_us
            .saturating_add(interval_us)
            .max(self.retry_after_us)
    }

    pub fn due(self, now_us: u64, interval_us: u64) -> bool {
        now_us >= self.deadline_us(interval_us)
    }

    pub fn manual_allowed(self, now_us: u64, limit_us: u64) -> bool {
        match self.last_manual_refresh_us {
            None => true,
            Some(last) => now_us.saturating_sub(last) >= limit_us,
        }
    }

    pub fn cooling_down(self, now_us: u64) -> bool {
        now_us < self.retry_after_us
    }

    pub fn refresh_succeeded(&mut self, completed_at_us: u64, manual: bool) {
        self.last_refresh_us = completed_at_us;
        self.retry_after_us = 0;
        if manual {
            self.last_manual_refresh_us = Some(completed_at_us);
        }
    }

    pub fn refresh_failed(&mut self, failed_at_us: u64, interval_us: u64) {
        // An early manual failure keeps the scheduled refresh; an overdue
        // failure backs off without pretending a refresh completed.
        if failed_at_us >= self.last_refresh_us.saturating_add(interval_us) {
            self.defer_retry(failed_at_us, interval_us);
        }
    }

    pub fn defer_retry(&mut self, now_us: u64, interval_us: u64) {
        self.retry_after_us = now_us.saturating_add(interval_us);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_intervals_are_twenty_four_hours_and_one_hour() {
        assert_eq!(DAILY_INTERVAL_US, 86_400_000_000);
        assert_eq!(MANUAL_LIMIT_US, 3_600_000_000);
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
        let mut schedule = RefreshSchedule::new(u64::MAX - 5);
        assert_eq!(schedule.deadline_us(100), u64::MAX);
        schedule.defer_retry(u64::MAX - 2, 100);
        assert_eq!(schedule.deadline_us(100), u64::MAX);
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
}
