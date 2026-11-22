use crate::safety::{
    self, Diagnostic, DiagnosticStore, FaultReason, SafetyError, SafetyGuard, SafetyStore,
    WorkPhase,
};
use esp_idf_svc::nvs::{EspDefaultNvs, EspDefaultNvsPartition};
use esp_idf_svc::sys::EspError;

pub struct NvsSafetyStore(EspDefaultNvs);

impl NvsSafetyStore {
    /// Load diagnostics, rejecting unsupported records instead of hiding them.
    fn diagnostic(&self) -> Result<Option<Diagnostic>, EspError> {
        let value = self.0.get_u32("diag_v1")?;
        match value {
            None => Ok(None),
            Some(value) => Diagnostic::from_record(value).map(Some).ok_or_else(|| {
                log::error!("safety diagnostics: invalid record {value:#010x}; work blocked");
                EspError::from(esp_idf_svc::sys::ESP_ERR_INVALID_STATE).unwrap()
            }),
        }
    }

    /// Record a fault at the last phase and preserve it as historical evidence.
    fn fault(&mut self, reason: FaultReason) -> Result<(), EspError> {
        let previous = self.diagnostic()?;
        let diagnostic = Diagnostic {
            phase: previous.map_or(WorkPhase::Unknown, |value| value.phase),
            reason,
        };
        self.0.set_u32("diag_v1", diagnostic.record())?;
        self.0.set_u32("last_fault_v1", diagnostic.record())?;
        log::error!(
            "safety diagnostics: fault={reason:?}, phase={:?}",
            diagnostic.phase
        );
        Ok(())
    }
}

impl DiagnosticStore for NvsSafetyStore {
    /// Commit the current phase without altering safety permission.
    fn phase(&mut self, phase: WorkPhase) -> Result<(), EspError> {
        let diagnostic = Diagnostic {
            phase,
            reason: FaultReason::None,
        };
        self.0.set_u32("diag_v1", diagnostic.record())?;
        log::info!("safety diagnostics: phase={phase:?}");
        Ok(())
    }
}

impl SafetyStore for NvsSafetyStore {
    type Error = EspError;

    /// Read and log the authoritative compatible safety byte.
    fn read(&mut self) -> Result<Option<u8>, EspError> {
        let value = self.0.get_u8("state_v1")?;
        match value {
            None => log::info!("safety NVS: no state_v1 record; establishing first-use Clean"),
            Some(0xA0) => log::info!("safety NVS: stored Clean (0xA0)"),
            Some(0xA1) => log::error!("safety NVS: stored Working (0xA1); interrupted work"),
            Some(0xA2) => {
                log::warn!("safety NVS: stored Latched (0xA2); see separate diagnostic record if available")
            }
            Some(value) => log::error!("safety NVS: unknown state_v1={value:#04x}"),
        }
        Ok(value)
    }

    /// Commit safety state, recording a runtime fault before a new latch.
    fn write(&mut self, value: u8) -> Result<(), EspError> {
        if value == 0xA2 {
            let diagnostic = self.diagnostic()?;
            if diagnostic.is_none_or(|value| value.reason == FaultReason::None) {
                self.fault(FaultReason::RuntimeFailure)?;
            }
        }
        self.0.set_u8("state_v1", value)?;
        log::info!("safety NVS: committed state_v1={value:#04x}");
        Ok(())
    }
}

/// Open production recovery without automatic partition erasure or acknowledgement.
pub fn open(
    namespace: &str,
    brownout: bool,
) -> Result<SafetyGuard<NvsSafetyStore>, SafetyError<EspError>> {
    open_store(namespace, brownout, false)
}

usb_items! {
    /// Start a fresh development session in the isolated USB namespace.
    pub fn open_usb_session(
        brownout: bool,
    ) -> Result<SafetyGuard<NvsSafetyStore>, SafetyError<EspError>> {
        open_store("hof_usb", brownout, true)
    }

    /// Log production fault evidence read-only without clearing or recovering it.
    pub fn log_production_diagnostics() -> Result<(), EspError> {
        let partition = EspDefaultNvsPartition::take_with(false)?;
        let nvs = EspDefaultNvs::new(partition, "hof_safety", false)?;
        let state = nvs.get_u8("state_v1")?;
        let diagnostic = nvs.get_u32("diag_v1")?;
        let last_fault = nvs.get_u32("last_fault_v1")?;
        log::info!(
            "production safety NVS snapshot (read-only): state={state:?}, diag_v1 raw={diagnostic:?} decoded={:?}, last_fault_v1 raw={last_fault:?} decoded={:?}",
            diagnostic.and_then(Diagnostic::from_record),
            last_fault.and_then(Diagnostic::from_record)
        );
        Ok(())
    }
}

/// Load diagnostic history, infer boot faults, and apply only explicit USB recovery.
fn open_store(
    namespace: &str,
    brownout: bool,
    reset_usb_session: bool,
) -> Result<SafetyGuard<NvsSafetyStore>, SafetyError<EspError>> {
    log::info!("safety NVS: opening namespace={namespace}; current reset brownout={brownout}");
    // Never erase a full/incompatible partition containing recovery metadata.
    let partition = EspDefaultNvsPartition::take_with(false).map_err(SafetyError::Storage)?;
    let nvs = EspDefaultNvs::new(partition, namespace, true).map_err(SafetyError::Storage)?;
    let mut store = NvsSafetyStore(nvs);
    let previous_state = store.read().map_err(SafetyError::Storage)?;
    let previous_diagnostic = store.diagnostic().map_err(SafetyError::Storage)?;
    log::info!(
        "safety diagnostics: previous state={previous_state:?}, work={previous_diagnostic:?}"
    );
    match store.0.get_u32("last_fault_v1").map_err(SafetyError::Storage)? {
        None => log::info!("safety diagnostics: no recorded historical fault (legacy causes cannot be reconstructed)"),
        Some(value) => {
            let diagnostic = Diagnostic::from_record(value).ok_or_else(|| {
                log::error!("safety diagnostics: invalid historical fault {value:#010x}");
                SafetyError::Storage(EspError::from(esp_idf_svc::sys::ESP_ERR_INVALID_STATE).unwrap())
            })?;
            log::warn!("safety diagnostics: last recorded fault={diagnostic:?}; no wall-clock timestamp");
        }
    }
    let reason = safety::boot_fault(previous_state, brownout);
    if let Some(reason) = reason {
        store.fault(reason).map_err(SafetyError::Storage)?;
    }
    if reset_usb_session {
        let previous = previous_state;
        log::warn!("USB development recovery: starting new session; previous state={previous:?}; resetting ONLY hof_usb to Clean; production recovery untouched; this is NOT battery-qualified acknowledgement");
        if previous != Some(0xA0) {
            store.write(0xA0).map_err(SafetyError::Storage)?;
        }
        store.phase(WorkPhase::Idle).map_err(SafetyError::Storage)?;
    }
    SafetyGuard::open(store, brownout && !reset_usb_session)
}
