#!/usr/bin/env python3
"""Pack a BedrockSceneCapture Geometry Pool v2 ZIP/directory as an embedded .bin.

The .bin is a deterministic ZIP container. BMCBL holds the ZIP index in memory
and only decompresses referenced geometry/appearance files upon first use.

Usage:
    python scripts/pack_bsc_model_library.py 1.26.51.1.zip crates/bedrock-block-model/generated/bsc_models.bin
"""
import argparse
import json
import re
import sys
import zipfile
from pathlib import Path

HASH = re.compile(r"[0-9a-f]{64}\Z")
ROOT_FILES = (
    "manifest.json", "blocks.json", "entities.json", "materials.json",
    "geometry_pool.json", "geometry_catalog.json",
)


def pack(source: Path, target: Path) -> None:
    archive = zipfile.ZipFile(source) if source.is_file() else None
    if archive:
        names = set(archive.namelist())
        roots = [p[:-len("manifest.json")] for p in names
                 if p.endswith("manifest.json") and
                 p[:-len("manifest.json")] + "blocks.json" in names]
        if len(roots) != 1:
            raise ValueError("expected exactly one BSC model export in ZIP")
        prefix = roots[0]

        def read(name: str) -> bytes:
            return archive.read(prefix + name)
    else:
        def read(name: str) -> bytes:
            return (source / name).read_bytes()

    manifest = json.loads(read("manifest.json"))
    if (manifest.get("format") != "BedrockSceneCaptureModelLibrary"
            or manifest.get("shared_geometry_pool_schema") != 2):
        raise ValueError("expected BSC Geometry Pool v2 export")
    blocks = json.loads(read("blocks.json"))
    if not isinstance(blocks.get("blocks"), list):
        raise ValueError("invalid blocks.json")
    entities = json.loads(read("entities.json"))

    refs = {kind: set() for kind in ("geometry", "appearance", "materials", "models")}
    for block in blocks["blocks"]:
        ref = block.get("shared_material_ref")
        if ref:
            refs["materials"].add(ref)
        for variant in block.get("variants", []):
            for kind in ("geometry", "appearance"):
                refs[kind].update(variant.get(f"shared_{kind}_refs", []))
    for part in entities.get("model_parts", []):
        if part.get("model_ref"):
            refs["models"].add(part["model_ref"])

    paths = set(ROOT_FILES)
    for kind, hashes in refs.items():
        for hash_value in hashes:
            if not HASH.fullmatch(hash_value):
                raise ValueError(f"invalid {kind} hash: {hash_value!r}")
            paths.add(f"geometry_pool/{kind}/{hash_value}.json")

    target.parent.mkdir(parents=True, exist_ok=True)
    temp = target.with_suffix(target.suffix + ".tmp")
    try:
        with zipfile.ZipFile(temp, mode="w", compression=zipfile.ZIP_DEFLATED,
                             compresslevel=6, allowZip64=False) as result:
            for name in sorted(paths):
                try:
                    document = json.loads(read(name))
                except (FileNotFoundError, KeyError) as error:
                    raise ValueError(f"missing referenced data: {name}") from error
                data = json.dumps(
                    document, ensure_ascii=False, separators=(",", ":"), allow_nan=False
                ).encode("utf-8")
                entry = zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0))
                entry.compress_type = zipfile.ZIP_DEFLATED
                entry.external_attr = 0o644 << 16
                result.writestr(entry, data, compress_type=zipfile.ZIP_DEFLATED,
                                compresslevel=6)
        with zipfile.ZipFile(temp) as result:
            if result.testzip() is not None or len(result.namelist()) != len(paths):
                raise ValueError("packed BSC archive verification failed")
        temp.replace(target)
        print(f"BSC packed: {len(paths)} entries, {target.stat().st_size:,} bytes; "
              f"version={manifest['target_game_version']}", file=sys.stderr)
        print("Reference pool: " + ", ".join(
            f"{k}={len(v)}" for k, v in refs.items()), file=sys.stderr)
        if not manifest.get("complete"):
            print("WARNING: incomplete BSC capture; missing models not synthesized",
                  file=sys.stderr)
    finally:
        temp.unlink(missing_ok=True)
        if archive:
            archive.close()


if __name__ == "__main__":
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("source", type=Path)
    ap.add_argument("target", type=Path)
    args = ap.parse_args()
    pack(args.source, args.target)
