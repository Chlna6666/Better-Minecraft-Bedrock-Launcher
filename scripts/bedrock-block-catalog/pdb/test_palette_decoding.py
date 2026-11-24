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

KIND_NAME = {0: "Byte", 1: "Short", 2: "Int", 3: "Long", 8: "String"}


def decode_baked_blob(path: Path) -> dict[str, list[tuple]]:
    """Decode the committed zstd blob into per-version state tuples.

    This mirrors the Rust decoder independently so a mismatch between the
    Python encoder and Rust decoder schema is caught by comparison with the
    raw NBT parse.
    """
    import zstandard

    raw = path.read_bytes()
    assert raw[:8] == b"BMCBRP01"
    body = zstandard.ZstdDecompressor().decompress(raw[12:])
    pos = 0

    def read_u32() -> int:
        nonlocal pos
        value = struct.unpack_from("<I", body, pos)[0]
        pos += 4
        return value

    def read_i32() -> int:
        nonlocal pos
        value = struct.unpack_from("<i", body, pos)[0]
        pos += 4
        return value

    pool_len = read_u32()
    pool = []
    for _ in range(pool_len):
        length = read_u32()
        pool.append(body[pos:pos + length].decode("utf-8"))
        pos += length
    version_count = read_u32()
    result = {}
    for _ in range(version_count):
        label = pool[read_u32()]
        state_count = read_u32()
        states = []
        for index in range(state_count):
            name = pool[read_u32()]
            version = read_i32()
            property_count = read_u32()
            properties = {}
            for _ in range(property_count):
                prop = pool[read_u32()]
                kind = body[pos]
                pos += 1
                if kind == 8:
                    properties[prop] = ("String", pool[read_u32()])
                elif kind == 3:  # NBT Long is i64
                    value = struct.unpack_from("<q", body, pos)[0]
                    pos += 8
                    properties[prop] = ("Long", value)
                else:
                    properties[prop] = (KIND_NAME[kind], read_i32())
            states.append((index, name, version, properties))
        result[label] = states
    assert pos == len(body), "blob has trailing bytes"
    return result


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

    def test_baked_blob_long_is_eight_bytes(self):
        # Encoder writes Long as <q>; decoder must consume 8 bytes, not 4.
        blob = bytearray()
        blob += struct.pack("<I", 2)  # pool: "", "lit"
        blob += struct.pack("<I", 0) + b""
        blob += struct.pack("<I", 3) + b"lit"
        blob += struct.pack("<I", 1)  # one version
        blob += struct.pack("<I", 1)  # version id -> "lit"
        blob += struct.pack("<I", 1)  # one state
        blob += struct.pack("<I", 1) + struct.pack("<i", 123)  # name "lit", version
        blob += struct.pack("<I", 1)  # one property
        blob += struct.pack("<I", 1) + bytes([3]) + struct.pack("<q", -5)  # Long -5
        path = Path(__file__).parent / "_tmp_long_test.bin.zst"
        import zstandard
        path.write_bytes(b"BMCBRP01" + struct.pack("<I", 0) + zstandard.ZstdCompressor().compress(bytes(blob)))
        try:
            decoded = decode_baked_blob(path)
            states = decoded["lit"]
            self.assertEqual(states[0][3], {"lit": ("Long", -5)})
        finally:
            path.unlink(missing_ok=True)

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

    def test_baked_blob_matches_raw_nbt_for_every_state(self):
        # Full roundtrip: name, typed properties, NBT version and palette
        # index must agree for EVERY state of EVERY baked version. This
        # detects encoder/decoder schema drift that count/hash checks miss.
        palettes = REPO / "crates/bedrock-world/data/block-catalog/palettes"
        blob_path = REPO / "crates/bedrock-world/data/block-catalog/runtime-palettes/catalogs.bin.zst"
        if not palettes.is_dir() or not blob_path.is_file():
            self.skipTest("runtime palette data not present")
        try:
            import zstandard  # noqa: F401
        except ImportError:
            self.skipTest("zstandard not installed")
        baked = decode_baked_blob(blob_path)
        for label, baked_states in baked.items():
            raw_states, _ = parse_states(palettes / f"{label}.nbt")
            self.assertEqual(len(raw_states), len(baked_states), label)
            for index, (raw, baked_state) in enumerate(zip(raw_states, baked_states)):
                expected = (
                    index,
                    raw["name"],
                    raw["version"],
                    {key: (value["type"], value["value"])
                     for key, value in raw["states"].items()},
                )
                self.assertEqual(baked_state, expected, f"{label} state {index}")


if __name__ == "__main__":
    unittest.main()
