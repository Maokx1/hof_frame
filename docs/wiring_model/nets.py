"""Every net of the design and the part pins it joins, in one enum.

To add a net, add a `Net` member built from a `NetSpec`; pydantic rejects a
malformed spec and `unique_pins` rejects a pin that sits on two nets.
"""

from enum import Enum

from pydantic import BaseModel, ConfigDict, Field

from wiring_model.parts import Part

type Pin = tuple[Part, int]  # (part, pin number)


class NetSpec(BaseModel):
    model_config = ConfigDict(frozen=True, extra="forbid")

    name: str = Field(pattern=r"^[A-Z0-9_]+$")  # label in the netlist and schematic
    pins: tuple[Pin, ...] = Field(min_length=1)
    notes: str = ""


def unique_pins[E: Enum](enum_cls: type[E]) -> type[E]:
    """Class decorator: reject duplicate net names and pins used on two nets."""
    names = [net.value.name for net in enum_cls]
    pins = [f"{part.ref}.{pin}" for net in enum_cls for part, pin in net.value.pins]
    for kind, items in (("net names", names), ("pins", pins)):
        if duplicates := {item for item in items if items.count(item) > 1}:
            raise ValueError(f"Duplicate {kind}: {sorted(duplicates)}")
    return enum_cls


@unique_pins
class Net(Enum):
    """Every net in the design; each member's value is its `NetSpec`."""

    BAT_POS_SWITCHED = NetSpec(
        name="BAT_POS_SWITCHED",
        pins=(
            (Part.BATTERY_HOLDER, 1),
            (Part.STEP_UP_DOWN_CONVERTER, 3),
            (Part.VOLTAGE_SENSOR, 1),  # battery side, before the converter
        ),
        notes=(
            "After the holder's integrated switch. Optional fuse/PTC is omitted; "
            "insert it between here and the converter once the load is measured."
        ),
    )
    GND = NetSpec(
        name="GND",
        pins=(
            (Part.BATTERY_HOLDER, 2),
            (Part.STEP_UP_DOWN_CONVERTER, 2),
            (Part.CONTROLLER_5V_INPUT, 2),
            (Part.EE04_ADC_PADS, 2),
            (Part.VOLTAGE_SENSOR, 3),
        ),
    )
    EE04_5V = NetSpec(
        name="EE04_5V",
        pins=((Part.STEP_UP_DOWN_CONVERTER, 1), (Part.CONTROLLER_5V_INPUT, 1)),
        notes="S9V11MACMA 5 V output to the EE04 USB-C input; no isolation diode.",
    )
    VBAT_SENSE = NetSpec(
        name="VBAT_SENSE",
        pins=((Part.EE04_ADC_PADS, 1), (Part.VOLTAGE_SENSOR, 2)),
        notes=(
            "External NiMH divider output to XIAO D9/GPIO8 (ADC1 candidate); "
            "check pad access on the physical EE04 before wiring."
        ),
    )

    @property
    def spec(self) -> NetSpec:
        return self.value
