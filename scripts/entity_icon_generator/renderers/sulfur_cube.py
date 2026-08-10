from __future__ import annotations

from pathlib import Path

from PIL import Image

from entity_icon_generator.geometry import texture_file
from entity_icon_generator.renderers.model import render_model_3d
from entity_icon_generator.texture import normalize_entity_texture


def render_sulfur_cube(
    inner_texture: Image.Image,
    geometry: dict,
    resource_packs: list[Path],
) -> Image.Image | None:
    inner = render_model_3d(inner_texture, geometry, view="north")
    outer_path = texture_file(
        resource_packs,
        "textures/entity/sulfur_cube/sulfur_cube_outer",
    )
    outer = (
        render_model_3d(
            normalize_entity_texture(Image.open(outer_path), preserve_low_alpha=True),
            geometry,
            view="north",
        )
        if outer_path is not None
        else None
    )
    layers = [layer for layer in (inner, outer) if layer is not None]
    if not layers:
        return None
    width = max(layer.width for layer in layers)
    height = max(layer.height for layer in layers)
    result = Image.new("RGBA", (width, height))
    for layer in layers:
        result.alpha_composite(
            layer,
            ((width - layer.width) // 2, (height - layer.height) // 2),
        )
    return result
