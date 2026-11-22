use firmware::dispatch::*;
use firmware::{gesture::Gesture, safety::SafetyState, schedule::RefreshSchedule};

#[test]
fn safety_and_mode_blocks_prevent_every_work_source() {
    let schedule = RefreshSchedule::new(0);
    for gesture in [None, Some(Gesture::Upload), Some(Gesture::ManualRefresh)] {
        for safety in [SafetyState::Working, SafetyState::Latched] {
            assert_eq!(
                decide(gesture, safety, false, schedule, 0),
                Decision::Blocked
            );
        }
        assert_eq!(
            decide(gesture, SafetyState::Clean, true, schedule, 0),
            Decision::Blocked
        );
    }
    for (safety, blocked) in [
        (SafetyState::Clean, false),
        (SafetyState::Clean, true),
        (SafetyState::Working, false),
        (SafetyState::Latched, true),
    ] {
        assert_eq!(
            decide(Some(Gesture::Sleep), safety, blocked, schedule, 0),
            Decision::Sleep
        );
    }
}

#[test]
fn manual_limit_does_not_block_upload_or_automatic_refresh() {
    let mut schedule = RefreshSchedule::new(0);
    let hour = 3_600_000_000;
    let last_manual = 2 * hour;
    schedule.refresh_succeeded(last_manual, true);
    assert_eq!(
        decide(
            Some(Gesture::ManualRefresh),
            SafetyState::Clean,
            false,
            schedule,
            last_manual + hour - 1
        ),
        Decision::RateLimited
    );
    assert_eq!(
        decide(
            Some(Gesture::ManualRefresh),
            SafetyState::Clean,
            false,
            schedule,
            last_manual + hour
        ),
        Decision::Refresh { manual: true }
    );
    assert_eq!(
        decide(
            Some(Gesture::Upload),
            SafetyState::Clean,
            false,
            schedule,
            last_manual
        ),
        Decision::Upload
    );
    assert_eq!(
        decide(None, SafetyState::Clean, false, schedule, last_manual),
        Decision::Refresh { manual: false }
    );
}
