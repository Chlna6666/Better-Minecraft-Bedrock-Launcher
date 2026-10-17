//! Image decoding and mip-chain generation for imported texture assets.

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
pub(crate) fn decode_texture(
    bytes: &[u8],
    srgb: bool,
    max_dimension: u32,
    generate_mipmaps: bool,
) -> Result<TextureAsset, ImageError> {
    let decoded =
        image::load_from_memory(bytes).map_err(|error| ImageError::Decode(error.to_string()))?;
    let rgba = decoded.to_rgba8();
    let (width, height) = rgba.dimensions();
    if width == 0 || height == 0 {
        return Err(ImageError::Decode("image has no pixels".into()));
    }
    if width > max_dimension || height > max_dimension {
        return Err(ImageError::TooLarge {
            width,
            height,
            limit: max_dimension,
        });
    }
    let base: Vec<u8> = rgba.into_raw();
    if !generate_mipmaps {
        return if srgb {
            Ok(TextureAsset::rgba8(width, height, base)?)
        } else {
            Ok(TextureAsset::linear_rgba8(width, height, base)?)
        };
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
    if srgb {
        Ok(TextureAsset::rgba8_mip_chain(width, height, levels)?)
    } else {
        Ok(TextureAsset::linear_rgba8_mip_chain(width, height, levels)?)
    }
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
