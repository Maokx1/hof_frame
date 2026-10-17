# hof_frame firmware

ESP-IDF-backed Rust firmware for the Good Display ESP32-133C02
(ESP32-S3-WROOM-1) controller, generated from
[`esp-rs/esp-idf-template`](https://github.com/esp-rs/esp-idf-template).
See `../docs/PLAN.md` for the full project plan and firmware milestones.

## Current status: display bring-up and button wake

The firmware logs over the USB serial port and draws a colour test card (six
colour stripes and text) on the 13.3" Spectra 6 panel, then powers the panel
down and enters deep sleep. The panel keeps the image without power. Holding
the received board's SW7 (GPIO13, active-high) for five seconds wakes the
controller and causes the image to be drawn again; shorter presses are
ignored. The driver is in
`src/epd.rs`, ported from Good Display's vendor example; the pin map and the
received-board button mapping are in `../docs/PLAN.md` under "Received board
findings".

A colour refresh takes tens of seconds. The native USB port disappears during
deep sleep; hold SW7 for five seconds to wake it, or unplug and re-plug the
cable if needed.

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
