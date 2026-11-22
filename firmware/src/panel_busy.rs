use std::time::Duration;

pub const ASSERT_TIMEOUT: Duration = Duration::from_secs(1);
pub const COMPLETE_TIMEOUT: Duration = Duration::from_secs(120);
// Plausibility guard, not a panel timing specification (12 s typical at 25 C).
pub const MIN_WAVEFORM_BUSY: Duration = Duration::from_secs(1);

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum BusyTimeout {
    /// BUSY did not assert before the start deadline.
    #[error("panel BUSY never asserted")]
    NeverAsserted,
    /// BUSY did not complete before the refresh deadline.
    #[error("panel BUSY never completed")]
    NeverCompleted,
    /// BUSY returned to idle too quickly to indicate a plausible refresh.
    #[error("panel BUSY waveform was implausibly short")]
    ImplausiblyShort,
}

#[derive(Default)]
pub struct RefreshBusy {
    asserted_at: Option<Duration>,
}

impl RefreshBusy {
    /// An idle level is completion only after the waveform has asserted BUSY.
    pub fn sample(&mut self, elapsed: Duration, idle: bool) -> Result<bool, BusyTimeout> {
        let asserted_at = match self.asserted_at {
            Some(asserted_at) => asserted_at,
            None if elapsed >= ASSERT_TIMEOUT => return Err(BusyTimeout::NeverAsserted),
            None if idle => return Ok(false),
            None => *self.asserted_at.insert(elapsed),
        };
        if elapsed >= COMPLETE_TIMEOUT {
            return Err(BusyTimeout::NeverCompleted);
        }
        if idle && elapsed.saturating_sub(asserted_at) < MIN_WAVEFORM_BUSY {
            return Err(BusyTimeout::ImplausiblyShort);
        }
        Ok(idle)
    }
}
