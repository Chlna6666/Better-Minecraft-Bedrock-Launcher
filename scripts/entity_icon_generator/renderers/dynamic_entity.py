from __future__ import annotations

from pathlib import Path

from PIL import Image, ImageOps

from entity_icon_generator.geometry import texture_file


def _load_texture(resource_packs: list[Path], texture_ref: str) -> Image.Image | None:
    texture_path = texture_file(resource_packs, texture_ref)
    if texture_path is None:
        return None
    with Image.open(texture_path) as texture:
        return texture.convert("RGBA")


def render_falling_block(resource_packs: list[Path]) -> Image.Image | None:
    return _load_texture(resource_packs, "textures/blocks/sand")


def render_tnt(resource_packs: list[Path]) -> Image.Image | None:
    return _load_texture(resource_packs, "textures/blocks/tnt_side")


def render_painting(resource_packs: list[Path]) -> Image.Image | None:
    return _load_texture(resource_packs, "textures/items/painting")


def render_dropped_item(resource_packs: list[Path]) -> Image.Image | None:
    tool_specs = (
        ("textures/items/diamond_pickaxe", False),
        ("textures/items/iron_shovel", True),
    )
    canvas = Image.new("RGBA", (64, 64))
    rendered_tools = 0
    for texture_ref, mirror in tool_specs:
        texture = _load_texture(resource_packs, texture_ref)
        if texture is None:
            continue
        texture = texture.resize((48, 48), Image.Resampling.NEAREST)
        if mirror:
            texture = ImageOps.mirror(texture)
        canvas.alpha_composite(texture, (8, 8))
        rendered_tools += 1
    return canvas if rendered_tools else None


def render_dynamic_entity_icon(
    identifier: str, resource_packs: list[Path]
) -> Image.Image | None:
    if identifier == "falling_block":
        return render_falling_block(resource_packs)
    if identifier == "item":
        return render_dropped_item(resource_packs)
    if identifier == "tnt":
        return render_tnt(resource_packs)
    if identifier == "painting":
        return render_painting(resource_packs)
    return None
