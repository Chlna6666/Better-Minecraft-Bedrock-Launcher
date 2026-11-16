use super::prelude::*;

pub(super) fn tile_image(
    width: u32,
    height: u32,
    pixel_format: TilePixelFormat,
    pixels: Arc<[u8]>,
) -> Result<(Arc<RenderImage>, TilePixelFormat, u32, u32, usize), String> {
    let pixel_len = pixels.len();
    let estimated_bytes = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .unwrap_or(pixel_len);
    let image_pixel_format = match pixel_format {
        TilePixelFormat::Rgba8 => gpui::ImagePixelFormat::Rgba8,
        TilePixelFormat::Bgra8 => gpui::ImagePixelFormat::Bgra8,
    };
    let image = RenderImage::from_raw_pixel_bytes(width, height, image_pixel_format, pixels)
        .map_err(|error| {
            t!(
                "MapViewer.core.tile_cache.invalid_tile_image_size",
                width = width,
                height = height,
                error = error
            )
            .to_string()
        })?;
    Ok((
        Arc::new(image),
        pixel_format,
        width,
        height,
        estimated_bytes,
    ))
}

pub(super) fn decoded_tile_byte_len(width: u32, height: u32) -> Result<usize, String> {
    let pixels = width
        .checked_mul(height)
        .ok_or_else(|| format!("decoded tile dimensions overflow: {width}x{height}"))?;
    let bytes = pixels
        .checked_mul(4)
        .ok_or_else(|| format!("decoded tile byte length overflow: {width}x{height}"))?;
    usize::try_from(bytes)
        .map_err(|_| format!("decoded tile byte length does not fit usize: {width}x{height}"))
}

pub(super) fn render_image_pixels(
    image: &RenderImage,
    pixel_format: Option<TilePixelFormat>,
    width: u32,
    height: u32,
) -> Result<(&[u8], TilePixelFormat), String> {
    let pixel_format = pixel_format
        .ok_or_else(|| t!("MapViewer.core.tile_cache.missing_pixel_format").to_string())?;
    let expected_len = decoded_tile_byte_len(width, height)?;
    let pixels = image
        .as_bytes(0)
        .ok_or_else(|| t!("MapViewer.core.tile_cache.pixel_bytes_unavailable").to_string())?;
    if pixels.len() != expected_len {
        return Err(t!(
            "MapViewer.core.tile_cache.pixel_byte_length_mismatch",
            expected = expected_len,
            actual = pixels.len()
        )
        .to_string());
    }
    Ok((pixels, pixel_format))
}
