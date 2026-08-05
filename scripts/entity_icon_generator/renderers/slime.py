from __future__ import annotations

from PIL import Image
from entity_icon_generator.geometry import get_merged_geometry
from entity_icon_generator.texture import normalize_entity_texture


def render_slime(
    texture: Image.Image, geometry: dict, models: dict[str, dict]
) -> Image.Image | None:
    """Standalone 2D Renderer for Slime.

    Composites outer semi-transparent green body cube (geometry.slime.armor at UV [0,0]),
    inner core cube (UV [0,16]), and facial features (eyes/mouth).
    """
    texture = normalize_entity_texture(texture)

    outer_geom = get_merged_geometry(models, "geometry.slime.armor") or geometry
    outer_cube = None
    for bone in outer_geom.get("bones", []):
        if bone.get("name") == "cube":
            cubes = bone.get("cubes", [])
            if cubes:
                outer_cube = cubes[0]
                break

    inner_cube = None
    eye0_cube = None
    eye1_cube = None
    mouth_cube = None
    for bone in geometry.get("bones", []):
        name = bone.get("name", "")
        cubes = bone.get("cubes", [])
        if not cubes:
            continue
        if name == "cube":
            inner_cube = cubes[0]
        elif name == "eye0":
            eye0_cube = cubes[0]
        elif name == "eye1":
            eye1_cube = cubes[0]
        elif name == "mouth":
            mouth_cube = cubes[0]

    # Render flat 8x8 canvas
    canvas = Image.new("RGBA", (8, 8))

    # 1. Outer semi-transparent body
    if outer_cube and texture.width >= 16 and texture.height >= 16:
        outer_img = texture.crop((8, 8, 16, 16))
        canvas.alpha_composite(outer_img, (0, 0))

    # 2. Inner core
    if inner_cube and texture.width >= 12 and texture.height >= 28:
        inner_img = texture.crop((6, 22, 12, 28))
        canvas.alpha_composite(inner_img, (1, 1))

    # 3. Eyes & Mouth
    if eye0_cube and texture.width >= 36 and texture.height >= 4:
        eye0_img = texture.crop((34, 2, 36, 4))
        canvas.alpha_composite(eye0_img, (1, 2))

    if eye1_cube and texture.width >= 36 and texture.height >= 8:
        eye1_img = texture.crop((34, 6, 36, 8))
        canvas.alpha_composite(eye1_img, (5, 2))

    if mouth_cube and texture.width >= 34 and texture.height >= 10:
        mouth_img = texture.crop((33, 9, 34, 10))
        canvas.alpha_composite(mouth_img, (3, 4))

    return canvas
