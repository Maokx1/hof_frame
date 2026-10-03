# hof_frame firmware

ESP-IDF-backed Rust firmware for the Good Display ESP32-133C02
(ESP32-S3-WROOM-1) controller, generated from
[`esp-rs/esp-idf-template`](https://github.com/esp-rs/esp-idf-template).
See `../docs/PLAN.md` for the full project plan and firmware milestones.

## Current status: milestone 2

The firmware boots, logs the reset reason and deep-sleep wakeup cause, and
demonstrates a full deep-sleep cycle waking on SW2 (GPIO12, active-high per
Good Display documentation, so the firmware uses an internal pull-down; confirm
on the physical board, including that the SW2-SW4 board revision is fitted).
Later milestones will replace the placeholder init delay with display
bring-up, the Wi-Fi access point and upload page, and the five-minute idle timer.

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
