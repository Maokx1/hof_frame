"""Schematic placement, pin geometry and wire routes (mm, A4 sheet)."""

from typing import Any

from circuit_synth.kicad.sch_gen.collision_manager import CollisionManager
from circuit_synth.kicad.schematic import text_flow_placement

from wiring_model.nets import Net
from wiring_model.parts import Part

# Dashed outline of the ESP32-133C02 board: (left, top, right, bottom).
BOARD_BOX = (149.86, 40.64, 284.48, 152.4)
# Battery net: the left loop, the trunk feeding U1/U3, and where they meet.
BAT_LOOP_X, BAT_TRUNK_X, BAT_BELOW_J1 = 27.94, 83.82, 86.36
BAT_JUNCTION = (BAT_TRUNK_X, BAT_BELOW_J1)

type Route = tuple[list[list[tuple[float, float]]], Any]


def spread_layout(component_bboxes, spacing=2.54):
    """Replace circuit-synth's cramped text-flow placement with the positions in `Part`."""
    placements = [
        (key, *Part.from_ref(key.split("_")[0]).spec.position)
        for key, _w, _h in component_bboxes
    ]
    return placements, "A4"


def skip_sequential_placement(self, symbol_width, symbol_height):
    """circuit-synth's first-pass placement is overwritten by `spread_layout`."""
    return self.current_x, self.current_y


def install_layout() -> None:
    text_flow_placement.place_with_text_flow = spread_layout
    CollisionManager.place_symbol = skip_sequential_placement


def pin(ref: Part, number: int) -> tuple[float, float]:
    """Connection point of a wired pin."""
    spec = ref.spec
    (cx, cy), (dx, dy) = spec.position, spec.pin_offsets[number]
    return cx + dx, cy + dy


def power_routes() -> dict[Net, Route]:
    """Battery -> converter -> diode -> board 5 V input."""
    j1, u1_vout, u1_vin = (
        pin(Part.BATTERY_HOLDER, 1),
        pin(Part.STEP_UP_DOWN_CONVERTER, 1),
        pin(Part.STEP_UP_DOWN_CONVERTER, 3),
    )
    u3_vin = pin(Part.VOLTAGE_SENSOR, 1)
    d1_anode, d1_cathode = pin(Part.SCHOTTKY_DIODE, 2), pin(Part.SCHOTTKY_DIODE, 1)
    j2_5v = pin(Part.CONTROLLER_5V_INPUT, 1)
    return {
        Net.BAT_POS_SWITCHED: (
            [
                [
                    j1,
                    (BAT_LOOP_X, j1[1]),
                    (BAT_LOOP_X, BAT_BELOW_J1),
                    (BAT_TRUNK_X, BAT_BELOW_J1),
                ],
                [u1_vin, (BAT_TRUNK_X, u1_vin[1]), (BAT_TRUNK_X, u3_vin[1]), u3_vin],
            ],
            (50.8, BAT_BELOW_J1),
        ),
        Net.REG_5V: (
            [[u1_vout, (80.01, u1_vout[1]), (80.01, d1_anode[1]), d1_anode]],
            (95.25, d1_anode[1]),
        ),
        Net.CTRL_5V: ([[d1_cathode, j2_5v]], (134.62, d1_cathode[1])),
    }


def sense_routes() -> dict[Net, Route]:
    """Voltage sensor output to the ESP32 ADC pin."""
    stub = 10.16  # short labelled stubs: a long wire would cross WAKE
    u3_out, gpio4 = pin(Part.VOLTAGE_SENSOR, 2), pin(Part.ESP32_S3_SOC, 9)
    return {
        Net.VBAT_SENSE: (
            [
                [u3_out, (u3_out[0] - stub, u3_out[1])],
                [gpio4, (gpio4[0] + stub, gpio4[1])],
            ],
            [
                (u3_out[0] - stub, u3_out[1], "right"),
                (gpio4[0] + stub, gpio4[1], "left"),
            ],
        ),
    }


def wake_routes() -> dict[Net, Route]:
    """SW2 wake button wiring."""
    gpio12, sw1, sw2_pin = (
        pin(Part.ESP32_S3_SOC, 17),
        pin(Part.WAKE_BUTTON, 1),
        pin(Part.WAKE_BUTTON, 2),
    )
    return {
        Net.WAKE: ([[gpio12, sw1]], (gpio12[0] + 2.54, gpio12[1])),
        Net.BOARD_3V3: (
            [[sw2_pin, (sw2_pin[0] + 2.54, sw2_pin[1])]],
            (sw2_pin[0] + 2.54, sw2_pin[1]),
        ),
    }


def net_routes() -> dict[Net, Route]:
    """Orthogonal wire polylines per net, plus where to put the net label."""
    return {**power_routes(), **sense_routes(), **wake_routes()}
