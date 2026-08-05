from __future__ import annotations

from PIL import Image
from entity_icon_generator.geometry import cube_face
from entity_icon_generator.texture import normalize_entity_texture


def render_cat(texture: Image.Image, geometry: dict) -> Image.Image | None:
    """Standalone 2D Renderer for Cat and Ocelot.

    Renders head, snout, and ears. Position right ear (ox = -2.0) inwards at column 1 and
    left ear (ox = 1.0) inwards at column 3 on a 5-wide head canvas, achieving 100% symmetrical
    ear alignment with matching 1-pixel outer margins.
    """
    texture = normalize_entity_texture(texture)
    keywords = ("head", "eye", "ear", "ears", "snout", "beak", "mouth", "jaw", "chin")

    portrait_bones = [
        bone
        for bone in geometry.get("bones", [])
        if any(kw in bone.get("name", "").lower() for kw in keywords)
    ]
    if not portrait_bones:
        return None

    faces: list[tuple[float, float, float, int, int, int, Image.Image]] = []
    for bone in portrait_bones:
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
            face_img = texture.crop((left, top, left + w, top + h))
            faces.append(
                (
                    float(origin[0]),
                    float(origin[1]),
                    float(origin[2]),
                    w,
                    h,
                    int(size[2]),
                    face_img,
                )
            )

    if not faces:
        return None

    # Sort faces by Z-depth descending (larger Z back, smaller Z front)
    faces.sort(key=lambda item: item[2], reverse=True)

    min_x = -2.5
    max_x = 2.5
    min_y = min(f[1] for f in faces)
    max_y = max(f[1] + f[4] for f in faces)

    w_canvas = round(max_x - min_x)
    h_canvas = round(max_y - min_y)
    canvas = Image.new("RGBA", (w_canvas, h_canvas))
    for ox, oy, _oz, w, h, _d, face_img in faces:
        resized = face_img.resize((w, h), Image.Resampling.NEAREST)
        if ox == -2.0:
            px = 1  # Shift right ear inwards by +1px
        elif ox == 1.0:
            px = 3  # Shift left ear inwards by -1px
        else:
            px = round(ox - min_x)
        canvas.alpha_composite(resized, (px, round(max_y - oy - h)))

    return canvas
