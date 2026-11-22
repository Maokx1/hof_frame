//! Named flash-partition lookup shared by frame and upload storage.

use std::ffi::CStr;
use std::ptr::NonNull;

use esp_idf_svc::sys::{self, EspError};

pub type Partition = NonNull<sys::esp_partition_t>;

#[derive(Debug, thiserror::Error)]
pub enum FlashError {
    /// The partition table has no data partition with this label.
    #[error("flash partition {0:?} not found; flash the updated partition table")]
    Missing(&'static CStr),
    /// The partition exists but its size cannot hold the expected data.
    #[error("flash partition {label:?} has {actual} bytes; expected {expected}")]
    WrongSize {
        label: &'static CStr,
        actual: usize,
        expected: usize,
    },
    /// The storage operation started after its deadline.
    #[error("flash operation deadline exceeded")]
    DeadlineExceeded,
    /// The requested frame slot does not exist.
    #[error("frame slot {0} does not exist")]
    InvalidSlot(usize),
    /// An ESP-IDF partition call failed.
    #[error("flash partition call failed: {0}")]
    Esp(#[from] EspError),
}

/// Find a data partition by label and require that `fits` accepts its size.
pub fn find(
    label: &'static CStr,
    expected: usize,
    fits: impl Fn(usize) -> bool,
) -> Result<Partition, FlashError> {
    let partition = unsafe {
        sys::esp_partition_find_first(
            sys::esp_partition_type_t_ESP_PARTITION_TYPE_DATA,
            sys::esp_partition_subtype_t_ESP_PARTITION_SUBTYPE_DATA_UNDEFINED,
            label.as_ptr(),
        )
    };
    let partition = NonNull::new(partition.cast_mut()).ok_or(FlashError::Missing(label))?;
    let actual = unsafe { partition.as_ref().size } as usize;
    if !fits(actual) {
        return Err(FlashError::WrongSize {
            label,
            actual,
            expected,
        });
    }
    Ok(partition)
}
