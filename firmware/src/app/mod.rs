//! Mode-specific state handlers; hardware work lives outside the entry point.

production_items! {
    mod production;
    pub use production::Application;
}

usb_items! {
    mod usb;
    pub use usb::Application;
}

/// States visible to the entry-point driver in either operating mode.
pub enum State {
    /// Choose work for the current production wake.
    Dispatch,
    /// Poll USB-mode buttons and scheduled refreshes.
    Idle,
    /// Classify the gesture that began at this time.
    Classify(std::time::Instant),
    /// Run the upload window using its original start time.
    Upload(std::time::Instant),
    /// Refresh the saved frame, optionally as a manual request.
    Refresh { manual: bool },
    /// Return to sleep or to idle waiting, depending on the mode.
    PrepareSleep,
}

/// Link ESP-IDF runtime patches and initialize serial logging once.
fn initialize_runtime() {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
}
