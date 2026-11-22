//! Pure work-selection rules, independent of GPIO, clocks, and NVS.

use crate::{
    gesture::Gesture,
    safety::SafetyState,
    schedule::{MANUAL_LIMIT_US, RefreshSchedule},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Return to sleep or idle without starting work.
    Sleep,
    /// Block work because safety or operating mode prevents it.
    Blocked,
    /// Reject a manual refresh because its rate limit has not elapsed.
    RateLimited,
    /// Open an upload window.
    Upload,
    /// Refresh the display, manually or on schedule.
    Refresh { manual: bool },
}

/// Select work from a gesture; absent gestures represent automatic refreshes.
/// Callers provide mode-specific blocking (simulation or retained recovery).
pub fn decide(
    gesture: Option<Gesture>,
    safety: SafetyState,
    blocked: bool,
    schedule: RefreshSchedule,
    now_us: u64,
) -> Decision {
    if gesture == Some(Gesture::Sleep) {
        return Decision::Sleep;
    }
    if blocked || safety != SafetyState::Clean {
        return Decision::Blocked;
    }
    match gesture {
        Some(Gesture::Upload) => Decision::Upload,
        Some(Gesture::ManualRefresh) if !schedule.manual_allowed(now_us, MANUAL_LIMIT_US) => {
            Decision::RateLimited
        }
        Some(Gesture::ManualRefresh) => Decision::Refresh { manual: true },
        None => Decision::Refresh { manual: false },
        Some(Gesture::Sleep) => Decision::Sleep,
    }
}
