"""Hall of Fame frame wiring, captured as a circuit-synth design.

Mirrors the connection schedule in WIRING.md. Off-the-shelf modules are
modelled as generic connectors, so each pin must match the received boards.
Do not treat this as verified until the converter output has been measured.

Needs the KiCad symbol libraries (Connector_Generic, Device, power, Switch) and
Espressif.kicad_sym from espressif/kicad-libraries; run
from the repository root:

    KICAD_SYMBOL_DIR=.kicad/symbols uv run python -m wiring_model         # net list
    KICAD_SYMBOL_DIR=.kicad/symbols uv run python -m wiring_model --kicad # docs/kicad/hof_frame
"""

import argparse
import os
from collections.abc import Callable
from pathlib import Path
from typing import cast

from circuit_synth import Circuit

from wiring_model.circuit import hof_frame_wiring
from wiring_model.kicad import write_kicad_schematic


def main() -> None:
    parser = argparse.ArgumentParser(description=(__doc__ or "").splitlines()[0])
    parser.add_argument(
        "--kicad",
        nargs="?",
        const=str(Path(__file__).parent.parent / "kicad"),
        metavar="DIR",
        help="generate a KiCad schematic project in DIR/hof_frame (default: docs/kicad)",
    )
    args = parser.parse_args()

    # write_kicad_schematic() changes directory, so a relative path would break
    if "KICAD_SYMBOL_DIR" in os.environ:
        os.environ["KICAD_SYMBOL_DIR"] = str(
            Path(os.environ["KICAD_SYMBOL_DIR"]).resolve()
        )

    # circuit-synth's decorator is callable at runtime, but its type hints say Circuit.
    build_circuit = cast(Callable[[], Circuit], hof_frame_wiring)
    circuit_obj = build_circuit()
    if args.kicad:
        print(f"Wrote {write_kicad_schematic(circuit_obj, Path(args.kicad).resolve())}")
    else:
        print(circuit_obj.generate_text_netlist())


if __name__ == "__main__":
    main()
