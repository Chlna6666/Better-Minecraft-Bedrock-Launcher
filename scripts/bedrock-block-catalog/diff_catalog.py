#!/usr/bin/env python3
"""Compare BMCBL block catalogs without inventing name or numeric-ID migrations.

Read-only: outputs changes and *candidates*, never authoritative aliases.
"""
from __future__ import annotations

import argparse
from collections import defaultdict
import json
from pathlib import Path


class Reader:
    def __init__(self, payload: bytes):
        self.payload = payload
        self.cursor = 0

    def take(self, count: int) -> bytes:
        if count < 0 or self.cursor + count > len(self.payload):
            raise ValueError("Catalog manifest truncated")
        result = self.payload[self.cursor:self.cursor + count]
        self.cursor += count
        return result

    def varint(self) -> int:
        result = 0
        for shift in range(0, 70, 7):
            value = self.take(1)[0]
            result |= (value & 0x7f) << shift
            if not value & 0x80:
                return result
        raise ValueError("Catalog LEB128 too large")

    def string(self) -> str:
        length = self.varint()
        if length > 4096:
            raise ValueError("Unexpectedly long block identifier")
        return self.take(length).decode("utf-8")


def manifest(path: Path) -> dict[str, tuple[str, int]]:
    reader = Reader(path.read_bytes())
    if reader.take(8) != b"BMCBLP01":
        raise ValueError("Unsupported catalog manifest header")
    count = reader.varint()
    if not 1 <= count <= 10000:
        raise ValueError("Invalid block count")
    blocks = {}
    for _ in range(count):
        name = reader.string()
        digest = f"{int.from_bytes(reader.take(16), 'little'):032x}"
        default_index = reader.varint()
        if name in blocks:
            raise ValueError("Duplicate block identifier " + name)
        blocks[name] = (digest, default_index)
    if reader.cursor != len(reader.payload):
        raise ValueError("Trailing bytes in catalog manifest")
    return blocks


def compare(catalog: Path, source: str, target: str) -> dict:
    index = json.loads((catalog / "index.json").read_text(encoding="utf-8"))
    if index["format_version"] != 1:
        raise ValueError("Unsupported catalog index format")
    versions = index["versions"]
    if source not in versions or target not in versions:
        raise ValueError("Both source and target must already be captured in index.json")
    before = manifest(catalog / "manifests" / (versions[source]["manifest"] + ".bin"))
    after = manifest(catalog / "manifests" / (versions[target]["manifest"] + ".bin"))
    source_names = set(before)
    target_names = set(after)
    unchanged = sorted(n for n in source_names & target_names if before[n] == after[n])
    modified = sorted(n for n in source_names & target_names if before[n] != after[n])
    removed = sorted(source_names - target_names)
    added = sorted(target_names - source_names)
    candidates = []
    added_by_content = defaultdict(list)
    for name in added:
        added_by_content[after[name]].append(name)
    for old_name in removed:
        for new_name in added_by_content.get(before[old_name], []):
            candidates.append({
                "from": old_name,
                "to": new_name,
                "evidence": "equal states and default-state ordinal only",
                "verified": False,
            })
    return {
        "source_version": source,
        "target_version": target,
        "source_storage_version": versions[source]["storage_version"],
        "target_storage_version": versions[target]["storage_version"],
        "counts": {
            "unchanged_identifier_and_semantics": len(unchanged),
            "modified_same_identifier": len(modified),
            "removed_identifiers": len(removed),
            "added_identifiers": len(added),
            "unverified_rename_candidates": len(candidates),
        },
        "modified": modified,
        "removed": removed,
        "added": added,
        "rename_candidates_not_authoritative": candidates,
        "warning": (
            "Equal property sets are not evidence of semantic identity. "
            "Legacy numeric IDs, metadata, runtime network IDs and "
            "migration rules require separate versioned evidence."
        ),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--catalog", type=Path, default=Path(
        "crates/bedrock-world/data/block-catalog"))
    parser.add_argument("--from", dest="source", required=True)
    parser.add_argument("--to", dest="target", required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    report = compare(args.catalog, args.source, args.target)
    result = json.dumps(report, ensure_ascii=False, indent=2) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(result, encoding="utf-8")
    else:
        print(result, end="")


if __name__ == "__main__":
    main()
