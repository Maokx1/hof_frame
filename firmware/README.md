# hof_frame firmware

ESP-IDF-backed Rust firmware for the Good Display ESP32-133C02
(ESP32-S3-WROOM-1) controller, generated from
[`esp-rs/esp-idf-template`](https://github.com/esp-rs/esp-idf-template).
See the [project plan](../docs/PLAN.md) for the full project plan and firmware
milestones, and [firmware state machine](STATE_MACHINE.md) for the target state
diagrams, required firmware components, recovery rules, and acceptance checks.

## Current status: display bring-up

The current firmware logs over the USB serial port, draws
[`assets/sunflower.bin`](assets/sunflower.bin) (860 x 953, centred on white) on
the 13.3" Spectra 6 panel, powers the panel down and then idles. The panel
keeps the image without power. The driver is in [src/epd.rs](src/epd.rs),
ported from Good Display's vendor example; the pin map and received-board
button mapping are in the
[received board findings](../docs/PLAN.md#received-board-findings).

Button wake, the 24-hour RTC refresh and deep sleep are temporarily removed
while display power is debugged. Every reset redraws the image, so a supply
that browns out during refresh causes a refresh loop.

This is an intermediate bring-up milestone, not battery-ready behavior. There
is no Wi-Fi upload, persistent photo, white default frame, battery measurement
or low-battery latch yet. A colour refresh takes tens of seconds.

## Target firmware

The target is a battery-guarded, wake-to-work state machine:

- A qualified SW7 wake (five-second hold) opens a WPA2 access point and a
  bounded PNG/JPEG upload window.
- Three quick SW7 presses trigger a manual refresh of the persisted frame,
  with Wi-Fi off.
- A nominal 24-hour RTC wake refreshes the last valid persisted frame without
  starting Wi-Fi.
- Every cycle checks battery safety before radio, storage writes, or refresh.
  Low-battery and interrupted-work recovery require a persistent latch.
- Upload, decode/dither, atomic frame storage, and panel refresh remain separate
  stages; HTTP `200 OK` means received, not displayed.
- All normal and recoverable-error exits shut down peripherals and prepare
  deep sleep, with a fresh 24-hour timer and safe button-release handling.

These are requirements, not implemented features. The
[state-machine specification](STATE_MACHINE.md) records the implementation
order and decisions still needed before enabling battery operation.

## Prerequisites

- The `esp` Rust toolchain installed via [`espup`](https://github.com/esp-rs/espup)
  (`espup install`, then `. ~/export-esp.sh` in each new shell).
- `ldproxy` and `espflash`/`cargo-espflash` (`cargo install ldproxy espflash cargo-espflash`).
- `cmake`, `ninja`, `git`, and Python 3 for the managed ESP-IDF toolchain that
  `esp-idf-sys` downloads on first build (into `.embuild/`, gitignored).

## Build

```sh
cd firmware
cargo build --release
```

The first build downloads and builds ESP-IDF v5.5.3 and its toolchain into
`.embuild/`; this can take a long time and several GB of disk space. Later
builds reuse the cached `.embuild/` directory.

## Flash and monitor

With the board connected over USB:

```sh
cargo run --release
```

This uses the `espflash flash --monitor` runner configured in
`.cargo/config.toml`.
