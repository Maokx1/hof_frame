use firmware::safety::*;

#[derive(Default)]
struct Store {
    value: Option<u8>,
    writes: usize,
    fail: bool,
    fail_state_write: bool,
    phase: Option<WorkPhase>,
}

impl SafetyStore for Store {
    type Error = &'static str;
    fn read(&mut self) -> Result<Option<u8>, Self::Error> {
        if self.fail {
            Err("read")
        } else {
            Ok(self.value)
        }
    }
    fn write(&mut self, value: u8) -> Result<(), Self::Error> {
        if self.fail || self.fail_state_write {
            return Err("write");
        }
        self.value = Some(value);
        self.writes += 1;
        Ok(())
    }
}

impl DiagnosticStore for Store {
    fn phase(&mut self, phase: WorkPhase) -> Result<(), Self::Error> {
        if self.fail {
            return Err("diagnostic write");
        }
        self.phase = Some(phase);
        Ok(())
    }
}

#[test]
fn diagnostic_records_preserve_persisted_phase_and_reason_codes() {
    for (phase, phase_code) in [
        (WorkPhase::Idle, 0),
        (WorkPhase::Setup, 1),
        (WorkPhase::Upload, 2),
        (WorkPhase::Decode, 3),
        (WorkPhase::Save, 4),
        (WorkPhase::Display, 5),
        (WorkPhase::PanelCleanup, 6),
        (WorkPhase::SafetyCleanup, 7),
        (WorkPhase::RadioCleanup, 8),
        (WorkPhase::LoadFrame, 9),
        (WorkPhase::Unknown, 255),
    ] {
        for (reason, reason_code) in [
            (FaultReason::None, 0),
            (FaultReason::Interrupted, 1),
            (FaultReason::Brownout, 2),
            (FaultReason::InvalidState, 3),
            (FaultReason::RuntimeFailure, 4),
        ] {
            let value = Diagnostic { phase, reason };
            let record = 0xD100_0000 | (phase_code << 8) | reason_code;
            assert_eq!(value.record(), record);
            assert_eq!(Diagnostic::from_record(record), Some(value));
        }
    }
    for invalid in [0, 0xD200_0000, 0xD101_0000, 0xD100_0A00, 0xD100_0005] {
        assert_eq!(Diagnostic::from_record(invalid), None);
    }
}

#[test]
fn boot_diagnostics_distinguish_new_faults_from_existing_latch() {
    assert_eq!(boot_fault(None, false), None);
    assert_eq!(boot_fault(Some(0xA0), false), None);
    assert_eq!(boot_fault(Some(0xA2), false), None);
    assert_eq!(
        boot_fault(Some(0xA1), false),
        Some(FaultReason::Interrupted)
    );
    assert_eq!(
        boot_fault(Some(0x42), false),
        Some(FaultReason::InvalidState)
    );
    for state in [None, Some(0xA0), Some(0xA1), Some(0xA2), Some(0x42)] {
        assert_eq!(boot_fault(state, true), Some(FaultReason::Brownout));
    }
}

#[test]
fn phase_tracking_does_not_complete_or_clear_work() {
    let mut guard = SafetyGuard::open(Store::default(), false).unwrap();
    guard.begin_work().unwrap();
    guard.set_phase(WorkPhase::Save).unwrap();
    assert_eq!(guard.store.phase, Some(WorkPhase::Save));
    assert_eq!(guard.state(), SafetyState::Working);
    assert_eq!(guard.store.value, Some(0xA1));
    guard.latch().unwrap();
    guard.set_phase(WorkPhase::RadioCleanup).unwrap();
    assert_eq!(guard.state(), SafetyState::Latched);
    assert_eq!(guard.store.value, Some(0xA2));
}

#[test]
fn diagnostic_write_failure_blocks_current_work() {
    let mut guard = SafetyGuard::open(Store::default(), false).unwrap();
    guard.begin_work().unwrap();
    guard.store.fail = true;
    assert_eq!(
        guard.set_phase(WorkPhase::Display),
        Err(SafetyError::Storage("diagnostic write"))
    );
    assert_eq!(guard.state(), SafetyState::Latched);
    assert_eq!(guard.store.value, Some(0xA1));
    assert_eq!(guard.finish_work(), Err(SafetyError::Blocked));
}

#[test]
fn first_boot_establishes_clean_record() {
    let guard = SafetyGuard::open(Store::default(), false).unwrap();
    assert_eq!(guard.state(), SafetyState::Clean);
    assert_eq!(guard.store.value, Some(0xA0));
}

#[test]
fn marker_is_committed_before_work_and_cleaned_after_completion() {
    let mut guard = SafetyGuard::open(Store::default(), false).unwrap();
    guard.begin_work().unwrap();
    assert_eq!(guard.store.value, Some(0xA1));
    assert_eq!(guard.begin_work(), Err(SafetyError::Blocked));
    guard.finish_work().unwrap();
    assert_eq!(guard.store.value, Some(0xA0));
}

#[test]
fn interrupted_work_latches_on_reopen() {
    let mut guard = SafetyGuard::open(Store::default(), false).unwrap();
    guard.begin_work().unwrap();
    let mut rebooted = SafetyGuard::open(guard.store, false).unwrap();
    assert_eq!(rebooted.state(), SafetyState::Latched);
    assert_eq!(rebooted.store.value, Some(0xA2));
    assert_eq!(rebooted.begin_work(), Err(SafetyError::Blocked));
    assert_eq!(rebooted.finish_work(), Err(SafetyError::Blocked));
}

#[test]
fn brownout_and_unknown_records_latch_across_reboots() {
    for (value, brownout) in [(0xA0, true), (0x42, false)] {
        let mut guard = SafetyGuard::open(
            Store {
                value: Some(value),
                ..Store::default()
            },
            brownout,
        )
        .unwrap();
        assert_eq!(guard.state(), SafetyState::Latched);
        assert_eq!(guard.store.value, Some(0xA2));
        assert_eq!(guard.begin_work(), Err(SafetyError::Blocked));

        let mut rebooted = SafetyGuard::open(guard.store, false).unwrap();
        assert_eq!(rebooted.state(), SafetyState::Latched);
        assert_eq!(rebooted.begin_work(), Err(SafetyError::Blocked));
    }
}

#[test]
fn latched_reboots_do_not_write_again() {
    let guard = SafetyGuard::open(
        Store {
            value: Some(0xA2),
            ..Store::default()
        },
        false,
    )
    .unwrap();
    assert_eq!(guard.store.writes, 0);
}

#[test]
fn only_qualified_hold_and_safe_recovery_voltage_clear_latch() {
    let mut guard = SafetyGuard::open(
        Store {
            value: Some(0xA2),
            ..Store::default()
        },
        false,
    )
    .unwrap();
    for (hold, safe) in [(false, false), (false, true), (true, false)] {
        assert_eq!(guard.acknowledge(hold, safe), Err(SafetyError::Blocked));
        assert_eq!(guard.store.writes, 0);
    }
    guard.acknowledge(true, true).unwrap();
    assert_eq!(guard.state(), SafetyState::Clean);
    assert_eq!(guard.store.value, Some(0xA0));
}

#[test]
fn read_failure_is_explicit() {
    assert!(matches!(
        SafetyGuard::open(
            Store {
                fail: true,
                ..Store::default()
            },
            false
        ),
        Err(SafetyError::Storage("read"))
    ));
}

#[test]
fn marker_write_failure_blocks_work() {
    let mut guard = SafetyGuard::open(Store::default(), false).unwrap();
    guard.store.fail = true;
    assert_eq!(guard.begin_work(), Err(SafetyError::Storage("write")));
    assert_eq!(guard.state(), SafetyState::Latched);
    assert_eq!(guard.begin_work(), Err(SafetyError::Blocked));
}

#[test]
fn failed_clean_commit_leaves_durable_work_marker() {
    let mut guard = SafetyGuard::open(Store::default(), false).unwrap();
    guard.begin_work().unwrap();
    guard.store.fail = true;
    assert_eq!(guard.finish_work(), Err(SafetyError::Storage("write")));
    assert_eq!(guard.store.value, Some(0xA1));
    assert_eq!(guard.state(), SafetyState::Latched);
}

#[test]
fn failed_acknowledgement_does_not_grant_permission() {
    let mut guard = SafetyGuard::open(
        Store {
            value: Some(0xA2),
            ..Store::default()
        },
        false,
    )
    .unwrap();
    guard.store.fail = true;
    assert_eq!(
        guard.acknowledge(true, true),
        Err(SafetyError::Storage("write"))
    );
    assert_eq!(guard.state(), SafetyState::Latched);
}

#[test]
fn simulated_boot_rereads_committed_marker_and_latches() {
    let mut guard = SafetyGuard::open(Store::default(), false).unwrap();
    guard.begin_work().unwrap();
    guard.recover_interrupted_work().unwrap();
    assert_eq!(guard.state(), SafetyState::Latched);
    assert_eq!(guard.store.value, Some(0xA2));
    assert_eq!(guard.begin_work(), Err(SafetyError::Blocked));
}

#[test]
fn repeated_recovery_checks_do_not_rewrite_latch() {
    let mut guard = SafetyGuard::open(
        Store {
            value: Some(0xA2),
            ..Store::default()
        },
        true,
    )
    .unwrap();
    guard.recover_interrupted_work().unwrap();
    assert_eq!(guard.store.writes, 0);
}

#[test]
fn failed_recovery_read_blocks_work_until_clean_state_is_verified() {
    let mut guard = SafetyGuard::open(Store::default(), false).unwrap();
    guard.store.fail = true;
    assert_eq!(
        guard.recover_interrupted_work(),
        Err(SafetyError::Storage("read"))
    );
    assert_eq!(guard.state(), SafetyState::Latched);
    assert_eq!(guard.begin_work(), Err(SafetyError::Blocked));

    guard.store.fail = false;
    let writes = guard.store.writes;
    guard.recover_interrupted_work().unwrap();
    assert_eq!(guard.state(), SafetyState::Clean);
    assert_eq!(guard.store.writes, writes);
    guard.begin_work().unwrap();
    assert_eq!(guard.store.value, Some(0xA1));
}

#[test]
fn failed_recovery_latch_write_blocks_work_and_preserves_interrupted_marker() {
    let mut guard = SafetyGuard::open(Store::default(), false).unwrap();
    guard.begin_work().unwrap();
    guard.store.fail_state_write = true;

    assert_eq!(
        guard.recover_interrupted_work(),
        Err(SafetyError::Storage("write"))
    );
    assert_eq!(guard.state(), SafetyState::Latched);
    assert_eq!(guard.store.value, Some(0xA1));
    assert_eq!(guard.begin_work(), Err(SafetyError::Blocked));

    guard.store.fail_state_write = false;
    let rebooted = SafetyGuard::open(guard.store, false).unwrap();
    assert_eq!(rebooted.state(), SafetyState::Latched);
    assert_eq!(rebooted.store.value, Some(0xA2));
}

#[test]
fn failed_latch_write_keeps_work_blocked() {
    let mut guard = SafetyGuard::open(Store::default(), false).unwrap();
    guard.store.fail = true;
    assert_eq!(guard.latch(), Err(SafetyError::Storage("write")));
    assert_eq!(guard.begin_work(), Err(SafetyError::Blocked));
}

#[test]
fn clean_reboot_requires_no_metadata_write() {
    let guard = SafetyGuard::open(
        Store {
            value: Some(0xA0),
            ..Store::default()
        },
        false,
    )
    .unwrap();
    assert_eq!(guard.state(), SafetyState::Clean);
    assert_eq!(guard.store.writes, 0);
}
