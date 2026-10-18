#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SafetyState {
    Clean,
    Working,
    Latched,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkPhase {
    Idle = 0,
    Setup = 1,
    Upload = 2,
    Decode = 3,
    Save = 4,
    Display = 5,
    PanelCleanup = 6,
    SafetyCleanup = 7,
    RadioCleanup = 8,
    LoadFrame = 9,
    Unknown = 255,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultReason {
    None = 0,
    Interrupted = 1,
    Brownout = 2,
    InvalidState = 3,
    RuntimeFailure = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub phase: WorkPhase,
    pub reason: FaultReason,
}

impl Diagnostic {
    pub fn record(self) -> u32 {
        0xD100_0000 | ((self.phase as u32) << 8) | self.reason as u32
    }

    pub fn from_record(value: u32) -> Option<Self> {
        if value & 0xFFFF_0000 != 0xD100_0000 {
            return None;
        }
        let phase = match (value >> 8) & 255 {
            0 => WorkPhase::Idle,
            1 => WorkPhase::Setup,
            2 => WorkPhase::Upload,
            3 => WorkPhase::Decode,
            4 => WorkPhase::Save,
            5 => WorkPhase::Display,
            6 => WorkPhase::PanelCleanup,
            7 => WorkPhase::SafetyCleanup,
            8 => WorkPhase::RadioCleanup,
            9 => WorkPhase::LoadFrame,
            255 => WorkPhase::Unknown,
            _ => return None,
        };
        let reason = match value & 255 {
            0 => FaultReason::None,
            1 => FaultReason::Interrupted,
            2 => FaultReason::Brownout,
            3 => FaultReason::InvalidState,
            4 => FaultReason::RuntimeFailure,
            _ => return None,
        };
        Some(Self { phase, reason })
    }
}

pub trait DiagnosticStore: SafetyStore {
    fn phase(&mut self, phase: WorkPhase) -> Result<(), Self::Error>;
}

pub fn boot_fault(previous_state: Option<u8>, brownout: bool) -> Option<FaultReason> {
    if brownout {
        Some(FaultReason::Brownout)
    } else {
        match previous_state {
            Some(0xA1) => Some(FaultReason::Interrupted),
            None | Some(0xA0) | Some(0xA2) => None,
            Some(_) => Some(FaultReason::InvalidState),
        }
    }
}

pub trait SafetyStore {
    type Error;
    fn read(&mut self) -> Result<Option<u8>, Self::Error>;
    fn write(&mut self, value: u8) -> Result<(), Self::Error>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum SafetyError<E> {
    Storage(E),
    Blocked,
}

pub struct SafetyGuard<S: SafetyStore> {
    store: S,
    state: SafetyState,
}

impl<S: SafetyStore> SafetyGuard<S> {
    pub fn open(mut store: S, brownout: bool) -> Result<Self, SafetyError<S::Error>> {
        let value = store.read().map_err(SafetyError::Storage)?;
        let state = match value {
            None | Some(0xA0) => SafetyState::Clean,
            Some(0xA1) => SafetyState::Working,
            _ => SafetyState::Latched,
        };
        let mut guard = Self { store, state };
        if (brownout && state != SafetyState::Latched)
            || state == SafetyState::Working
            || (state == SafetyState::Latched && value != Some(0xA2))
        {
            guard.latch()?;
        } else if value.is_none() {
            guard.persist(SafetyState::Clean)?;
        }
        Ok(guard)
    }

    pub fn state(&self) -> SafetyState {
        self.state
    }

    pub fn recover_interrupted_work(&mut self) -> Result<(), SafetyError<S::Error>> {
        self.state = SafetyState::Latched;
        let value = self.store.read().map_err(SafetyError::Storage)?;
        if value == Some(0xA0) {
            self.state = SafetyState::Clean;
            Ok(())
        } else if value == Some(0xA2) {
            Ok(())
        } else {
            self.latch()
        }
    }

    pub fn begin_work(&mut self) -> Result<(), SafetyError<S::Error>> {
        if self.state != SafetyState::Clean {
            return Err(SafetyError::Blocked);
        }
        self.persist(SafetyState::Working)
    }

    pub fn finish_work(&mut self) -> Result<(), SafetyError<S::Error>> {
        if self.state != SafetyState::Working {
            return Err(SafetyError::Blocked);
        }
        self.persist(SafetyState::Clean)
    }

    pub fn latch(&mut self) -> Result<(), SafetyError<S::Error>> {
        self.persist(SafetyState::Latched)
    }

    pub fn acknowledge(
        &mut self,
        qualified_hold: bool,
        recovery_voltage_safe: bool,
    ) -> Result<(), SafetyError<S::Error>> {
        if self.state != SafetyState::Latched || !qualified_hold || !recovery_voltage_safe {
            return Err(SafetyError::Blocked);
        }
        self.persist(SafetyState::Clean)
    }

    fn persist(&mut self, state: SafetyState) -> Result<(), SafetyError<S::Error>> {
        let value = match state {
            SafetyState::Clean => 0xA0,
            SafetyState::Working => 0xA1,
            SafetyState::Latched => 0xA2,
        };
        // A failed/uncertain commit cannot grant permission in this boot.
        self.state = SafetyState::Latched;
        self.store.write(value).map_err(SafetyError::Storage)?;
        self.state = state;
        Ok(())
    }
}

impl<S: DiagnosticStore> SafetyGuard<S> {
    pub fn set_phase(&mut self, phase: WorkPhase) -> Result<(), SafetyError<S::Error>> {
        if let Err(error) = self.store.phase(phase) {
            self.state = SafetyState::Latched;
            return Err(SafetyError::Storage(error));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
