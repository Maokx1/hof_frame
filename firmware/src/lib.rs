macro_rules! espidf_items {
    ($($item:item)*) => {
        $(
            #[cfg(target_os = "espidf")]
            $item
        )*
    };
}

/// Compile the enclosed items only in USB development mode.
#[cfg(target_os = "espidf")]
macro_rules! usb_items {
    ($($item:item)*) => {
        $(
            #[cfg(feature = "usb")]
            $item
        )*
    };
}

/// Compile the enclosed items only in battery-powered production mode.
#[cfg(target_os = "espidf")]
macro_rules! production_items {
    ($($item:item)*) => {
        $(
            #[cfg(not(feature = "usb"))]
            $item
        )*
    };
}

espidf_items! {
    mod buttons;
    mod display;
    mod epd;
    mod frame_flash;
    mod image_decode;
    mod partition;
    mod pipeline;
    mod psram;
    mod rtc;
    mod safety_nvs;
    mod upload_flash;
    mod upload_wifi;
    mod work;
    pub mod app;
}

pub mod battery;
pub mod conversion;
pub mod dispatch;
pub mod frame;
pub mod gesture;
pub mod overlay;
pub mod panel_busy;
pub mod png_decode;
pub mod safety;
pub mod schedule;
pub mod upload;
