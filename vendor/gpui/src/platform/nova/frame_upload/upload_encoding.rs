use super::*;

const NOVA_ATLAS_TRANSPARENT_COVERAGE: [u8; 1] = [0];
const NOVA_ATLAS_TRANSPARENT_COLOR: [u8; 4] = [0, 0, 0, 0];

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

    for upload_y in 0..upload_height {
        let y = upload_y
            .saturating_sub(padding)
            .min(height.saturating_sub(1));
        for upload_x in 0..upload_width {
            let x = upload_x
                .saturating_sub(padding)
                .min(width.saturating_sub(1));
            let source_index = (y * width + x) * NOVA_ATLAS_BYTES_PER_PIXEL;
            let atlas_index = (upload_y * upload_width + upload_x) * NOVA_ATLAS_BYTES_PER_PIXEL;
            destination[atlas_index..atlas_index + 4]
                .copy_from_slice(&source[source_index..source_index + 4]);
        }
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
    let source_len = width
        .checked_mul(height)?
        .checked_mul(NOVA_ATLAS_BYTES_PER_PIXEL)?;
    let source = bytes.get(..source_len)?;
    let upload_len = upload_width
        .checked_mul(upload_height)?
        .checked_mul(NOVA_ATLAS_BYTES_PER_PIXEL)?;
    let destination = pixels.get_mut(..upload_len)?;

    for upload_y in 0..upload_height {
        let y = upload_y
            .saturating_sub(padding)
            .min(height.saturating_sub(1));
        for upload_x in 0..upload_width {
            let x = upload_x
                .saturating_sub(padding)
                .min(width.saturating_sub(1));
            let source_index = (y * width + x) * NOVA_ATLAS_BYTES_PER_PIXEL;
            let atlas_index = (upload_y * upload_width + upload_x) * NOVA_ATLAS_BYTES_PER_PIXEL;
            let source = &source[source_index..source_index + 4];
            destination[atlas_index] = source[2];
            destination[atlas_index + 1] = source[1];
            destination[atlas_index + 2] = source[0];
            destination[atlas_index + 3] = source[3];
        }
    }
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

    for upload_y in 0..upload_height {
        let y = upload_y
            .saturating_sub(padding)
            .min(height.saturating_sub(1));
        for upload_x in 0..upload_width {
            let x = upload_x
                .saturating_sub(padding)
                .min(width.saturating_sub(1));
            let coverage = source[y * width + x];
            let atlas_index = (upload_y * upload_width + upload_x) * NOVA_ATLAS_BYTES_PER_PIXEL;
            destination[atlas_index] = 0;
            destination[atlas_index + 1] = 0;
            destination[atlas_index + 2] = coverage;
            destination[atlas_index + 3] = 255;
        }
    }
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

    for upload_y in 0..upload_height {
        let y = upload_y
            .saturating_sub(padding)
            .min(height.saturating_sub(1));
        for upload_x in 0..upload_width {
            let x = upload_x
                .saturating_sub(padding)
                .min(width.saturating_sub(1));
            let source_index = (y * width + x) * NOVA_ATLAS_BYTES_PER_PIXEL;
            let coverage = subpixel_coverage(
                source[source_index],
                source[source_index + 1],
                source[source_index + 2],
                source[source_index + 3],
            );
            let atlas_index = (upload_y * upload_width + upload_x) * NOVA_ATLAS_BYTES_PER_PIXEL;
            destination[atlas_index] = 0;
            destination[atlas_index + 1] = 0;
            destination[atlas_index + 2] = coverage;
            destination[atlas_index + 3] = 255;
        }
    }
    Some(())
}

fn subpixel_coverage(red: u8, green: u8, blue: u8, alpha: u8) -> u8 {
    let coverage = u16::from(red)
        .saturating_add(u16::from(green))
        .saturating_add(u16::from(blue))
        / 3;
    let premultiplied_coverage = coverage.saturating_mul(u16::from(alpha)) / 255;
    u8::try_from(premultiplied_coverage).unwrap_or(u8::MAX)
}
