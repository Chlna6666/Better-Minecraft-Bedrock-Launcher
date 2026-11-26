use crate::{ObjectFit, Result, size};
use image::{ColorType, ImageDecoder as _, codecs::bmp::BmpDecoder};
use smallvec::SmallVec;
use std::io::Cursor;

use super::resample::{bgra_byte_len, intermediate_sample_size, resize_bgra_bytes, scaled_axis};
use crate::assets::{AnimatedFrame, RenderImage};
use crate::assets::{ImageRenderInfo, ImageRenderSize};

pub(super) fn frame(bytes: &[u8]) -> Result<AnimatedFrame> {
    let decoder = BmpDecoder::new(Cursor::new(bytes))?;
    let (width, height) = decoder.dimensions();
    let color_type = decoder.color_type();
    let byte_len = usize::try_from(decoder.total_bytes())
        .map_err(|_| anyhow::anyhow!("BMP decoded buffer size overflowed"))?;
    let mut pixels = if color_type == ColorType::Rgba8 {
        crate::acquire_bitmap_buffer(byte_len)
    } else {
        vec![0; byte_len]
    };
    decoder.read_image(&mut pixels)?;
    let bgra = image_pixels_to_bgra_bytes(pixels, color_type, width, height)?;
    Ok(AnimatedFrame::from_bgra_bytes(
        0,
        size(width.into(), height.into()),
        bgra,
    ))
}

pub(super) fn render_sized(
    bytes: &[u8],
    target: ImageRenderSize,
    object_fit: ObjectFit,
) -> Result<(RenderImage, ImageRenderInfo)> {
    let decoder = BmpDecoder::new(Cursor::new(bytes))?;
    let (original_width, original_height) = decoder.dimensions();
    let color_type = decoder.color_type();
    let original_size = size(original_width, original_height);
    let fitted_target = target.fit(original_size, object_fit);
    let sample_target = intermediate_sample_size(original_size, fitted_target);
    let output = sample_bmp_rows_to_bgra(
        decoder,
        original_width,
        original_height,
        color_type,
        sample_target,
    )?;
    let (image, render_path) = if sample_target == fitted_target {
        let frame = AnimatedFrame::from_bgra_bytes(0, sample_target.size(), output);
        (
            RenderImage::from_resident_frames(SmallVec::from_elem(frame, 1)),
            "bmp_decoded_sample",
        )
    } else {
        let (frame, render_path) =
            resize_bgra_bytes(output, sample_target, fitted_target, "bmp_decoded_sample")?;
        (
            RenderImage::from_resident_frames(SmallVec::from_elem(frame, 1)),
            render_path,
        )
    };
    Ok((
        image,
        ImageRenderInfo {
            original_width,
            original_height,
            size: fitted_target,
            render_path,
        },
    ))
}

fn sample_bmp_rows_to_bgra<R: std::io::BufRead + std::io::Seek>(
    decoder: BmpDecoder<R>,
    source_width: u32,
    source_height: u32,
    color_type: ColorType,
    sample_target: ImageRenderSize,
) -> Result<Vec<u8>> {
    let source_row_len = usize::from(color_type.bytes_per_pixel())
        .checked_mul(source_width as usize)
        .ok_or_else(|| anyhow::anyhow!("BMP source row size overflowed"))?;
    let output_len = bgra_byte_len(sample_target)?;
    let source_len = usize::try_from(decoder.total_bytes())
        .map_err(|_| anyhow::anyhow!("BMP decoded buffer size overflowed"))?;
    let mut source_pixels = crate::acquire_bitmap_buffer(source_len);
    // image's BMP read_rect decodes a full frame for every call, even for a single row.
    // Decode once; the former row path already allocated this full-frame scratch internally.
    decoder.read_image(&mut source_pixels)?;
    let mut output = crate::acquire_bitmap_buffer(output_len);

    for target_y in 0..sample_target.height {
        let source_y = scaled_axis(target_y, source_height, sample_target.height) as usize;
        let row_start = source_y * source_row_len;
        let source_row = &source_pixels[row_start..row_start + source_row_len];
        if let Err(error) = write_sampled_image_row(
            source_row,
            color_type,
            source_width,
            sample_target.width,
            &mut output,
            target_y,
        ) {
            crate::release_bitmap_buffer(source_pixels);
            crate::release_bitmap_buffer(output);
            return Err(error);
        }
    }
    crate::release_bitmap_buffer(source_pixels);
    Ok(output)
}

fn write_sampled_image_row(
    source_row: &[u8],
    color_type: ColorType,
    source_width: u32,
    target_width: u32,
    output: &mut [u8],
    target_y: u32,
) -> Result<()> {
    let output_row_start = target_y as usize * target_width as usize * 4;
    let output_row = &mut output[output_row_start..output_row_start + target_width as usize * 4];

    for target_x in 0..target_width {
        let source_x = scaled_axis(target_x, source_width, target_width) as usize;
        let out = &mut output_row[target_x as usize * 4..target_x as usize * 4 + 4];
        match color_type {
            ColorType::L8 => {
                let luma = source_row[source_x];
                out.copy_from_slice(&[luma, luma, luma, 255]);
            }
            ColorType::La8 => {
                let offset = source_x * 2;
                let luma = source_row[offset];
                out.copy_from_slice(&[luma, luma, luma, source_row[offset + 1]]);
            }
            ColorType::Rgb8 => {
                let offset = source_x * 3;
                out.copy_from_slice(&[
                    source_row[offset + 2],
                    source_row[offset + 1],
                    source_row[offset],
                    255,
                ]);
            }
            ColorType::Rgba8 => {
                let offset = source_x * 4;
                out.copy_from_slice(&[
                    source_row[offset + 2],
                    source_row[offset + 1],
                    source_row[offset],
                    source_row[offset + 3],
                ]);
            }
            ColorType::L16
            | ColorType::La16
            | ColorType::Rgb16
            | ColorType::Rgba16
            | ColorType::Rgb32F
            | ColorType::Rgba32F => {
                anyhow::bail!("unsupported sampled image row color type: {color_type:?}");
            }
            _ => anyhow::bail!("unsupported sampled image row color type: {color_type:?}"),
        }
    }

    Ok(())
}

fn image_pixels_to_bgra_bytes(
    mut pixels: Vec<u8>,
    color_type: ColorType,
    width: u32,
    height: u32,
) -> Result<Vec<u8>> {
    let pixel_count = width as usize * height as usize;
    anyhow::ensure!(
        pixels.len() == pixel_count.saturating_mul(usize::from(color_type.bytes_per_pixel())),
        "decoded image buffer dimensions were invalid"
    );
    if color_type == ColorType::Rgba8 {
        crate::swap_rgba_to_bgra_rows(&mut pixels, width as usize * 4, height as usize);
        return Ok(pixels);
    }
    let mut bgra = crate::acquire_bitmap_buffer_capacity(pixel_count * 4);
    match color_type {
        ColorType::L8 => {
            for &luma in &pixels {
                bgra.extend_from_slice(&[luma, luma, luma, 255]);
            }
        }
        ColorType::La8 => {
            for pixel in pixels.chunks_exact(2) {
                bgra.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
        }
        ColorType::Rgb8 => {
            for pixel in pixels.chunks_exact(3) {
                bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
            }
        }
        ColorType::L16 => {
            for luma in pixels.chunks_exact(2) {
                bgra.extend_from_slice(&[luma[0], luma[0], luma[0], 255]);
            }
        }
        ColorType::La16 => {
            for pixel in pixels.chunks_exact(4) {
                bgra.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[2]]);
            }
        }
        ColorType::Rgb16 => {
            for pixel in pixels.chunks_exact(6) {
                bgra.extend_from_slice(&[pixel[4], pixel[2], pixel[0], 255]);
            }
        }
        ColorType::Rgba16 => {
            for pixel in pixels.chunks_exact(8) {
                bgra.extend_from_slice(&[pixel[4], pixel[2], pixel[0], pixel[6]]);
            }
        }
        ColorType::Rgb32F | ColorType::Rgba32F => {
            anyhow::bail!("floating-point BMP decode is not supported for GPUI assets");
        }
        _ => anyhow::bail!("unsupported BMP color type: {color_type:?}"),
    }

    anyhow::ensure!(
        bgra.len() == pixel_count.saturating_mul(4),
        "decoded image buffer dimensions were invalid"
    );
    Ok(bgra)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_colors_preserve_bgra_pixels() {
        let cases = [
            (ColorType::L8, vec![23], [23, 23, 23, 255]),
            (ColorType::La8, vec![23, 79], [23, 23, 23, 79]),
            (ColorType::Rgb8, vec![11, 23, 37], [37, 23, 11, 255]),
            (ColorType::Rgba8, vec![11, 23, 37, 79], [37, 23, 11, 79]),
            (ColorType::L16, vec![23, 127], [23, 23, 23, 255]),
            (ColorType::La16, vec![23, 127, 79, 251], [23, 23, 23, 79]),
            (
                ColorType::Rgb16,
                vec![11, 127, 23, 128, 37, 129],
                [37, 23, 11, 255],
            ),
            (
                ColorType::Rgba16,
                vec![11, 127, 23, 128, 37, 129, 79, 251],
                [37, 23, 11, 79],
            ),
        ];
        for (color_type, input, expected) in cases {
            assert_eq!(
                image_pixels_to_bgra_bytes(input, color_type, 1, 1).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn rgba_conversion_reuses_the_decode_allocation_including_simd_tails() {
        for width in [1, 3, 17, 67] {
            let pixels: Vec<u8> = (0..width as usize * 3 * 4)
                .map(|index| index.wrapping_mul(37) as u8)
                .collect();
            let pointer = pixels.as_ptr();
            let mut expected = pixels.clone();
            for pixel in expected.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            let output = image_pixels_to_bgra_bytes(pixels, ColorType::Rgba8, width, 3).unwrap();
            assert_eq!(output.as_ptr(), pointer);
            assert_eq!(output, expected);
        }
    }

    #[test]
    fn malformed_or_float_pixels_are_rejected() {
        assert!(image_pixels_to_bgra_bytes(vec![1; 3], ColorType::Rgba8, 1, 1).is_err());
        assert!(image_pixels_to_bgra_bytes(vec![1; 5], ColorType::Rgb8, 1, 1).is_err());
        assert!(image_pixels_to_bgra_bytes(vec![0; 12], ColorType::Rgb32F, 1, 1).is_err());
        assert!(frame(b"not a BMP").is_err());
    }

    #[test]
    fn large_bmp_sampling_matches_full_decode_pixels() {
        use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
        let pixels = RgbaImage::from_fn(1920, 1080, |x, y| {
            Rgba([x as u8, y as u8, (x ^ y) as u8, 255])
        });
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(pixels)
            .write_to(&mut encoded, ImageFormat::Bmp)
            .unwrap();
        let reference = image::load_from_memory_with_format(encoded.get_ref(), ImageFormat::Bmp)
            .unwrap()
            .into_rgba8();
        for (width, height) in [(320, 180), (17, 3)] {
            let target = ImageRenderSize::new(width, height).unwrap();
            let decoder = BmpDecoder::new(Cursor::new(encoded.get_ref())).unwrap();
            let color_type = decoder.color_type();
            let sampled = sample_bmp_rows_to_bgra(decoder, 1920, 1080, color_type, target).unwrap();
            for y in 0..height {
                for x in 0..width {
                    let source = reference.get_pixel(x * 1920 / width, y * 1080 / height);
                    let offset = (y as usize * width as usize + x as usize) * 4;
                    assert_eq!(
                        &sampled[offset..offset + 4],
                        &[source[2], source[1], source[0], source[3]]
                    );
                }
            }
            crate::release_bitmap_buffer(sampled);
        }
    }
}
