#!/usr/bin/env python3
"""Convert PocketMine's authoritative legacy-ID tables into a BMCBL
id-registry import (namespace legacy_numeric_block).

Source: pmmp/BedrockBlockUpgradeSchema
  * block_legacy_id_map.json   -> string id -> numeric id
  * id_meta_to_nbt/<ver>.bin   -> (id, metadata) -> BlockState NBT

The numeric id here is the AUTHORITATIVE historical numeric block id, not a
PDB registry ordinal. Every emitted entry keeps the exact NBT `version` and
typed states from the dataset; nothing is inferred or derived by name.

Bedrock NBT is little-endian; this parses only the subset the dataset uses.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import struct
import sys
from pathlib import Path

# Historical legacy identifiers preserve their exact case (concretePowder,
# tripWire, ...); the namespace itself is always lowercase.
NAME_RE = re.compile(r"[a-z0-9_.-]+:[a-zA-Z0-9_./-]+")


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def varint(data: bytes, pos: int) -> tuple[int, int]:
    result = 0
    shift = 0
    while True:
        if pos >= len(data) or shift > 35:
            raise ValueError("bad varint")
        b = data[pos]
        pos += 1
        result |= (b & 0x7F) << shift
        if not (b & 0x80):
            return result, pos
        shift += 7


class NBT:
    """Minimal Bedrock LE NBT reader returning Python values plus tag type."""

    def __init__(self, data: bytes, pos: int):
        self.data = data
        self.pos = pos

    def _u8(self) -> int:
        v = self.data[self.pos]
        self.pos += 1
        return v

    def _u16(self) -> int:
        v = struct.unpack_from("<H", self.data, self.pos)[0]
        self.pos += 2
        return v

    def _i32(self) -> int:
        v = struct.unpack_from("<i", self.data, self.pos)[0]
        self.pos += 4
        return v

    def _i64(self) -> int:
        v = struct.unpack_from("<q", self.data, self.pos)[0]
        self.pos += 8
        return v

    def _string(self) -> str:
        n = self._u16()
        raw = self.data[self.pos:self.pos + n]
        if len(raw) != n:
            raise ValueError("truncated NBT string")
        self.pos += n
        return raw.decode("utf-8")

    def _payload(self, tag: int):
        if tag == 1:   # Byte
            v = struct.unpack_from("<b", self.data, self.pos)[0]
            self.pos += 1
            return v
        if tag == 2:   # Short
            v = struct.unpack_from("<h", self.data, self.pos)[0]
            self.pos += 2
            return v
        if tag == 3:   # Int
            return self._i32()
        if tag == 4:   # Long
            return self._i64()
        if tag == 5:   # Float
            v = struct.unpack_from("<f", self.data, self.pos)[0]
            self.pos += 4
            return v
        if tag == 6:   # Double
            v = struct.unpack_from("<d", self.data, self.pos)[0]
            self.pos += 8
            return v
        if tag == 7:   # Byte_Array
            n = self._i32()
            raw = self.data[self.pos:self.pos + n]
            self.pos += n
            return list(raw)
        if tag == 8:   # String
            return self._string()
        if tag == 9:   # List
            item_type = self._u8()
            n = self._i32()
            return [self._payload(item_type) for _ in range(n)]
        if tag == 10:  # Compound
            out: dict[str, tuple[int, object]] = {}
            while True:
                child = self._u8()
                if child == 0:
                    return out
                name = self._string()
                out[name] = (child, self._payload(child))
        if tag == 11:  # Int_Array
            n = self._i32()
            vals = [self._i32() for _ in range(n)]
            return vals
        raise ValueError(f"unsupported NBT tag {tag}")

    def parse(self):
        tag = self._u8()
        name = self._string()
        return tag, name, self._payload(tag)


def typed_state(tag: int, value) -> dict | None:
    """Map an NBT tag to the importer's strict {type,value} form.
    BlockState properties are only Byte/Int/String per the maintenance guide;
    other tags are reported as unsupported and cause the entry to be skipped.
    """
    if tag == 1:
        return {"type": "Byte", "value": int(value)}
    if tag == 3:
        return {"type": "Int", "value": int(value)}
    if tag == 8:
        return {"type": "String", "value": str(value)}
    return None


def convert(table: bytes, legacy: dict[str, int], version: str) -> tuple[list[dict], dict]:
    pos = 0
    count, pos = varint(table, pos)
    entries: list[dict] = []
    stats = {"blocks": count, "mappings": 0, "skipped_unsupported_nbt": 0,
             "skipped_unknown_name": 0, "skipped_nonconforming_name": 0}
    for _ in range(count):
        name_len, pos = varint(table, pos)
        name = table[pos:pos + name_len].decode("utf-8")
        pos += name_len
        meta_count, pos = varint(table, pos)
        numeric_id = legacy.get(name)
        if numeric_id is None:
            stats["skipped_unknown_name"] += 1
        for _ in range(meta_count):
            metadata, pos = varint(table, pos)
            nbt = NBT(table, pos)
            _, _, root = nbt.parse()
            pos = nbt.pos
            if numeric_id is None:
                continue
            if not isinstance(root, dict):
                stats["skipped_unsupported_nbt"] += 1
                continue
            tag_version = root.get("version")
            states_raw = root.get("states")
            # Early datasets (1.9.0) carry no NBT version field. Keep them with
            # an explicit null version rather than dropping or inventing one.
            if tag_version is not None and tag_version[0] != 3:
                stats["skipped_unsupported_nbt"] += 1
                continue
            versionless = tag_version is None
            if versionless:
                stats["versionless"] = stats.get("versionless", 0) + 1
            states: dict[str, dict] = {}
            ok = True
            if states_raw is not None:
                if states_raw[0] != 10:
                    ok = False
                else:
                    for key, (stag, sval) in states_raw[1].items():
                        typed = typed_state(stag, sval)
                        if typed is None:
                            ok = False
                            break
                        states[key] = typed
            if not ok:
                stats["skipped_unsupported_nbt"] += 1
                continue
            # Historical identifiers keep their EXACT case (concretePowder,
            # tripWire, seaLantern, ...). Never normalise or drop them: the
            # importer validates legacy names with a case-preserving rule.
            if not NAME_RE.fullmatch(name):
                stats["historical_case_names"] = stats.get("historical_case_names", 0) + 1
            entries.append({
                "id": numeric_id,
                "metadata": metadata,
                "name": name,
                "version": None if versionless else tag_version[1],
                "states": states,
                "evidence": (
                    f"pmmp/BedrockBlockUpgradeSchema id_meta_to_nbt/{version}.bin "
                    f"for {name} metadata {metadata}: authoritative (numeric_id, metadata) "
                    f"-> BlockState NBT with exact typed states and "
                    + ("NO persisted version field (explicit null; not derived)"
                       if versionless else "persisted version")
                    + "; numeric id from block_legacy_id_map.json"
                ),
            })
            stats["mappings"] += 1
    if pos != len(table):
        raise ValueError(f"trailing bytes in legacy table: {len(table) - pos}")
    return entries, stats


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--table", type=Path, required=True, help="id_meta_to_nbt/<version>.bin")
    ap.add_argument("--id-map", type=Path, required=True, help="block_legacy_id_map.json")
    ap.add_argument("--version", required=True, help="BDS game version, e.g. 1.12.0.28")
    ap.add_argument("--source-version", required=True, help="dataset version label, e.g. 1.12.0")
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args()

    legacy = json.loads(args.id_map.read_text(encoding="utf-8"))
    table = args.table.read_bytes()
    entries, stats = convert(table, legacy, args.source_version)
    doc = {
        "schema_version": 1,
        "game_version": args.version,
        "source": {
            "method": "authoritative_dataset",
            "platform": "dataset",
            # This is the DATASET file hash, explicitly not a BDS EXE fingerprint.
            "binary_sha256": sha256(args.table),
            "dataset": {
                "name": "PocketMine BedrockBlockUpgradeSchema",
                "version": args.source_version,
                "file_sha256": sha256(args.table),
                # The legacy numeric id table is a SECOND authoritative input;
                # record its hash too so a change to either file is visible.
                "legacy_id_map_sha256": sha256(args.id_map),
                "upstream": "https://github.com/pmmp/BedrockBlockUpgradeSchema",
            },
            "tool": "BMCBL import_legacy_ids from authoritative PocketMine id_meta_to_nbt dataset",
        },
        "registries": [{"namespace": "legacy_numeric_block", "entries": entries}],
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(doc, indent=2) + "\n", encoding="utf-8")
    print(f"{args.version}: {len(entries)} legacy mappings, stats={stats}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (ValueError, KeyError, struct.error) as error:
        print("legacy import conversion failed:", error, file=sys.stderr)
        raise SystemExit(1)
