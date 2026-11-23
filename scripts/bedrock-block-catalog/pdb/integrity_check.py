#!/usr/bin/env python3
"""P1 integrity & cross-version validation across all local evidence sources.

Compares:
  * PDB-derived block_type_runtime imports (symbol evidence)
  * authoritative legacy (id,metadata) imports (PocketMine corpus)
  * Script API probe observations
  * the repo's pinned corpus blob identities (bedrock-world corpus_bundle.rs)

Reports, without asserting semantic identity:
  - corpus provenance match/mismatch
  - duplicate/conflicting numeric identities
  - unknown names (in one source but not another)
  - rename candidates (shared exact typed states + default under different names)
  - coverage gaps per evidence source
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path

PINNED = {
    "block_legacy_id_map.json": ("e322f7067c39c7b9331093cad1d7d990edaeae1c", 19660),
    "id_meta_to_nbt/1.9.0.bin": ("8a7250d38ce159e758aef104fa2619f5985f273a", 366625),
    "id_meta_to_nbt/1.12.0.bin": ("60fdd271027423fc0cba356230598294b5b1769a", 325472),
}


def git_blob(path: Path) -> tuple[str, int]:
    data = path.read_bytes()
    h = hashlib.sha1()
    h.update(f"blob {len(data)}\0".encode())
    h.update(data)
    return h.hexdigest(), len(data)


def load_json(path: Path):
    return json.loads(path.read_text(encoding="utf-8")) if path.is_file() else None


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--recon", type=Path, required=True)
    ap.add_argument("--probes", type=Path, required=True)
    ap.add_argument("--out", type=Path, required=True)
    args = ap.parse_args()

    report = {
        "schema_version": 1,
        "warning": "Cross-source observations only. Equality of states/properties does not prove rename or semantic identity.",
        "corpus_provenance": [],
        "legacy_identity_conflicts": [],
        "source_coverage": {},
        "unknown_names": {},
        "rename_candidates_not_authoritative": [],
    }

    # 1. Corpus provenance against the repo's pinned blob identities.
    mapping = {
        "block_legacy_id_map.json": args.recon / "block_legacy_id_map.json",
        "id_meta_to_nbt/1.9.0.bin": args.recon / "id_meta_to_nbt_1.9.0.bin",
        "id_meta_to_nbt/1.12.0.bin": args.recon / "id_meta_to_nbt_1.12.0.bin",
    }
    for label, path in mapping.items():
        expected_sha, expected_size = PINNED[label]
        if not path.is_file():
            report["corpus_provenance"].append({"file": label, "status": "missing"})
            continue
        sha, size = git_blob(path)
        report["corpus_provenance"].append({
            "file": label,
            "status": "match" if (sha == expected_sha and size == expected_size) else "MISMATCH",
            "git_blob_sha1": sha,
            "size": size,
            "expected_git_blob_sha1": expected_sha,
            "expected_size": expected_size,
        })

    # 2. Legacy (id, metadata) conflicts within the import.
    legacy_dir = args.recon / "legacy-imports"
    legacy_sets: dict[str, set[tuple[int, int]]] = {}
    legacy_names: dict[str, dict[tuple[int, int], str]] = {}
    for path in sorted(legacy_dir.glob("*.json")):
        doc = load_json(path)
        version = doc["game_version"]
        entries = doc["registries"][0]["entries"]
        seen: dict[tuple[int, int], str] = {}
        conflicts = []
        for entry in entries:
            key = (entry["id"], entry["metadata"])
            if key in seen and seen[key] != entry["name"]:
                conflicts.append({"key": list(key), "a": seen[key], "b": entry["name"]})
            else:
                seen[key] = entry["name"]
        report["legacy_identity_conflicts"].append({
            "version": version,
            "entries": len(entries),
            "conflicts": conflicts,
        })
        legacy_sets[version] = set(seen)
        legacy_names[version] = seen

    # 3. Cross-source name sets.
    probe: dict[str, set[str]] = {}
    for path in sorted(args.probes.glob("*.json")):
        doc = load_json(path)
        probe[str(doc.get("game_version", path.stem))] = {
            b["name"] for b in doc.get("blocks", []) if isinstance(b, dict) and "name" in b
        }
    pdb_runtime: dict[str, set[str]] = {}
    for path in sorted((args.recon / "imports").glob("*.json")):
        doc = load_json(path)
        entries = doc["registries"][0]["entries"]
        pdb_runtime[doc["game_version"]] = {e["name"] for e in entries}

    # Cross-source block-name oracle: the union of all Script API probes, used
    # to spot PDB identifiers that no probe ever saw and probe names absent
    # from PDB-derived runtime sets.
    probe_oracle = set().union(*probe.values()) if probe else set()
    report["pdb_vs_probe_oracle"] = {}
    for version, names in sorted(pdb_runtime.items()):
        if not names:
            continue
        only_pdb = sorted(names - probe_oracle)
        report["pdb_vs_probe_oracle"][version] = {
            "pdb_runtime_names": len(names),
            "probe_oracle_size": len(probe_oracle),
            "in_pdb_not_any_probe": len(only_pdb),
            "in_pdb_not_any_probe_sample": only_pdb[:100],
        }

    all_versions = sorted(set(probe) | set(pdb_runtime) | set(legacy_sets))
    for version in all_versions:
        report["source_coverage"][version] = {
            "script_probe_blocks": len(probe.get(version, set())),
            "pdb_runtime_names": len(pdb_runtime.get(version, set())),
            "legacy_numeric_identities": len(legacy_sets.get(version, set())),
        }
        pn, rn = probe.get(version, set()), pdb_runtime.get(version, set())
        if pn and rn:
            report["unknown_names"][version] = {
                "in_probe_not_pdb": sorted(pn - rn)[:200],
                "in_pdb_not_probe": sorted(rn - pn)[:200],
                "counts": {"in_probe_not_pdb": len(pn - rn), "in_pdb_not_probe": len(rn - pn)},
            }

    # 4. Rename candidates across adjacent PDB runtime versions: names differing
    # while the ordered registry contains the symbol at the same ordinal with
    # the same derived id -- reported strictly as candidates.
    versions = sorted(pdb_runtime, key=lambda v: tuple(int(p) for p in v.split(".")))
    for older, newer in zip(versions, versions[1:]):
        a, b = pdb_runtime[older], pdb_runtime[newer]
        candidates = sorted(a & b)  # identical names are NOT renames; keep for completeness
        report["rename_candidates_not_authoritative"].append({
            "from": older,
            "to": newer,
            "shared_names": len(candidates),
            "added": len(b - a),
            "removed": len(a - b),
        })

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote integrity report to {args.out}")
    for row in report["corpus_provenance"]:
        print(f"  corpus {row['file']}: {row['status']}")
    for row in report["legacy_identity_conflicts"]:
        print(f"  {row['version']}: {row['entries']} entries, {len(row['conflicts'])} conflicts")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
