from __future__ import annotations

from pathlib import Path
from PIL import Image
from entity_icon_generator.geometry import cube_face, get_merged_geometry, texture_file
from entity_icon_generator.texture import normalize_entity_texture


def render_front_face_2d(
    identifier: str,
    texture: Image.Image,
    geometry: dict,
    resource_packs: list[Path] | None = None,
    models: dict[str, dict] | None = None,
    preserve_low_alpha: bool = False,
    force_opaque: bool = False,
) -> Image.Image | None:
    """Category 1: Front Face (正脸) 2D Renderer.

    Default baseline renderer for entity portraits.
    Collects head, lower jaw (jaw, mouth, chin), ears (ear, gills), nose/snout (nose, snout, beak),
    horns (horn, tusk), headwear (hat, helmet, brim), and decoration cubes.
    Composites faces flat in 2D with back-to-front Z-depth layer ordering.
    Uses float origin rounding for 100% symmetrical sub-pixel alignment.
    """
    texture = normalize_entity_texture(
        texture, preserve_low_alpha=preserve_low_alpha, force_opaque=force_opaque
    )

    # Bogged's front face is the overlay clothes head (hood + face), not the
    # base skeleton head or its separate mushroom cubes.
    if identifier == "bogged" and resource_packs and models:
        overlay_path = texture_file(resource_packs, "textures/entity/skeleton/bogged_clothes")
        if overlay_path:
            overlay_tex = normalize_entity_texture(Image.open(overlay_path))
            overlay_geom = get_merged_geometry(models, "geometry.bogged.armor")
            if overlay_geom:
                for bone in overlay_geom.get("bones", []):
                    if bone.get("name") != "head":
                        continue
                    for cube in bone.get("cubes", []):
                        face = cube_face(cube, "north")
                        if face is None:
                            continue
                        left, top, width, height = face
                        if (
                            left + width <= overlay_tex.width
                            and top + height <= overlay_tex.height
                        ):
                            return overlay_tex.crop(
                                (left, top, left + width, top + height)
                            )

    # Entity-specific bone filters
    if identifier in {"zombie", "husk"}:
        keywords = ("head",)
    elif identifier == "blaze":
        keywords = ("head",)
    elif identifier in {"squid", "glow_squid"}:
        keywords = ("body",)
    elif identifier == "ender_dragon":
        keywords = ("head", "jaw", "snout", "horn", "horns")
    elif identifier == "wither":
        keywords = ("head1", "head2", "head3", "upperbodypart1", "upperbodypart2", "upperbodypart3")
    elif identifier == "endermite":
        keywords = ("section_0", "section_1", "head", "eye")
    elif identifier == "allay":
        keywords = ("look_at", "head")
    elif identifier == "frog":
        keywords = ("head", "body", "eye", "eyes")
    elif identifier == "evocation_fang":
        keywords = ("jaw", "base")
    elif identifier in {"hoglin", "zoglin"}:
        keywords = ("head", "tusk", "tusks", "ear", "ears")
    elif identifier == "vex":
        keywords = ("head",)
    else:
        keywords = (
            "head",
            "nose",
            "hat",
            "helmet",
            "brim",
            "jaw",
            "mouth",
            "chin",
            "horn",
            "horns",
            "tusk",
            "tusks",
            "ear",
            "ears",
            "gills",
            "snout",
            "beak",
            "eye",
            "mushroom",
            "cap",
            "section_0",
            "look_at",
        )

    portrait_bones = [
        bone
        for bone in geometry.get("bones", [])
        if any(kw in bone.get("name", "").lower() for kw in keywords)
    ]
    if not portrait_bones:
        return None

    face_direction = "north"
    faces: list[tuple[float, float, float, int, int, int, Image.Image]] = []
    for bone in portrait_bones:
        bname = bone.get("name", "").lower()
        if identifier == "frog" and "croaking" in bname:
            continue
        for cube in bone.get("cubes", []):
            direction = face_direction
            # Hoglin/zoglin heads are rotated in the game model; show the
            # slanted snout top face instead of a flat frontal cube.
            if (
                identifier in {"hoglin", "zoglin"}
                and bname == "head"
                and cube_face(cube, "north") == (80, 20, 14, 6)
            ):
                direction = "up"
            face = cube_face(cube, direction)
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

            # Frog's head face has an open mouth row; keep the closed face.
            if identifier == "frog" and bname == "head" and height == 3:
                height = 2
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
            oy = float(origin[1])
            oz = float(origin[2])
            # Evocation fang jaws are the spike in front of the base; draw
            # them on top so the assembled fang is not hidden by the base.
            if identifier == "evocation_fang" and "jaw" in bname:
                oz -= 2
            # Frog's eyes sit one pixel above the face; shift the face up so
            # the front portrait is contiguous instead of a floating eye strip.
            if identifier == "frog" and bname in ("head", "body"):
                oy += 1
            # Enderman's static jaw bone sits above the head in model space;
            # overlap it one row under the head so the head stays on top.
            if identifier == "enderman" and bname == "hat":
                oy -= 14
            faces.append(
                (
                    float(origin[0]),
                    oy,
                    oz,
                    width,
                    height,
                    int(size[2]),
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
        resized = face_img.resize((w, h), Image.Resampling.NEAREST)
        canvas.alpha_composite(resized, (round(ox - min_x), round(max_y - oy - h)))

    return canvas
