"""KiCad project generation and post-processing of the schematic."""

import logging
import os
import re
from pathlib import Path

from wiring_model.layout import (
    BAT_JUNCTION,
    BOARD_BOX,
    install_layout,
    net_routes,
)
from wiring_model.nets import Net
from wiring_model.parts import Part

install_layout()


class _SingleEndedNetFilter(logging.Filter):
    """Drop circuit-synth's 'only 1 connection' warning for nets that are
    single-ended by design (the other end is off-board or not modelled)."""

    def __init__(self) -> None:
        super().__init__()
        self._markers = [
            f"Net '{net.spec.name}' has only" for net in Net if len(net.spec.pins) == 1
        ]

    def filter(self, record: logging.LogRecord) -> bool:
        message = record.getMessage()
        return not any(marker in message for marker in self._markers)


logging.getLogger("circuit_synth.kicad.netlist_service.CircuitReconstructor").addFilter(
    _SingleEndedNetFilter()
)


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


def _apply_rotations(text: str) -> str:
    for part in Part:
        spec = part.spec
        if spec.rotation:
            x, y = spec.position
            text = re.sub(
                rf'(\(lib_id "{re.escape(spec.symbol)}"\)\s*\(at )[\d.]+ [\d.]+ 0\)',
                rf"\g<1>{x} {y} {spec.rotation})",
                text,
            )
    return text


def _wire_items() -> list[str]:
    items = []
    for net, (polylines, label_at) in net_routes().items():
        name = net.spec.name
        for line in polylines:
            for (x1, y1), (x2, y2) in zip(line, line[1:]):
                items.append(
                    f"\t(wire (pts (xy {x1:.2f} {y1:.2f}) (xy {x2:.2f} {y2:.2f}))"
                    " (stroke (width 0) (type default)))"
                )
        for lx, ly, *side in label_at if isinstance(label_at, list) else [label_at]:
            justify = f"{side[0] if side else 'left'} bottom"
            items.append(
                f'\t(label "{name}" (at {lx:.2f} {ly:.2f} 0)'
                f" (effects (font (size 1.27 1.27)) (justify {justify})))"
            )
    jx, jy = BAT_JUNCTION
    items.append(f"\t(junction (at {jx} {jy}) (diameter 0) (color 0 0 0 0))")
    return items


def _board_items() -> list[str]:
    left, top, right, bottom = BOARD_BOX
    return [
        f"\t(rectangle (start {left} {top}) (end {right} {bottom})"
        " (stroke (width 0.254) (type dash)) (fill (type none)))",
        f'\t(text "ESP32-133C02 board" (at {left + 2.54} {top + 5.08} 0)'
        " (effects (font (size 2.54 2.54) (bold yes)) (justify left)))",
        '\t(text "5 V -> on-board regulators -> ESP32-S3 3V3 (not modelled)"'
        f" (at {left + 2.54} {bottom - 3.81} 0)"
        " (effects (font (size 1.27 1.27) italic) (justify left)))",
    ]


def wire_schematic(sch: Path) -> None:
    """Replace circuit-synth's net labels with real wires between the parts."""
    text = _drop_blocks(sch.read_text(), "hierarchical_label")
    text = text.replace('(paper "A3")', '(paper "A4")')  # the layout fits A4
    text = _apply_rotations(text)
    body = text.rstrip()
    assert body.endswith(")")
    items = _wire_items() + _board_items()
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
