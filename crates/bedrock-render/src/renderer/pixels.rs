use fearless_simd::u32x4;
use fearless_simd::{dispatch, prelude::*, Level, Simd};
use std::sync::LazyLock;

use super::pipeline::{RenderSimdPolicy, TilePixelFormat};

const PACKED_PIXEL_LANES: usize = 4;
static SIMD_LEVEL: LazyLock<Level> = LazyLock::new(Level::new);

/// Packs palette-resolved `u32` words into the requested native byte order.
/// Large contiguous buffers use the runtime-selected safe SIMD kernel; small
/// buffers and the explicit scalar policy stay on the scalar path.
pub(super) fn pack_colors(
    colors: &[u32],
    pixels: &mut [u8],
    pixel_format: TilePixelFormat,
    policy: RenderSimdPolicy,
) {
    debug_assert_eq!(pixels.len(), colors.len().saturating_mul(4));
    if !policy.uses_simd(pixels.len()) {
        scalar_pack_colors(colors, pixels, pixel_format);
        return;
    }
    let level = *SIMD_LEVEL;
    if level.is_fallback() {
        scalar_pack_colors(colors, pixels, pixel_format);
    } else {
        dispatch!(level, simd => vector_pack_colors(simd, colors, pixels, pixel_format));
    }
}

fn scalar_pack_colors(colors: &[u32], pixels: &mut [u8], pixel_format: TilePixelFormat) {
    for (color, pixel) in colors.iter().copied().zip(pixels.chunks_exact_mut(4)) {
        let [red, green, blue, alpha] = color.to_le_bytes();
        match pixel_format {
            TilePixelFormat::Rgba8 => pixel.copy_from_slice(&[red, green, blue, alpha]),
            TilePixelFormat::Bgra8 => pixel.copy_from_slice(&[blue, green, red, alpha]),
        }
    }
}

fn vector_pack_colors<S: Simd>(
    simd: S,
    colors: &[u32],
    pixels: &mut [u8],
    pixel_format: TilePixelFormat,
) {
    let mut color_chunks = colors.chunks_exact(PACKED_PIXEL_LANES);
    let mut pixel_chunks = pixels.chunks_exact_mut(PACKED_PIXEL_LANES * 4);
    for (color_chunk, pixel_chunk) in color_chunks.by_ref().zip(pixel_chunks.by_ref()) {
        let vector = u32x4::from_slice(simd, color_chunk);
        let packed = match pixel_format {
            TilePixelFormat::Rgba8 => vector,
            TilePixelFormat::Bgra8 => {
                let mask = u32x4::splat(simd, 0x00ff_00ff);
                let red = (vector & mask) << 16;
                let blue = (vector >> 16) & mask;
                (vector & !mask) | red | blue
            }
        };
        let mut words = [0_u32; PACKED_PIXEL_LANES];
        packed.store_slice(&mut words);
        for (word, pixel) in words.iter().zip(pixel_chunk.chunks_exact_mut(4)) {
            pixel.copy_from_slice(&word.to_le_bytes());
        }
    }
    scalar_pack_colors(
        color_chunks.remainder(),
        pixel_chunks.into_remainder(),
        pixel_format,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{hint::black_box, time::Instant};

    #[test]
    fn packed_colors_match_scalar_for_both_orders() {
        let colors: Vec<u32> = (0_u32..256)
            .map(|index| {
                u32::from(index as u8)
                    | (u32::from(index.wrapping_mul(3) as u8) << 8)
                    | (u32::from(index.wrapping_mul(5) as u8) << 16)
                    | (u32::from(index.wrapping_mul(7) as u8) << 24)
            })
            .collect();
        for pixel_format in [TilePixelFormat::Rgba8, TilePixelFormat::Bgra8] {
            let mut expected = vec![0; colors.len() * 4];
            scalar_pack_colors(&colors, &mut expected, pixel_format);
            let mut actual = vec![0; colors.len() * 4];
            pack_colors(
                &colors,
                &mut actual,
                pixel_format,
                RenderSimdPolicy::Auto,
            );
            assert_eq!(actual, expected);
        }
    }

    #[test]
    #[ignore = "manual release-mode instruction-set comparison"]
    fn benchmark_packed_color_instruction_sets() {
        let colors: Vec<u32> = (0_u32..256 * 256)
            .map(|index| {
                u32::from(index as u8)
                    | (u32::from(index.wrapping_mul(3) as u8) << 8)
                    | (u32::from(index.wrapping_mul(5) as u8) << 16)
                    | (u32::from(index.wrapping_mul(7) as u8) << 24)
            })
            .collect();
        let measure = |label: &str,
                       pixel_format: TilePixelFormat,
                       mut run: Box<dyn FnMut(&[u32], &mut [u8])>| {
            let mut pixels = vec![0; colors.len() * 4];
            for _ in 0..8 {
                run(&colors, &mut pixels);
            }
            let start = Instant::now();
            for _ in 0..80 {
                run(black_box(&colors), black_box(&mut pixels));
            }
            println!(
                "simd_pack level={label} format={pixel_format:?} ns_per_tile={}",
                start.elapsed().as_nanos() / 80
            );
        };
        for pixel_format in [TilePixelFormat::Rgba8, TilePixelFormat::Bgra8] {
            measure(
                "scalar",
                pixel_format,
                Box::new(move |colors, pixels| {
                    scalar_pack_colors(colors, pixels, pixel_format);
                }),
            );
            let level = *SIMD_LEVEL;
            #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
            {
                if let Some(sse2) = level.as_sse2() {
                    measure(
                        "sse2",
                        pixel_format,
                        Box::new(move |colors, pixels| {
                            dispatch!(
                                Level::Sse2(sse2),
                                simd => vector_pack_colors(simd, colors, pixels, pixel_format)
                            );
                        }),
                    );
                }
                if let Some(sse42) = level.as_sse4_2() {
                    measure(
                        "sse4.2",
                        pixel_format,
                        Box::new(move |colors, pixels| {
                            dispatch!(
                                Level::Sse4_2(sse42),
                                simd => vector_pack_colors(simd, colors, pixels, pixel_format)
                            );
                        }),
                    );
                }
                if let Some(avx2) = level.as_avx2() {
                    measure(
                        "avx2",
                        pixel_format,
                        Box::new(move |colors, pixels| {
                            dispatch!(
                                Level::Avx2(avx2),
                                simd => vector_pack_colors(simd, colors, pixels, pixel_format)
                            );
                        }),
                    );
                }
            }
            measure(
                "auto",
                pixel_format,
                Box::new(move |colors, pixels| {
                    pack_colors(colors, pixels, pixel_format, RenderSimdPolicy::Auto)
                }),
            );
        }
    }
}
