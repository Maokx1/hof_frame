use std::time::Instant;

use esp_idf_svc::hal::delay::FreeRtos;
use firmware::{frame, png_decode};

use crate::frame_flash::FlashFrames;
use crate::psram::PsramBuffer;

#[derive(Debug)]
pub enum PipelineError {
    Rejected(Box<dyn std::error::Error>),
    Fault(String),
}

pub fn save(
    encoded: &[u8],
    metadata: frame::Metadata,
    deadline: Instant,
    safety: &mut firmware::safety::SafetyGuard<crate::safety_nvs::NvsSafetyStore>,
) -> Result<(), PipelineError> {
    crate::set_work_phase(safety, firmware::safety::WorkPhase::Decode)
        .map_err(|error| PipelineError::Fault(error.to_string()))?;
    log::info!(
        "Upload pipeline: decoding {} encoded bytes by rows; {metadata:?}; AP ON; panel OFF",
        encoded.len()
    );
    let mut packed = PsramBuffer::new(frame::FRAME_BYTES)
        .map_err(|error| PipelineError::Fault(error.to_string()))?;
    let internal_caps = esp_idf_svc::sys::MALLOC_CAP_INTERNAL | esp_idf_svc::sys::MALLOC_CAP_8BIT;
    let external_caps = esp_idf_svc::sys::MALLOC_CAP_SPIRAM | esp_idf_svc::sys::MALLOC_CAP_8BIT;
    log::info!(
        "Upload pipeline: decoder heap available: internal={} bytes (largest={}), PSRAM={} bytes (largest={})",
        unsafe { esp_idf_svc::sys::heap_caps_get_free_size(internal_caps) },
        unsafe { esp_idf_svc::sys::heap_caps_get_largest_free_block(internal_caps) },
        unsafe { esp_idf_svc::sys::heap_caps_get_free_size(external_caps) },
        unsafe { esp_idf_svc::sys::heap_caps_get_largest_free_block(external_caps) },
    );
    png_decode::decode(encoded, metadata, packed.bytes_mut(), || {
        if Instant::now() >= deadline {
            return Err("decode/conversion deadline exceeded");
        }
        FreeRtos::delay_ms(1);
        Ok(())
    })
    .map_err(PipelineError::Rejected)?;
    if Instant::now() >= deadline {
        return Err(PipelineError::Rejected("commit deadline exceeded".into()));
    }
    log::info!(
        "Upload pipeline: conversion complete; committing 960000 packed bytes to inactive slot"
    );
    crate::set_work_phase(safety, firmware::safety::WorkPhase::Save)
        .map_err(|error| PipelineError::Fault(error.to_string()))?;
    let mut storage =
        FlashFrames::open().map_err(|error| PipelineError::Fault(error.to_string()))?;
    storage.set_deadline(deadline);
    let record = frame::commit(&mut storage, packed.bytes_mut(), metadata)
        .map_err(|error| PipelineError::Fault(format!("frame commit failed: {error:?}")))?;
    let selected = storage
        .load()
        .map_err(|error| PipelineError::Fault(format!("frame reload failed: {error:?}")))?
        .current;
    if selected != Some(record) {
        return Err(PipelineError::Fault(
            "committed frame was not selected on reload".into(),
        ));
    }
    log::info!(
        "Upload pipeline: SAVED {record:?}; dithered native frame; panel OFF; no display refresh"
    );
    Ok(())
}
