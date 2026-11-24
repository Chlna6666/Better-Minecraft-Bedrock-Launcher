"""PDB import invariants: exact namespace, byte types, evidence, no ID collisions."""
import importlib.util
import json
import re
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
        block.update({"metadata": 0, "version": 18168865, "states": {
            "stone_type": {"type": "String", "value": "stone"},
        }})
    elif namespace == "network_runtime":
        block.update({"version": 18168865, "states": {
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

    def test_persisted_storage_version_is_required_and_distinct(self):
        first = registry("legacy_numeric_block")
        second = registry("legacy_numeric_block")
        second["entries"][0]["version"] = 18168864
        self.assertNotEqual(
            parse_registry(first, {"minecraft:stone"}, False)[2],
            parse_registry(second, {"minecraft:stone"}, False)[2],
        )
        del first["entries"][0]["version"]
        with self.assertRaisesRegex(ValueError, "version must be explicit"):
            parse_registry(first, {"minecraft:stone"}, False)

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

    def test_network_negative_signed_id_normalizes_to_u32(self):
        negative = registry("network_runtime", -1)
        unsigned = registry("network_runtime", 0xffffffff)
        self.assertEqual(
            parse_registry(negative, {"minecraft:stone"}, False)[2],
            parse_registry(unsigned, {"minecraft:stone"}, False)[2],
        )

    def test_block_type_one_name_cannot_have_two_ids(self):
        table = registry("block_type_runtime")
        entry = dict(table["entries"][0])
        entry["id"] = 2
        table["entries"].append(entry)
        with self.assertRaisesRegex(ValueError, "multiple runtime type IDs"):
            parse_registry(table, {"minecraft:stone"}, False)

    def test_legacy_names_keep_historical_case(self):
        obj = registry("legacy_numeric_block")
        obj["entries"][0]["name"] = "minecraft:concretePowder"
        namespace, _, binary, _, _ = parse_registry(obj, {"minecraft:concretePowder"}, False)
        self.assertEqual(namespace, "legacy_numeric_block")
        self.assertIn(b"concretePowder", binary)

    def test_modern_namespace_rejects_uppercase(self):
        obj = registry("block_type_runtime")
        obj["entries"][0]["name"] = "minecraft:Stone"
        with self.assertRaisesRegex(ValueError, "lowercase"):
            parse_registry(obj, {"minecraft:Stone"}, False)

    def test_pdb_symbol_order_requires_explicit_ordinal(self):
        obj = {"namespace": "pdb_symbol_order", "entries": [{
            "id": 3, "name": "minecraft:stone", "evidence": "symbol at RVA 0x12ab",
        }]}
        with self.assertRaisesRegex(ValueError, "symbol_ordinal"):
            parse_registry(obj, {"minecraft:stone"}, False)
        obj["entries"][0]["symbol_ordinal"] = 3
        namespace, _, _, _, _ = parse_registry(obj, {"minecraft:stone"}, False)
        self.assertEqual(namespace, "pdb_symbol_order")

    def test_same_version_different_provenance_coexists(self):
        from import_id_registry import provenance_scope
        src_a = {"platform": "windows-x64", "binary_sha256": "a" * 64}
        src_b = {"platform": "linux-x64", "binary_sha256": "b" * 64}
        scope_a = provenance_scope("1.12.0.28", "legacy_numeric_block", src_a, None)
        scope_b = provenance_scope("1.12.0.28", "legacy_numeric_block", src_b, None)
        scope_c = provenance_scope("1.12.0.28", "legacy_numeric_block", src_a, "f" * 64)
        self.assertEqual(len({scope_a, scope_b, scope_c}), 3)
        self.assertIn("1.12.0.28", scope_a)
        self.assertIn("legacy_numeric_block", scope_a)

    def test_dataset_source_is_not_claimed_as_windows_binary(self):
        from import_id_registry import validate_source
        dataset_source = {
            "method": "authoritative_dataset", "platform": "dataset",
            "binary_sha256": "a" * 64,
            "dataset": {"name": "PocketMine schema", "version": "1.12.0",
                        "file_sha256": "a" * 64, "upstream": "https://example.test/dataset"},
            "tool": "importer v1",
        }
        result = validate_source(dataset_source)
        self.assertEqual(result["method"], "authoritative_dataset")
        self.assertEqual(result["platform"], "dataset")
        self.assertEqual(result["dataset"]["file_sha256"], "a" * 64)
        bad = json.loads(json.dumps(dataset_source))
        bad["platform"] = "windows-x64"
        with self.assertRaisesRegex(ValueError, "platform=dataset"):
            validate_source(bad)

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
            scope = next(iter(report["namespaces"]))
            self.assertIn("block_type_runtime", scope)
            self.assertFalse(report["namespaces"][scope]["verified_block_names"])
            import_data(payload, folder, True, False)
            index = json.loads((folder / "id-registry" / "index.json").read_text())
            self.assertEqual(1, len(index["versions"]))
            self.assertEqual(1, len(list((folder / "id-registry" / "objects").glob("*.bin"))))
            self.assertEqual(1, len(list((folder / "id-registry" / "evidence").glob("*.zlib"))))
            changed = json.loads(json.dumps(payload))
            changed["registries"][0]["entries"][0]["id"] = 7
            with self.assertRaisesRegex(ValueError, "different ID data"):
                import_data(changed, folder, True, False)


class CommittedStagedRegistryTests(unittest.TestCase):
    """The committed id-registry is auditable evidence, never authoritative."""

    def setUp(self):
        self.root = HERE.parents[1] / "crates/bedrock-world/data/block-catalog/id-registry"
        self.index_path = self.root / "index.json"
        if not self.index_path.is_file():
            self.skipTest("no committed id-registry")

    def test_all_staged_records_are_unverified_and_scoped(self):
        index = json.loads(self.index_path.read_text(encoding="utf-8"))
        self.assertEqual(index["format_version"], 1)
        self.assertGreater(len(index["versions"]), 0)
        for version, entry in index["versions"].items():
            self.assertRegex(version, r"^\d+(?:\.\d+){3}$")
            for scope, record in entry["registries"].items():
                self.assertFalse(record["verified_block_names"], version)
                self.assertEqual(record["status"], "unverified_names", version)
                self.assertEqual(scope, record["provenance_scope"], version)
                self.assertIn(version, scope)
                self.assertRegex(record["object_sha256"], r"^[0-9a-f]{64}$")
                self.assertTrue((self.root / "objects" / (record["object_sha256"] + ".bin")).is_file())
                self.assertTrue((self.root / "evidence" / (record["evidence_sha256"] + ".json.zlib")).is_file())

    def test_staged_evidence_is_not_loaded_by_destructive_conversion(self):
        # No production Rust code may read the staged evidence directory.
        source_root = HERE.parents[1] / "crates/bedrock-world/src"
        hits = []
        for path in source_root.rglob("*.rs"):
            text = path.read_text(encoding="utf-8")
            if "id-registry" in text or "id_registry" in text:
                hits.append(str(path.relative_to(source_root)))
        self.assertEqual(hits, [], "staged id-registry must not be read by production code")

    def test_no_machine_paths_in_staged_evidence(self):
        import zlib
        for path in (self.root / "evidence").glob("*.zlib"):
            raw = zlib.decompress(path.read_bytes()).decode("utf-8")
            self.assertNotIn("/Users/", raw)
            self.assertNotIn("/home/", raw)
            self.assertIsNone(re.search(r"[A-Za-z]:" + re.escape(chr(92)), raw))


if __name__ == "__main__":
    unittest.main()
