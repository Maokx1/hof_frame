# HOF-Frame firmware

Rust / ESP-IDF firmware for the Seeed XIAO ePaper Display Board EE04, XIAO
ESP32-S3 Plus, and 7.3-inch 800 x 480 Spectra 6 panel. There are exactly two
firmware modes.

## Modes and flashing

| Behavior | Production (default) | USB (`--features usb`) |
| --- | --- | --- |
| Upload, conversion, atomic storage, panel, web progress | Real, shared | Real, shared |
| Deep sleep | Real; native USB disconnects | Simulated; logs continue |
| Scheduled refresh | 24 hours after completion | 60 seconds after completion |
| Manual-refresh limit | One hour after manual completion | One hour after manual completion |
| Battery | Website shows a voltage-based `~%` estimate (`?%` if unavailable); protection is not qualified | Explicitly simulated safe for development; voltage-based estimate is display-only |
| Runtime recovery | Persistent `hof_safety` NVS | `hof_usb`, explicitly reset to Clean on reboot |
| KEY1 / GPIO3 | Unused; no simulation behavior | Toggles separate simulated fault while idle |

**Production is the target operating mode, not a claim of battery readiness.**
Use stable USB/bench power until ADC wiring, calibration, thresholds, and the
external 5 V power path are qualified. Do not disable brownout protection.

### Set the hotspot password once

Store the password in `.cargo/wifi.local.toml`, which is explicitly ignored by
Git (including editor backups). From `firmware`, run this once:

```sh
python3 - <<'PY'
import getpass
import json
import os

password = getpass.getpass("Hotspot password (8-63 printable ASCII characters): ")
if not (8 <= len(password) <= 63 and all(0x20 <= ord(c) <= 0x7E for c in password)):
    raise SystemExit("Invalid password; no file was written.")
path = ".cargo/wifi.local.toml"
fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
with os.fdopen(fd, "w", encoding="utf-8") as config:
    config.write("[env]\nHOF_WIFI_PASSWORD = " + json.dumps(password) + "\n")
print("Created " + path)
PY
```

The prompt does not echo the password or put it in shell history. On Linux/macOS,
the new file is readable and writable only by its owner. The command refuses to
overwrite an existing file; edit it locally when changing the password.
Verify the ignore rule with `git check-ignore .cargo/wifi.local.toml`.
Do not force-add this file, copy it into tracked files, or share it.

Alternatively, create the ignored local file from the
[example](.cargo/example.wifi.local.toml), then edit the **local copy**:

```sh
cd firmware
(umask 077; set -C; cat .cargo/example.wifi.local.toml > .cargo/wifi.local.toml)
```

This refuses to overwrite an existing local file. Replace `YOUR_PASSWORD` in
`.cargo/wifi.local.toml` before building; it is only a placeholder, not a safe
default. Keep the example unchanged so no real password is committed.

Load it with `--config .cargo/wifi.local.toml` when building or flashing.
Cargo does not load this filename automatically. An already exported
`HOF_WIFI_PASSWORD` takes precedence; unset it to use the file's value.
If Cargo reports that it cannot parse `.cargo/wifi.local.toml` as a dotted key,
the file may not exist at that path. Run the setup above from `firmware` first,
then run the commands below from the same directory.

USB development:

```sh
cd firmware
cargo --config .cargo/wifi.local.toml run --release --features usb
```

Production:

```sh
cd firmware
cargo --config .cargo/wifi.local.toml run --release
```

The password is embedded at build time, never logged, and must not be committed
or shared. It must be 8-63 printable ASCII characters. Builds without a valid
password compile but refuse AP setup; there is no insecure default.
The local file is plaintext, not an encrypted secret store. Compiled firmware
also contains the password: keep those binaries private. For one-off builds or
CI, exporting `HOF_WIFI_PASSWORD` still works without a local config file.

Former save-only, display-only, battery-preview, and USB-bench feature names
have been removed. Use `usb` instead. All automated host tests remain.

## Reading the source

[main.rs](src/main.rs) only boots the application and advances its states.
Start there, then follow the selected mode's handlers:

- [app/production.rs](src/app/production.rs): dispatch one wake cycle, perform
  requested work, and prepare real deep sleep.
- [app/usb.rs](src/app/usb.rs): idle polling, gesture classification, upload,
  refresh, and simulated sleep. KEY1 is sampled only in the idle state.
- [dispatch.rs](src/dispatch.rs): pure work-selection rules. Time, safety,
  gestures, and mode-specific blocking are supplied by the caller.
- [work.rs](src/work.rs), [rtc.rs](src/rtc.rs), and
  [buttons.rs](src/buttons.rs): durable safety boundaries, retained scheduling,
  GPIO sampling, and sleep preparation.
- [upload_wifi.rs](src/upload_wifi.rs): bounded HTTP transport, independent
  request handlers, retry/result handling, and unconditional radio cleanup.
  [pipeline.rs](src/pipeline.rs) coordinates verified save and display;
  [image_decode.rs](src/image_decode.rs) handles decode and commit verification.
- [display.rs](src/display.rs): validated frame loading, overlay composition,
  display outcomes, and ordered power/SPI cleanup.
  [epd.rs](src/epd.rs) contains only panel-driver operations.

Pure transformations and decisions remain in the host-testable library:
gestures, dispatch, scheduling, conversion, overlay layout, frame validation,
and HTTP parsing. Hardware adapters own side effects. Existing storage traits
and decoder checkpoints provide extension points without introducing a
general-purpose framework. Functions have Rust `///` documentation describing
their responsibilities and important constraints.

The refactor does not change the operating modes, timing limits, stored record
formats, retry policy, or safety qualification requirements. In particular,
panel cleanup still runs before durable Clean, and an accepted upload continues
processing if the browser disconnects.

## Buttons and USB simulated fault

The EE04 user buttons KEY0/GPIO2, KEY1/GPIO3, and KEY2/GPIO5 are active-low
inputs: released HIGH, pressed LOW. Button debounce is 50 ms.

- Hold the waking KEY0 press for three seconds to open the upload window.
- Three presses within three seconds, counting the waking press, request a
  real refresh of the saved photo with Wi-Fi off.
- Other KEY0 gestures do no work. Held KEY0 must be released before rearming.
- In USB mode, press KEY1 while idle to toggle the **simulated fault**. It
  starts OFF each boot. Holding KEY1 does not repeatedly toggle it.
- When simulated fault is ON, uploads, manual refresh, and timer refresh are
  blocked. Clearing it does not move the existing refresh deadline; an overdue
  timer can refresh immediately. It never changes persistent recovery records.
- KEY1 is polled while idle, not inside synchronous gesture/upload/display
  work. It is not an emergency-stop button or cancellation of an active upload.
  Make fault-injection presses after `READY` while no work is running.

USB startup begins a fresh 60-second awake schedule after initialization,
without an immediate boot refresh. Every successful upload/manual/timer refresh
restarts that countdown from completion; uploads and scheduled refreshes do not
consume the manual limit. Upload work defers timer dispatch until AP cleanup;
expiry without a successful display can leave the scheduled refresh already due.

The accelerated USB timer physically refreshes repeatedly. Stop bench work
when finished; closing the serial monitor alone does not stop firmware.
Do not remove power mid-refresh.

## EE04 panel and power pins

The firmware targets the EE04 50-pin connector and the Seeed 7.3-inch
800 x 480 Spectra 6 panel with its ED2208 controller. The panel jumper must be
set to 50-pin. The display interface uses CS GPIO44, DC GPIO10, RST GPIO38,
MOSI GPIO9 (`D10`), SCK GPIO7 (`D8`), BUSY GPIO4, and active-high panel enable
GPIO43.

The frame uses four AAA NiMH cells and the external 5 V converter through
EE04 USB-C. Leave the JST Li-ion connector empty: the board's built-in battery
ADC monitors only a single-cell Li-ion pack on that connector. The NiMH pack
voltage divider connects to D9/GPIO8; firmware uses it for a rough website
estimate, not battery protection. Verify pad access and calibrate the reading
before relying on the estimate. See [WIRING.md](../docs/WIRING.md) and
[PLAN.md](../docs/PLAN.md#board-interface).

## Runtime safety and recovery

Both modes use the shared [SafetyGuard](src/safety.rs) and
[NVS adapter](src/safety_nvs.rs). `state_v1` contains Clean (`0xA0`),
Working (`0xA1`), or Latched (`0xA2`).

Working is committed before image/panel work. Clean is committed only after
successful cleanup; interrupted work, brownout, unknown metadata, and explicit
faults block work. Receive/decode rejection permits bounded retries without
publishing a candidate. Idle expiry cleans the upload session.

USB's new `hof_usb` namespace isolates development from the existing production
`hof_safety` latch. Neither the old production record nor retired development
records are erased or acknowledged by this migration. At each USB boot, the
previous `hof_usb` state is logged and explicitly reset to Clean. This includes
boots after interrupted work or brownout and is a development-only policy,
not battery-qualified recovery; USB must not be used for battery operation.
Before that reset, USB mode also prints a **read-only snapshot** of the
production `hof_safety` NVS state and diagnostic records. It does not change
the production namespace. The current reset reason is logged separately. The
simulated fault is separate from this real runtime guard: **KEY1 cannot clear
actual storage, radio, or cleanup faults**. In USB mode, a panel init/show
failure is logged but does not latch or halt the ESP32 if panel power-off,
SPI release, and durable safety cleanup all succeed. Buttons and uploads
remain available; the next scheduled panel attempt is deferred until 60 seconds
after failure cleanup. Failed attempts do not count as successful refreshes
or consume the manual-refresh limit. Uploads explicitly report that the image
was saved but display refresh failed; they do not report display success, and
the AP closes after the normal result grace period.
Other runtime failures stop without automatic retry for the rest of that USB
session; reboot starts a new USB session. KEY1 cannot perform that runtime recovery. Production faults
remain blocked across reboot. NVS read/write failures still block boot and
never trigger automatic partition erasure.

There is no production latch acknowledgement until qualified battery recovery
checks exist. Raw state and transitions are logged. A missing record initializes
first use; external erasure cannot be
distinguished from first use. Power-loss guarantees still need qualification.

### Persistent fault diagnostics

The original `state_v1` byte remains compatible. Two additional versioned
32-bit NVS records in the same mode-specific namespace hold diagnostics:

- `diag_v1`: current/last work phase and fault reason.
- `last_fault_v1`: most recently recorded fault, retained after successful
  work and after USB's development reset. This is one record, not a fault log.

Phases are Setup, Upload (AP/receive/retry window), Decode, Save (including
commit verification), LoadFrame, Display, PanelCleanup (power/SPI),
SafetyCleanup (durable Clean), RadioCleanup, Idle, or Unknown for legacy work.
Markers are committed at phase boundaries, not per pixel, row, or progress tick.
If a reset interrupts panel cleanup, its phase is retained; if display fails
and cleanup subsequently runs, the primary Display failure phase is restored.
Detailed driver errors and any secondary cleanup failure remain in serial logs.

Boot prints the previous work and last recorded fault **before** USB resets
its runtime state. A previous Working marker becomes an Interrupted diagnostic;
current brownout is recorded as Brownout (taking precedence over interruption);
unknown safety-state bytes become InvalidState; explicit latches record
RuntimeFailure with the active phase. A current ordinary USB/programmer reset
does not imply that the historical fault was a brownout.

To inspect a failed battery-powered run, switch off/disconnect the battery
converter, connect the board to the computer, then flash and run the USB
development build. Read the `production safety NVS snapshot (read-only)` line
in the serial monitor. Normal application flashing preserves NVS; do not erase
the NVS partition or perform a full-chip erase before collecting the snapshot.
The stored phase and reason are useful evidence, not a full serial log: there
is no timestamp or saved driver error. A `Working` state with phase `Display`
means power was lost or execution stopped during display work; a recorded
`Brownout` means the production firmware restarted and observed a brownout
reset. If power disappears before firmware can reboot, the NVS marker can
identify interrupted display work but cannot prove the electrical cause.
For the full reproduce → power down → read diagnostics sequence, see the
[next bench session checklist](../docs/WIRING.md#next-bench-session-checklist).

Example information (not a result from your board):

```text
production safety NVS snapshot (read-only): state=Some(161), diag_v1 raw=Some(3506439424) decoded=Some(Diagnostic { phase: Display, reason: None }), last_fault_v1 raw=None decoded=None
```

USB archives an inferred interruption before clearing its runtime guard; the
historical fault remains visible on later boots until another real fault
replaces it. KEY1 simulation does not create or clear these diagnostics.
Production still latches rather than granting recovery.

There is no wall-clock timestamp or stored error string. A reason/phase identifies
the operation or last committed boundary, not an exact failing instruction or
electrical root cause. Old latches without diagnostic records remain unknown;
this cannot reconstruct their original cause. Diagnostic read/write failures
block work explicitly. Separate NVS key writes are not a cross-key transaction:
an abrupt power cut or failing NVS may leave incomplete diagnostic history,
but diagnostics never grant permission or replace the authoritative work marker.
New diagnostic persistence and boot output require board confirmation.

Normal mode retains scheduling across deep sleep in RTC memory. USB mode
does not qualify real wake causes, sleep current, RTC retention, or battery
recovery. The normal held-button timeout is 60 seconds, after which that sleep
arms only the RTC timer; a later wake rechecks button release.

## Upload and display

1. Hold KEY0, join WPA2 **HOF-Frame**, and stay connected despite the "no internet"
   warning. One client at a time.
2. Open **[http://bursztynki.local](http://bursztynki.local)**. The board
   advertises this `.local` hostname over mDNS while the hotspot is active.
   DHCP uses the explicitly configured `192.168.4.1/24` subnet; the firmware
   checks the actual interface address.
   The website estimates battery percentage from the documented D9/GPIO8
   voltage-divider input (`~%`); it shows `?%` if measurement is unavailable.
   The estimate is not calibrated and is not a battery-protection feature.
3. Select portrait (480 x 800) or landscape (800 x 480), Stretch or Fit, and
   whether to show the frame battery indicator (off by default). Its corner
   selector appears only when the indicator is enabled. Choose
   PNG/JPEG up to **7 MB (7,000,000 bytes)**.
4. The browser normalizes to display-sized PNG, also limited to
   **7 MB (7,000,000 bytes)**.
   Stretch fills the layout; Fit preserves the photo with white borders.
5. The bar appears immediately: **Send image -> Process image -> Update
   display -> Complete**. It covers local resize/upload, verified processing
   and save, actual panel refresh, then cleanup.
6. Wi-Fi stays on throughout. A final success event confirms the completed
   display and safety cleanup. AP closes after six seconds, without a Done
   button. A successful TCP write does not prove the user read the message.

Both modes always physically refresh after a successful upload. There is no
save-only mode, fake percentage preview, persistent red error mark, or extra
fallback refresh after upload rejection.

The display waveform must assert BUSY (GPIO4 LOW) within one second, then return
HIGH within 120 seconds. An initially HIGH pin is not proof of completion.
A LOW pulse shorter than one second is also rejected as implausible: the panel
datasheet specifies **12 seconds typical at 25 C**. The one-second guard is a
conservative software sanity check, not a vendor-specified minimum or an added
delay that makes a failed refresh succeed.
Serial logs identify assertion and completion in milliseconds and read each
controller's three raw `0xF2` status bytes after init, power-on, and the waveform
(including waveform failure if BUSY is HIGH; no reads while BUSY stays LOW).
Only the vendor example's ready bit is labelled;
other bits are not decoded without a register specification. A ready bit alone
does not confirm panel voltage, waveform execution, or a visible image update.
In production, failure blocks further work through the runtime safety guard
with no automatic retry. USB mode continues after successful cleanup as
described above; a disconnected panel can therefore be attempted again on each
60-second schedule. Check panel power, the supplied display cable, and SPI/reset connections
with power off. This check is shared by USB and production and still needs
physical board confirmation; it cannot diagnose an electrical cause.

The POST uses HTTP chunked `application/x-hof-progress` text. Initial `200 OK`
starts streaming; it does **not** mean work succeeded. Flushed lines report
`received`, `processed` (including verified save), `displaying`, and `displayed`.
Terminal `success`, `retry` (remaining seconds), or `fatal` ends the sequence.
Tab/newline characters in error messages are sanitized. A missing final event
or lost network connection means unknown outcome, not cancellation or proof of
failure. Do not automatically resend an uncertain upload.

Early receive errors use ordinary HTTP errors and `X-Window-Seconds`.
Recoverable rejection keeps AP open within the original window and re-enables
the form when time remains. Fatal faults return a best-effort failure before
radio cleanup and blocking work. A radio-stop error can occur after the browser
has already confirmed successful image work.

### Bounds, memory, and image format

Named deadlines live in [upload.rs](src/upload.rs): idle 300 s, header 10 s,
transfer inactivity/absolute 120 s each, awake budget 600 s. Retries and page
loads never extend the original idle window. Accepted bodies can finish after
idle expiry under their transfer/global bounds. Headers/trailers are capped
at 8 KiB, chunk overhead at 128 KiB, and cumulative body size at
7 MB (7,000,000 bytes, not 7 MiB).
Malformed framing, conflicting lengths, multipart, and `Expect: 100-continue`
are unsupported and explicitly rejected.

The global budget checks decoding, flash operations, and starting refresh.
Active panel BUSY waits retain 120-second per-wait bounds rather than being
interrupted at the global deadline. Result writes and six-second grace are
additional bounded cleanup; the browser waits up to 1,211 seconds.

The device decodes static non-interlaced display-sized PNG by rows, composites
alpha over white, applies ordered 4 x 4 Bayer six-color dithering, and rotates
portrait clockwise into native landscape order. Browser JPEG support uses PNG
normalization; direct JPEG reception passes signature checks but fails decoding.
APNG, wrong dimensions, corrupt and interlaced PNGs are rejected before commit.
PNG tracked allocations are capped at 128 KiB, not including all inflater
workspace. No full RGB image is allocated on device.

Uploads stream through a 4 KiB transfer buffer into the temporary `upload`
flash partition. Sectors are erased on demand and decoding reads the completed
body through a separate 4 KiB buffered, seekable reader. The encoded body is
never allocated in PSRAM; only the 960,000-byte candidate and decoder workspace
need image-processing RAM. Interrupted or invalid staging is never used as a
saved frame, and retries overwrite it from offset zero. Temporary upload bytes
remain in flash until overwritten; they are not a second saved photo or
securely erased after use. Flash erase/write/read failures are explicit.
The 120-second transfer deadline now includes staging erase/write time;
maximum-size throughput and flash endurance still need board qualification.

Octal PSRAM is 8 MiB at 40 MHz. Ordinary large malloc/Vec allocations prefer
PSRAM; 32 KiB internal memory is reserved for DMA/internal requests. The startup
check verifies physical size, writes/reads every byte of a 4 MiB allocation,
and verifies a Rust Vec growing 128 -> 256 KiB while input and a 960,000-byte
candidate remain allocated as a conservative memory stress test, not as the
upload buffering design. It yields every 64 KiB; measured runtime is about
1.6 seconds, down from roughly 20 seconds. USB runs it at startup; production
runs it before upload AP setup, not on every timer wake.

## Storage and overlay

| Partition | Offset | Size |
| --- | --- | --- |
| NVS | `0x9000` | `0x6000` |
| PHY | `0xF000` | `0x1000` |
| App | `0x10000` | `0x6F0000` |
| Temporary upload staging | `0x700000` | `0x700000` |
| Frames | `0xE00000` | `0x200000` |

**Upgrade requirement:** reflash the firmware **and partition table** using
the updated [partitions.csv](partitions.csv), not an application-only update.
The former 4 MiB upload partition is rejected as too small. The application
partition is now 7,274,496 bytes; staging has 7 MiB of space but accepts at most
7,000,000 encoded bytes. NVS and saved-frame offsets are unchanged; do not erase
the whole chip if retaining those records. Qualification on the physical board
is still required after this layout change.

No OTA partition. Two 1 MiB slots contain 960,000 packed pixel bytes each,
with a 36-byte header and payload offset 4096. CRCs, palette/geometry validation,
generation selection, candidate readback, and header/tag-last publication
preserve a valid previous slot. Invalid newer data falls back to an older
valid slot; storage I/O failures are explicit, not missing-frame fallbacks.
No valid photo generates a white default at render time, not a stored fixture.

Validated flash is mapped read-only for display. Saved pixels are not modified
by the optional overlay: a 68 x 28 white box, 1-pixel black border, 9-pixel
corner margins, 10 x 7 battery icon, one font-space before the `?%` label, and
up-to-14-pixel text. The full icon is a fixed symbol, not a measured charge
level. Orientation/corner metadata rotates placement and text. The panel is
powered off and SPI bus/device freed after each refresh; failures attempt
emergency power cutoff.

The 1,904-byte overlay uses `swash` and the subset
[DejaVu Sans font](assets/status-font.ttf), distributed under the
[Bitstream Vera license](assets/DejaVu-LICENSE.txt). To regenerate:

```sh
pyftsubset /usr/share/fonts/truetype/dejavu/DejaVuSans.ttf \
  --text=' 0123456789%?!' --output-file=assets/status-font.ttf \
  --name-IDs='*' --name-legacy --name-languages='*'
```

## Developer workflow

Run the commands below from the `firmware` directory. Embedded builds require
the Espressif Rust 1.100.0.0 `esp` toolchain installed with
`espup install --toolchain-version 1.100.0.0` (or
`espup update --toolchain-version 1.100.0.0` for an existing installation),
`~/export-esp.sh` sourced, and `ldproxy`, `espflash`, CMake, Ninja, Git, and
Python 3 available. CI uses the same toolchain version. Configure the
[hotspot password](#set-the-hotspot-password-once) before running the firmware.

Before embedded builds, leave any active Python virtual environment (for
example, run `deactivate` in the terminal that activated the wiring project's
`.venv`) or use a terminal without Python environment auto-activation.
ESP-IDF creates its own Python environment during SDK setup. Simply unsetting
`VIRTUAL_ENV` is insufficient: `python3` must resolve to a base interpreter.
The check below reports this before starting embedded builds. Host tests and
coverage do not require leaving the Python virtual environment.

### Run on the board

Choose one command to build and flash the firmware, then start the serial
monitor:

```sh
cd firmware
python3 -c 'import sys; assert sys.prefix == sys.base_prefix, "Leave the Python virtual environment before building ESP-IDF"'
# Production mode:
cargo --config .cargo/wifi.local.toml run --release
# USB development mode (alternative):
cargo --config .cargo/wifi.local.toml run --release --features usb
```

To build without flashing, replace `run` with `build`.

### Test on the host

```sh
cd firmware
cargo +stable test --tests --all-features --target x86_64-unknown-linux-gnu
```

Firmware integration tests live in [tests](tests) as `test_<module>.rs`.
Cargo discovers each file automatically; tests access the library's public API.
Use your host target triple instead of `x86_64-unknown-linux-gnu` on other platforms.
Do not use plain `cargo test`: the default ESP32-S3 target flashes the test
executable to the board. If that happens, stop the monitor with Ctrl+C and
restore the application using a run command above.

### Test with coverage

Install `cargo-llvm-cov` and the stable toolchain's LLVM tools once:

```sh
cargo install cargo-llvm-cov
rustup component add llvm-tools-preview --toolchain stable
```

Run the host tests with coverage and open the HTML report:

```sh
cd firmware
cargo +stable llvm-cov --tests --all-features --target x86_64-unknown-linux-gnu --html --open
```

The stable toolchain and host target are needed because the project's default
`esp` toolchain is custom and its tests would target the board. The report
covers host-testable code, not ESP-IDF-specific hardware code.

### Lint and format

Run host Clippy with the same options as CI, then check both embedded modes:

```sh
cd firmware
cargo +stable clippy --lib --tests --all-features --target x86_64-unknown-linux-gnu -- -D warnings
cargo clippy -- -D warnings
cargo clippy --features usb -- -D warnings
cargo fmt --all -- --check
```

CI workflows live in the repository-root [.github/workflows](../.github/workflows).
GitHub Actions caches Cargo dependencies and ESP-IDF tools; a new cache key
requires one successful run to populate it. The ESP Rust toolchain itself is
still installed on each runner.

CI runtime needs improvement: even with warm caches, 4-5 minutes is not
acceptable long term. In a [November 2026 run](https://github.com/Maokx1/hof_frame/actions/runs/45066459918),
the ESP jobs reinstalled the toolchain and rebuilt Rust's standard library and
other dependencies despite cache hits. Reduce this time while retaining the
existing checks, including on workflow-only changes.

See [STATE_MACHINE.md](STATE_MACHINE.md) for target contracts and battery work.

## Validation and acceptance

The upload page is Polish-first, with a self-contained dark/amber layout.
It needs no internet connection, CDN, or remote fonts. The language selector at
the top switches between Polish (the default on each page load) and English
without reloading, restarting the countdown, clearing the selected photo, or
interrupting an upload. Team members are called “Bursztynek” in Polish and
“Amber” in English. Raw device errors remain verbatim under
“Szczegóły techniczne” / “Technical details”.
Edit markup in [upload.html](assets/upload.html), styles in
[upload.css](assets/upload.css), and behavior in [upload.js](assets/upload.js).
Translations and live text bindings are in
[upload-i18n.js](assets/upload-i18n.js); Polish source messages are the keys.
Firmware embeds the CSS and JavaScript into the HTML template when serving it,
so the browser still receives one self-contained page, with no extra asset
requests. The template also supplies the current timeout values.
An always-visible sticky session bar shows the remaining send window, including
while the browser prepares the image. It is independent of status/error text.
During transfer and display work, the shutdown time is unknown and is labelled
accordingly; success starts the six-second shutdown countdown, and retry uses
the remaining time reported by the board. Screen readers receive time warnings
at 60, 30, and 10 seconds rather than announcements on every timer tick.

Host tests exercise the actual library without ESP-IDF dependencies: gestures,
KEY1 debounce, scheduling, safety, atomic storage/torn writes, decoder, overlay,
and HTTP progress framing. They do not replace hardware acceptance.

Previously user-confirmed on USB hardware: upload/display with Wi-Fi on, live
four-stage web progress, compact overlay, triple-press refresh, truncated-PNG
rejection/retry, invalid-signature `415`/retry, declared-oversize `413`/retry,
idle expiry, and previous-photo preservation. The PSRAM test optimization is
confirmed at 1,574-1,585 ms. Those observations predate the two-mode migration.
The new USB KEY1 toggle, real scheduled cycle, and production sleep/wake still
need acceptance. Disconnect/stall cases, peak power/memory, power-cut durability,
panel faults, and battery operation remain unqualified.

The historical production latch remains unexplained: `hof_safety` was already
`0xA2` at boot with current-reset brownout false. There are no earlier logs to
establish its original cause. This migration preserves it, not silently clears it.
