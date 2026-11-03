//! Color coverage and linear material channels require different mip filters.
use gpui_3d::TextureAsset;
use image::{Rgba, RgbaImage};
use std::sync::{Arc, OnceLock};

pub(super) fn mipmapped_texture(image: RgbaImage) -> Option<TextureAsset> {
    mipmapped_texture_with(image, downsample_texture)
}

pub(super) fn mipmapped_data_texture(image: RgbaImage) -> Option<TextureAsset> {
    mipmapped_texture_with(image, downsample_data_texture)
}

fn mipmapped_texture_with(
    mut image: RgbaImage,
    downsample: fn(&RgbaImage) -> RgbaImage,
) -> Option<TextureAsset> {
    let (width, height) = image.dimensions();
    let mut levels = Vec::new();
    loop {
        let next = (image.width() != 1 || image.height() != 1).then(|| downsample(&image));
        levels.push(Arc::<[u8]>::from(image.into_raw()));
        let Some(next) = next else {
            break;
        };
        image = next;
    }
    // The unlit scene view writes directly to a UNORM target, so keep the source's
    // display-encoded RGB values intact instead of applying an sRGB decode on upload.
    TextureAsset::linear_rgba8_mip_chain(width, height, levels).ok()
}

pub(super) fn downsample_texture(source: &RgbaImage) -> RgbaImage {
    static SRGB_TO_LINEAR: OnceLock<[f32; 256]> = OnceLock::new();
    static LINEAR_TO_SRGB: OnceLock<Box<[u8; 65536]>> = OnceLock::new();
    let decode = SRGB_TO_LINEAR.get_or_init(|| {
        std::array::from_fn(|index| {
            let value = index as f32 / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        })
    });
    let encode = LINEAR_TO_SRGB.get_or_init(|| {
        Box::new(std::array::from_fn(|index| {
            let value = index as f32 / 65535.0;
            let encoded = if value <= 0.0031308 {
                value * 12.92
            } else {
                1.055 * value.powf(1.0 / 2.4) - 0.055
            };
            (encoded * 255.0).round() as u8
        }))
    });
    let width = (source.width() / 2).max(1);
    let height = (source.height() / 2).max(1);
    RgbaImage::from_fn(width, height, |x, y| {
        let (x0, x1) = (x * source.width() / width, (x + 1) * source.width() / width);
        let (y0, y1) = (
            y * source.height() / height,
            (y + 1) * source.height() / height,
        );
        let mut linear = [0.0_f32; 3];
        let mut alpha_sum = 0.0_f32;
        let count = (x1 - x0) * (y1 - y0);
        for sample_y in y0..y1 {
            for sample_x in x0..x1 {
                let pixel = source.get_pixel(sample_x, sample_y).0;
                let alpha = f32::from(pixel[3]) / 255.0;
                alpha_sum += alpha;
                for channel in 0..3 {
                    linear[channel] += decode[usize::from(pixel[channel])] * alpha;
                }
            }
        }
        let mut result = [0_u8; 4];
        if alpha_sum > 0.0 {
            for channel in 0..3 {
                let value = (linear[channel] / alpha_sum).clamp(0.0, 1.0);
                result[channel] = encode[(value * 65535.0).round() as usize];
            }
        }
        result[3] = (alpha_sum * 255.0 / count as f32).round() as u8;
        Rgba(result)
    })
}

fn downsample_data_texture(source: &RgbaImage) -> RgbaImage {
    let width = (source.width() / 2).max(1);
    let height = (source.height() / 2).max(1);
    RgbaImage::from_fn(width, height, |x, y| {
        let (x0, x1) = (x * source.width() / width, (x + 1) * source.width() / width);
        let (y0, y1) = (
            y * source.height() / height,
            (y + 1) * source.height() / height,
        );
        let count = (x1 - x0) * (y1 - y0);
        let mut sum = [0u32; 4];
        for sy in y0..y1 {
            for sx in x0..x1 {
                let pixel = source.get_pixel(sx, sy);
                for c in 0..4 {
                    sum[c] += u32::from(pixel[c]);
                }
            }
        }
        Rgba(sum.map(|value| ((value + count / 2) / count) as u8))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn material_mips_preserve_roughness_when_subsurface_is_zero() {
        let texture =
            mipmapped_data_texture(RgbaImage::from_pixel(16, 16, Rgba([0, 0, 187, 0]))).unwrap();
        for level in 0..texture.mip_level_count() {
            assert!(
                texture
                    .mip_pixels(level)
                    .unwrap()
                    .chunks_exact(4)
                    .all(|p| p == [0, 0, 187, 0]),
                "mip {level}"
            );
        }
        // The color filter intentionally suppresses RGB of transparent pixels.
        assert_eq!(
            downsample_texture(&RgbaImage::from_pixel(2, 2, Rgba([0, 0, 187, 0])))
                .get_pixel(0, 0)
                .0,
            [0; 4]
        );
    }
    #[test]
    fn material_mips_average_linear_channels_without_alpha_weighting() {
        let image = RgbaImage::from_fn(2, 2, |x, _| {
            if x == 0 {
                Rgba([0, 64, 0, 0])
            } else {
                Rgba([128, 192, 200, 255])
            }
        });
        assert_eq!(
            downsample_data_texture(&image).get_pixel(0, 0).0,
            [64, 128, 100, 128]
        );
    }
}
