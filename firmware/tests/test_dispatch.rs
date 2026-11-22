use firmware::dispatch::*;
use firmware::{
    gesture::Gesture,
    safety::SafetyState,
    schedule::{RefreshSchedule, MANUAL_LIMIT_US},
};

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
    assert_eq!(
        decide(
            Some(Gesture::Sleep),
            SafetyState::Latched,
            true,
            schedule,
            0
        ),
        Decision::Sleep
    );
}

#[test]
fn manual_limit_does_not_block_upload_or_automatic_refresh() {
    let mut schedule = RefreshSchedule::new(0);
    schedule.refresh_succeeded(10, true);
    assert_eq!(
        decide(
            Some(Gesture::ManualRefresh),
            SafetyState::Clean,
            false,
            schedule,
            10
        ),
        Decision::RateLimited
    );
    assert_eq!(
        decide(
            Some(Gesture::ManualRefresh),
            SafetyState::Clean,
            false,
            schedule,
            10 + MANUAL_LIMIT_US
        ),
        Decision::Refresh { manual: true }
    );
    assert_eq!(
        decide(
            Some(Gesture::Upload),
            SafetyState::Clean,
            false,
            schedule,
            10
        ),
        Decision::Upload
    );
    assert_eq!(
        decide(None, SafetyState::Clean, false, schedule, 10),
        Decision::Refresh { manual: false }
    );
}
