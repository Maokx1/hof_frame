pub const DAILY_INTERVAL_US: u64 = 24 * 60 * 60 * 1_000_000;
pub const MANUAL_LIMIT_US: u64 = 60 * 60 * 1_000_000;
pub const USB_INTERVAL_US: u64 = 60 * 1_000_000;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RefreshSchedule {
    /// Time of the last successful refresh.
    pub last_refresh_us: u64,
    last_manual_refresh_us: Option<u64>,
    retry_after_us: u64,
}

impl RefreshSchedule {
    /// Start a schedule without recording a manual refresh.
    pub const fn new(now_us: u64) -> Self {
        Self {
            last_refresh_us: now_us,
            last_manual_refresh_us: None,
            retry_after_us: 0,
        }
    }

    /// Return the later of the successful-refresh deadline and retry cooldown.
    pub fn deadline_us(self, interval_us: u64) -> u64 {
        self.last_refresh_us
            .saturating_add(interval_us)
            .max(self.retry_after_us)
    }

    /// Check whether the supplied time reaches the effective refresh deadline.
    pub fn due(self, now_us: u64, interval_us: u64) -> bool {
        now_us >= self.deadline_us(interval_us)
    }

    /// Check the manual-only limit without consuming it or moving the schedule.
    pub fn manual_allowed(self, now_us: u64, limit_us: u64) -> bool {
        match self.last_manual_refresh_us {
            None => true,
            Some(last) => now_us.saturating_sub(last) >= limit_us,
        }
    }

    /// Report whether interrupted/failed work still forbids retry.
    pub fn cooling_down(self, now_us: u64) -> bool {
        now_us < self.retry_after_us
    }

    /// Restart the interval at completion, consuming the manual limit only if manual.
    pub fn refresh_succeeded(&mut self, completed_at_us: u64, manual: bool) {
        self.last_refresh_us = completed_at_us;
        self.retry_after_us = 0;
        if manual {
            self.last_manual_refresh_us = Some(completed_at_us);
        }
    }

    /// Defer overdue failures without pretending that a refresh succeeded.
    pub fn refresh_failed(&mut self, failed_at_us: u64, interval_us: u64) {
        // An early manual failure keeps the scheduled refresh; an overdue
        // failure backs off without pretending a refresh completed.
        if failed_at_us >= self.last_refresh_us.saturating_add(interval_us) {
            self.defer_retry(failed_at_us, interval_us);
        }
    }

    /// Set a retry cooldown independently of the last successful/manual refresh.
    pub fn defer_retry(&mut self, now_us: u64, interval_us: u64) {
        self.retry_after_us = now_us.saturating_add(interval_us);
    }

    /// Remove the retry cooldown after a manual latch acknowledgement.
    pub fn clear_retry(&mut self) {
        self.retry_after_us = 0;
    }
}
