from __future__ import annotations

import argparse
import json
import re
from pathlib import Path
from PIL import Image

from entity_icon_generator import data_source
from entity_icon_generator.geometry import (
    DEFAULT_OUTPUT,
    DEFAULT_RESOURCE_PACKS_ROOT,
    ENTITY_RESOURCE_PACK_PINS,
    PORTRAIT_GEOMETRY_OVERRIDES,
    PREFERRED_TEXTURE_KEYS,
    SPECIAL_ITEMS,
    get_merged_geometry,
    geometry_index,
    load_jsonc,
    output_name,
    texture_file,
)
from entity_icon_generator.renderers.dispatcher import dispatch_render_portrait
from entity_icon_generator.renderers.dynamic_entity import render_dynamic_entity_icon
from entity_icon_generator.texture import write_icon

ICON_SIZE_OVERRIDES = {
    "ghast": 128,
    "happy_ghast": 128,
}

ICON_CROP_CONTENT_BOTTOM = {
    "camel": 24,
    "camel_husk": 24,
    "horse": 12,
    "donkey": 12,
    "mule": 12,
    "zombie_horse": 16,
}

ICON_OFFSET_X = {
    "camel": 14,
    "camel_husk": 14,
}


def entity_head_assets(resource_packs: list[Path], output: Path) -> dict[str, str]:
    models = geometry_index(resource_packs)
    entity_defs: dict[str, tuple[Path, dict, dict]] = {}

    def version_score(path: Path) -> int:
        match = re.search(r"_v(\d+)", path.stem)
        return int(match.group(1)) if match else 0

    for resource_pack in resource_packs:
        for definition_path in (resource_pack / "entity").glob("*.entity.json"):
            try:
                document = load_jsonc(definition_path)
                description = document["minecraft:client_entity"]["description"]
                identifier = output_name(description["identifier"])
                pinned_pack = ENTITY_RESOURCE_PACK_PINS.get(identifier)
                current = entity_defs.get(identifier)
                prefer_pinned = pinned_pack and resource_pack.name == pinned_pack
                if current is None or prefer_pinned or version_score(definition_path) > version_score(current[0]):
                    entity_defs[identifier] = (definition_path, document, description)
            except (KeyError, TypeError, json.JSONDecodeError):
                continue

    manifest: dict[str, str] = {}
    for identifier, (definition_path, document, description) in sorted(entity_defs.items()):
        try:
            textures = description.get("textures", {})
            preferred_key = PREFERRED_TEXTURE_KEYS.get(identifier)
            texture_ref = (
                textures.get("elder")
                if identifier == "elder_guardian"
                else textures.get(preferred_key)
                if preferred_key and preferred_key in textures
                else "textures/entity/zombie_villager2/zombie-villager"
                if identifier in {"zombie_villager", "zombie_villager_v2"}
                else textures.get("default") or textures.get("base")
            )
            if not isinstance(texture_ref, str) and textures:
                texture_ref = next((value for value in textures.values() if isinstance(value, str)), None)
            if not texture_ref:
                continue

            geometry_definitions = description.get("geometry", {})
            geometry_id = geometry_definitions.get("default")
            if identifier == "tropicalfish":
                geometry_id = geometry_definitions.get("typeA") or geometry_definitions.get("typeB")

            geometry_id = PORTRAIT_GEOMETRY_OVERRIDES.get(identifier, geometry_id)
            if not isinstance(geometry_id, str):
                continue

            geometry = get_merged_geometry(models, geometry_id)
            texture_path = (
                texture_file(
                    resource_packs,
                    texture_ref,
                    pack_pin=ENTITY_RESOURCE_PACK_PINS.get(identifier),
                )
                if geometry
                else None
            )
            if not geometry or texture_path is None:
                continue

            face = dispatch_render_portrait(
                identifier, texture_path, geometry, resource_packs, models
            )
            if face is None:
                continue

            output_path = output / f"{identifier}.png"
            write_icon(
                face,
                output_path,
                size=ICON_SIZE_OVERRIDES.get(identifier, 64),
                crop_content_bottom=ICON_CROP_CONTENT_BOTTOM.get(identifier, 0),
                offset_x=ICON_OFFSET_X.get(identifier, 0),
            )
            manifest[identifier] = output_path.name
        except OSError:
            continue

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
    try:
        packs = [data_source.resource_pack_root()]
    except Exception:
        packs = [
            DEFAULT_RESOURCE_PACKS_ROOT / "vanilla",
            *sorted(DEFAULT_RESOURCE_PACKS_ROOT.glob("vanilla_*")),
            DEFAULT_RESOURCE_PACKS_ROOT / "chemistry",
        ]
    # Education-mode content (balloon etc.) lives in the local chemistry packs.
    chemistry_root = DEFAULT_RESOURCE_PACKS_ROOT
    for pack in sorted(chemistry_root.glob("chemistry*")):
        if pack.is_dir() and pack not in packs:
            packs.append(pack)
    return packs


def generate(resource_packs: list[Path], output: Path) -> dict[str, str]:
    output.mkdir(parents=True, exist_ok=True)
    for generated in output.glob("*.png"):
        generated.unlink()
    manifest: dict[str, str] = {}
    resource_packs = [path for path in resource_packs if path.is_dir()]
    manifest.update(entity_head_assets(resource_packs, output))

    for identifier in ("falling_block", "item"):
        if identifier in manifest:
            continue
        icon = render_dynamic_entity_icon(identifier, resource_packs)
        if icon is None:
            continue
        output_path = output / f"{identifier}.png"
        write_icon(icon, output_path)
        manifest[identifier] = output_path.name

    models = geometry_index(resource_packs)
    # Entities without their own client definition still render from their
    # shared model instead of item sprites.
    model_fallbacks = {
        "egg": ("geometry.item_sprite", "textures/items/egg"),
        "snowball": ("geometry.item_sprite", "textures/items/snowball"),
        "trident": ("geometry.trident", "textures/entity/trident"),
        "xp_orb": ("geometry.experience_orb", "textures/entity/experience_orb"),
        "leash_knot": ("geometry.leash_knot", "textures/entity/lead_knot"),
    }
    for identifier, (geometry_id, texture_ref) in model_fallbacks.items():
        if identifier in manifest:
            continue
        geometry = get_merged_geometry(models, geometry_id)
        texture_path = texture_file(resource_packs, texture_ref)
        if geometry is None or texture_path is None:
            continue
        face = dispatch_render_portrait(
            identifier, texture_path, geometry, resource_packs, models
        )
        if face is None:
            continue
        output_path = output / f"{identifier}.png"
        write_icon(face, output_path)
        manifest[identifier] = output_path.name

    for identifier, item_name in SPECIAL_ITEMS.items():
        source = texture_file(resource_packs, f"textures/items/{item_name}")
        if source is None:
            source = texture_file(resource_packs, f"textures/{item_name}")
        if source is None:
            continue
        output_path = output / f"{output_name(identifier)}.png"
        write_icon(Image.open(source), output_path)
        manifest[output_name(identifier)] = output_path.name

    for legacy, canonical in {
        "villager_v2": "villager",
    }.items():
        if canonical in manifest:
            source = output / manifest[canonical]
            alias = output / f"{legacy}.png"
            alias.write_bytes(source.read_bytes())
            manifest[legacy] = alias.name

    with (output / "manifest.json").open("w", encoding="utf-8") as file:
        json.dump(dict(sorted(manifest.items())), file, indent=2)
        file.write("\n")
    return manifest


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Generate stable map entity icons from Bedrock vanilla and education packs."
    )
    parser.add_argument("--resource-pack", type=Path, action="append")
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    parser.add_argument("--reference-directory", type=Path)
    arguments = parser.parse_args()
    resource_packs = arguments.resource_pack or default_resource_packs()
    manifest = generate([path.resolve() for path in resource_packs], arguments.output.resolve())
    print(f"generated {len(set(manifest.values()))} PNG entity icons in {arguments.output}")


if __name__ == "__main__":
    main()
