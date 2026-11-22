use firmware::safety::*;

#[derive(Default)]
struct Store {
    value: Option<u8>,
    writes: usize,
    fail: bool,
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
        if self.fail {
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
fn diagnostic_record_roundtrips_every_phase_and_reason() {
    for phase in [
        WorkPhase::Idle,
        WorkPhase::Setup,
        WorkPhase::Upload,
        WorkPhase::Decode,
        WorkPhase::Save,
        WorkPhase::Display,
        WorkPhase::PanelCleanup,
        WorkPhase::SafetyCleanup,
        WorkPhase::RadioCleanup,
        WorkPhase::LoadFrame,
        WorkPhase::Unknown,
    ] {
        for reason in [
            FaultReason::None,
            FaultReason::Interrupted,
            FaultReason::Brownout,
            FaultReason::InvalidState,
            FaultReason::RuntimeFailure,
        ] {
            let value = Diagnostic { phase, reason };
            assert_eq!(Diagnostic::from_record(value.record()), Some(value));
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
fn brownout_latches_even_with_clean_record() {
    let guard = SafetyGuard::open(
        Store {
            value: Some(0xA0),
            ..Store::default()
        },
        true,
    )
    .unwrap();
    assert_eq!(guard.state(), SafetyState::Latched);
}

#[test]
fn unknown_record_is_latched_not_treated_as_first_boot() {
    let guard = SafetyGuard::open(
        Store {
            value: Some(0x42),
            ..Store::default()
        },
        false,
    )
    .unwrap();
    assert_eq!(guard.store.value, Some(0xA2));
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
fn failed_recovery_read_blocks_work() {
    let mut guard = SafetyGuard::open(Store::default(), false).unwrap();
    guard.store.fail = true;
    assert_eq!(
        guard.recover_interrupted_work(),
        Err(SafetyError::Storage("read"))
    );
    assert_eq!(guard.begin_work(), Err(SafetyError::Blocked));
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
