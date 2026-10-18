use std::marker::PhantomData;
use std::ptr::NonNull;
use std::time::Instant;

use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::sys::{self, esp, EspError};
use firmware::frame::{self, FrameRecord, FrameStorage, Scan, STORAGE_BYTES};

pub struct FlashFrames {
    partition: NonNull<sys::esp_partition_t>,
    operations: u8,
    deadline: Option<Instant>,
}

impl FlashFrames {
    pub fn open() -> Result<Self, EspError> {
        let partition = unsafe {
            sys::esp_partition_find_first(
                sys::esp_partition_type_t_ESP_PARTITION_TYPE_DATA,
                sys::esp_partition_subtype_t_ESP_PARTITION_SUBTYPE_DATA_UNDEFINED,
                c"frames".as_ptr(),
            )
        };
        let partition = NonNull::new(partition.cast_mut())
            .ok_or_else(|| EspError::from(sys::ESP_ERR_NOT_FOUND).unwrap())?;
        if unsafe { partition.as_ref().size } as usize != STORAGE_BYTES {
            return Err(EspError::from(sys::ESP_ERR_INVALID_SIZE).unwrap());
        }
        Ok(Self {
            partition,
            operations: 0,
            deadline: None,
        })
    }

    pub fn set_deadline(&mut self, deadline: Instant) {
        self.deadline = Some(deadline);
    }

    fn check_deadline(&self) -> Result<(), EspError> {
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(EspError::from(sys::ESP_ERR_TIMEOUT).unwrap());
        }
        Ok(())
    }

    fn yield_periodically(&mut self) {
        self.operations += 1;
        if self.operations == 16 {
            self.operations = 0;
            FreeRtos::delay_ms(1);
        }
    }

    pub fn load(&mut self) -> Result<Scan, frame::FrameError<EspError>> {
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

    pub fn map(&self, record: FrameRecord) -> Result<MappedFrame<'_>, EspError> {
        if record.slot >= 2 {
            return Err(EspError::from(sys::ESP_ERR_INVALID_ARG).unwrap());
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
    type Error = EspError;

    fn read(&mut self, offset: usize, bytes: &mut [u8]) -> Result<(), EspError> {
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

    fn erase(&mut self, offset: usize, len: usize) -> Result<(), EspError> {
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

    fn write(&mut self, offset: usize, bytes: &[u8]) -> Result<(), EspError> {
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
    pub fn bytes(&self) -> &[u8] {
        // The immutable storage borrow prevents erasing/writing while mapped.
        unsafe { std::slice::from_raw_parts(self.pointer, frame::FRAME_BYTES) }
    }
}

impl Drop for MappedFrame<'_> {
    fn drop(&mut self) {
        unsafe { sys::esp_partition_munmap(self.handle) };
    }
}
