#!/usr/bin/env python3
"""Generate stable map entity icons from Bedrock vanilla and education packs.

The generated WebP files are source assets. Run this script after updating the
bundled Minecraft version, then rebuild BMCBL so build.rs embeds the results.
"""

from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

from PIL import Image, ImageChops, ImageEnhance, ImageFilter, ImageOps


PROJECT_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_RESOURCE_PACKS_ROOT = (
    PROJECT_ROOT
    / "target"
    / "debug"
    / "BMCBL"
    / "versions"
    / "26.21"
    / "data"
    / "resource_packs"
)
DEFAULT_OUTPUT = PROJECT_ROOT / "assets" / "images" / "map" / "entity"
ICON_SIZE = 64
ICON_INSET = 2

SPECIAL_ITEMS = {
    "minecart": "minecart_normal",
    "spawner_minecart": "minecart_normal",
    "chest_minecart": "minecart_chest",
    "command_block_minecart": "minecart_command_block",
    "furnace_minecart": "minecart_furnace",
    "hopper_minecart": "minecart_hopper",
    "tnt_minecart": "minecart_tnt",
    "xp_orb": "experience_bottle",
    "trident": "trident",
    "thrown_trident": "trident",
    "snowball": "snowball",
    "balloon": "balloon",
    "armor_stand": "armor_stand",
    "egg": "egg",
    "boat": "boat",
    "chest_boat": "boat",
}

# These entities present their identifying eye and silhouette from the side.
SIDE_PROFILE_ENTITIES = {
    "axolotl",
    "cod",
    "dolphin",
    "glow_squid",
    "salmon",
    "squid",
    "tropicalfish",
}

FRONT_BODY_ENTITIES = {
    "tadpole",
}

# These entities should contribute only the head, viewed from the side.
SIDE_HEAD_ENTITIES = {
    "armadillo",
    "sheep",
    "sniffer",
    "turtle",
    "vex",
}

SIDE_HEAD_DIRECTIONS = {
    "armadillo": "east",
    "sniffer": "east",
    "vex": "north",
    "sheep": "north",
}

FRONT_PROFILE_ENTITIES = {
    "breeze_wind_charge_projectile",
    "ender_crystal",
    "endermite",
    "evocation_fang",
    "frog",
    "glow_squid",
    "magma_cube",
    "silverfish",
    "slime",
    "squid",
    "witch",
}

HEAD_NECK_PROFILE_ENTITIES = {
    "camel",
    "donkey",
    "horse",
    "llama",
    "mule",
    "skeleton_horse",
    "trader_llama",
    "zombie_horse",
}

# The Wiki's compact entity sprites use a deliberate head/neck portrait box;
# keeping these per-model ratios prevents the raw Bedrock Y coordinates from
# stretching a horse or camel neck into a full-body icon.
HEAD_NECK_TARGET_SIZES = {
    "camel": (13, 16),
    "donkey": (12, 16),
    "horse": (14, 13),
    "llama": (16, 16),
    "mule": (12, 16),
    "skeleton_horse": (14, 13),
    "trader_llama": (16, 16),
    "zombie_horse": (14, 13),
}

# The vanilla villager head texture already contains the front-facing eyes and
# nose. The separate nose cube is a depth aid for 3-D rendering, not a second
# sprite layer; compositing it into a 2-D icon shifts the nose off the Wiki
# reference. Crop the same square front portrait used by the Wiki sprites.
VILLAGER_FRONT_ENTITIES = {
    "villager",
    "villager_v2",
    "zombie_villager",
}

# Some definitions intentionally use a tiny growth-stage geometry. The map
# icon needs the stable, recognisable adult silhouette instead of that stage.
PORTRAIT_GEOMETRY_OVERRIDES = {
    "pufferfish": "geometry.pufferfish.mid",
    "witch": "geometry.villager.v1.8",
}

PORTRAIT_DIRECTIONS = {
    "bat": "east",
    "camel": "east",
    "fox": "east",
    "horse": "east",
    "pig": "east",
    "rabbit": "east",
    "pufferfish": "north",
}

# Definitions with multiple texture variants do not have a meaningful
# ``default`` key. Pick a canonical variant so the generated catalog is stable
# across resource-pack ordering.
PREFERRED_TEXTURE_KEYS = {
    "axolotl": "lucy",
    "tropicalfish": "typeA",
}

def opaque_crop(image: Image.Image) -> Image.Image:
    rgba = image.convert("RGBA")
    alpha_bounds = rgba.getchannel("A").getbbox()
    return rgba.crop(alpha_bounds) if alpha_bounds else rgba


def normalize_entity_texture(image: Image.Image) -> Image.Image:
    rgba = image.convert("RGBA")
    alpha = rgba.getchannel("A")
    if any(0 < value < 16 for value in alpha.getdata()):
        alpha = alpha.point(lambda value: 0 if value == 0 else 255)
        rgba.putalpha(alpha)
    return rgba


def colorize_entity_texture(
    image: Image.Image,
    dark: tuple[int, int, int],
    light: tuple[int, int, int],
) -> Image.Image:
    rgba = normalize_entity_texture(image)
    colorized = ImageOps.colorize(rgba.convert("L"), black=dark, white=light)
    colorized.putalpha(rgba.getchannel("A"))
    return colorized


def tropicalfish_texture(
    resource_packs: list[Path],
    texture_path: Path,
    geometry: dict,
) -> Image.Image:
    base = colorize_entity_texture(
        Image.open(texture_path),
        dark=(8, 32, 48),
        light=(50, 205, 220),
    )
    pattern_path = texture_file(
        resource_packs,
        "textures/entity/fish/tropical_a_pattern_1",
        geometry_texture_size(geometry),
    )
    if pattern_path is None:
        return base
    pattern = colorize_entity_texture(
        Image.open(pattern_path),
        dark=(80, 24, 12),
        light=(255, 135, 40),
    )
    base.alpha_composite(pattern)
    return base


def write_icon(image: Image.Image, output: Path) -> None:
    cropped = opaque_crop(image)
    target = ICON_SIZE - ICON_INSET * 2
    scale = min(target / cropped.width, target / cropped.height)
    width = max(1, round(cropped.width * scale))
    height = max(1, round(cropped.height * scale))
    resized = cropped.resize((width, height), Image.Resampling.NEAREST)
    alpha = resized.getchannel("A")
    expanded_alpha = alpha.filter(ImageFilter.MaxFilter(3))
    outline_alpha = ImageChops.subtract(expanded_alpha, alpha)
    outline = Image.new("RGBA", resized.size, (0, 0, 0, 0))
    outline.putalpha(outline_alpha)
    canvas = Image.new("RGBA", (ICON_SIZE, ICON_SIZE))
    offset = ((ICON_SIZE - width) // 2, (ICON_SIZE - height) // 2)
    canvas.alpha_composite(outline, offset)
    canvas.alpha_composite(resized, offset)
    output.parent.mkdir(parents=True, exist_ok=True)
    canvas.save(output, format="WEBP", lossless=True, method=6)


def output_name(identifier: str) -> str:
    return identifier.removeprefix("minecraft:").lower().replace("-", "_")


def geometry_index(resource_packs: list[Path]) -> dict[str, dict]:
    index: dict[str, dict] = {}
    for resource_pack in resource_packs:
        models = resource_pack / "models" / "entity"
        for path in models.glob("*.json"):
            try:
                document = load_jsonc(path)
            except (OSError, json.JSONDecodeError):
                continue
            for geometry in document.get("minecraft:geometry", []):
                identifier = geometry.get("description", {}).get("identifier")
                if identifier and identifier not in index:
                    index[identifier] = geometry
            for identifier, geometry in document.items():
                if (
                    identifier.startswith("geometry.")
                    and isinstance(geometry, dict)
                    and identifier not in index
                ):
                    index[identifier] = geometry
    return index


def geometry_for_identifier(models: dict[str, dict], identifier: str) -> dict | None:
    if geometry := models.get(identifier):
        return geometry
    return next(
        (
            geometry
            for aliases, geometry in models.items()
            if identifier in aliases.split(":")
        ),
        None,
    )


def load_jsonc(path: Path) -> dict:
    source = path.read_text(encoding="utf-8")
    without_block_comments = re.sub(r"/\*.*?\*/", "", source, flags=re.DOTALL)
    without_comments = re.sub(r"//[^\r\n]*", "", without_block_comments)
    return json.loads(without_comments)


def cube_face(cube: dict, direction: str) -> tuple[int, int, int, int] | None:
    uv = cube.get("uv")
    if isinstance(uv, dict):
        aliases = (direction, "front") if direction == "north" else (direction,)
        for alias in aliases:
            face = uv.get(alias)
            if not isinstance(face, dict):
                continue
            origin = face.get("uv")
            face_size = face.get("uv_size")
            if isinstance(origin, list) and isinstance(face_size, list):
                return int(origin[0]), int(origin[1]), int(face_size[0]), int(face_size[1])
    size = cube.get("size")
    if not (isinstance(uv, list) and isinstance(size, list) and len(uv) >= 2 and len(size) >= 3):
        return None
    width, height, depth = (max(0, int(value)) for value in size[:3])
    offsets = {
        "north": (depth, depth, width, height),
        "east": (depth + width, depth, depth, height),
        "south": (depth + width + depth, depth, width, height),
        "west": (0, depth, depth, height),
    }
    try:
        offset_x, offset_y, face_width, face_height = offsets[direction]
    except KeyError:
        return None
    return int(uv[0]) + offset_x, int(uv[1]) + offset_y, face_width, face_height


def portrait_cubes(geometry: dict) -> list[dict]:
    bones = geometry.get("bones", [])
    head = next((bone for bone in bones if "head" in bone.get("name", "").lower()), None)
    if not head:
        head = next(
            (
                bone
                for bone in bones
                if bone.get("name", "").lower() in ("body", "main", "torso")
                and bone.get("cubes")
            ),
            None,
        )
    if not head:
        fallback_parts = (
            "body",
            "cube",
            "crystal",
            "eye",
            "fang",
            "inner",
            "jaw",
            "mouth",
            "nose",
            "outer",
            "projectile",
            "section",
            "wind",
        )
        fallback_bones = [
            bone
            for bone in bones
            if bone.get("cubes")
            and any(part in bone.get("name", "").lower() for part in fallback_parts)
        ]
        if not fallback_bones:
            fallback_bones = [bone for bone in bones if bone.get("cubes")]
        cubes: list[dict] = []
        for bone in fallback_bones:
            cubes.extend(bone.get("cubes", []))
        return cubes
    cubes = []
    for cube in head.get("cubes", []):
        marked = dict(cube)
        marked["_portrait_role"] = "head"
        cubes.append(marked)
    head_name = head.get("name")
    for bone in bones:
        name = bone.get("name", "").lower()
        if bone.get("parent") == head_name and any(
            part in name
            for part in (
                "antenna",
                "beak",
                "bill",
                "ear",
                "eye",
                "horn",
                "muzzle",
                "nose",
                "snout",
                "tusk",
            )
        ):
            for cube in bone.get("cubes", []):
                marked = dict(cube)
                marked["_portrait_role"] = name
                cubes.append(marked)
    return cubes


def render_head(
    texture: Image.Image, geometry: dict, direction: str = "north"
) -> Image.Image | None:
    texture = texture.convert("RGBA")
    faces: list[tuple[int, int, int, int, int, int, Image.Image, dict]] = []
    for cube in portrait_cubes(geometry):
        face = cube_face(cube, direction)
        origin = cube.get("origin")
        size = cube.get("size")
        if not (face and isinstance(origin, list) and isinstance(size, list) and len(origin) >= 3):
            continue
        left, top, width, height = face
        if left < 0 or top < 0 or left + width > texture.width or top + height > texture.height:
            continue
        faces.append(
            (
                int(origin[0]),
                int(origin[1]),
                int(origin[2]),
                width,
                height,
                int(size[2]),
                texture.crop((left, top, left + width, top + height)),
                cube,
            )
        )
    if not faces:
        return None
    horizontal_origins = [face[0] if direction == "north" else face[2] for face in faces]
    min_x = min(horizontal_origins)
    max_x = max(origin + face[3] for origin, face in zip(horizontal_origins, faces))
    min_y = min(face[1] for face in faces)
    max_y = max(face[1] + face[4] for face in faces)
    canvas = Image.new("RGBA", (max_x - min_x, max_y - min_y))
    for origin_x, origin_y, origin_z, width, height, _depth, face, _cube in sorted(
        faces, key=lambda item: item[2]
    ):
        resized = face.resize((width, height), Image.Resampling.NEAREST)
        horizontal_origin = origin_x if direction == "north" else origin_z
        canvas.alpha_composite(
            resized,
            (horizontal_origin - min_x, max_y - origin_y - height),
        )
    if direction == "north":
        for origin_x, origin_y, _origin_z, width, height, _depth, _face, cube in faces:
            if cube.get("_portrait_role") != "nose":
                continue
            side = cube_face(cube, "east")
            if side is None:
                continue
            left, top, side_width, side_height = side
            if (
                left < 0
                or top < 0
                or left + side_width > texture.width
                or top + side_height > texture.height
            ):
                continue
            side_image = texture.crop((left, top, left + side_width, top + side_height))
            side_image = ImageEnhance.Brightness(side_image).enhance(0.65)
            side_image = side_image.resize((max(1, int(cube["size"][2])), height), Image.Resampling.NEAREST)
            x = origin_x - min_x + width - 1
            y = max_y - origin_y - height
            if 0 <= x < canvas.width:
                canvas.alpha_composite(side_image, (x, y))
    return canvas


def render_villager_front(texture: Image.Image, geometry: dict) -> Image.Image | None:
    """Render the square front face represented by the Wiki villager sprites."""
    head = next(
        (bone for bone in geometry.get("bones", []) if "head" in bone.get("name", "").lower()),
        None,
    )
    if not head:
        return None
    head_cube = next(
        (
            cube
            for cube in head.get("cubes", [])
            if isinstance(cube.get("size"), list)
            and len(cube["size"]) >= 3
            and int(cube["size"][0]) >= 8
            and int(cube["size"][1]) >= 8
        ),
        None,
    )
    if head_cube is None:
        return None
    face = cube_face(head_cube, "north")
    if face is None:
        return None
    left, top, width, height = face
    if left < 0 or top < 0 or left + width > texture.width or top + height > texture.height:
        return None
    source = texture.convert("RGBA").crop((left, top, left + width, top + height))
    portrait_height = min(8, source.height)
    return source.crop((0, source.height - portrait_height, source.width, source.height))


def render_side_profile(texture: Image.Image, geometry: dict) -> Image.Image | None:
    texture = texture.convert("RGBA")
    faces: list[tuple[int, int, int, int, int, Image.Image]] = []
    for bone in geometry.get("bones", []):
        for cube in bone.get("cubes", []):
            face = None
            face_image = None
            for direction in ("east", "west", "north", "south"):
                candidate = cube_face(cube, direction)
                if candidate is None:
                    continue
                left, top, width, height = candidate
                if (
                    width <= 0
                    or height <= 0
                    or left < 0
                    or top < 0
                    or left + width > texture.width
                    or top + height > texture.height
                ):
                    continue
                candidate_image = texture.crop((left, top, left + width, top + height))
                if candidate_image.getchannel("A").getbbox() is None:
                    continue
                face = candidate
                face_image = candidate_image
                break
            origin = cube.get("origin")
            size = cube.get("size")
            if not (
                face
                and face_image is not None
                and isinstance(origin, list)
                and isinstance(size, list)
                and len(origin) >= 3
                and len(size) >= 3
            ):
                continue
            left, top, width, height = face
            faces.append(
                (
                    int(origin[0]) + int(size[0]),
                    int(origin[1]),
                    int(origin[2]),
                    width,
                    height,
                    face_image,
                )
            )
    if not faces:
        return None
    min_z = min(face[2] for face in faces)
    max_z = max(face[2] + face[3] for face in faces)
    min_y = min(face[1] for face in faces)
    max_y = max(face[1] + face[4] for face in faces)
    canvas = Image.new("RGBA", (max_z - min_z, max_y - min_y))
    for _depth, origin_y, origin_z, width, height, face in sorted(
        faces,
        key=lambda item: (item[0], item[1], item[2], item[3], item[4]),
    ):
        canvas.alpha_composite(face, (origin_z - min_z, max_y - origin_y - height))
    return canvas


def render_head_neck_profile(
    texture: Image.Image, geometry: dict, identifier: str
) -> Image.Image | None:
    """Render a single-ear head and neck without saddle, chest, or body bones."""
    bones = geometry.get("bones", [])
    head = next((bone for bone in bones if "head" in bone.get("name", "").lower()), None)
    if head is None:
        return None
    head_name = head.get("name")
    selected: list[dict] = [head]
    selected_names = {head_name}
    neck = next(
        (
            bone
            for bone in bones
            if "neck" in bone.get("name", "").lower() and bone.get("cubes")
        ),
        None,
    )
    if neck is not None and neck.get("name") not in selected_names:
        selected.append(neck)
        selected_names.add(neck.get("name"))

    ear_bones = [
        bone
        for bone in bones
        if "ear" in bone.get("name", "").lower() and bone.get("cubes")
    ]
    if ear_bones:
        if identifier in {"donkey", "mule"}:
            ear = max(
                ear_bones,
                key=lambda bone: max(
                    (int(cube.get("size", [0, 0, 0])[1]) for cube in bone.get("cubes", [])),
                    default=0,
                ),
            )
        else:
            ear = ear_bones[0]
        selected.append(ear)

    for bone in bones:
        name = bone.get("name", "").lower()
        if bone in selected or bone in ear_bones:
            continue
        if bone.get("parent") in selected_names and any(
            part in name for part in ("bridle", "mouth", "muzzle", "nose", "snout")
        ):
            selected.append(bone)

    filtered_geometry = dict(geometry)
    filtered_geometry["bones"] = selected
    profile = render_side_profile(texture, filtered_geometry)
    target_size = HEAD_NECK_TARGET_SIZES.get(identifier)
    if profile is not None and target_size is not None:
        profile = profile.resize(target_size, Image.Resampling.NEAREST)
    return profile


def render_front_body_profile(texture: Image.Image, geometry: dict) -> Image.Image | None:
    """Render a body-only front sprite for models without a head bone."""
    body = next(
        (
            bone
            for bone in geometry.get("bones", [])
            if bone.get("name", "").lower() in {"body", "main", "torso"}
            and bone.get("cubes")
        ),
        None,
    )
    if body is None:
        return None
    filtered_geometry = dict(geometry)
    filtered_geometry["bones"] = [body]
    return render_head(texture, filtered_geometry, "north")


def render_front_profile(texture: Image.Image, geometry: dict) -> Image.Image | None:
    profile = render_side_profile(texture, geometry)
    front = render_head(texture, geometry, "north")
    if profile is None:
        return front
    if front is None:
        return profile
    offset = ((profile.width - front.width) // 2, 0)
    profile.alpha_composite(front, offset)
    return profile


def texture_file(
    resource_packs: list[Path],
    reference: str,
    expected_size: tuple[int, int] | None = None,
) -> Path | None:
    fallback = None
    for resource_pack in reversed(resource_packs):
        base = resource_pack / reference
        for extension in (".png", ".tga"):
            candidate = base.with_suffix(extension)
            if candidate.is_file():
                fallback = fallback or candidate
                if expected_size is None:
                    return candidate
                try:
                    with Image.open(candidate) as image:
                        if image.size == expected_size:
                            return candidate
                except OSError:
                    continue
    return fallback


def geometry_texture_size(geometry: dict) -> tuple[int, int] | None:
    description = geometry.get("description", {})
    width = description.get(
        "texture_width",
        description.get(
            "texturewidth",
            geometry.get("texture_width", geometry.get("texturewidth")),
        ),
    )
    height = description.get(
        "texture_height",
        description.get(
            "textureheight",
            geometry.get("texture_height", geometry.get("textureheight")),
        ),
    )
    if isinstance(width, (int, float)) and isinstance(height, (int, float)):
        return max(1, int(width)), max(1, int(height))
    return None


def entity_head_assets(resource_packs: list[Path], output: Path) -> dict[str, str]:
    models = geometry_index(resource_packs)
    definitions: dict[str, Path] = {}
    for resource_pack in resource_packs:
        for definition_path in (resource_pack / "entity").glob("*.entity.json"):
            definitions[definition_path.name] = definition_path
    manifest: dict[str, str] = {}
    for definition_path in (definitions[name] for name in sorted(definitions)):
        try:
            document = load_jsonc(definition_path)
            description = document["minecraft:client_entity"]["description"]
            identifier = output_name(description["identifier"])
            textures = description["textures"]
            preferred_key = PREFERRED_TEXTURE_KEYS.get(identifier)
            texture_ref = (
                textures.get("elder")
                if identifier == "elder_guardian"
                else textures.get(preferred_key)
                if preferred_key
                else textures.get("default") or textures.get("base")
            )
            if not isinstance(texture_ref, str):
                texture_ref = next(value for value in textures.values() if isinstance(value, str))
            geometry_definitions = description.get("geometry", {})
            geometry_id = geometry_definitions.get("default")
            if identifier == "tropicalfish":
                geometry_id = geometry_definitions.get("typeA") or geometry_definitions.get("typeB")
            if not isinstance(geometry_id, str):
                continue
        except (KeyError, StopIteration, TypeError, json.JSONDecodeError):
            continue
        geometry_id = PORTRAIT_GEOMETRY_OVERRIDES.get(identifier, geometry_id)
        geometry = (
            geometry_for_identifier(models, "geometry.sheep.sheared.v1.8")
            if identifier == "sheep"
            else geometry_for_identifier(models, geometry_id)
        )
        texture_path = (
            texture_file(resource_packs, texture_ref, geometry_texture_size(geometry))
            if geometry
            else None
        )
        if not geometry or texture_path is None:
            continue
        try:
            texture = (
                tropicalfish_texture(resource_packs, texture_path, geometry)
                if identifier == "tropicalfish"
                else normalize_entity_texture(Image.open(texture_path))
            )
            face = (
                render_front_body_profile(texture, geometry)
                if identifier in FRONT_BODY_ENTITIES
                else render_head_neck_profile(texture, geometry, identifier)
                if identifier in HEAD_NECK_PROFILE_ENTITIES
                else render_front_profile(texture, geometry)
                if identifier in FRONT_PROFILE_ENTITIES
                else render_side_profile(texture, geometry)
                if identifier in SIDE_PROFILE_ENTITIES
                else render_villager_front(texture, geometry)
                if identifier in VILLAGER_FRONT_ENTITIES
                else render_head(
                    texture,
                    geometry,
                    SIDE_HEAD_DIRECTIONS.get(identifier, "east"),
                )
                if identifier in SIDE_HEAD_ENTITIES
                else render_head(
                    texture,
                    geometry,
                    PORTRAIT_DIRECTIONS.get(identifier, "north"),
                )
            )
        except OSError:
            continue
        if face is None:
            continue
        output_path = output / f"{identifier}.webp"
        write_icon(face, output_path)
        manifest[identifier] = output_path.name
    return manifest


def coverage_report(reference_directory: Path, manifest: dict[str, str]) -> dict[str, object]:
    reference_names = {path.stem for path in reference_directory.glob("*.png")}
    generated_names = set(manifest)
    return {
        "reference_count": len(reference_names),
        "generated_count": len(generated_names),
        "missing": sorted(reference_names - generated_names),
    }


def default_resource_packs() -> list[Path]:
    return [
        DEFAULT_RESOURCE_PACKS_ROOT / "vanilla",
        *sorted(DEFAULT_RESOURCE_PACKS_ROOT.glob("vanilla_*")),
        DEFAULT_RESOURCE_PACKS_ROOT / "chemistry",
    ]


def generate(resource_packs: list[Path], output: Path) -> dict[str, str]:
    output.mkdir(parents=True, exist_ok=True)
    for generated in output.glob("*.webp"):
        generated.unlink()
    manifest: dict[str, str] = {}
    resource_packs = [path for path in resource_packs if path.is_dir()]
    manifest.update(entity_head_assets(resource_packs, output))

    for identifier, item_name in SPECIAL_ITEMS.items():
        source = texture_file(resource_packs, f"textures/items/{item_name}")
        if source is None:
            continue
        output_path = output / f"{output_name(identifier)}.webp"
        write_icon(Image.open(source), output_path)
        manifest[output_name(identifier)] = output_path.name

    for legacy, canonical in {
        "villager_v2": "villager",
        "zombie_villager_v2": "zombie_villager",
    }.items():
        if canonical in manifest:
            source = output / manifest[canonical]
            alias = output / f"{legacy}.webp"
            alias.write_bytes(source.read_bytes())
            manifest[legacy] = alias.name

    with (output / "manifest.json").open("w", encoding="utf-8") as file:
        json.dump(dict(sorted(manifest.items())), file, indent=2)
        file.write("\n")
    return manifest


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--resource-pack", type=Path, action="append")
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--reference-directory", type=Path)
    arguments = parser.parse_args()
    resource_packs = arguments.resource_pack or default_resource_packs()
    manifest = generate([path.resolve() for path in resource_packs], arguments.output.resolve())
    print(f"generated {len(set(manifest.values()))} WebP entity icons in {arguments.output}")
    if arguments.reference_directory:
        report = coverage_report(arguments.reference_directory.resolve(), manifest)
        with (arguments.output / "coverage.json").open("w", encoding="utf-8") as file:
            json.dump(report, file, indent=2)
            file.write("\n")
        print(
            f"coverage: {report['generated_count']}/{report['reference_count']} "
            f"reference identifiers, {len(report['missing'])} missing"
        )


if __name__ == "__main__":
    main()
