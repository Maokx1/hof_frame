pub const DAILY_INTERVAL_US: u64 = 24 * 60 * 60 * 1_000_000;
pub const MANUAL_LIMIT_US: u64 = 60 * 60 * 1_000_000;
pub const USB_INTERVAL_US: u64 = 60 * 1_000_000;

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
#[path = "../tests/unit/schedule.rs"]
mod tests;
