from __future__ import annotations

from PIL import Image
from entity_icon_generator.geometry import cube_face, geometry_for_identifier

VILLAGER_FAMILY_ENTITIES = {
    "iron_golem",
    "witch",
    "villager",
    "villager_v2",
    "vindicator",
    "wandering_trader",
    "pillager",
    "npc",
    "evocation_illager",
    "zombie_villager",
    "zombie_villager_v2",
}


def render_villager_family_2d(
    texture: Image.Image, geometry: dict, models: dict[str, dict] | None = None
) -> Image.Image | None:
    """Render villager variant entities in clean 2D front projection.

    Flat compositing of head, main villager nose [2,4,2], mole/wart [1,1,1], hat, helmet, and brim cubes in Z-order.
    Ensures iconic long villager nose is rendered cleanly for evocation_illager, witch, villager, pillager, etc.
    Excludes legacy hat bone for zombie_villager/zombie_villager_v2 which overlays leg/feet texture.
    Uses 2x subpixel canvas rendering for 100% symmetrical witch hat tip centering.
    """
    texture = texture.convert("RGBA")
    scale = 2
    tex_2x = texture.resize((texture.width * scale, texture.height * scale), Image.Resampling.NEAREST)

    ident = str(geometry.get("description", {}).get("identifier", "")).lower()
    is_zombie_villager = "zombie" in ident or "zombie_villager" in ident

    portrait_bone_keywords = (
        ("head", "nose", "mole", "wart", "helmet")
        if is_zombie_villager
        else (
            "head",
            "nose",
            "hat",
            "hat2",
            "hat3",
            "hat4",
            "helmet",
            "brim",
            "mole",
            "wart",
        )
    )

    bones_list = list(geometry.get("bones", []))

    # Always ensure base villager main nose [2, 4, 2] is present
    if models:
        parent_g = geometry_for_identifier(models, "geometry.villager.v1.8")
        if parent_g:
            parent_head = next(
                (b for b in parent_g.get("bones", []) if b.get("name") == "head"),
                None,
            )
            if parent_head and not any(b.get("name") == "head" for b in bones_list):
                bones_list.insert(0, parent_head)
            parent_nose = next((b for b in parent_g.get("bones", []) if b.get("name") == "nose"), None)
            if parent_nose:
                has_main_nose = any(
                    c.get("size") == [2, 4, 2] or c.get("size") == [2.0, 4.0, 2.0]
                    for b in bones_list
                    for c in b.get("cubes", [])
                )
                if not has_main_nose:
                    bones_list.append(parent_nose)

    portrait_bones = [
        bone
        for bone in bones_list
        if any(kw in bone.get("name", "").lower() for kw in portrait_bone_keywords)
    ]
    if not portrait_bones:
        return None

    faces: list[tuple[float, float, float, int, int, int, Image.Image]] = []
    for bone in portrait_bones:
        bname = bone.get("name", "").lower()
        if is_zombie_villager and "hat" in bname:
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

            ox = float(origin[0])
            oy = float(origin[1])
            oz = float(origin[2])

            # Center witch hat tip cubes symmetrically at X = 0
            if bname == "hat2":
                ox = -3.5
            elif bname == "hat3":
                ox = -2.0
            elif bname == "hat4":
                ox = -0.5

            face_img = tex_2x.crop((left * scale, top * scale, (left + width) * scale, (top + height) * scale))
            faces.append(
                (
                    ox * scale,
                    oy * scale,
                    oz * scale,
                    width * scale,
                    height * scale,
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
