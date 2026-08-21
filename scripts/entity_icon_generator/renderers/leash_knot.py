from __future__ import annotations

from pathlib import Path
from PIL import Image

from entity_icon_generator.geometry import cube_face, texture_file
from entity_icon_generator.texture import normalize_entity_texture


def render_leash_knot(
    texture: Image.Image,
    geometry: dict,
    resource_packs: list[Path],
) -> Image.Image | None:
    """Render a standalone oak fence post with a leash knot composited at its center.

    - Standalone oak fence post: 4x16 voxels, front face from planks_oak.
    - Leash knot: 6x8 voxels, front face from geometry.leash_knot (knot cube).
    - Composited with the leash knot centered directly on the fence post.
    """
    knot_face_img = None
    declared_width = int((geometry.get("description") or {}).get("texture_width") or 32)
    declared_height = int((geometry.get("description") or {}).get("texture_height") or 32)
    uv_scale_x = texture.width / declared_width if declared_width else 1.0
    uv_scale_y = texture.height / declared_height if declared_height else 1.0

    for bone in geometry.get("bones", []):
        if "knot" not in bone.get("name", "").lower():
            continue
        for cube in bone.get("cubes", []):
            face = cube_face(cube, "north")
            if face is None:
                continue
            left, top, width, height = face
            left = round(left * uv_scale_x)
            top = round(top * uv_scale_y)
            width = round(width * uv_scale_x)
            height = round(height * uv_scale_y)
            if (
                width <= 0
                or height <= 0
                or left < 0
                or top < 0
                or left + width > texture.width
                or top + height > texture.height
            ):
                continue
            face_img = texture.crop((left, top, left + width, top + height))
            if face_img.getchannel("A").getbbox() is not None:
                knot_face_img = face_img
                break
        if knot_face_img is not None:
            break

    if knot_face_img is None:
        left = round(6 * uv_scale_x)
        top = round(6 * uv_scale_y)
        width = round(6 * uv_scale_x)
        height = round(8 * uv_scale_y)
        knot_face_img = texture.crop((left, top, left + width, top + height))

    planks_path = texture_file(resource_packs, "textures/blocks/planks_oak")
    if planks_path is None:
        planks_path = texture_file(resource_packs, "textures/blocks/planks")

    if planks_path is not None:
        planks_tex = normalize_entity_texture(Image.open(planks_path))
        p_uv_x = planks_tex.width / 16.0
        p_uv_y = planks_tex.height / 16.0
        fence_crop = planks_tex.crop((
            round(6 * p_uv_x),
            0,
            round(10 * p_uv_x),
            planks_tex.height,
        ))
    else:
        fence_crop = Image.new("RGBA", (4, 16), (162, 130, 78, 255))

    knot_unit_x = knot_face_img.width / 6.0
    knot_unit_y = knot_face_img.height / 8.0
    fence_unit_x = fence_crop.width / 4.0
    fence_unit_y = fence_crop.height / 16.0

    unit_scale = max(1, round(max(knot_unit_x, knot_unit_y, fence_unit_x, fence_unit_y)))

    canvas_w = 6 * unit_scale
    canvas_h = 16 * unit_scale
    canvas = Image.new("RGBA", (canvas_w, canvas_h))

    fence_w = 4 * unit_scale
    fence_h = 16 * unit_scale
    fence_resized = fence_crop.resize((fence_w, fence_h), Image.Resampling.NEAREST)
    fence_x = (canvas_w - fence_w) // 2
    fence_y = 0
    canvas.alpha_composite(fence_resized, (fence_x, fence_y))

    knot_w = 6 * unit_scale
    knot_h = 8 * unit_scale
    knot_resized = knot_face_img.resize((knot_w, knot_h), Image.Resampling.NEAREST)
    knot_x = (canvas_w - knot_w) // 2
    knot_y = (canvas_h - knot_h) // 2
    canvas.alpha_composite(knot_resized, (knot_x, knot_y))

    return canvas
