#!/usr/bin/env python3
"""Standalone Geometry Pool v3 packaging regression tests."""
import json
import struct
import tempfile
import zipfile
from pathlib import Path
from pack_bsc_model_library import inspect_pool, pack

def fixture(root: Path):
    payloads = [b"geometry", b"appearance", b'{"schema":2,"texture_bindings":[]}', b"model"]
    start = 28 + len(payloads) * 12
    directory = bytearray()
    for data in payloads:
        directory.extend(struct.pack("<QI", start, len(data)))
        start += len(data)
    (root / "geometry_pool.bin").write_bytes(
        b"BSCPOOL3" + struct.pack("<5I", 3,1,1,1,1)
        + directory + b"".join(payloads))
    docs = {
        "manifest.json": {"format":"BedrockSceneCaptureModelLibrary",
            "shared_geometry_pool_schema":3,"legacy_meshes_retained":False,
            "target_game_version":"1.26.60.5"},
        "blocks.json": {"blocks":[{"shared_material_ref":0,
            "variants":[{"shared_geometry_refs":[0],"shared_appearance_refs":[0]}]}]},
        "entities.json": {"model_parts":[{"model_ref":0}]},
        "geometry_catalog.json": {"models":[{"model_ref":0}]},
        "materials.json": {"materials":[]},
        "coverage.json": {"counts":{}},
    }
    for name, doc in docs.items():
        (root / name).write_text(json.dumps(doc), encoding="utf-8")

def run():
    with tempfile.TemporaryDirectory() as folder:
        root = Path(folder)
        fixture(root)
        assert inspect_pool((root / "geometry_pool.bin").read_bytes()) == (1,1,1,1)
        archive = root / "embedded.bin"
        pack(root,archive)
        with zipfile.ZipFile(archive) as zipped:
            assert zipped.namelist() == sorted((
                "manifest.json","blocks.json","entities.json","materials.json",
                "coverage.json","geometry_catalog.json","geometry_pool.bin"))
            assert zipped.read("geometry_pool.bin").startswith(b"BSCPOOL3")
        before = archive.read_bytes()
        pack(root,archive)
        assert archive.read_bytes() == before
        doc=json.loads((root/"blocks.json").read_text())
        doc["blocks"][0]["variants"][0]["shared_geometry_refs"]=[9]
        (root/"blocks.json").write_text(json.dumps(doc))
        try:
            pack(root,archive)
        except ValueError:
            pass
        else:
            raise AssertionError("out-of-range v3 reference accepted")
        assert archive.read_bytes() == before
    print("Geometry Pool v3 packer: deterministic ZIP and invalid-reference tests passed")

if __name__=="__main__":
    run()
