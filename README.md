# hof_frame

Hall of Fame frame with E-ink display

1. [Detailed plan can be found here](docs/PLAN.md).
2. [Electrical documentation can be found here](docs/WIRING.md).
3. [Firmware (ESP-IDF-backed Rust) can be found here](firmware/README.md).

## Wiring source of truth

The proposed electrical connections are modeled in code with
[circuit-synth](https://www.circuit-synth.com/) in
[`docs/wiring_model/`](docs/wiring_model). [`docs/WIRING.md`](docs/WIRING.md)
provides the connection schedule, bill of materials, and bring-up instructions.

The Python environment uses [uv](https://docs.astral.sh/uv/). circuit-synth
requires KiCad's `Connector_Generic`, `Device`, and `Switch` symbol libraries,
plus Espressif's `Espressif` library for the ESP32-S3. If KiCad isn't installed,
download these libraries to the git-ignored `.kicad/symbols` directory:

```sh
mkdir -p .kicad/symbols && cd .kicad/symbols
for l in Connector_Generic Device Switch; do
  curl -fsSLO "https://gitlab.com/kicad/libraries/kicad-symbols/-/raw/9.0.0/$l.kicad_sym"
done
curl -fsSLO "https://raw.githubusercontent.com/espressif/kicad-libraries/main/symbols/Espressif.kicad_sym"
cd ../..
```

Then print the net list:

```sh
KICAD_SYMBOL_DIR=.kicad/symbols uv run wiring
```

To generate the KiCad schematic project in `docs/kicad/hof_frame` (open the
`.kicad_pro` in KiCad; the KiCad symbol and footprint libraries must be
configured there):

```sh
KICAD_SYMBOL_DIR=.kicad/symbols uv run wiring --kicad
```

`--kicad DIR` writes to `DIR/hof_frame` instead. The output is regenerated from
`wiring_model` and overwrites manual edits.
