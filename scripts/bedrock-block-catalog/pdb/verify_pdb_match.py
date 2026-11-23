#!/usr/bin/env python3
"""Validate PE/PDB identity via the CodeView RSDS record.

Prints one row per BDS version with the EXE's CodeView GUID+Age, the PDB's
own GUID+Age (from the PDB Info stream, when available), both SHA-256 values
and an explicit match verdict. This is a prerequisite for any symbol-based
extraction; a version without a matching pair is reported, never assumed.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import struct
import sys
from pathlib import Path

import pdbparse
import pefile


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def pe_codeview(exe: Path) -> dict | None:
    pe = pefile.PE(str(exe), fast_load=False)
    machine = int(pe.FILE_HEADER.Machine)
    for entry in getattr(pe, "DIRECTORY_ENTRY_DEBUG", []):
        data = entry.struct
        if getattr(data, "Type", 0) != 2:
            continue
        raw = pe.get_data(data.AddressOfRawData, data.SizeOfData)
        if raw[:4] != b"RSDS":
            continue
        guid = struct.unpack("<IHH8s", raw[4:20])
        guid_str = (
            f"{guid[0]:08x}-{guid[1]:04x}-{guid[2]:04x}-"
            f"{guid[3][0:2].hex()}-{guid[3][2:8].hex()}"
        )
        return {
            "machine": hex(machine),
            "guid": guid_str,
            "age": struct.unpack("<I", raw[20:24])[0],
            "pdb_path": raw[24:].split(b"\x00", 1)[0].decode("utf-8", "replace"),
        }
    return None


def pdb_identity(pdb_path: Path) -> dict | None:
    """Read GUID+Age from PDB 7 info stream (PDB 2.x has no such stream)."""
    pdb = pdbparse.parse(str(pdb_path), fast_load=True)
    info = getattr(pdb, "STREAM_PDB", None)
    if info is None:
        return None
    data = getattr(info, "data", b"")
    if isinstance(data, (bytes, bytearray)) and len(data) >= 24:
        # PDB 7 Info stream: Version(4) Signature(4) Age(4) GUID(16)
        guid = struct.unpack("<IHH8s", data[12:28])
        guid_str = (
            f"{guid[0]:08x}-{guid[1]:04x}-{guid[2]:04x}-"
            f"{guid[3][0:2].hex()}-{guid[3][2:8].hex()}"
        )
        return {"age": struct.unpack("<I", data[8:12])[0], "guid": guid_str}
    return None


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--extracted", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args()

    versions = sorted(
        p.name for p in args.extracted.iterdir()
        if p.is_dir() and (p / "bedrock_server.exe").is_file()
    )
    rows = []
    for version in versions:
        base = args.extracted / version
        exe = base / "bedrock_server.exe"
        pdb = base / "bedrock_server.pdb"
        row = {
            "game_version": version,
            "exe_sha256": sha256(exe),
            "pdb_sha256": sha256(pdb) if pdb.is_file() else None,
            "codeview": pe_codeview(exe),
            "pdb_identity": pdb_identity(pdb) if pdb.is_file() else None,
        }
        cv, pi = row["codeview"], row["pdb_identity"]
        if not pdb.is_file():
            row["verdict"] = "no_pdb_shipped"
        elif not cv or not pi:
            row["verdict"] = "unknown_identity"
        elif cv["guid"] == pi["guid"] and cv["age"] == pi["age"]:
            row["verdict"] = "matched"
        else:
            row["verdict"] = "MISMATCH"
        print(f"{version:12s} {row['verdict']:18s} "
              f"cv={cv['guid'] if cv else '-'}/{cv['age'] if cv else '-'} "
              f"pdb={pi['guid'] if pi else '-'}/{pi['age'] if pi else '-'}")
        rows.append(row)

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(rows, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {len(rows)} rows to {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
