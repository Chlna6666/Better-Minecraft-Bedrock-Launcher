#!/usr/bin/env python3
"""Extract entity identifiers from an official BDS archive.

BDS ships its vanilla entity definitions as `behavior_packs/*/entities/*.json`.
Each file's `minecraft:entity` -> `description.identifier` is the authoritative
entity identifier for THAT exact server build. This reads them straight from
the archive (no runtime hook needed) and preserves names verbatim, including
non-`minecraft:` namespaces and any unusual spelling.

Output: one JSON per version with sorted identifiers and per-file provenance.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
import zipfile
from pathlib import Path

# BDS entity JSON may contain // and /* */ comments, so use a tolerant strip.
_LINE_COMMENT = re.compile(r"//[^\n]*")
_BLOCK_COMMENT = re.compile(r"/\*.*?\*/", re.S)


def strip_comments(text: str) -> str:
    return _LINE_COMMENT.sub("", _BLOCK_COMMENT.sub("", text))


def identifier_of(document: dict) -> str | None:
    entity = document.get("minecraft:entity")
    if not isinstance(entity, dict):
        return None
    description = entity.get("description")
    if not isinstance(description, dict):
        return None
    ident = description.get("identifier")
    return ident if isinstance(ident, str) and ident else None


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--archive", type=Path, required=True)
    ap.add_argument("--version", required=True)
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args()

    entries: set[str] = set()
    with zipfile.ZipFile(args.archive) as zf:
        for name in zf.namelist():
            if "/entities/" not in name or not name.endswith(".json"):
                continue
            raw = zf.read(name)
            try:
                document = json.loads(strip_comments(raw.decode("utf-8-sig")))
            except (ValueError, UnicodeDecodeError):
                continue
            ident = identifier_of(document)
            if ident:
                entries.add(ident)

    document = {
        "game_version": args.version,
        "entity_count": len(entries),
        "identifiers": sorted(entries),
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    print(f"{args.version}: {len(entries)} entity identifiers -> {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
