use crate::{ObjectFit, Result, size};
use image::RgbaImage;
use smallvec::SmallVec;
use std::io::Cursor;

use super::resample::resize_rgba_frame;
use crate::assets::{AnimatedFrame, RenderImage};
use crate::assets::{ImageRenderInfo, ImageRenderSize};

pub(super) fn frame(bytes: &[u8]) -> Result<AnimatedFrame> {
    let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(bytes));
    let pixels = decoder.decode()?;
    let info = decoder
        .info()
        .ok_or_else(|| anyhow::anyhow!("JPEG decoder did not report image dimensions"))?;
    let rgba = jpeg_pixels_to_rgba_image(&pixels, info)?;
    Ok(AnimatedFrame::from_rgba_image(0, rgba))
}

pub(super) fn render_sized(
    bytes: &[u8],
    target: ImageRenderSize,
    object_fit: ObjectFit,
) -> Result<(RenderImage, ImageRenderInfo)> {
    let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(bytes));
    decoder.read_info()?;
    let original_info = decoder
        .info()
        .ok_or_else(|| anyhow::anyhow!("JPEG decoder did not report image dimensions"))?;
    let original_size = size(
        u32::from(original_info.width),
        u32::from(original_info.height),
    );
    let fitted_target = target.fit(original_size, object_fit);

    let requested_width = u16::try_from(fitted_target.width.min(u32::from(u16::MAX)))?;
    let requested_height = u16::try_from(fitted_target.height.min(u32::from(u16::MAX)))?;
    decoder.scale(requested_width.max(1), requested_height.max(1))?;
    let pixels = decoder.decode()?;
    let scaled_info = decoder
        .info()
        .ok_or_else(|| anyhow::anyhow!("JPEG decoder did not report scaled dimensions"))?;
    let rgba = jpeg_pixels_to_rgba_image(&pixels, scaled_info)?;
    let (rgba, render_path) = resize_rgba_frame(rgba, fitted_target, "jpeg_scaled")?;
    let frame = AnimatedFrame::from_rgba_image(0, rgba);
    let image = RenderImage::from_resident_frames(SmallVec::from_elem(frame, 1));

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

fn jpeg_pixels_to_rgba_image(pixels: &[u8], info: jpeg_decoder::ImageInfo) -> Result<RgbaImage> {
    let width = u32::from(info.width);
    let height = u32::from(info.height);
    let pixel_count = width as usize * height as usize;
    let mut rgba = Vec::with_capacity(pixel_count * 4);

    match info.pixel_format {
        jpeg_decoder::PixelFormat::L8 => {
            for &luma in pixels {
                rgba.extend_from_slice(&[luma, luma, luma, 255]);
            }
        }
        jpeg_decoder::PixelFormat::L16 => {
            for luma in pixels.chunks_exact(2) {
                let luma = luma[0];
                rgba.extend_from_slice(&[luma, luma, luma, 255]);
            }
        }
        jpeg_decoder::PixelFormat::RGB24 => {
            for pixel in pixels.chunks_exact(3) {
                rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
            }
        }
        jpeg_decoder::PixelFormat::CMYK32 => {
            for pixel in pixels.chunks_exact(4) {
                let c = u16::from(pixel[0]);
                let m = u16::from(pixel[1]);
                let y = u16::from(pixel[2]);
                let k = u16::from(pixel[3]);
                let convert = |channel: u16| {
                    255u8.saturating_sub(((channel * (255 - k)) / 255 + k).min(255) as u8)
                };
                rgba.extend_from_slice(&[convert(c), convert(m), convert(y), 255]);
            }
        }
    }

    RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| anyhow::anyhow!("JPEG decoded buffer dimensions were invalid"))
}
