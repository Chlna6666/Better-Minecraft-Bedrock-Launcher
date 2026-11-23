#!/usr/bin/env python3
"""Recover block registry symbol NAMES from an old PDB whose streams pdbparse
cannot decode (e.g. BDS 1.6.0.15, MSVC 7-era DBI/TPI).

This scans raw PDB bytes for the MSVC-mangled `?m<Name>@VanillaBlockTypes@@`
pattern. It deliberately records NAME-ONLY evidence: without a parsed symbol
stream there is no trustworthy segment/offset, so no registry ordinal is
emitted. These names cannot be used as numeric IDs.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path

PATTERN = re.compile(rb"\?m([A-Za-z0-9_]+)@VanillaBlockTypes@@")


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--pdb", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args()

    raw = args.pdb.read_bytes()
    names = sorted({m.group(1).decode("ascii") for m in PATTERN.finditer(raw)})
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps({
        "kind": "vanilla_block_registry_symbols",
        "extraction": "raw_byte_scan_name_only",
        "pdb_sha256": sha256(args.pdb),
        "count": len(names),
        "ordinal_available": False,
        "families": {"block_legacy_name_only": len(names)},
        "symbols": [
            {"symbol_member": n, "family": "block_legacy_name_only",
             "segment": None, "offset": None,
             "symbol": f"?m{n}@VanillaBlockTypes@@"}
            for n in names
        ],
    }, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {len(names)} name-only registry symbols to {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
