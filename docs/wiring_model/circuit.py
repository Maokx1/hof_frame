"""Builds the circuit-synth design from the `Part` and `Net` enums."""

from circuit_synth import Net as CircuitNet
from circuit_synth import circuit

from wiring_model.nets import Net
from wiring_model.parts import build_components


@circuit(name="hof_frame", comments=False)
def hof_frame_wiring():
    """Battery -> S9V11MACMA -> 1N5822 -> ESP32-133C02, plus pack-voltage sense."""
    components = build_components()
    for net in Net:
        circuit_net = CircuitNet(net.spec.name)
        for part, pin in net.spec.pins:
            circuit_net += components[part][pin]
