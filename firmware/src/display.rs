//! Saved-frame rendering and ordered panel cleanup.

use std::time::Instant;

use crate::{frame, overlay::StatusOverlay, safety::WorkPhase};
use esp_idf_svc::sys::EspError;

use crate::{
    epd::Epd,
    frame_flash::FlashFrames,
    work::{self, Guard},
};

pub enum DisplayOutcome {
    /// The display refresh completed successfully.
    Displayed,
    /// The refresh failed, but panel cleanup completed and USB mode can continue.
    #[cfg(feature = "usb")]
    Failed(EspError),
}

/// Load validated storage and compose a live overlay without changing saved pixels.
pub fn refresh_saved(safety: &mut Guard) -> Result<DisplayOutcome, EspError> {
    work::set_phase(safety, WorkPhase::LoadFrame)?;
    let mut storage = FlashFrames::open()?;
    let scan = storage.load().map_err(|error| {
        log::error!(
            "LoadFrame failed: {error:?}; storage I/O errors are not a missing-frame fallback"
        );
        match error {
            frame::FrameError::Storage(error) => error,
            _ => EspError::from(esp_idf_svc::sys::ESP_ERR_INVALID_STATE).unwrap(),
        }
    })?;
    let metadata = scan
        .current
        .map(|record| record.metadata)
        .unwrap_or_default();
    // NVS writes disable the flash cache; mark the phase before rendering/mapping.
    work::set_phase(safety, WorkPhase::Display)?;
    let overlay = prepare_overlay(metadata)?;
    if let Some(record) = scan.current {
        let mapped = storage.map(record)?;
        log::info!("rendering validated native-order frame: {record:?}; live status overlay");
        refresh_with(safety, move |x, y| {
            overlay.pixel(x, y).unwrap_or_else(|| {
                frame::pixel(mapped.bytes(), x, y).expect("validated frame and panel coordinates")
            })
        })
    } else {
        refresh_with(safety, move |x, y| {
            overlay
                .pixel(x, y)
                .unwrap_or_else(|| frame::default_pixel(x, y))
        })
    }
}

/// Render the optional status overlay before enabling panel power.
fn prepare_overlay(metadata: frame::Metadata) -> Result<StatusOverlay, EspError> {
    if metadata.battery_indicator_enabled {
        log::info!("battery overlay: unknown (?%); calibrated measurement unavailable");
        log::info!(
            "battery overlay: corner={:?}, orientation={:?}",
            metadata.battery_corner,
            metadata.orientation
        );
    } else {
        log::info!("battery overlay: disabled");
    }
    StatusOverlay::new(metadata, None).map_err(|error| {
        log::error!("battery overlay rendering failed: {error}; panel remains OFF");
        EspError::from(esp_idf_svc::sys::ESP_ERR_INVALID_STATE).unwrap()
    })
}

/// Initialize and show pixels, dropping their source before NVS cleanup writes.
fn show(epd: &Epd, color_at: impl Fn(usize, usize) -> u8) -> Result<(), EspError> {
    log::info!("sending panel initialization sequence");
    epd.init()?;
    log::info!("panel initialization complete");
    log::info!("streaming image pixels");
    let start = Instant::now();
    epd.show(color_at)?;
    log::info!(
        "panel show sequence completed in {:.1} s",
        start.elapsed().as_secs_f32()
    );
    Ok(())
}

/// Always attempt power and SPI cleanup, even if marking cleanup fails.
fn cleanup(epd: Epd, safety: &mut Guard, displayed: bool) -> Result<(), EspError> {
    let phase = work::set_phase(safety, WorkPhase::PanelCleanup);
    let power_off = if displayed {
        epd.power_off()
    } else {
        log::warn!("display refresh failed; cutting panel power without another BUSY wait");
        Epd::disable_panel_power()
    };
    let closed = epd.close();
    phase?;
    if let Err(error) = power_off {
        log::error!("panel power-off failed: {error:?}; work blocked");
        if let Err(cleanup_error) = closed {
            log::error!("panel SPI cleanup also failed: {cleanup_error:?}");
        }
        return Err(error);
    }
    closed
}

/// Distinguish USB display failure from non-recoverable setup/cleanup failure.
fn refresh_with(
    safety: &mut Guard,
    color_at: impl Fn(usize, usize) -> u8,
) -> Result<DisplayOutcome, EspError> {
    log::info!("creating panel interface; panel load switch will turn on");
    let epd = Epd::new().inspect_err(|_| {
        if let Err(power_error) = Epd::disable_panel_power() {
            log::error!("panel setup failure power cutoff also failed: {power_error:?}");
        }
    })?;
    log::info!("panel interface ready");
    let refresh = show(&epd, color_at);
    if let Err(error) = refresh {
        log::error!("panel display operation failed: {error:?}");
    }
    cleanup(epd, safety, refresh.is_ok())?;
    conclude(refresh, safety)
}

usb_items! {
    /// Report a cleaned-up panel failure so USB mode can keep running.
    fn conclude(refresh: Result<(), EspError>, _: &mut Guard) -> Result<DisplayOutcome, EspError> {
        match refresh {
            Ok(()) => Ok(DisplayOutcome::Displayed),
            Err(error) => {
                log::error!(
                    "USB display refresh failed: {error:?}; panel OFF, SPI released; continuing execution"
                );
                Ok(DisplayOutcome::Failed(error))
            }
        }
    }
}

production_items! {
    /// Restore the Display phase as fault evidence before propagating a panel failure.
    fn conclude(refresh: Result<(), EspError>, safety: &mut Guard) -> Result<DisplayOutcome, EspError> {
        if refresh.is_err() {
            work::set_phase(safety, WorkPhase::Display)?;
        }
        refresh.map(|()| DisplayOutcome::Displayed)
    }
}
