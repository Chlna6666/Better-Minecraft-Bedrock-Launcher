use super::*;
use crate::swap_rgba_to_bgra_rows;
#[cfg(feature = "bench")]
use crate::{swap_rgba_to_bgra_rows_scalar, swap_rgba_to_bgra_rows_simd};

const NOVA_ATLAS_TRANSPARENT_COVERAGE: [u8; 1] = [0];
const NOVA_ATLAS_TRANSPARENT_COLOR: [u8; 4] = [0, 0, 0, 0];

#[cfg(feature = "bench")]
pub(crate) struct AtlasPixelEncodingBenchmarkCore {
    destination: Vec<u8>,
    source: Vec<u8>,
    size: Size<DevicePixels>,
    texture_kind: AtlasTextureKind,
    padding: u32,
}

#[cfg(feature = "bench")]
impl AtlasPixelEncodingBenchmarkCore {
    pub(crate) fn rgba(width: u32, height: u32, padding: u32) -> Self {
        Self::new(width, height, padding, AtlasTextureKind::Rgba)
    }

    pub(crate) fn bgra(width: u32, height: u32, padding: u32) -> Self {
        Self::new(width, height, padding, AtlasTextureKind::Bgra)
    }

    pub(crate) fn monochrome(width: u32, height: u32, padding: u32) -> Self {
        Self::new(width, height, padding, AtlasTextureKind::Monochrome)
    }

    pub(crate) fn subpixel(width: u32, height: u32, padding: u32) -> Self {
        Self::new(width, height, padding, AtlasTextureKind::Subpixel)
    }

    fn new(width: u32, height: u32, padding: u32, texture_kind: AtlasTextureKind) -> Self {
        let upload_width = width.saturating_add(padding.saturating_mul(2));
        let upload_height = height.saturating_add(padding.saturating_mul(2));
        let source_bytes_per_pixel = match texture_kind {
            AtlasTextureKind::Monochrome => 1,
            AtlasTextureKind::Bgra | AtlasTextureKind::Rgba | AtlasTextureKind::Subpixel => {
                NOVA_ATLAS_BYTES_PER_PIXEL
            }
        };
        let source_byte_len = width as usize * height as usize * source_bytes_per_pixel;
        let destination_byte_len =
            upload_width as usize * upload_height as usize * NOVA_ATLAS_BYTES_PER_PIXEL;
        Self {
            destination: vec![0; destination_byte_len],
            source: (0..source_byte_len)
                .map(|index| u8::try_from(index % 251).expect("modulo 251 must fit u8"))
                .collect(),
            size: crate::size(
                DevicePixels(i32::try_from(width).expect("benchmark width must fit i32")),
                DevicePixels(i32::try_from(height).expect("benchmark height must fit i32")),
            ),
            texture_kind,
            padding,
        }
    }

    pub(crate) fn encode(&mut self) -> usize {
        self.encode_with(encode_bgra_upload_with_padding)
    }

    #[cfg(feature = "bench")]
    pub(crate) fn encode_scalar(&mut self) -> usize {
        self.encode_with(encode_bgra_upload_with_padding_scalar)
    }

    #[cfg(feature = "bench")]
    pub(crate) fn encode_simd(&mut self) -> usize {
        self.encode_with(encode_bgra_upload_with_padding_simd)
    }

    fn encode_with(
        &mut self,
        encoder: fn(&mut [u8], Size<DevicePixels>, &[u8], AtlasTextureKind, u32) -> Option<()>,
    ) -> usize {
        encoder(
            &mut self.destination,
            self.size,
            &self.source,
            self.texture_kind,
            self.padding,
        )
        .expect("benchmark atlas dimensions and buffers must agree");
        std::hint::black_box(self.destination.as_slice()).len()
    }
}

#[cfg(test)]
pub(in crate::platform::nova) fn encode_bgra_upload(
    pixels: &mut [u8],
    size: Size<DevicePixels>,
    bytes: &[u8],
    texture_kind: AtlasTextureKind,
) -> Option<()> {
    encode_bgra_upload_with_padding(pixels, size, bytes, texture_kind, 0)
}

pub(in crate::platform::nova) fn atlas_kind_index(texture_kind: AtlasTextureKind) -> usize {
    texture_kind as usize
}

pub(in crate::platform::nova) fn fallback_atlas_bytes(
    texture_kind: AtlasTextureKind,
) -> &'static [u8] {
    match texture_kind {
        AtlasTextureKind::Monochrome => &NOVA_ATLAS_TRANSPARENT_COVERAGE,
        AtlasTextureKind::Bgra | AtlasTextureKind::Rgba | AtlasTextureKind::Subpixel => {
            &NOVA_ATLAS_TRANSPARENT_COLOR
        }
    }
}

pub(in crate::platform::nova) fn atlas_source_byte_len(
    size: Size<DevicePixels>,
    texture_kind: AtlasTextureKind,
) -> Option<usize> {
    let width = size.width.0.max(1) as usize;
    let height = size.height.0.max(1) as usize;
    let bytes_per_pixel = match texture_kind {
        AtlasTextureKind::Monochrome => 1,
        AtlasTextureKind::Bgra | AtlasTextureKind::Rgba | AtlasTextureKind::Subpixel => {
            NOVA_ATLAS_BYTES_PER_PIXEL
        }
    };
    width.checked_mul(height)?.checked_mul(bytes_per_pixel)
}

pub(in crate::platform::nova) fn encode_bgra_upload_with_padding(
    pixels: &mut [u8],
    size: Size<DevicePixels>,
    bytes: &[u8],
    texture_kind: AtlasTextureKind,
    padding: u32,
) -> Option<()> {
    let width = size.width.0.max(1) as usize;
    let height = size.height.0.max(1) as usize;
    let padding = padding as usize;
    let upload_width = width.saturating_add(padding.saturating_mul(2));
    let upload_height = height.saturating_add(padding.saturating_mul(2));
    if pixels.len()
        < upload_width
            .saturating_mul(upload_height)
            .saturating_mul(NOVA_ATLAS_BYTES_PER_PIXEL)
    {
        return None;
    }

    match texture_kind {
        AtlasTextureKind::Monochrome => encode_monochrome_upload(
            pixels,
            bytes,
            width,
            height,
            upload_width,
            upload_height,
            padding,
        ),
        AtlasTextureKind::Rgba => encode_rgba_upload(
            pixels,
            bytes,
            width,
            height,
            upload_width,
            upload_height,
            padding,
        ),
        AtlasTextureKind::Bgra => encode_bgra_upload_kind(
            pixels,
            bytes,
            width,
            height,
            upload_width,
            upload_height,
            padding,
        ),
        AtlasTextureKind::Subpixel => encode_subpixel_upload(
            pixels,
            bytes,
            width,
            height,
            upload_width,
            upload_height,
            padding,
        ),
    }
}

#[cfg(feature = "bench")]
fn encode_bgra_upload_with_padding_scalar(
    pixels: &mut [u8],
    size: Size<DevicePixels>,
    bytes: &[u8],
    texture_kind: AtlasTextureKind,
    padding: u32,
) -> Option<()> {
    if texture_kind != AtlasTextureKind::Rgba {
        return encode_bgra_upload_with_padding(pixels, size, bytes, texture_kind, padding);
    }

    let width = size.width.0.max(1) as usize;
    let height = size.height.0.max(1) as usize;
    let padding = padding as usize;
    let upload_width = width.saturating_add(padding.saturating_mul(2));
    let upload_height = height.saturating_add(padding.saturating_mul(2));
    if pixels.len()
        < upload_width
            .saturating_mul(upload_height)
            .saturating_mul(NOVA_ATLAS_BYTES_PER_PIXEL)
    {
        return None;
    }

    encode_rgba_upload_scalar(
        pixels,
        bytes,
        width,
        height,
        upload_width,
        upload_height,
        padding,
    )
}

#[cfg(feature = "bench")]
fn encode_bgra_upload_with_padding_simd(
    pixels: &mut [u8],
    size: Size<DevicePixels>,
    bytes: &[u8],
    texture_kind: AtlasTextureKind,
    padding: u32,
) -> Option<()> {
    if texture_kind != AtlasTextureKind::Rgba {
        return encode_bgra_upload_with_padding(pixels, size, bytes, texture_kind, padding);
    }

    let width = size.width.0.max(1) as usize;
    let height = size.height.0.max(1) as usize;
    let padding = padding as usize;
    let upload_width = width.saturating_add(padding.saturating_mul(2));
    let upload_height = height.saturating_add(padding.saturating_mul(2));
    if pixels.len()
        < upload_width
            .saturating_mul(upload_height)
            .saturating_mul(NOVA_ATLAS_BYTES_PER_PIXEL)
    {
        return None;
    }

    encode_rgba_upload_simd(
        pixels,
        bytes,
        width,
        height,
        upload_width,
        upload_height,
        padding,
    )
}

fn encode_bgra_upload_kind(
    pixels: &mut [u8],
    bytes: &[u8],
    width: usize,
    height: usize,
    upload_width: usize,
    upload_height: usize,
    padding: usize,
) -> Option<()> {
    let source_len = width
        .checked_mul(height)?
        .checked_mul(NOVA_ATLAS_BYTES_PER_PIXEL)?;
    let source = bytes.get(..source_len)?;
    let upload_len = upload_width
        .checked_mul(upload_height)?
        .checked_mul(NOVA_ATLAS_BYTES_PER_PIXEL)?;
    let destination = pixels.get_mut(..upload_len)?;
    if padding == 0 {
        destination.copy_from_slice(source);
        return Some(());
    }

    let source_row_bytes = width.checked_mul(NOVA_ATLAS_BYTES_PER_PIXEL)?;
    let upload_row_bytes = upload_width.checked_mul(NOVA_ATLAS_BYTES_PER_PIXEL)?;
    for (source_row, destination_row) in source
        .chunks_exact(source_row_bytes)
        .zip(destination[padding * upload_row_bytes..].chunks_exact_mut(upload_row_bytes))
        .take(height)
    {
        let center_start = padding * NOVA_ATLAS_BYTES_PER_PIXEL;
        destination_row[center_start..center_start + source_row_bytes].copy_from_slice(source_row);
        replicate_horizontal_padding(destination_row, width, padding)?;
    }
    replicate_vertical_padding(destination, upload_width, height, padding)?;
    Some(())
}

fn replicate_horizontal_padding(row: &mut [u8], width: usize, padding: usize) -> Option<()> {
    if padding == 0 {
        return Some(());
    }
    let center_start = padding.checked_mul(NOVA_ATLAS_BYTES_PER_PIXEL)?;
    let center_end = center_start.checked_add(width.checked_mul(NOVA_ATLAS_BYTES_PER_PIXEL)?)?;
    let first_pixel: [u8; NOVA_ATLAS_BYTES_PER_PIXEL] = row
        .get(center_start..center_start + NOVA_ATLAS_BYTES_PER_PIXEL)?
        .try_into()
        .ok()?;
    let last_pixel: [u8; NOVA_ATLAS_BYTES_PER_PIXEL] = row
        .get(center_end - NOVA_ATLAS_BYTES_PER_PIXEL..center_end)?
        .try_into()
        .ok()?;
    for pixel in row
        .get_mut(..center_start)?
        .chunks_exact_mut(NOVA_ATLAS_BYTES_PER_PIXEL)
    {
        pixel.copy_from_slice(&first_pixel);
    }
    for pixel in row
        .get_mut(center_end..)?
        .chunks_exact_mut(NOVA_ATLAS_BYTES_PER_PIXEL)
    {
        pixel.copy_from_slice(&last_pixel);
    }
    Some(())
}

fn replicate_vertical_padding(
    destination: &mut [u8],
    upload_width: usize,
    height: usize,
    padding: usize,
) -> Option<()> {
    if padding == 0 {
        return Some(());
    }
    let row_bytes = upload_width.checked_mul(NOVA_ATLAS_BYTES_PER_PIXEL)?;
    let first_row = padding.checked_mul(row_bytes)?;
    let last_row = padding
        .checked_add(height)?
        .checked_sub(1)?
        .checked_mul(row_bytes)?;
    for row in 0..padding {
        destination.copy_within(first_row..first_row + row_bytes, row * row_bytes);
    }
    for row in padding + height..padding + height + padding {
        destination.copy_within(last_row..last_row + row_bytes, row * row_bytes);
    }
    Some(())
}

fn encode_rgba_upload(
    pixels: &mut [u8],
    bytes: &[u8],
    width: usize,
    height: usize,
    upload_width: usize,
    upload_height: usize,
    padding: usize,
) -> Option<()> {
    encode_rgba_upload_with_swap(
        pixels,
        bytes,
        width,
        height,
        upload_width,
        upload_height,
        padding,
        swap_rgba_to_bgra_rows,
    )
}

#[cfg(feature = "bench")]
fn encode_rgba_upload_scalar(
    pixels: &mut [u8],
    bytes: &[u8],
    width: usize,
    height: usize,
    upload_width: usize,
    upload_height: usize,
    padding: usize,
) -> Option<()> {
    encode_rgba_upload_with_swap(
        pixels,
        bytes,
        width,
        height,
        upload_width,
        upload_height,
        padding,
        swap_rgba_to_bgra_rows_scalar,
    )
}

#[cfg(feature = "bench")]
fn encode_rgba_upload_simd(
    pixels: &mut [u8],
    bytes: &[u8],
    width: usize,
    height: usize,
    upload_width: usize,
    upload_height: usize,
    padding: usize,
) -> Option<()> {
    encode_rgba_upload_with_swap(
        pixels,
        bytes,
        width,
        height,
        upload_width,
        upload_height,
        padding,
        swap_rgba_to_bgra_rows_simd,
    )
}

fn encode_rgba_upload_with_swap(
    pixels: &mut [u8],
    bytes: &[u8],
    width: usize,
    height: usize,
    upload_width: usize,
    upload_height: usize,
    padding: usize,
    swap_rgba: fn(&mut [u8], usize, usize),
) -> Option<()> {
    let source_len = width
        .checked_mul(height)?
        .checked_mul(NOVA_ATLAS_BYTES_PER_PIXEL)?;
    let source = bytes.get(..source_len)?;
    let upload_len = upload_width
        .checked_mul(upload_height)?
        .checked_mul(NOVA_ATLAS_BYTES_PER_PIXEL)?;
    let destination = pixels.get_mut(..upload_len)?;
    let source_row_bytes = width.checked_mul(NOVA_ATLAS_BYTES_PER_PIXEL)?;
    let upload_row_bytes = upload_width.checked_mul(NOVA_ATLAS_BYTES_PER_PIXEL)?;

    if padding == 0 {
        destination.copy_from_slice(source);
        swap_rgba(destination, upload_row_bytes, upload_height);
        return Some(());
    }

    for (source_row, destination_row) in source
        .chunks_exact(source_row_bytes)
        .zip(destination[padding * upload_row_bytes..].chunks_exact_mut(upload_row_bytes))
        .take(height)
    {
        let center_start = padding * NOVA_ATLAS_BYTES_PER_PIXEL;
        let center = &mut destination_row[center_start..center_start + source_row_bytes];
        center.copy_from_slice(source_row);
        replicate_horizontal_padding(destination_row, width, padding)?;
    }
    replicate_vertical_padding(destination, upload_width, height, padding)?;
    swap_rgba(destination, upload_row_bytes, upload_height);
    Some(())
}

fn encode_monochrome_upload(
    pixels: &mut [u8],
    bytes: &[u8],
    width: usize,
    height: usize,
    upload_width: usize,
    upload_height: usize,
    padding: usize,
) -> Option<()> {
    let source_len = width.checked_mul(height)?;
    let source = bytes.get(..source_len)?;
    let upload_len = upload_width
        .checked_mul(upload_height)?
        .checked_mul(NOVA_ATLAS_BYTES_PER_PIXEL)?;
    let destination = pixels.get_mut(..upload_len)?;
    let center_byte_len = width.checked_mul(NOVA_ATLAS_BYTES_PER_PIXEL)?;
    let upload_row_bytes = upload_width.checked_mul(NOVA_ATLAS_BYTES_PER_PIXEL)?;

    for (source_row, destination_row) in source
        .chunks_exact(width)
        .zip(destination[padding * upload_row_bytes..].chunks_exact_mut(upload_row_bytes))
        .take(height)
    {
        let center_start = padding * NOVA_ATLAS_BYTES_PER_PIXEL;
        let center = &mut destination_row[center_start..center_start + center_byte_len];
        for (&coverage, destination_pixel) in source_row
            .iter()
            .zip(center.chunks_exact_mut(NOVA_ATLAS_BYTES_PER_PIXEL))
        {
            // Monochrome shaders sample only the logical red channel of the BGRA atlas.
            destination_pixel.copy_from_slice(&[0, 0, coverage, 255]);
        }
        replicate_horizontal_padding(destination_row, width, padding)?;
    }
    replicate_vertical_padding(destination, upload_width, height, padding)?;
    Some(())
}

fn encode_subpixel_upload(
    pixels: &mut [u8],
    bytes: &[u8],
    width: usize,
    height: usize,
    upload_width: usize,
    upload_height: usize,
    padding: usize,
) -> Option<()> {
    let source_len = width
        .checked_mul(height)?
        .checked_mul(NOVA_ATLAS_BYTES_PER_PIXEL)?;
    let source = bytes.get(..source_len)?;
    let upload_len = upload_width
        .checked_mul(upload_height)?
        .checked_mul(NOVA_ATLAS_BYTES_PER_PIXEL)?;
    let destination = pixels.get_mut(..upload_len)?;
    let source_row_bytes = width.checked_mul(NOVA_ATLAS_BYTES_PER_PIXEL)?;
    let upload_row_bytes = upload_width.checked_mul(NOVA_ATLAS_BYTES_PER_PIXEL)?;

    for (source_row, destination_row) in source
        .chunks_exact(source_row_bytes)
        .zip(destination[padding * upload_row_bytes..].chunks_exact_mut(upload_row_bytes))
        .take(height)
    {
        let center_start = padding * NOVA_ATLAS_BYTES_PER_PIXEL;
        let center = &mut destination_row[center_start..center_start + source_row_bytes];
        for (source_pixel, destination_pixel) in source_row
            .chunks_exact(NOVA_ATLAS_BYTES_PER_PIXEL)
            .zip(center.chunks_exact_mut(NOVA_ATLAS_BYTES_PER_PIXEL))
        {
            let alpha = u16::from(source_pixel[3]);
            let red = u16::from(source_pixel[0]).saturating_mul(alpha) / 255;
            let green = u16::from(source_pixel[1]).saturating_mul(alpha) / 255;
            let blue = u16::from(source_pixel[2]).saturating_mul(alpha) / 255;

            // The GPU atlas uses BGRA8 memory layout. Preserve DirectWrite's independent
            // R/G/B coverage values instead of collapsing them into one grayscale channel.
            destination_pixel.copy_from_slice(&[
                u8::try_from(blue).unwrap_or(u8::MAX),
                u8::try_from(green).unwrap_or(u8::MAX),
                u8::try_from(red).unwrap_or(u8::MAX),
                255,
            ]);
        }
        replicate_horizontal_padding(destination_row, width, padding)?;
    }
    replicate_vertical_padding(destination, upload_width, height, padding)?;
    Some(())
}
