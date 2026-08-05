from __future__ import annotations

from PIL import Image
from entity_icon_generator.renderers.goat import render_side_entity


def render_armadillo(texture: Image.Image, geometry: dict) -> Image.Image | None:
    """Render armadillo as a side profile matching the game model layout."""
    return render_side_entity(
        texture,
        geometry,
        {"head", "right_ear", "left_ear"},
        {"body", "tail", "right_front_leg", "left_front_leg"},
        crop_to_head=True,
    )
