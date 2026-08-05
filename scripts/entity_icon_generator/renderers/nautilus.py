from __future__ import annotations

from PIL import Image
from entity_icon_generator.geometry import cube_face


def render_nautilus(texture: Image.Image, geometry: dict) -> Image.Image | None:
    """Side profile of the shell with mouth/tentacle faces drawn on top."""
    texture = texture.convert("RGBA")
    faces: list[tuple[int, int, int, int, int, Image.Image, bool]] = []
    for bone in geometry.get("bones", []):
        name = bone.get("name", "")
        is_mouth = "mouth" in name.lower()
        for cube in bone.get("cubes", []):
            origin = cube.get("origin")
            size = cube.get("size")
            if not (isinstance(origin, list) and isinstance(size, list)):
                continue
            for direction in ("east", "west", "north", "south"):
                face = cube_face(cube, direction)
                if face is None:
                    continue
                left, top, width, height = face
                if (
                    width <= 0
                    or height <= 0
                    or left < 0
                    or top < 0
                    or left + width > texture.width
                    or top + height > texture.height
                ):
                    continue
                face_image = texture.crop(
                    (left, top, left + width, top + height)
                )
                if face_image.getchannel("A").getbbox() is None:
                    continue
                faces.append(
                    (
                        int(origin[0]) + int(size[0]),
                        int(origin[1]) + (-6 if is_mouth else 0),
                        int(origin[2]),
                        width,
                        height,
                        face_image,
                        is_mouth,
                    )
                )
                break

    if not faces:
        return None
    min_z = min(face[2] for face in faces)
    max_y = max(face[1] + face[4] for face in faces)
    min_y = min(face[1] for face in faces)
    canvas_width = max(face[2] + face[3] for face in faces) - min_z
    canvas_height = max_y - min_y
    canvas = Image.new("RGBA", (canvas_width, canvas_height))

    body_faces = [face for face in faces if not face[6]]
    mouth_faces = [face for face in faces if face[6]]
    for group in (body_faces, mouth_faces):
        for origin_x, origin_y, origin_z, width, height, face_image, _ in sorted(
            group, key=lambda face: face[0]
        ):
            canvas.alpha_composite(
                face_image,
                (origin_z - min_z, max_y - origin_y - height),
            )
    return canvas
