use std::time::Instant;

use crate::{
    frame::{self, FrameError},
    png_decode::{self, DecodeError},
};
use esp_idf_svc::hal::delay::FreeRtos;

use crate::frame_flash::FlashFrames;
use crate::partition::FlashError;
use crate::psram::{PsramBuffer, PsramError};
use crate::safety_nvs::NvsSafetyError;

#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    /// The image cannot be decoded; another upload may be attempted.
    #[error("{0}")]
    Rejected(DecodeError),
    /// A safety diagnostic boundary could not be committed.
    #[error("{0}")]
    Safety(#[from] NvsSafetyError),
    /// The PSRAM candidate buffer could not be allocated.
    #[error("{0}")]
    Memory(#[from] PsramError),
    /// The staged upload could not be read back from flash.
    #[error("upload flash read failed: {0}")]
    StagingRead(std::io::Error),
    /// The frames partition could not be opened.
    #[error("{0}")]
    Flash(#[from] FlashError),
    /// The decoded frame could not be committed.
    #[error("frame commit failed: {0}")]
    Commit(FrameError<FlashError>),
    /// Stored frames could not be reloaded after the commit.
    #[error("frame reload failed: {0}")]
    Reload(FrameError<FlashError>),
    /// Reloading storage did not select the newly committed frame.
    #[error("committed frame was not selected on reload")]
    NotSelected,
}

impl From<DecodeError> for PipelineError {
    /// Separate staging-flash read faults from invalid image content.
    fn from(error: DecodeError) -> Self {
        match error {
            DecodeError::Png(png::DecodingError::IoError(io))
                if !matches!(
                    io.kind(),
                    std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::TimedOut
                ) =>
            {
                Self::StagingRead(io)
            }
            error => Self::Rejected(error),
        }
    }
}

/// Decode into a bounded candidate, then publish and verify an inactive frame slot.
pub fn save(
    encoded: impl std::io::BufRead + std::io::Seek,
    encoded_len: usize,
    metadata: frame::Metadata,
    deadline: Instant,
    safety: &mut crate::safety::SafetyGuard<crate::safety_nvs::NvsSafetyStore>,
) -> Result<(), PipelineError> {
    crate::work::set_phase(safety, crate::safety::WorkPhase::Decode)?;
    log::info!(
        "Upload pipeline: decoding {} encoded bytes by rows; {metadata:?}; AP ON; panel OFF",
        encoded_len
    );
    let mut packed = PsramBuffer::new(frame::FRAME_BYTES)?;
    log_decoder_heap();
    decode(encoded, encoded_len, metadata, packed.bytes_mut(), deadline)?;
    if Instant::now() >= deadline {
        return Err(PipelineError::Rejected(DecodeError::DeadlineExceeded));
    }
    log::info!(
        "Upload pipeline: conversion complete; committing 960000 packed bytes to inactive slot"
    );
    crate::work::set_phase(safety, crate::safety::WorkPhase::Save)?;
    commit(packed.bytes_mut(), metadata, deadline)
}

/// Log both memory pools before invoking the row decoder.
fn log_decoder_heap() {
    let internal_caps = esp_idf_svc::sys::MALLOC_CAP_INTERNAL | esp_idf_svc::sys::MALLOC_CAP_8BIT;
    let external_caps = esp_idf_svc::sys::MALLOC_CAP_SPIRAM | esp_idf_svc::sys::MALLOC_CAP_8BIT;
    log::info!(
        "Upload pipeline: decoder heap available: internal={} bytes (largest={}), PSRAM={} bytes (largest={})",
        unsafe { esp_idf_svc::sys::heap_caps_get_free_size(internal_caps) },
        unsafe { esp_idf_svc::sys::heap_caps_get_largest_free_block(internal_caps) },
        unsafe { esp_idf_svc::sys::heap_caps_get_free_size(external_caps) },
        unsafe { esp_idf_svc::sys::heap_caps_get_largest_free_block(external_caps) },
    );
}

/// Convert by rows and distinguish invalid input from staging read faults.
fn decode(
    encoded: impl std::io::BufRead + std::io::Seek,
    encoded_len: usize,
    metadata: frame::Metadata,
    packed: &mut [u8],
    deadline: Instant,
) -> Result<(), PipelineError> {
    png_decode::decode_reader(encoded, encoded_len, metadata, packed, || {
        if Instant::now() >= deadline {
            return Err(DecodeError::DeadlineExceeded);
        }
        FreeRtos::delay_ms(1);
        Ok(())
    })?;
    Ok(())
}

/// Publish a candidate only after readback and ensure reload selects that record.
fn commit(
    packed: &[u8],
    metadata: frame::Metadata,
    deadline: Instant,
) -> Result<(), PipelineError> {
    let mut storage = FlashFrames::open()?;
    storage.set_deadline(deadline);
    let record = frame::commit(&mut storage, packed, metadata).map_err(PipelineError::Commit)?;
    let selected = storage.load().map_err(PipelineError::Reload)?.current;
    if selected != Some(record) {
        return Err(PipelineError::NotSelected);
    }
    log::info!(
        "Upload pipeline: SAVED {record:?}; dithered native frame; panel OFF; no display refresh"
    );
    Ok(())
}
