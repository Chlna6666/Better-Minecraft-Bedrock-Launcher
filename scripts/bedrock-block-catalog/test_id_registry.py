"""PDB import invariants: exact namespace, byte types, evidence, no ID collisions."""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest

HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
from import_id_registry import import_data, parse_registry


def source():
    return {
        "method": "pdb_symbol_inspection",
        "platform": "windows-x64",
        "binary_sha256": "a" * 64,
        "pdb_sha256": "b" * 64,
        "tool": "pdb-extractor v0.1 with symbol and RVA logs",
    }


def registry(namespace, value=1):
    block = {"id": value, "name": "minecraft:stone",
             "evidence": "BlockPalette::mRegistry RVA 0x00012ea0"}
    if namespace == "legacy_numeric_block":
        block.update({"metadata": 0, "states": {
            "stone_type": {"type": "String", "value": "stone"},
        }})
    elif namespace == "network_runtime":
        block.update({"states": {
            "stone_type": {"type": "String", "value": "stone"},
        }})
    obj = {"namespace": namespace, "entries": [block]}
    if namespace == "network_runtime":
        obj["palette_fingerprint"] = "c" * 64
    return obj


class IdentityImportTests(unittest.TestCase):
    def test_duplicate_legacy_key_fails_even_identical(self):
        obj = registry("legacy_numeric_block")
        obj["entries"].append(dict(obj["entries"][0]))
        with self.assertRaisesRegex(ValueError, "duplicate"):
            parse_registry(obj, {"minecraft:stone"}, False)

    def test_same_number_in_different_namespaces_is_legal(self):
        a = parse_registry(registry("block_type_runtime"), {"minecraft:stone"}, False)
        b = parse_registry(registry("legacy_numeric_block"), {"minecraft:stone"}, False)
        self.assertNotEqual(a[2], b[2])

    def test_typed_state_matters(self):
        a = registry("legacy_numeric_block")
        b = registry("legacy_numeric_block")
        b["entries"][0]["states"]["stone_type"] = {"type": "Int", "value": 1}
        self.assertNotEqual(
            parse_registry(a, {"minecraft:stone"}, False)[2],
            parse_registry(b, {"minecraft:stone"}, False)[2],
        )

    def test_unverified_id_requires_flag(self):
        with self.assertRaisesRegex(ValueError, "cannot verify"):
            parse_registry(registry("block_type_runtime"), set(), False)
        result = parse_registry(registry("block_type_runtime"), set(), True)
        self.assertFalse(result[4])

    def test_network_runtime_requires_palette_fingerprint(self):
        item = registry("network_runtime")
        del item["palette_fingerprint"]
        with self.assertRaisesRegex(ValueError, "palette_fingerprint"):
            parse_registry(item, {"minecraft:stone"}, False)

    def test_import_is_idempotent_and_deduplicated(self):
        with tempfile.TemporaryDirectory() as temp:
            folder = Path(temp)
            payload = {
                "schema_version": 1, "game_version": "1.20.80.05",
                "source": source(),
                "registries": [registry("block_type_runtime")],
            }
            report = import_data(payload, folder, True, False)
            self.assertFalse(report["namespaces"]["block_type_runtime"]["verified_block_names"])
            import_data(payload, folder, True, False)
            index = json.loads((folder / "id-registry" / "index.json").read_text())
            self.assertEqual(1, len(index["versions"]))
            self.assertEqual(1, len(list((folder / "id-registry" / "objects").glob("*.bin"))))
            self.assertEqual(1, len(list((folder / "id-registry" / "evidence").glob("*.zlib"))))
            changed = json.loads(json.dumps(payload))
            changed["registries"][0]["entries"][0]["id"] = 7
            with self.assertRaisesRegex(ValueError, "different ID data"):
                import_data(changed, folder, True, False)


if __name__ == "__main__":
    unittest.main()
