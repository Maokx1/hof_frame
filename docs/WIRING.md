# Wiring and bill of materials

This document describes the Hall of Fame frame's 4×AAA NiMH supply and the
Seeed XIAO ePaper Display Board EE04 with the 7.3-inch Spectra 6 panel. The
board/panel details are based on Seeed's EE04 documentation and reference
configuration; confirm connector orientation, pad access, voltage, and current
on the physical hardware before final assembly.

## EE04 and panel

The Seeed 7.3-inch Spectra 6 panel is 800×480 with a 50-pin FPC connector. Set
the EE04 jumper to **50-pin** before connecting power. The Seeed_GFX2 EE04
configuration and ED2208 example document this pin map:

| Signal | ESP32-S3 GPIO | XIAO alias / polarity |
| --- | ---: | --- |
| Panel CS | 44 | Active low |
| Panel DC | 10 | Command low, data high |
| Panel reset | 38 | Active low |
| Panel MOSI | 9 | D10 |
| Panel SCK | 7 | D8 |
| Panel BUSY | 4 | Active low while refreshing |
| Panel enable | 43 | Active high |
| KEY0 | 2 | Active-low upload/wake button |
| KEY1 | 3 | Active-low USB-development fault toggle |
| KEY2 | 5 | Active-low, unused |

The ED2208 uses 4-bit transfer codes for black, white, green, blue, red, and
yellow. Firmware frame geometry is native landscape (800×480); portrait input
is rotated into that native orientation.

## Battery and 5 V power

Use four matched AAA NiMH cells in series in the switched holder. The pack is
about 4.8 V nominal and can approach 6 V immediately after charging. Remove
the cells for charging and use a four-cell NiMH charger.

| From | To | Connection / requirement |
| --- | --- | --- |
| Holder BAT+ after switch | S9V11MACMA VIN+ | Pack positive |
| Holder BAT− | S9V11MACMA VIN−/GND | Pack return |
| S9V11MACMA VOUT+ | EE04 USB-C input VBUS | Set converter to 5.0 V before connecting |
| S9V11MACMA GND | EE04 GND | Common return |

There is no isolation diode in this wiring. **Never connect computer USB or
another 5 V source while the converter is on.** Switch the holder off before
plugging in USB for flashing. Leave the EE04 JST battery connector empty:
it is for a single-cell 3.7 V Li-ion battery and is connected to the board's
charger/power path. **Never connect the 4-cell NiMH pack to that connector.**

The S9V11MACMA accepts the pack's range and provides a 5 V output plus an
adjustable low-voltage cutoff. Start characterization around 4.4 V pack
cutoff, then set the final value from loaded tests and cell limits. The cutoff
is not latching; test voltage recovery after shutdown to ensure the converter
does not cycle. A pack-level cutoff cannot detect an individual weak cell, so
use matched cells and charge or replace them as a set.

## Battery-voltage measurement

The EE04 includes a battery measurement path for a **single-cell Li-ion pack
on the JST connector**. Seeed's PlatformIO guide reads `BAT_ADC` on A0/GPIO1,
enables its divider with XIAO D5/GPIO6, and uses a nominal 2:1 divider ratio.
That path does not measure the external NiMH pack when the frame is powered
through USB-C. Changing to the Li-ion battery input would also change the
project's battery chemistry and charging arrangement; it is not a drop-in
replacement for the NiMH pack.

For the retained NiMH setup, use the passive 0–25 V / 5:1 voltage-sensor
module:

```text
BAT+ after holder switch ── module VIN / +
BAT− / system GND ───────── module GND / −
module VOUT / S ─────────── EE04 XIAO D9 / GPIO8 (ADC1 candidate)
module header − ─────────── EE04 GND
module header + ─────────── leave disconnected
```

The EE04 display configuration uses GPIO4, 7, 9, 10, 38, 43, and 44; its
buttons use GPIO2, 3, and 5. GPIO8/D9 is not assigned by that configuration
and is an ADC1 input used for the external divider. Verify that the pad is
accessible and uncommitted on the received board before wiring. A button does
not need to be removed for this pin assignment. A typical 5:1 divider
produces about 0.96 V at 4.8 V pack and 1.2 V at 6.0 V pack; verify the module
ratio and ADC range, then calibrate against a multimeter at two or more pack
voltages before relying on the estimate. GPIO8 is ADC1.

Firmware samples the divider on D9/GPIO8 and shows a rough voltage-based
percentage estimate on the upload website. The estimate uses the
documented 5:1 ratio and 4.4–5.8 V pack range; it is not calibrated and does
not qualify battery protection. The website shows `?%` when ADC setup, reading,
or voltage-range validation fails. The software low-voltage latch is not
implemented, so treat the converter cutoff as the only low-battery protection.

## Bring-up

1. Confirm the panel's 50-pin cable orientation and set the EE04 jumper to
   50-pin before applying power.
2. With the NiMH holder switched off, power the EE04 by USB-C and run a display
   test using the official EE04 / 7.3-inch Spectra 6 example.
3. Confirm the panel BUSY signal and buttons KEY0–KEY2, including KEY0 wake
   from deep sleep.
4. With the EE04 Li-ion JST connector empty, set the converter to 5.0 V and
   measure it before connecting it to USB-C. Never attach computer USB while
   the converter is active.
5. Measure board-input voltage during Wi-Fi startup and panel refresh. Measure
   sleep current at both the board's 5 V input and the battery-holder input.
6. Verify GPIO8/D9 pad access, divider output, ADC calibration, and cutoff
   behavior before enabling battery-aware firmware.

### Next bench session checklist

Follow these steps in order to investigate a failed battery-powered run. Do
not connect computer USB while the battery converter is on.

1. **Reproduce the failure with the same firmware mode/build that previously
   failed.** Do not change firmware mode before reproducing it. Check that the
   converter and cells are connected as usual, and note the displayed stage
   or behavior when refresh stops. If convenient, note the board-input
   voltage/minimum captured during the attempt.
2. **Stop battery power safely.** Switch off the holder/converter and
   disconnect the battery-powered USB-C cable. Do not attach computer USB
   until the converter is off.
3. **Read the persistent diagnostics.** Connect the board to the computer and
   from the repository root run (outside any Python virtual environment, with
   the hotspot password configured per the
   [firmware README](../firmware/README.md#set-the-hotspot-password-once)):

   ```sh
   cd firmware
   cargo --config .cargo/wifi.local.toml run --release --features usb
   ```

   This flashes/runs the USB development build. Normal application flashing
   preserves NVS; do not erase NVS or use a full-chip erase.
4. **Capture the boot output promptly.** Save the line beginning
   `production safety NVS snapshot (read-only)` and the nearby lines for
   `USB session reset reason`, `safety diagnostics: previous state`, and
   `last recorded fault`. USB mode resets only its own `hof_usb` runtime
   guard after logging it; the production `hof_safety` snapshot is read-only.
5. **Interpret, don't overdiagnose.** `state=Some(161)` (`0xA1`, Working)
   with phase `Display` means execution or power stopped during display work.
   A `Brownout` reason means firmware rebooted and recorded that reset cause.
   Neither result by itself proves the root cause. A clean state or missing
   marker may mean this firmware did not persist the attempted work, or that
   the failure happened outside the marked interval.
6. **Record the evidence before changing wiring.** Copy the captured NVS
   lines, note whether refresh stopped at the same point as before, and
   include any minimum-voltage reading. Use that to decide whether the next
   step is firmware/panel investigation or better transient/current
   measurements.

The USB development build performs real panel refreshes on its accelerated
schedule; do not leave it running unattended. Disconnect power when finished.

## KiCad wiring model

The connection model in [`wiring_model/`](wiring_model) uses
[circuit-synth](https://www.circuit-synth.com/) and
[uv](https://docs.astral.sh/uv/). From the repository root, download KiCad's
`Connector_Generic` and `power` symbol libraries into the git-ignored
`.kicad/symbols` directory:

```sh
mkdir -p .kicad/symbols
for lib in Connector_Generic power; do
  curl -fsSL "https://gitlab.com/kicad/libraries/kicad-symbols/-/raw/9.0.0/$lib.kicad_sym" \
    -o ".kicad/symbols/$lib.kicad_sym"
done
```

Print the model's net list with:

```sh
KICAD_SYMBOL_DIR=.kicad/symbols uv run wiring
```

The generated project at [`kicad/hof_frame`](kicad/hof_frame) shows the
external holder, converter, and battery-sense module on its first sheet. Open
the **Hierarchy Navigator** with **View > Panels > Hierarchy Navigator**
(`Ctrl+H` in KiCad 10), expand **Seeed EE04 v1.2**, and double-click **Power**,
**XIAO**, or **E-ink** to inspect the editable vendor circuits. The overview
and vendor hierarchy contain seven pages in total. The vendor board is a
purchased assembly, not an additional set of parts to assemble.

The USB 5 V connection reaches XIAO `U3` pin 14 on the input side of the
vendor's fuse `F1`; it does not bypass that fuse. The external divider reaches
`U3` pin 10 (D9/GPIO8), not the separate Li-ion `BAT_ADC` circuit. Confirm the
physical D9 pad is accessible on the received board before wiring it.

Save your changes and close the KiCad schematic editor before regenerating the
external schematic and full KiCad netlist with `kicad-cli` installed:

```sh
KICAD_SYMBOL_DIR=.kicad/symbols uv run wiring --kicad
```

Use `--kicad DIR` to write the project to `DIR/hof_frame` instead. To check
the generated connections and regeneration behavior, run from the repository
root:

```sh
KICAD_SYMBOL_DIR=.kicad/symbols uv run python -m unittest discover -s docs/wiring_model/tests -p test_wiring_schematic.py
```

The `hof_frame.json` circuit-synth model uses abstract board connectors for the
external connections; the generated `hof_frame.net` is exported from the full
KiCad hierarchy and includes the components already on the EE04. Regeneration
preserves project settings and vendor circuitry, updates project-specific page
metadata in the vendor root sheet, and replaces the generated top-level
schematic, JSON model, netlist, and `hof_frame.pdf`. The PDF contains only the
project's wiring overview page, not Seeed's EE04 pages, so the wiring can be
viewed without KiCad. It checks the battery, ground, USB input,
fuse, and D9 connections in the exported hierarchy before replacing them. It
also checks for an open editor and for changes made during generation.

The vendor sheets contain embedded symbols, so they can be viewed and
exported without installing Seeed's libraries. KiCad electrical-rule checks
may still warn about unavailable symbol and footprint libraries; those warnings
are separate from electrical connection errors. The unmodified vendor design
also produces local/global label-name warnings. Review those warnings and
confirm the physical wiring; an error-free check does not verify the hardware.

The EE04 sheets are adapted from Seeed Studio's
[EE04 V1.2 schematic and PCB source](https://files.seeedstudio.com/wiki/Epaper/EE04/XIAO_ePaper_Display_Board_EE04_V1.2_SCH&PCB.zip),
whose schematic title blocks specify
[CC BY-SA 4.0](https://creativecommons.org/licenses/by-sa/4.0/). The
adaptations add hierarchical USB 5 V, ground, and GPIO8/D9 connections and
project-specific hierarchy page instances, and remove the original no-connect
marker on D9; the vendor title and revision are retained. The original and
adapted Seeed sheets are distributed under
CC BY-SA 4.0, separately from the project's MIT-licensed code; see the
[vendor notice](kicad/hof_frame/seeed-ee04-v1.2/NOTICE.txt). The official
[EE04 V1.2 schematic PDF](https://files.seeedstudio.com/wiki/Epaper/EE04/XIAO_ePaper_Display_Board_EE04_SCH_V1.2.pdf)
is also available.

**Revision warning:** this project uses a local **EE04 V1.2** schematic
snapshot, not an automatically updated vendor copy. Seeed may publish revised
schematics or ship a different board revision. Before using this wiring, check
the [Seeed wiki's Resources section](https://wiki.seeedstudio.com/epaper_ee04/#resources)
to see whether the published schematic is still V1.2, and confirm your physical
board's revision. Compare any newer revision before assuming the wiring and pin
assignments here still apply.

The [EE04 getting-started guide](https://wiki.seeedstudio.com/epaper_ee04/),
[battery ADC/button guide](https://wiki.seeedstudio.com/ee04_with_platformio/),
and [Seeed_GFX2 7.3-inch ED2208 example](https://github.com/Seeed-Studio/Seeed_GFX2/tree/main/examples/ePaper%20Displays/Expansion%20Board%20Series/ESP32%20Series/XIAO%20ePaper%20Display%20Board%20-%20EE04/7.3-inch%20ePaper%20-%20Spectra%206%20800x480)
are the references for board pin and panel behavior.

## Bill of materials

| Qty. | Part | Notes |
| ---: | --- | --- |
| 1 | Seeed XIAO ePaper Display Board EE04 | With XIAO ESP32-S3 Plus |
| 1 | Seeed 7.3-inch Spectra 6 800×480 panel | 50-pin FPC |
| 1 | 4×AAA switched battery holder | Keep cell access and holder switch accessible |
| 4 | Matched AAA NiMH cells | Charge and replace as a set |
| 1 | Four-cell NiMH charger | Do not use a Li-ion charger |
| 1 | Pololu S9V11MACMA #2868 | Set to 5.0 V; cutoff is adjustable and non-latching |
| 1 | Passive 0–25 V, 5:1 voltage-sensor module | External NiMH measurement; verify divider ratio |
| 1 | USB-C cable | Used for either converter input or computer flashing, never both at once |
| 1 | Multimeter | Voltage, cutoff, and current measurements |
| — | Hookup wire, heat-shrink, labels | Keep power and signal wiring short and strain-relieved |

See the [project plan](PLAN.md) for runtime assumptions, firmware behavior,
and the 3D-printed frame requirements.
