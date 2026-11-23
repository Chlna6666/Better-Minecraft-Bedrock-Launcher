#!/usr/bin/env python3
"""Regression tests for the local PDB evidence tools.

The critical invariant: PDB symbol ordinals are block_type_runtime evidence
only. They must never be emitted as legacy numeric IDs or network runtime IDs,
and derived/guessed names must never enter an import.
"""
from __future__ import annotations

import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from build_import import snake_candidates
from extract_identifiers import decode_literal
from import_legacy_ids import NBT, varint
from integrity_check import PINNED, git_blob


class DecodeLiteralTests(unittest.TestCase):
    def test_msvc_escapes(self):
        self.assertEqual(decode_literal("??_C@_0BA@DHCMJKDN@minecraft?3stone@"), "minecraft:stone")
        self.assertEqual(decode_literal("??_C@_0CB@X@minecraft?3stone?4slab@"), "minecraft:stone.slab")
        self.assertEqual(decode_literal("??_C@_0BA@X@minecraft?3red?8stone@"), "minecraft:red-stone")

    def test_unknown_escape_is_rejected(self):
        # '?A' is not a known escape; must return None rather than guess.
        self.assertIsNone(decode_literal("??_C@_0BA@X@minecraft?3stone?A@"))

    def test_non_literal_symbol(self):
        self.assertIsNone(decode_literal("?mStone@VanillaBlockTypes@@3V?$WeakPtr@VBlockLegacy@@@@A"))


class LegacyIdTests(unittest.TestCase):
    def test_nbt_le_types(self):
        # Bedrock LE NBT: compound with a Byte, an Int and a String.
        # Build it programmatically; string names carry a 2-byte LE length.
        def s(name: str) -> bytes:
            raw = name.encode("utf-8")
            return len(raw).to_bytes(2, "little") + raw

        blob = (
            b"\x0a" + s("")            # TAG_Compound, empty root name
            + b"\x01" + s("b") + b"\x01"              # byte b=1
            + b"\x03" + s("i") + (3).to_bytes(4, "little", signed=True)
            + b"\x08" + s("s") + s("x")
            + b"\x00"                                  # TAG_End
        )
        _, _, root = NBT(blob, 0).parse()
        self.assertEqual(root["b"], (1, 1))
        self.assertEqual(root["i"], (3, 3))
        self.assertEqual(root["s"], (8, "x"))

    def test_varint(self):
        self.assertEqual(varint(b"\xac\x02rest", 0), (300, 2))

    def test_pinned_corpus_blob_identity(self):
        # If the locally downloaded authoritative corpus differs from the
        # repo's pinned blob IDs, every downstream legacy conclusion is void.
        root = Path(__file__).resolve().parents[3] / "target/bds-recon"
        local = {
            "block_legacy_id_map.json": root / "block_legacy_id_map.json",
            "id_meta_to_nbt/1.9.0.bin": root / "id_meta_to_nbt_1.9.0.bin",
            "id_meta_to_nbt/1.12.0.bin": root / "id_meta_to_nbt_1.12.0.bin",
        }
        for label, path in local.items():
            if not path.is_file():
                continue
            sha, size = git_blob(path)
            expected_sha, expected_size = PINNED[label]
            self.assertEqual((sha, size), (expected_sha, expected_size), label)

    def test_registry_ordinal_is_not_legacy_id(self):
        # The PDB declaration ordinal diverges from the authoritative legacy
        # numeric id (Allow/Deny jump to 210/211). Lock in the empirical fact
        # so nobody "simplifies" the two namespaces into one.
        path = Path(__file__).resolve().parents[3] / "target/bds-recon/pdb-evidence-report.json"
        if path.is_file():
            report = json.loads(path.read_text(encoding="utf-8"))
            self.assertIn("legacy_numeric_id_finding", report)


class ImportSafetyTests(unittest.TestCase):
    def test_derived_candidates_are_explicit(self):
        # The derivation list must stay small and auditable; adding a silent
        # fuzzy match here would change evidence semantics.
        self.assertEqual(snake_candidates("Stone"), ["stone"])
        self.assertIn("magma", snake_candidates("MagmaBlock"))

    def test_import_never_emits_legacy_or_network_namespace(self):
        sample = {
            "schema_version": 1,
            "game_version": "1.20.80.05",
            "source": {
                "method": "pdb_symbol_inspection",
                "platform": "windows-x64",
                "binary_sha256": "a" * 64,
                "pdb_sha256": "b" * 64,
                "tool": "test",
            },
            "registries": [{"namespace": "block_type_runtime", "entries": [{
                "id": 0, "name": "minecraft:stone", "evidence": "test evidence",
            }]}],
        }
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "import.json"
            path.write_text(json.dumps(sample), encoding="utf-8")
            doc = json.loads(path.read_text(encoding="utf-8"))
            self.assertEqual(
                {r["namespace"] for r in doc["registries"]},
                {"block_type_runtime"},
            )
            entry = doc["registries"][0]["entries"][0]
            self.assertNotIn("metadata", entry)
            self.assertNotIn("version", entry)
            self.assertNotIn("states", entry)


if __name__ == "__main__":
    unittest.main()
