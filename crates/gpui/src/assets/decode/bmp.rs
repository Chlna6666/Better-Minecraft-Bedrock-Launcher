use crate::{ObjectFit, Result, size};
use image::{ColorType, ImageDecoder as _, ImageDecoderRect as _, codecs::bmp::BmpDecoder};
use smallvec::SmallVec;
use std::io::{BufReader, Cursor};

use super::target::{
    bgra_bytes_to_rgba_image, bgra_len, fitted_target_size, high_quality_intermediate_target,
    resize_rgba_to_target, source_axis_for_target,
};
use crate::assets::render_image::{AnimatedFrame, RenderImage};
use crate::assets::types::{ImageDecodeTarget, TargetImageDecodeMetadata};

pub(super) fn decode_static_bmp_frame(bytes: &[u8]) -> Result<AnimatedFrame> {
    let decoder = BmpDecoder::new(Cursor::new(bytes))?;
    let (width, height) = decoder.dimensions();
    let color_type = decoder.color_type();
    let byte_len = usize::try_from(decoder.total_bytes())
        .map_err(|_| anyhow::anyhow!("BMP decoded buffer size overflowed"))?;
    let mut pixels = vec![0; byte_len];
    decoder.read_image(&mut pixels)?;
    let bgra = image_pixels_to_bgra_bytes(&pixels, color_type, width, height)?;
    Ok(AnimatedFrame::from_bgra_bytes(
        0,
        size(width.into(), height.into()),
        bgra,
    ))
}

pub(super) fn decode_bmp_to_target(
    bytes: &[u8],
    target: ImageDecodeTarget,
    object_fit: ObjectFit,
) -> Result<(RenderImage, TargetImageDecodeMetadata)> {
    let mut decoder = BmpDecoder::new(Cursor::new(bytes))?;
    let (original_width, original_height) = decoder.dimensions();
    let color_type = decoder.color_type();
    let original_size = size(original_width, original_height);
    let fitted_target = fitted_target_size(original_size, target, object_fit);
    let sample_target = high_quality_intermediate_target(original_size, fitted_target);
    let output = sample_bmp_rows_to_bgra(
        &mut decoder,
        original_width,
        original_height,
        color_type,
        sample_target,
    )?;
    let (image, decode_mode) = if sample_target == fitted_target {
        let frame = AnimatedFrame::from_bgra_bytes(0, sample_target.size(), output);
        (
            RenderImage::from_resident_frames(SmallVec::from_elem(frame, 1)),
            "bmp_rect_sample_decode",
        )
    } else {
        let rgba = bgra_bytes_to_rgba_image(output, sample_target)?;
        let (rgba, decode_mode) =
            resize_rgba_to_target(rgba, fitted_target, "bmp_rect_sample_decode")?;
        let frame = AnimatedFrame::from_rgba_image(0, rgba);
        (
            RenderImage::from_resident_frames(SmallVec::from_elem(frame, 1)),
            decode_mode,
        )
    };
    Ok((
        image,
        TargetImageDecodeMetadata {
            original_width,
            original_height,
            target: fitted_target,
            decode_mode,
        },
    ))
}

pub(super) fn decode_bmp_path_to_target(
    path: &std::path::Path,
    target: ImageDecodeTarget,
    object_fit: ObjectFit,
) -> Result<(RenderImage, TargetImageDecodeMetadata)> {
    let file = std::fs::File::open(path)?;
    let mut decoder = BmpDecoder::new(BufReader::new(file))?;
    let (original_width, original_height) = decoder.dimensions();
    let color_type = decoder.color_type();
    let original_size = size(original_width, original_height);
    let fitted_target = fitted_target_size(original_size, target, object_fit);
    let sample_target = high_quality_intermediate_target(original_size, fitted_target);
    let output = sample_bmp_rows_to_bgra(
        &mut decoder,
        original_width,
        original_height,
        color_type,
        sample_target,
    )?;
    let (image, decode_mode) = if sample_target == fitted_target {
        let frame = AnimatedFrame::from_bgra_bytes(0, sample_target.size(), output);
        (
            RenderImage::from_resident_frames(SmallVec::from_elem(frame, 1)),
            "bmp_rect_sample_decode",
        )
    } else {
        let rgba = bgra_bytes_to_rgba_image(output, sample_target)?;
        let (rgba, decode_mode) =
            resize_rgba_to_target(rgba, fitted_target, "bmp_rect_sample_decode")?;
        let frame = AnimatedFrame::from_rgba_image(0, rgba);
        (
            RenderImage::from_resident_frames(SmallVec::from_elem(frame, 1)),
            decode_mode,
        )
    };
    Ok((
        image,
        TargetImageDecodeMetadata {
            original_width,
            original_height,
            target: fitted_target,
            decode_mode,
        },
    ))
}

fn sample_bmp_rows_to_bgra<R: std::io::BufRead + std::io::Seek>(
    decoder: &mut BmpDecoder<R>,
    source_width: u32,
    source_height: u32,
    color_type: ColorType,
    sample_target: ImageDecodeTarget,
) -> Result<Vec<u8>> {
    let source_row_len = usize::from(color_type.bytes_per_pixel())
        .checked_mul(source_width as usize)
        .ok_or_else(|| anyhow::anyhow!("BMP source row size overflowed"))?;
    let mut source_row = vec![0; source_row_len];
    let output_len = bgra_len(sample_target)?;
    let mut output = vec![0; output_len];
    let mut next_target_y = 0u32;

    for source_y in 0..source_height {
        decoder.read_rect(
            0,
            source_y,
            source_width,
            1,
            &mut source_row,
            source_row_len,
        )?;

        while next_target_y < sample_target.height
            && source_axis_for_target(next_target_y, source_height, sample_target.height)
                == source_y
        {
            write_sampled_image_row(
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
        "BMP decoder ended before filling target image"
    );

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
        let source_x = source_axis_for_target(target_x, source_width, target_width) as usize;
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
    pixels: &[u8],
    color_type: ColorType,
    width: u32,
    height: u32,
) -> Result<Vec<u8>> {
    let pixel_count = width as usize * height as usize;
    let mut bgra = Vec::with_capacity(pixel_count * 4);
    match color_type {
        ColorType::L8 => {
            for &luma in pixels {
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
        ColorType::Rgba8 => {
            for pixel in pixels.chunks_exact(4) {
                bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
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
