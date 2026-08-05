from __future__ import annotations

from PIL import Image
from entity_icon_generator.geometry import SIDE_HEAD_DIRECTIONS
from entity_icon_generator.renderers.standard import render_head


def render_side_face_2d(
    identifier: str, texture: Image.Image, geometry: dict
) -> Image.Image | None:
    """Category 2: Side Face (侧脸) 2D Renderer.

    Renders side head profile views (east/west UV faces) for entities best identified
    from side head perspective (armadillo, sniffer, turtle, vex, sheep).
    """
    direction = SIDE_HEAD_DIRECTIONS.get(identifier, "east")
    return render_head(texture, geometry, direction)
