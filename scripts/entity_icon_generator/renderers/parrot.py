from __future__ import annotations

from PIL import Image
from entity_icon_generator.renderers.standard import render_side_profile


def render_parrot(texture: Image.Image, geometry: dict) -> Image.Image | None:
    """Side head portrait including crest, head, and beak decorations."""
    head_bones = [
        bone
        for bone in geometry.get("bones", [])
        if bone.get("name") in {"head", "head2", "beak1", "beak2", "feather"}
    ]
    if not head_bones:
        return None
    return render_side_profile(texture, {"bones": list(head_bones)})
