use std::marker::PhantomData;
use std::time::Instant;

use crate::frame::{self, FrameRecord, FrameStorage, Scan, STORAGE_BYTES};
use crate::partition::{self, FlashError};
use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::sys::{self, esp};

pub struct FlashFrames {
    partition: partition::Partition,
    operations: u8,
    deadline: Option<Instant>,
}

impl FlashFrames {
    /// Open only the expected two-slot frames partition.
    pub fn open() -> Result<Self, FlashError> {
        let partition = partition::find(c"frames", STORAGE_BYTES, |size| size == STORAGE_BYTES)?;
        Ok(Self {
            partition,
            operations: 0,
            deadline: None,
        })
    }

    /// Bound subsequent storage operations by the upload's awake budget.
    pub fn set_deadline(&mut self, deadline: Instant) {
        self.deadline = Some(deadline);
    }

    /// Reject operations that start after the configured deadline.
    fn check_deadline(&self) -> Result<(), FlashError> {
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(FlashError::DeadlineExceeded);
        }
        Ok(())
    }

    /// Yield after a bounded number of small storage operations.
    fn yield_periodically(&mut self) {
        self.operations += 1;
        if self.operations == 16 {
            self.operations = 0;
            FreeRtos::delay_ms(1);
        }
    }

    /// Scan both slots and log validation/fallback decisions.
    pub fn load(&mut self) -> Result<Scan, frame::FrameError<FlashError>> {
        let result = frame::scan(self)?;
        for (slot, status) in result.slots.iter().enumerate() {
            match status {
                frame::SlotStatus::Invalid(reason) => {
                    log::warn!("frame slot {slot} invalid: {reason:?}; not selected");
                }
                _ => log::info!("frame slot {slot}: {status:?}"),
            }
        }
        if result.current.is_none() {
            log::info!("LoadFrame: no valid stored frame; generated white default; status overlay applied at render time");
        }
        Ok(result)
    }

    /// Borrow a validated payload read-only until the mapping is dropped.
    pub fn map(&self, record: FrameRecord) -> Result<MappedFrame<'_>, FlashError> {
        if record.slot >= 2 {
            return Err(FlashError::InvalidSlot(record.slot));
        }
        let mut pointer = std::ptr::null();
        let mut handle = 0;
        unsafe {
            esp!(sys::esp_partition_mmap(
                self.partition.as_ptr(),
                record.payload_offset(),
                frame::FRAME_BYTES,
                sys::spi_flash_mmap_memory_t_SPI_FLASH_MMAP_DATA,
                &mut pointer,
                &mut handle,
            ))?;
        }
        Ok(MappedFrame {
            pointer: pointer.cast(),
            handle,
            _storage: PhantomData,
        })
    }
}

impl FrameStorage for FlashFrames {
    type Error = FlashError;

    /// Read partition bytes under the storage deadline.
    fn read(&mut self, offset: usize, bytes: &mut [u8]) -> Result<(), FlashError> {
        self.check_deadline()?;
        unsafe {
            esp!(sys::esp_partition_read(
                self.partition.as_ptr(),
                offset,
                bytes.as_mut_ptr().cast(),
                bytes.len()
            ))?;
        }
        self.yield_periodically();
        Ok(())
    }

    /// Erase one sector at a time, checking the deadline between sectors.
    fn erase(&mut self, offset: usize, len: usize) -> Result<(), FlashError> {
        for sector in (offset..offset + len).step_by(4096) {
            self.check_deadline()?;
            unsafe {
                esp!(sys::esp_partition_erase_range(
                    self.partition.as_ptr(),
                    sector,
                    4096
                ))?;
            }
            self.yield_periodically();
        }
        Ok(())
    }

    /// Write partition bytes and yield to the RTOS.
    fn write(&mut self, offset: usize, bytes: &[u8]) -> Result<(), FlashError> {
        self.check_deadline()?;
        unsafe {
            esp!(sys::esp_partition_write(
                self.partition.as_ptr(),
                offset,
                bytes.as_ptr().cast(),
                bytes.len()
            ))?;
        }
        FreeRtos::delay_ms(1);
        Ok(())
    }
}

pub struct MappedFrame<'a> {
    pointer: *const u8,
    handle: sys::esp_partition_mmap_handle_t,
    _storage: PhantomData<&'a FlashFrames>,
}

impl MappedFrame<'_> {
    /// Borrow native packed pixels without copying them into RAM.
    pub fn bytes(&self) -> &[u8] {
        // The immutable storage borrow prevents erasing/writing while mapped.
        unsafe { std::slice::from_raw_parts(self.pointer, frame::FRAME_BYTES) }
    }
}

impl Drop for MappedFrame<'_> {
    /// Release the flash mapping before any subsequent NVS/storage writes.
    fn drop(&mut self) {
        unsafe { sys::esp_partition_munmap(self.handle) };
    }
}
