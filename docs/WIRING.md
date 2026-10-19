# Wiring diagram and bill of materials

This document describes the proof-of-concept wiring for the Hall of Fame
frame. It prioritizes off-the-shelf modules and separates confirmed
connections from values that must be checked on the received boards. Do not
power the controller until the converter output has been measured.

## Executable wiring model

The generated KiCad schematic and netlist come from [`wiring_model`](wiring_model).
Use that script as the source for the modeled connections; this document
explains the design, assumptions, and bring-up requirements. To regenerate the
schematic, follow the setup instructions in the repository
[README](../README.md) and run:

```sh
KICAD_SYMBOL_DIR=.kicad/symbols uv run wiring --kicad
```

For the proof of concept, use an off-the-shelf passive DC voltage-sensor
module, commonly sold as a `0–25 V` or `0–25 V 5:1 voltage divider` module.
[Purchased Botland 5V/25V voltage-divider module](https://botland.com.pl/czujniki-pradu/8231-dzielnik-napiecia-5v25v-dla-arduino-5904422311285.html):

```text
BAT+ after holder switch ── module VIN / +
BAT− / system GND ───────── module GND / −
module VOUT / S ──────────── GPIO14 / ADC2_CH3 (ESP32-S3-WROOM-1 module pad 22)
module header − ──────────── controller GND
module header + ──────────── leave disconnected (not connected internally)
```

GPIO14 was selected after the user reported an accessible pad and no
interfering board connection. GPIO4 was not selected: the user found
continuity to a microSD socket joint. Battery sensing is not yet implemented;
sample ADC2 before starting Wi-Fi. The executable wiring model and generated
schematic still show the earlier GPIO4 proposal and have not yet been updated.
Do not power the divider signal into an unpowered controller; for bench tests,
power the controller before enabling the divider and disable the divider
before removing controller power. Permanent power sequencing remains unresolved.

### U22 unpopulated footprint: user-reported pad mapping

On the received board, viewed from the component side with the **USB-C port
pointing down**, the user identified these connections on the five-pad row
at the unpopulated U22 footprint, ordered **top to bottom**:

| Position in the row | Connection | ESP32-S3-WROOM-1 module pad |
| --- | --- | --- |
| Top | GPIO43 / TXD0 | 37 |
| Second | GPIO44 / RXD0 | 36 |
| Third | GND | — |
| Fourth | 3V3 | 2 |
| Bottom | GPIO14 / ADC2_CH3 | 22 |

These are physical positions in the identified row, not U22 package pin
numbers or a complete U22 footprint pinout. This mapping is user-reported
from the received board, not verified against a controller schematic.
The bottom pad provides the selected battery-sense solder point for divider
`S`; the third pad provides GND for divider header `−`. Leave divider header
`+` disconnected; do not connect it to U22's 3V3 pad.

These modules are normally only resistor dividers; they need no supply
connection. A typical 5:1 module produces 0.96 V from a 4.8 V pack and 1.2 V
from a 6.0 V pack, which is suitable for the ESP32-S3 ADC input. Confirm the
actual divider ratio and that its maximum output is within the confirmed ADC
input range before connecting it. Keep its output wire away from the
S9V11MACMA switching node.

### Recorded divider measurement

Measured with the battery pack connected to the divider and the controller
disconnected:

| Item | Measured / calculated value |
| --- | --- |
| Input voltage (screw-terminal positive to negative) | 5.104 V |
| Output voltage (header S to header −) | 1.0155 V |
| Estimated input/output ratio (5.104 / 1.0155) | ≈ 5.026:1 |

The output is approximately 0.52% below the nominal 5:1 divider's expected
1.0208 V. This is a single measurement point, not a full divider or ADC
calibration; the estimated ratio includes multimeter uncertainty.

Typical 0–25 V modules use a 30 kΩ and 7.5 kΩ divider, drawing approximately
128 µA from a 4.8 V pack. This is acceptable for the PoC but is a permanent
sleep-current cost. When this power system is consolidated onto a custom PCB,
replace the module with a 100 kΩ upper resistor, 33 kΩ lower resistor, and
100 nF ADC filter capacitor. That circuit scales 0–6.0 V to approximately
0–1.49 V and draws about 36 µA at 4.8 V.

## Estimated runtime per charged pack

With four matched Green Cell 950 mAh AAA NiMH cells, the series pack is
950 mAh at approximately 4.8 V nominal (4.56 Wh). For the specified profile
of one received photo every 14 days and one full panel refresh every 24 hours,
the corrected preliminary estimate is **about 10 weeks as an upper bound**,
excluding the unmeasured items below. Plan for **about six weeks** until it is
measured on the final hardware.

> **Correction.** An earlier revision listed 250–500 mAh for one five-minute
> BLE wake. That was wrong: five minutes at 50–100 mA is 4–8 mAh (the old
> number was in mA·min), and the claim that BLE is the main lifetime limiter
> was wrong with it. The old table's own subtotal implied only 17–27 days,
> contradicting its "one to two months". See [PLAN.md](PLAN.md) for details.

This estimate includes 14 scheduled refreshes and one Wi-Fi photo-transfer wake
per 14-day period. It assumes approximately 750 mAh remains usable before the
loaded low-battery cutoff, about 90% converter efficiency, an estimated
1 W/30 s full refresh, and a five-minute Wi-Fi wake averaging an assumed
100–200 mA at the pack. The S9V11MACMA's low-load current and this PoC's 128 µA sensor module also
consume capacity continuously and are the largest known drain.

The preliminary input-side budget for one 14-day photo interval is:

| Component | Assumption | Estimated pack use |
| --- | --- | ---: |
| Scheduled full refreshes | 14 refreshes at 1 W for 30 s, 90% converter efficiency | ~27 mAh |
| S9V11MACMA and PoC sensor-module idle draw | 0.2 mA regulator quiescent current plus 0.128 mA divider current for 14 days | ~110 mAh |
| Wi-Fi/photo-transfer wake | One five-minute wake averaging an assumed 100–200 mA at the pack | ~8–17 mAh |
| **Subtotal of known terms** | | **~145–154 mAh** |
| ESP32-133C02 board sleep current | Whole board on its 5 V input (regulators, USB-UART, level shifters, SD slot), not just the ESP32-S3 | **Unmeasured**; each 0.1 mA adds ~34 mAh |
| NiMH self-discharge | Depends on cell type | **Unmeasured** |
| Schottky diode reverse leakage | | **Unmeasured**, expected small |

The unmeasured board sleep current can dominate everything else; measure it
first. The six-week planning figure leaves about 100 mAh per 14 days (about
0.3 mA continuous) for the unmeasured items.

Measure these values at the switched battery-holder input, before the
converter, so the final result includes losses from the complete power path.

Do not use this estimate for a battery replacement schedule until it has been
confirmed by a full discharge test through this complete power path.

**Power loss mid-refresh.** If the pack runs out during the roughly 30 s
refresh, the panel can be left partially updated. No hardware mitigation is
planned; keep a spare charged set of cells and swap it in, after which the
next refresh restores the picture.

## Connection schedule

| From | To | Connection | Status / requirement |
| --- | --- | --- | --- |
| Holder BAT+ after switch | S9V11MACMA VIN+ | Switched battery positive | Required |
| Holder BAT− | S9V11MACMA VIN−/GND and system GND | Battery return | Required |
| S9V11MACMA VOUT+ | 1N5822 Schottky diode anode | Regulated 5 V | Diode is 3 A, 40 V; band (cathode) toward the controller |
| 1N5822 cathode | ESP32-133C02 5 V/VBUS input | Regulated 5 V (minus about 0.4–0.5 V diode drop) | Verify board label and polarity; confirm the controller runs reliably with the drop, else raise the converter output slightly |
| S9V11MACMA GND | ESP32-133C02 GND | Common return | Required |
| Holder BAT+ after switch | Voltage-sensor module VIN/+ | Battery measurement source | Battery side, before the converter |
| Holder BAT− | Voltage-sensor module GND/− | Measurement return | Common system ground |
| Voltage-sensor module VOUT/S | GPIO14 / ADC2_CH3, module pad 22 | Pack-voltage sense | Selected accessible connection; measure before Wi-Fi. Firmware and [`wiring_model`](wiring_model) update pending |
| SW7 | GPIO13 | **Pull-up, active-low**, RTC-capable | Fitted switch; firmware wakes from deep sleep and redraws the display |
| SW8 | GPIO12 | **Pull-up, active-low**, RTC-capable | Fitted switch; same polarity confirmed by the user, currently unused |
| ESP32-133C02 display connector | GDEP133C02 | Supplied display cable | Do not substitute a cable or assume pin order |

The USB-C input on the controller must not be used in a way that back-feeds
the battery-powered 5 V rail into a computer or charger. The series Schottky
diode on the converter output stops USB power from driving the converter
output. It does not by itself stop the battery rail from energizing the USB
connector's VBUS if the board ties VBUS to its 5 V input; confirm this on the
schematic, or switch the holder off while USB is connected. The diode's
forward drop and reverse leakage also count against the supply voltage and
the sleep-current budget.

## Bring-up and cutoff procedure

### Next session: first task (planned for 2026-10-20)

First solder the divider signal wire to the bottom GPIO14 pad of the
identified U22 row and its ground wire to the third (GND) pad, using the
orientation documented above. Disconnect USB and batteries before soldering.
Inspect for bridges and check continuity before applying power. For the
initial USB-powered test, keep the converter output disconnected from the
controller, power the controller before enabling the divider, and disable the
divider before removing controller power. ADC firmware work comes later.

Battery powering is a separate follow-up: qualify the converter/diode output
and a confirmed controller 5 V/VBUS input, or use a correctly wired USB-C
power-input adapter. Do not connect the pack to U22's 3V3 pad or the onboard
Li-ion battery connector. Permanent divider power sequencing still needs
resolution before unattended battery use.

### Power-path qualification

1. Install matched, removable AAA NiMH cells only after all continuity checks.
2. With the controller disconnected, verify holder polarity, switch operation,
   and continuity to the converter input.
3. Set the S9V11MACMA output to 5.0 V with its output potentiometer. Measure
   it with the battery connected and no controller load.
4. Confirm the controller input pin and ground from the ESP32-133C02
   documentation, then connect the regulated output.
5. Confirm the ADC reading against a multimeter at at least two known pack
   voltages and calibrate the firmware.
6. Characterize startup and display-refresh reliability while lowering the
   loaded pack voltage. Set the firmware cutoff above the measured failure
   point with engineering margin.
7. On wake, sample the pack before Wi-Fi or display work. Below the firmware
   threshold, skip the transfer and display refresh. The converter's built-in
   cutoff remains the primary protection.

The S9V11MACMA has a built-in low-voltage cutoff: its input voltage feeds the
EN pin through the cutoff potentiometer, and the converter sleeps when EN falls
below 0.7 V. No separate monitor board is needed. The ADC check is only a
secondary firmware safeguard and cannot protect the pack before the MCU
samples.

To set the cutoff:

1. With the controller disconnected, power the board from a bench supply or
   the cells and set the output to 5.0 V.
2. Measure VIN and EN, then turn the cutoff potentiometer until
   `EN / 0.7 V = VIN / VIN_cutoff`. Clockwise raises the cutoff. For example,
   with VIN = 5.4 V and a 4.4 V target, EN should be about 0.86 V.
3. Verify by running real cells down under load (or with a bench supply, if one is available) until the output switches off.

### Measurement procedure and recorded calibration

Both trimmers are 11-turn potentiometers. The 0.7 V (sleep) and 0.8 V
(re-enable) EN thresholds come from the Pololu
[S9V11MACMA product page](https://www.pololu.com/product/2868). EN is a fixed
fraction of VIN set by the cutoff trimmer, so set it once; it does not need
readjusting as the pack discharges.

Multimeter on DC volts, black probe on the regulator GND pin, battery
connected and switched on, controller and OUT load disconnected. Pins read
from the pin row labelled SEL, PG, EN, IN, GND, OUT:

1. Output: red probe on OUT; turn the output trimmer until it reads 5.0 V.
2. Input: red probe on IN to read VIN.
3. Cutoff: calculate `EN_target = 0.7 V × VIN / VIN_cutoff`, then turn the
   cutoff trimmer until EN reads that value. Measure VIN and EN together.
4. Check: `EN / VIN` should stay constant; `VIN_cutoff = 0.7 V × VIN / EN`.

Recorded values (this unit):

| Item | Value |
| --- | --- |
| Output (OUT to GND) | 5.005 V |
| VIN at calibration | 5.103 V |
| Target cutoff | 4.4 V (EN target 0.812 V) |
| EN set (trimmer is hard to set precisely) | 0.816 V |
| Resulting EN / VIN | 0.1599 |
| Resulting cutoff (EN = 0.7 V) | ≈ 4.38 V |
| Resulting restart (EN = 0.8 V) | ≈ 5.00 V |

The result is within 0.02 V of the 4.4 V target. Still verify it with real
cells or a bench supply as described above.

Start with about 4.4 V for the four-cell pack (1.1 V/cell); this is a test
value, not a Green Cell specification. Choose the final threshold above the
cell discharge limit and above the minimum voltage at which the
converter or controller becomes unreliable, with margin confirmed under load.
The regulator needs 3 V to start and runs down to 2 V.

The cutoff is **not latching**. The converter restarts when EN rises above
0.8 V, i.e. when VIN reaches about 114% of the cutoff (about 5.0 V for a 4.4 V
cutoff). Cells recover somewhat once the load stops; verify with real
discharged cells that the pack stays below the restart voltage and the
converter does not repeatedly restart. In sleep, the board still draws about
7 µA per volt on VIN, so switch off the holder or remove the cells for
prolonged storage. A four-cell series voltage only reports the sum, so it
cannot identify a weak individual cell: keep the cells matched, charge and
replace them as a set, and use per-cell monitoring if protection of each cell
is required.

## Bill of materials

### To order

| Item | Qty. | Part / specification | Notes |
| --- | ---: | --- | --- |
| 1 | 1 | Good Display GDEP133C02 panel with ESP32-133C02 controller (ESP32-S3-WROOM-1), as the paired development kit | Panel and board come together; confirm the 5 V/VBUS input during bring-up |
| 2 | 1 | 4× AAA battery holder with integrated switch (Allegro listing) | Retain cell-removal access; confirm the listing has a switch and takes AAA cells |
| 3 | 1 | [Pololu S9V11MACMA #2868](https://www.pololu.com/product/2868) step-up/step-down converter with adjustable low-voltage cutoff | Set output to 5.0 V and cutoff (start ~4.4 V) before connection; not latching. Ships without terminals or headers |
| 4 | 1 | 1N5822 Schottky diode (3 A, 40 V, DO-201AD through-hole) | Series diode between converter output and controller 5 V input; band toward the controller |
| 5 | 1 | [Passive 0–25 V, 5:1 DC voltage-sensor module](https://botland.com.pl/czujniki-pradu/8231-dzielnik-napiecia-5v25v-dla-arduino-5904422311285.html) | PoC battery-voltage measurement; verify its actual resistor values and ratio |

### Already owned (do not order)

| Item | Qty. | Part / specification | Notes |
| --- | ---: | --- | --- |
| 6 | 4 | AAA NiMH cells (matched set; plan assumed Green Cell 950 mAh) | Charge and replace as a set; spare sets available for swapping |
| 7 | 1 | Charger for NiMH cells | Do not use TP4056 or a constant-voltage charger |
| 8 | 1 set | Hookup wire, heat-shrink, labels, soldering iron | Keep power and signal wiring short and strain-relieved |
| 9 | 1 | USB cable for flashing and development power | |
| 10 | 1 | Multimeter | Bring-up, current and loaded-voltage measurements |

### Optional

| Item | Qty. | Part / specification | Notes |
| --- | ---: | --- | --- |
| 11 | 1 | Fuse or resettable PTC | Choose rating after measured load |

A reverse-polarity protector was removed from the design because the holder
fixes the polarity. Validate the converter's cutoff and restart behavior with
real cells before regular battery use.

## Assembly notes

- Keep the battery holder accessible so cells can be removed for charging.
- Provide access to the holder switch and enough bend radius for the supplied
  display cable.
- Secure the S9V11MACMA and the diode so their terminals cannot contact the frame or
  display backplane.
- Label BAT+, regulated +5 V, and GND before final assembly.
- Record measured dimensions, switch behavior, loaded voltages, current,
  display refresh timing, wireless reliability, and runtime separately from
  this provisional wiring plan.
