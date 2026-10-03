# hof_frame

Hall of Fame frame with E-ink display

1. [Detailed plan can be found here](docs/PLAN.md).
2. [Electrical documentation can be found here](docs/WIRING.md).
3. [Firmware (ESP-IDF-backed Rust) can be found here](firmware/README.md).

## Wiring source of truth

The wiring is being captured as code with [circuit-synth](https://www.circuit-synth.com/)
in [`docs/wiring.py`](docs/wiring.py). It records the proposed electrical
connections; [`docs/WIRING.md`](docs/WIRING.md) keeps the connection schedule,
prose, bill of materials and bring-up procedure.

The Python environment is managed with [uv](https://docs.astral.sh/uv/).
circuit-synth needs KiCad symbol libraries (`Connector_Generic`, `Device`,
`power`) plus Espressif's `Espressif` library for the ESP32-S3 symbol. Without a KiCad install, download them into the git-ignored
`.kicad/symbols` directory:

```sh
mkdir -p .kicad/symbols && cd .kicad/symbols
for l in Connector_Generic Device power; do
  curl -fsSLO "https://gitlab.com/kicad/libraries/kicad-symbols/-/raw/9.0.0/$l.kicad_sym"
done
curl -fsSLO "https://raw.githubusercontent.com/espressif/kicad-libraries/main/symbols/Espressif.kicad_sym"
cd ../..
```

Then print the net list:

```sh
KICAD_SYMBOL_DIR=.kicad/symbols uv run python docs/wiring.py
```

To generate the KiCad schematic project in `docs/kicad/hof_frame` (open the
`.kicad_pro` in KiCad; the KiCad symbol and footprint libraries must be
configured there):

```sh
KICAD_SYMBOL_DIR=.kicad/symbols uv run python docs/wiring.py --kicad
```

`--kicad DIR` writes to `DIR/hof_frame` instead. The output is regenerated from
`wiring.py` and overwrites manual edits.
