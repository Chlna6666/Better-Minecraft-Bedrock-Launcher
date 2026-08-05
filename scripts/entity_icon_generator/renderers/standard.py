from __future__ import annotations

from PIL import Image
from entity_icon_generator.geometry import (
    HEAD_NECK_TARGET_SIZES,
    cube_face,
    portrait_cubes,
)


def render_head(
    texture: Image.Image, geometry: dict, direction: str = "north"
) -> Image.Image | None:
    texture = texture.convert("RGBA")
    faces: list[tuple[int, int, int, int, int, int, Image.Image, dict]] = []
    for cube in portrait_cubes(geometry):
        face = cube_face(cube, direction)
        origin = cube.get("origin")
        size = cube.get("size")
        if not (face and isinstance(origin, list) and isinstance(size, list) and len(origin) >= 3):
            continue
        left, top, width, height = face
        if left < 0 or top < 0 or left + width > texture.width or top + height > texture.height:
            continue
        faces.append(
            (
                int(origin[0]),
                int(origin[1]),
                int(origin[2]),
                width,
                height,
                int(size[2]),
                texture.crop((left, top, left + width, top + height)),
                cube,
            )
        )
    if not faces:
        return None
    horizontal_origins = [face[0] if direction == "north" else face[2] for face in faces]
    min_x = min(horizontal_origins)
    max_x = max(origin + face[3] for origin, face in zip(horizontal_origins, faces))
    min_y = min(face[1] for face in faces)
    max_y = max(face[1] + face[4] for face in faces)
    canvas = Image.new("RGBA", (max_x - min_x, max_y - min_y))
    for origin_x, origin_y, origin_z, width, height, _depth, face, _cube in sorted(
        faces, key=lambda item: item[2]
    ):
        resized = face.resize((width, height), Image.Resampling.NEAREST)
        horizontal_origin = origin_x if direction == "north" else origin_z
        canvas.alpha_composite(
            resized,
            (horizontal_origin - min_x, max_y - origin_y - height),
        )
    return canvas


def render_villager_front(texture: Image.Image, geometry: dict) -> Image.Image | None:
    """Render the square front face represented by the Wiki villager sprites."""
    head = next(
        (bone for bone in geometry.get("bones", []) if "head" in bone.get("name", "").lower()),
        None,
    )
    if not head:
        return None
    head_cube = next(
        (
            cube
            for cube in head.get("cubes", [])
            if isinstance(cube.get("size"), list)
            and len(cube["size"]) >= 3
            and int(cube["size"][0]) >= 8
            and int(cube["size"][1]) >= 8
        ),
        None,
    )
    if head_cube is None:
        return None
    face = cube_face(head_cube, "north")
    if face is None:
        return None
    left, top, width, height = face
    if left < 0 or top < 0 or left + width > texture.width or top + height > texture.height:
        return None
    source = texture.convert("RGBA").crop((left, top, left + width, top + height))
    portrait_height = min(8, source.height)
    return source.crop((0, source.height - portrait_height, source.width, source.height))


def render_side_profile(texture: Image.Image, geometry: dict) -> Image.Image | None:
    texture = texture.convert("RGBA")
    faces: list[tuple[int, int, int, int, int, Image.Image]] = []
    for bone in geometry.get("bones", []):
        for cube in bone.get("cubes", []):
            face = None
            face_image = None
            for direction in ("east", "west", "north", "south"):
                candidate = cube_face(cube, direction)
                if candidate is None:
                    continue
                left, top, width, height = candidate
                if (
                    width <= 0
                    or height <= 0
                    or left < 0
                    or top < 0
                    or left + width > texture.width
                    or top + height > texture.height
                ):
                    continue
                candidate_image = texture.crop((left, top, left + width, top + height))
                if candidate_image.getchannel("A").getbbox() is None:
                    continue
                face = candidate
                face_image = candidate_image
                break
            origin = cube.get("origin")
            size = cube.get("size")
            if not (
                face
                and face_image is not None
                and isinstance(origin, list)
                and isinstance(size, list)
                and len(origin) >= 3
                and len(size) >= 3
            ):
                continue
            left, top, width, height = face
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
    if not faces:
        return None
    min_z = min(face[2] for face in faces)
    max_z = max(face[2] + face[3] for face in faces)
    min_y = min(face[1] for face in faces)
    max_y = max(face[1] + face[4] for face in faces)
    canvas = Image.new("RGBA", (max_z - min_z, max_y - min_y))
    for _depth, origin_y, origin_z, _width, height, face in sorted(
        faces,
        key=lambda item: (item[0], item[1], item[2], item[3], item[4]),
    ):
        canvas.alpha_composite(face, (origin_z - min_z, max_y - origin_y - height))
    return canvas


def render_head_neck_profile(
    texture: Image.Image, geometry: dict, identifier: str
) -> Image.Image | None:
    """Render a single-ear head and neck without saddle, chest, or body bones."""
    bones = geometry.get("bones", [])
    head = next((bone for bone in bones if "head" in bone.get("name", "").lower()), None)
    if head is None:
        return None
    head_name = head.get("name")
    selected: list[dict] = [head]
    selected_names = {head_name}
    neck = next(
        (
            bone
            for bone in bones
            if "neck" in bone.get("name", "").lower() and bone.get("cubes")
        ),
        None,
    )
    if neck is not None and neck.get("name") not in selected_names:
        selected.append(neck)
        selected_names.add(neck.get("name"))

    ear_bones = [
        bone
        for bone in bones
        if "ear" in bone.get("name", "").lower() and bone.get("cubes")
    ]
    if ear_bones:
        if identifier in {"donkey", "mule"}:
            ear = max(
                ear_bones,
                key=lambda bone: max(
                    (int(cube.get("size", [0, 0, 0])[1]) for cube in bone.get("cubes", [])),
                    default=0,
                ),
            )
        else:
            ear = ear_bones[0]
        selected.append(ear)

    for bone in bones:
        name = bone.get("name", "").lower()
        if bone in selected or bone in ear_bones:
            continue
        if bone.get("parent") in selected_names and any(
            part in name for part in ("bridle", "mouth", "muzzle", "nose", "snout")
        ):
            selected.append(bone)

    filtered_geometry = dict(geometry)
    filtered_geometry["bones"] = selected
    profile = render_side_profile(texture, filtered_geometry)
    target_size = HEAD_NECK_TARGET_SIZES.get(identifier)
    if profile is not None and target_size is not None:
        # Keep the full head and only a short neck stub so the portrait is not
        # dominated by the long neck, and scale proportionally without warping.
        crop_height = max(1, round(profile.height * 0.7))
        profile = profile.crop((0, 0, profile.width, crop_height))
        scale = min(target_size[0] / profile.width, target_size[1] / profile.height)
        if scale < 1:
            profile = profile.resize(
                (
                    max(1, round(profile.width * scale)),
                    max(1, round(profile.height * scale)),
                ),
                Image.Resampling.NEAREST,
            )
    return profile


def render_front_body_profile(texture: Image.Image, geometry: dict) -> Image.Image | None:
    """Render a body-only front sprite for models without a head bone."""
    body = next(
        (
            bone
            for bone in geometry.get("bones", [])
            if bone.get("name", "").lower() in {"body", "main", "torso"}
            and bone.get("cubes")
        ),
        None,
    )
    if body is None:
        return None
    filtered_geometry = dict(geometry)
    filtered_geometry["bones"] = [body]
    return render_head(texture, filtered_geometry, "north")


def render_front_profile(texture: Image.Image, geometry: dict) -> Image.Image | None:
    profile = render_side_profile(texture, geometry)
    front = render_head(texture, geometry, "north")
    if profile is None:
        return front
    if front is None:
        return profile
    offset = ((profile.width - front.width) // 2, 0)
    profile.alpha_composite(front, offset)
    return profile
