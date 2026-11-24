#!/usr/bin/env python3
"""Strict, content-addressed import of human-reviewed PDB/runtime block identities.

Never confuses legacy (id, meta), internal BlockType IDs, network palette
runtime IDs or persisted NBT version. Stdlib-only; no PDB parsing here.
"""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import zlib

# pdb_symbol_order stores only the declaration/address order of static PDB
# symbols. It is explicitly NOT a verified runtime type ID: use it as evidence
# only until a live registry or runtime observation proves an ID.
NAMESPACES = {"legacy_numeric_block": 1, "block_type_runtime": 2,
              "network_runtime": 3, "pdb_symbol_order": 4}
TYPES = {"Byte": 1, "Int": 3, "String": 8}
MAX_RECORDS = 300000
# Modern identifiers are lowercase. Historical legacy numeric IDs preserve the
# exact case used by the game (concretePowder, tripWire, seaLantern, ...).
MODERN_NAME_RE = re.compile(r"[a-z0-9_.-]+:[a-z0-9_./-]+")
LEGACY_NAME_RE = re.compile(r"[a-z0-9_.-]+:[a-zA-Z0-9_./-]+")


def require(condition: bool, reason: str) -> None:
    if not condition:
        raise ValueError(reason)


def digest64(value: object, field: str) -> str:
    require(isinstance(value, str) and re.fullmatch("[0-9a-fA-F]{64}", value) is not None,
            field + " must be a 64-digit SHA-256 hex string")
    return value.lower()


def natural(value: object, field: str) -> int:
    require(type(value) is int and 0 <= value <= 0xffffffff, field + " must be a u32")
    return value


def name_of(value: object, namespace: str | None = None) -> str:
    pattern = LEGACY_NAME_RE if namespace == "legacy_numeric_block" else MODERN_NAME_RE
    require(isinstance(value, str) and bool(pattern.fullmatch(value)),
            "block identifier must be an explicit namespaced ID"
            + (" (lowercase)" if namespace != "legacy_numeric_block" else " (historical case preserved)"))
    return value


def varint(value: int) -> bytes:
    out = bytearray()
    while value >= 128:
        out.append((value & 127) | 128)
        value >>= 7
    out.append(value)
    return bytes(out)


def text(value: str) -> bytes:
    raw = value.encode("utf-8")
    return varint(len(raw)) + raw


def encode_state(states: object, required: bool) -> bytes:
    if states is None:
        require(not required, "state registry requires explicit exact typed states")
        return varint(0)
    require(isinstance(states, dict), "states must be an object")
    require(len(states) <= 64, "too many BlockState properties")
    out = bytearray(varint(len(states)))
    for key in sorted(states):
        require(isinstance(key, str) and len(key) <= 128 and bool(key),
                "invalid property name")
        tag = states[key]
        require(isinstance(tag, dict) and set(tag) == {"type", "value"},
                "each state must be exactly {type,value}")
        kind = tag["type"]
        require(kind in TYPES, "only NBT Byte / Int / String are permitted")
        out.extend(text(key))
        out.append(TYPES[kind])
        value = tag["value"]
        if kind == "Byte":
            require(type(value) is int and -128 <= value <= 127, "NBT Byte out of range")
            out.extend(struct.pack("<b", value))
        elif kind == "Int":
            require(type(value) is int and -(2**31) <= value < 2**31, "NBT Int out of range")
            out.extend(struct.pack("<i", value))
        else:
            require(isinstance(value, str) and len(value) <= 65536,
                    "NBT String invalid or oversized")
            out.extend(text(value))
    return bytes(out)


def parse_registry(obj: dict, names: set[str] | None, allow_unknown: bool) -> tuple[str, str | None, bytes, int, bool]:
    require(isinstance(obj, dict), "registry must be an object")
    namespace = obj.get("namespace")
    require(namespace in NAMESPACES, "unsupported ID namespace")
    palette = obj.get("palette_fingerprint")
    if namespace == "network_runtime":
        palette = digest64(palette, "palette_fingerprint")
    else:
        require(palette is None, "palette_fingerprint allowed only for network_runtime")
    entries = obj.get("entries")
    require(isinstance(entries, list) and 0 < len(entries) <= MAX_RECORDS,
            "registry entries missing, empty or too large")
    seen = set()
    seen_types = set()
    seen_network_states = set()
    normalized = []
    unknown = False
    for record in entries:
        require(isinstance(record, dict), "each registry entry must be an object")
        raw_id = record.get("id")
        if namespace == "network_runtime":
            require(type(raw_id) is int and -(2**31) <= raw_id <= 0xffffffff,
                    "network ID must be signed i32 or unsigned u32")
            # Network varints and 32-bit FNV hashes may appear signed in dumps.
            # Keep the exact bit pattern in the binary; original sign in evidence.
            id_num = raw_id & 0xffffffff
        else:
            id_num = natural(raw_id, "entry id")
        name = name_of(record.get("name"), namespace)
        evidence = record.get("evidence")
        require(isinstance(evidence, str) and len(evidence.strip()) >= 8,
                "each ID requires precise per-entry symbol/RVA evidence")
        if names is None or name not in names:
            unknown = True
            require(allow_unknown, "BlockState catalog cannot verify name " + name
                    + "; use --stage-unverified only for non-authoritative storage")
        meta = record.get("metadata")
        if namespace == "legacy_numeric_block":
            meta = natural(meta, "legacy metadata")
            require(meta <= 65535, "legacy metadata exceeds u16")
        else:
            require(meta is None, "metadata is only valid for legacy_numeric_block")
        if namespace == "pdb_symbol_order":
            # Require an explicit marker so this evidence namespace can never be
            # mistaken for a verified runtime ID.
            require(record.get("symbol_ordinal") == raw_id,
                    "pdb_symbol_order entries must set symbol_ordinal equal to id (evidence only)")
        key = (id_num, meta) if meta is not None else id_num
        require(key not in seen, "duplicate/conflicting numeric identity " + repr(key))
        seen.add(key)
        states = record.get("states")
        storage_version = record.get("version")
        if namespace in ("block_type_runtime", "pdb_symbol_order"):
            require(states is None, namespace + " maps a block type, not an NBT state")
            require(storage_version is None, namespace + " has no persisted NBT version")
        else:
            require(type(storage_version) is int and -(2**31) <= storage_version < 2**31,
                    "persisted NBT BlockState.version must be explicit signed i32")
        serialized_state = encode_state(states,
                required=namespace in ("legacy_numeric_block", "network_runtime"))
        if namespace in ("block_type_runtime", "pdb_symbol_order"):
            message = ("one block name has multiple runtime type IDs; resolve explicit aliases first"
                       if namespace == "block_type_runtime"
                       else "one block name has multiple PDB symbol ordinals")
            require(name not in seen_types, message)
            seen_types.add(name)
        if namespace == "network_runtime":
            semantic_key = (name, serialized_state)
            require(semantic_key not in seen_network_states,
                    "same network BlockState mapped to multiple runtime IDs")
            seen_network_states.add(semantic_key)
        normalized.append((id_num, -1 if meta is None else meta, name,
                           storage_version, serialized_state))
    normalized.sort(key=lambda row: (row[0], row[1], row[2]))
    dictionary = sorted(set(row[2] for row in normalized))
    names_map = {name: i for i, name in enumerate(dictionary)}
    # Format v2 includes exact persisted NBT BlockState.version per mapping.
    binary = bytearray(b"BMCBID02")
    binary.append(NAMESPACES[namespace])
    binary.extend(varint(len(dictionary)))
    for name in dictionary:
        binary.extend(text(name))
    binary.extend(varint(len(normalized)))
    for id_num, metadata, name, storage_version, state in normalized:
        binary.extend(varint(id_num))
        if namespace == "legacy_numeric_block":
            binary.extend(varint(metadata))
        binary.extend(varint(names_map[name]))
        if namespace not in ("block_type_runtime", "pdb_symbol_order"):
            binary.extend(struct.pack("<i", storage_version))
        binary.extend(state)
    return namespace, palette, bytes(binary), len(entries), not unknown


def read_manifest_names(catalog: Path, version: str) -> set[str] | None:
    index = catalog / "index.json"
    if not index.is_file():
        return None
    from diff_catalog import manifest
    versions = json.loads(index.read_text(encoding="utf-8"))["versions"]
    entry = versions.get(version)
    if not entry:
        return None
    filename = catalog / "manifests" / (entry["manifest"] + ".bin")
    return set(manifest(filename))


def validate_source(source: object) -> dict:
    require(isinstance(source, dict), "source must be an object")
    method = source.get("method")
    require(method in ("pdb_symbol_inspection", "runtime_registry_observation", "authoritative_dataset"),
            "source method must identify real symbol/runtime/dataset observation")
    binary_sha = digest64(source.get("binary_sha256"), "source.binary_sha256")
    pdb_sha = source.get("pdb_sha256")
    dataset = source.get("dataset")
    if method == "pdb_symbol_inspection":
        pdb_sha = digest64(pdb_sha, "source.pdb_sha256")
        require(dataset is None, "dataset is only valid for authoritative_dataset")
    elif method == "authoritative_dataset":
        # An authoritative dataset file is NOT a BDS executable fingerprint:
        # record its identity separately and never claim a platform binary.
        require(isinstance(dataset, dict), "authoritative_dataset requires source.dataset")
        require(isinstance(dataset.get("name"), str) and len(dataset["name"]) >= 3,
                "source.dataset.name must identify the dataset")
        require(isinstance(dataset.get("version"), str) and dataset["version"],
                "source.dataset.version must be explicit")
        digest64(dataset.get("file_sha256"), "source.dataset.file_sha256")
        require(isinstance(dataset.get("upstream"), str) and len(dataset["upstream"]) >= 3,
                "source.dataset.upstream must be a source URL")
        dataset = {
            "name": dataset["name"], "version": dataset["version"],
            "file_sha256": dataset["file_sha256"].lower(), "upstream": dataset["upstream"],
        }
        require(pdb_sha is None, "pdb_sha256 is not valid for authoritative_dataset")
    else:
        require(pdb_sha is None or isinstance(pdb_sha, str), "source.pdb_sha256 invalid")
        require(dataset is None, "dataset is only valid for authoritative_dataset")
    platform = source.get("platform")
    allowed = ("windows-x64", "linux-x64", "windows-arm64", "linux-arm64", "dataset")
    require(platform in allowed, "source.platform must be explicit")
    if method == "authoritative_dataset":
        require(platform == "dataset", "authoritative_dataset must use platform=dataset")
    else:
        require(platform != "dataset", "platform=dataset is reserved for authoritative_dataset")
    tool = source.get("tool")
    require(isinstance(tool, str) and len(tool.strip()) >= 3,
            "source.tool must identify extraction code and version")
    validated = {"method": method, "platform": platform, "binary_sha256": binary_sha,
                 "pdb_sha256": pdb_sha, "tool": tool}
    if dataset is not None:
        validated["dataset"] = dataset
    return validated


def provenance_scope(version: str, namespace: str, source: dict, fingerprint: str | None) -> str:
    """Stable key identifying one registry's exact source.

    Same version + namespace can legitimately hold data from different
    platforms, binary builds or network palettes; those must not overwrite
    each other. game_version is already the index's outer key and is included
    here so the scope is unambiguous on its own.
    """
    parts = [version, namespace, source["platform"], source["binary_sha256"]]
    if fingerprint is not None:
        parts.append(fingerprint)
    return "|".join(parts)


def import_data(payload: dict, catalog: Path, stage_unverified: bool, replace: bool) -> dict:
    require(isinstance(payload, dict) and payload.get("schema_version") == 1,
            "unsupported input schema_version")
    version = payload.get("game_version")
    require(isinstance(version, str) and bool(re.fullmatch(r"\d+(?:\.\d+){3}", version)),
            "game_version must identify exact 4-component BDS build")
    source = validate_source(payload.get("source"))
    registries = payload.get("registries")
    require(isinstance(registries, list) and registries, "nonempty registries array required")
    known = read_manifest_names(catalog, version)
    require(known is not None or stage_unverified,
            "game version absent from verified BlockState catalog; use --stage-unverified")
    existing_file = catalog / "id-registry" / "index.json"
    current = json.loads(existing_file.read_text(encoding="utf-8")) if existing_file.exists() else {
        "format_version": 1, "versions": {}
    }
    require(current.get("format_version") == 1, "unsupported ID registry index")
    version_entry = current["versions"].get(version, {"registries": {}})
    outputs: dict[str, bytes] = {}
    evidence_objects: dict[str, bytes] = {}
    staged: dict[str, dict] = {}
    for item in registries:
        namespace, fingerprint, binary, count, checked = parse_registry(item, known, stage_unverified)
        object_hash = hashlib.sha256(binary).hexdigest()
        audit = json.dumps(
            {"game_version": version, "source": source,
             "registry": item, "status": "identity_evidence_only" if checked else "unverified_names"},
            sort_keys=True, separators=(",", ":"), ensure_ascii=False,
        ).encode("utf-8")
        audit_bytes = zlib.compress(audit, level=9)
        audit_hash = hashlib.sha256(audit_bytes).hexdigest()
        metadata = {
            "object_sha256": object_hash, "evidence_sha256": audit_hash, "entries": count,
            "source": source,
            "verified_block_names": checked,
            "status": "identity_evidence_only" if checked else "unverified_names",
        }
        if fingerprint is not None:
            metadata["palette_fingerprint"] = fingerprint
        scope = provenance_scope(version, namespace, source, fingerprint)
        metadata["provenance_scope"] = scope
        prior = version_entry["registries"].get(scope)
        require(prior is None or prior == metadata or replace,
                "registry already exists with different ID data/provenance: "
                + scope + " (require explicit --replace)")
        staged[scope] = metadata
        outputs[object_hash] = binary
        evidence_objects[audit_hash] = audit_bytes
    output_folder = catalog / "id-registry" / "objects"
    output_folder.mkdir(parents=True, exist_ok=True)
    # No mutation until all records and conflicts validate.
    for digest, binary in outputs.items():
        path = output_folder / (digest + ".bin")
        if path.exists():
            require(path.read_bytes() == binary, "SHA-256 object collision / corruption")
        else:
            temporary = path.with_suffix(".tmp")
            temporary.write_bytes(binary)
            os.replace(temporary, path)
    evidence_folder = catalog / "id-registry" / "evidence"
    evidence_folder.mkdir(parents=True, exist_ok=True)
    for digest, payload in evidence_objects.items():
        path = evidence_folder / (digest + ".json.zlib")
        if path.exists():
            require(path.read_bytes() == payload, "evidence archive corrupted")
        else:
            temporary = path.with_suffix(".tmp")
            temporary.write_bytes(payload)
            os.replace(temporary, path)
    version_entry["registries"].update(staged)
    current["versions"][version] = version_entry
    existing_file.parent.mkdir(parents=True, exist_ok=True)
    temporary = existing_file.with_suffix(".json.tmp")
    temporary.write_text(json.dumps(current, indent=2, ensure_ascii=False, sort_keys=True)
                         + "\n", encoding="utf-8")
    os.replace(temporary, existing_file)
    return {"game_version": version, "namespaces": staged}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--input", required=True, type=Path)
    parser.add_argument("--catalog", type=Path,
                        default=Path("crates/bedrock-world/data/block-catalog"))
    parser.add_argument("--stage-unverified", action="store_true",
                        help="permit uncatalogued names, permanently marked non-authoritative")
    parser.add_argument("--replace", action="store_true",
                        help="explicitly replace a conflicting same-version namespace")
    args = parser.parse_args()
    report = import_data(json.loads(args.input.read_text(encoding="utf-8")),
                         args.catalog, args.stage_unverified, args.replace)
    print(json.dumps(report, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
