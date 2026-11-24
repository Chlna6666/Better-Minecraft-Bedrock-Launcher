#!/usr/bin/env python3
"""Probe each extracted BDS version for the runtime registry map symbol.

Reports, per version: whether BlockTypeRegistry::mBlockLookupMap exists, its
RVA (section VA + offset), the containing PE section and its file offset, plus
the .data section RVA needed by the runtime dumper. Read-only.
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

try:
    import pdbparse
    import pefile
except ImportError:
    print("pdbparse and pefile are required", file=sys.stderr)
    raise SystemExit(2)

MAP_SUFFIX = "?mBlockLookupMap@BlockTypeRegistry@@"


def find_map(gsym) -> tuple[str, int, int] | None:
    for name, sym in gsym.vars.items():
        if MAP_SUFFIX in name and "map@" in name:
            return name, int(sym.segment), int(sym.offset)
    return None


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--extracted", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args()

    rows = []
    for version_dir in sorted(p for p in args.extracted.iterdir() if p.is_dir()):
        version = version_dir.name
        pdb_path = version_dir / "bedrock_server.pdb"
        exe_path = version_dir / "bedrock_server.exe"
        if not pdb_path.is_file() or not exe_path.is_file():
            continue
        try:
            try:
                pdb = pdbparse.parse(str(pdb_path))
            except (AttributeError, ValueError):
                pdb = pdbparse.parse(str(pdb_path), fast_load=True)
            gsym = getattr(pdb, "STREAM_GSYM", None)
            if gsym is None or not hasattr(gsym, "vars"):
                rows.append({"version": version, "status": "no_gsym"})
                continue
            found = find_map(gsym)
            if found is None:
                rows.append({"version": version, "status": "no_map_symbol"})
                continue
            symbol, segment, offset = found
            sections = list(pdb.STREAM_SECT_HDR.sections)
            section_va = int(sections[segment - 1].VirtualAddress)
            rva = section_va + offset
            pe = pefile.PE(str(exe_path), fast_load=True)
            file_off = None
            for sec in pe.sections:
                if sec.VirtualAddress <= rva < sec.VirtualAddress + max(
                        sec.Misc_VirtualSize, sec.SizeOfRawData):
                    file_off = sec.PointerToRawData + (rva - sec.VirtualAddress)
                    break
            rows.append({
                "version": version,
                "status": "ok",
                "symbol": symbol,
                "segment": segment,
                "offset": offset,
                "section_va": hex(section_va),
                "map_rva": hex(rva),
                "file_offset": file_off,
            })
            print(f"{version:12s} map_rva={hex(rva)} file={file_off}")
        except Exception as error:  # noqa: BLE001 - report, never abort the sweep
            rows.append({"version": version, "status": "error", "error": str(error)})
            print(f"{version:12s} ERROR {error}", file=sys.stderr)

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(rows, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {len(rows)} rows to {args.out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
