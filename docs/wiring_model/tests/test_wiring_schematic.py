"""Integration checks for the generated EE04 KiCad hierarchy."""

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET
from collections.abc import Callable
from pathlib import Path
from typing import cast
from unittest.mock import patch

from circuit_synth import Circuit
from wiring_model.circuit import hof_frame_wiring
from wiring_model.kicad import write_kicad_schematic

ROOT = Path(__file__).resolve().parents[3]
VENDOR_DIR = "seeed-ee04-v1.2"
OUTPUTS = ("hof_frame.kicad_sch", "hof_frame.json", "hof_frame.net", "hof_frame.pdf")
VENDOR_ROOT = "XIAO ePaper Display Board EE04_V1.2.kicad_sch"
VENDOR_PAGES = {
    "01 Descriptions.kicad_sch": "3",
    "02 Block Diagram.kicad_sch": "4",
    "Power.kicad_sch": "5",
    "XIAO.kicad_sch": "6",
    "E-ink.kicad_sch": "7",
}


@unittest.skipUnless(shutil.which("kicad-cli"), "KiCad CLI is required")
class WiringSchematicTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.temp = tempfile.TemporaryDirectory(prefix="hof-wiring-tests-")
        cls.addClassCleanup(cls.temp.cleanup)
        cls.output_dir = Path(cls.temp.name)
        cls.generate(cls.output_dir)
        cls.project = cls.output_dir / "hof_frame"

        xml = cls.output_dir / "hierarchy.xml"
        subprocess.run(
            [
                "kicad-cli",
                "sch",
                "export",
                "netlist",
                "--format",
                "kicadxml",
                "-o",
                str(xml),
                str(cls.project / "hof_frame.kicad_sch"),
            ],
            check=True,
            capture_output=True,
            text=True,
        )
        cls.hierarchy = ET.parse(xml).getroot()

    @staticmethod
    def generate(
        out_dir: Path, check: bool = True, symbol_dir: Path | None = None
    ) -> subprocess.CompletedProcess[str]:
        env = os.environ.copy()
        env["KICAD_SYMBOL_DIR"] = str(
            (
                symbol_dir
                or Path(env.get("KICAD_SYMBOL_DIR", ROOT / ".kicad" / "symbols"))
            ).resolve()
        )
        result = subprocess.run(
            [sys.executable, "-m", "wiring_model", "--kicad", str(out_dir)],
            cwd=ROOT,
            env=env,
            capture_output=True,
            text=True,
        )
        if check and result.returncode:
            raise AssertionError(f"KiCad generation failed:\n{result.stderr[-8000:]}")
        return result

    def copy_project(self) -> tuple[Path, Path]:
        temp = tempfile.TemporaryDirectory(prefix="hof-wiring-copy-")
        self.addCleanup(temp.cleanup)
        out_dir = Path(temp.name)
        project = out_dir / "hof_frame"
        shutil.copytree(self.project, project)
        return out_dir, project

    def assert_sheet_instances(
        self, project: Path, expected_root: str | None = None
    ) -> str:
        schematic = (project / "hof_frame.kicad_sch").read_text()
        root = re.search(r'(?m)^\t\(uuid "([^"]+)"\)', schematic)
        self.assertIsNotNone(root)
        root_uuid = root.group(1)
        if expected_root is not None:
            self.assertEqual(expected_root, root_uuid)

        parent = re.search(r"(?ms)^\t\(sheet\n(?P<sheet>.*?)^\t\)", schematic)
        self.assertIsNotNone(parent)
        parent_uuid = re.search(r'\(uuid "([^"]+)"\)', parent.group("sheet"))
        self.assertIsNotNone(parent_uuid)
        parent_path = f"/{root_uuid}/{parent_uuid.group(1)}"

        source = (project / VENDOR_DIR / VENDOR_ROOT).read_text()
        sheets = re.findall(r"(?ms)^\t\(sheet\n(.*?)^\t\)", source)
        self.assertEqual(len(VENDOR_PAGES), len(sheets))
        pages = {}
        for sheet in sheets:
            filename = re.search(r'\(property "Sheetfile" "([^"]+)"', sheet)
            instance = re.search(
                r'\(project "hof_frame"\s*\(path "([^"]+)"\s*'
                r'\(page "([^"]+)"\)',
                sheet,
            )
            self.assertIsNotNone(filename)
            self.assertIsNotNone(instance, filename.group(1))
            self.assertEqual(parent_path, instance.group(1), filename.group(1))
            pages[filename.group(1)] = instance.group(2)
        self.assertEqual(VENDOR_PAGES, pages)
        return root_uuid

    def test_vendor_sheet_instances_match_generated_root(self) -> None:
        self.assert_sheet_instances(self.project)

    def test_missing_vendor_instances_preserve_crlf_and_other_content(self) -> None:
        out_dir, project = self.copy_project()
        vendor_root = project / VENDOR_DIR / VENDOR_ROOT
        pattern = (
            r'(?m)^[ \t]*\(project "hof_frame"\s*'
            r'\(path "[^"]+"\s*\(page "[^"]+"\)\s*\)\s*\)[ \t]*\n'
        )
        source, count = re.subn(pattern, "", vendor_root.read_text())
        self.assertEqual(len(VENDOR_PAGES), count)
        vendor_root.write_bytes(source.replace("\n", "\r\n").encode())

        self.generate(out_dir)
        self.assert_sheet_instances(project)
        updated = vendor_root.read_bytes()
        self.assertNotIn(b"\n", updated.replace(b"\r\n", b""))
        without_instances, count = re.subn(pattern, "", vendor_root.read_text())
        self.assertEqual(len(VENDOR_PAGES), count)
        self.assertEqual(source, without_instances)

    def test_external_and_vendor_nets(self) -> None:
        nodes = {
            (node.attrib["ref"], node.attrib["pin"]): net.attrib["code"]
            for net in self.hierarchy.findall("./nets/net")
            for node in net.findall("node")
        }

        def net(ref: str, pin: int) -> str:
            return nodes[(ref, str(pin))]

        self.assertEqual(net("J101", 1), net("U101", 3))
        self.assertEqual(net("J101", 1), net("U103", 1))
        self.assertEqual(net("U101", 1), net("U3", 14))
        self.assertEqual(net("U101", 1), net("F1", 1))
        self.assertNotEqual(net("U101", 1), net("F1", 2))
        for ref, pin in (("U101", 2), ("U103", 3), ("U3", 13)):
            self.assertEqual(net("J101", 2), net(ref, pin))
        self.assertEqual(net("U103", 2), net("U3", 10))
        self.assertNotEqual(net("U103", 2), net("U3", 1))
        self.assertEqual(
            4,
            len({net("J101", 1), net("J101", 2), net("U101", 1), net("U103", 2)}),
        )

        refs = [
            comp.attrib["ref"] for comp in self.hierarchy.findall("./components/comp")
        ]
        self.assertEqual(len(refs), len(set(refs)))
        self.assertGreater(len(refs), 170)
        self.assertNotIn("J102", refs)
        self.assertNotIn("J103", refs)
        self.assertEqual(7, len(self.hierarchy.findall("./design/sheet")))

        model = json.loads((self.project / "hof_frame.json").read_text())
        self.assertIn("J102", model["components"])
        self.assertIn("J103", model["components"])
        pdf = (self.project / "hof_frame.pdf").read_bytes()
        self.assertTrue(pdf.startswith(b"%PDF-"))
        self.assertEqual(1, len(re.findall(rb"/Type\s*/Page(?!s)", pdf)))
        exported = (self.project / "hof_frame.net").read_text()
        self.assertIn('(source "hof_frame.kicad_sch")', exported)
        self.assertNotIn(self.temp.name, exported)
        vendor = self.project / VENDOR_DIR
        self.assertIn("CC BY-SA 4.0", (vendor / "NOTICE.txt").read_text())
        self.assertIn(
            "CC BY-SA 4.0",
            (vendor / "XIAO ePaper Display Board EE04_V1.2.kicad_sch").read_text(),
        )

    def test_regeneration_preserves_project_and_vendor_edits(self) -> None:
        out_dir, project = self.copy_project()
        old_root = self.assert_sheet_instances(project)
        settings = project / "hof_frame.kicad_pro"
        data = json.loads(settings.read_text())
        data["wiring_test_marker"] = "preserve existing project settings"
        project_root = "9361cd5c-cd50-42fc-b140-69827a04bbf7"
        data["schematic"]["top_level_sheets"] = [
            {
                "filename": "hof_frame.kicad_sch",
                "name": "hof_frame",
                "uuid": project_root,
            }
        ]
        settings.write_text(json.dumps(data, indent=2) + "\n")
        original_settings = settings.read_bytes()

        vendor = project / VENDOR_DIR / "XIAO.kicad_sch"
        vendor.write_bytes(vendor.read_bytes() + b"\n")
        original_vendor = vendor.read_bytes()
        vendor_root = project / VENDOR_DIR / VENDOR_ROOT
        original_title = b"Seeed Studio EE04 V1.2 - CC BY-SA 4.0"
        edited_title = original_title + b" (user edit)"
        vendor_root.write_bytes(
            vendor_root.read_bytes()
            .replace(original_title, edited_title)
            .replace(b"\n", b"\r\n")
        )
        original_vendor_root = vendor_root.read_bytes()

        self.generate(out_dir)
        self.assertEqual(original_settings, settings.read_bytes())
        self.assertEqual(original_vendor, vendor.read_bytes())
        self.assertEqual(
            original_vendor_root.replace(old_root.encode(), project_root.encode()),
            vendor_root.read_bytes(),
        )
        self.assert_sheet_instances(project, project_root)
        updated_vendor_root = vendor_root.read_bytes()

        self.generate(out_dir)
        self.assertEqual(original_settings, settings.read_bytes())
        self.assertEqual(original_vendor, vendor.read_bytes())
        self.assertEqual(updated_vendor_root, vendor_root.read_bytes())
        self.assert_sheet_instances(project, project_root)

    def test_existing_schematic_identity_without_project_metadata(self) -> None:
        out_dir, project = self.copy_project()
        root = self.assert_sheet_instances(project)
        settings = project / "hof_frame.kicad_pro"
        original_settings = settings.read_bytes()

        self.generate(out_dir)
        self.assertEqual(original_settings, settings.read_bytes())
        self.assert_sheet_instances(project, root)

        settings.unlink()
        self.generate(out_dir)
        self.assertEqual(root, json.loads(settings.read_text())["sheets"][0][0])
        self.assert_sheet_instances(project, root)

    def test_open_schematic_is_not_overwritten(self) -> None:
        out_dir, project = self.copy_project()
        outputs = {name: (project / name).read_bytes() for name in OUTPUTS}
        vendor_root = project / VENDOR_DIR / VENDOR_ROOT
        original_vendor = vendor_root.read_bytes()
        (project / "~hof_frame.kicad_sch.lck").write_text('{"hostname": "test"}')

        result = self.generate(out_dir, check=False)
        self.assertNotEqual(0, result.returncode)
        self.assertIn("Close the KiCad schematic editor", result.stderr)
        self.assertEqual(original_vendor, vendor_root.read_bytes())
        for name, contents in outputs.items():
            self.assertEqual(contents, (project / name).read_bytes())

    def test_concurrent_vendor_edit_is_not_overwritten(self) -> None:
        out_dir, project = self.copy_project()
        outputs = {name: (project / name).read_bytes() for name in OUTPUTS}
        vendor = project / VENDOR_DIR / VENDOR_ROOT
        edited = vendor.read_bytes() + b"\n"
        original_run = subprocess.run
        modified = False

        def edit_during_export(*args, **kwargs):
            nonlocal modified
            result = original_run(*args, **kwargs)
            if not modified and args[0][0] == "kicad-cli":
                vendor.write_bytes(edited)
                modified = True
            return result

        env = {
            "KICAD_SYMBOL_DIR": str(
                Path(
                    os.environ.get("KICAD_SYMBOL_DIR", ROOT / ".kicad" / "symbols")
                ).resolve()
            )
        }
        build_circuit = cast(Callable[[], Circuit], hof_frame_wiring)
        with (
            patch.dict(os.environ, env),
            patch("wiring_model.kicad.subprocess.run", side_effect=edit_during_export),
        ):
            with self.assertRaisesRegex(RuntimeError, "changed during regeneration"):
                write_kicad_schematic(build_circuit(), out_dir)

        self.assertTrue(modified)
        self.assertEqual(edited, vendor.read_bytes())
        for name, contents in outputs.items():
            self.assertEqual(contents, (project / name).read_bytes())

    def test_disconnected_vendor_port_does_not_replace_outputs(self) -> None:
        out_dir, project = self.copy_project()
        outputs = {name: (project / name).read_bytes() for name in OUTPUTS}
        vendor = project / VENDOR_DIR / "XIAO.kicad_sch"
        original = vendor.read_text()
        broken = original.replace("(at 196.85 58.42 90)", "(at 196.85 48.26 90)", 1)
        self.assertNotEqual(original, broken)
        vendor.write_text(broken)

        result = self.generate(out_dir, check=False)
        self.assertNotEqual(0, result.returncode)
        self.assertIn("Incorrect USB input connections", result.stderr)
        self.assertEqual(broken, vendor.read_text())
        for name, contents in outputs.items():
            self.assertEqual(contents, (project / name).read_bytes())

    def test_incomplete_vendor_hierarchy_fails_explicitly(self) -> None:
        out_dir, project = self.copy_project()
        original_netlist = (project / "hof_frame.net").read_bytes()
        (project / VENDOR_DIR / "Power.kicad_sch").unlink()
        result = self.generate(out_dir, check=False)
        self.assertNotEqual(0, result.returncode)
        self.assertIn("Missing EE04 vendor sheets", result.stderr)
        self.assertEqual(original_netlist, (project / "hof_frame.net").read_bytes())

    def test_unexpected_vendor_hierarchy_does_not_replace_outputs(self) -> None:
        out_dir, project = self.copy_project()
        outputs = {name: (project / name).read_bytes() for name in OUTPUTS}
        vendor_root = project / VENDOR_DIR / VENDOR_ROOT
        edited = vendor_root.read_text().replace(
            '"Sheetfile" "Power.kicad_sch"', '"Sheetfile" "Other.kicad_sch"', 1
        )
        vendor_root.write_text(edited)

        result = self.generate(out_dir, check=False)
        self.assertNotEqual(0, result.returncode)
        self.assertIn("Unexpected EE04 vendor sheet hierarchy", result.stderr)
        self.assertEqual(edited, vendor_root.read_text())
        for name, contents in outputs.items():
            self.assertEqual(contents, (project / name).read_bytes())

    def test_missing_power_symbol_library_fails_explicitly(self) -> None:
        temp = tempfile.TemporaryDirectory(prefix="hof-wiring-symbols-")
        self.addCleanup(temp.cleanup)
        symbol_dir = Path(temp.name) / "symbols"
        symbol_dir.mkdir()
        source = Path(
            os.environ.get("KICAD_SYMBOL_DIR", ROOT / ".kicad" / "symbols")
        ).resolve()
        shutil.copy2(source / "Connector_Generic.kicad_sym", symbol_dir)
        result = self.generate(Path(temp.name), check=False, symbol_dir=symbol_dir)
        self.assertNotEqual(0, result.returncode)
        self.assertIn("Missing required KiCad symbol libraries", result.stderr)
        self.assertIn("power.kicad_sym", result.stderr)
        self.assertFalse((Path(temp.name) / "hof_frame").exists())


if __name__ == "__main__":
    unittest.main()
