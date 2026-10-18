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

This real-sleep baseline behaves as follows (the latest bench capture used
the mock-sleep mode below):

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

Abort idle messages now repeat the recorded failure reason, ESP error code,
and the EXT1 mask captured at boot, rather than re-reading a wake mask that
sleep setup may already have cleared. These diagnostics share the RTC-memory
limitations above; they do not establish what happened before a power loss or
reset that reinitializes RTC data.

Allow the panel to rest between refreshes. The
[panel driver](src/epd.rs) retains its working initialization, pixel streaming,
BUSY polling, waveform, and power-off sequence.

## Gesture and scheduling test (mock sleep, continuous USB logging)

```sh
cd firmware
cargo run --release --features debug-gestures
```

This mode uses the normal-path [gesture classifier](src/gesture.rs) but
**never enters deep sleep**. USB and the monitor stay connected throughout
repeated tests. The panel load switch stays off; refresh and upload actions
are mocked. `debug-gestures` is independent of `debug-button-sleep` and takes
precedence if both features are enabled.

Wait for `mock DeepSleep: READY`, then perform one gesture:

- One short SW7 click: `Sleep`.
- Three presses within three seconds, including the waking press:
  `ManualRefresh`, followed by a simulated refresh if allowed.
- Hold the waking press for five seconds: `Upload` (classification only).
- Two presses, a late third press, or holding only a later press: `Sleep`.

Scheduling uses the same [scheduler](src/schedule.rs) as the normal path,
with **test-only** intervals of 120 seconds for daily refresh and 30 seconds
for manual rate limiting. Normal firmware retains 24 hours and one hour.
Simulated refreshes take one second; only successful completion restarts the
daily deadline. Startup arms the initial timer without refreshing.

Acceptance sequence (keep the monitor open throughout):

1. Triple-press: expect `mock Refresh: manual started`, then `SUCCESS` and
   a new 120-second deadline.
2. Triple-press again within 30 seconds of completion: expect `RATE LIMITED`
   and the same absolute daily deadline.
3. Short-click or hold for upload: expect `daily deadline unchanged`.
   Upload is still a placeholder, not a successful image receipt or refresh.
4. After 30 seconds, triple-press again: expect an allowed simulated refresh.
5. Leave the board untouched for 120 seconds after the last completion:
   expect `TIMER wake`, a simulated refresh, and a fresh deadline.

Status logs every five seconds show remaining daily time and whether manual
refresh is allowed. Daily timer checks continue even while waiting for a held
button to release. If a deadline expires during gesture classification, it is
serviced after the gesture unless a successful manual refresh replaces it.
Automatic refreshes do not reset the manual-refresh limit.

Each result is logged immediately with elapsed time. The test then waits for
SW7 to be released for 50 ms and prints `READY` again, with no reporting delay,
reset, or monitor reconnection. Holding SW7 after a result cannot start another
gesture: release handling waits indefinitely and reports its status every five
seconds. This intentionally mocks production's 60-second stuck-button sleep
policy rather than disconnecting the test monitor.

The gesture timer starts when polling detects the first active-low SW7 press,
which simulates the waking press. Polling yields to FreeRTOS between samples.
This tests physical gestures without boot latency; actual deep-sleep wake,
RTC retention across resets, boot-time edges, and sleep current still need separate hardware
checks in the real sleep modes.

## Persistent recovery test (mock sleep, real NVS writes)

The normal and mocked paths now use the shared [safety guard](src/safety.rs)
and [NVS adapter](src/safety_nvs.rs). A single versioned byte represents
`Clean`, `Working`, or `Latched`. The `Working` marker is committed before
work, and `Clean` is committed only after successful work and panel cleanup.
Booting with unfinished work, a brownout, or an unknown record latches work
off. Triple presses and timer wakes cannot clear it. Storage errors stop
work and repeat an explicit error while awake, without retries or erasing NVS.

The existing NVS partition is used without changing partitions or automatically
erasing full/incompatible storage. Missing metadata initializes a clean record
on first use; missing data after an external erase cannot be distinguished
from first use. ESP-IDF NVS commits are used, but power-loss/low-voltage write
guarantees still require hardware testing. No battery voltage is measured yet.
The marker is currently wired to refresh only; transfer/frame-write paths do
not exist yet.

Mock and normal safety records are isolated in `hof_mock` and `hof_safety`
namespaces. The real-sleep `debug-button-sleep` baseline remains RTC-only
and does not use this persistent guard.

To inject one interrupted simulated refresh per boot:

```sh
cd firmware
cargo run --release --features debug-interrupted-work
```

1. Wait for `READY`, then triple-press. Expect `WORKING marker committed`,
   an injected interruption, and state `Latched`. The test rereads real NVS
   to simulate reboot recovery **without resetting or disconnecting USB**.
2. Triple-press again: expect `BLOCKED by persistent safety latch`.
   Short clicks do not clear it either.
3. Leave it latched until the 120-second timer: expect another blocked
   refresh and a new recheck interval, not a successful refresh.
4. Hold the waking press for five seconds: expect acknowledgement committed
   and state `CLEAN`. This test explicitly simulates safe recovery voltage;
   it is not battery qualification and starts no upload or refresh.
5. Release, then triple-press: expect a successful simulated refresh. The
   injection was consumed, so later cycles run normally until reboot.

For a separate persistence check, leave the test latched and restart/reflash
with `debug-gestures` (without the injection feature). It should load
`Latched`; only the mock qualified hold clears it. A deliberate real restart
or power cycle may disconnect USB; ordinary mock cycles never do.
Fault injection resets once per actual boot, so reflashing the injection
build rearms it.

**Normal firmware fails closed:** it checks persistent metadata before panel
work, commits the marker before powering the panel, and latches on refresh
failure. A triple press no longer overrides recovery. Since calibrated battery
recovery checks are not implemented, a normal-path latch cannot yet be cleared
by SW7. Do not treat this as battery-ready firmware or use mock acknowledgement
to clear the separate normal safety namespace.

## Frame format, storage, and N16R8 memory budget

The board is an ESP32-S3-WROOM-1 N16R8: **16 MiB flash and 8 MiB PSRAM**.
Flash capacity was reported as 16 MB by the programmer. Firmware now enables
octal PSRAM at 40 MHz with ESP-IDF's boot memory test. Encoded input and packed
frames use explicit capability allocations. Ordinary malloc/Rust heap
allocations may also use PSRAM: allocations above 16 KiB prefer it, while
32 KiB of internal RAM is reserved for internal/DMA-specific requests.
This is necessary for PNG inflater buffers allocated by the decoder's
ordinary `Vec`/`Box`, not by our explicit buffer wrapper. Small allocations
prefer internal memory; either preference may fall back to the other heap.
The `debug-upload` mode checks the physical 8 MiB size and write/read-verifies
a separate 4 MiB allocation before Wi-Fi setup. Those checks passed on the
board. It now also checks a fallible 128 KiB Rust Vec allocation and growth
to 256 KiB, verifying PSRAM placement and data preservation while a 4 MiB
input and 960,000-byte candidate remain allocated. This allocator regression
check is implemented. Subsequent board logs confirmed the corrected decoder
path through successful frame commit/reload.

The [partition table](partitions.csv) divides flash as follows:

| Region | Offset | Size | Purpose |
| --- | --- | --- | --- |
| NVS | `0x9000` | `0x6000` (24 KiB) | Small settings and safety records, not image bodies |
| PHY | `0xF000` | 4 KiB | Radio calibration |
| Application | `0x10000` | `0x9F0000` (9.9375 MiB) | Firmware, including bring-up fixture |
| Upload | `0xA00000` | `0x400000` (4 MiB) | Reserved encoded PNG/JPEG flash staging; current receiver uses PSRAM |
| Frames | `0xE00000` | `0x200000` (2 MiB) | Two 1 MiB slots for old and candidate packed frames |

The encoded upload limit remains **4,194,304 bytes**. A decoded display frame
is always **960,000 bytes**, not 4 MiB: 1200 x 1600 native portrait pixels at
4 bits per pixel. Stream/decode the encoded upload into this representation;
larger decoded source images still need explicit decoder/memory limits.
The reserved upload partition is not written by the current PSRAM receiver.

Each frame slot has a 36-byte explicit little-endian header in its first
sector and a payload at offset 4096. The header includes format/version,
native dimensions, bpp, length, generation, orientation, battery corner,
payload CRC32, header CRC32, and a final publication tag. The image pipeline
must rotate landscape input clockwise into native portrait order before
committing; stored pixels are already rotated. Battery-corner metadata is
relative to the chosen orientation; the status overlay uses this metadata
at render time rather than modifying saved pixels.

Commit validates input size and every palette nibble, erases only the inactive
slot, writes and rereads the candidate, then writes header and publication tag
last. Loading validates both slots and chooses the highest valid generation.
A corrupt newer candidate falls back to the previous valid frame; invalid
slots are logged. If neither is valid, the normal path generates white pixels
without storing a default frame. Storage I/O errors remain explicit failures,
not missing-frame fallbacks. Both saved photos and the generated white default
now receive the render-time status box. Calibrated battery measurement and the
persistent upload-error mark are not implemented.

Loading/verification uses bounded 1 KiB buffers. Rendering maps validated flash
read-only rather than allocating a 960,000-byte internal-RAM framebuffer.
Real flash operations yield periodically to FreeRTOS. CRCs detect corruption;
actual power-cut durability and physical rotation still need bench testing.

### Storage bring-up with mocked sleep and panel

```sh
cd firmware
cargo run --release --features debug-frame-storage
```

The Cargo runner now passes `--flash-size 16mb --partition-table partitions.csv`.
This installs the new layout without an erase-all command, keeping NVS/PHY
offsets unchanged. The old factory partition is reduced to make room for data:
use only on this confirmed 16 MiB development board with the current small app,
and back up any pre-existing data in the newly reserved region first. Never
use an old partition table/runner with firmware that writes these frame slots.
This layout has one factory app and no OTA slots.

At boot this feature loads existing slots, then commits the bundled sunflower
fixture once to the inactive slot under the mock NVS safety guard. It reloads,
maps, and byte-compares the stored frame with the fixture. Expect
`frame storage test: PASS; committed/reloaded/mapped 960000 bytes`, followed by
the usual mock gesture/scheduling logs. Flashing this feature again alternates
slots and increments generation. It writes real flash but never refreshes the
panel, starts Wi-Fi, or enters deep sleep. Do not combine it with interruption
injection for the initial storage check.

If mock safety is latched, the storage test logs `BLOCKED`; acknowledge with
a five-second hold, then reflash this feature to rerun it. To verify loading
without another write, flash `debug-gestures` and triple-press or wait for its
timer: simulated refresh now validates and reports the selected stored frame.
The real-sleep `debug-button-sleep` baseline still renders the bundled fixture;
the normal path now renders validated storage or plain white fallback.

## PSRAM and Wi-Fi image-save test (mock sleep and panel)

### Real display after upload (mock sleep)

To display a successfully saved upload, use `debug-upload-display` instead
of `debug-upload` in the password-prompt command below:

```sh
cd firmware
bash -c 'read -r -s -p "Hotspot password (8-63 ASCII characters): " HOF_WIFI_PASSWORD; printf "\n"; export HOF_WIFI_PASSWORD; cargo run --release --features debug-upload-display'
```

This mode performs a **real panel refresh only after a successful upload,
decode, atomic commit, and reload**. It does not refresh on boot.
After `READY`, hold SW7 for five seconds, join HOF-Frame, open
http://192.168.4.1, choose sizing/layout, and upload. An existing safety latch
requires an acknowledgement hold, release, then another hold for upload.

Wi-Fi stops before decoding. The encoded buffer is released before display;
the verified saved frame is mapped read-only and rendered through the existing
panel sequence. Expect `SAVED`, then `starting REAL panel refresh`, and finally
`display test: SUCCESS; panel OFF; safety CLEAN`. Keep stable bench power
connected until completion and allow the panel to rest between tests.
Battery checks remain simulated: this is not battery-ready firmware.

The Working marker remains committed through display and power/SPI cleanup.
Panel failure forces power off and latches work without automatic retry;
the valid committed photo remains stored. The SPI device and bus are released
after each refresh so another upload can refresh in the same boot.
Only completed display restarts the daily deadline; decode rejection or
save-only operation does not. Timer and triple-press refreshes still remain
simulated, avoiding automatic physical updates every two minutes.
Deep sleep stays mocked and USB logging continues. The render-time battery
box shows `?`; the persistent upload-error mark is not implemented yet.

### Battery/status overlay

The box includes a 17 x 11 pixel filled battery icon before `?%`, or before
the simulated `100%` in preview mode. The full icon is a fixed symbol,
not a claimed charge-level measurement. Text is at most 24 pixels high and
scales down as needed to fit the icon and `100%` inside the 120 x 48 box,
75% of the original width and height. The icon rotates with the text in landscape.

Every saved/default-frame render draws a 120 x 48 pixel white box with a
2-pixel black border, 16 pixels from the selected layout corner. Landscape
placement and text rotate with the photo. The default white frame uses
portrait/top-right. The current display always shows **`?`** because battery
ADC wiring/calibration is not qualified; no percentage or voltage is invented.

For a size/legibility preview, replace `debug-upload-display` with
`debug-battery-preview` in the flash command. It includes the real upload/display
test and renders a fixed **simulated `100%`** instead of `?`, using the same
120 x 48 box and font size. Logs explicitly identify the simulation; no battery
measurement is made or persisted. Without this opt-in feature, `?` is unchanged.

Text is rasterized from a subset of DejaVu Sans outlines using `fontdue`,
with a 24-pixel-high question mark. Ordered black/white coverage represents
the smooth outline edges on the six-color panel, which has no grayscale.
The 5,760-byte overlay is prepared before powering the panel on and composited
over read-only frame pixels; it is never baked into the stored photo.
Overlay construction errors are logged and prevent panel power-on.
The baseline `debug-button-sleep` sunflower diagnostic is unchanged.

Host tests cover all eight corner/orientation combinations, exact box
margins/border, 24-pixel question-mark height, and fitting all `0%` through
`100%` values for later calibrated measurements. Physical legibility,
positioning, and font memory behavior still need a board test. Upload using
`debug-upload-display` with the desired corner to preview it; timer/manual
physical refreshes remain mocked.

Font provenance/license: [status-font.ttf](assets/status-font.ttf) is a
subset of DejaVu Sans, distributed under the
[Bitstream Vera license](assets/DejaVu-LICENSE.txt). Regenerate using FontTools:

```sh
pyftsubset /usr/share/fonts/truetype/dejavu/DejaVuSans.ttf \
  --text='0123456789%?!' --output-file=assets/status-font.ttf \
  --name-IDs='*' --name-legacy --name-languages='*'
```

### Save-only bring-up

`debug-upload` extends mock gestures with a real WPA2 hotspot, HTTP
receiver, bounded row decoder, dithering, and atomic frame save.
Normal firmware and `debug-gestures` alone still have no upload
window. Do not combine this mode with interruption injection for initial
receiver testing.

Choose an 8-63 character printable ASCII hotspot password locally, supply it through
`HOF_WIFI_PASSWORD` at build time, and keep it out of git and shared logs.
It is embedded in your local firmware image, never logged by firmware or
included in the page. For a hidden prompt that works regardless of your
interactive shell:

```sh
cd firmware
bash -c 'read -r -s -p "Hotspot password (8-63 ASCII characters): " HOF_WIFI_PASSWORD; printf "\n"; export HOF_WIFI_PASSWORD; cargo run --release --features debug-upload'
```

The password remains fixed for that compiled image. A build without a valid
password compiles for validation but refuses to start this test at runtime;
there is no insecure default.

1. Look for physical PSRAM size `8388608` and
   `PSRAM: PASS; explicit 4194304-byte allocation write/read verified`.
   The yielding write/read test can take roughly 20 seconds; let it finish.
   Also expect `PSRAM: Rust heap PASS`, verifying the decoder's allocation path.
2. Wait for `READY`, then hold SW7 for five seconds. A latched mock safety
   record makes the first hold acknowledge only; release and hold again to
   request upload.
3. Join **HOF-Frame** with your chosen password. Keep the connection despite
   the phone/computer's "no internet" warning. Only one device may join at a
   time; disconnect it before trying another device.
4. Open **http://192.168.4.1**, select layout, image sizing, and battery
   corner, and choose a PNG/JPEG no larger than **4,194,304 bytes**.
   **Stretch** fills the display while changing proportions; **Fit** preserves
   proportions and adds white borders. Both resize in the browser to
   1200 x 1600 portrait or 1600 x 1200 landscape and upload a PNG.
   The resized PNG must also fit the 4 MiB limit; otherwise the page reports
   an error without transmitting it. Browser resizing uses the original
   five-minute idle window. Reloading does not restart that window.
   The AP explicitly configures `192.168.4.1/24` with DHCP, and the ready
   log reports the checked live interface address after the HTTP listener binds.
5. Upload. Expect progress and a receipt-only message, followed by hotspot
   shutdown. The board decodes rows, composites transparency over white,
   applies ordered six-color dithering, rotates landscape into native order,
   commits the inactive frame slot, and reloads it to verify selection.
   Look for **`Upload pipeline: SAVED`** with the new generation/metadata.
   The screen does not change, mock sleep logs continue, and the daily
   deadline is unchanged because no display refresh occurred.

**Save-only milestone:** `200 OK` confirms receipt, not successful decoding,
save, or display. Check serial logs for the result after radio shutdown.
The board accepts only static, non-interlaced PNGs at the selected display
resolution for decoding; direct/raw JPEG uploads can pass receiver checks
but are rejected by the decoder without changing the saved frame. JPEG
support in this milestone is through the browser's PNG normalization, not an
on-device JPEG decoder. Palette/grayscale/RGB/alpha PNGs are expanded to
8-bit rows, with 16-bit input reduced to 8-bit. APNG, interlaced, malformed,
wrong-size, and over-budget images are rejected explicitly before commit.

The encoded input (at most 4 MiB) and packed candidate (960,000 bytes) use
explicit PSRAM allocations. PNG decoding uses rows rather than a full RGB
image and sets the PNG library's tracked allocation limit to 128 KiB.
That library limit is not a total workspace cap: its inflater has separate
ordinary heap buffers, now backed by PSRAM for large allocations. Decoder
startup logs both internal/PSRAM free space and largest blocks; total peak
library/runtime/network overhead still requires board measurement.
Dithering uses a 4 x 4 ordered
matrix and the six nominal RGB palette colors; physical color calibration
and visual quality remain unverified. The global awake deadline is checked
during conversion and before flash operations. No upload-partition writes
occur. Orientation/corner are saved with the valid frame, but battery
measurement and persistent error marks are not yet implemented. Decode
rejections log an error and clean up; allocation/storage faults latch work
off. Battery recovery safety remains simulated in this test, not measured.

One AP client and one upload attempt per window are allowed. Both declared
`Content-Length` and chunked/unknown-length transfers are supported with a
cumulative 4 MiB bound. Declared oversize is rejected before body allocation.
Unframed POSTs, conflicting or duplicate length headers, unsupported transfer
encodings, malformed chunks, wrong MIME types, and missing PNG/JPEG signatures
are rejected explicitly. HTTP/1.1, raw image bodies, and explicit layout/corner
query values are required; multipart form bodies and `Expect: 100-continue`
are not supported. Chunk framing has a 128 KiB overhead bound and headers/
trailers are bounded at 8 KiB.

Named deadlines in [upload.rs](src/upload.rs): initial idle 300 seconds,
transfer inactivity 120 seconds, absolute transfer 120 seconds, headers
10 seconds, global virtual awake budget 600 seconds from the gesture's
simulated wake. A synchronous upload window temporarily defers mock daily
dispatch until cleanup; a due daily refresh is then serviced. Partial input,
disconnect, timeout, or rejection during an accepted upload body ends the
attempt with the previous frame unchanged. Header-stage timeout, disconnect,
or invalid request closes only that TCP connection and keeps accepting
requests until the original idle/global deadline. Browsers can open idle
speculative connections; these must not close the hotspot before upload.
Header failures and GET/404 requests do not extend the idle window. Radio/setup/
shutdown or allocation errors are explicit blocking faults; no automatic
retry. Ordinary receipt/rejection/timeout paths close Wi-Fi and return to
mock sleep without USB disconnecting.

Run the shared pure logic and PNG-decoder tests without ESP-IDF dependencies:

```sh
cd firmware
cargo +stable test --manifest-path host-tests/Cargo.toml --target x86_64-unknown-linux-gnu
```

Browser checks exercised both sizing modes in both orientations using JPEG
input, checking PNG output dimensions, white Fit borders, and Stretch coverage.
The decode/dither/save integration still needs board acceptance.
The first board decode attempt received 3,278,491 bytes and shut the radio
down, then aborted on a 131,072-byte inflater Vec allocation because ordinary
malloc could not use PSRAM. No commit started; reboot correctly recovered the
unfinished work marker as Latched. The allocator fix enables PSRAM-backed
malloc with the internal reserve above, rather than reducing image limits
or disabling the safety latch. After flashing, acknowledge the mock latch
with one five-second SW7 hold, release, then hold again to open the hotspot.
With the allocator fix, board logs confirm conversion and successful
commit/reload of that 3,278,491-byte upload: slot 0, generation 3, portrait/
top-right, payload CRC 949275097. Slot 1/generation 2 remained valid.
Safety returned to Clean and USB logs continued. Real display and repeated
SPI cleanup in `debug-upload-display` were implemented. The user subsequently
reported the upload-to-screen test worked flawlessly; this confirms the
exercised physical render, not all failure paths or repeated-upload cleanup.
The newly added status overlay still needs a panel preview.

Host tests cover fixed/chunked exact 4 MiB versus +1 byte, fragmented input,
ambiguous framing, truncation, palette-independent signature checks, and exact
inactivity/absolute deadline boundaries (including simulated slow drip).
Board logs confirmed 8 MiB octal PSRAM at 40 MHz, the ESP-IDF memory test,
and our 4 MiB allocation/write/read test. They also confirmed AP startup and
DHCP assignment, but exposed the library's default `192.168.71.1` subnet
behind an incorrect `192.168.4.1` ready message. After the explicit subnet
fix, board logs confirmed DHCP at `192.168.4.1`, a client at `192.168.4.2`,
and receipt of 3,192,335 bytes into PSRAM in roughly 6.2 seconds with portrait/
top-right metadata. The receipt response was sent, sockets and Wi-Fi closed,
the upload buffer was discarded, and mock safety returned to Clean with the
daily deadline unchanged and USB logs continuing. Browser receipt display,
repeat windows, on-board exact-limit/oversize/chunked transfers,
disconnect/stall/slow-drip behavior, and loaded power still need qualification.

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

At the initial handoff the board was left running the repeating SW7 test,
asleep waiting for SW7. Cleanup afterward did not reflash or reset it.
No commits were created.

### Subsequent bench results - 2026-10-18

- Reflashing the repeating test restored SW7 response after an aborted test.
  A short SW7 press completed a refresh. A held-button attempt reportedly
  left a partial update on USB-only power; no log from the failed wake was
  captured, so the cause remains unknown. The startup `Broken pipe` log only
  established entry into deep sleep, not a refresh failure.
- The board was then flashed with the mock-sleep `debug-gestures` mode.
  The supplied continuous log showed `Sleep` at 3.000 s, `ManualRefresh` at
  0.920 s, and `Upload` at 5.000 s. Each cycle rearmed after release without
  USB disconnects or a reset in the captured session.
- Another gesture returned `Sleep` at 5.040 s, consistent with release
  debounce at the hold boundary; raw transition timing was not captured.
  These results validate the exercised gestures with mock sleep only, not
  real deep-sleep boot timing, panel refresh, or upload behavior.
- The subsequent scheduling log confirmed a simulated manual refresh completed
  at uptime 7.056 s. Triple presses at 12.026 s and 18.656 s were rate-limited,
  and a short-click result at 26.706 s preserved the same absolute deadline
  (`713676711 us`). Manual eligibility returned after the 30-second test limit.
  A later manual refresh completed at 61.026 s and moved the deadline to
  `767633907 us`. With no further presses, timer dispatch occurred at
  181.096 s, about 120 seconds after that completion; its simulated refresh
  completed at 182.096 s and armed another 120-second interval. USB logging
  remained continuous, with no reset or watchdog warning in the supplied
  capture. This qualifies the exercised shortened scheduling flow, not RTC
  retention across real deep sleep, 24-hour clock drift, or physical refresh.
- The persistent-recovery mock capture started `Clean`, committed `Working`
  at uptime 6.290 s, then reread the NVS record after an injected interruption
  and entered `Latched`. A triple press at 17.420 s was blocked. A qualified
  hold completed at 28.040 s and committed acknowledgement at 28.050 s using
  explicitly simulated safe recovery voltage. The next triple press committed
  `Working` at 38.450 s and completed a simulated refresh with NVS clean at
  39.460 s. USB remained connected throughout. This verifies the exercised
  NVS-backed mock recovery sequence; this capture does not verify blocked
  timer dispatch while latched, persistence across an actual reset/power loss,
  or battery recovery voltage.
- The first `debug-frame-storage` bench run installed the new 16 MiB partition
  table with a 4 MiB upload region and 2 MiB frame region. Both slots initially
  read empty. The test committed, validated, reloaded, mapped, and byte-compared
  the full 960,000-byte fixture, reporting PASS at uptime 18.791 s with slot 0,
  generation 1, portrait/top-right metadata, and payload CRC32 `3479119083`.
  NVS ended clean; panel and Wi-Fi stayed off. Subsequent hold and short-click
  gestures preserved the daily deadline and USB logging stayed connected.
  This capture verifies the initial real-flash storage path, not replacement
  into slot 1, loading across an actual restart, power-cut durability, physical
  rendering, or PSRAM availability.
- The second storage run loaded slot 0/generation 1 after programmer reset,
  committed the fixture into slot 1/generation 2, and reported full
  commit/reload/map comparison PASS at uptime 22.331 s. Slot 0 remained valid.
  A subsequent triple press selected slot 1/generation 2 for the simulated
  refresh and completed with NVS clean at 28.301 s. This verifies retention
  across the observed flash/reset sequence, alternate-slot replacement, and
  newest-valid generation selection on real flash. It does not establish
  power-cut atomicity or physical panel rendering. USB remained connected
  during the application test, with panel and Wi-Fi off.

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
4. Verify the implemented NVS work marker and latch in mock mode, then verify
   brownout, ordinary reset, and power-loss behavior explicitly. Implement
   calibrated battery recovery before enabling normal-path acknowledgement.
5. Continue the remaining target work: persistent frame storage and white
   default, battery ADC/pin verification and low-battery latch, Wi-Fi upload,
   decoding/dithering, and atomic frame replacement. Measure whole-board
   sleep current and supply behavior before claiming battery readiness.

### Cleanup completed

Removed the GPIO mapping probe, standalone click counter, timer-only test,
display-only awake mode, no-panel probe, and waveform-skipping probe.
Replaced the accumulated VS Code debug tasks with one repeating-cycle task.
At handoff only `debug-button-sleep` remained as a bring-up feature.
The subsequent implementation added `debug-gestures` for no-panel gesture
qualification and repeated abort diagnostics.

## Normal path: implemented but not hardware-qualified

```sh
cd firmware
cargo run --release
```

The normal path contains gesture classification, a nominal 24-hour timer,
one-hour manual-refresh limiting, held-button handling, RTC scheduling
metadata, and a persistent NVS work marker/latch. It refreshes on cold/reset
boot only when the persistent guard permits work and has not passed
the same end-to-end hardware acceptance checks as the repeating cycle.
Do not confuse a successful build with a verified production state machine.

The gesture classifier and scheduler are isolated from hardware so their
timing rules can be tested deterministically:

```sh
cd firmware
cargo +stable test --manifest-path host-tests/Cargo.toml --target x86_64-unknown-linux-gnu
```

The firmware's default Cargo target is ESP32-S3/ESP-IDF, so these pure logic
tests use a separate host manifest without ESP-IDF dependencies, including the
PNG decoder and font rasterizer tests. They do not replace hardware checks
of boot latency, button polarity, and physical press timing.
They cover waking presses already released before sampling, triple-press
deadline boundaries, bounce, pending release at the five-second boundary,
later-press holds, and terminal decisions. The normal path samples the gesture
before logging/recovery dispatch so those operations do not block sampling.
Scheduler tests cover exact daily/hourly boundaries, unchanged deadlines on
non-refresh wakes, successful completion timestamps, failed-refresh backoff,
recovery cooldown, and a manual success at timestamp zero. Failed work never
updates the last-success time or consumes the manual limit. The retained
layout version changed, so a newly flashed normal build initializes new RTC
scheduling metadata; this is not a persistent recovery latch.

Host frame tests cover full-size commits, supported palettes, version/geometry/
metadata validation, CRCs, slot alternation, candidate corruption, torn
erase/payload/header/publication operations, and landscape corner mapping.

Normal firmware has no Wi-Fi upload/decoder; `debug-upload` tests browser
resizing, bounded PNG decoding, dithering, and atomic save without refreshing
the panel unless `debug-upload-display` is enabled. The render-time overlay
shows `?`; there is no calibrated battery measurement or voltage-driven
low-battery latch yet. Packed-frame storage and plain white fallback are
implemented; the persistent guard handles interrupted work and brownout,
not battery thresholds.
This is not battery-ready firmware.

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
cargo build --release --features debug-gestures
cargo build --release --features debug-interrupted-work
cargo build --release --features debug-frame-storage
cargo build --release --features debug-upload
```

The first build downloads and builds ESP-IDF v5.5.3 into `.embuild/`
(gitignored); subsequent builds reuse it.

## USB monitoring and power

The [Cargo runner](.cargo/config.toml) uses `espflash flash --monitor`
with the 16 MiB [partition table](partitions.csv).
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
