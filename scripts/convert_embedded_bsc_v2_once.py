#!/usr/bin/env python3
"""ONE-TIME migration of the committed v2 embedded ZIP into v3 binary pool.

This utility is deliberately deleted by the migration CI after writing the
converted artifact. Neither BedrockSceneCapture nor BMCBL reads v2 at runtime.
"""
import json
import math
import struct
import zipfile
from pathlib import Path

ARCHIVE = Path("crates/bedrock-block-model/generated/bsc_models.bin")
KINDS = ("geometry","appearance","materials","models")
NONE = 0xffffffff

def uint(v):
    if type(v) is not int or not 0 <= v <= 0xffffffff:
        raise ValueError(f"invalid u32 {v!r}")
    return v

def p32(*values):
    return struct.pack("<"+"I"*len(values), *(uint(v) for v in values))

def scalar(v):
    n = float(v)
    if not math.isfinite(n):
        raise ValueError("nonfinite float in legacy geometry")
    return struct.pack("<f",n)

def jbytes(value):
    return (json.dumps(value,ensure_ascii=False,separators=(",",":"),allow_nan=False)+"\n").encode()

def convert() -> None:
    if not ARCHIVE.is_file():
        raise ValueError(f"missing old embedded archive: {ARCHIVE}")
    with zipfile.ZipFile(ARCHIVE) as old:
        paths = set(old.namelist())
        def read(name):
            if name not in paths:
                raise ValueError(f"missing v2 entry: {name}")
            return old.read(name)
        manifest = json.loads(read("manifest.json"))
        if manifest.get("shared_geometry_pool_schema") == 3:
            print("Embedded library is already Geometry Pool v3")
            return
        if manifest.get("shared_geometry_pool_schema") != 2:
            raise ValueError("expected previously shipped v2 embedded model ZIP")
        blocks = json.loads(read("blocks.json"))
        entities = json.loads(read("entities.json"))
        catalog = json.loads(read("geometry_catalog.json"))
        rest = {"materials.json":json.loads(read("materials.json"))}
        rest["coverage.json"] = (json.loads(read("coverage.json"))
            if "coverage.json" in paths else
            {"schema":1,"counts":{},"coverage_report_available":False,
             "reason":"original embedded v2 ZIP omitted coverage.json"})

        refs = {kind:set() for kind in KINDS}
        for block in blocks["blocks"]:
            material = block.get("shared_material_ref")
            if material is not None:
                refs["materials"].add(material)
            for sample in block.get("variants", []):
                geos = sample.get("shared_geometry_refs", [])
                apps = sample.get("shared_appearance_refs", [])
                if len(geos) != len(apps):
                    raise ValueError("legacy capture has mismatched per-mesh references")
                refs["geometry"].update(geos)
                refs["appearance"].update(apps)
        for actor in entities.get("model_parts", []):
            refs["models"].add(actor["model_ref"])
        # The older BMCBL packer omitted unreferenced static model definitions.
        # Do not invent missing models; retain resolvable catalog entries only.
        static = []
        for item in catalog.get("models", []):
            h = item.get("model_ref")
            if f"geometry_pool/models/{h}.json" in paths:
                refs["models"].add(h)
                static.append(item)
        catalog["models"] = static
        ids = {kind:{h:i for i,h in enumerate(sorted(refs[kind]))} for kind in KINDS}
        groups = {kind:[] for kind in KINDS}

        for kind in KINDS:
            for h in sorted(refs[kind]):
                source = f"geometry_pool/{kind}/{h}.json"
                raw = read(source)
                if kind in ("materials","models"):
                    groups[kind].append(jbytes(json.loads(raw)))
                    continue
                doc = json.loads(raw)
                if doc.get("schema") != 2:
                    raise ValueError(f"unexpected legacy {kind} schema")
                if kind == "geometry":
                    points = doc["vertices"]
                    tris = doc["triangles"]
                    out = bytearray(p32(len(points),len(tris)))
                    for point in points:
                        if len(point)!=3:
                            raise ValueError("invalid legacy vertex")
                        for coordinate in point:
                            out.extend(scalar(coordinate))
                    for tri in tris:
                        if len(tri)!=3 or any(uint(i)>=len(points) for i in tri):
                            raise ValueError("invalid legacy triangle")
                        out.extend(p32(*tri))
                    groups[kind].append(bytes(out))
                else:
                    geo = ids["geometry"][doc["geometry_ref"]]
                    source_kind = {
                        "registry_visual_bounds":0,
                        "registry_cross_planes":1,
                        "native_world_tessellation":2,
                    }.get(doc.get("geometry_source"))
                    if source_kind is None:
                        raise ValueError("invalid legacy appearance source")
                    corners = doc["corner_attributes"]
                    slots = doc["triangle_texture_slots"]
                    variants = doc["triangle_texture_variant_indices"]
                    if len(corners)!=len(slots)*3 or len(slots)!=len(variants):
                        raise ValueError("legacy appearance corner/triangle count mismatch")
                    flags = (1 if doc.get("uv_complete") else 0) | (
                        2 if doc.get("normals_reconstructed") else 0)
                    out = bytearray(p32(geo, doc.get("render_layer",0), source_kind,flags,len(slots)))
                    for corner in corners:
                        if len(corner)!=6:
                            raise ValueError("legacy corner attribute count mismatch")
                        for component in corner[:5]:
                            out.extend(scalar(component))
                        out.extend(p32(corner[5]))
                    for slot, variant in zip(slots,variants):
                        if variant is None:
                            v = (NONE,NONE)
                        elif len(variant)==2:
                            v = (uint(variant[0]),uint(variant[1]))
                        else:
                            raise ValueError("legacy atlas variant is not a pair")
                        out.extend(p32(NONE if slot is None else uint(slot),*v))
                    groups[kind].append(bytes(out))

        for block in blocks["blocks"]:
            material = block.get("shared_material_ref")
            if material is not None:
                block["shared_material_ref"] = ids["materials"][material]
            for sample in block.get("variants", []):
                sample["shared_geometry_refs"] = [
                    ids["geometry"][x] for x in sample.get("shared_geometry_refs",[])
                ]
                sample["shared_appearance_refs"] = [
                    ids["appearance"][x] for x in sample.get("shared_appearance_refs",[])
                ]
        for actor in entities.get("model_parts", []):
            actor["model_ref"] = ids["models"][actor["model_ref"]]
        for item in catalog.get("models", []):
            item["model_ref"] = ids["models"][item["model_ref"]]
        manifest.update(shared_geometry_pool_schema=3,
                        shared_geometry_count=len(groups["geometry"]),
                        shared_appearance_count=len(groups["appearance"]),
                        shared_material_count=len(groups["materials"]),
                        static_geometry_models=len(catalog["models"]),
                        legacy_meshes_retained=False)
        flat = [group for kind in KINDS for group in groups[kind]]
        header = bytearray(b"BSCPOOL3")
        header.extend(p32(3,*(len(groups[k]) for k in KINDS)))
        offset = 28 + 12*len(flat)
        for entry in flat:
            header.extend(struct.pack("<QI",offset,len(entry)))
            offset += len(entry)
        blob = bytes(header) + b"".join(flat)
        docs = {"manifest.json":manifest,"blocks.json":blocks,
                "entities.json":entities,"geometry_catalog.json":catalog,**rest}
        temp = ARCHIVE.with_suffix(".v3.tmp")
        try:
            with zipfile.ZipFile(temp,"w",compression=zipfile.ZIP_DEFLATED,
                                 compresslevel=6,allowZip64=False) as dst:
                for name in sorted([*docs,"geometry_pool.bin"]):
                    payload = blob if name=="geometry_pool.bin" else jbytes(docs[name])
                    item = zipfile.ZipInfo(name,(1980,1,1,0,0,0))
                    item.compress_type=zipfile.ZIP_DEFLATED
                    item.external_attr=0o644<<16
                    dst.writestr(item,payload,compress_type=zipfile.ZIP_DEFLATED,
                                 compresslevel=6)
            with zipfile.ZipFile(temp) as check:
                if check.testzip() is not None or check.read("geometry_pool.bin")[:8] != b"BSCPOOL3":
                    raise ValueError("converted v3 embedded ZIP failed integrity check")
            temp.replace(ARCHIVE)
        finally:
            temp.unlink(missing_ok=True)
        print(f"Migrated embedded BSC {manifest.get('target_game_version')}: "
              f"{len(blob)} pool bytes, {ARCHIVE.stat().st_size} ZIP bytes, "
              + ", ".join(f"{kind}={len(groups[kind])}" for kind in KINDS))

if __name__=="__main__":
    convert()
