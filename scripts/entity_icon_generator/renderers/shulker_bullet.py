from __future__ import annotations

from PIL import Image

from entity_icon_generator.renderers.model import render_model_3d


def render_shulker_bullet(texture: Image.Image, geometry: dict) -> Image.Image | None:
    return render_model_3d(
        texture,
        geometry,
        view="north",
        bone_filter={"body"},
        double_sided_bones={"body"},
        pad_square=True,
    )
