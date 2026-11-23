#!/usr/bin/env python3
"""Cross-validate PDB identifiers against Script API probes and report
name evolution candidates. Never asserts semantic identity: differences are
reported as candidates requiring human review, per the maintenance guide.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--recon", type=Path, required=True)
    ap.add_argument("--probes", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args()

    probes: dict[str, set[str]] = {}
    for path in sorted(args.probes.glob("*.json")):
        data = json.loads(path.read_text(encoding="utf-8"))
        if data.get("warning") and "API-only" not in data.get("warning", ""):
            continue
        probes[str(data.get("game_version", path.stem))] = {
            b["name"] for b in data.get("blocks", []) if isinstance(b, dict) and "name" in b
        }
    oracle = set().union(*probes.values()) if probes else set()

    pdb_sets: dict[str, set[str]] = {}
    for path in sorted(args.recon.glob("ids-*.json")):
        data = json.loads(path.read_text(encoding="utf-8"))
        version = path.stem.removeprefix("ids-")
        pdb_sets[version] = {r["name"] for r in data.get("identifiers", [])}

    def version_key(v: str) -> tuple[int, ...]:
        return tuple(int(p) for p in v.split("."))

    versions = sorted(pdb_sets, key=version_key)
    report = {
        "schema_version": 1,
        "warning": "Candidates only. Identifier equality/inequality does not prove semantic identity or rename.",
        "script_probe_oracle": {
            "versions": sorted(probes),
            "union_block_names": len(oracle),
        },
        "pdb_identifiers": {},
        "adjacent_version_name_changes": [],
    }
    for version, ids in pdb_sets.items():
        blockish = ids & oracle
        report["pdb_identifiers"][version] = {
            "total_validated_identifiers": len(ids),
            "names_seen_in_script_probes": len(blockish),
            "names_only_in_pdb": len(ids - oracle),
        }
    for older, newer in zip(versions, versions[1:]):
        a, b = pdb_sets[older], pdb_sets[newer]
        report["adjacent_version_name_changes"].append({
            "from": older,
            "to": newer,
            "added": sorted(b - a),
            "removed": sorted(a - b),
            "added_count": len(b - a),
            "removed_count": len(a - b),
        })

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote cross-validation report to {args.out}")
    for row in report["adjacent_version_name_changes"]:
        print(f"  {row['from']} -> {row['to']}: +{row['added_count']} / -{row['removed_count']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
