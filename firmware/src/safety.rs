#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SafetyState {
    Clean,
    Working,
    Latched,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Store {
        value: Option<u8>,
        writes: usize,
        fail: bool,
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
