use std::ptr::NonNull;
use std::time::Instant;

use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::sys::{self, EspError};

pub struct PsramBuffer {
    pointer: NonNull<u8>,
    len: usize,
}

impl PsramBuffer {
    /// Allocate and initialize bytes explicitly in external PSRAM.
    pub fn new(len: usize) -> Result<Self, EspError> {
        let caps = sys::MALLOC_CAP_SPIRAM | sys::MALLOC_CAP_8BIT;
        let pointer = NonNull::new(unsafe { sys::heap_caps_malloc(len, caps) }.cast::<u8>())
            .ok_or_else(|| EspError::from(sys::ESP_ERR_NO_MEM).unwrap())?;
        // Initialization keeps every byte of the exposed slice valid.
        unsafe { pointer.as_ptr().write_bytes(0, len) };
        Ok(Self { pointer, len })
    }

    /// Borrow the initialized allocation for decoding or verification.
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.pointer.as_ptr(), self.len) }
    }
}

impl Drop for PsramBuffer {
    /// Return the allocation to ESP-IDF's capability-aware heap.
    fn drop(&mut self) {
        unsafe { sys::heap_caps_free(self.pointer.as_ptr().cast()) };
    }
}

/// Verify physical PSRAM and Rust heap growth while large allocations coexist.
pub fn verify() -> Result<(), EspError> {
    let started = Instant::now();
    let caps = sys::MALLOC_CAP_SPIRAM | sys::MALLOC_CAP_8BIT;
    let physical = unsafe { sys::esp_psram_get_size() };
    let total = unsafe { sys::heap_caps_get_total_size(caps) };
    let free = unsafe { sys::heap_caps_get_free_size(caps) };
    log::info!("PSRAM: physical={physical} bytes, heap={total} bytes, free={free} bytes; N16R8 expects 8388608 physical bytes");
    if physical != 8 * 1024 * 1024 {
        return Err(EspError::from(sys::ESP_ERR_INVALID_SIZE).unwrap());
    }
    let mut buffer = PsramBuffer::new(4 * 1024 * 1024)?;
    verify_bytes(buffer.bytes_mut())?;
    verify_decoder_heap()?;
    drop(buffer);
    log::info!(
        "PSRAM: PASS; explicit 4194304-byte allocation write/read verified; buffer released; elapsed={} ms",
        started.elapsed().as_millis()
    );
    Ok(())
}

/// Write and read every physical byte, yielding every 64 KiB.
fn verify_bytes(buffer: &mut [u8]) -> Result<(), EspError> {
    // Volatile access makes this a physical RAM check, not an optimized comparison.
    for (block, bytes) in buffer.chunks_mut(4096).enumerate() {
        for (index, byte) in bytes.iter_mut().enumerate() {
            unsafe {
                std::ptr::write_volatile(byte, (block as u8).wrapping_mul(31) ^ index as u8);
            }
        }
        // Yield every 64 KiB; each nonzero delay costs at least one RTOS tick.
        if block % 16 == 15 {
            FreeRtos::delay_ms(1);
        }
    }
    for (block, bytes) in buffer.chunks(4096).enumerate() {
        for (index, byte) in bytes.iter().enumerate() {
            if unsafe { std::ptr::read_volatile(byte) }
                != ((block as u8).wrapping_mul(31) ^ index as u8)
            {
                return Err(EspError::from(sys::ESP_ERR_INVALID_STATE).unwrap());
            }
        }
        if block % 16 == 15 {
            FreeRtos::delay_ms(1);
        }
    }
    Ok(())
}

/// Ensure Rust Vec allocation/growth stays external and preserves existing bytes.
fn verify_decoder_heap() -> Result<(), EspError> {
    let _candidate = PsramBuffer::new(crate::frame::FRAME_BYTES)?;
    let mut workspace = Vec::<u8>::new();
    workspace.try_reserve_exact(128 * 1024).map_err(|error| {
        log::error!("PSRAM: Rust decoder heap allocation failed: {error}");
        EspError::from(sys::ESP_ERR_NO_MEM).unwrap()
    })?;
    workspace.resize(128 * 1024, 0x5a);
    if !unsafe { sys::esp_ptr_external_ram(workspace.as_ptr().cast()) } {
        log::error!(
            "PSRAM: 128 KiB Rust Vec is not external; decoder requires CONFIG_SPIRAM_USE_MALLOC"
        );
        return Err(EspError::from(sys::ESP_ERR_INVALID_STATE).unwrap());
    }
    workspace.try_reserve_exact(128 * 1024).map_err(|error| {
        log::error!("PSRAM: Rust decoder heap growth failed: {error}");
        EspError::from(sys::ESP_ERR_NO_MEM).unwrap()
    })?;
    if !unsafe { sys::esp_ptr_external_ram(workspace.as_ptr().cast()) }
        || workspace.iter().any(|byte| *byte != 0x5a)
    {
        log::error!("PSRAM: Rust decoder heap growth misplaced or corrupted the buffer");
        return Err(EspError::from(sys::ESP_ERR_INVALID_STATE).unwrap());
    }
    log::info!("PSRAM: Rust heap PASS; 128 KiB Vec allocated and grown to 256 KiB in PSRAM while 4 MiB input and 960000-byte candidate remain allocated");
    Ok(())
}
