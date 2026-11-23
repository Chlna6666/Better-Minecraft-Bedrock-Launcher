#!/usr/bin/env python3
"""Reconnaissance probe for a Bedrock Dedicated Server PE/PDB pair.

Read-only: prints CodeView identity, block-related symbols and candidate
block identifiers found in the PDB. It never writes catalog data.
"""
from __future__ import annotations

import argparse
import hashlib
import re
import sys
from pathlib import Path

import pdbparse
import pefile

# A Bedrock block identifier: namespace:path, e.g. minecraft:stone,
# minecraft:wall_sign, minecraft:double_plant. Keep this deliberately narrow
# so the report does not fill with arbitrary C++ strings.
BLOCK_ID = re.compile(rb"\bminecraft:[a-z0-9_./-]{1,64}\b")


def codeview(exe: Path) -> dict:
    pe = pefile.PE(str(exe), fast_load=False)
    found = []
    for entry in getattr(pe, "DIRECTORY_ENTRY_DEBUG", []):
        data = entry.struct
        if getattr(data, "Type", 0) != 2:  # IMAGE_DEBUG_TYPE_CODEVIEW
            continue
        raw = pe.get_data(data.AddressOfRawData, data.SizeOfData)
        if raw.startswith(b"RSDS"):
            guid = raw[4:20]
            guid_str = (
                f"{int.from_bytes(guid[0:4], 'little'):08x}-"
                f"{int.from_bytes(guid[4:6], 'little'):04x}-"
                f"{int.from_bytes(guid[6:8], 'little'):04x}-"
                f"{guid[8:10].hex()}-{guid[10:16].hex()}"
            )
            age = int.from_bytes(raw[20:24], "little")
            path = raw[24:].split(b"\x00", 1)[0].decode("utf-8", "replace")
            found.append({"guid": guid_str, "age": age, "pdb_path": path})
    return {"machine": hex(pe.FILE_HEADER.Machine), "codeview": found}


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", type=Path, required=True)
    ap.add_argument("--pdb", type=Path, required=True)
    ap.add_argument("--max-names", type=int, default=40)
    args = ap.parse_args()

    print("=== binary identity ===")
    print("exe sha256:", sha256(args.exe))
    print("pdb sha256:", sha256(args.pdb))
    cv = codeview(args.exe)
    print("pe machine:", cv["machine"])
    for item in cv["codeview"]:
        print("codeview:", item)

    print("\n=== parsing PDB ===")
    pdb = pdbparse.parse(str(args.pdb))
    dbg = pdb.STREAM_DBI
    gsym = pdb.STREAM_GSYM
    print("machine from PDB:", int(dbg.machine))
    print("global symbols:", len(gsym.globals) if gsym else 0)
    print("global funcs:", len(gsym.funcs) if gsym else 0)
    print("global vars:", len(gsym.vars) if gsym else 0)

    print("\n=== block-related global functions ===")
    hits = [
        (name, sym)
        for name, sym in (gsym.funcs.items() if gsym else [])
        if re.search(r"block|blockpalette|blockstate", name, re.I)
    ]
    for name, sym in hits[:120]:
        seg = getattr(sym, "segment", None)
        off = getattr(sym, "offset", None)
        print(f"  seg={seg} off={off} {name}")
    print(f"(total {len(hits)} block-related functions)")

    print("\n=== block-related global variables ===")
    vhits = [
        (name, sym)
        for name, sym in (gsym.vars.items() if gsym else [])
        if re.search(r"block|palette|blockstate", name, re.I)
    ]
    for name, sym in vhits[:120]:
        seg = getattr(sym, "segment", None)
        off = getattr(sym, "offset", None)
        print(f"  seg={seg} off={off} {name}")
    print(f"(total {len(vhits)} block-related variables)")

    print("\n=== candidate block identifiers in PDB ===")
    names: set[bytes] = set()
    with args.pdb.open("rb") as fh:
        while True:
            chunk = fh.read(1 << 22)
            if not chunk:
                break
            for m in BLOCK_ID.finditer(chunk):
                names.add(m.group(0))
    ordered = sorted(names)
    for raw in ordered[: args.max_names]:
        print("  ", raw.decode("ascii"))
    print(f"(total {len(ordered)} distinct minecraft: identifiers)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
