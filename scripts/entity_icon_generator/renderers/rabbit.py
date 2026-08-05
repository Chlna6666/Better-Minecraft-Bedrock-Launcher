from __future__ import annotations

from PIL import Image
from entity_icon_generator.geometry import cube_face
from entity_icon_generator.texture import normalize_entity_texture


def render_rabbit(texture: Image.Image, geometry: dict) -> Image.Image | None:
    """Standalone 2x Subpixel Renderer for Rabbit.

    Renders ONLY head [5, 5, 5] and ears [2, 5, 1], excluding all front and back legs.
    Uses 2x subpixel canvas rendering for 100% symmetrical ear alignment.
    """
    texture = normalize_entity_texture(texture)
    scale = 2
    tex_2x = texture.resize((texture.width * scale, texture.height * scale), Image.Resampling.NEAREST)

    keywords = ("head", "ear", "ears", "nose")

    portrait_bones = [
        bone
        for bone in geometry.get("bones", [])
        if any(kw in bone.get("name", "").lower() for kw in keywords)
    ]
    if not portrait_bones:
        return None

    faces: list[tuple[float, float, float, int, int, int, Image.Image]] = []
    for bone in portrait_bones:
        bname = bone.get("name", "").lower()
        # "rearFoot" contains "ear"; exclude all limb/body bones explicitly.
        if any(part in bname for part in ("leg", "foot", "haunch", "tail", "body")):
            continue

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

            face_img = tex_2x.crop((left * scale, top * scale, (left + w) * scale, (top + h) * scale))
            faces.append(
                (
                    float(origin[0]) * scale,
                    float(origin[1]) * scale,
                    float(origin[2]) * scale,
                    w * scale,
                    h * scale,
                    int(size[2]) * scale,
                    face_img,
                )
            )

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
        canvas.alpha_composite(face_img, (round(ox - min_x), round(max_y - oy - h)))

    return canvas
