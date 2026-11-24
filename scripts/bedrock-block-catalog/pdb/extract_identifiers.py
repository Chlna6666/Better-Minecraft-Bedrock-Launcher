#!/usr/bin/env python3
"""Extract literal `minecraft:` identifiers from a BDS PDB string-literal table.

MSVC emits each string literal as a `??_C@...` public symbol whose name embeds
the literal text with `?X` escapes. This tool decodes those symbols, keeps the
ones that are valid Bedrock identifiers, and records their section/RVA.
It also cross-checks every decoded string against the raw bytes of the matched
EXE so a decoding mistake cannot silently invent an identifier.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

import pdbparse

# MSVC escapes in the string-literal symbol text. The full table is large;
# these are the ones that occur in Bedrock identifier literals. We validate
# empirically against EXE bytes and drop anything that fails the round-trip.
ESCAPES = {
    "?0": " ", "?1": "@", "?2": "?", "?3": ":", "?4": ".",
    "?5": "\\", "?6": '"', "?7": "'", "?8": "-", "?9": "\n",
}
IDENT = re.compile(r"^minecraft:[a-z0-9_./-]{1,64}$")


def decode_literal(name: str) -> str | None:
    """Decode the literal text of a ??_C@ symbol; None if not decodable."""
    if not name.startswith("??_C@"):
        return None
    parts = name.split("@")
    # ??_C@_0<len>@<hash>@<text>@  ->  ['??_C', '_0<len>', '<hash>', '<text>', '']
    if len(parts) < 4:
        return None
    text = parts[3]
    out: list[str] = []
    i = 0
    while i < len(text):
        if text[i] == "?":
            # A literal '?' is itself encoded (?2). Any other ?X pair must be
            # a known escape; an unrecognised one is refused rather than
            # passed through as text (which could invent a wrong identifier).
            if i + 1 >= len(text) or text[i:i + 2] not in ESCAPES:
                return None
            out.append(ESCAPES[text[i:i + 2]])
            i += 2
        else:
            out.append(text[i])
            i += 1
    return "".join(out)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--pdb", type=Path, required=True)
    ap.add_argument("--exe", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args()

    try:
        pdb = pdbparse.parse(str(args.pdb))
    except (AttributeError, ValueError):
        pdb = pdbparse.parse(str(args.pdb), fast_load=True)
    gsym = getattr(pdb, "STREAM_GSYM", None)
    if gsym is None or not hasattr(gsym, "vars"):
        print("no global symbol stream", file=sys.stderr)
        return 2

    exe_bytes = args.exe.read_bytes()

    # Validate at the symbol's ADDRESS, not merely anywhere in the EXE.
    # Map (PDB segment, offset) -> file offset via section headers + PE sections.
    import pefile
    pe = pefile.PE(str(args.exe), fast_load=True)
    sections = list(pdb.STREAM_SECT_HDR.sections)
    section_map = []
    for section in pe.sections:
        name = section.Name.rstrip(b"\x00").decode("ascii", "replace")
        section_map.append((int(section.VirtualAddress),
                            max(int(section.Misc_VirtualSize), int(section.SizeOfRawData)),
                            int(section.PointerToRawData), name))

    def literal_at(segment: int, offset: int) -> bytes | None:
        if not (1 <= segment <= len(sections)):
            return None
        rva = int(sections[segment - 1].VirtualAddress) + int(offset)
        for base, size, pointer, _ in section_map:
            if base <= rva < base + size:
                start = pointer + (rva - base)
                end = exe_bytes.find(b"\x00", start, min(start + 512, len(exe_bytes)))
                if end < 0:
                    end = min(start + 512, len(exe_bytes))
                return exe_bytes[start:end]
        return None

    rows: list[dict] = []
    rejected_decode = 0
    rejected_bytes = 0
    rejected_address = 0
    seen: set[str] = set()
    for symname, sym in gsym.vars.items():
        decoded = decode_literal(symname)
        if not decoded or not IDENT.match(decoded):
            continue
        if decoded in seen:
            continue
        seen.add(decoded)
        segment = int(getattr(sym, "segment", -1))
        offset = int(getattr(sym, "offset", -1))
        # Empirical validation: the literal must physically exist in the EXE
        # AND the PDB symbol's address must point at that literal.
        if decoded.encode("ascii") not in exe_bytes:
            rejected_bytes += 1
            continue
        at_address = literal_at(segment, offset)
        if at_address != decoded.encode("ascii"):
            rejected_address += 1
            continue
        rows.append({
            "name": decoded,
            "segment": segment,
            "offset": offset,
            "symbol": symname,
        })
    rows.sort(key=lambda r: (r["segment"], r["offset"], r["name"]))

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps({
        "kind": "pdb_string_literal_identifiers",
        "count": len(rows),
        "rejected_byte_mismatch": rejected_bytes,
        "rejected_address_mismatch": rejected_address,
        "identifiers": rows,
    }, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {len(rows)} address-validated identifiers "
          f"({rejected_bytes} byte-rejected, {rejected_address} address-rejected) to {args.out}",
          flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
