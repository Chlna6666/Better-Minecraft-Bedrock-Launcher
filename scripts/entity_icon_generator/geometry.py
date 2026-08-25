from __future__ import annotations

import json
import re
from pathlib import Path
from PIL import Image

PROJECT_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_RESOURCE_PACKS_ROOT = (
    PROJECT_ROOT
    / "target"
    / "debug"
    / "BMCBL"
    / "versions"
    / "26.33"
    / "data"
    / "resource_packs"
)
DEFAULT_OUTPUT = PROJECT_ROOT / "assets" / "images" / "map" / "entity"

SPECIAL_ITEMS = {
    "bamboo_chest_raft": "bamboo_chest_raft",
    "bamboo_raft": "bamboo_raft",
    "boat": "boat_oak",
    "chest_minecart": "minecart_chest",
    "chest_boat": "oak_chest_boat",
    "command_block_minecart": "minecart_command_block",
    "ender_crystal": "end_crystal",
    "furnace_minecart": "minecart_furnace",
    "hopper_minecart": "minecart_hopper",
    "minecart": "minecart_normal",
    "painting": "painting",
    "spawner_minecart": "minecart_normal",
    "tnt_minecart": "minecart_tnt",
}

# Category 3: Side Profile Face + Body entities
SIDE_PROFILE_ENTITIES = {
    "cod",
    "dolphin",
    "nautilus",
    "salmon",
    "silverfish",
    "tropicalfish",
    "zombie_nautilus",
}

FRONT_BODY_ENTITIES = {
    "tadpole",
}

# Category 2: Side Profile Face entities
SIDE_HEAD_ENTITIES = {
    "sniffer",
    "turtle",
}

SIDE_HEAD_DIRECTIONS = {
    "sniffer": "east",
    "turtle": "east",
}

# Category 3: Head & Neck Side Profile entities
HEAD_NECK_PROFILE_ENTITIES = {
    "camel",
    "camel_husk",
    "donkey",
    "horse",
    "mule",
    "skeleton_horse",
    "zombie_horse",
}

HEAD_NECK_TARGET_SIZES = {
    "camel": (13, 16),
    "camel_husk": (13, 16),
    "donkey": (12, 16),
    "horse": (14, 13),
    "mule": (12, 16),
    "skeleton_horse": (14, 13),
    "zombie_horse": (14, 13),
}

PORTRAIT_GEOMETRY_OVERRIDES = {
    "minecart": "geometry.minecart.v1.8",
    "pufferfish": "geometry.pufferfish.mid",
    "sheep": "geometry.sheep.v1.8",
    "skull": "geometry.mob_head",
}

# Bind entities whose current asset is only correct in a newer resource pack.
# Without a pin the first matching pack wins, which can pick an older texture
# layout (for example the 64x64 vex texture vs the matching 32x32 one).
ENTITY_RESOURCE_PACK_PINS = {
    "bogged": "vanilla_1.21.90",
    "glow_squid": "vanilla_1.21.90",
    "rabbit": "vanilla_1.26.10",
    "vex": "vanilla_1.19.50",
}

PORTRAIT_DIRECTIONS = {
    "bat": "east",
    "fox": "east",
    "pig": "east",
    "pufferfish": "north",
}

PREFERRED_TEXTURE_KEYS = {
    "axolotl": "lucy",
    "skull": "skeleton",
    "tropicalfish": "typeA",
    "zombie_villager": "default",
}


def load_jsonc(path: Path) -> dict:
    source = path.read_text(encoding="utf-8")
    without_block_comments = re.sub(r"/\*.*?\*/", "", source, flags=re.DOTALL)
    without_comments = re.sub(r"//[^\r\n]*", "", without_block_comments)
    return json.loads(without_comments)


def output_name(identifier: str) -> str:
    return identifier.removeprefix("minecraft:").lower().replace("-", "_")


def geometry_texture_size(geometry: dict) -> tuple[int, int]:
    description = geometry.get("description", {})
    w = int(description.get("texture_width") or 64)
    h = int(description.get("texture_height") or 32)
    return w, h


def portrait_cubes(geometry: dict) -> list[dict]:
    keywords = (
        "head",
        "nose",
        "hat",
        "helmet",
        "brim",
        "jaw",
        "mouth",
        "chin",
        "horn",
        "horns",
        "tusk",
        "tusks",
        "ear",
        "ears",
        "gills",
        "snout",
        "beak",
        "eye",
        "mushroom",
        "cap",
        "section_0",
        "look_at",
    )
    portrait_bones = [
        bone
        for bone in geometry.get("bones", [])
        if any(kw in bone.get("name", "").lower() for kw in keywords)
    ]
    cubes = []
    for bone in portrait_bones:
        cubes.extend(bone.get("cubes", []))
    return cubes


def geometry_index(resource_packs: list[Path]) -> dict[str, dict]:
    index: dict[str, dict] = {}

    def index_document(document: dict) -> None:
        format_version = str(document.get("format_version", "1.10.0"))
        if format_version.startswith("1.8") or format_version.startswith("1.10"):
            for key, value in document.items():
                if key.startswith("geometry.") and isinstance(value, dict):
                    index[key] = value
        else:
            geometries = document.get("minecraft:geometry", [])
            if isinstance(geometries, list):
                for geometry in geometries:
                    identifier = geometry.get("description", {}).get("identifier")
                    if identifier:
                        index[identifier] = geometry

    for resource_pack in resource_packs:
        for geometry_path in (resource_pack / "models" / "entity").glob("*.json"):
            try:
                document = load_jsonc(geometry_path)
            except (json.JSONDecodeError, OSError):
                continue
            index_document(document)
        mobs_path = resource_pack / "models" / "mobs.json"
        if mobs_path.exists():
            try:
                index_document(load_jsonc(mobs_path))
            except (json.JSONDecodeError, OSError):
                continue
    return index


def get_merged_geometry(models: dict[str, dict], identifier: str) -> dict | None:
    visited = set()

    def merge_recursive(curr_id: str) -> dict | None:
        if curr_id in visited:
            return None
        visited.add(curr_id)

        geom = models.get(curr_id)
        if not geom:
            matches = [k for k in models.keys() if k.startswith(curr_id + ":")]
            if matches:
                geom = models[matches[0]]

        if not geom:
            return None

        description = geom.get("description", {})
        parent_id = description.get("parent") or (
            curr_id.split(":", 1)[1] if ":" in curr_id else None
        )

        if parent_id and parent_id != curr_id:
            parent_geom = merge_recursive(parent_id)
            if parent_geom:
                merged = dict(parent_geom)
                pbones = list(parent_geom.get("bones", []))
                cbones = list(geom.get("bones", []))
                bone_map = {b.get("name"): b for b in pbones}
                for cb in cbones:
                    bone_map[cb.get("name")] = cb
                merged["bones"] = list(bone_map.values())
                merged["description"] = description
                return merged

        return geom

    return merge_recursive(identifier)


def geometry_for_identifier(models: dict[str, dict], identifier: str) -> dict | None:
    if identifier in models:
        return get_merged_geometry(models, identifier)
    matches = [k for k in models if k.startswith(identifier + ":")]
    if matches:
        return get_merged_geometry(models, matches[0])
    return None


def texture_file(
    resource_packs: list[Path],
    texture_reference: str,
    size: tuple[int, int] | None = None,
    pack_pin: str | None = None,
) -> Path | None:
    cleaned = texture_reference.removeprefix("textures/").removesuffix(".png").removesuffix(".tga")
    pinned = [pack for pack in resource_packs if pack.name == pack_pin] if pack_pin else []
    ordered_packs = [*pinned, *[pack for pack in resource_packs if pack not in pinned]]
    for resource_pack in ordered_packs:
        for extension in [".png", ".tga"]:
            candidate = resource_pack / "textures" / f"{cleaned}{extension}"
            if candidate.exists():
                return candidate
    return None


def cube_face(cube: dict, direction: str) -> tuple[int, int, int, int] | None:
    uv = cube.get("uv")
    size = cube.get("size")
    if not (isinstance(size, list) and len(size) >= 3):
        return None

    x, y, z = size[0], size[1], size[2]
    if isinstance(uv, dict):
        face_uv = uv.get(direction)
        if not (isinstance(face_uv, dict) and isinstance(face_uv.get("uv"), list)):
            return None
        u, v = face_uv["uv"][0], face_uv["uv"][1]
        uv_size = face_uv.get("uv_size") or [0, 0]
        if uv_size == [0, 0]:
            if direction in {"north", "south"}:
                uv_size = [x, y]
            elif direction in {"east", "west"}:
                uv_size = [z, y]
            else:
                uv_size = [x, z]
        return int(u), int(v), int(abs(uv_size[0])), int(abs(uv_size[1]))
    if not (isinstance(uv, list) and len(uv) >= 2):
        return None
    u, v = uv[0], uv[1]

    if direction == "north":
        return int(u + z), int(v + z), int(x), int(y)
    if direction == "south":
        return int(u + z + x + z), int(v + z), int(x), int(y)
    if direction == "west":
        return int(u), int(v + z), int(z), int(y)
    if direction == "east":
        return int(u + z + x), int(v + z), int(z), int(y)
    if direction == "up":
        return int(u + z), int(v), int(x), int(z)
    if direction == "down":
        return int(u + z + x), int(v), int(x), int(z)
    return None
