#!/usr/bin/env python3
"""Parse a bds-mod-mapping varint-NBT block palette dump.

Format (Bedrock network varint NBT):
  concatenated root TAG_Compound values; each state's index in the file is its
  network runtime ID. String/array lengths and numeric payloads are zigzag
  varints. This is the output of pmmp/bds-mod-mapping's `getBlockPalette()`
  dumper, archived in opencollab-incubator/BedrockBlockPaletteArchive.

Emits per-state: name, typed states, NBT storage version, network runtime id.
No default state and no legacy numeric ID are inferred here.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def read_varint(data: bytes, pos: int) -> tuple[int, int]:
    result = 0
    shift = 0
    while True:
        if pos >= len(data) or shift > 63:
            raise ValueError("bad varint")
        b = data[pos]
        pos += 1
        result |= (b & 0x7f) << shift
        if not (b & 0x80):
            return result, pos
        shift += 7


def zigzag(value: int) -> int:
    return (value >> 1) ^ -(value & 1)


class VarintNBT:
    def __init__(self, data: bytes, pos: int):
        self.data = data
        self.pos = pos

    def _uvar(self) -> int:
        value, self.pos = read_varint(self.data, self.pos)
        return value

    def _svar(self) -> int:
        return zigzag(self._uvar())

    def _string(self) -> str:
        length = self._uvar()
        raw = self.data[self.pos:self.pos + length]
        if len(raw) != length:
            raise ValueError("truncated string")
        self.pos += length
        return raw.decode("utf-8")

    def payload(self, tag: int):
        if tag == 1:   # Byte
            return self._svar()
        if tag == 2:   # Short
            return self._svar()
        if tag == 3:   # Int
            return self._svar()
        if tag == 4:   # Long
            return self._svar()
        if tag == 5:   # Float
            import struct
            v = struct.unpack_from("<f", self.data, self.pos)[0]
            self.pos += 4
            return v
        if tag == 6:   # Double
            import struct
            v = struct.unpack_from("<d", self.data, self.pos)[0]
            self.pos += 8
            return v
        if tag == 7:   # Byte_Array
            length = self._svar()
            raw = self.data[self.pos:self.pos + length]
            self.pos += length
            return list(raw)
        if tag == 8:   # String
            return self._string()
        if tag == 9:   # List
            item_type = self.data[self.pos]
            self.pos += 1
            length = self._svar()
            return [self.payload(item_type) for _ in range(length)]
        if tag == 10:  # Compound
            out: dict[str, tuple[int, object]] = {}
            while True:
                child = self.data[self.pos]
                self.pos += 1
                if child == 0:
                    return out
                name = self._string()
                out[name] = (child, self.payload(child))
        if tag == 11:  # Int_Array
            length = self._svar()
            return [self._svar() for _ in range(length)]
        if tag == 12:  # Long_Array
            length = self._svar()
            return [self._svar() for _ in range(length)]
        raise ValueError(f"unsupported NBT tag {tag}")


def parse_states(path: Path) -> tuple[list[dict], dict]:
    data = path.read_bytes()
    pos = 0
    states = []
    versions: set[int] = set()
    while pos < len(data):
        if data[pos] != 10:
            raise ValueError(f"expected TAG_Compound at {pos}, got {data[pos]}")
        pos += 1
        reader = VarintNBT(data, pos)
        reader._string()  # root name (usually empty)
        root = reader.payload(10)
        pos = reader.pos
        name_tag = root.get("name")
        if not name_tag or name_tag[0] != 8:
            raise ValueError(f"state {len(states)} has no string name")
        state_props = {}
        raw_states = root.get("states")
        if raw_states:
            if raw_states[0] == 10:
                for key, (tag, value) in raw_states[1].items():
                    type_name = {1: "Byte", 2: "Short", 3: "Int", 4: "Long", 8: "String"}.get(tag)
                    if type_name is None:
                        raise ValueError(f"unsupported state property tag {tag}")
                    state_props[key] = {"type": type_name, "value": value}
            else:
                raise ValueError("states is not a compound")
        version_tag = root.get("version")
        if not version_tag or version_tag[0] != 3:
            raise ValueError(f"state {len(states)} has no int version")
        versions.add(version_tag[1])
        states.append({
            "network_runtime_id": len(states),
            "name": name_tag[1],
            "states": state_props,
            "version": version_tag[1],
        })
    return states, {"count": len(states), "versions": sorted(versions), "sha256": sha256(path)}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--input", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args()
    states, meta = parse_states(args.input)
    doc = {
        "schema_version": 1,
        "source": "opencollab-incubator/BedrockBlockPaletteArchive (bds-mod-mapping runtime dump)",
        "input": str(args.input),
        "input_sha256": meta["sha256"],
        "state_count": meta["count"],
        "nbt_versions": meta["versions"],
        "states": states,
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(doc, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"parsed {meta['count']} states, versions {meta['versions']} -> {args.out}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (ValueError, KeyError, IndexError) as error:
        print("palette parse failed:", error, file=sys.stderr)
        raise SystemExit(1)
