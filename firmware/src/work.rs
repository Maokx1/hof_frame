//! Durable safety boundaries shared by display, upload, and startup.

use crate::safety::{SafetyError, SafetyGuard, SafetyState, WorkPhase};
use esp_idf_svc::{hal::delay::FreeRtos, sys::EspError};

pub type Guard = SafetyGuard<crate::safety_nvs::NvsSafetyStore>;

/// Commit a diagnostic boundary before starting the corresponding operation.
pub fn set_phase(safety: &mut Guard, phase: WorkPhase) -> Result<(), EspError> {
    safety.set_phase(phase).map_err(|error| {
        log::error!("safety diagnostics write failed: {error:?}; work blocked");
        match error {
            SafetyError::Storage(error) => error,
            SafetyError::Blocked => {
                EspError::from(esp_idf_svc::sys::ESP_ERR_INVALID_STATE).unwrap()
            }
        }
    })
}

/// Commit successful work cleanup without acknowledging an existing latch.
pub fn finish(safety: &mut Guard) {
    if safety.state() == SafetyState::Working {
        require(set_phase(safety, WorkPhase::SafetyCleanup));
        require(safety.finish_work());
    }
    require(set_phase(safety, WorkPhase::Idle));
}

/// Stop the session when a required safety operation fails.
pub fn require<T>(result: Result<T, impl std::fmt::Debug>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => halt(error),
    }
}

/// Latch a runtime fault and stop rather than retrying physical work.
pub fn latch_and_halt(safety: &mut Guard, error: impl std::fmt::Debug) -> ! {
    require(safety.latch());
    halt(error)
}

/// Keep USB/logging available while permanently blocking work this session.
pub fn halt(error: impl std::fmt::Debug) -> ! {
    loop {
        log::error!("FIRMWARE SAFETY FAULT: {error:?}; work blocked; staying awake, no retry");
        FreeRtos::delay_ms(5_000);
    }
}
