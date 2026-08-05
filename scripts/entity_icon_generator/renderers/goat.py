from __future__ import annotations

from PIL import Image
from entity_icon_generator.geometry import cube_face


def render_side_entity(
    texture: Image.Image,
    geometry: dict,
    head_bone_names: set[str],
    body_bone_names: set[str],
    head_directions: tuple[str, ...] = ("east", "west", "north", "south"),
    crop_to_head: bool = True,
) -> Image.Image | None:
    """Model-position side projection, cropped so the head stays proportionate."""

    def collect_faces(
        bone_names: set[str], directions: tuple[str, ...]
    ) -> list[tuple]:
        faces: list[tuple] = []
        for bone in geometry.get("bones", []):
            if bone.get("name") not in bone_names:
                continue
            for cube in bone.get("cubes", []):
                origin = cube.get("origin")
                size = cube.get("size")
                if not (isinstance(origin, list) and isinstance(size, list)):
                    continue
                for direction in directions:
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
                            int(origin[1]),
                            int(origin[2]),
                            width,
                            height,
                            face_image,
                        )
                    )
                    break
        return faces

    head_faces = collect_faces(head_bone_names, head_directions)
    body_faces = collect_faces(body_bone_names, ("east", "west", "north", "south"))
    if not head_faces or not body_faces:
        return None

    all_faces = [*head_faces, *body_faces]
    min_z = min(face[2] for face in all_faces)
    max_y = max(face[1] + face[4] for face in all_faces)
    min_y = min(face[1] for face in all_faces)
    canvas_width = max(face[2] + face[3] for face in all_faces) - min_z
    canvas_height = max_y - min_y
    canvas = Image.new("RGBA", (canvas_width, canvas_height))
    # Body first, head on top, matching the game model hierarchy.
    for origin_x, origin_y, origin_z, width, height, face_image in sorted(
        body_faces, key=lambda face: face[0]
    ):
        canvas.alpha_composite(
            face_image,
            (origin_z - min_z, max_y - origin_y - height),
        )
    # Small head decorations (horns/ears) may overlap the main head in the
    # flattened projection; draw the largest head face last so eyes stay visible.
    for origin_x, origin_y, origin_z, width, height, face_image in sorted(
        head_faces, key=lambda face: face[3] * face[4]
    ):
        canvas.alpha_composite(
            face_image,
            (origin_z - min_z, max_y - origin_y - height),
        )

    if not crop_to_head:
        return canvas
    head_min_x = min(face[2] for face in head_faces) - min_z
    head_max_x = max(face[2] + face[3] for face in head_faces) - min_z
    head_min_y = min(max_y - face[1] - face[4] for face in head_faces)
    head_max_y = max(max_y - face[1] for face in head_faces)
    head_height = head_max_y - head_min_y
    if head_height <= 0:
        return canvas

    # Keep the full head plus only the body area immediately beside it.
    crop_left = head_min_x
    head_width = head_max_x - head_min_x
    crop_right = min(canvas_width, head_max_x + head_width)
    crop_top = head_min_y
    crop_bottom = min(canvas_height, head_max_y + head_height)
    if crop_right <= crop_left or crop_bottom <= crop_top:
        return canvas
    return canvas.crop((crop_left, crop_top, crop_right, crop_bottom))


def render_goat(texture: Image.Image, geometry: dict) -> Image.Image | None:
    """Render goat as a side profile matching the game model layout."""
    goat_geometry = dict(geometry)
    adjusted_bones = []
    for bone in geometry.get("bones", []):
        if bone.get("name") != "head":
            adjusted_bones.append(bone)
            continue
        head_bone = dict(bone)
        cubes = []
        for cube in bone.get("cubes", []):
            size = cube.get("size")
            if (
                isinstance(size, list)
                and len(size) >= 3
                and size[0] == 0
                and size[1] == 7
            ):
                cube = dict(cube)
                origin = list(cube.get("origin", [0, 0, 0]))
                origin[1] += 2
                cube["origin"] = origin
            cubes.append(cube)
        head_bone["cubes"] = cubes
        adjusted_bones.append(head_bone)
    goat_geometry["bones"] = adjusted_bones
    return render_side_entity(
        texture,
        goat_geometry,
        {"head", "right_horn", "left_horn"},
        {"body", "right_front_leg", "left_front_leg"},
    )
