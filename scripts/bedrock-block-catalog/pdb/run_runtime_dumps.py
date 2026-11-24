#!/usr/bin/env python3
"""Run BDS per version and dump the live block registry for each.

For every version that has a PDB-derived `mBlockLookupMap` address, this:
  1. extracts the full official BDS archive into a disposable per-version dir,
  2. starts bedrock_server.exe,
  3. waits for it to finish loading,
  4. runs runtime_dump.py against the live process,
  5. stops the server.

Never commits anything; outputs a names-only JSON per version under --out-dir.
Versions whose registry uses an unsupported container layout are reported, not
silently skipped.
"""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
SERVER_PROPERTIES = """level-name=bmcbl_runtime
server-port=19171
server-portv6=19172
online-mode=false
allow-list=false
max-players=1
view-distance=4
tick-distance=4
difficulty=peaceful
content-log-console-output-enabled=true
"""


def stop_existing_servers() -> None:
    subprocess.run(
        ["powershell.exe", "-NoProfile", "-Command",
         "Get-Process bedrock_server -ErrorAction SilentlyContinue | Stop-Process -Force"],
        capture_output=True)


def wait_for_server(timeout: float = 90.0) -> int | None:
    deadline = time.time() + timeout
    while time.time() < deadline:
        result = subprocess.run(
            ["powershell.exe", "-NoProfile", "-Command",
             "Get-Process bedrock_server -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Id"],
            capture_output=True, text=True)
        pids = [int(line) for line in result.stdout.split() if line.strip().isdigit()]
        if pids:
            return pids[0]
        time.sleep(1)
    return None


def map_layout(version: str) -> str:
    """Registry map container by build era.

    1.9-1.14: unordered_map<basic_string, SharedPtr>
    1.16:     std::map<basic_string, SharedPtr>  (plain-string key)
    1.18+:    std::map<HashedString, SharedPtr>
    """
    parts = tuple(int(p) for p in version.split("."))
    if parts[:2] < (1, 16):
        return "unordered_map"
    if parts[:2] < (1, 18):
        return "std_map_string"
    return "std_map"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--symtable", type=Path, required=True,
                    help="registry-symbols.json from probe_registry_symbols.py")
    ap.add_argument("--downloads", type=Path, required=True)
    ap.add_argument("--runtime", type=Path, required=True)
    ap.add_argument("--out-dir", type=Path, required=True)
    ap.add_argument("--only", nargs="*", help="restrict to these versions")
    args = ap.parse_args()

    symbols = {row["version"]: row for row in json.loads(args.symtable.read_text(encoding="utf-8"))}
    args.out_dir.mkdir(parents=True, exist_ok=True)
    results = []

    for version, row in sorted(symbols.items()):
        if args.only and version not in args.only:
            continue
        if row.get("status") != "ok":
            print(f"{version}: skip ({row.get('status')})", flush=True)
            results.append({"version": version, "status": row.get("status")})
            continue
        archive = args.downloads / f"bedrock-server-{version}.zip"
        if not archive.is_file():
            print(f"{version}: no archive {archive.name}", flush=True)
            results.append({"version": version, "status": "no_archive"})
            continue

        workdir = args.runtime / version
        workdir.mkdir(parents=True, exist_ok=True)
        if not (workdir / "bedrock_server.exe").is_file():
            print(f"{version}: extracting archive", flush=True)
            with zipfile.ZipFile(archive) as zf:
                zf.extractall(workdir)
        (workdir / "server.properties").write_text(SERVER_PROPERTIES, encoding="utf-8")

        stop_existing_servers()
        print(f"{version}: starting server", flush=True)
        log = workdir / "runtime-dump-server.log"
        with log.open("w", encoding="utf-8") as handle:
            subprocess.Popen([str(workdir / "bedrock_server.exe")], cwd=workdir,
                             stdout=handle, stderr=subprocess.STDOUT)
        pid = wait_for_server()
        if pid is None:
            print(f"{version}: server did not start", flush=True)
            results.append({"version": version, "status": "start_failed"})
            continue
        # Let world/chunk generation finish.
        time.sleep(8)

        out = args.out_dir / f"{version}.json"
        dump = subprocess.run(
            [sys.executable, str(HERE / "runtime_dump.py"),
             "--map-rva", row["map_rva"], "--pid", str(pid),
             "--layout", map_layout(version),
             "--names-only", "--out", str(out)],
            capture_output=True, text=True)
        stop_existing_servers()
        if dump.returncode == 0:
            doc = json.loads(out.read_text(encoding="utf-8"))
            print(f"{version}: {doc['named_blocks']}/{doc['declared_map_size']} names", flush=True)
            results.append({
                "version": version, "status": "ok",
                "named": doc["named_blocks"], "declared": doc["declared_map_size"],
            })
        else:
            print(f"{version}: dump failed: {dump.stderr.strip()[:200]}", flush=True)
            results.append({"version": version, "status": "dump_failed",
                            "error": dump.stderr.strip()[:400]})

    (args.out_dir / "run-summary.json").write_text(
        json.dumps(results, indent=2) + "\n", encoding="utf-8")
    print("summary:", json.dumps(results))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
