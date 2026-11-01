use super::*;

#[test]
fn initial_idle_is_not_a_completed_refresh() {
    let mut busy = RefreshBusy::default();
    assert_eq!(busy.sample(Duration::ZERO, true), Ok(false));
    assert_eq!(busy.sample(Duration::from_millis(40), true), Ok(false));
    assert_eq!(
        busy.sample(ASSERT_TIMEOUT, true),
        Err(BusyTimeout::NeverAsserted)
    );
}

#[test]
fn delayed_assertion_must_be_followed_by_idle() {
    let mut busy = RefreshBusy::default();
    assert_eq!(busy.sample(Duration::ZERO, true), Ok(false));
    assert_eq!(busy.sample(Duration::from_millis(100), false), Ok(false));
    assert_eq!(busy.sample(Duration::from_secs(20), false), Ok(false));
    assert_eq!(busy.sample(Duration::from_secs(30), true), Ok(true));
}

#[test]
fn immediate_assertion_is_accepted() {
    let mut busy = RefreshBusy::default();
    assert_eq!(busy.sample(Duration::ZERO, false), Ok(false));
    assert_eq!(busy.sample(Duration::from_secs(30), true), Ok(true));
}

#[test]
fn stuck_busy_times_out_even_if_it_returns_idle_at_the_deadline() {
    for idle in [false, true] {
        let mut busy = RefreshBusy::default();
        assert_eq!(busy.sample(Duration::ZERO, false), Ok(false));
        assert_eq!(
            busy.sample(COMPLETE_TIMEOUT, idle),
            Err(BusyTimeout::NeverCompleted)
        );
    }
}

#[test]
fn late_assertion_does_not_bypass_start_timeout() {
    let mut busy = RefreshBusy::default();
    assert_eq!(
        busy.sample(ASSERT_TIMEOUT, false),
        Err(BusyTimeout::NeverAsserted)
    );
}

#[test]
fn reported_forty_millisecond_pulse_is_not_success() {
    let mut busy = RefreshBusy::default();
    assert_eq!(busy.sample(Duration::ZERO, false), Ok(false));
    assert_eq!(
        busy.sample(Duration::from_millis(40), true),
        Err(BusyTimeout::ImplausiblyShort)
    );
}

#[test]
fn minimum_busy_duration_is_measured_from_assertion() {
    let asserted_at = Duration::from_millis(900);
    for (duration, expected) in [
        (
            MIN_WAVEFORM_BUSY - Duration::from_millis(1),
            Err(BusyTimeout::ImplausiblyShort),
        ),
        (MIN_WAVEFORM_BUSY, Ok(true)),
    ] {
        let mut busy = RefreshBusy::default();
        assert_eq!(busy.sample(Duration::ZERO, true), Ok(false));
        assert_eq!(busy.sample(asserted_at, false), Ok(false));
        assert_eq!(busy.sample(asserted_at + duration, true), expected);
    }
}
