"""Tests for committed live runtime registry dumps."""
import json
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
DUMP_DIR = REPO / "crates/bedrock-world/data/block-catalog/runtime-registry"
PALETTES = REPO / "crates/bedrock-world/data/block-catalog/palettes"


class RuntimeRegistryEvidenceTests(unittest.TestCase):
    def setUp(self):
        if not DUMP_DIR.is_dir():
            self.skipTest("no runtime registry dumps")

    def test_dumps_are_complete_and_deduplicated(self):
        for path in DUMP_DIR.glob("*.json"):
            if path.name == "run-summary.json":
                continue
            doc = json.loads(path.read_text(encoding="utf-8"))
            self.assertEqual(doc["method"], "live_frida_block_type_registry_dump", path.name)
            # A complete walk names exactly as many blocks as the map declares.
            self.assertEqual(doc["named_blocks"], doc["declared_map_size"], path.name)
            names = [block["name"] for block in doc["blocks"]]
            self.assertEqual(len(names), len(set(names)), path.name)
            # Names are verbatim registry keys: old builds use short keys
            # without a namespace, 1.18+ use full minecraft: identifiers.
            for name in names:
                self.assertRegex(name, r"^[A-Za-z0-9_.:-]+$")
                self.assertNotIn("\x00", name)

    def test_all_pdb_versions_present(self):
        expected = {
            "1.9.0.15", "1.10.0.7", "1.11.4.2", "1.13.3.0", "1.14.60.5",
            "1.16.221.01", "1.18.33.02", "1.19.83.01", "1.20.80.05", "1.21.3.01",
        }
        present = {p.stem for p in DUMP_DIR.glob("*.json")}
        self.assertEqual(expected - present, set())

    def test_live_names_match_archived_palette(self):
        # The runtime dump must agree with the independent bds-mod-mapping
        # palette for the same build.
        dump = DUMP_DIR / "1.20.80.05.json"
        palette = PALETTES / "1.20.80.24_beta.nbt"
        if not dump.is_file() or not palette.is_file():
            self.skipTest("missing runtime dump or palette")
        doc = json.loads(dump.read_text(encoding="utf-8"))
        live = {block["name"] for block in doc["blocks"]}
        # Parse the palette with the same decoder used by the catalog tool.
        import sys
        sys.path.insert(0, str(REPO / "scripts/bedrock-block-catalog/pdb"))
        from parse_runtime_palette import parse_states
        states, _ = parse_states(palette)
        archived = {state["name"] for state in states}
        self.assertEqual(live, archived)

    def test_no_machine_paths_or_heap_addresses(self):
        for path in DUMP_DIR.glob("*.json"):
            if path.name == "run-summary.json":
                continue
            text = path.read_text(encoding="utf-8")
            self.assertNotIn("C:" + chr(92), text)
            self.assertNotIn("/Users/", text)
            # names-only dumps must not carry per-run heap pointers; the module
            # base and RVA are documented provenance and are allowed.
            doc = json.loads(text)
            for block in doc["blocks"]:
                self.assertEqual(set(block), {"name"}, path.name)


ENTITY_DIR = REPO / "crates/bedrock-world/data/block-catalog/entities"


class EntityIdentifierTests(unittest.TestCase):
    def setUp(self):
        if not ENTITY_DIR.is_dir():
            self.skipTest("no entity identifiers")

    def test_counts_and_format(self):
        expected = {
            "1.9.0.15": 93, "1.10.0.7": 97, "1.11.4.2": 97, "1.12.0.28": 97,
            "1.13.3.0": 98, "1.14.60.5": 99, "1.16.221.01": 105,
            "1.18.33.02": 113, "1.19.83.01": 116, "1.20.80.05": 121,
            "1.21.3.01": 122,
        }
        for version, count in expected.items():
            path = ENTITY_DIR / f"{version}.json"
            self.assertTrue(path.is_file(), version)
            doc = json.loads(path.read_text(encoding="utf-8"))
            self.assertEqual(doc["entity_count"], count, version)
            ids = doc["identifiers"]
            self.assertEqual(len(ids), count, version)
            self.assertEqual(ids, sorted(ids), version)
            self.assertEqual(len(ids), len(set(ids)), version)
            for ident in ids:
                self.assertRegex(ident, r"^[A-Za-z0-9_.-]+:[A-Za-z0-9_./-]+$")

    def test_monotonic_growth(self):
        import re

        def key(path):
            return tuple(int(part) for part in re.findall(r"\d+", path.stem))

        versions = sorted(ENTITY_DIR.glob("*.json"), key=key)
        counts = [json.loads(p.read_text(encoding="utf-8"))["entity_count"] for p in versions]
        self.assertEqual(counts, sorted(counts), "entity counts should grow with version")


if __name__ == "__main__":
    unittest.main()
