use image::RgbaImage;

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
use fearless_simd::{Avx2, Level, Simd, dispatch, prelude::*, u32x8};

/// Alpha classes distinguish fully opaque, cutout and blended textures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TextureAlphaContent {
    Opaque,
    Binary,
    Partial,
    Unknown,
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
static AVX2: std::sync::LazyLock<Option<Avx2>> =
    std::sync::LazyLock::new(|| Level::new().as_avx2());

pub(super) fn texture_alpha_content(image: &RgbaImage) -> TextureAlphaContent {
    // ImageBuffer may have extra backing bytes past the logical image.
    classify(&image.as_raw()[..image.pixels().len() * 4])
}

fn classify(rgba: &[u8]) -> TextureAlphaContent {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    if rgba.len() >= 1024 {
        // Translucent textures commonly exit in their first few pixels.
        let (prefix, rest) = rgba.split_at(64);
        let start = scalar(prefix);
        if start == TextureAlphaContent::Partial {
            return start;
        }
        let tail = if let Some(avx2) = *AVX2
            && let Ok(words) = bytemuck::try_cast_slice(rest)
        {
            dispatch!(Level::Avx2(avx2), simd => vector(simd, words))
        } else {
            scalar(rest)
        };
        return match tail {
            TextureAlphaContent::Opaque => start,
            other => other,
        };
    }
    scalar(rgba)
}

fn scalar(rgba: &[u8]) -> TextureAlphaContent {
    let mut transparent = false;
    for pixel in rgba.as_chunks::<4>().0 {
        match pixel[3] {
            255 => {}
            0 => transparent = true,
            _ => return TextureAlphaContent::Partial,
        }
    }
    if transparent {
        TextureAlphaContent::Binary
    } else {
        TextureAlphaContent::Opaque
    }
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[expect(clippy::inline_always, reason = "fearless SIMD dispatch ISA boundary")]
#[inline(always)]
fn vector<S: Simd>(simd: S, words: &[u32]) -> TextureAlphaContent {
    let (blocks, tail) = words.as_chunks::<8>();
    let mask = u32x8::splat(simd, 0xff00_0000);
    let zero = u32x8::splat(simd, 0);
    let mut transparent = false;
    for block in blocks {
        let alpha = u32x8::from_slice(simd, block) & mask;
        let opaque = alpha.simd_eq(mask);
        let clear = alpha.simd_eq(zero);
        if !(opaque | clear).all_true() {
            return TextureAlphaContent::Partial;
        }
        transparent |= clear.any_true();
    }
    match scalar(bytemuck::cast_slice(tail)) {
        TextureAlphaContent::Opaque if transparent => TextureAlphaContent::Binary,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_alpha_identically_at_every_prefix_vector_and_tail_position() {
        for offset in 0..4 {
            let mut bytes = vec![0; 263 * 4 + offset];
            let rgba = &mut bytes[offset..];
            for pixel in rgba.chunks_exact_mut(4) {
                pixel[3] = 255;
            }
            assert_eq!(classify(rgba), TextureAlphaContent::Opaque);
            for pixel in 0..263 {
                let index = pixel * 4 + 3;
                for alpha in [0, 1, 128, 254] {
                    rgba[index] = alpha;
                    assert_eq!(classify(rgba), scalar(rgba));
                }
                rgba[index] = 255;
            }
        }
    }

    #[test]
    fn unused_backing_pixels_do_not_change_classification() {
        let image = RgbaImage::from_raw(16, 16, [vec![255; 1024], vec![128; 4]].concat()).unwrap();
        assert_eq!(texture_alpha_content(&image), TextureAlphaContent::Opaque);
    }
}
