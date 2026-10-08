use super::resample::{
    bgra_byte_len, intermediate_sample_size, render_sized as render_animated_image,
    resize_bgra_bytes, scaled_axis,
};
use crate::assets::{AnimatedFrame, EncodedImage, RenderImage};
use crate::assets::{AnimatedImageConfig, ImageRenderInfo, ImageRenderSize};
use crate::{ObjectFit, Result, size};
use smallvec::SmallVec;
use std::io::Cursor;

pub(super) fn render_sized(
    source: EncodedImage,
    config: AnimatedImageConfig,
    target: ImageRenderSize,
    object_fit: ObjectFit,
) -> Result<(RenderImage, ImageRenderInfo)> {
    let mut decoder = png::Decoder::new(Cursor::new(source.bytes.as_bytes()));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info()?;
    let info = reader.info().clone();

    if info.animation_control.is_some() || info.interlaced {
        drop(reader);
        return render_animated_image(source, config, target, object_fit);
    }

    let original_size = size(info.width, info.height);
    let fitted_target = target.fit(original_size, object_fit);
    let sample_target = intermediate_sample_size(original_size, fitted_target);
    let (color_type, bit_depth) = reader.output_color_type();
    anyhow::ensure!(
        bit_depth == png::BitDepth::Eight,
        "target-size PNG row decode expected 8-bit output, got {bit_depth:?}"
    );

    let output = sample_png_rows_to_bgra(
        &mut reader,
        info.width,
        info.height,
        color_type,
        sample_target,
    )?;
    let (image, render_path) = if sample_target == fitted_target {
        let frame = AnimatedFrame::from_bgra_bytes(0, sample_target.size(), output);
        (
            RenderImage::from_resident_frames(SmallVec::from_elem(frame, 1)),
            "png_row_sample",
        )
    } else {
        let (frame, render_path) =
            resize_bgra_bytes(output, sample_target, fitted_target, "png_row_sample")?;
        (
            RenderImage::from_resident_frames(SmallVec::from_elem(frame, 1)),
            render_path,
        )
    };
    Ok((
        image,
        ImageRenderInfo {
            original_width: original_size.width,
            original_height: original_size.height,
            size: fitted_target,
            render_path,
        },
    ))
}

fn sample_png_rows_to_bgra<R: std::io::BufRead + std::io::Seek>(
    reader: &mut png::Reader<R>,
    source_width: u32,
    source_height: u32,
    color_type: png::ColorType,
    sample_target: ImageRenderSize,
) -> Result<Vec<u8>> {
    let source_row_len = reader
        .output_line_size(source_width)
        .ok_or_else(|| anyhow::anyhow!("PNG row size overflowed"))?;
    let output_len = bgra_byte_len(sample_target)?;
    let mut source_row = vec![0; source_row_len];
    let mut output = crate::acquire_bitmap_buffer(output_len);
    let mut next_target_y = 0u32;

    for source_y in 0..source_height {
        if reader.read_row(&mut source_row)?.is_none() {
            break;
        }

        while next_target_y < sample_target.height
            && scaled_axis(next_target_y, source_height, sample_target.height) == source_y
        {
            write_sampled_png_row(
                &source_row,
                color_type,
                source_width,
                sample_target.width,
                &mut output,
                next_target_y,
            )?;
            next_target_y += 1;
        }
    }

    anyhow::ensure!(
        next_target_y == sample_target.height,
        "PNG row decoder ended before filling target image"
    );

    Ok(output)
}

fn write_sampled_png_row(
    source_row: &[u8],
    color_type: png::ColorType,
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
            png::ColorType::Grayscale => {
                let luma = source_row[source_x];
                out.copy_from_slice(&[luma, luma, luma, 255]);
            }
            png::ColorType::GrayscaleAlpha => {
                let offset = source_x * 2;
                let luma = source_row[offset];
                out.copy_from_slice(&[luma, luma, luma, source_row[offset + 1]]);
            }
            png::ColorType::Rgb => {
                let offset = source_x * 3;
                out.copy_from_slice(&[
                    source_row[offset + 2],
                    source_row[offset + 1],
                    source_row[offset],
                    255,
                ]);
            }
            png::ColorType::Rgba => {
                let offset = source_x * 4;
                out.copy_from_slice(&[
                    source_row[offset + 2],
                    source_row[offset + 1],
                    source_row[offset],
                    source_row[offset + 3],
                ]);
            }
            png::ColorType::Indexed => {
                anyhow::bail!("indexed PNG output was not expanded before target-size sampling");
            }
        }
    }

    Ok(())
}

pub(super) fn frame(bytes: &[u8]) -> Result<AnimatedFrame> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info()?;
    let output_len = reader
        .output_buffer_size()
        .ok_or_else(|| anyhow::anyhow!("PNG decoded buffer size overflowed"))?;
    let mut pixels = if reader.output_color_type().0 == png::ColorType::Rgba {
        // Four-channel output becomes BitmapBytes directly and returns to this pool on drop.
        crate::acquire_bitmap_buffer(output_len)
    } else {
        vec![0; output_len]
    };
    let output_info = reader.next_frame(&mut pixels)?;
    pixels.truncate(output_info.buffer_size());
    let bgra = png_pixels_to_bgra_bytes(
        pixels,
        output_info.color_type,
        output_info.width,
        output_info.height,
    )?;
    Ok(AnimatedFrame::from_bgra_bytes(
        0,
        size(output_info.width.into(), output_info.height.into()),
        bgra,
    ))
}

fn png_pixels_to_bgra_bytes(
    mut pixels: Vec<u8>,
    color_type: png::ColorType,
    width: u32,
    height: u32,
) -> Result<Vec<u8>> {
    let pixel_count = width as usize * height as usize;
    let mut bgra = if color_type == png::ColorType::Rgba {
        Vec::new()
    } else {
        Vec::with_capacity(pixel_count * 4)
    };
    match color_type {
        png::ColorType::Grayscale => {
            for &luma in &pixels {
                bgra.extend_from_slice(&[luma, luma, luma, 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for pixel in pixels.chunks_exact(2) {
                bgra.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
        }
        png::ColorType::Rgb => {
            for pixel in pixels.chunks_exact(3) {
                bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
            }
        }
        png::ColorType::Rgba => {
            crate::swap_rgba_to_bgra_rows(&mut pixels, width as usize * 4, height as usize);
            // The decoder already owns the full four-channel output; retain it without a
            // second image-sized allocation or copy. Alpha and source dimensions are unchanged.
            bgra = pixels;
        }
        png::ColorType::Indexed => {
            anyhow::bail!("indexed PNG output was not expanded before static decode");
        }
    }

    anyhow::ensure!(
        bgra.len() == pixel_count.saturating_mul(4),
        "PNG decoded buffer dimensions were invalid"
    );
    Ok(bgra)
}

#[cfg(test)]
mod tests {
    use super::*;

    mod profile;

    #[test]
    fn rgba_conversion_reuses_decoder_storage_and_preserves_alpha() {
        let pixels = vec![11, 22, 33, 0, 44, 55, 66, 127, 77, 88, 99, 255];
        let address = pixels.as_ptr();
        let output =
            png_pixels_to_bgra_bytes(pixels, png::ColorType::Rgba, 3, 1).expect("valid pixels");
        assert_eq!(output.as_ptr(), address);
        assert_eq!(output, [33, 22, 11, 0, 66, 55, 44, 127, 99, 88, 77, 255]);
    }

    #[test]
    fn rgba_conversion_preserves_simd_rows_and_scalar_tails() {
        let pixels: Vec<u8> = (0..17 * 3 * 4).map(|index| (index * 37) as u8).collect();
        let mut expected = pixels.clone();
        for pixel in expected.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        let address = pixels.as_ptr();
        let output =
            png_pixels_to_bgra_bytes(pixels, png::ColorType::Rgba, 17, 3).expect("valid pixels");
        assert_eq!(output.as_ptr(), address);
        assert_eq!(output, expected);
    }

    #[test]
    fn static_rgba_decode_preserves_exact_pixels() {
        let pixels = [11, 22, 33, 0, 44, 55, 66, 127, 77, 88, 99, 255];
        let mut encoded = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut encoded, 3, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .expect("header")
                .write_image_data(&pixels)
                .expect("pixels");
        }
        let image = frame(&encoded).expect("decode");
        assert_eq!(
            image.bytes.as_slice(),
            [33, 22, 11, 0, 66, 55, 44, 127, 99, 88, 77, 255]
        );
        assert_eq!(image.size, size(3.into(), 1.into()));
    }

    #[test]
    fn rgba_conversion_rejects_invalid_dimensions() {
        assert!(png_pixels_to_bgra_bytes(vec![1, 2, 3], png::ColorType::Rgba, 1, 1).is_err());
    }
}
