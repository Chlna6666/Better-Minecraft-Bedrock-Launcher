use crate::{ObjectFit, Result, Size, size};
use image::{
    AnimationDecoder, ImageFormat, Rgba, RgbaImage,
    codecs::{gif::GifDecoder, png::PngDecoder, webp::WebPDecoder},
};
use smallvec::SmallVec;
use std::io::Cursor;
use std::sync::Arc;

use super::{
    decode_static_bmp_frame, decode_static_jpeg_frame, decode_static_png_frame,
    decode_static_webp_frame,
};
use crate::assets::render_image::{AnimatedFrame, AnimatedImageSource, RenderImage};
use crate::assets::types::{
    AnimatedImageConfig, ImageDecodeTarget, RenderImagePixelFormat, TargetImageDecodeMetadata,
};

pub(super) fn decode_image_bytes_to_target_via_full_decode(
    bytes: &[u8],
    format: ImageFormat,
    config: AnimatedImageConfig,
    target: ImageDecodeTarget,
    object_fit: ObjectFit,
) -> Result<(RenderImage, TargetImageDecodeMetadata)> {
    let source = AnimatedImageSource {
        bytes: Arc::from(bytes),
        format,
    };
    let decoded = decode_animation_prefix_to_target(&source, config.clamped(), target, object_fit)?;
    let DecodedTargetAnimation {
        first_frame,
        remaining_frames,
        is_complete,
        source_size,
        target: fitted_target,
    } = decoded;
    let is_animated = !remaining_frames.is_empty() || !is_complete;

    let image = if !is_complete {
        RenderImage::streaming_with_target(
            source,
            Some(fitted_target),
            first_frame,
            remaining_frames,
            config,
        )
    } else {
        let mut frames = SmallVec::<[AnimatedFrame; 1]>::new();
        frames.push(first_frame);
        frames.extend(remaining_frames);
        RenderImage::from_resident_frames(frames)
    };

    Ok((
        image,
        TargetImageDecodeMetadata {
            original_width: source_size.width,
            original_height: source_size.height,
            target: fitted_target,
            decode_mode: if source_size
                == fitted_target.size().map(|dimension| u32::from(dimension))
            {
                if is_animated {
                    "animated_original_decode"
                } else {
                    "original_decode"
                }
            } else if is_animated {
                "animated_frame_sample_decode"
            } else {
                "frame_sample_decode"
            },
        },
    ))
}

struct DecodedTargetAnimation {
    first_frame: AnimatedFrame,
    remaining_frames: SmallVec<[AnimatedFrame; 8]>,
    is_complete: bool,
    source_size: Size<u32>,
    target: ImageDecodeTarget,
}

fn decode_animation_prefix_to_target(
    source: &AnimatedImageSource,
    config: AnimatedImageConfig,
    target: ImageDecodeTarget,
    object_fit: ObjectFit,
) -> Result<DecodedTargetAnimation> {
    match source.format {
        ImageFormat::Gif => {
            let decoder = GifDecoder::new(Cursor::new(source.bytes.as_ref()))?;
            decode_frames_prefix_to_target(decoder.into_frames(), config, target, object_fit)
        }
        ImageFormat::Png => decode_png_prefix_to_target(source, config, target, object_fit),
        ImageFormat::WebP => decode_webp_prefix_to_target(source, config, target, object_fit),
        ImageFormat::Jpeg => decoded_static_frame_to_target(
            decode_static_jpeg_frame(source.bytes.as_ref())?,
            target,
            object_fit,
        ),
        ImageFormat::Bmp => decoded_static_frame_to_target(
            decode_static_bmp_frame(source.bytes.as_ref())?,
            target,
            object_fit,
        ),
        format => anyhow::bail!("unsupported GPUI image asset format: {format:?}"),
    }
}

fn decode_png_prefix_to_target(
    source: &AnimatedImageSource,
    config: AnimatedImageConfig,
    target: ImageDecodeTarget,
    object_fit: ObjectFit,
) -> Result<DecodedTargetAnimation> {
    let decoder = PngDecoder::new(Cursor::new(source.bytes.as_ref()))?;
    if decoder.is_apng()? {
        return decode_frames_prefix_to_target(
            decoder.apng()?.into_frames(),
            config,
            target,
            object_fit,
        );
    }

    decoded_static_frame_to_target(
        decode_static_png_frame(source.bytes.as_ref())?,
        target,
        object_fit,
    )
}

fn decode_webp_prefix_to_target(
    source: &AnimatedImageSource,
    config: AnimatedImageConfig,
    target: ImageDecodeTarget,
    object_fit: ObjectFit,
) -> Result<DecodedTargetAnimation> {
    let mut decoder = WebPDecoder::new(Cursor::new(source.bytes.as_ref()))?;
    if decoder.has_animation() {
        let _ = decoder.set_background_color(Rgba([0, 0, 0, 0]));
        return decode_frames_prefix_to_target(decoder.into_frames(), config, target, object_fit);
    }

    decoded_static_frame_to_target(
        decode_static_webp_frame(source.bytes.as_ref())?,
        target,
        object_fit,
    )
}

fn decoded_static_frame_to_target(
    frame: AnimatedFrame,
    target: ImageDecodeTarget,
    object_fit: ObjectFit,
) -> Result<DecodedTargetAnimation> {
    let source_size = frame.size().map(|dimension| u32::from(dimension));
    let fitted_target = fitted_target_size(source_size, target, object_fit);
    let first_frame = resample_bgra_frame_to_target(frame, fitted_target)?;
    Ok(DecodedTargetAnimation {
        first_frame,
        remaining_frames: SmallVec::new(),
        is_complete: true,
        source_size,
        target: fitted_target,
    })
}

fn decode_frames_prefix_to_target(
    frames: image::Frames<'_>,
    config: AnimatedImageConfig,
    target: ImageDecodeTarget,
    object_fit: ObjectFit,
) -> Result<DecodedTargetAnimation> {
    let config = config.clamped();
    let mut frames = frames.enumerate();
    let Some((_, first_frame)) = frames.next() else {
        return Err(anyhow::anyhow!("animated image did not contain any frames"));
    };
    let first_frame = AnimatedFrame::from_rgba_frame(0, first_frame?);
    let source_size = first_frame.size().map(|dimension| u32::from(dimension));
    let fitted_target = fitted_target_size(source_size, target, object_fit);
    let first_frame = resample_bgra_frame_to_target(first_frame, fitted_target)?;
    let mut decoded_byte_len = first_frame.byte_len();
    let mut remaining_frames = SmallVec::<[AnimatedFrame; 8]>::new();

    for (sequence, frame) in frames {
        if remaining_frames.len() + 1 >= config.max_resident_frames
            || decoded_byte_len >= config.max_resident_bytes
        {
            return Ok(DecodedTargetAnimation {
                first_frame,
                remaining_frames,
                is_complete: false,
                source_size,
                target: fitted_target,
            });
        }

        let frame = AnimatedFrame::from_rgba_frame(sequence, frame?);
        let frame = resample_bgra_frame_to_target(frame, fitted_target)?;
        let next_decoded_byte_len = decoded_byte_len.saturating_add(frame.byte_len());
        if next_decoded_byte_len > config.max_resident_bytes {
            return Ok(DecodedTargetAnimation {
                first_frame,
                remaining_frames,
                is_complete: false,
                source_size,
                target: fitted_target,
            });
        }

        decoded_byte_len = next_decoded_byte_len;
        remaining_frames.push(frame);
    }

    Ok(DecodedTargetAnimation {
        first_frame,
        remaining_frames,
        is_complete: true,
        source_size,
        target: fitted_target,
    })
}

pub(crate) fn resample_bgra_frame_to_target(
    frame: AnimatedFrame,
    target: ImageDecodeTarget,
) -> Result<AnimatedFrame> {
    let source_size = frame.size().map(|dimension| u32::from(dimension));
    if source_size == target.size().map(|dimension| u32::from(dimension)) {
        return Ok(frame);
    }

    anyhow::ensure!(
        source_size.width > 0 && source_size.height > 0,
        "decoded image frame has invalid dimensions"
    );
    let source_len = bgra_len(ImageDecodeTarget {
        width: source_size.width,
        height: source_size.height,
    })?;
    anyhow::ensure!(
        frame.bytes.len() >= source_len,
        "decoded image frame buffer was shorter than its dimensions"
    );

    let output_len = bgra_len(target)?;
    let mut output = vec![0; output_len];
    let source = frame.bytes.as_ref();
    for target_y in 0..target.height {
        let source_y = source_axis_for_target(target_y, source_size.height, target.height);
        for target_x in 0..target.width {
            let source_x = source_axis_for_target(target_x, source_size.width, target.width);
            let source_offset =
                (source_y as usize * source_size.width as usize + source_x as usize) * 4;
            let target_offset = (target_y as usize * target.width as usize + target_x as usize) * 4;
            output[target_offset..target_offset + 4]
                .copy_from_slice(&source[source_offset..source_offset + 4]);
        }
    }

    Ok(AnimatedFrame {
        sequence: frame.sequence,
        size: target.size(),
        delay: frame.delay,
        bytes: Arc::from(output),
        pixel_format: RenderImagePixelFormat::Bgra8,
    })
}

pub(super) fn resize_rgba_to_target(
    rgba: RgbaImage,
    target: ImageDecodeTarget,
    decode_mode: &'static str,
) -> Result<(RgbaImage, &'static str)> {
    let current = size(rgba.width(), rgba.height());
    if current == target.size().map(|dimension| u32::from(dimension)) {
        return Ok((rgba, decode_mode));
    }

    let resized = image::imageops::resize(
        &rgba,
        target.width,
        target.height,
        image::imageops::FilterType::Lanczos3,
    );
    Ok((resized, "decoder_scaled_then_resized"))
}

pub(super) fn bgra_len(target: ImageDecodeTarget) -> Result<usize> {
    target
        .width
        .try_into()
        .ok()
        .and_then(|width: usize| {
            target
                .height
                .try_into()
                .ok()
                .and_then(|height: usize| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| anyhow::anyhow!("target image buffer size overflowed"))
}

pub(crate) fn fitted_target_size(
    source_size: Size<u32>,
    target: ImageDecodeTarget,
    object_fit: ObjectFit,
) -> ImageDecodeTarget {
    let source_width = source_size.width.max(1) as f32;
    let source_height = source_size.height.max(1) as f32;
    let target_width = target.width.max(1) as f32;
    let target_height = target.height.max(1) as f32;
    let scale = match object_fit {
        ObjectFit::Fill => {
            return ImageDecodeTarget {
                width: target.width.max(1),
                height: target.height.max(1),
            };
        }
        ObjectFit::Cover => (target_width / source_width).max(target_height / source_height),
        ObjectFit::Contain => (target_width / source_width).min(target_height / source_height),
        ObjectFit::ScaleDown => (target_width / source_width)
            .min(target_height / source_height)
            .min(1.0),
        ObjectFit::None => 1.0,
    };

    let width = ((source_width * scale).ceil() as u32).max(1);
    let height = ((source_height * scale).ceil() as u32).max(1);
    ImageDecodeTarget { width, height }
}

pub(super) fn source_axis_for_target(target_axis: u32, source_len: u32, target_len: u32) -> u32 {
    ((u64::from(target_axis) * u64::from(source_len)) / u64::from(target_len))
        .min(u64::from(source_len.saturating_sub(1))) as u32
}

pub(super) fn high_quality_intermediate_target(
    source_size: Size<u32>,
    fitted_target: ImageDecodeTarget,
) -> ImageDecodeTarget {
    let oversample_limit = 2u32;
    let width = fitted_target
        .width
        .saturating_mul(oversample_limit)
        .min(source_size.width.max(1));
    let height = fitted_target
        .height
        .saturating_mul(oversample_limit)
        .min(source_size.height.max(1));
    ImageDecodeTarget {
        width: width.max(fitted_target.width).max(1),
        height: height.max(fitted_target.height).max(1),
    }
}

pub(super) fn bgra_bytes_to_rgba_image(
    bytes: Vec<u8>,
    size: ImageDecodeTarget,
) -> Result<RgbaImage> {
    let mut rgba = bytes;
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    RgbaImage::from_raw(size.width, size.height, rgba)
        .ok_or_else(|| anyhow::anyhow!("decoded image buffer dimensions were invalid"))
}
