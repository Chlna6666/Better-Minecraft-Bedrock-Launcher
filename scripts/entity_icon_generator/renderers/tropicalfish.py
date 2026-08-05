from __future__ import annotations

from pathlib import Path
from PIL import Image
from entity_icon_generator.geometry import geometry_texture_size, texture_file
from entity_icon_generator.texture import colorize_entity_texture


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
