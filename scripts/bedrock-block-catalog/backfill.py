#!/usr/bin/env python3
"""Run selected historical official BDS builds, retaining honest per-version results.

One runner processes builds serially. Only complete Structure/NBT captures
enter the binary catalog; API-only probes and failures are tracked separately.
"""
from __future__ import annotations
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import traceback
from typing import Any

import run_bds

DEFAULT_VERSIONS = (
    "1.19.70.02",
    "1.20.80.05",
    "1.21.20.03",
    "1.21.80.3",
    "1.21.100.7",
)


def run_versions(versions: list[str], catalog: Path, work_root: Path,
                 compiler: Path, timeout: int, force: bool = False) -> list[dict[str, Any]]:
    if not compiler.is_file():
        raise FileNotFoundError("build the bedrock-block-catalog release CLI first")
    work_root.mkdir(parents=True, exist_ok=True)
    catalog.mkdir(parents=True, exist_ok=True)
    outcomes = []
    for version in versions:
        print(f"\n===== Historical BDS {version} =====", flush=True)
        record: dict[str, Any] = {
            "game_version": version,
            "expected_api": run_bds.script_profile(version),
            "archive_sha1_reference": run_bds.HISTORICAL_SHA1[version],
        }
        existing = catalog / "index.json"
        if existing.is_file() and not force:
            index = json.loads(existing.read_text(encoding="utf-8"))
            if version in index.get("versions", {}):
                record["status"] = "already_captured"
                outcomes.append(record)
                continue
        base = work_root / version
        base.mkdir(parents=True, exist_ok=True)
        metadata = base / "capture.json"
        log = base / "server.log"
        try:
            job = subprocess.run(
                [sys.executable, str(Path(run_bds.__file__).resolve()), "--version", version,
                 "--workdir", str(base / "server"), "--metadata", str(metadata),
                 "--log", str(log), "--timeout", str(timeout)],
                check=False, timeout=timeout + 360,
                env={k: v for k, v in os.environ.items() if k != "GITHUB_OUTPUT"},
            )
            if job.returncode:
                raise RuntimeError(f"BDS runner exited {job.returncode}")
            capture = json.loads(metadata.read_text(encoding="utf-8"))
            record["capture_method"] = capture["capture_method"]
            record["bds_sha256"] = capture["bds_sha256"]
            record["statistics"] = capture["stats"]
            if capture["capture_method"] == "script_api_probe":
                probe = metadata.with_name("bmcbl-api-probe.json")
                if not probe.is_file():
                    raise RuntimeError("probe output missing despite completed capture")
                target = catalog / "probes" / (version + ".json")
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(probe.read_bytes())
                record["status"] = "partial_probe"
                record["probe_file"] = "probes/" + version + ".json"
            elif capture["capture_method"] == "script_api_structure":
                subprocess.run(
                    [str(compiler), "compile", "--world",
                     str(base / "server" / "worlds" / run_bds.WORLD_NAME),
                     "--metadata", str(metadata), "--log", str(log), "--out", str(catalog)],
                    check=True, timeout=180,
                )
                record["status"] = "complete_structure_capture"
            else:
                raise ValueError("unexpected capture method")
        except (Exception, KeyboardInterrupt) as error:
            record["status"] = "capture_failed"
            record["error_type"] = type(error).__name__
            record["error"] = str(error)[:600]
            if log.is_file():
                # Only store a short, redacted summary of the failure in Git;
                # full logs are uploaded as Actions diagnostics artifacts.
                lines = log.read_text(encoding="utf-8", errors="replace").splitlines()
                record["log_tail"] = [
                    line[:600] for line in lines[-12:]
                    if "BMCBL-DUMP-" in line or "error" in line.lower() or "not found" in line.lower()
                ][-8:]
        outcomes.append(record)
        print(json.dumps(record, ensure_ascii=False)[:3500], flush=True)
    report_path = catalog / "historical-capture-report.json"
    report_path.write_text(json.dumps({
        "schema_version": 1,
        "documentation": "docs/BEDROCK_BLOCK_DATA_MAINTENANCE.md",
        "results": outcomes,
    }, indent=2, ensure_ascii=False, sort_keys=True) + "\n", encoding="utf-8")
    return outcomes


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--versions", default=" ".join(DEFAULT_VERSIONS))
    parser.add_argument("--catalog", type=Path,
                        default=Path("crates/bedrock-world/data/block-catalog"))
    parser.add_argument("--work-root", type=Path, required=True)
    parser.add_argument("--compiler", type=Path,
                        default=Path("target/release/bedrock-block-catalog"))
    parser.add_argument("--timeout", type=int, default=420)
    parser.add_argument("--force", action="store_true")
    args = parser.parse_args()
    versions = args.versions.replace(",", " ").split()
    if len(versions) < 1 or len(versions) > 8 or len(set(versions)) != len(versions):
        parser.error("supply between 1 and 8 distinct exact versions")
    for version in versions:
        run_bds.script_profile(version)
        if version not in run_bds.HISTORICAL_SHA1:
            parser.error("historical build is not SHA-1 pinned: " + version)
    results = run_versions(versions, args.catalog, args.work_root, args.compiler,
                           args.timeout, args.force)
    print("Historical BDS result summary:", flush=True)
    for item in results:
        print(f"  {item['game_version']}: {item['status']}", flush=True)


if __name__ == "__main__":
    main()
