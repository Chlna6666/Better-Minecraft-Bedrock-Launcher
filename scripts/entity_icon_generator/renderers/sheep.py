from __future__ import annotations

from PIL import Image

# The wool sheep head cube points at a wool-only atlas region. The actual face
# (eyes/nose/mouth, stored as low-alpha TGA detail) lives in the sheared head
# face region of the same 64x64 texture.
SHEEP_FACE_UV = (8, 8, 6, 6)


def render_sheep(texture: Image.Image) -> Image.Image | None:
    """Render the sheep face crop that contains eyes, nose, and mouth."""
    left, top, width, height = SHEEP_FACE_UV
    if (
        left + width > texture.width
        or top + height > texture.height
        or width <= 0
        or height <= 0
    ):
        return None
    return texture.convert("RGBA").crop((left, top, left + width, top + height))
