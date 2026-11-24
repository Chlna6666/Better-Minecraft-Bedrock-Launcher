#!/usr/bin/env python3
"""Regression tests for runtime palette decoding.

Locks the network-NBT numeric rules and the raw-dump -> baked-blob equivalence.
"""
from __future__ import annotations

import json
import struct
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from parse_runtime_palette import VarintNBT, parse_states, read_varint, zigzag

REPO = Path(__file__).resolve().parents[3]


class NetworkNbtTests(unittest.TestCase):
    def test_byte_is_raw_not_zigzag(self):
        # TAG_Byte payload 0x01 is a raw signed byte = 1; zigzag(1) would be -1.
        reader = VarintNBT(b"\x01\xff", 0)
        self.assertEqual(reader.payload(1), 1)
        self.assertEqual(reader.pos, 1)
        self.assertEqual(VarintNBT(b"\xff", 0).payload(1), -1)  # raw -1

    def test_short_is_raw_le_not_zigzag(self):
        reader = VarintNBT(b"\x01\x00", 0)
        self.assertEqual(reader.payload(2), 1)
        self.assertEqual(reader.pos, 2)

    def test_int_is_zigzag(self):
        self.assertEqual(zigzag(2), 1)
        self.assertEqual(zigzag(1), -1)
        self.assertEqual(read_varint(b"\x02rest", 0), (2, 1))


class BakedBlobEquivalenceTests(unittest.TestCase):
    def test_every_dump_matches_baked_manifest_counts_and_versions(self):
        palettes = REPO / "crates/bedrock-world/data/block-catalog/palettes"
        runtime = REPO / "crates/bedrock-world/data/block-catalog/runtime-palettes/manifest.json"
        if not palettes.is_dir() or not runtime.is_file():
            self.skipTest("runtime palette data not present")
        manifest = json.loads(runtime.read_text(encoding="utf-8"))
        baked = {v["label"]: v for v in manifest["versions"]}
        for path in palettes.glob("*.nbt"):
            label = path.stem
            try:
                states, meta = parse_states(path)
            except (ValueError, KeyError):
                self.assertNotIn(label, baked, "unparseable dump must not be baked")
                continue
            if label not in baked:
                # pre-1.13 dumps without a version are intentionally raw-only
                continue
            self.assertEqual(baked[label]["state_count"], meta["count"], label)
            self.assertEqual(baked[label]["storage_version"], meta["versions"][0], label)
            self.assertEqual(baked[label]["source_sha256"], meta["sha256"], label)

    def test_no_negative_byte_values_in_baked_sources(self):
        # A negative Byte is the signature of the old zigzag-on-Byte bug.
        palettes = REPO / "crates/bedrock-world/data/block-catalog/palettes"
        if not palettes.is_dir():
            self.skipTest("runtime palette data not present")
        for path in palettes.glob("*.nbt"):
            try:
                states, _ = parse_states(path)
            except (ValueError, KeyError):
                continue
            for state in states:
                for prop in state["states"].values():
                    if prop["type"] == "Byte":
                        self.assertGreaterEqual(prop["value"], 0, path.name)


if __name__ == "__main__":
    unittest.main()
