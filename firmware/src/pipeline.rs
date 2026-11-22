//! Upload processing: verified save, physical display, then durable completion.

use std::time::Instant;

use crate::{
    display::{self, DisplayOutcome},
    epd::Epd,
    image_decode, rtc,
    upload_wifi::{ProcessingError, Received},
    work::{self, Guard},
};
use crate::{safety::WorkPhase, schedule::RefreshSchedule, upload::ProgressStage};

/// Convert image rejection into retry feedback, keeping storage faults fatal.
fn save(received: Received, deadline: Instant, safety: &mut Guard) -> Result<(), ProcessingError> {
    image_decode::save(received.buffer.reader(deadline), received.len,
        received.metadata, deadline, safety).map_err(|error| match error {
        image_decode::PipelineError::Rejected(error) => ProcessingError::Retry(format!(
            "Image could not be decoded: {error}. Saved picture and display are unchanged. Choose another image and retry."
        )),
        fault => ProcessingError::Fatal(format!(
            "Image save failed: {fault}. Display was not refreshed."
        )),
    })
}

/// Ensure a fatal display failure attempts emergency power cutoff.
fn refresh(safety: &mut Guard) -> Result<DisplayOutcome, ProcessingError> {
    display::refresh_saved(safety).map_err(|error| {
        if let Err(power_error) = Epd::disable_panel_power() {
            log::error!("display: emergency panel power cutoff failed: {power_error}");
            return ProcessingError::Fatal(format!(
                "Image saved, but display refresh failed: {error}. Emergency panel power cutoff also failed: {power_error}. Disconnect power."
            ));
        }
        ProcessingError::Fatal(format!(
            "Image saved, but display refresh failed: {error}. Panel power is off; no automatic retry."
        ))
    })
}

/// Commit Clean only after successful panel cleanup.
fn finish(safety: &mut Guard) -> Result<(), ProcessingError> {
    work::set_phase(safety, WorkPhase::SafetyCleanup).map_err(|error| {
        ProcessingError::Fatal(format!("Safety diagnostic cleanup failed: {error}"))
    })?;
    safety.finish_work().map_err(|error| {
        ProcessingError::Fatal(format!(
            "Image processing completed but safety cleanup failed: {error}."
        ))
    })
}

usb_items! {
    /// Close a failed USB refresh cleanly and schedule the next attempt.
    fn require_displayed(
        outcome: DisplayOutcome,
        safety: &mut Guard,
        schedule: &mut RefreshSchedule,
    ) -> Result<(), ProcessingError> {
        let DisplayOutcome::Failed(error) = outcome else {
            return Ok(());
        };
        finish(safety)?;
        schedule.defer_retry(rtc::now_us(), crate::schedule::USB_INTERVAL_US);
        Err(ProcessingError::DisplayFailed(format!(
            "Image saved, but display refresh failed: {error}. Panel power is off. USB operation continues; next scheduled attempt in 60 s."
        )))
    }
}

production_items! {
    /// Production display failures are already fatal errors, so only success remains.
    fn require_displayed(
        outcome: DisplayOutcome,
        _: &mut Guard,
        _: &mut RefreshSchedule,
    ) -> Result<(), ProcessingError> {
        let DisplayOutcome::Displayed = outcome;
        Ok(())
    }
}

/// Report progress and update the schedule only after actual display completion.
pub fn process(
    received: Received,
    deadline: Instant,
    progress: &mut dyn FnMut(ProgressStage),
    safety: &mut Guard,
    schedule: &mut RefreshSchedule,
) -> Result<&'static str, ProcessingError> {
    save(received, deadline, safety)?;
    progress(ProgressStage::Processed);
    if Instant::now() >= deadline {
        return Err(ProcessingError::Fatal(
            "Image saved, but awake deadline expired before display refresh.".into(),
        ));
    }
    log::info!("display: verified save; AP ON; starting REAL panel refresh");
    progress(ProgressStage::Displaying);
    require_displayed(refresh(safety)?, safety, schedule)?;
    progress(ProgressStage::Displayed);
    finish(safety)?;
    schedule.refresh_succeeded(rtc::now_us(), false);
    log::info!("display: SUCCESS; panel OFF; safety CLEAN; final result returning to browser");
    Ok("Success: image saved and display refresh completed. Panel power is off.")
}
