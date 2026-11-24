#!/usr/bin/env python3
"""Bake a compact, deduplicated query catalog from the runtime palette dumps.

This is the queryable companion to the raw `palettes/*.nbt` dumps. It keeps,
per version and network runtime ID: the block name, exact typed states and the
persisted NBT storage version. Strings are pooled and state values interned so
all 50 dumps compress into a small blob.

Output: `data/block-catalog/runtime-palettes/catalogs.bin.zst` + manifest.json.
Encoding BMCBRP01: magic + crc32 + zstd(body)
  body = pool_count, [u32 len + utf8]*, version_count,
         [version_id, state_count, (name_id, version, prop_count,
             (prop_id, kind, value)*)*]*
  kind 0=Byte 1=Short 2=Int 3=Long 8=String; Byte/Short/Int are i32, Long is i64, String is a pool id.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import struct
import sys
import zlib
from pathlib import Path

import zstandard

sys.path.insert(0, str(Path(__file__).resolve().parent / "pdb"))
from parse_runtime_palette import parse_states  # noqa: E402

MAGIC = b"BMCBRP01"
KIND_ID = {1: 0, 2: 1, 3: 2, 4: 3, 8: 8}


class Pool:
    def __init__(self) -> None:
        self.items: list[str] = []
        self.index: dict[str, int] = {}

    def id(self, value: str) -> int:
        found = self.index.get(value)
        if found is not None:
            return found
        found = len(self.items)
        self.index[value] = found
        self.items.append(value)
        return found


def zz(value: int) -> int:
    return (value << 1) ^ (value >> 63) if value >= 0 else ((~value) << 1) | 1


def put_varint(out: bytearray, value: int) -> None:
    value &= 0xFFFFFFFFFFFFFFFF
    while value >= 0x80:
        out.append((value & 0x7f) | 0x80)
        value >>= 7
    out.append(value)


def version_key(label: str) -> tuple:
    return tuple(int(p) for p in re.findall(r"\d+", label))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--palettes", type=Path, required=True)
    ap.add_argument("--out-dir", type=Path, required=True)
    args = ap.parse_args()

    files = sorted(args.palettes.glob("*.nbt"), key=lambda p: version_key(p.stem))
    pool = Pool()
    pool.id("")
    versions_blob = bytearray()
    version_manifest = []
    for path in files:
        try:
            states, meta = parse_states(path)
        except (ValueError, KeyError, IndexError) as error:
            # Pre-1.13 dumps are on-disk blockitem palettes in a different
            # format: they have no persisted NBT version and cannot enter the
            # query catalog. The raw file stays in palettes/ for viewing.
            print(f"skip {path.stem}: unparseable as versioned palette ({error})")
            continue
        # Keep only dumps with a single coherent storage version.
        if len(meta["versions"]) != 1:
            print(f"skip {path.stem}: no unique storage version ({meta['versions']})")
            continue
        label = path.stem
        versions_blob += struct.pack("<I", pool.id(label))
        versions_blob += struct.pack("<I", len(states))
        for state in states:
            versions_blob += struct.pack("<I", pool.id(state["name"]))
            versions_blob += struct.pack("<i", state["version"])
            props = state["states"]
            versions_blob += struct.pack("<I", len(props))
            for name, tag in sorted(props.items()):
                versions_blob += struct.pack("<I", pool.id(name))
                kind = tag["type"]
                versions_blob += bytes([KIND_ID[{"Byte": 1, "Short": 2, "Int": 3, "Long": 4, "String": 8}[kind]]])
                value = tag["value"]
                if kind == "String":
                    versions_blob += struct.pack("<I", pool.id(str(value)))
                elif kind == "Long":
                    versions_blob += struct.pack("<q", int(value))
                else:  # Byte / Short / Int: Rust reads a fixed i32
                    versions_blob += struct.pack("<i", int(value))
        version_manifest.append({
            "label": label,
            "state_count": len(states),
            "storage_version": meta["versions"][0],
            "source_sha256": meta["sha256"],
        })
        print(f"{label}: {len(states)} states")

    body = bytearray()
    body += struct.pack("<I", len(pool.items))
    for item in pool.items:
        raw = item.encode("utf-8")
        body += struct.pack("<I", len(raw)) + raw
    body += struct.pack("<I", len(version_manifest))
    body += versions_blob

    compressed = zstandard.ZstdCompressor(level=19).compress(bytes(body))
    crc = zlib.crc32(bytes(body)) & 0xffffffff
    blob = MAGIC + struct.pack("<I", crc) + compressed

    args.out_dir.mkdir(parents=True, exist_ok=True)
    (args.out_dir / "catalogs.bin.zst").write_bytes(blob)
    (args.out_dir / "manifest.json").write_text(json.dumps({
        "schema_version": 1,
        "source": "pmmp/bds-mod-mapping runtime palette dumps",
        "encoding": "BMCBRP01: crc32 + zstd; pool [len+utf8]; versions [version_id, state_count, (name_id, version, prop_count, (prop_id, kind, value)*)*]; kind 0=Byte 1=Short 2=Int (i32) 3=Long (i64) 8=String (pool id)",
        "evidence_boundary": "Runtime registry evidence with palette indices (NOT verified wire network runtime IDs; 1.26.51+ uses hashed IDs) and NBT storage version. No default-state field; not a legacy numeric ID table.",
        "version_count": len(version_manifest),
        "versions": version_manifest,
    }, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {len(version_manifest)} versions, {len(blob)} bytes compressed")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (ValueError, KeyError, struct.error) as error:
        print("failed to bake runtime palette catalog:", error, file=sys.stderr)
        raise SystemExit(1)
