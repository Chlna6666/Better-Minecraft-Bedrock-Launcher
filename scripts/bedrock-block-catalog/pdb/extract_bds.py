#!/usr/bin/env python3
"""Extract official BDS PE/PDB pairs into one directory per game version.

Verifies the archive SHA-1 against Bedrock-OSS/BDS-Versions metadata and
records SHA-256 for both the archive and extracted binaries. Only official
Mojang CDN archives are used; nothing here downloads or writes catalog data.
"""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import sys
import zipfile
from pathlib import Path


def digest(path: Path, algorithm: str) -> str:
    h = hashlib.new(algorithm)
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--downloads", type=Path, required=True)
    ap.add_argument("--extracted", type=Path, required=True)
    ap.add_argument("--metadata", type=Path, required=True, help="Directory with win-<version>.json files")
    args = ap.parse_args()

    args.extracted.mkdir(parents=True, exist_ok=True)
    manifest = []
    for archive in sorted(args.downloads.glob("bedrock-server-*.zip")):
        version = archive.stem.removeprefix("bedrock-server-")
        metadata_path = args.metadata / f"win-{version}.json"
        if not metadata_path.is_file():
            print(f"[{version}] SKIP: no pinned metadata", file=sys.stderr)
            continue
        raw = json.loads(metadata_path.read_text(encoding="utf-8"))["content"]
        meta = json.loads(base64.b64decode(raw))
        sha1 = digest(archive, "sha1")
        expected_sha1 = meta.get("sha1")
        if expected_sha1 and sha1.lower() != str(expected_sha1).lower():
            print(f"[{version}] SKIP: SHA-1 mismatch {sha1} != {expected_sha1}", file=sys.stderr)
            continue
        if archive.stat().st_size != meta.get("size_in_bytes"):
            print(f"[{version}] SKIP: size mismatch", file=sys.stderr)
            continue

        target = args.extracted / version
        target.mkdir(parents=True, exist_ok=True)
        entry = {
            "game_version": version,
            "archive_sha1": sha1,
            "archive_sha1_verified": bool(expected_sha1),
            "archive_sha256": digest(archive, "sha256"),
            "archive_size": archive.stat().st_size,
            "source_url": meta.get("download_url"),
            "files": [],
        }
        with zipfile.ZipFile(archive) as zf:
            for name in ("bedrock_server.exe", "bedrock_server.pdb"):
                if name not in zf.namelist():
                    continue
                zf.extract(name, target)
                extracted = target / name
                entry["files"].append({
                    "name": name,
                    "size": extracted.stat().st_size,
                    "sha256": digest(extracted, "sha256"),
                })
        manifest.append(entry)
        print(f"[{version}] ok: {[f['name'] for f in entry['files']]}")

    (args.extracted / "manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote {len(manifest)} entries to {args.extracted / 'manifest.json'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
