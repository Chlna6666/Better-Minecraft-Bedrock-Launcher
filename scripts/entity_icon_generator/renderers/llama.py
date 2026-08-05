from __future__ import annotations

from pathlib import Path
from PIL import Image
from entity_icon_generator.geometry import cube_face, texture_file
from entity_icon_generator.texture import normalize_entity_texture


def render_llama(
    identifier: str,
    texture: Image.Image,
    geometry: dict,
    resource_packs: list[Path],
) -> Image.Image | None:
    """Standalone 2D Renderer for Llama and Trader Llama.

    - Restricts head/neck cube [8, 18, 6] to upper 8 pixels, cropping out lower neck.
    - For trader_llama, composites base llama texture with trader_llama_decor overlay.
    - Uses float origin rounding for symmetrical pixel alignment.
    """
    texture = normalize_entity_texture(texture)

    if identifier == "trader_llama":
        decor_path = texture_file(
            resource_packs, "textures/entity/llama/decor/trader_llama_decor"
        )
        if decor_path:
            decor_img = normalize_entity_texture(Image.open(decor_path))
            texture.alpha_composite(decor_img)

    faces: list[tuple[float, float, float, int, int, int, Image.Image]] = []
    head_bones = [
        b for b in geometry.get("bones", []) if "head" in b.get("name", "").lower()
    ]
    if not head_bones:
        return None

    for bone in head_bones:
        for cube in bone.get("cubes", []):
            face = cube_face(cube, "north")
            origin = cube.get("origin")
            size = cube.get("size")
            if not (
                face
                and isinstance(origin, list)
                and isinstance(size, list)
                and len(origin) >= 3
                and len(size) >= 3
            ):
                continue
            left, top, w, h = face
            if (
                w <= 0
                or h <= 0
                or left < 0
                or top < 0
                or left + w > texture.width
                or top + h > texture.height
            ):
                continue

            ox = float(origin[0])
            oy = float(origin[1])
            oz = float(origin[2])
            depth = int(size[2])

            # Restrict height of long head/neck cube to upper 8 pixels
            if int(size[0]) == 8 and int(size[1]) == 18:
                h = 8
                oy = oy + 10.0  # Position at top 8 pixels

            face_img = texture.crop((left, top, left + w, top + h))
            faces.append((ox, oy, oz, w, h, depth, face_img))

    if not faces:
        return None

    # Sort faces by Z-depth descending (larger Z back, smaller Z front)
    faces.sort(key=lambda item: item[2], reverse=True)

    min_x = min(f[0] for f in faces)
    max_x = max(f[0] + f[3] for f in faces)
    min_y = min(f[1] for f in faces)
    max_y = max(f[1] + f[4] for f in faces)

    w_canvas = round(max_x - min_x)
    h_canvas = round(max_y - min_y)
    canvas = Image.new("RGBA", (w_canvas, h_canvas))
    for ox, oy, _oz, w, h, _d, face_img in faces:
        resized = face_img.resize((w, h), Image.Resampling.NEAREST)
        canvas.alpha_composite(resized, (round(ox - min_x), round(max_y - oy - h)))

    return canvas
