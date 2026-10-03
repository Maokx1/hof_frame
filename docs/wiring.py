"""Hall of Fame frame wiring, captured as a circuit-synth design.

Mirrors the connection schedule in WIRING.md. Off-the-shelf modules are
modelled as generic connectors, so each pin must match the received boards.
Do not treat this as verified until the converter output has been measured.

Needs the KiCad symbol libraries (Connector_Generic, Device, power, Switch) and
Espressif.kicad_sym from espressif/kicad-libraries; run
from the repository root:

    KICAD_SYMBOL_DIR=.kicad/symbols uv run python docs/wiring.py         # net list
    KICAD_SYMBOL_DIR=.kicad/symbols uv run python docs/wiring.py --kicad # docs/kicad/hof_frame
"""

import argparse
import os
import re
from collections.abc import Callable
from pathlib import Path
from typing import Any, cast

# write_kicad_schematic() changes directory, so a relative path would break
if "KICAD_SYMBOL_DIR" in os.environ:
    os.environ["KICAD_SYMBOL_DIR"] = str(Path(os.environ["KICAD_SYMBOL_DIR"]).resolve())

from circuit_synth import Circuit, Component, Net, circuit
from circuit_synth.kicad.schematic import text_flow_placement

CONN_2 = "Connector_Generic:Conn_01x02"
CONN_3 = "Connector_Generic:Conn_01x03"
CONN_4 = "Connector_Generic:Conn_01x04"
CONN_5 = "Connector_Generic:Conn_01x05"
ESP32_S3 = "Espressif:ESP32-S3"
SW_PUSH = "Switch:SW_Push"
HDR_FP = "Connector_PinHeader_2.54mm:PinHeader_1x{n:02d}_P2.54mm_Vertical"


def header(symbol: str, pins: int, ref: str, value: str) -> Component:
    return Component(
        symbol=symbol,
        ref=ref,
        value=value,
        footprint=HDR_FP.format(n=pins),
    )


@circuit(name="hof_frame", comments=False)
def hof_frame_wiring():
    """Battery -> S9V11MACMA -> 1N5822 -> ESP32-133C02, plus pack-voltage sense."""
    bat_pos = Net("BAT_POS_SWITCHED")  # after the holder's integrated switch
    gnd = Net("GND")
    reg_5v = Net("REG_5V")  # S9V11MACMA output, before the diode
    ctrl_5v = Net("CTRL_5V")  # after the diode, to the controller 5 V/VBUS input
    vbat_sense = Net("VBAT_SENSE")  # divider output to a spare ESP32-S3 ADC pin

    # 4x AAA NiMH holder with integrated switch: pin 1 = BAT+ (switched), 2 = BAT-
    holder = header(CONN_2, 2, "J1", "4xAAA holder")
    bat_pos += holder[1]
    gnd += holder[2]

    # Optional fuse/PTC is omitted; insert it between BAT_POS_SWITCHED and the
    # converter once the load has been measured.

    # Pololu S9V11MACMA #2868 connector order: 1 = VOUT, 2 = GND, 3 = VIN,
    # 4 = EN, 5 = PG. Its cutoff potentiometer drives EN; PG is unused.
    conv = header(CONN_5, 5, "U1", "Pololu S9V11MACMA #2868")
    reg_5v += conv[1]
    gnd += conv[2]
    bat_pos += conv[3]

    # 1N5822 Schottky: anode at the converter, cathode (band) at the controller
    diode = Component(
        symbol="Device:D_Schottky",
        ref="D1",
        value="1N5822",
        footprint="Diode_THT:D_DO-201AD_P15.24mm_Horizontal",
    )
    reg_5v += diode[2]  # anode
    ctrl_5v += diode[1]  # cathode

    # ESP32-133C02 board. 5 V enters through its USB-C/battery power input and
    # is regulated on the board (not modelled), so the SoC's own supply pins are
    # left open. J2 is that board-level 5 V input (verify the real connector).
    board_in = header(CONN_2, 2, "J2", "Board 5 V in (USB-C VBUS)")
    ctrl_5v += board_in[1]
    gnd += board_in[2]

    # ESP32-S3 SoC: GPIO4 (ADC1_CH3) is not used by the board's display, SD card
    # or buttons per its datasheet; confirm it is broken out before relying on it.
    soc = Component(symbol=ESP32_S3, ref="U2", value="ESP32-S3")
    vbat_sense += soc[9]  # GPIO4/ADC1_CH3
    gnd += soc[57]  # exposed ground pad
    # SW2 wake button, on the controller board: GPIO12, triggered by a high level
    # (Good Display datasheet). Its other side is the board's 3V3 rail.
    sw2 = Component(
        symbol=SW_PUSH,
        ref="SW2",
        value="SW2",
        footprint="Button_Switch_SMD:SW_SPST_PTS645",
    )
    wake = Net("WAKE")
    board_3v3 = Net("BOARD_3V3")  # on-board regulator output (regulator not modelled)
    wake += soc[17]  # GPIO12/ADC2_CH1, RTC-capable
    wake += sw2[1]
    board_3v3 += sw2[2]
    # The display uses its supplied cable and is not wired here.

    # Passive 0-25 V (5:1) sensor module: 1 = VIN+, 2 = S (output), 3 = GND
    sense = header(CONN_3, 3, "U3", "0-25 V divider module")
    bat_pos += sense[1]  # battery side, before the converter
    vbat_sense += sense[2]
    gnd += sense[3]


# Symbol centres in mm on an A4 sheet; all on the 2.54 mm grid so pins land on-grid.
# Power flows left to right and the ESP32 board sits at the right-hand end.
SCHEMATIC_LAYOUT = {
    "J1": (40.64, 76.2),  # battery holder
    "U1": (91.44, 76.2),  # S9V11MACMA converter
    "D1": (127.0, 55.88),  # 1N5822, rotated so the anode faces the converter
    "J2": (154.94, 55.88),  # board power port, pins on the board outline
    "U2": (210.82, 96.52),  # ESP32-S3 SoC, inside the board outline
    "U3": (91.44, 127.0),  # voltage sensor module
    "SW2": (261.62, 106.68),  # wake button, horizontal, pin 1 on the left
}
# Lib y of pin 1 for each generic connector: pin n is at y = cy - top + (n - 1) * 2.54.
PIN1_TOP = {"J1": 0.0, "U1": 5.08, "J2": 0.0, "U3": 2.54}
# Library coordinates (x, y up) of the ESP32-S3 pins that are wired.
SOC_PINS = {9: (33.02, 10.16), 17: (33.02, -10.16)}
# Dashed outline of the ESP32-133C02 board: (left, top, right, bottom).
BOARD_BOX = (149.86, 40.64, 284.48, 152.4)


def spread_layout(component_bboxes, spacing=2.54):
    """Replace circuit-synth's cramped text-flow placement with SCHEMATIC_LAYOUT."""
    placements = [
        (key, *SCHEMATIC_LAYOUT[key.split("_")[0]]) for key, _w, _h in component_bboxes
    ]
    return placements, "A4"


text_flow_placement.place_with_text_flow = spread_layout


def pin(ref: str, number: int) -> tuple[float, float]:
    """Connection point of a wired pin: connectors face left, D1 is rotated."""
    cx, cy = SCHEMATIC_LAYOUT[ref]
    if ref == "D1":  # rotated 180 degrees: pin 2 (anode) left, pin 1 (cathode) right
        return (cx - 3.81, cy) if number == 2 else (cx + 3.81, cy)
    if ref == "SW2":
        return (cx - 5.08, cy) if number == 1 else (cx + 5.08, cy)
    if ref == "U2":
        px, py = SOC_PINS[number]
        return cx + px, cy - py
    return cx - 5.08, cy - PIN1_TOP[ref] + (number - 1) * 2.54


def net_routes() -> dict[str, tuple[list[list[tuple[float, float]]], Any]]:
    """Orthogonal wire polylines per net, plus where to put the net label."""
    loop_x, trunk_x = 27.94, 83.82
    below_j1 = 86.36
    j1, u1_vout, u1_vin = pin("J1", 1), pin("U1", 1), pin("U1", 3)
    u3_vin, u3_out = pin("U3", 1), pin("U3", 2)
    j2_5v, gpio4 = pin("J2", 1), pin("U2", 9)
    d1_anode, d1_cathode = pin("D1", 2), pin("D1", 1)
    stub = 10.16
    sw1, sw2_pin = pin("SW2", 1), pin("SW2", 2)
    gpio12 = pin("U2", 17)
    return {
        "BAT_POS_SWITCHED": (
            [
                [j1, (loop_x, j1[1]), (loop_x, below_j1), (trunk_x, below_j1)],
                [u1_vin, (trunk_x, u1_vin[1]), (trunk_x, u3_vin[1]), u3_vin],
            ],
            (50.8, below_j1),
        ),
        "REG_5V": (
            [[u1_vout, (80.01, u1_vout[1]), (80.01, d1_anode[1]), d1_anode]],
            (95.25, d1_anode[1]),
        ),
        "CTRL_5V": ([[d1_cathode, j2_5v]], (134.62, d1_cathode[1])),
        # Short labelled stubs at each end: a long wire would cross WAKE.
        "VBAT_SENSE": (
            [[u3_out, (u3_out[0] - stub, u3_out[1])], [gpio4, (gpio4[0] + stub, gpio4[1])]],
            [(u3_out[0] - stub, u3_out[1], "right"), (gpio4[0] + stub, gpio4[1], "left")],
        ),
        "WAKE": ([[gpio12, sw1]], (gpio12[0] + 2.54, gpio12[1])),
        "BOARD_3V3": ([[sw2_pin, (sw2_pin[0] + 2.54, sw2_pin[1])]], (sw2_pin[0] + 2.54, sw2_pin[1])),
    }


def _drop_blocks(text: str, keyword: str) -> str:
    """Remove every top-level '(keyword ...)' s-expression."""
    out, i = [], 0
    marker = "(" + keyword + " "
    while (j := text.find(marker, i)) != -1:
        out.append(text[i:j])
        depth, k = 0, j
        while True:
            depth += {"(": 1, ")": -1}.get(text[k], 0)
            k += 1
            if depth == 0:
                break
        i = k
    out.append(text[i:])
    return "".join(out)


def wire_schematic(sch: Path) -> None:
    """Replace circuit-synth's net labels with real wires between the parts."""
    text = _drop_blocks(sch.read_text(), "hierarchical_label")
    text = text.replace('(paper "A3")', '(paper "A4")')  # the layout fits A4
    d1_x, d1_y = SCHEMATIC_LAYOUT["D1"]
    text = re.sub(
        r'(\(lib_id "Device:D_Schottky"\)\s*\(at )[\d.]+ [\d.]+ 0\)',
        rf"\g<1>{d1_x} {d1_y} 180)",
        text,
    )
    items = []
    for name, (polylines, label_at) in net_routes().items():
        for line in polylines:
            for (x1, y1), (x2, y2) in zip(line, line[1:]):
                items.append(
                    f"\t(wire (pts (xy {x1:.2f} {y1:.2f}) (xy {x2:.2f} {y2:.2f}))"
                    ' (stroke (width 0) (type default)))'
                )
        for lx, ly, *side in label_at if isinstance(label_at, list) else [label_at]:
            justify = f"{side[0] if side else 'left'} bottom"
            items.append(
                f'\t(label "{name}" (at {lx:.2f} {ly:.2f} 0)'
                f" (effects (font (size 1.27 1.27)) (justify {justify})))"
            )
    items.append("\t(junction (at 83.82 86.36) (diameter 0) (color 0 0 0 0))")
    left, top, right, bottom = BOARD_BOX
    items.append(
        f"\t(rectangle (start {left} {top}) (end {right} {bottom})"
        " (stroke (width 0.254) (type dash)) (fill (type none)))"
    )
    items.append(
        f'\t(text "ESP32-133C02 board" (at {left + 2.54} {top + 5.08} 0)'
        " (effects (font (size 2.54 2.54) (bold yes)) (justify left)))"
    )
    items.append(
        f'\t(text "5 V -> on-board regulators -> ESP32-S3 3V3 (not modelled)"'
        f" (at {left + 2.54} {bottom - 3.81} 0)"
        " (effects (font (size 1.27 1.27) italic) (justify left)))"
    )
    body = text.rstrip()
    assert body.endswith(")")
    sch.write_text(body[:-1] + "\n".join(items) + "\n)\n")


def write_kicad_schematic(circuit_obj, out_dir: Path) -> Path:
    """Generate a schematic-only KiCad project under out_dir."""
    out_dir = out_dir.resolve()
    out_dir.mkdir(parents=True, exist_ok=True)
    cwd = Path.cwd()
    os.chdir(out_dir)  # circuit-synth creates the project in the current directory
    try:
        circuit_obj.generate_kicad_project(
            "hof_frame",
            generate_pcb=False,
            force_regenerate=True,
            update_source_refs=False,
        )
    finally:
        os.chdir(cwd)
    wire_schematic(out_dir / "hof_frame" / "hof_frame.kicad_sch")
    return out_dir / "hof_frame"


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=(__doc__ or "").splitlines()[0])
    parser.add_argument(
        "--kicad",
        nargs="?",
        const=str(Path(__file__).with_name("kicad")),
        metavar="DIR",
        help="generate a KiCad schematic project in DIR/hof_frame (default: docs/kicad)",
    )
    args = parser.parse_args()

    # circuit-synth's decorator is callable at runtime, but its type hints say Circuit.
    build_circuit = cast(Callable[[], Circuit], hof_frame_wiring)
    circuit_obj = build_circuit()
    if args.kicad:
        print(f"Wrote {write_kicad_schematic(circuit_obj, Path(args.kicad).resolve())}")
    else:
        print(circuit_obj.generate_text_netlist())
