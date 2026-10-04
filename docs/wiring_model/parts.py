"""Every component of the design and its static properties, in one enum.

Off-the-shelf modules are modelled as generic connectors, so each pin must
match the received boards. To add a component, add a `Part` member built from
a `PartSpec`; pydantic rejects an incomplete or inconsistent spec at import.
"""

from enum import Enum
from typing import Literal, Self

from circuit_synth import Component
from pydantic import BaseModel, ConfigDict, Field, field_validator

Point = tuple[float, float]

GRID = 2.54
HDR_FP = "Connector_PinHeader_2.54mm:PinHeader_1x{n:02d}_P2.54mm_Vertical"


class PartSpec(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")

    ref: str = Field(pattern=r"^[A-Z]+\d+$")  # schematic reference designator
    symbol: str = Field(pattern=r"^[^:]+:[^:]+$")  # KiCad "Library:Name"
    value: str = Field(min_length=1)
    position: Point  # symbol centre in mm on the A4 sheet
    pin_offsets: dict[int, Point] = Field(min_length=1)  # wired pins, y down
    footprint: str | None = None
    rotation: Literal[0, 90, 180, 270] = 0  # degrees, applied to the placed symbol
    notes: str = ""  # pin meanings and assumptions to verify on the real part

    @field_validator("position")
    @classmethod
    def _on_grid(cls, position: Point) -> Point:
        for coord in position:
            if abs(coord / GRID - round(coord / GRID)) > 1e-6:
                raise ValueError(f"{coord} is not on the {GRID} mm grid")
        return position


def header(pins: int, top: float, **kwargs) -> PartSpec:
    """Generic 1xN connector facing left; `top` is pin 1's library y."""
    return PartSpec(
        symbol=f"Connector_Generic:Conn_01x{pins:02d}",
        footprint=HDR_FP.format(n=pins),
        pin_offsets={n: (-5.08, -top + (n - 1) * GRID) for n in range(1, pins + 1)},
        **kwargs,
    )


def unique_refs[E: Enum](enum_cls: type[E]) -> type[E]:
    """Class decorator: reject two parts sharing a reference designator."""
    refs = [member.value.ref for member in enum_cls]
    if duplicates := {ref for ref in refs if refs.count(ref) > 1}:
        raise ValueError(f"Duplicate reference designators: {sorted(duplicates)}")
    return enum_cls


@unique_refs
class Part(Enum):
    """Every component in the design; each member's value is its `PartSpec`.

    Power flows left to right and the ESP32 board sits at the right-hand end.
    """

    BATTERY_HOLDER = header(
        2,
        0.0,
        ref="J1",
        value="4xAAA holder",
        position=(40.64, 76.2),
        notes="4x AAA NiMH holder with integrated switch: 1 = BAT+ (switched), 2 = BAT-.",
    )
    STEP_UP_DOWN_CONVERTER = header(
        5,
        5.08,
        ref="U1",
        value="Pololu S9V11MACMA #2868",
        position=(91.44, 76.2),
        notes=(
            "1 = VOUT, 2 = GND, 3 = VIN, 4 = EN, 5 = PG. "
            "Its cutoff potentiometer drives EN; PG is unused."
        ),
    )
    SCHOTTKY_DIODE = PartSpec(
        ref="D1",
        symbol="Device:D_Schottky",
        value="1N5822",
        footprint="Diode_THT:D_DO-201AD_P15.24mm_Horizontal",
        position=(127.0, 55.88),
        rotation=180,  # anode faces the converter
        pin_offsets={2: (-3.81, 0.0), 1: (3.81, 0.0)},
        notes="Pin 2 = anode (converter side), pin 1 = cathode (band, controller).",
    )
    CONTROLLER_5V_INPUT = header(
        2,
        0.0,
        ref="J2",
        value="Board 5 V in (USB-C VBUS)",
        position=(154.94, 55.88),  # pins on the board outline
        notes="Board-level 5 V input of the ESP32-133C02; verify the real connector.",
    )
    ESP32_S3_SOC = PartSpec(
        ref="U2",
        symbol="Espressif:ESP32-S3",
        value="ESP32-S3",
        position=(210.82, 96.52),  # inside the board outline
        # Library pin coordinates have y up; offsets here have y down.
        pin_offsets={9: (33.02, -10.16), 17: (33.02, 10.16)},
        notes=(
            "GPIO4/ADC1_CH3 is the proposed battery-sense pin, not confirmed "
            "broken out; only wired pins are modelled and supply pins stay open."
        ),
    )
    WAKE_BUTTON = PartSpec(
        ref="SW2",
        symbol="Switch:SW_Push",
        value="SW2",
        footprint="Button_Switch_SMD:SW_SPST_PTS645",
        position=(261.62, 106.68),  # horizontal, pin 1 on the left
        pin_offsets={1: (-5.08, 0.0), 2: (5.08, 0.0)},
        notes="SW2 on the controller board: GPIO12, triggered by a high level.",
    )
    VOLTAGE_SENSOR = header(
        3,
        2.54,
        ref="U3",
        value="0-25 V divider module",
        position=(91.44, 127.0),
        notes="Passive 0-25 V (5:1) module: 1 = VIN+, 2 = S (output), 3 = GND.",
    )

    @property
    def spec(self) -> PartSpec:
        return self.value

    @property
    def ref(self) -> str:
        return self.value.ref

    @classmethod
    def from_ref(cls, ref: str) -> Self:
        return next(part for part in cls if part.ref == ref)


def build_components() -> dict[Part, Component]:
    """Instantiate every part as a circuit-synth component."""
    return {
        part: Component(
            symbol=part.spec.symbol,
            ref=part.ref,
            value=part.spec.value,
            **({"footprint": part.spec.footprint} if part.spec.footprint else {}),
        )
        for part in Part
    }
