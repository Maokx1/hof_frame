//! Minimal driver for the GDEP133C02 13.3" Spectra 6 panel on the Good Display
//! ESP32-133C02 board.
//!
//! The command set, init sequence and wire protocol are ported from Good
//! Display's vendor example (`GDEP133C02.c` / `comm.c`). The panel has two
//! driver ICs, each owning one half of the image and selected by its own CS
//! line. Pixels are 4-bit colour codes, two per byte, streamed row by row.

use std::ptr;
use std::thread::sleep;
use std::time::{Duration, Instant};

use esp_idf_svc::sys::{self, esp, EspError};

pub const WIDTH: usize = 1200;
pub const HEIGHT: usize = 1600;
const BYTES_PER_ROW: usize = WIDTH / 2;
const HALF_ROW_BYTES: usize = BYTES_PER_ROW / 2;
const ROWS_PER_CHUNK: usize = 16;

pub const BLACK: u8 = 0x0;
pub const WHITE: u8 = 0x1;
pub const YELLOW: u8 = 0x2;
pub const RED: u8 = 0x3;
pub const BLUE: u8 = 0x5;
pub const GREEN: u8 = 0x6;

const PIN_CS_M: i32 = 18;
const PIN_CS_S: i32 = 17;
const PIN_SCK: i32 = 9;
const PIN_MOSI: i32 = 41;
const PIN_MISO: i32 = 40;
const PIN_RST: i32 = 6;
const PIN_BUSY: i32 = 7;
/// Load switch that powers the panel and its driver circuitry.
const PIN_LOAD_SW: i32 = 45;

const CMD_PSR: u8 = 0x00;
const CMD_PWR: u8 = 0x01;
const CMD_POF: u8 = 0x02;
const CMD_PON: u8 = 0x04;
const CMD_BTST_N: u8 = 0x05;
const CMD_BTST_P: u8 = 0x06;
const CMD_DEEP_SLEEP: u8 = 0x07;
const CMD_DTM: u8 = 0x10;
const CMD_DRF: u8 = 0x12;
const CMD_CDI: u8 = 0x50;
const CMD_TCON: u8 = 0x60;
const CMD_TRES: u8 = 0x61;
const CMD_AN_TM: u8 = 0x74;
const CMD_AGID: u8 = 0x86;
const CMD_BUCK_BOOST_VDDN: u8 = 0xB0;
const CMD_TFT_VCOM_POWER: u8 = 0xB1;
const CMD_EN_BUF: u8 = 0xB6;
const CMD_BOOST_VDDP_EN: u8 = 0xB7;
const CMD_CCSET: u8 = 0xE0;
const CMD_PWS: u8 = 0xE3;
const CMD_CMD66: u8 = 0xF0;

const BUSY_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, PartialEq)]
enum Select {
    Main,
    Both,
}

pub struct Epd {
    spi: sys::spi_device_handle_t,
}

fn timeout_error() -> EspError {
    EspError::from(sys::ESP_ERR_TIMEOUT).expect("ESP_ERR_TIMEOUT is not ESP_OK")
}

fn set_level(pin: i32, high: bool) {
    unsafe { sys::gpio_set_level(pin, high as u32) };
}

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
    pub fn disable_panel_power() -> Result<(), EspError> {
        output_pin(PIN_LOAD_SW, false)
    }

    /// Powers the panel, configures the SPI bus and pulses the reset line.
    pub fn new() -> Result<Self, EspError> {
        output_pin(PIN_CS_M, true)?;
        output_pin(PIN_CS_S, true)?;
        output_pin(PIN_RST, true)?;
        output_pin(PIN_LOAD_SW, true)?;
        unsafe {
            esp!(sys::gpio_reset_pin(PIN_BUSY))?;
            esp!(sys::gpio_set_direction(
                PIN_BUSY,
                sys::gpio_mode_t_GPIO_MODE_INPUT
            ))?;
        }

        let mut bus: sys::spi_bus_config_t = unsafe { std::mem::zeroed() };
        bus.__bindgen_anon_1.mosi_io_num = PIN_MOSI;
        bus.__bindgen_anon_2.miso_io_num = PIN_MISO;
        bus.sclk_io_num = PIN_SCK;
        bus.__bindgen_anon_3.quadwp_io_num = -1;
        bus.__bindgen_anon_4.quadhd_io_num = -1;
        bus.max_transfer_sz = 32768;

        let mut dev: sys::spi_device_interface_config_t = unsafe { std::mem::zeroed() };
        dev.command_bits = 8;
        dev.clock_speed_hz = 10_000_000;
        dev.duty_cycle_pos = 128;
        dev.queue_size = 7;
        dev.spics_io_num = -1;

        let mut spi: sys::spi_device_handle_t = ptr::null_mut();
        unsafe {
            esp!(sys::spi_bus_initialize(
                sys::spi_host_device_t_SPI3_HOST,
                &bus,
                sys::spi_common_dma_t_SPI_DMA_CH_AUTO,
            ))?;
            if let Err(error) = esp!(sys::spi_bus_add_device(
                sys::spi_host_device_t_SPI3_HOST,
                &dev,
                &mut spi,
            )) {
                if let Err(cleanup_error) =
                    esp!(sys::spi_bus_free(sys::spi_host_device_t_SPI3_HOST))
                {
                    log::error!("SPI setup cleanup failed: {cleanup_error:?}");
                }
                return Err(error);
            }
        }

        // Let the panel supply settle after the load switch turns on.
        sleep(Duration::from_millis(50));
        let epd = Self { spi };
        epd.hardware_reset();
        Ok(epd)
    }

    fn hardware_reset(&self) {
        set_level(PIN_RST, false);
        sleep(Duration::from_millis(20));
        set_level(PIN_RST, true);
        sleep(Duration::from_millis(20));
    }

    fn select(&self, which: Select) {
        set_level(PIN_CS_M, false);
        set_level(PIN_CS_S, which != Select::Both);
    }

    fn select_secondary(&self) {
        set_level(PIN_CS_M, true);
        set_level(PIN_CS_S, false);
    }

    fn deselect(&self) {
        set_level(PIN_CS_M, true);
        set_level(PIN_CS_S, true);
    }

    fn wait_busy_high(&self) -> Result<(), EspError> {
        let start = Instant::now();
        while unsafe { sys::gpio_get_level(PIN_BUSY) } == 0 {
            if start.elapsed() > BUSY_TIMEOUT {
                log::error!(
                    "panel BUSY did not return idle within {} s",
                    BUSY_TIMEOUT.as_secs()
                );
                return Err(timeout_error());
            }
            sleep(Duration::from_millis(10));
        }
        Ok(())
    }

    /// Sends the 8-bit command phase followed by optional data.
    fn transmit_command(&self, command: u8, data: &[u8]) -> Result<(), EspError> {
        let mut trans: sys::spi_transaction_t = unsafe { std::mem::zeroed() };
        trans.cmd = command as u16;
        trans.length = data.len() * 8;
        if !data.is_empty() {
            trans.__bindgen_anon_1.tx_buffer = data.as_ptr().cast();
        }
        esp!(unsafe { sys::spi_device_transmit(self.spi, &mut trans) })
    }

    /// Continues the current CS-low frame with data only (no command phase).
    fn transmit_data(&self, data: &[u8]) -> Result<(), EspError> {
        let mut trans: sys::spi_transaction_ext_t = unsafe { std::mem::zeroed() };
        trans.base.flags = sys::SPI_TRANS_VARIABLE_CMD;
        trans.base.length = data.len() * 8;
        trans.base.__bindgen_anon_1.tx_buffer = data.as_ptr().cast();
        esp!(unsafe { sys::spi_device_transmit(self.spi, &mut trans.base) })
    }

    fn write(&self, which: Select, command: u8, data: &[u8]) -> Result<(), EspError> {
        self.select(which);
        let result = self.transmit_command(command, data);
        self.deselect();
        result
    }

    pub fn init(&self) -> Result<(), EspError> {
        self.hardware_reset();
        self.wait_busy_high()?;

        use Select::{Both, Main};
        self.write(
            Main,
            CMD_AN_TM,
            &[0xC0, 0x1C, 0x1C, 0xCC, 0xCC, 0xCC, 0x15, 0x15, 0x55],
        )?;
        self.write(Both, CMD_CMD66, &[0x49, 0x55, 0x13, 0x5D, 0x05, 0x10])?;
        self.write(Both, CMD_PSR, &[0xDF, 0x69])?;
        self.write(Both, CMD_CDI, &[0xF7])?;
        self.write(Both, CMD_TCON, &[0x03, 0x03])?;
        self.write(Both, CMD_AGID, &[0x10])?;
        self.write(Both, CMD_PWS, &[0x22])?;
        self.write(Both, CMD_CCSET, &[0x01])?;
        self.write(Both, CMD_TRES, &[0x04, 0xB0, 0x03, 0x20])?;
        self.write(Main, CMD_PWR, &[0x0F, 0x00, 0x28, 0x2C, 0x28, 0x38])?;
        self.write(Main, CMD_EN_BUF, &[0x07])?;
        self.write(Main, CMD_BTST_P, &[0xE8, 0x28])?;
        self.write(Main, CMD_BOOST_VDDP_EN, &[0x01])?;
        self.write(Main, CMD_BTST_N, &[0xE8, 0x28])?;
        self.write(Main, CMD_BUCK_BOOST_VDDN, &[0x01])?;
        self.write(Main, CMD_TFT_VCOM_POWER, &[0x02])
    }

    /// Streams one full frame produced by `color_at(x, y)` (a 4-bit colour code)
    /// and refreshes the panel. Takes tens of seconds for a colour refresh.
    pub fn show(&self, color_at: impl Fn(usize, usize) -> u8) -> Result<(), EspError> {
        let mut buf = vec![0u8; ROWS_PER_CHUNK * HALF_ROW_BYTES];

        for half in 0..2 {
            if half == 0 {
                self.select(Select::Main);
            } else {
                self.select_secondary();
            }
            let result = self.stream_half(half, &mut buf, &color_at);
            self.deselect();
            result?;
        }
        log::info!("frame pixel streaming complete");
        self.refresh()
    }

    fn stream_half(
        &self,
        half: usize,
        buf: &mut [u8],
        color_at: &impl Fn(usize, usize) -> u8,
    ) -> Result<(), EspError> {
        self.transmit_command(CMD_DTM, &[])?;
        for first_row in (0..HEIGHT).step_by(ROWS_PER_CHUNK) {
            let rows = ROWS_PER_CHUNK.min(HEIGHT - first_row);
            for r in 0..rows {
                let y = first_row + r;
                for b in 0..HALF_ROW_BYTES {
                    let x = 2 * (half * HALF_ROW_BYTES + b);
                    buf[r * HALF_ROW_BYTES + b] = (color_at(x, y) << 4) | color_at(x + 1, y);
                }
            }
            self.transmit_data(&buf[..rows * HALF_ROW_BYTES])?;
        }
        Ok(())
    }

    fn refresh(&self) -> Result<(), EspError> {
        log::info!("sending panel power-on command");
        self.select(Select::Both);
        let result = self
            .transmit_command(CMD_PON, &[])
            .and_then(|_| self.wait_busy_high());
        self.deselect();
        result?;

        self.select(Select::Both);
        sleep(Duration::from_millis(30));
        log::info!("starting display waveform; waiting for panel BUSY to become idle");
        let result = self
            .transmit_command(CMD_DRF, &[0x01])
            .and_then(|_| self.wait_busy_high());
        self.deselect();
        result?;

        self.select(Select::Both);
        let result = self
            .transmit_command(CMD_POF, &[0x00])
            .and_then(|_| self.wait_busy_high());
        self.deselect();
        result
    }

    /// Puts the panel controllers into deep sleep and cuts panel power.
    pub fn power_off(&self) -> Result<(), EspError> {
        let result = self.write(Select::Both, CMD_DEEP_SLEEP, &[0xA5]);
        sleep(Duration::from_millis(50));
        set_level(PIN_LOAD_SW, false);
        result
    }

    pub fn close(self) -> Result<(), EspError> {
        set_level(PIN_LOAD_SW, false);
        unsafe {
            esp!(sys::spi_bus_remove_device(self.spi))?;
            esp!(sys::spi_bus_free(sys::spi_host_device_t_SPI3_HOST))?;
        }
        Ok(())
    }
}
