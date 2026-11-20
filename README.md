# hof_frame

Hall of Fame frame with E-ink display

1. [Detailed plan can be found here](docs/PLAN.md).
2. [Electrical documentation can be found here](docs/WIRING.md).
3. [Firmware (ESP-IDF-backed Rust) can be found here](firmware/README.md).
4. [Parametric Onshape enclosure](docs/enclosure/hof_frame.fs).

The enclosure centers its outer frame on the display opening for equal visible
borders. **Minimum border outside panel** controls the minimum clearance from
the panel to the outside of the frame; the visible borders are wider. The
**Inner front round-over radius** rounds the opening while leaving the outer
corners square (set it to zero for a flat front). Check the fit and print a
test section before printing the full enclosure.

## Wiring source of truth

The proposed electrical connections are modeled in code with
[circuit-synth](https://www.circuit-synth.com/) in
[`docs/wiring_model/`](docs/wiring_model). [`docs/WIRING.md`](docs/WIRING.md)
provides the connection schedule, bill of materials, and bring-up instructions.

The Python environment uses [uv](https://docs.astral.sh/uv/). circuit-synth
requires KiCad's `Connector_Generic` and `power` symbol libraries. Download them to the
git-ignored `.kicad/symbols` directory:

```sh
mkdir -p .kicad/symbols
for lib in Connector_Generic power; do
  curl -fsSL "https://gitlab.com/kicad/libraries/kicad-symbols/-/raw/9.0.0/$lib.kicad_sym" \
    -o ".kicad/symbols/$lib.kicad_sym"
done
```

Then print the net list:

```sh
KICAD_SYMBOL_DIR=.kicad/symbols uv run wiring
```

Save your edits and close the KiCad schematic editor before generating the
project in `docs/kicad/hof_frame` (requires `kicad-cli`):

```sh
KICAD_SYMBOL_DIR=.kicad/symbols uv run wiring --kicad
```

`--kicad DIR` writes to `DIR/hof_frame` instead. Regeneration replaces the
generated top-level schematic, its PDF (`hof_frame.pdf`, the project wiring page
only, without Seeed's pages, for viewing without KiCad), circuit-synth JSON, and
full KiCad netlist. It
preserves project settings and vendor circuitry, updating only project-specific
page metadata in the vendor hierarchy. Before replacing files, it checks for
an open editor and for changes made during generation. Open the `.kicad_pro` in
KiCad; see the
[wiring documentation](docs/WIRING.md#kicad-wiring-model) for hierarchy
navigation, attribution, and the distinction between the model and full netlist.

The bundled Seeed schematic is an **EE04 V1.2 snapshot** and is not automatically
updated. Before using this wiring, check the
[Seeed wiki's Resources section](https://wiki.seeedstudio.com/epaper_ee04/#resources)
for schematic revisions and confirm your board's revision; newer revisions may
require different wiring.

To check the generated connections and regeneration behavior:

```sh
KICAD_SYMBOL_DIR=.kicad/symbols uv run python -m unittest discover -s docs/wiring_model/tests -p test_wiring_schematic.py
```
