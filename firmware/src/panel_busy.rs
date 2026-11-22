use std::time::Duration;

pub const ASSERT_TIMEOUT: Duration = Duration::from_secs(1);
pub const COMPLETE_TIMEOUT: Duration = Duration::from_secs(120);
// Plausibility guard, not a panel timing specification (12 s typical at 25 C).
pub const MIN_WAVEFORM_BUSY: Duration = Duration::from_secs(1);

#[derive(Debug, PartialEq)]
pub enum BusyTimeout {
    /// BUSY did not assert before the start deadline.
    NeverAsserted,
    /// BUSY did not complete before the refresh deadline.
    NeverCompleted,
    /// BUSY returned to idle too quickly to indicate a plausible refresh.
    ImplausiblyShort,
}

#[derive(Default)]
pub struct RefreshBusy {
    asserted_at: Option<Duration>,
}

impl RefreshBusy {
    /// An idle level is completion only after the waveform has asserted BUSY.
    pub fn sample(&mut self, elapsed: Duration, idle: bool) -> Result<bool, BusyTimeout> {
        if self.asserted_at.is_none() {
            if elapsed >= ASSERT_TIMEOUT {
                return Err(BusyTimeout::NeverAsserted);
            }
            if idle {
                return Ok(false);
            }
            self.asserted_at = Some(elapsed);
        }
        if elapsed >= COMPLETE_TIMEOUT {
            return Err(BusyTimeout::NeverCompleted);
        }
        if idle
            && elapsed.saturating_sub(self.asserted_at.expect("BUSY assertion observed"))
                < MIN_WAVEFORM_BUSY
        {
            return Err(BusyTimeout::ImplausiblyShort);
        }
        Ok(idle)
    }
}
