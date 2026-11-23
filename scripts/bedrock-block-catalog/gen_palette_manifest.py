#!/usr/bin/env python3
"""Write a provenance manifest for the checked-in raw runtime palette dumps.

These `.nbt` files are the output of pmmp/bds-mod-mapping's runtime block
palette dumper, archived in opencollab-incubator/BedrockBlockPaletteArchive.
Each state's file index is its network runtime ID for that version.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def version_key(label: str) -> tuple:
    parts = re.findall(r"\d+", label)
    return tuple(int(p) for p in parts)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--palettes", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args()

    files = sorted(args.palettes.glob("*.nbt"), key=lambda p: version_key(p.stem))
    versions = []
    for path in files:
        label = path.stem
        versions.append({
            "label": label,
            "file": f"palettes/{path.name}",
            "size": path.stat().st_size,
            "sha256": sha256(path),
            "source_url": (
                "https://raw.githubusercontent.com/opencollab-incubator/"
                f"BedrockBlockPaletteArchive/master/{path.name}"
            ),
        })
    manifest = {
        "schema_version": 1,
        "purpose": (
            "Raw, unprocessed runtime block-palette dumps for developer viewing. "
            "Each state's index is its network runtime ID. Do not edit these files."
        ),
        "source": "pmmp/bds-mod-mapping -> opencollab-incubator/BedrockBlockPaletteArchive",
        "format": "concatenated varint-NBT root compounds; network runtime ID = file index",
        "evidence_boundary": (
            "Runtime dump evidence. Beta build labels are preserved verbatim and are not "
            "four-component stable BDS versions. No default state field; states are full permutations."
        ),
        "version_count": len(versions),
        "versions": versions,
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    print(f"wrote manifest with {len(versions)} versions to {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
