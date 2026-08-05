from __future__ import annotations

from pathlib import Path
from PIL import Image, ImageEnhance, ImageOps

ICON_SIZE = 64
ICON_INSET = 2


def opaque_crop(image: Image.Image) -> Image.Image:
    rgba = image.convert("RGBA")
    alpha_bounds = rgba.getchannel("A").getbbox()
    return rgba.crop(alpha_bounds) if alpha_bounds else rgba


def normalize_entity_texture(
    image: Image.Image,
    preserve_low_alpha: bool = False,
    force_opaque: bool = False,
) -> Image.Image:
    rgba = image.convert("RGBA")
    alpha = rgba.getchannel("A")
    # Bedrock TGA textures sometimes store valid detail (spider eyes, sheep face)
    # at alpha 1-7, while other low-alpha pixels are just scanline noise.
    if force_opaque:
        # Blaze and glow squid are translucent in-game; map icons need full color.
        alpha = alpha.point(lambda value: 255 if 0 < value < 255 else value)
        rgba.putalpha(alpha)
    elif any(0 < value < 8 for value in alpha.getdata()):
        if preserve_low_alpha:
            alpha = alpha.point(lambda value: 255 if 0 < value < 8 else value)
        else:
            alpha = alpha.point(lambda value: 0 if value < 8 else value)
        rgba.putalpha(alpha)
    return rgba


def colorize_entity_texture(
    image: Image.Image,
    dark: tuple[int, int, int],
    light: tuple[int, int, int],
) -> Image.Image:
    rgba = normalize_entity_texture(image)
    colorized = ImageOps.colorize(rgba.convert("L"), black=dark, white=light)
    colorized.putalpha(rgba.getchannel("A"))
    return colorized


def write_icon(
    image: Image.Image,
    output: Path,
    size: int = ICON_SIZE,
    crop_content_bottom: int = 0,
    align_top: bool = False,
    offset_x: int = 0,
) -> None:
    cropped = opaque_crop(image)
    inset = ICON_INSET if size == ICON_SIZE else 4
    target = size - inset * 2
    scale = min(target / cropped.width, target / cropped.height)
    width = max(1, round(cropped.width * scale))
    height = max(1, round(cropped.height * scale))
    resized = cropped.resize((width, height), Image.Resampling.NEAREST)
    if crop_content_bottom:
        resized = resized.crop(
            (0, 0, resized.width, max(1, resized.height - crop_content_bottom))
        )
    canvas = Image.new("RGBA", (size, size))
    offset = (
        (size - resized.width) // 2 + offset_x,
        0 if align_top else (size - resized.height) // 2,
    )
    canvas.alpha_composite(resized, offset)
    output.parent.mkdir(parents=True, exist_ok=True)
    canvas.save(output, format="PNG", optimize=True)
