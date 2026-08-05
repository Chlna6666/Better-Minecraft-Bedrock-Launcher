from __future__ import annotations

from PIL import Image
from entity_icon_generator.geometry import HEAD_NECK_PROFILE_ENTITIES
from entity_icon_generator.renderers.standard import (
    render_head_neck_profile,
    render_side_profile,
)


def render_side_body_2d(
    identifier: str, texture: Image.Image, geometry: dict
) -> Image.Image | None:
    """Category 3: Side Face + Body (侧脸加身体) 2D Renderer.

    Renders side profile including body, or head & neck portrait box for quadrupeds
    and aquatic entities.
    """
    if identifier in HEAD_NECK_PROFILE_ENTITIES:
        return render_head_neck_profile(texture, geometry, identifier)

    return render_side_profile(texture, geometry)
