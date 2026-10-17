# hof_frame firmware

ESP-IDF-backed Rust firmware for the Good Display ESP32-133C02
(ESP32-S3-WROOM-1) controller and GDEP133C02 13.3" Spectra 6 panel.
See the [project plan](../docs/PLAN.md), [wiring guide](../docs/WIRING.md),
and [target state machine](STATE_MACHINE.md).

## Confirmed button wiring

**SW7/GPIO13 and SW8/GPIO12 use pull-ups and are active-low:
released = HIGH, pressed = LOW. Do not use pull-downs or high-level wake.**

SW7 was hardware-verified with five clicks and repeated deep-sleep wake cycles.
The user confirmed the same polarity for SW8, which is currently unused.
Button polling uses a scheduler-yielding FreeRTOS delay and 50 ms debounce.
Short `std::thread::sleep` calls can busy-wait on ESP-IDF and starve the idle
task; this caused watchdog warnings in the earlier counter.

## Current verified mode: repeating SW7 sleep/wake

```sh
cd firmware
cargo run --release --features debug-button-sleep
```

This is the firmware currently flashed on the board:

1. Startup leaves the displayed image unchanged and turns panel power off.
2. Wait for SW7 to remain released for 50 ms, then enter GPIO13-only deep sleep.
3. A SW7 press wakes the board. Refresh only when reset is `DeepSleep`, wake
   cause is `Button`, and the EXT1 mask is exactly GPIO13 (`0x2000`).
4. Refresh the bundled full-frame
   [sunflower image](assets/sunflower.bin) (1200 x 1600, 4 bpp).
5. Wait for panel completion, power it off, wait for button release, and sleep
   again. There is no timer, gesture classification, or hourly limit in this mode.

Holding SW7 does not trigger repeated refreshes. If it does not release within
60 seconds, or refresh/setup fails, log the error and stay awake without retry.
Unexpected wake causes also abort without refreshing. The test phase lives in
RTC memory: it survives deep sleep but is **not a durable reset/recovery latch**.
If another reset reinitializes it, startup waits for SW7 rather than refreshing.

Allow the panel to rest between refreshes. The
[panel driver](src/epd.rs) retains its working initialization, pixel streaming,
BUSY polling, waveform, and power-off sequence.

## Session handoff - 2026-10-18

### Hardware results

- Five physical SW7 clicks produced five presses, five completed clicks, and
  ten raw transitions, followed by 20 seconds without extra activity.
- With pull-down, untouched GPIO13 produced hundreds of transitions and false
  clicks. Pull-up/active-low fixed the observed input behavior.
- One-shot button wake completed a full refresh in 27.7 seconds and remained
  idle for over 30 seconds without another refresh.
- Two consecutive repeating cycles with the display connected reported
  `DeepSleep` / `Button`, EXT1 `0x2000`, full refreshes in 27.7 and 27.8 seconds,
  and return to GPIO13-only deep sleep. No watchdog or brownout was logged.
- An earlier timer-only test also completed its initial refresh and a refresh
  after 120 seconds of deep sleep.
- Earlier brownouts were real, and the user observed they stopped with the
  display disconnected. Their exact electrical cause is unresolved; successful
  recent cycles do not prove that all power/reset failure modes are fixed.

The board was left running the repeating SW7 test, asleep waiting for SW7.
Cleanup afterward did not reflash or reset it. No commits were created.

### Next steps, in order

1. Keep the repeating cycle as the known-good baseline. Check held-button
   release behavior and a longer untouched sleep interval without causing
   back-to-back panel refreshes.
2. Reintroduce and test gestures in the normal path: waking press counts as
   press one, three presses within three seconds request manual refresh,
   an uninterrupted five-second waking hold requests upload, and incomplete
   gestures return to sleep. Add deterministic timing/debounce tests before
   hardware testing; upload is currently only a log placeholder.
3. Integrate the daily RTC deadline and one-hour manual-refresh limit.
   Test with a deliberately shortened development interval, then restore
   24 hours. Advance the deadline only after successful refresh. Verify that
   early button wakes do not accidentally reset the daily deadline.
4. Replace RTC-only interrupted-refresh protection with the persistent
   recovery/safety latch required by the state-machine specification.
   Verify brownout, ordinary reset, and power-loss behavior explicitly.
5. Continue the remaining target work: persistent frame storage and white
   default, battery ADC/pin verification and low-battery latch, Wi-Fi upload,
   decoding/dithering, and atomic frame replacement. Measure whole-board
   sleep current and supply behavior before claiming battery readiness.

### Cleanup completed

Removed the GPIO mapping probe, standalone click counter, timer-only test,
display-only awake mode, no-panel probe, and waveform-skipping probe.
Replaced the accumulated VS Code debug tasks with one repeating-cycle task.
Only `debug-button-sleep` remains as a bring-up feature.

## Normal path: implemented but not hardware-qualified

```sh
cd firmware
cargo run --release
```

The normal path contains gesture classification, a nominal 24-hour timer,
one-hour manual-refresh limiting, held-button handling, and temporary RTC
recovery metadata. It still refreshes on cold/reset boot and has not passed
the same end-to-end hardware acceptance checks as the repeating cycle.
Do not confuse a successful build with a verified production state machine.

There is no Wi-Fi upload, persistent photo, white default frame, battery
measurement, or low-battery latch yet. This is not battery-ready firmware.

## Build prerequisites

- The `esp` Rust toolchain installed via `espup`:
  `espup install`, then `. ~/export-esp.sh` in each new shell.
- `ldproxy` and `espflash`:
  `cargo install ldproxy espflash`.
- `cmake`, `ninja`, `git`, and Python 3 for the managed ESP-IDF toolchain.

```sh
cd firmware
cargo build --release
cargo build --release --features debug-button-sleep
```

The first build downloads and builds ESP-IDF v5.5.3 into `.embuild/`
(gitignored); subsequent builds reuse it.

## USB monitoring and power

The [Cargo runner](.cargo/config.toml) uses `espflash flash --monitor`.
Native USB-Serial/JTAG disconnects during deep sleep, so a monitor's broken
pipe immediately after the sleep log is expected. Do not reflash merely
because USB disconnected. Wake with SW7 before reconnecting a monitor.
Keep one serial reader at a time.

For a passive, bounded capture after wake, without programmer reset/sync:

```sh
stty -F /dev/ttyACM0 115200 raw -echo -hupcl
timeout 40s cat /dev/ttyACM0
```

`BOD: Brownout detector was triggered` is a distinct brownout indication, not
a watchdog diagnosis. Watchdog warnings identify the affected task separately.
Broken pipe alone does not establish that the panel refresh failed.

Disconnect board power before connecting/disconnecting the display cable.
For power investigation, measure the board's 5 V input and ESP32 supply
during refresh. Do not connect external power and USB VBUS together without
confirmed power-path isolation, and do not raise voltage or disable protection
to conceal resets.
