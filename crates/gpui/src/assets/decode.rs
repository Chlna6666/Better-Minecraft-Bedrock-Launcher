mod animation;
mod bmp;
mod jpeg;
mod png;
mod target;
mod webp;

pub(crate) use target::fitted_target_size;
pub(super) use target::resample_bgra_frame_to_target;

use animation::decode_animation_prefix;
use bmp::{decode_bmp_path_to_target, decode_bmp_to_target, decode_static_bmp_frame};
use jpeg::{decode_jpeg_path_to_target, decode_jpeg_to_target, decode_static_jpeg_frame};
use png::{decode_png_path_to_target, decode_png_to_target, decode_static_png_frame};
use target::decode_image_bytes_to_target_via_full_decode;
use webp::{decode_static_webp_frame, decode_webp_to_target};

use crate::{BackgroundExecutor, ObjectFit, Result};
use image::ImageFormat;
use smallvec::SmallVec;
use std::sync::Arc;

use super::render_image::{
    AnimatedFrame, AnimatedImageSource, DecodedAnimation, RenderImage, RenderImageData,
};
use super::types::{AnimatedImageConfig, ImageDecodeTarget, TargetImageDecodeMetadata};

pub(crate) fn decode_image_bytes(
    bytes: &[u8],
    format: ImageFormat,
    config: AnimatedImageConfig,
    executor: Option<BackgroundExecutor>,
) -> Result<RenderImage> {
    let source = AnimatedImageSource {
        bytes: Arc::from(bytes),
        format,
    };
    decode_image_source(source, config, executor)
}

/// Decode image bytes directly to a target device-pixel size where the format supports it.
///
/// Static WebP, JPEG, PNG, and BMP avoid retaining an original-size frame for the
/// returned image. Animated formats may still decode source frames internally, but
/// the returned resident image is resampled to the requested target size.
pub fn decode_image_bytes_to_target(
    bytes: &[u8],
    format: ImageFormat,
    config: AnimatedImageConfig,
    target: ImageDecodeTarget,
    object_fit: ObjectFit,
) -> Result<(RenderImage, TargetImageDecodeMetadata)> {
    match format {
        ImageFormat::Jpeg => decode_jpeg_to_target(bytes, target, object_fit),
        ImageFormat::Png => decode_png_to_target(bytes, config, target, object_fit),
        ImageFormat::WebP => decode_webp_to_target(bytes, target, object_fit).or_else(|_| {
            decode_image_bytes_to_target_via_full_decode(bytes, format, config, target, object_fit)
        }),
        ImageFormat::Bmp => decode_bmp_to_target(bytes, target, object_fit),
        _ => {
            decode_image_bytes_to_target_via_full_decode(bytes, format, config, target, object_fit)
        }
    }
}

/// Decode an image file directly to a target device-pixel size where the format supports streaming.
pub fn decode_image_path_to_target(
    path: &std::path::Path,
    config: AnimatedImageConfig,
    target: ImageDecodeTarget,
    object_fit: ObjectFit,
) -> Result<(RenderImage, TargetImageDecodeMetadata)> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "jpg" | "jpeg" => decode_jpeg_path_to_target(path, target, object_fit),
        "png" => decode_png_path_to_target(path, config, target, object_fit),
        "bmp" => decode_bmp_path_to_target(path, target, object_fit),
        _ => {
            let bytes = std::fs::read(path)?;
            let format = image::guess_format(&bytes)?;
            decode_image_bytes_to_target(&bytes, format, config, target, object_fit)
        }
    }
}

pub(crate) fn decode_image_source(
    source: AnimatedImageSource,
    config: AnimatedImageConfig,
    executor: Option<BackgroundExecutor>,
) -> Result<RenderImage> {
    let config = config.clamped();
    let DecodedAnimation {
        first_frame,
        remaining_frames,
        is_complete,
    } = decode_animation_prefix(
        &source,
        config.max_resident_frames,
        config.max_resident_bytes,
    )?;

    let image = if !is_complete && let Some(executor) = executor {
        let image = RenderImage::streaming(source, first_frame, remaining_frames, config);
        if let RenderImageData::Streaming(state) = &image.data {
            state.ensure_decode_task(&executor);
        }
        image
    } else {
        let mut frames = SmallVec::<[AnimatedFrame; 1]>::new();
        frames.push(first_frame);
        frames.extend(remaining_frames);
        RenderImage::from_resident_frames(frames)
    };

    Ok(image)
}
