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
#[path = "../tests/unit/safety.rs"]
mod tests;
