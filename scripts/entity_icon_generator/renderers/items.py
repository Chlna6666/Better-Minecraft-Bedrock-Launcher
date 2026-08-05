from __future__ import annotations

from pathlib import Path
from PIL import Image
from entity_icon_generator.geometry import SPECIAL_ITEMS, output_name, texture_file


def is_special_item(identifier: str) -> bool:
    """Check if identifier belongs to Category 4 (Items)."""
    return identifier in SPECIAL_ITEMS or output_name(identifier) in SPECIAL_ITEMS


def render_item(
    identifier: str, resource_packs: list[Path]
) -> Image.Image | None:
    """Category 4: Items (物品) Renderer.

    Renders 2D item textures (minecart, snowball, trident, egg, etc.).
    """
    clean_name = output_name(identifier)
    item_name = SPECIAL_ITEMS.get(clean_name)
    if not item_name:
        return None

    source = texture_file(resource_packs, f"textures/items/{item_name}")
    if source is None:
        return None

    return Image.open(source)
