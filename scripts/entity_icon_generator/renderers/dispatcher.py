from __future__ import annotations

import copy
from pathlib import Path
from PIL import Image, ImageOps

from entity_icon_generator.renderers.armadillo import render_armadillo
from entity_icon_generator.geometry import (
    FRONT_BODY_ENTITIES,
    HEAD_NECK_PROFILE_ENTITIES,
    SIDE_HEAD_ENTITIES,
    SIDE_PROFILE_ENTITIES,
    texture_file,
)
from entity_icon_generator.renderers.cat import render_cat
from entity_icon_generator.renderers.front_face import render_front_face_2d
from entity_icon_generator.renderers.goat import render_goat
from entity_icon_generator.renderers.llama import render_llama
from entity_icon_generator.renderers.model import render_model_3d
from entity_icon_generator.renderers.parrot import render_parrot
from entity_icon_generator.renderers.rabbit import render_rabbit
from entity_icon_generator.renderers.sheep import render_sheep
from entity_icon_generator.renderers.side_body import render_side_body_2d
from entity_icon_generator.renderers.side_face import render_side_face_2d
from entity_icon_generator.renderers.slime import render_slime
from entity_icon_generator.renderers.standard import (
    render_front_body_profile,
    render_head,
)
from entity_icon_generator.renderers.tropicalfish import tropicalfish_texture
from entity_icon_generator.renderers.villager import (
    VILLAGER_FAMILY_ENTITIES,
    render_villager_family_2d,
)
from entity_icon_generator.renderers.wolf import render_wolf
from entity_icon_generator.texture import normalize_entity_texture

MODEL_RENDER_3D = {
    "armor_stand": ("north", None, None, 0.5, None, None, False),
    "egg": ("north", None, None, 0.5, None, None, False),
    "skull": ("north", None, None, 0.5, None, None, False),
    "snowball": ("north", None, None, 0.5, None, None, False),
    "thrown_trident": ("north", None, None, 0.5, None, None, False),
    "trident": ("north", None, None, 0.5, None, None, False),
    "armadillo": (
        "east",
        {
            "head",
            "right_ear",
            "left_ear",
            "body",
            "tail",
            "right_front_leg",
            "left_front_leg",
        },
        {"head"},
        0.6,
        None,
        None,
        False,
    ),
    "elder_guardian": (
        "north",
        {
            "head",
            "eye",
            "spikepart0",
            "spikepart1",
            "spikepart2",
            "spikepart3",
            "spikepart4",
            "spikepart5",
            "spikepart6",
            "spikepart7",
        },
        None,
        0.5,
        {
            "spikepart0",
            "spikepart1",
            "spikepart2",
            "spikepart3",
            "spikepart4",
            "spikepart5",
            "spikepart6",
            "spikepart7",
        },
        {
            "spikepart0",
            "spikepart1",
            "spikepart2",
            "spikepart3",
            "spikepart4",
            "spikepart5",
            "spikepart6",
            "spikepart7",
        },
        True,
    ),
    "evocation_fang": ("east", None, None, 0.5, None, None, False),
    "ghast": ("north", {"body"}, None, 0.5, None, None, False),
    "guardian": (
        "north",
        {
            "head",
            "eye",
            "spikepart0",
            "spikepart1",
            "spikepart2",
            "spikepart3",
            "spikepart4",
            "spikepart5",
            "spikepart6",
            "spikepart7",
        },
        None,
        0.5,
        {
            "spikepart0",
            "spikepart1",
            "spikepart2",
            "spikepart3",
            "spikepart4",
            "spikepart5",
            "spikepart6",
            "spikepart7",
        },
        {
            "spikepart0",
            "spikepart1",
            "spikepart2",
            "spikepart3",
            "spikepart4",
            "spikepart5",
            "spikepart6",
            "spikepart7",
        },
        True,
    ),
    "happy_ghast": ("north", {"body"}, None, 0.5, None, None, False),
    "nautilus": ("east", None, None, 0.5, None, None, False),
    "zombie_nautilus": ("east", None, None, 0.5, None, None, False),
}


GUARDIAN_SPIKE_ROTATIONS = {
    "spikepart0": [0, 0, 0],
    "spikepart1": [0, 0, 0],
    "spikepart2": [0, 0, 90],
    "spikepart3": [0, 0, -90],
    "spikepart4": [0, 0, 45],
    "spikepart5": [0, 0, -45],
    "spikepart6": [0, 0, -45],
    "spikepart7": [0, 0, 45],
}


GUARDIAN_SPIKE_POSITIONS = {
    "spikepart0": [0, 5.5, 0],
    "spikepart1": [0, -11.0, 0],
    "spikepart2": [-4.5, 10.0, 0],
    "spikepart3": [4.5, 10.0, 0],
    "spikepart4": [-3.0, 11.0, 0],
    "spikepart5": [3.0, 11.0, 0],
    "spikepart6": [3.0, -13.5, 0],
    "spikepart7": [-3.0, -13.5, 0],
}

def _apply_guardian_pose(geometry: dict) -> dict:
    posed = copy.deepcopy(geometry)
    for bone in posed.get("bones", []):
        rotation = GUARDIAN_SPIKE_ROTATIONS.get(bone.get("name"))
        position = GUARDIAN_SPIKE_POSITIONS.get(bone.get("name"))
        if rotation is not None:
            bone["rotation"] = rotation
        if position is not None:
            bone["position"] = position
        if bone.get("name") in GUARDIAN_SPIKE_ROTATIONS:
            for cube in bone.get("cubes", []):
                origin = list(cube.get("origin") or [0, 0, 0])
                size = list(cube.get("size") or [0, 0, 0])
                size[1] = size[1] * 0.5
                cube["origin"] = origin
                cube["size"] = size
        if bone.get("name") == "head":
            for cube in bone.get("cubes", []):
                origin = list(cube.get("origin") or [0, 0, 0])
                origin[1] += 12
                origin[0] -= 0.5
                cube["origin"] = origin
        elif bone.get("name") == "eye":
            for cube in bone.get("cubes", []):
                origin = list(cube.get("origin") or [0, 0, 0])
                origin[1] += 12
                origin[0] -= 0.5
                cube["origin"] = origin
    return posed


def _apply_fang_pose(geometry: dict) -> dict:
    posed = copy.deepcopy(geometry)
    for bone in posed.get("bones", []):
        if bone.get("name") == "upper_jaw":
            bone["rotation"] = [0, 0, 160]
        elif bone.get("name") == "lower_jaw":
            bone["rotation"] = [0, 0, -160]
    return posed


def render_camel_head_3d(texture, geometry) -> Image.Image | None:
    """Side 3D portrait: full head plus a short piece of neck."""
    import copy as _copy

    geometry = _copy.deepcopy(geometry)
    for bone in geometry.get("bones", []):
        name = bone.get("name")
        if name in {"left_ear", "right_ear"}:
            for cube in bone.get("cubes", []):
                cube["size"] = [4, 5, 2]
                cube["origin"] = [
                    (cube.get("origin") or [0, 0, 0])[0],
                    40,
                    -26 if name == "left_ear" else -18,
                ]
    result = render_model_3d(
        texture,
        geometry,
        view="east",
        bone_filter={"head", "right_ear"},
    )
    if result is None:
        return None
    return result


def render_horse_family_head_3d(
    texture, geometry, identifier: str
) -> Image.Image | None:
    """Side 3D portrait for horse/donkey/mule: head plus a short neck."""
    ear = "MuleEarL" if identifier in {"donkey", "mule"} else "EarL"
    result = render_model_3d(
        texture,
        geometry,
        view="east",
        bone_filter={"Head", "Neck", "Muzzle", "Mane", ear},
    )
    if result is None:
        return None
    width, height = result.size
    crop_height = max(1, round(height * 0.85))
    return result.crop((0, 0, width, crop_height))


MINECART_MODEL_ENTITIES = {
    "minecart",
    "chest_minecart",
    "hopper_minecart",
    "furnace_minecart",
    "spawner_minecart",
    "command_block_minecart",
    "tnt_minecart",
}


def render_minecart_side(texture, geometry) -> Image.Image | None:
    """Side 3D portrait for minecarts using the entity model, not item art."""
    return render_model_3d(texture, geometry, view="east")


def dispatch_render_portrait(
    identifier: str,
    texture_path: Path,
    geometry: dict,
    resource_packs: list[Path],
    models: dict[str, dict],
) -> Image.Image | None:
    """Central router for 4 top-level renderer categories and variants.

    Categories:
    1. Front Face (正脸) - Default baseline category (render_front_face_2d)
    2. Side Face (侧脸) - (render_side_face_2d)
    3. Side Face + Body (侧脸加身体) - (render_side_body_2d)
    4. Items (物品) - Handled separately by render_item
    Subcategories / Standalone Variants:
    1.1 Villager Family 2D - (render_villager_family_2d)
    1.2 Slime Standalone - (render_slime)
    1.3 Llama & Trader Llama Standalone - (render_llama)
    1.4 Rabbit Standalone - (render_rabbit)
    1.5 Wolf Standalone (2x Subpixel) - (render_wolf)
    1.6 Cat & Ocelot Standalone - (render_cat)
    """
    texture = (
        tropicalfish_texture(resource_packs, texture_path, geometry)
        if identifier == "tropicalfish"
        else normalize_entity_texture(
            Image.open(texture_path),
            preserve_low_alpha=identifier in {"sheep", "spider", "cave_spider", "enderman"},
            force_opaque=identifier in {"blaze", "glow_squid"},
        )
    )
    # Zombie horse uses a 64x64 texture while the horse geometry expects the
    # 128x128 layout; upscale it so UVs line up like the normal horse.
    if identifier == "zombie_horse" and texture.size == (64, 64):
        texture = texture.resize((128, 128), Image.Resampling.NEAREST)

    # Hoglin/zoglin: south face is the real front; mirror back to keep the
    # face from being flipped.
    if identifier in {"hoglin", "zoglin"}:
        result = render_model_3d(
            texture, geometry, view="south", bone_filter={"head"}
        )
        if result is not None:
            return ImageOps.mirror(result)

    # Guardian spikes fan out via animation; bake the fixed setup pose.
    if identifier in {"guardian", "elder_guardian"}:
        geometry = _apply_guardian_pose(geometry)

    # Evoker fang jaws open during the bite animation; bake the side profile.
    if identifier == "evocation_fang":
        geometry = _apply_fang_pose(geometry)

    # XP orbs are a fixed yellow-green glow in the icon.
    if identifier == "xp_orb":
        result = render_model_3d(texture, geometry, view="south")
        if result is None:
            return None
        gray = result.convert("L")
        colored = ImageOps.colorize(gray, black=(0, 0, 0), white=(170, 255, 0))
        colored.putalpha(result.getchannel("A"))
        return colored

    # Education balloons are a flat sprite tinted red by default.
    if identifier == "balloon":
        item_texture = texture_file(resource_packs, "textures/items/balloon")
        if item_texture is not None:
            texture = normalize_entity_texture(Image.open(item_texture))
        sprite = texture.crop(texture.getchannel("A").getbbox())
        gray = sprite.convert("L")
        colored = ImageOps.colorize(gray, black=(0, 0, 0), white=(220, 40, 40))
        colored.putalpha(sprite.getchannel("A"))
        return colored

    # Minecarts render from their entity model in side view.
    if identifier in MINECART_MODEL_ENTITIES:
        return render_minecart_side(texture, geometry)

    # Assembled 3D model projection for models with bone rotations/hierarchy.
    if identifier in MODEL_RENDER_3D:
        view, bone_filter, focus_bones, focus_ratio, double_sided, front, pad_square = MODEL_RENDER_3D[identifier]
        return render_model_3d(
            texture,
            geometry,
            view=view,
            bone_filter=bone_filter,
            focus_bones=focus_bones,
            focus_ratio=focus_ratio,
            double_sided_bones=double_sided,
            front_bones=front,
            pad_square=pad_square,
        )

    # Standalone Variant: Armadillo
    if identifier == "armadillo":
        return render_armadillo(texture, geometry)

    # Standalone Variant: Slime
    if identifier == "slime":
        return render_slime(texture, geometry, models)

    # Standalone Variant: Wolf
    if identifier == "wolf":
        return render_wolf(texture, geometry)

    # Standalone Variant: Cat & Ocelot
    if identifier in {"cat", "ocelot"}:
        return render_cat(texture, geometry)

    # Standalone Variant: Llama / Trader Llama
    if identifier in {"llama", "trader_llama"}:
        return render_llama(identifier, texture, geometry, resource_packs)

    # Standalone Variant: Rabbit
    if identifier == "rabbit":
        return render_rabbit(texture, geometry)

    # Standalone Variant: Sheep
    if identifier == "sheep":
        return render_sheep(texture)

    # Standalone Variant: Goat
    if identifier == "goat":
        return render_goat(texture, geometry)

    # Standalone Variant: Parrot
    if identifier == "parrot":
        return render_parrot(texture, geometry)

    # Standalone Variant: Nautilus
    if identifier in {"nautilus", "zombie_nautilus"}:
        return render_nautilus(texture, geometry)

    # Subcategory Variant: Villager Family
    if identifier in VILLAGER_FAMILY_ENTITIES:
        return render_villager_family_2d(texture, geometry, models)

    # Category 3: Side Face + Body
    if identifier in {"camel", "camel_husk"}:
        return render_camel_head_3d(texture, geometry)

    if identifier in SIDE_PROFILE_ENTITIES or identifier in HEAD_NECK_PROFILE_ENTITIES:
        return render_side_body_2d(identifier, texture, geometry)

    if identifier in FRONT_BODY_ENTITIES:
        return render_front_body_profile(texture, geometry)

    # Category 2: Side Face
    if identifier in SIDE_HEAD_ENTITIES:
        return render_side_face_2d(identifier, texture, geometry)

    # Category 1: Front Face (Default baseline strategy for all entity portraits)
    front_portrait = render_front_face_2d(
        identifier,
        texture,
        geometry,
        resource_packs,
        models,
        preserve_low_alpha=identifier in {"sheep", "spider", "cave_spider", "enderman"},
        force_opaque=identifier in {"blaze", "glow_squid"},
    )
    if front_portrait is not None:
        return front_portrait

    # Fallback to standard head renderer if front face extraction produced no faces
    return render_head(texture, geometry, "north")
