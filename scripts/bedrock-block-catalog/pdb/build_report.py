#!/usr/bin/env python3
"""Build an auditable PDB evidence report (no catalog writes).

Correlates, per BDS version:
  * PE/PDB CodeView identity (from verify_pdb_match.py)
  * archive SHA-1/SHA-256 provenance (from extract_bds.py)
  * ordered VanillaBlockTypes/Ids registry symbols (vanilla_block_types.py)
  * validated literal minecraft: identifiers (extract_identifiers.py)

Nothing here asserts a legacy numeric ID or a network runtime ID. Those are
different namespaces and cannot be inferred from static symbols alone.
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def load(path: Path, default=None):
    if not path.is_file():
        return default
    return json.loads(path.read_text(encoding="utf-8"))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--recon", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args()

    recon = args.recon
    matches = load(recon / "pdb-match.json", []) or []
    extract = {
        row["game_version"]: row
        for row in (load(recon.parent / "bds-extracted" / "manifest.json", []) or [])
    }
    report = {
        "schema_version": 1,
        "purpose": "Evidence only. No legacy numeric ID or network runtime ID is inferred from PDB symbols.",
        "namespace_policy": {
            "block_type_runtime": "declaration order of VanillaBlockTypes/Ids static members (symbol evidence only)",
            "legacy_numeric_block": "NOT extracted; needs verified (id,metadata) dataset or runtime dump",
            "network_runtime": "NOT extracted; session/palette-specific",
        },
        "versions": [],
    }

    for match in sorted(matches, key=lambda r: r["game_version"]):
        version = match["game_version"]
        vbt = load(recon / f"vbt2-{version}.json")
        ids = load(recon / f"ids-{version}.json")
        arc = extract.get(version, {})
        entry = {
            "game_version": version,
            "codeview_verdict": match.get("verdict"),
            "codeview": match.get("codeview"),
            "pdb_identity": match.get("pdb_identity"),
            "exe_sha256": match.get("exe_sha256"),
            "pdb_sha256": match.get("pdb_sha256"),
            "archive_sha1": arc.get("archive_sha1"),
            "archive_sha1_verified": arc.get("archive_sha1_verified"),
            "archive_sha256": arc.get("archive_sha256"),
            "source_url": arc.get("source_url"),
            "registry_symbols": {
                "count": vbt.get("count") if vbt else None,
                "families": vbt.get("families") if vbt else None,
            },
            "validated_identifiers": {
                "count": ids.get("count") if ids else None,
                "rejected_byte_mismatch": ids.get("rejected_byte_mismatch") if ids else None,
            },
            "can_import_block_type_runtime": bool(
                match.get("verdict") == "matched" and vbt and vbt.get("count", 0) > 0
            ),
        }
        report["versions"].append(entry)
        print(f"{version:12s} match={entry['codeview_verdict']:16s} "
              f"registry={entry['registry_symbols']['count']} "
              f"identifiers={entry['validated_identifiers']['count']}")

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote report to {args.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
