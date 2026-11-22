#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SafetyState {
    Clean,
    Working,
    Latched,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkPhase {
    /// No tracked work is in progress.
    Idle = 0,
    /// Initialize hardware and services for work.
    Setup = 1,
    /// Receive an image during the upload window.
    Upload = 2,
    /// Decode the uploaded image.
    Decode = 3,
    /// Save the decoded frame to storage.
    Save = 4,
    /// Refresh the display panel.
    Display = 5,
    /// Power down the panel and release its resources.
    PanelCleanup = 6,
    /// Complete safety bookkeeping after work.
    SafetyCleanup = 7,
    /// Stop the upload radio and release its resources.
    RadioCleanup = 8,
    /// Load the saved frame for display.
    LoadFrame = 9,
    /// The phase associated with a fault is unknown.
    Unknown = 255,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultReason {
    /// No fault has been recorded.
    None = 0,
    /// Work was interrupted before completion.
    Interrupted = 1,
    /// A brownout was detected at startup.
    Brownout = 2,
    /// The stored safety state was invalid.
    InvalidState = 3,
    /// A failure occurred while the firmware was running.
    RuntimeFailure = 4,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub phase: WorkPhase,
    pub reason: FaultReason,
}

impl Diagnostic {
    /// Encode a versioned phase/reason record compatible with NVS storage.
    pub fn record(self) -> u32 {
        0xD100_0000 | ((self.phase as u32) << 8) | self.reason as u32
    }

    /// Decode only supported versions, phases, and reasons.
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
    /// Persist a work boundary without changing the authoritative safety state.
    fn phase(&mut self, phase: WorkPhase) -> Result<(), Self::Error>;
}

/// Infer a new boot fault, giving the current brownout precedence over interruption.
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
    /// Read the durable state; absence denotes first use, not implicit recovery.
    fn read(&mut self) -> Result<Option<u8>, Self::Error>;
    /// Commit the authoritative safety byte before returning success.
    fn write(&mut self, value: u8) -> Result<(), Self::Error>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum SafetyError<E> {
    Storage(E),
    Blocked,
}

pub struct SafetyGuard<S: SafetyStore> {
    /// Store backing the guard's durable safety state.
    pub store: S,
    state: SafetyState,
}

impl<S: SafetyStore> SafetyGuard<S> {
    /// Load durable safety state and latch interrupted, unknown, or brownout work.
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

    /// Report the current permission state, including uncertain write failures.
    pub fn state(&self) -> SafetyState {
        self.state
    }

    /// Re-read durable state and latch anything that is neither Clean nor Latched.
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

    /// Commit Working before operations that require recovery after interruption.
    pub fn begin_work(&mut self) -> Result<(), SafetyError<S::Error>> {
        if self.state != SafetyState::Clean {
            return Err(SafetyError::Blocked);
        }
        self.persist(SafetyState::Working)
    }

    /// Commit Clean after successful cleanup; never clear an existing latch.
    pub fn finish_work(&mut self) -> Result<(), SafetyError<S::Error>> {
        if self.state != SafetyState::Working {
            return Err(SafetyError::Blocked);
        }
        self.persist(SafetyState::Clean)
    }

    /// Persist a fault that blocks subsequent work.
    pub fn latch(&mut self) -> Result<(), SafetyError<S::Error>> {
        self.persist(SafetyState::Latched)
    }

    /// Clear a latch only after a qualified hold and qualified safe recovery voltage.
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

    /// Grant permission only after a successful durable write.
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
    /// Commit a diagnostic phase, blocking work if diagnostic persistence fails.
    pub fn set_phase(&mut self, phase: WorkPhase) -> Result<(), SafetyError<S::Error>> {
        if let Err(error) = self.store.phase(phase) {
            self.state = SafetyState::Latched;
            return Err(SafetyError::Storage(error));
        }
        Ok(())
    }
}
