from __future__ import annotations

import math
from PIL import Image
from entity_icon_generator.geometry import cube_face


def _mat_mul(a: list[list[float]], b: list[list[float]]) -> list[list[float]]:
    return [
        [
            a[0][0] * b[0][0] + a[0][1] * b[1][0] + a[0][2] * b[2][0],
            a[0][0] * b[0][1] + a[0][1] * b[1][1] + a[0][2] * b[2][1],
            a[0][0] * b[0][2] + a[0][1] * b[1][2] + a[0][2] * b[2][2],
        ],
        [
            a[1][0] * b[0][0] + a[1][1] * b[1][0] + a[1][2] * b[2][0],
            a[1][0] * b[0][1] + a[1][1] * b[1][1] + a[1][2] * b[2][1],
            a[1][0] * b[0][2] + a[1][1] * b[1][2] + a[1][2] * b[2][2],
        ],
        [
            a[2][0] * b[0][0] + a[2][1] * b[1][0] + a[2][2] * b[2][0],
            a[2][0] * b[0][1] + a[2][1] * b[1][1] + a[2][2] * b[2][1],
            a[2][0] * b[0][2] + a[2][1] * b[1][2] + a[2][2] * b[2][2],
        ],
    ]


def _rotation_matrix(rotation: list[float] | None) -> list[list[float]]:
    rx, ry, rz = [math.radians(value) for value in (rotation or [0, 0, 0])]
    cx, sx = math.cos(rx), math.sin(rx)
    cy, sy = math.cos(ry), math.sin(ry)
    cz, sz = math.cos(rz), math.sin(rz)
    rot_x = [[1, 0, 0], [0, cx, -sx], [0, sx, cx]]
    rot_y = [[cy, 0, sy], [0, 1, 0], [-sy, 0, cy]]
    rot_z = [[cz, -sz, 0], [sz, cz, 0], [0, 0, 1]]
    return _mat_mul(rot_z, _mat_mul(rot_y, rot_x))


def _apply_matrix(point: tuple[float, float, float], matrix: list[list[float]]) -> tuple[float, float, float]:
    x, y, z = point
    return (
        matrix[0][0] * x + matrix[0][1] * y + matrix[0][2] * z,
        matrix[1][0] * x + matrix[1][1] * y + matrix[1][2] * z,
        matrix[2][0] * x + matrix[2][1] * y + matrix[2][2] * z,
    )


def _bone_world_transforms(geometry: dict) -> dict[str, object]:
    bones = {bone.get("name"): bone for bone in geometry.get("bones", [])}
    transforms: dict[str, object] = {}

    def compute(name: str, parent: object) -> None:
        bone = bones[name]
        pivot = tuple(bone.get("pivot") or [0, 0, 0])
        position = tuple(bone.get("position") or [0, 0, 0])
        matrix = _rotation_matrix(bone.get("rotation"))

        def world(point: tuple[float, float, float]) -> tuple[float, float, float]:
            moved = (
                point[0] + position[0] - pivot[0],
                point[1] + position[1] - pivot[1],
                point[2] + position[2] - pivot[2],
            )
            rotated = _apply_matrix(moved, matrix)
            local = (
                rotated[0] + pivot[0],
                rotated[1] + pivot[1],
                rotated[2] + pivot[2],
            )
            return parent(local)

        transforms[name] = world
        for child_name, child in bones.items():
            if child.get("parent") == name:
                compute(child_name, world)

    for name, bone in bones.items():
        if not bone.get("parent"):
            compute(name, lambda point: point)
    return transforms


def _face_corners(cube: dict, direction: str) -> list[tuple[float, float, float]]:
    origin = cube.get("origin")
    size = cube.get("size")
    x0, y0, z0 = origin[0], origin[1], origin[2]
    x1, y1, z1 = x0 + size[0], y0 + size[1], z0 + size[2]
    if direction == "north":
        return [
            (x0, y1, z0),
            (x1, y1, z0),
            (x1, y0, z0),
            (x0, y0, z0),
        ]
    if direction == "south":
        return [
            (x1, y1, z1),
            (x0, y1, z1),
            (x0, y0, z1),
            (x1, y0, z1),
        ]
    if direction == "west":
        return [
            (x0, y1, z1),
            (x0, y1, z0),
            (x0, y0, z0),
            (x0, y0, z1),
        ]
    if direction == "east":
        return [
            (x1, y1, z0),
            (x1, y1, z1),
            (x1, y0, z1),
            (x1, y0, z0),
        ]
    if direction == "up":
        return [
            (x0, y1, z1),
            (x1, y1, z1),
            (x1, y1, z0),
            (x0, y1, z0),
        ]
    return [
        (x1, y0, z1),
        (x0, y0, z1),
        (x0, y0, z0),
        (x1, y0, z0),
    ]


def _face_normal(points: list[tuple[float, float, float]]) -> tuple[float, float, float]:
    a = points[0]
    u = (points[1][0] - a[0], points[1][1] - a[1], points[1][2] - a[2])
    v = (points[3][0] - a[0], points[3][1] - a[1], points[3][2] - a[2])
    return (
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    )


def _project(point: tuple[float, float, float], view: str) -> tuple[float, float, float]:
    x, y, z = point
    if view == "north":
        return (x, -y, z)
    if view == "south":
        return (-x, -y, z)
    if view == "east":
        return (z, -y, x)
    return (-z, -y, x)


def _visible(normal: tuple[float, float, float], view: str) -> bool:
    if view == "north":
        return normal[2] < 0
    if view == "south":
        return normal[2] > 0
    if view == "east":
        return normal[0] > 0
    return normal[0] < 0


def _affine_coeffs(src: list[tuple[float, float]], dst: list[tuple[float, float]]) -> tuple[float, float, float, float, float, float]:
    x1, y1 = dst[0]
    x2, y2 = dst[1]
    x3, y3 = dst[2]
    u1, v1 = src[0]
    u2, v2 = src[1]
    u3, v3 = src[2]
    denom = x1 * (y2 - y3) - y1 * (x2 - x3) + (x2 * y3 - x3 * y2)
    a = (u1 * (y2 - y3) - y1 * (u2 - u3) + (u2 * y3 - u3 * y2)) / denom
    b = (x1 * (u2 - u3) - u1 * (x2 - x3) + (x2 * u3 - x3 * u2)) / denom
    c = (x1 * (y2 * u3 - y3 * u2) - y1 * (x2 * u3 - x3 * u2) + u1 * (x2 * y3 - x3 * y2)) / denom
    d = (v1 * (y2 - y3) - y1 * (v2 - v3) + (v2 * y3 - v3 * y2)) / denom
    e = (x1 * (v2 - v3) - v1 * (x2 - x3) + (x2 * v3 - x3 * v2)) / denom
    f = (x1 * (y2 * v3 - y3 * v2) - y1 * (x2 * v3 - x3 * v2) + v1 * (x2 * y3 - x3 * y2)) / denom
    return (a, b, c, d, e, f)


def render_model_3d(
    texture: Image.Image,
    geometry: dict,
    view: str = "north",
    bone_filter: set[str] | None = None,
    focus_bones: set[str] | None = None,
    focus_ratio: float = 0.5,
    double_sided_bones: set[str] | None = None,
    front_bones: set[str] | None = None,
    pad_square: bool = False,
) -> Image.Image | None:
    """Project an assembled Bedrock model to a 2D face texture image."""
    texture = texture.convert("RGBA")
    declared_width = int((geometry.get("description") or {}).get("texture_width") or 0)
    declared_height = int((geometry.get("description") or {}).get("texture_height") or 0)
    uv_scale_x = texture.width / declared_width if declared_width else 1.0
    uv_scale_y = texture.height / declared_height if declared_height else 1.0
    transforms = _bone_world_transforms(geometry)
    records: list[tuple[float, Image.Image, list[tuple[float, float]], bool]] = []

    for bone in geometry.get("bones", []):
        name = bone.get("name")
        if bone_filter is not None and name not in bone_filter:
            continue
        if bone.get("neverRender"):
            continue
        world = transforms.get(name)
        if world is None:
            continue
        for cube in bone.get("cubes", []):
            size = cube.get("size")
            if not (isinstance(size, list) and len(size) >= 3):
                continue
            origin = cube.get("origin") or [0, 0, 0]
            cube_rotation = cube.get("rotation")
            cube_pivot = cube.get("pivot")
            if cube_pivot is None:
                cube_pivot = [
                    origin[0] + size[0] / 2.0,
                    origin[1] + size[1] / 2.0,
                    origin[2] + size[2] / 2.0,
                ]
            cube_matrix = _rotation_matrix(cube_rotation) if cube_rotation else None

            def to_world(point: tuple[float, float, float]) -> tuple[float, float, float]:
                if cube_matrix is not None:
                    moved = (
                        point[0] - cube_pivot[0],
                        point[1] - cube_pivot[1],
                        point[2] - cube_pivot[2],
                    )
                    rotated = _apply_matrix(moved, cube_matrix)
                    point = (
                        rotated[0] + cube_pivot[0],
                        rotated[1] + cube_pivot[1],
                        rotated[2] + cube_pivot[2],
                    )
                return world(point)

            for direction in ("north", "south", "east", "west", "up", "down"):
                face = cube_face(cube, direction)
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
                face_image = texture.crop(
                    (left, top, left + width, top + height)
                )
                if face_image.getchannel("A").getbbox() is None:
                    continue
                world_points = [to_world(point) for point in _face_corners(cube, direction)]
                normal = _face_normal(world_points)
                if (
                    double_sided_bones is None
                    or name not in double_sided_bones
                ) and not _visible(normal, view):
                    continue
                depth_axis = 2 if view in {"north", "south"} else 0
                depth = sum(point[depth_axis] for point in world_points) / 4.0
                projected = [_project(point, view) for point in world_points]
                area = abs(
                    (projected[1][0] - projected[0][0])
                    * (projected[2][1] - projected[0][1])
                    - (projected[1][1] - projected[0][1])
                    * (projected[2][0] - projected[0][0])
                )
                if area < 1e-9:
                    continue
                records.append(
                    (
                        depth,
                        face_image,
                        [(point[0], point[1]) for point in projected],
                        focus_bones is not None and name in focus_bones,
                        name,
                    )
                )

    if not records:
        return None

    min_x = min(point[0] for _, _, quad, _, _ in records for point in quad)
    max_x = max(point[0] for _, _, quad, _, _ in records for point in quad)
    min_y = min(point[1] for _, _, quad, _, _ in records for point in quad)
    max_y = max(point[1] for _, _, quad, _, _ in records for point in quad)
    canvas = Image.new(
        "RGBA",
        (max(1, round(max_x - min_x)), max(1, round(max_y - min_y))),
    )

    far_to_near = view in {"north", "east"}
    ordered = sorted(
        records, key=lambda record: record[0], reverse=far_to_near
    )
    front = (
        [record for record in ordered if record[4] in front_bones]
        if front_bones
        else []
    )
    back = (
        [record for record in ordered if record[4] not in front_bones]
        if front_bones
        else ordered
    )
    for depth, face_image, quad, _, _ in [*back, *front]:
        src = [(0, 0), (face_image.width, 0), (face_image.width, face_image.height)]
        dst = [
            (quad[0][0] - min_x, quad[0][1] - min_y),
            (quad[1][0] - min_x, quad[1][1] - min_y),
            (quad[2][0] - min_x, quad[2][1] - min_y),
        ]
        quad_min_x = min(point[0] for point in dst)
        quad_min_y = min(point[1] for point in dst)
        quad_max_x = max(point[0] for point in dst)
        quad_max_y = max(point[1] for point in dst)
        out_width = max(1, round(quad_max_x - quad_min_x))
        out_height = max(1, round(quad_max_y - quad_min_y))
        shifted_dst = [
            (point[0] - quad_min_x, point[1] - quad_min_y) for point in dst
        ]
        coeffs = _affine_coeffs(src, shifted_dst)
        warped = face_image.transform(
            (out_width, out_height),
            Image.AFFINE,
            coeffs,
            resample=Image.BILINEAR,
        )
        canvas.alpha_composite(warped, (round(quad_min_x), round(quad_min_y)))

    if pad_square and not focus_bones:
        side = max(canvas.width, canvas.height)
        padded = Image.new("RGBA", (side, side))
        padded.alpha_composite(
            canvas,
            ((side - canvas.width) // 2, (side - canvas.height) // 2),
        )
        return padded

    if not focus_bones:
        return canvas
    focus_quads = [
        quad
        for _, _, quad, is_focus, _ in records
        if is_focus
    ]
    if not focus_quads:
        return canvas
    focus_min_x = min(point[0] for quad in focus_quads for point in quad) - min_x
    focus_max_x = max(point[0] for quad in focus_quads for point in quad) - min_x
    focus_min_y = min(point[1] for quad in focus_quads for point in quad) - min_y
    focus_max_y = max(point[1] for quad in focus_quads for point in quad) - min_y
    focus_width = focus_max_x - focus_min_x
    focus_height = focus_max_y - focus_min_y
    if focus_width <= 0 or focus_height <= 0:
        return canvas

    # Zoom to a square viewport so the focused head fills focus_ratio of it,
    # keeping nearby distinctive body parts in frame.
    crop_size = min(
        canvas.width,
        canvas.height,
        max(1, round(max(focus_width, focus_height) / focus_ratio)),
    )
    center_x = (focus_min_x + focus_max_x) / 2.0
    center_y = (focus_min_y + focus_max_y) / 2.0
    left = round(center_x - crop_size / 2.0)
    top = round(center_y - crop_size / 2.0)
    left = max(0, min(left, canvas.width - crop_size))
    top = max(0, min(top, canvas.height - crop_size))
    canvas = canvas.crop((left, top, left + crop_size, top + crop_size))
    if not pad_square:
        return canvas
    side = max(canvas.width, canvas.height)
    padded = Image.new("RGBA", (side, side))
    padded.alpha_composite(
        canvas,
        ((side - canvas.width) // 2, (side - canvas.height) // 2),
    )
    return padded
