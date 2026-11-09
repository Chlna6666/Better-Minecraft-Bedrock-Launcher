#!/usr/bin/env python3
"""Pack a BSC Geometry Pool v3 directory or ZIP as a deterministic embedded ZIP.

Only v3 is supported. One geometry_pool.bin replaces all hash-addressed shards.
Usage: python scripts/pack_bsc_model_library.py export/1.26.60.5 generated/bsc_models.bin
"""
from __future__ import annotations
import argparse
import json
import struct
import sys
import zipfile
from pathlib import Path

ROOT_FILES = ("manifest.json", "blocks.json", "entities.json", "materials.json",
              "coverage.json", "geometry_catalog.json", "geometry_pool.bin")
KINDS = ("geometry", "appearance", "materials", "models")


def inspect_pool(data: bytes) -> tuple[int, int, int, int]:
    if len(data) < 28 or data[:8] != b"BSCPOOL3":
        raise ValueError("Geometry Pool v3 magic/size mismatch")
    version, *counts = struct.unpack_from("<IIIII", data, 8)
    if version != 3 or any(c > 1_000_000 for c in counts):
        raise ValueError("unsupported Geometry Pool v3 header")
    end = 28 + 12 * sum(counts)
    if end > len(data):
        raise ValueError("truncated Geometry Pool v3 index")
    offset = end
    cursor = 28
    for _ in range(sum(counts)):
        at, length = struct.unpack_from("<QI", data, cursor)
        if at != offset or length > 64 * 1024 * 1024 or at + length > len(data):
            raise ValueError("invalid Geometry Pool v3 offset or length")
        offset = at + length
        cursor += 12
    if offset != len(data):
        raise ValueError("Geometry Pool v3 trailing bytes")
    return tuple(counts)


def verify_ref(kind: str, value: object, counts: tuple[int, int, int, int]) -> None:
    maximum = counts[KINDS.index(kind)]
    if type(value) is not int or not 0 <= value < maximum:
        raise ValueError(f"invalid v3 {kind} reference: {value!r}, max={maximum}")


def pack(source: Path, target: Path) -> None:
    archive = zipfile.ZipFile(source) if source.is_file() else None
    try:
        if archive:
            names = set(archive.namelist())
            prefixes = [name[:-len("manifest.json")] for name in names
                        if name.endswith("manifest.json")
                        and name[:-len("manifest.json")] + "geometry_pool.bin" in names
                        and name[:-len("manifest.json")] + "blocks.json" in names]
            if len(prefixes) != 1:
                raise ValueError("expected exactly one v3 BSC export")
            prefix = prefixes[0]
            def read(name: str) -> bytes:
                return archive.read(prefix + name)
        else:
            def read(name: str) -> bytes:
                return (source / name).read_bytes()

        manifest = json.loads(read("manifest.json"))
        if (manifest.get("format") != "BedrockSceneCaptureModelLibrary"
                or manifest.get("shared_geometry_pool_schema") != 3
                or manifest.get("legacy_meshes_retained") is not False):
            raise ValueError("BSC Geometry Pool v3 exclusive export required")
        counts = inspect_pool(read("geometry_pool.bin"))
        blocks = json.loads(read("blocks.json"))
        entities = json.loads(read("entities.json"))
        models = json.loads(read("geometry_catalog.json"))
        for block in blocks["blocks"]:
            mid = block.get("shared_material_ref")
            if mid is not None:
                verify_ref("materials", mid, counts)
            for variant in block.get("variants", []):
                geo = variant.get("shared_geometry_refs", [])
                app = variant.get("shared_appearance_refs", [])
                if len(geo) != len(app):
                    raise ValueError("mismatched v3 geometry/appearance references")
                for gid, aid in zip(geo, app):
                    verify_ref("geometry", gid, counts)
                    verify_ref("appearance", aid, counts)
        for record in [*entities.get("model_parts", []), *models.get("models", [])]:
            verify_ref("models", record["model_ref"], counts)

        target.parent.mkdir(parents=True, exist_ok=True)
        temp = target.with_suffix(target.suffix + ".tmp")
        try:
            with zipfile.ZipFile(temp, "w", compression=zipfile.ZIP_DEFLATED,
                                 compresslevel=6, allowZip64=False) as dst:
                for name in sorted(ROOT_FILES):
                    data = read(name)
                    if name.endswith(".json"):
                        doc = json.loads(data)
                        data = json.dumps(doc, ensure_ascii=False, separators=(",", ":"),
                                          allow_nan=False).encode("utf-8")
                    entry = zipfile.ZipInfo(name, (1980, 1, 1, 0, 0, 0))
                    entry.compress_type = zipfile.ZIP_DEFLATED
                    entry.external_attr = 0o644 << 16
                    dst.writestr(entry, data, compress_type=zipfile.ZIP_DEFLATED,
                                 compresslevel=6)
            with zipfile.ZipFile(temp) as dst:
                if dst.testzip() is not None or set(dst.namelist()) != set(ROOT_FILES):
                    raise ValueError("v3 embedded ZIP failed integrity check")
            temp.replace(target)
            print(f"BSC v3 packed: {len(ROOT_FILES)} entries, "
                  f"{target.stat().st_size:,} bytes, version={manifest['target_game_version']}",
                  file=sys.stderr)
            print("Pool entries: " + ", ".join(
                f"{name}={count}" for name, count in zip(KINDS, counts)), file=sys.stderr)
            if not manifest.get("complete"):
                print("WARNING: partial capture, missing models not synthesized", file=sys.stderr)
        finally:
            temp.unlink(missing_ok=True)
    finally:
        if archive:
            archive.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    parser.add_argument("target", type=Path)
    options = parser.parse_args()
    pack(options.source, options.target)
