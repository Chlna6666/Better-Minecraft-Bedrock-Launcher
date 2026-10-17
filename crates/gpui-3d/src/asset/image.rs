//! Image decoding, optional downscaling, and mip-chain generation for imported textures.

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Image extents are bounded by the configured texture limit before any float ratio is applied, so the narrowing back to u32 is safe and cannot lose a sign."
)]

use crate::{TextureAsset, TextureError};
use std::sync::Arc;

/// Errors decoding an embedded or external image.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ImageError {
    #[error("image decoding failed: {0}")]
    Decode(String),
    #[error("image is {width}x{height}, exceeding the configured {limit}x{limit} limit")]
    TooLarge { width: u32, height: u32, limit: u32 },
    #[error("texture asset creation failed: {0}")]
    Texture(#[from] TextureError),
}

/// Decodes PNG/JPEG bytes into an RGBA8 texture with a complete box-filtered mip chain.
///
/// Returns the texture and the original extent when the image was downscaled to fit the
/// configured maximum dimension.
pub(crate) fn decode_texture(
    bytes: &[u8],
    srgb: bool,
    max_dimension: u32,
    generate_mipmaps: bool,
    downscale: bool,
) -> Result<(TextureAsset, Option<(u32, u32)>), ImageError> {
    let decoded =
        image::load_from_memory(bytes).map_err(|error| ImageError::Decode(error.to_string()))?;
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    if width == 0 || height == 0 {
        return Err(ImageError::Decode("image has no pixels".into()));
    }
    if width > max_dimension || height > max_dimension {
        if !downscale {
            return Err(ImageError::TooLarge {
                width,
                height,
                limit: max_dimension,
            });
        }
        let scale = (max_dimension as f32 / width as f32).min(max_dimension as f32 / height as f32);
        let next_width = ((width as f32 * scale) as u32).max(1);
        let next_height = ((height as f32 * scale) as u32).max(1);
        let resized = image::imageops::resize(
            &rgba,
            next_width,
            next_height,
            image::imageops::FilterType::Triangle,
        );
        return Ok((
            texture_from_rgba(
                resized.into_raw(),
                next_width,
                next_height,
                srgb,
                generate_mipmaps,
            )?,
            Some((width, height)),
        ));
    }
    let base = rgba.into_raw();
    Ok((
        texture_from_rgba(base, width, height, srgb, generate_mipmaps)?,
        None,
    ))
}

fn texture_from_rgba(
    base: Vec<u8>,
    width: u32,
    height: u32,
    srgb: bool,
    generate_mipmaps: bool,
) -> Result<TextureAsset, ImageError> {
    if !generate_mipmaps {
        return Ok(if srgb {
            TextureAsset::rgba8(width, height, base)?
        } else {
            TextureAsset::linear_rgba8(width, height, base)?
        });
    }
    let mut levels: Vec<Arc<[u8]>> = vec![Arc::from(base)];
    let (mut level_width, mut level_height) = (width, height);
    while level_width > 1 || level_height > 1 {
        let next_width = (level_width / 2).max(1);
        let next_height = (level_height / 2).max(1);
        let downsampled = downsample(
            &levels[levels.len() - 1],
            level_width,
            level_height,
            next_width,
            next_height,
        );
        levels.push(Arc::from(downsampled));
        level_width = next_width;
        level_height = next_height;
    }
    Ok(if srgb {
        TextureAsset::rgba8_mip_chain(width, height, levels)?
    } else {
        TextureAsset::linear_rgba8_mip_chain(width, height, levels)?
    })
}

fn downsample(
    source: &[u8],
    width: u32,
    height: u32,
    next_width: u32,
    next_height: u32,
) -> Vec<u8> {
    let mut output = vec![0_u8; next_width as usize * next_height as usize * 4];
    for y in 0..next_height {
        for x in 0..next_width {
            let mut accumulator = [0_u32; 4];
            let mut samples = 0_u32;
            for dy in 0..2 {
                for dx in 0..2 {
                    let source_x = (x * 2 + dx).min(width - 1);
                    let source_y = (y * 2 + dy).min(height - 1);
                    let offset = (source_y as usize * width as usize + source_x as usize) * 4;
                    for channel in 0..4 {
                        accumulator[channel] += u32::from(source[offset + channel]);
                    }
                    samples += 1;
                }
            }
            let offset = (y as usize * next_width as usize + x as usize) * 4;
            for channel in 0..4 {
                output[offset + channel] =
                    u8::try_from(accumulator[channel] / samples).unwrap_or(255);
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::decode_texture;

    fn encode_png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        let image = image::RgbaImage::from_fn(width, height, |x, y| {
            image::Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255])
        });
        image::DynamicImage::ImageRgba8(image)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .expect("encode png");
        bytes
    }

    #[test]
    fn oversized_images_downscale_to_the_configured_limit() {
        let png = encode_png(64, 16);
        let (texture, resized_from) =
            decode_texture(&png, true, 32, true, true).expect("downscale");
        assert_eq!(resized_from, Some((64, 16)));
        assert_eq!((texture.width(), texture.height()), (32, 8));
        assert!(texture.mip_level_count() > 1);
    }

    #[test]
    fn oversized_images_fail_without_downscaling() {
        let png = encode_png(64, 16);
        let error = decode_texture(&png, true, 32, true, false).expect_err("too large");
        assert!(matches!(error, super::ImageError::TooLarge { .. }));
    }

    #[test]
    fn invalid_image_bytes_are_reported_without_panicking() {
        let error = decode_texture(b"not an image", true, 64, true, true)
            .expect_err("garbage bytes must not decode");
        assert!(matches!(error, super::ImageError::Decode(_)));
    }

    #[test]
    fn mip_generation_can_be_disabled() {
        let png = encode_png(8, 8);
        let (texture, resized_from) = decode_texture(&png, false, 32, false, true).expect("decode");
        assert_eq!(resized_from, None);
        assert_eq!(texture.mip_level_count(), 1);
    }
}
