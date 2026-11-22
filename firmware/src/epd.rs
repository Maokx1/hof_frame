//! ED2208 driver for Seeed's 7.3-inch, 800x480 Spectra 6 panel on EE04.
//!
//! The pin map and command profile follow Seeed_GFX2's EE04 / ED2208 reference
//! implementation. Pixels are native 4-bit ED2208 transfer codes.

use std::ptr;
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::panel_busy::{
    BusyTimeout, RefreshBusy, ASSERT_TIMEOUT, COMPLETE_TIMEOUT, MIN_WAVEFORM_BUSY,
};
use esp_idf_svc::sys::{self, esp, EspError};

pub const WIDTH: usize = 800;
pub const HEIGHT: usize = 480;
const BYTES_PER_ROW: usize = WIDTH / 2;
const ROWS_PER_CHUNK: usize = 16;
const MAX_TRANSFER_BYTES: usize = 32_768;

pub const BLACK: u8 = 0;
pub const WHITE: u8 = 1;
pub const YELLOW: u8 = 2;
pub const RED: u8 = 3;
pub const BLUE: u8 = 5;
pub const GREEN: u8 = 6;

const PIN_CS: i32 = 44;
const PIN_DC: i32 = 10;
const PIN_RST: i32 = 38;
const PIN_MOSI: i32 = 9;
const PIN_SCK: i32 = 7;
const PIN_BUSY: i32 = 4;
const PIN_PANEL_ENABLE: i32 = 43;

const CMD_WRITE_RAM: u8 = 0x10;
const CMD_REFRESH: u8 = 0x12;
const CMD_POWER_ON: u8 = 0x04;
const CMD_POWER_OFF: u8 = 0x02;
const BUSY_TIMEOUT: Duration = Duration::from_secs(20);

// Command names follow the ED2208-GCA reference driver:
// https://github.com/zephyrproject-rtos/zephyr/blob/main/drivers/display/display_ed2208_gca.c
const INIT_SEQUENCE: &[(u8, &[u8])] = &[
    // Send the command header (CMDH).
    (0xAA, &[0x49, 0x55, 0x20, 0x08, 0x09, 0x18]),
    // Configure power settings (PWRR).
    (0x01, &[0x3F, 0x00, 0x32, 0x2A, 0x0E, 0x2A]),
    // Configure panel settings (PSR).
    (0x00, &[0x5F, 0x69]),
    // Configure the power-off sequence (POFS).
    (0x03, &[0x00, 0x54, 0x00, 0x44]),
    // Configure the first booster stage (BTST1).
    (0x05, &[0x40, 0x1F, 0x1F, 0x2C]),
    // Configure the second booster stage (BTST2).
    (0x06, &[0x6F, 0x1F, 0x16, 0x25]),
    // Configure the third booster stage (BTST3).
    (0x08, &[0x6F, 0x1F, 0x1F, 0x22]),
    // Set the IPC register.
    (0x13, &[0x00, 0x04]),
    // Configure the controller clock (PLL).
    (0x30, &[0x02]),
    // Set the TSE register.
    (0x41, &[0x00]),
    // Set the CDI register.
    (0x50, &[0x3F]),
    // Configure the timing controller (TCON).
    (0x60, &[0x02, 0x00]),
    // Set the resolution to 800 by 480 pixels (TRES).
    (0x61, &[0x03, 0x20, 0x01, 0xE0]),
    // Set the VDCS register.
    (0x82, &[0x1E]),
    // Set the T_VDCS register.
    (0x84, &[0x01]),
    // Set the AGID register.
    (0x86, &[0x00]),
    // Set the PWS register.
    (0xE3, &[0x2F]),
    // Set the CCSET register.
    (0xE0, &[0x00]),
    // Set the TSSET register.
    (0xE6, &[0x00]),
];

pub struct Epd {
    spi: sys::spi_device_handle_t,
}

#[derive(Debug, thiserror::Error)]
pub enum EpdError {
    /// A GPIO or SPI call into ESP-IDF failed.
    #[error("panel ESP-IDF call failed: {0}")]
    Esp(#[from] EspError),
    /// The controller stayed busy past the setup/power-operation timeout.
    #[error("panel BUSY did not return idle within {} s", BUSY_TIMEOUT.as_secs())]
    BusyStuck,
    /// The refresh waveform could not be confirmed.
    #[error("panel refresh not confirmed: {0}")]
    Waveform(#[from] BusyTimeout),
    /// A single SPI transaction exceeded the configured DMA limit.
    #[error("SPI transfer of {0} bytes exceeds {MAX_TRANSFER_BYTES}")]
    TransferTooLarge(usize),
    /// The pixel source has no color for a panel coordinate.
    #[error("no pixel color for panel coordinate ({x}, {y})")]
    MissingPixel { x: usize, y: usize },
}

/// Set an already configured GPIO to its command/data/power level.
fn set_level(pin: i32, high: bool) {
    unsafe { sys::gpio_set_level(pin, high as u32) };
}

/// Reset a GPIO and configure its initial output level.
fn output_pin(pin: i32, initial_high: bool) -> Result<(), EspError> {
    unsafe {
        esp!(sys::gpio_reset_pin(pin))?;
        esp!(sys::gpio_set_direction(
            pin,
            sys::gpio_mode_t_GPIO_MODE_OUTPUT
        ))?;
    }
    set_level(pin, initial_high);
    Ok(())
}

impl Epd {
    /// Cut the EE04 panel rail without waiting for the controller.
    pub fn disable_panel_power() -> Result<(), EpdError> {
        Ok(output_pin(PIN_PANEL_ENABLE, false)?)
    }

    /// Powers the panel, configures SPI, then resets the ED2208 controller.
    pub fn new() -> Result<Self, EpdError> {
        output_pin(PIN_CS, true)?;
        output_pin(PIN_DC, true)?;
        output_pin(PIN_RST, true)?;
        output_pin(PIN_PANEL_ENABLE, true)?;
        Self::configure_busy_pin()?;
        let spi = Self::open_spi()?;
        sleep(Duration::from_millis(50));
        let epd = Self { spi };
        epd.hardware_reset();
        Ok(epd)
    }

    /// Configure active-low BUSY as a floating input.
    fn configure_busy_pin() -> Result<(), EspError> {
        unsafe {
            esp!(sys::gpio_reset_pin(PIN_BUSY))?;
            esp!(sys::gpio_set_direction(
                PIN_BUSY,
                sys::gpio_mode_t_GPIO_MODE_INPUT
            ))?;
            esp!(sys::gpio_set_pull_mode(
                PIN_BUSY,
                sys::gpio_pull_mode_t_GPIO_FLOATING
            ))?;
        }
        Ok(())
    }

    /// Initialize the SPI bus/device, releasing the bus on device setup failure.
    fn open_spi() -> Result<sys::spi_device_handle_t, EspError> {
        let mut bus: sys::spi_bus_config_t = unsafe { std::mem::zeroed() };
        bus.__bindgen_anon_1.mosi_io_num = PIN_MOSI;
        bus.__bindgen_anon_2.miso_io_num = -1;
        bus.sclk_io_num = PIN_SCK;
        bus.__bindgen_anon_3.quadwp_io_num = -1;
        bus.__bindgen_anon_4.quadhd_io_num = -1;
        bus.max_transfer_sz = MAX_TRANSFER_BYTES as i32;
        bus.isr_cpu_id = sys::esp_intr_cpu_affinity_t_ESP_INTR_CPU_AFFINITY_1;

        let mut dev: sys::spi_device_interface_config_t = unsafe { std::mem::zeroed() };
        dev.clock_speed_hz = 10_000_000;
        dev.duty_cycle_pos = 128;
        dev.queue_size = 1;
        dev.spics_io_num = -1;

        let mut spi: sys::spi_device_handle_t = ptr::null_mut();
        unsafe {
            esp!(sys::spi_bus_initialize(
                sys::spi_host_device_t_SPI2_HOST,
                &bus,
                sys::spi_common_dma_t_SPI_DMA_CH_AUTO,
            ))?;
            if let Err(error) = esp!(sys::spi_bus_add_device(
                sys::spi_host_device_t_SPI2_HOST,
                &dev,
                &mut spi,
            )) {
                if let Err(cleanup_error) =
                    esp!(sys::spi_bus_free(sys::spi_host_device_t_SPI2_HOST))
                {
                    log::error!("SPI setup cleanup failed: {cleanup_error:?}");
                }
                set_level(PIN_PANEL_ENABLE, false);
                return Err(error);
            }
        }

        Ok(spi)
    }

    /// Pulse the controller's hardware reset pin.
    fn hardware_reset(&self) {
        set_level(PIN_RST, false);
        sleep(Duration::from_millis(20));
        set_level(PIN_RST, true);
        sleep(Duration::from_millis(10));
    }

    /// Transmit one bounded SPI transaction.
    fn transfer(&self, bytes: &[u8]) -> Result<(), EpdError> {
        if bytes.is_empty() {
            return Ok(());
        }
        if bytes.len() > MAX_TRANSFER_BYTES {
            return Err(EpdError::TransferTooLarge(bytes.len()));
        }
        let mut trans: sys::spi_transaction_t = unsafe { std::mem::zeroed() };
        trans.length = bytes.len() * 8;
        trans.__bindgen_anon_1.tx_buffer = bytes.as_ptr().cast();
        Ok(esp!(unsafe { sys::spi_device_transmit(self.spi, &mut trans) })?)
    }

    /// Select the controller and send a command before switching to data mode.
    fn begin_command(&self, command: u8) -> Result<(), EpdError> {
        set_level(PIN_CS, false);
        set_level(PIN_DC, false);
        if let Err(error) = self.transfer(&[command]) {
            set_level(PIN_CS, true);
            return Err(error);
        }
        set_level(PIN_DC, true);
        Ok(())
    }

    /// Send a command/data pair and release chip select even on failure.
    fn write_command(&self, command: u8, data: &[u8]) -> Result<(), EpdError> {
        self.begin_command(command)?;
        let result = self.transfer(data);
        set_level(PIN_CS, true);
        result
    }

    /// Wait for controller idle with the setup/power-operation timeout.
    fn wait_busy_high(&self) -> Result<(), EpdError> {
        let start = Instant::now();
        while unsafe { sys::gpio_get_level(PIN_BUSY) } == 0 {
            if start.elapsed() > BUSY_TIMEOUT {
                log::error!(
                    "panel BUSY GPIO{PIN_BUSY} did not return idle within {} s",
                    BUSY_TIMEOUT.as_secs()
                );
                return Err(EpdError::BusyStuck);
            }
            sleep(Duration::from_millis(10));
        }
        Ok(())
    }

    /// Require a complete, plausible BUSY waveform rather than initial idle.
    fn wait_refresh_complete(&self) -> Result<(), EpdError> {
        let start = Instant::now();
        let mut busy = RefreshBusy::default();
        let mut logged_assertion = false;
        loop {
            let idle = unsafe { sys::gpio_get_level(PIN_BUSY) } != 0;
            match busy.sample(start.elapsed(), idle) {
                Ok(true) => {
                    log::info!(
                        "ED2208 waveform BUSY cycle complete in {} ms",
                        start.elapsed().as_millis()
                    );
                    return Ok(());
                }
                Ok(false) => {
                    if !idle && !logged_assertion {
                        log::info!("ED2208 waveform BUSY asserted (GPIO{PIN_BUSY} LOW)");
                        logged_assertion = true;
                    }
                }
                Err(reason) => {
                    self.log_busy_timeout(&reason, start.elapsed());
                    return Err(reason.into());
                }
            }
            sleep(Duration::from_millis(10));
        }
    }

    /// Explain why the waveform could not be confirmed without retrying it.
    fn log_busy_timeout(&self, reason: &BusyTimeout, elapsed: Duration) {
        match reason {
            BusyTimeout::NeverAsserted => log::error!(
                "ED2208 waveform did not assert BUSY within {} s (GPIO{PIN_BUSY} stayed HIGH); \
                             refresh NOT confirmed; check panel power, cable and SPI/reset wiring",
                ASSERT_TIMEOUT.as_secs()
            ),
            BusyTimeout::NeverCompleted => log::error!(
                "ED2208 waveform BUSY did not return idle within {} s",
                COMPLETE_TIMEOUT.as_secs()
            ),
            BusyTimeout::ImplausiblyShort => log::error!(
                "ED2208 waveform ended after {} ms; BUSY LOW for less than {} ms; \
                             refresh NOT confirmed; no automatic retry",
                elapsed.as_millis(),
                MIN_WAVEFORM_BUSY.as_millis()
            ),
        }
    }

    /// Reset the controller, apply the profile, and power on its waveform engine.
    pub fn init(&self) -> Result<(), EpdError> {
        self.hardware_reset();
        for (command, data) in INIT_SEQUENCE {
            self.write_command(*command, data)?;
        }
        self.write_command(CMD_POWER_ON, &[])?;
        self.wait_busy_high()
    }

    /// Streams one full frame of native 4-bit ED2208 color codes and refreshes.
    pub fn show(&self, color_at: impl Fn(usize, usize) -> Option<u8>) -> Result<(), EpdError> {
        self.begin_command(CMD_WRITE_RAM)?;
        let result = self.stream_pixels(color_at);
        set_level(PIN_CS, true);
        result?;

        log::info!("ED2208 frame pixel streaming complete");
        self.write_command(CMD_REFRESH, &[0x00])?;
        self.wait_refresh_complete()
    }

    /// Pack and transmit every pixel in bounded row chunks while chip select is low.
    fn stream_pixels(&self, color_at: impl Fn(usize, usize) -> Option<u8>) -> Result<(), EpdError> {
        let color = |x, y| color_at(x, y).ok_or(EpdError::MissingPixel { x, y });
        let mut buffer = vec![0_u8; ROWS_PER_CHUNK * BYTES_PER_ROW];
        for first_row in (0..HEIGHT).step_by(ROWS_PER_CHUNK) {
            let rows = ROWS_PER_CHUNK.min(HEIGHT - first_row);
            for row in 0..rows {
                let y = first_row + row;
                for column in 0..BYTES_PER_ROW {
                    let x = column * 2;
                    buffer[row * BYTES_PER_ROW + column] = (color(x, y)? << 4) | color(x + 1, y)?;
                }
            }
            self.transfer(&buffer[..rows * BYTES_PER_ROW])?;
        }
        Ok(())
    }

    /// Sends the panel power-off sequence and disables the EE04 panel rail.
    pub fn power_off(&self) -> Result<(), EpdError> {
        let result = self.write_command(CMD_POWER_OFF, &[0x00]).and_then(|_| {
            sleep(Duration::from_millis(1));
            self.wait_busy_high()
        });
        sleep(Duration::from_millis(50));
        set_level(PIN_PANEL_ENABLE, false);
        result
    }

    /// Disable panel power and attempt both device removal and bus release.
    pub fn close(self) -> Result<(), EpdError> {
        set_level(PIN_PANEL_ENABLE, false);
        let remove_result = unsafe { esp!(sys::spi_bus_remove_device(self.spi)) };
        let free_result = unsafe { esp!(sys::spi_bus_free(sys::spi_host_device_t_SPI2_HOST)) };
        Ok(remove_result.and(free_result)?)
    }
}
