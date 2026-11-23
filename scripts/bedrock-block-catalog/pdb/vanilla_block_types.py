#!/usr/bin/env python3
"""Extract ordered vanilla block registry symbols from a BDS PDB.

Two symbol families are recognised:
  * VanillaBlockTypes::m<Name>  - WeakPtr<BlockLegacy> (older BDS)
  * VanillaBlockTypeIds::<Name> - HashedString block identifiers (newer BDS)

Both are static, ordered members; their declaration order is the vanilla
block registry order and the PDB offsets expose the layout. This is
symbol-only evidence: it does NOT by itself prove a network runtime ID or a
legacy numeric block ID.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

import pdbparse

# VanillaBlockTypes::m<Name> (WeakPtr<BlockLegacy>) - older BDS.
MEMBER_LEGACY = re.compile(
    r"\?m(?P<name>[A-Za-z0-9_]+)@VanillaBlockTypes@@3V\?\$WeakPtr@VBlockLegacy@@@@A$"
)
# VanillaBlockTypeIds::<Name> (HashedString) - newer BDS, identifiers are the
# camel-cased block name (e.g. ?Stone@VanillaBlockTypeIds@@3VHashedString@@B).
MEMBER_IDS = re.compile(
    r"\?(?P<name>[A-Za-z0-9_]+)@VanillaBlockTypeIds@@3VHashedString@@B$"
)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--pdb", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args()

    try:
        pdb = pdbparse.parse(str(args.pdb))
    except (AttributeError, ValueError) as exc:
        # Some old PDBs (e.g. 1.6.0.15) hit a pdbparse TPI fixup bug. The
        # global symbol stream is still usable with lazy type loading.
        print(f"full parse failed ({exc}); retrying fast_load", flush=True)
        pdb = pdbparse.parse(str(args.pdb), fast_load=True)
    gsym = getattr(pdb, "STREAM_GSYM", None)
    if gsym is None or not hasattr(gsym, "vars"):
        try:
            dbg = getattr(pdb, "STREAM_DBI", None)
            if dbg is not None:
                gsym = pdb.streams[int(dbg.gsym_file)]
        except Exception:
            gsym = None
    if gsym is None or not hasattr(gsym, "vars"):
        print("no global symbol stream; cannot extract", file=sys.stderr)
        return 2

    rows = []
    seen: set[tuple[int, int]] = set()
    for symname, sym in gsym.vars.items():
        kind = None
        m = MEMBER_LEGACY.match(symname)
        if m:
            kind = "block_legacy"
        else:
            m = MEMBER_IDS.match(symname)
            if m:
                kind = "block_type_id"
        if not m:
            continue
        segment = int(getattr(sym, "segment", -1))
        offset = int(getattr(sym, "offset", -1))
        key = (segment, offset)
        if key in seen:
            continue
        seen.add(key)
        rows.append({
            "symbol_member": m.group("name"),
            "family": kind,
            "segment": segment,
            "offset": offset,
            "symbol": symname,
        })
    # Layout order: segment then offset.
    rows.sort(key=lambda r: (r["segment"], r["offset"]))

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps({
        "kind": "vanilla_block_registry_symbols",
        "count": len(rows),
        "families": {
            family: sum(1 for r in rows if r["family"] == family)
            for family in sorted({r["family"] for r in rows})
        },
        "symbols": rows,
    }, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {len(rows)} registry symbols to {args.out}", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
