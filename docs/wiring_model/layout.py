"""Schematic placement, pin geometry and wire routes (mm, A4 sheet)."""

from typing import Any

from circuit_synth.kicad.sch_gen.collision_manager import CollisionManager
from circuit_synth.kicad.schematic import text_flow_placement

from wiring_model.nets import Net
from wiring_model.parts import Part

# The abstract board connectors in the circuit model become these sheet pins.
BOARD_BOX = (165.1, 55.88, 228.6, 114.3)
BAT_LOOP_X, BAT_TRUNK_X, BAT_BELOW_J1 = 20.32, 73.66, 96.52
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
    """Battery -> converter -> EE04 USB-C 5 V input."""
    battery_positive, converter_out, converter_in = (
        pin(Part.BATTERY_HOLDER, 1),
        pin(Part.STEP_UP_DOWN_CONVERTER, 1),
        pin(Part.STEP_UP_DOWN_CONVERTER, 3),
    )
    divider_in = pin(Part.VOLTAGE_SENSOR, 1)
    board_5v = pin(Part.CONTROLLER_5V_INPUT, 1)
    return {
        Net.BAT_POS_SWITCHED: (
            [
                [
                    battery_positive,
                    (BAT_LOOP_X, battery_positive[1]),
                    (BAT_LOOP_X, BAT_BELOW_J1),
                    BAT_JUNCTION,
                ],
                [
                    converter_in,
                    (BAT_TRUNK_X, converter_in[1]),
                    BAT_JUNCTION,
                ],
                [
                    BAT_JUNCTION,
                    (BAT_TRUNK_X, divider_in[1]),
                    divider_in,
                ],
            ],
            (35.56, BAT_BELOW_J1),
        ),
        Net.EE04_5V: (
            [
                [
                    converter_out,
                    (78.74, converter_out[1]),
                    (78.74, 58.42),
                    (142.24, 58.42),
                    (142.24, board_5v[1]),
                    board_5v,
                ]
            ],
            (99.06, 58.42),
        ),
    }


def sense_routes() -> dict[Net, Route]:
    """Voltage sensor output to the EE04/XIAO ADC pad."""
    divider_out = pin(Part.VOLTAGE_SENSOR, 2)
    adc_pad = pin(Part.EE04_ADC_PADS, 1)
    return {
        Net.VBAT_SENSE: (
            [
                [
                    divider_out,
                    (68.58, divider_out[1]),
                    (68.58, 142.24),
                    (142.24, 142.24),
                    (142.24, adc_pad[1]),
                    adc_pad,
                ]
            ],
            (88.9, 142.24),
        ),
    }


def net_routes() -> dict[Net, Route]:
    """Orthogonal wire polylines per net, plus where to put the net label."""
    return {**power_routes(), **sense_routes()}
