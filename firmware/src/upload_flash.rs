use std::io::{self, BufReader, Read, Seek, SeekFrom, Write};
use std::ptr::NonNull;
use std::time::Instant;

use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::sys::{self, esp, EspError};

const SECTOR_BYTES: usize = 4096;

pub struct FlashUpload {
    partition: NonNull<sys::esp_partition_t>,
    len: usize,
    deadline: Instant,
    bytes_since_yield: usize,
}

impl FlashUpload {
    pub fn open(deadline: Instant) -> Result<Self, EspError> {
        let partition = unsafe {
            sys::esp_partition_find_first(
                sys::esp_partition_type_t_ESP_PARTITION_TYPE_DATA,
                sys::esp_partition_subtype_t_ESP_PARTITION_SUBTYPE_DATA_UNDEFINED,
                c"upload".as_ptr(),
            )
        };
        let partition = NonNull::new(partition.cast_mut())
            .ok_or_else(|| EspError::from(sys::ESP_ERR_NOT_FOUND).unwrap())?;
        if (unsafe { partition.as_ref().size } as usize) < firmware::upload::MAX_UPLOAD_BYTES {
            log::error!("Upload partition too small; flash the updated partition table");
            return Err(EspError::from(sys::ESP_ERR_INVALID_SIZE).unwrap());
        }
        Ok(Self {
            partition,
            len: 0,
            deadline,
            bytes_since_yield: 0,
        })
    }

    fn check_deadline(&self) -> io::Result<()> {
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "upload flash deadline exceeded",
            ));
        }
        Ok(())
    }

    pub fn reader(&self, deadline: Instant) -> BufReader<UploadReader<'_>> {
        BufReader::with_capacity(
            SECTOR_BYTES,
            UploadReader {
                upload: self,
                position: 0,
                deadline,
                bytes_since_yield: 0,
            },
        )
    }
}

impl Write for FlashUpload {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.check_deadline()?;
        if bytes.len() > firmware::upload::MAX_UPLOAD_BYTES - self.len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "upload flash capacity exceeded",
            ));
        }
        let mut remaining = bytes;
        while !remaining.is_empty() {
            self.check_deadline()?;
            let within_sector = self.len % SECTOR_BYTES;
            if within_sector == 0 {
                unsafe {
                    esp!(sys::esp_partition_erase_range(
                        self.partition.as_ptr(),
                        self.len,
                        SECTOR_BYTES
                    ))
                }
                .map_err(io::Error::other)?;
            }
            let count = remaining.len().min(SECTOR_BYTES - within_sector);
            unsafe {
                esp!(sys::esp_partition_write(
                    self.partition.as_ptr(),
                    self.len,
                    remaining.as_ptr().cast(),
                    count
                ))
            }
            .map_err(io::Error::other)?;
            self.len += count;
            remaining = &remaining[count..];
            self.bytes_since_yield += count;
            if self.bytes_since_yield >= 64 * 1024 {
                self.bytes_since_yield = 0;
                FreeRtos::delay_ms(1);
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct UploadReader<'a> {
    upload: &'a FlashUpload,
    position: usize,
    deadline: Instant,
    bytes_since_yield: usize,
}

impl Read for UploadReader<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "upload flash read deadline exceeded",
            ));
        }
        let count = bytes
            .len()
            .min(self.upload.len.saturating_sub(self.position));
        if count == 0 {
            return Ok(0);
        }
        unsafe {
            esp!(sys::esp_partition_read(
                self.upload.partition.as_ptr(),
                self.position,
                bytes.as_mut_ptr().cast(),
                count
            ))
        }
        .map_err(io::Error::other)?;
        self.position += count;
        self.bytes_since_yield += count;
        if self.bytes_since_yield >= 64 * 1024 {
            self.bytes_since_yield = 0;
            FreeRtos::delay_ms(1);
        }
        Ok(count)
    }
}

impl Seek for UploadReader<'_> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let position = match from {
            SeekFrom::Start(position) => i64::try_from(position).ok(),
            SeekFrom::End(offset) => (self.upload.len as i64).checked_add(offset),
            SeekFrom::Current(offset) => (self.position as i64).checked_add(offset),
        }
        .and_then(|position| usize::try_from(position).ok());
        self.position = position
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid upload seek"))?;
        Ok(self.position as u64)
    }
}
