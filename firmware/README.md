# HOF-Frame firmware

Rust / ESP-IDF firmware for the ESP32-S3-WROOM-1 N16R8 and a 1200 x 1600
Spectra 6 panel. There are exactly two firmware modes.

## Modes and flashing

| Behavior | Production (default) | USB (`--features usb`) |
| --- | --- | --- |
| Upload, conversion, atomic storage, panel, web progress | Real, shared | Real, shared |
| Deep sleep | Real; native USB disconnects | Simulated; logs continue |
| Scheduled refresh | 24 hours after completion | 60 seconds after completion |
| Manual-refresh limit | One hour after manual completion | One hour after manual completion |
| Battery | Measurement/protection not yet qualified | Explicitly simulated safe for development; label remains `?%` |
| Runtime recovery | Persistent `hof_safety` NVS | `hof_usb`, explicitly reset to Clean on reboot |
| SW8 | Unused; no simulation behavior | Toggles separate simulated fault while idle |

**Production is the target operating mode, not a claim of battery readiness.**
Use stable USB/bench power until ADC wiring, calibration, thresholds, and the
external 5 V power path are qualified. Do not disable brownout protection.

USB development:

```sh
cd firmware
bash -c 'read -r -s -p "Hotspot password (8-63 ASCII characters): " HOF_WIFI_PASSWORD; printf "\n"; export HOF_WIFI_PASSWORD; cargo run --release --features usb'
```

Production:

```sh
cd firmware
bash -c 'read -r -s -p "Hotspot password (8-63 ASCII characters): " HOF_WIFI_PASSWORD; printf "\n"; export HOF_WIFI_PASSWORD; cargo run --release'
```

The password is embedded at build time, never logged, and must not be committed
or shared. It must be 8-63 printable ASCII characters. Builds without a valid
password compile but refuse AP setup; there is no insecure default.

Former save-only, display-only, battery-preview, and USB-bench feature names
have been removed. Use `usb` instead. All automated host tests remain.

## Buttons and USB simulated fault

The fitted SW7/GPIO13 and SW8/GPIO12 switches are pull-up, active-low inputs:
released HIGH, pressed LOW. Button debounce is 50 ms.

- Hold the waking SW7 press for three seconds to open the upload window.
- Three presses within three seconds, counting the waking press, request a
  real refresh of the saved photo with Wi-Fi off.
- Other SW7 gestures do no work. Held SW7 must be released before rearming.
- In USB mode, press SW8 while idle to toggle the **simulated fault**. It
  starts OFF each boot. Holding SW8 does not repeatedly toggle it.
- When simulated fault is ON, uploads, manual refresh, and timer refresh are
  blocked. Clearing it does not move the existing refresh deadline; an overdue
  timer can refresh immediately. It never changes persistent recovery records.
- SW8 is polled while idle, not inside synchronous gesture/upload/display
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
simulated fault is separate from this real runtime guard: **SW8 cannot clear
actual storage, panel, radio, or cleanup faults**. Runtime failures stop
without automatic retry for the rest of that USB session; reboot starts a new
USB session. SW8 cannot perform that runtime recovery. Production faults
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
replaces it. SW8 simulation does not create or clear these diagnostics.
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

1. Hold SW7, join WPA2 **HOF-Frame**, and stay connected despite the "no internet"
   warning. One client at a time.
2. Open **[http://192.168.4.1](http://192.168.4.1)**. DHCP uses the explicitly configured
   `192.168.4.1/24` subnet; the ready log checks the actual interface address.
3. Select portrait (1200 x 1600) or landscape (1600 x 1200), Stretch or Fit,
   and the battery-box corner. Choose PNG/JPEG up to **7 MB (7,000,000 bytes)**.
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
landscape clockwise into native portrait order. Browser JPEG support uses PNG
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
by the overlay: a 120 x 48 white box, 2-pixel black border, 16-pixel corner
margins, full 17 x 11 battery icon, one font-space before the percentage or `?%`
label, and up-to-24-pixel text. The full icon
is a fixed symbol, not a measured charge level. Orientation/corner metadata
rotates placement and text. The panel is powered off and SPI bus/device freed
after each refresh; failures attempt emergency power cutoff.

The 5,760-byte overlay uses `fontdue` and the subset
[DejaVu Sans font](assets/status-font.ttf), distributed under the
[Bitstream Vera license](assets/DejaVu-LICENSE.txt). To regenerate:

```sh
pyftsubset /usr/share/fonts/truetype/dejavu/DejaVuSans.ttf \
  --text=' 0123456789%?!' --output-file=assets/status-font.ttf \
  --name-IDs='*' --name-legacy --name-languages='*'
```

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

```sh
cd firmware
cargo +stable test --manifest-path host-tests/Cargo.toml --target x86_64-unknown-linux-gnu
cargo build --release
cargo build --release --features usb
cargo fmt -- --check
```

Host tests reuse actual library source without ESP-IDF dependencies: gestures,
SW8 debounce, scheduling, safety, atomic storage/torn writes, decoder, overlay,
and HTTP progress framing. They do not replace hardware acceptance.

Previously user-confirmed on USB hardware: upload/display with Wi-Fi on, live
four-stage web progress, compact overlay, triple-press refresh, truncated-PNG
rejection/retry, invalid-signature `415`/retry, declared-oversize `413`/retry,
idle expiry, and previous-photo preservation. The PSRAM test optimization is
confirmed at 1,574-1,585 ms. Those observations predate the two-mode migration.
The new USB SW8 toggle, real scheduled cycle, and production sleep/wake still
need acceptance. Disconnect/stall cases, peak power/memory, power-cut durability,
panel faults, and battery operation remain unqualified.

The historical production latch remains unexplained: `hof_safety` was already
`0xA2` at boot with current-reset brownout false. There are no earlier logs to
establish its original cause. This migration preserves it, not silently clears it.

Tooling: install the `esp` Rust toolchain with `espup`, source `~/export-esp.sh`,
and have `ldproxy`, `espflash`, CMake, Ninja, Git, and Python 3 available.
See [STATE_MACHINE.md](STATE_MACHINE.md) for target contracts and battery work.
