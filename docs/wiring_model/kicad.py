"""KiCad project generation and post-processing of the schematic."""

import json
import logging
import math
import os
import re
import shutil
import subprocess
import tempfile
import xml.etree.ElementTree as ET
from collections import Counter
from pathlib import Path
from uuid import NAMESPACE_URL, UUID, uuid5

from wiring_model.layout import (
    BAT_JUNCTION,
    BOARD_BOX,
    install_layout,
    net_routes,
    pin,
)
from wiring_model.nets import Net
from wiring_model.parts import Part

install_layout()

VENDOR_DIRECTORY = "seeed-ee04-v1.2"
VENDOR_ROOT = "XIAO ePaper Display Board EE04_V1.2.kicad_sch"
VENDOR_SHEETS = (
    VENDOR_ROOT,
    "01 Descriptions.kicad_sch",
    "02 Block Diagram.kicad_sch",
    "Power.kicad_sch",
    "XIAO.kicad_sch",
    "E-ink.kicad_sch",
)
VENDOR_PAGES = {name: str(index + 3) for index, name in enumerate(VENDOR_SHEETS[1:])}
BUNDLED_VENDOR = (
    Path(__file__).resolve().parents[1] / "kicad" / "hof_frame" / VENDOR_DIRECTORY
)


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


def _place_references(text: str) -> str:
    x, y = Part.BATTERY_HOLDER.spec.position
    pattern = (
        rf'(\(property "Reference" "{Part.BATTERY_HOLDER.ref}"\s*\(at )[\d.]+ [\d.]+'
    )
    text, count = re.subn(pattern, rf"\g<1>{x} {y + 7.62:.2f}", text)
    if count != 1:
        raise ValueError("Could not place the battery holder reference")
    return text


def _item_uuid(name: str) -> str:
    return str(uuid5(NAMESPACE_URL, f"urn:hof-frame:ee04:{name}"))


def _schematic_uuid(text: str) -> str:
    root = re.search(r'(?m)^[ \t]*\(uuid "([^"]+)"\)', text)
    if root is None:
        raise ValueError("Schematic has no root UUID")
    return str(UUID(root.group(1)))


def _existing_root_uuid(destination: Path) -> str | None:
    project = destination / "hof_frame.kicad_pro"
    if project.exists():
        settings = json.loads(project.read_text())
        top_sheets = settings.get("schematic", {}).get("top_level_sheets", [])
        if top_sheets:
            if (
                len(top_sheets) != 1
                or not isinstance(top_sheets[0], dict)
                or Path(top_sheets[0].get("filename", ""))
                != Path("hof_frame.kicad_sch")
            ):
                raise ValueError(
                    "Cannot regenerate a KiCad project with different top-level sheets"
                )
            root = UUID(top_sheets[0]["uuid"])
            if root.int:
                return str(root)

    schematic = destination / "hof_frame.kicad_sch"
    return _schematic_uuid(schematic.read_text()) if schematic.exists() else None


def _sexpr_children(text: str) -> list[tuple[str, int, int]]:
    """Locate immediate S-expression children without rewriting other content."""
    children: list[tuple[str, int, int]] = []
    depth = 0
    start = 0
    quoted = False
    escaped = False
    for index, char in enumerate(text):
        if quoted:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                quoted = False
        elif char == '"':
            quoted = True
        elif char == "(":
            if depth == 1:
                start = index
            depth += 1
        elif char == ")":
            if depth == 2:
                keyword = re.match(r"\(\s*([^\s()]+)", text[start : index + 1])
                if keyword is None:
                    raise ValueError("Invalid KiCad S-expression")
                children.append((keyword.group(1), start, index + 1))
            depth -= 1
            if depth < 0:
                raise ValueError("Invalid KiCad S-expression")
    if depth or quoted:
        raise ValueError("Incomplete KiCad S-expression")
    return children


def _single_child(text: str, keyword: str) -> tuple[int, int] | None:
    matches = [
        (start, end) for name, start, end in _sexpr_children(text) if name == keyword
    ]
    if len(matches) > 1:
        raise ValueError(f"Multiple {keyword} blocks in KiCad S-expression")
    return matches[0] if matches else None


def _append_child(text: str, child: str, newline: str) -> str:
    child = child.replace("\n", newline)
    line = text.rfind("\n") + 1
    if not text[line:-1].strip():
        return text[:line] + child + newline + text[line:]
    return text[:-1] + newline + child + newline + ")"


def _update_instance_path(instance: str, path: str, page: str, newline: str) -> str:
    location = re.match(r'^\(path\s+"([^"]+)"', instance)
    if location is None:
        raise ValueError("Invalid KiCad sheet instance path")
    if location.group(1) != path:
        instance = instance[: location.start(1)] + path + instance[location.end(1) :]

    page_span = _single_child(instance, "page")
    if page_span is None:
        return _append_child(instance, f'\t\t\t\t\t(page "{page}")', newline)
    start, end = page_span
    current = re.fullmatch(r'\(page\s+"([^"]+)"\s*\)', instance[start:end])
    if current is None:
        raise ValueError("Invalid KiCad sheet instance page")
    if current.group(1) != page:
        instance = instance[:start] + f'(page "{page}")' + instance[end:]
    return instance


def _update_host_project(project: str, path: str, page: str, newline: str) -> str:
    vendor_id = _item_uuid("vendor-sheet")
    matches: list[tuple[int, int]] = []
    for keyword, start, end in _sexpr_children(project):
        if keyword == "path":
            location = re.match(r'^\(path\s+"([^"]+)"', project[start:end])
            if location is None:
                raise ValueError("Invalid KiCad sheet instance path")
            if location.group(1).rstrip("/").split("/")[-1] == vendor_id:
                matches.append((start, end))

    if not matches:
        return _append_child(
            project, f'\t\t\t\t(path "{path}" (page "{page}"))', newline
        )

    first, *duplicates = matches
    for start, end in reversed(duplicates):
        project = project[:start] + project[end:]
    start, end = first
    updated = _update_instance_path(project[start:end], path, page, newline)
    return project[:start] + updated + project[end:]


def _update_sheet_instance(sheet: str, path: str, page: str, newline: str) -> str:
    span = _single_child(sheet, "instances")
    if span is None:
        return _append_child(
            sheet,
            "\t\t(instances\n"
            '\t\t\t(project "hof_frame"\n'
            f'\t\t\t\t(path "{path}" (page "{page}"))\n'
            "\t\t\t)\n\t\t)",
            newline,
        )

    start, end = span
    instances = sheet[start:end]
    projects = [
        (project_start, project_end)
        for keyword, project_start, project_end in _sexpr_children(instances)
        if keyword == "project"
        and re.match(r'^\(project\s+"hof_frame"(?=\s|\))', instances[project_start:])
    ]
    if len(projects) > 1:
        raise ValueError("Multiple hof_frame sheet instance blocks")
    if projects:
        project_start, project_end = projects[0]
        project = _update_host_project(
            instances[project_start:project_end], path, page, newline
        )
        instances = instances[:project_start] + project + instances[project_end:]
    else:
        instances = _append_child(
            instances,
            '\t\t\t(project "hof_frame"\n'
            f'\t\t\t\t(path "{path}" (page "{page}"))\n'
            "\t\t\t)",
            newline,
        )
    return sheet[:start] + instances + sheet[end:]


def _update_vendor_instances(schematic: Path, root_uuid: str) -> None:
    original = schematic.read_bytes()
    source = original.decode("utf-8")
    newline = "\r\n" if "\r\n" in source else "\n"
    path = f"/{root_uuid}/{_item_uuid('vendor-sheet')}"
    found: set[str] = set()
    replacements = []
    for keyword, start, end in _sexpr_children(source):
        if keyword != "sheet":
            continue
        sheet = source[start:end]
        names = [
            re.match(r'^\(property\s+"Sheetfile"\s+"([^"]+)"', sheet[a:b])
            for name, a, b in _sexpr_children(sheet)
            if name == "property"
        ]
        files = [match.group(1) for match in names if match is not None]
        if len(files) != 1 or files[0] not in VENDOR_PAGES or files[0] in found:
            raise ValueError("Unexpected EE04 vendor sheet hierarchy")
        filename = files[0]
        found.add(filename)
        replacements.append(
            (
                start,
                end,
                _update_sheet_instance(sheet, path, VENDOR_PAGES[filename], newline),
            )
        )
    if found != VENDOR_PAGES.keys():
        raise ValueError(
            f"Missing EE04 vendor sheet instances: {sorted(VENDOR_PAGES.keys() - found)}"
        )
    for start, end, sheet in reversed(replacements):
        source = source[:start] + sheet + source[end:]
    updated = source.encode("utf-8")
    if updated != original:
        schematic.write_bytes(updated)


def _ensure_editor_closed(destination: Path) -> None:
    if (destination / "~hof_frame.kicad_sch.lck").exists():
        raise RuntimeError(
            f"Close the KiCad schematic editor before regenerating {destination}"
        )


def _snapshot_files(paths: set[Path]) -> dict[Path, bytes | None]:
    return {path: path.read_bytes() if path.exists() else None for path in paths}


def _ensure_files_unchanged(snapshot: dict[Path, bytes | None]) -> None:
    changed = [
        path
        for path, contents in snapshot.items()
        if (path.read_bytes() if path.exists() else None) != contents
    ]
    if changed:
        raise RuntimeError(
            f"Project files changed during regeneration; no files were replaced: {changed}"
        )


def _drop_board_placeholders(text: str) -> str:
    """Replace abstract board connectors with sheet pins, retaining one GND."""
    placeholders = {Part.CONTROLLER_5V_INPUT.ref, Part.EE04_ADC_PADS.ref}
    adc_ground = pin(Part.EE04_ADC_PADS, 2)
    removed: set[str] = set()

    def remove_placeholder(match: re.Match[str]) -> str:
        block = match.group()
        for ref in placeholders:
            if f'(property "Reference" "{ref}"' in block:
                removed.add(ref)
                return ""
        if '(lib_id "power:GND")' in block:
            location = re.search(r"\(at ([\d.]+) ([\d.]+) ", block)
            if location and all(
                math.isclose(actual, expected, abs_tol=1e-6)
                for actual, expected in zip(map(float, location.groups()), adc_ground)
            ):
                removed.add("adc-ground")
                return ""
        return block

    text = re.sub(r"(?ms)^\t\(symbol\n.*?^\t\)", remove_placeholder, text)
    if removed != placeholders | {"adc-ground"}:
        raise ValueError(f"Could not replace the board placeholders: {sorted(removed)}")
    return text


def _wire_items() -> list[str]:
    items = []
    for net, (polylines, label_at) in net_routes().items():
        name = net.spec.name
        for line_index, line in enumerate(polylines):
            for segment_index, ((x1, y1), (x2, y2)) in enumerate(zip(line, line[1:])):
                items.append(
                    f"\t(wire (pts (xy {x1:.2f} {y1:.2f}) (xy {x2:.2f} {y2:.2f}))"
                    " (stroke (width 0) (type default))"
                    f' (uuid "{_item_uuid(f"{name}-{line_index}-{segment_index}")}"))'
                )
        for index, (lx, ly, *side) in enumerate(
            label_at if isinstance(label_at, list) else [label_at]
        ):
            justify = f"{side[0] if side else 'left'} bottom"
            items.append(
                f'\t(label "{name}" (at {lx:.2f} {ly:.2f} 0)'
                f" (effects (font (size 1.27 1.27)) (justify {justify}))"
                f' (uuid "{_item_uuid(f"{name}-label-{index}")}"))'
            )
    jx, jy = BAT_JUNCTION
    items.append(
        f"\t(junction (at {jx} {jy}) (diameter 0) (color 0 0 0 0)"
        f' (uuid "{_item_uuid("battery-junction")}"))'
    )
    return items


def _text(x: float, y: float, value: str, size: float = 1.27) -> str:
    return (
        f'\t(text "{value}" (at {x} {y} 0)'
        f" (effects (font (size {size} {size})) (justify left))"
        f' (uuid "{_item_uuid(f"text-{x}-{y}-{value}")}"))'
    )


def _board_items(root_uuid: str) -> list[str]:
    left, top, right, bottom = BOARD_BOX
    ports = (
        ("USB_5V", "input", pin(Part.CONTROLLER_5V_INPUT, 1)),
        ("GND", "passive", pin(Part.CONTROLLER_5V_INPUT, 2)),
        ("GPIO8_D9", "input", pin(Part.EE04_ADC_PADS, 1)),
    )
    if any(not math.isclose(x, left, abs_tol=1e-6) for _, _, (x, _) in ports):
        raise ValueError("EE04 sheet pins must lie on the left edge of the sheet")
    items = [
        "\t(sheet",
        f"\t\t(at {left} {top}) (size {right - left:.2f} {bottom - top:.2f})",
        "\t\t(exclude_from_sim no) (in_bom yes) (on_board yes) (dnp no)",
        "\t\t(stroke (width 0.1524) (type solid)) (fill (color 0 0 0 0))",
        f'\t\t(uuid "{_item_uuid("vendor-sheet")}")',
        f'\t\t(property "Sheetname" "Seeed EE04 v1.2" (at {left} {top - 1.27} 0)',
        "\t\t\t(effects (font (size 1.27 1.27)) (justify left bottom)))",
        f'\t\t(property "Sheetfile" "{VENDOR_DIRECTORY}/{VENDOR_ROOT}"',
        f"\t\t\t(at {left} {bottom + 1.27} 0)",
        "\t\t\t(effects (font (size 1.27 1.27)) (justify left top) (hide yes)))",
    ]
    for name, shape, (x, y) in ports:
        items.extend(
            [
                f'\t\t(pin "{name}" {shape} (at {x:.2f} {y:.2f} 180)',
                "\t\t\t(effects (font (size 1.27 1.27)) (justify left))",
                f'\t\t\t(uuid "{_item_uuid(f"sheet-pin-{name}")}"))',
            ]
        )
    items.extend(
        [
            f'\t\t(instances (project "hof_frame" (path "/{root_uuid}" (page "2"))))',
            "\t)",
            _text(27.94, 43.18, "NiMH supply and EE04", 2.0),
            _text(170.18, 83.82, "Editable Seeed board schematic", 1.0),
            _text(170.18, 88.9, "Power / XIAO / display circuits", 1.0),
            _text(170.18, 93.98, "Purchased assembly", 1.0),
            _text(170.18, 99.06, "Seeed Studio - CC BY-SA 4.0", 1.0),
            _text(
                27.94, 154.94, "Switch off the battery before connecting computer USB."
            ),
            _text(27.94, 160.02, "Leave the EE04 Li-ion battery connector empty."),
            _text(
                27.94, 165.1, "Verify GPIO8 pad access and the divider before assembly."
            ),
        ]
    )
    for part, names in (
        (Part.STEP_UP_DOWN_CONVERTER, ("VOUT", "GND", "VIN", "EN", "PG")),
        (Part.VOLTAGE_SENSOR, ("VIN+", "S", "GND")),
    ):
        for number, name in enumerate(names, 1):
            x, y = pin(part, number)
            items.append(_text(x + 12.7, y, name, 1.0))
    for number in (4, 5):
        x, y = pin(Part.STEP_UP_DOWN_CONVERTER, number)
        items.append(
            f'\t(no_connect (at {x} {y}) (uuid "{_item_uuid(f"converter-nc-{number}")}"))'
        )
    return items


def wire_schematic(sch: Path, root_uuid: str) -> None:
    """Replace circuit-synth's net labels with real wires between the parts."""
    text = sch.read_text()
    text = text.replace(_schematic_uuid(text), root_uuid)
    text = _drop_blocks(text, "hierarchical_label")
    text = text.replace('(paper "A3")', '(paper "A4")')  # the layout fits A4
    text = _apply_rotations(text)
    text = _place_references(text)
    text = _drop_board_placeholders(text)
    body = text.rstrip()
    if not body.endswith(")"):
        raise ValueError("Generated schematic is incomplete")
    items = _wire_items() + _board_items(root_uuid)
    schematic = body[:-1] + "\n".join(items) + "\n)\n"
    sch.write_text("\n".join(line.rstrip() for line in schematic.splitlines()) + "\n")


def _validate_hierarchy(netlist: Path) -> None:
    """Reject missing, shorted, or incorrectly connected external interfaces."""
    root = ET.parse(netlist).getroot()
    references = [
        component.attrib["ref"] for component in root.findall("./components/comp")
    ]
    duplicates = sorted(ref for ref, count in Counter(references).items() if count > 1)
    if duplicates:
        raise ValueError(f"Duplicate schematic references: {duplicates}")

    nodes: dict[tuple[str, str], str] = {}
    for net in root.findall("./nets/net"):
        for node in net.findall("node"):
            nodes[(node.attrib["ref"], node.attrib["pin"])] = net.attrib["code"]

    def net_of(ref: str, number: int) -> str:
        try:
            return nodes[(ref, str(number))]
        except KeyError as error:
            raise ValueError(f"Missing schematic pin {ref}.{number}") from error

    groups = {
        "switched battery": (
            (Part.BATTERY_HOLDER.ref, 1),
            (Part.STEP_UP_DOWN_CONVERTER.ref, 3),
            (Part.VOLTAGE_SENSOR.ref, 1),
        ),
        "USB input": (
            (Part.STEP_UP_DOWN_CONVERTER.ref, 1),
            ("U3", 14),
            ("F1", 1),
        ),
        "ground": (
            (Part.BATTERY_HOLDER.ref, 2),
            (Part.STEP_UP_DOWN_CONVERTER.ref, 2),
            (Part.VOLTAGE_SENSOR.ref, 3),
            ("U3", 13),
        ),
        "battery sense": ((Part.VOLTAGE_SENSOR.ref, 2), ("U3", 10)),
    }
    group_nets: dict[str, str] = {}
    for name, pins in groups.items():
        connected = {net_of(ref, number) for ref, number in pins}
        if len(connected) != 1:
            raise ValueError(f"Incorrect {name} connections: {pins}")
        group_nets[name] = connected.pop()
    if len(set(group_nets.values())) != len(group_nets):
        raise ValueError(f"External connections are shorted: {group_nets}")
    if group_nets["USB input"] == net_of("F1", 2):
        raise ValueError("The EE04 input fuse is bypassed")
    if group_nets["battery sense"] == net_of("U3", 1):
        raise ValueError("NiMH sense is connected to the Li-ion BAT_ADC input")


def write_kicad_schematic(circuit_obj, out_dir: Path) -> Path:
    """Regenerate the external sheet while preserving vendor circuitry and settings."""
    out_dir = out_dir.resolve()
    out_dir.mkdir(parents=True, exist_ok=True)
    destination = out_dir / "hof_frame"
    _ensure_editor_closed(destination)
    vendor_destination = destination / VENDOR_DIRECTORY
    vendor_existed = vendor_destination.exists()
    vendor_source = vendor_destination if vendor_existed else BUNDLED_VENDOR
    missing = [name for name in VENDOR_SHEETS if not (vendor_source / name).is_file()]
    if missing:
        raise FileNotFoundError(
            f"Missing EE04 vendor sheets in {vendor_source}: {missing}"
        )
    existing_project = destination / "hof_frame.kicad_pro"
    outputs = (
        "hof_frame.kicad_sch",
        "hof_frame.json",
        "hof_frame.net",
        "hof_frame.pdf",
    )
    snapshot = _snapshot_files(
        {existing_project, *(destination / name for name in outputs)}
        | {vendor_source / name for name in VENDOR_SHEETS}
    )
    existing_uuid = _existing_root_uuid(destination)

    with tempfile.TemporaryDirectory(prefix="hof-frame-kicad-") as temp_dir:
        cwd = Path.cwd()
        os.chdir(temp_dir)
        try:
            circuit_obj.generate_kicad_project(
                "hof_frame",
                generate_pcb=False,
                force_regenerate=True,
                update_source_refs=False,
            )
        finally:
            os.chdir(cwd)

        staged = Path(temp_dir) / "hof_frame"
        shutil.copytree(vendor_source, staged / VENDOR_DIRECTORY)
        schematic = staged / "hof_frame.kicad_sch"
        generated_uuid = _schematic_uuid(schematic.read_text())
        root_uuid = existing_uuid or generated_uuid
        wire_schematic(schematic, root_uuid)
        staged_project = staged / "hof_frame.kicad_pro"
        if existing_project.exists():
            shutil.copy2(existing_project, staged_project)
        elif root_uuid != generated_uuid:
            staged_project.write_text(
                staged_project.read_text().replace(generated_uuid, root_uuid)
            )
        _update_vendor_instances(staged / VENDOR_DIRECTORY / VENDOR_ROOT, root_uuid)
        subprocess.run(
            [
                "kicad-cli",
                "sch",
                "export",
                "netlist",
                "--format",
                "kicadxml",
                "-o",
                "validation.xml",
                "hof_frame.kicad_sch",
            ],
            cwd=staged,
            check=True,
        )
        _validate_hierarchy(staged / "validation.xml")
        subprocess.run(
            [
                "kicad-cli",
                "sch",
                "export",
                "netlist",
                "--format",
                "kicadsexpr",
                "-o",
                "hof_frame.net",
                "hof_frame.kicad_sch",
            ],
            cwd=staged,
            check=True,
        )
        subprocess.run(
            [
                "kicad-cli",
                "sch",
                "export",
                "pdf",
                "--pages",
                "1",
                "--exclude-pdf-hierarchical-links",
                "--exclude-pdf-property-popups",
                "-o",
                "hof_frame.pdf",
                "hof_frame.kicad_sch",
            ],
            cwd=staged,
            check=True,
        )
        netlist = staged / "hof_frame.net"
        netlist.write_text(
            netlist.read_text().replace(
                str(staged / "hof_frame.kicad_sch"), "hof_frame.kicad_sch"
            )
        )

        _ensure_editor_closed(destination)
        _ensure_files_unchanged(snapshot)
        if vendor_destination.exists() != vendor_existed:
            raise RuntimeError(
                "The vendor directory changed during regeneration; no files were replaced"
            )
        destination.mkdir(parents=True, exist_ok=True)
        if not vendor_existed:
            shutil.copytree(staged / VENDOR_DIRECTORY, vendor_destination)
        else:
            staged_root = staged / VENDOR_DIRECTORY / VENDOR_ROOT
            destination_root = vendor_destination / VENDOR_ROOT
            if staged_root.read_bytes() != snapshot[destination_root]:
                shutil.copy2(staged_root, destination_root)
        for name in outputs:
            shutil.copy2(staged / name, destination / name)
        if not (destination / "hof_frame.kicad_pro").exists():
            shutil.copy2(
                staged / "hof_frame.kicad_pro", destination / "hof_frame.kicad_pro"
            )
    return destination
