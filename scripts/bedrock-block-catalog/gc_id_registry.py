#!/usr/bin/env python3
"""Garbage-collect unreferenced content-addressed id-registry files.

The id-registry stores content-addressed `objects/<sha256>.bin` and
`evidence/<sha256>.json.zlib`. When a registry entry is replaced (e.g.
BMCBID02 -> BMCBID03) the old blobs are no longer referenced by index.json
but remain on disk. This tool removes ONLY files not referenced by the index.

Safety:
  * dry-run by default; deletion requires --apply
  * references are read exclusively from index.json (never from filenames)
  * evidence and object blobs are treated independently
  * a file whose SHA-256 does not match its name is NEVER deleted (corruption)
  * the index itself is never modified
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


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--catalog", type=Path, required=True,
                    help="crates/bedrock-world/data/block-catalog")
    ap.add_argument("--apply", action="store_true", help="actually delete unreferenced files")
    args = ap.parse_args()

    base = args.catalog / "id-registry"
    index_path = base / "index.json"
    if not index_path.is_file():
        print("no id-registry index", file=sys.stderr)
        return 2
    index = json.loads(index_path.read_text(encoding="utf-8"))

    referenced_objects: set[str] = set()
    referenced_evidence: set[str] = set()
    for entry in index["versions"].values():
        for record in entry["registries"].values():
            referenced_objects.add(record["object_sha256"])
            referenced_evidence.add(record["evidence_sha256"])

    stale_objects = []
    stale_evidence = []
    skipped_corrupt = []

    for path in sorted((base / "objects").glob("*.bin")):
        digest = path.stem
        if digest in referenced_objects:
            continue
        # Never delete a file whose name does not match its content.
        if sha256(path) != digest:
            skipped_corrupt.append(path)
            continue
        stale_objects.append(path)

    for path in sorted((base / "evidence").iterdir()):
        if not path.is_file():
            continue
        digest = path.name[: -len(".json.zlib")] if path.name.endswith(".json.zlib") else path.stem
        if digest in referenced_evidence:
            continue
        if sha256(path) != digest:
            skipped_corrupt.append(path)
            continue
        stale_evidence.append(path)

    for path in stale_objects + stale_evidence:
        print(("DELETE " if args.apply else "WOULD DELETE ") + str(path))
    for path in skipped_corrupt:
        print("SKIP (hash mismatch, possible corruption): " + str(path), file=sys.stderr)
    print(f"referenced objects={len(referenced_objects)} evidence={len(referenced_evidence)}; "
          f"stale objects={len(stale_objects)} evidence={len(stale_evidence)}; "
          f"mode={'apply' if args.apply else 'dry-run'}")

    if args.apply:
        for path in stale_objects + stale_evidence:
            path.unlink()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
