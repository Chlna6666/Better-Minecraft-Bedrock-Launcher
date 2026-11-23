#!/usr/bin/env python3
"""Build a stageable block_type_runtime import from exact PDB evidence.

Only pairs where the registry symbol maps by an EXACT, EXE-byte-validated
identifier already present in that version's PDB literal table are emitted.
Derived/guessed names are dropped. The ordinal is the declaration index of
the static registry member (symbol evidence), NOT a legacy numeric ID and
NOT a network runtime ID. Output is always intended for --stage-unverified.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path


def snake_candidates(camel: str) -> list[str]:
    """Conservative spellings; only exact hits in the validated set are used."""
    s1 = re.sub(r"(?<=[a-z0-9])(?=[A-Z])", "_", camel)
    s1 = re.sub(r"(?<=[A-Z])(?=[A-Z][a-z])", "_", s1).lower()
    out = [s1]
    # Common Bedrock spellings where the symbol omits/renames a token.
    aliases = {
        "magma_block": "magma", "grass_path_block": "grass_path",
        "blackstone_double_slab": "double_blackstone_slab",
        "wood_block": "wood", "log_block": "log",
    }
    if s1 in aliases:
        out.append(aliases[s1])
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--recon", type=Path, required=True)
    ap.add_argument("--version", required=True)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--probes", type=Path, help="Script API probe directory for corroboration")
    args = ap.parse_args()

    recon = args.recon
    match = next(
        (r for r in json.loads((recon / "pdb-match.json").read_text(encoding="utf-8"))
         if r["game_version"] == args.version),
        None,
    )
    if not match or match.get("verdict") != "matched":
        print("no matched PE/PDB pair for version", args.version, file=sys.stderr)
        return 2
    vbt = json.loads((recon / f"vbt2-{args.version}.json").read_text(encoding="utf-8"))
    ids = json.loads((recon / f"ids-{args.version}.json").read_text(encoding="utf-8"))
    valid = {r["name"] for r in ids["identifiers"]}
    probe_names: set[str] = set()
    if args.probes:
        for path in sorted(args.probes.glob("*.json")):
            probe = json.loads(path.read_text(encoding="utf-8"))
            probe_names |= {
                b["name"] for b in probe.get("blocks", [])
                if isinstance(b, dict) and "name" in b
            }

    entries = []
    dropped = 0
    seen_names: set[str] = set()
    for ordinal, sym in enumerate(vbt["symbols"]):
        member = sym["symbol_member"]
        found = None
        for cand in snake_candidates(member):
            ident = "minecraft:" + cand
            if ident in valid:
                found = ident
                break
        if found is None or found in seen_names:
            dropped += 1
            continue
        seen_names.add(found)
        tier = "pdb_byte_and_script_probe" if found in probe_names else "pdb_byte_only"
        entries.append({
            "id": ordinal,
            "name": found,
            "evidence": (
                f"[evidence_tier={tier}] "
                f"PDB symbol {sym['symbol']} at segment {sym['segment']} "
                f"offset {sym['offset']}; registry declaration ordinal {ordinal}; "
                f"identifier literal decoded from ??_C@ symbol and byte-validated in "
                f"bedrock_server.exe SHA-256 {match['exe_sha256']}; "
                f"PE/PDB CodeView {match['codeview']['guid']}/{match['codeview']['age']} "
                f"matched PDB SHA-256 {match['pdb_sha256']}"
                + (
                    "; name independently corroborated by a Script API block probe"
                    if tier == "pdb_byte_and_script_probe" else ""
                )
            ),
        })

    document = {
        "schema_version": 1,
        "game_version": args.version,
        "source": {
            "method": "pdb_symbol_inspection",
            "platform": "windows-x64",
            "binary_sha256": match["exe_sha256"],
            "pdb_sha256": match["pdb_sha256"],
            "tool": "BMCBL local PDB extractor (pdbparse + pefile), exact-match only",
        },
        "registries": [{
            "namespace": "block_type_runtime",
            "entries": entries,
        }],
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(document, indent=2) + "\n", encoding="utf-8")
    print(f"wrote {len(entries)} exact-match registry entries ({dropped} dropped) to {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
