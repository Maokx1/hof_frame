//! RTC clock and production scheduling retained across deep sleep.

use crate::schedule::{RefreshSchedule, DAILY_INTERVAL_US};
use esp_idf_svc::hal::reset::ResetReason;

const RETAINED_MAGIC: u32 = 0x484F_4632;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct RetainedState {
    magic: u32,
    pub schedule: RefreshSchedule,
    pub refresh_in_progress: u32,
}

#[link_section = ".rtc.data"]
static mut RETAINED_STATE: RetainedState = RetainedState {
    magic: 0,
    schedule: RefreshSchedule::new(0),
    refresh_in_progress: 0,
};

extern "C" {
    /// ESP-IDF's microsecond RTC clock, retained through deep sleep.
    fn esp_rtc_get_time_us() -> i64;
}

/// Read the RTC clock, which continues across deep sleep unlike `esp_timer`.
pub fn now_us() -> u64 {
    unsafe { esp_rtc_get_time_us().max(0) as u64 }
}

/// Copy the retained record using volatile access.
pub fn retained() -> RetainedState {
    unsafe { std::ptr::addr_of!(RETAINED_STATE).read_volatile() }
}

/// Replace the retained record after applying a single local update.
pub fn update(update: impl FnOnce(&mut RetainedState)) {
    unsafe {
        let state = std::ptr::addr_of_mut!(RETAINED_STATE);
        let mut value = state.read_volatile();
        update(&mut value);
        state.write_volatile(value);
    }
}

/// Initialize cold-boot memory without changing a valid retained schedule.
pub fn initialize(now_us: u64) {
    update(|state| {
        if state.magic != RETAINED_MAGIC {
            *state = RetainedState {
                magic: RETAINED_MAGIC,
                schedule: RefreshSchedule::new(now_us),
                refresh_in_progress: 0,
            };
        }
    });
}

/// Defer physical retries after interrupted work or brownout.
pub fn recover_interrupted(now_us: u64, reset_reason: ResetReason) -> bool {
    let interrupted = retained().refresh_in_progress != 0 || reset_reason == ResetReason::Brownout;
    if interrupted {
        update(|state| {
            state.refresh_in_progress = 0;
            state.schedule.defer_retry(now_us, DAILY_INTERVAL_US);
        });
    }
    interrupted
}

/// Retain upload completion before reporting success to the browser.
pub fn upload_succeeded(schedule: RefreshSchedule) {
    update(|state| {
        state.schedule = schedule;
        state.refresh_in_progress = 0;
    });
}
