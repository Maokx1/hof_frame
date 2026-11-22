//! Mode-specific state handlers; hardware work lives outside the entry point.

production_items! {
    mod production;
    pub use production::Application;
}

usb_items! {
    mod usb;
    pub use usb::Application;
}

use crate::{
    epd::EpdError, psram::PsramError, safety_nvs::NvsSafetyError, upload_wifi::UploadError,
};
use esp_idf_svc::sys::EspError;

/// States visible to the entry-point driver; each mode compiles only its own states.
pub enum State {
    /// Choose work for the current production wake.
    #[cfg(not(feature = "usb"))]
    Dispatch,
    /// Poll USB-mode buttons and scheduled refreshes.
    #[cfg(feature = "usb")]
    Idle,
    /// Classify the gesture that began at this time.
    #[cfg(feature = "usb")]
    Classify(std::time::Instant),
    /// Run the upload window using its original start time.
    Upload(std::time::Instant),
    /// Refresh the saved frame, optionally as a manual request.
    Refresh { manual: bool },
    /// Return to sleep or to idle waiting, depending on the mode.
    PrepareSleep,
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// Peripheral, GPIO, or sleep configuration failed.
    #[error("hardware setup failed: {0}")]
    Hardware(#[from] EspError),
    /// The panel power rail could not be switched off.
    #[error("panel power cutoff failed: {0}")]
    Panel(#[from] EpdError),
    /// Durable safety storage could not be opened or updated.
    #[error("{0}")]
    Safety(#[from] NvsSafetyError),
    /// External RAM failed its startup verification.
    #[error("PSRAM verification failed: {0}")]
    Psram(#[from] PsramError),
    /// The upload hotspot could not be configured or served.
    #[error("upload window failed: {0}")]
    Upload(#[from] UploadError),
    /// The single-use upload peripherals were already consumed this wake.
    #[cfg(not(feature = "usb"))]
    #[error("upload peripherals were already used this wake")]
    UploadHardwareUsed,
}

/// Link ESP-IDF runtime patches and initialize serial logging once.
fn initialize_runtime() {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
}
