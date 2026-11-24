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
            doc = json.loads(path.read_text(encoding="utf-8"))
            self.assertEqual(doc["method"], "live_frida_block_type_registry_dump", path.name)
            # A complete walk names exactly as many blocks as the map declares.
            self.assertEqual(doc["named_blocks"], doc["declared_map_size"], path.name)
            names = [block["name"] for block in doc["blocks"]]
            self.assertEqual(len(names), len(set(names)), path.name)
            for name in names:
                self.assertRegex(name, r"^[a-z0-9_.-]+:[a-z0-9_./-]+$")

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

    def test_no_machine_paths_or_addresses(self):
        for path in DUMP_DIR.glob("*.json"):
            text = path.read_text(encoding="utf-8")
            self.assertNotIn("0x1", text)  # no leaked heap addresses
            self.assertNotIn("C:" + chr(92), text)
            self.assertNotIn("/Users/", text)


if __name__ == "__main__":
    unittest.main()
