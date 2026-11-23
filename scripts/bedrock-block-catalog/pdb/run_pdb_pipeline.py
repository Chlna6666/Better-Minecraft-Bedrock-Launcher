#!/usr/bin/env python3
"""Run the full local BDS PDB evidence pipeline in one command.

Stages (all read-only with respect to the source repository catalog):
  1. verify + extract BDS EXE/PDB archives into target/bds-extracted/<version>/
  2. verify PE/PDB CodeView identity for every extracted version
  3. extract ordered registry symbols and byte-validated identifiers
  4. build stageable block_type_runtime imports for exact matches
  5. build the evidence report and cross-version name-change report

Dependency versions are recorded in the evidence directory so a result can be
reproduced or attributed to a tool bug later.
"""
from __future__ import annotations

import argparse
import json
import platform
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]


def run(label: str, *args: str) -> None:
    print(f"\n=== {label} ===", flush=True)
    subprocess.run([sys.executable, str(HERE / args[0]), *args[1:]], check=True)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--downloads", type=Path, required=True)
    ap.add_argument("--metadata", type=Path, required=True)
    ap.add_argument("--extracted", type=Path, required=True)
    ap.add_argument("--recon", type=Path, required=True)
    ap.add_argument("--probes", type=Path, default=REPO / "crates/bedrock-world/data/block-catalog/probes")
    ap.add_argument("--skip-download-verify", action="store_true")
    ap.add_argument("--legacy-id-map", type=Path,
                    help="PocketMine block_legacy_id_map.json (enables legacy ID imports)")
    ap.add_argument("--legacy-tables", nargs="*", default=[],
                    help="id_meta_to_nbt/<ver>.bin files; each filename supplies its dataset version")
    args = ap.parse_args()

    args.recon.mkdir(parents=True, exist_ok=True)
    (args.recon / "imports").mkdir(parents=True, exist_ok=True)
    (args.recon / "legacy-imports").mkdir(parents=True, exist_ok=True)
    if not args.skip_download_verify:
        run("extract & hash verify", "extract_bds.py",
            "--downloads", str(args.downloads),
            "--extracted", str(args.extracted),
            "--metadata", str(args.metadata))

    run("CodeView match", "verify_pdb_match.py",
        "--extracted", str(args.extracted),
        "--out", str(args.recon / "pdb-match.json"))

    manifest = json.loads((args.extracted / "manifest.json").read_text(encoding="utf-8"))
    for entry in manifest:
        version = entry["game_version"]
        pdb = args.extracted / version / "bedrock_server.pdb"
        exe = args.extracted / version / "bedrock_server.exe"
        if not pdb.is_file():
            print(f"[{version}] no PDB shipped; skipping symbol extraction", flush=True)
            continue
        # Legacy MSVC PDB with a TPI/DBI format pdbparse cannot decode: use the
        # name-only raw scanner instead of the stream-based extractor.
        raw_only = version == "1.6.0.15"
        if raw_only:
            run(f"{version} raw registry names", "legacy_raw_symbols.py",
                "--pdb", str(pdb), "--out", str(args.recon / f"vbt2-{version}.json"))
        else:
            run(f"{version} registry symbols", "vanilla_block_types.py",
                "--pdb", str(pdb), "--out", str(args.recon / f"vbt2-{version}.json"))
            run(f"{version} identifiers", "extract_identifiers.py",
                "--pdb", str(pdb), "--exe", str(exe),
                "--out", str(args.recon / f"ids-{version}.json"))

    matches = json.loads((args.recon / "pdb-match.json").read_text(encoding="utf-8"))
    for match in matches:
        version = match["game_version"]
        if match.get("verdict") != "matched":
            continue
        vbt_path = args.recon / f"vbt2-{version}.json"
        ids_path = args.recon / f"ids-{version}.json"
        if not vbt_path.is_file() or not ids_path.is_file():
            continue  # name-only or missing extraction: no ordinal-safe import
        run(f"{version} import", "build_import.py",
            "--recon", str(args.recon), "--version", version,
            "--probes", str(args.probes),
            "--out", str(args.recon / "imports" / f"{version}.json"))

    if args.legacy_id_map:
        # Legacy numeric IDs come from the authoritative PocketMine dataset,
        # NOT from PDB ordinals. Datasets only exist for 1.9.0 and 1.12.0.
        table_versions = {"1.9.0": "1.9.0.15", "1.12.0": "1.12.0.28"}
        for table in args.legacy_tables:
            source_version = table.stem.replace("id_meta_to_nbt_", "")
            game_version = table_versions.get(source_version)
            if not game_version:
                print(f"[legacy] no game-version mapping for {source_version}; skipping", flush=True)
                continue
            run(f"{game_version} legacy IDs", "import_legacy_ids.py",
                "--table", str(table), "--id-map", str(args.legacy_id_map),
                "--version", game_version, "--source-version", source_version,
                "--out", str(args.recon / "legacy-imports" / f"{game_version}.json"))

    run("evidence report", "build_report.py",
        "--recon", str(args.recon),
        "--out", str(args.recon / "pdb-evidence-report.json"))
    run("cross-version names", "cross_validate.py",
        "--recon", str(args.recon), "--probes", str(args.probes),
        "--out", str(args.recon / "cross-validation.json"))
    run("cross-source integrity", "integrity_check.py",
        "--recon", str(args.recon), "--probes", str(args.probes),
        "--out", str(args.recon / "integrity-report.json"))

    deps = {"python": sys.version.split()[0], "platform": platform.platform()}
    for module in ("pdbparse", "pefile", "construct", "jsonschema"):
        try:
            mod = __import__(module)
            deps[module] = getattr(mod, "__version__", "installed")
        except ImportError:
            deps[module] = "MISSING"
    (args.recon / "tool-versions.json").write_text(
        json.dumps(deps, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"\npipeline complete; tool versions in {args.recon / 'tool-versions.json'}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except subprocess.CalledProcessError as error:
        print("pipeline stage failed:", error, file=sys.stderr)
        raise SystemExit(1)
