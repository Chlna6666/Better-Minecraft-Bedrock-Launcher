use crate::assets::{
    AnimatedFrame, AnimatedImageConfig, BitmapBytes, EncodedImage, ImagePixelFormat, RenderImage,
    bmp, jpeg, png, webp,
};
use crate::{DevicePixels, ObjectFit, Result, Size, acquire_bitmap_buffer, size};
use image::{
    AnimationDecoder, ImageFormat, Rgba, RgbaImage,
    codecs::{gif::GifDecoder, png::PngDecoder, webp::WebPDecoder},
};
use smallvec::SmallVec;
use std::io::Cursor;

/// Requested output size for an image in device pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ImageRenderSize {
    /// Requested width in device pixels.
    pub width: u32,
    /// Requested height in device pixels.
    pub height: u32,
}

impl ImageRenderSize {
    /// Creates a render size when both dimensions are non-zero.
    pub fn new(width: u32, height: u32) -> Option<Self> {
        (width > 0 && height > 0).then_some(Self { width, height })
    }

    pub(crate) fn size(self) -> Size<DevicePixels> {
        size(self.width.into(), self.height.into())
    }

    pub(crate) fn fit(self, source_size: Size<u32>, object_fit: ObjectFit) -> Self {
        let source_width = source_size.width.max(1) as f32;
        let source_height = source_size.height.max(1) as f32;
        let target_width = self.width.max(1) as f32;
        let target_height = self.height.max(1) as f32;
        let scale = match object_fit {
            ObjectFit::Fill => {
                return Self {
                    width: self.width.max(1),
                    height: self.height.max(1),
                };
            }
            ObjectFit::Cover => (target_width / source_width).max(target_height / source_height),
            ObjectFit::Contain => (target_width / source_width).min(target_height / source_height),
            ObjectFit::ScaleDown => (target_width / source_width)
                .min(target_height / source_height)
                .min(1.0),
            ObjectFit::None => 1.0,
        };

        Self {
            width: ((source_width * scale).ceil() as u32).max(1),
            height: ((source_height * scale).ceil() as u32).max(1),
        }
    }
}

/// Information about producing a sized render image.
#[derive(Clone, Debug)]
pub struct ImageRenderInfo {
    /// Original source width, when available.
    pub original_width: u32,
    /// Original source height, when available.
    pub original_height: u32,
    /// Requested output size.
    pub size: ImageRenderSize,
    /// Processing path used to produce the image.
    pub render_path: &'static str,
}

pub(super) fn render_sized(
    source: EncodedImage,
    config: AnimatedImageConfig,
    size: ImageRenderSize,
    object_fit: ObjectFit,
) -> Result<(RenderImage, ImageRenderInfo)> {
    let frames = initial_frames(&source, config.clamped(), size, object_fit)?;
    let ImageFrames {
        first_frame,
        remaining_frames,
        is_complete,
        source_size,
        size: fitted_target,
    } = frames;
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
        ImageRenderInfo {
            original_width: source_size.width,
            original_height: source_size.height,
            size: fitted_target,
            render_path: if source_size
                == fitted_target.size().map(|dimension| u32::from(dimension))
            {
                if is_animated {
                    "animated_original"
                } else {
                    "original"
                }
            } else if is_animated {
                "animated_frame_sample"
            } else {
                "frame_sample"
            },
        },
    ))
}

struct ImageFrames {
    first_frame: AnimatedFrame,
    remaining_frames: SmallVec<[AnimatedFrame; 8]>,
    is_complete: bool,
    source_size: Size<u32>,
    size: ImageRenderSize,
}

fn initial_frames(
    source: &EncodedImage,
    config: AnimatedImageConfig,
    size: ImageRenderSize,
    object_fit: ObjectFit,
) -> Result<ImageFrames> {
    match source.format {
        ImageFormat::Gif => {
            let decoder = GifDecoder::new(Cursor::new(source.bytes.as_ref()))?;
            initial_frames_from_iter(decoder.into_frames(), config, size, object_fit)
        }
        ImageFormat::Png => png_initial_frames(source, config, size, object_fit),
        ImageFormat::WebP => webp_initial_frames(source, config, size, object_fit),
        ImageFormat::Jpeg => static_frame(jpeg::frame(source.bytes.as_ref())?, size, object_fit),
        ImageFormat::Bmp => static_frame(bmp::frame(source.bytes.as_ref())?, size, object_fit),
        format => anyhow::bail!("unsupported GPUI image asset format: {format:?}"),
    }
}

fn png_initial_frames(
    source: &EncodedImage,
    config: AnimatedImageConfig,
    target: ImageRenderSize,
    object_fit: ObjectFit,
) -> Result<ImageFrames> {
    let decoder = PngDecoder::new(Cursor::new(source.bytes.as_ref()))?;
    if decoder.is_apng()? {
        return initial_frames_from_iter(decoder.apng()?.into_frames(), config, target, object_fit);
    }

    static_frame(png::frame(source.bytes.as_ref())?, target, object_fit)
}

fn webp_initial_frames(
    source: &EncodedImage,
    config: AnimatedImageConfig,
    target: ImageRenderSize,
    object_fit: ObjectFit,
) -> Result<ImageFrames> {
    let mut decoder = WebPDecoder::new(Cursor::new(source.bytes.as_ref()))?;
    if decoder.has_animation() {
        let _ = decoder.set_background_color(Rgba([0, 0, 0, 0]));
        return initial_frames_from_iter(decoder.into_frames(), config, target, object_fit);
    }

    static_frame(webp::frame(source.bytes.as_ref())?, target, object_fit)
}

fn static_frame(
    frame: AnimatedFrame,
    size: ImageRenderSize,
    object_fit: ObjectFit,
) -> Result<ImageFrames> {
    let source_size = frame.size().map(|dimension| u32::from(dimension));
    let fitted_size = size.fit(source_size, object_fit);
    let first_frame = resample_bgra_frame(frame, fitted_size)?;
    Ok(ImageFrames {
        first_frame,
        remaining_frames: SmallVec::new(),
        is_complete: true,
        source_size,
        size: fitted_size,
    })
}

fn initial_frames_from_iter(
    frames: image::Frames<'_>,
    config: AnimatedImageConfig,
    target: ImageRenderSize,
    object_fit: ObjectFit,
) -> Result<ImageFrames> {
    let config = config.clamped();
    let mut frames = frames.enumerate();
    let Some((_, first_frame)) = frames.next() else {
        return Err(anyhow::anyhow!("animated image did not contain any frames"));
    };
    let first_frame = AnimatedFrame::from_rgba_frame(0, first_frame?);
    let source_size = first_frame.size().map(|dimension| u32::from(dimension));
    let fitted_target = target.fit(source_size, object_fit);
    let first_frame = resample_bgra_frame(first_frame, fitted_target)?;
    let mut remaining_frames = SmallVec::<[AnimatedFrame; 8]>::new();

    for (sequence, frame) in frames {
        if remaining_frames.len() + 1 >= config.max_resident_frames {
            return Ok(ImageFrames {
                first_frame,
                remaining_frames,
                is_complete: false,
                source_size,
                size: fitted_target,
            });
        }

        let frame = AnimatedFrame::from_rgba_frame(sequence, frame?);
        remaining_frames.push(resample_bgra_frame(frame, fitted_target)?);
    }

    Ok(ImageFrames {
        first_frame,
        remaining_frames,
        is_complete: true,
        source_size,
        size: fitted_target,
    })
}

pub(crate) fn resample_bgra_frame(
    frame: AnimatedFrame,
    target: ImageRenderSize,
) -> Result<AnimatedFrame> {
    let source_size = frame.size().map(|dimension| u32::from(dimension));
    if source_size == target.size().map(|dimension| u32::from(dimension)) {
        return Ok(frame);
    }

    anyhow::ensure!(
        source_size.width > 0 && source_size.height > 0,
        "decoded image frame has invalid dimensions"
    );
    let source_len = bgra_byte_len(ImageRenderSize {
        width: source_size.width,
        height: source_size.height,
    })?;
    anyhow::ensure!(
        frame.bytes.len() >= source_len,
        "decoded image frame buffer was shorter than its dimensions"
    );

    let output_len = bgra_byte_len(target)?;
    let mut output = acquire_bitmap_buffer(output_len);
    let source = frame.bytes();
    for target_y in 0..target.height {
        let source_y = scaled_axis(target_y, source_size.height, target.height);
        for target_x in 0..target.width {
            let source_x = scaled_axis(target_x, source_size.width, target.width);
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
        bytes: BitmapBytes::from_vec(output),
        pixel_format: ImagePixelFormat::Bgra8,
    })
}

pub(super) fn resize_rgba_frame(
    rgba: RgbaImage,
    target: ImageRenderSize,
    render_path: &'static str,
) -> Result<(RgbaImage, &'static str)> {
    let current = size(rgba.width(), rgba.height());
    if current == target.size().map(|dimension| u32::from(dimension)) {
        return Ok((rgba, render_path));
    }

    let resized = image::imageops::resize(
        &rgba,
        target.width,
        target.height,
        image::imageops::FilterType::Lanczos3,
    );
    Ok((resized, "scaled_then_resized"))
}

pub(super) fn bgra_byte_len(target: ImageRenderSize) -> Result<usize> {
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

pub(super) fn scaled_axis(target_axis: u32, source_len: u32, target_len: u32) -> u32 {
    ((u64::from(target_axis) * u64::from(source_len)) / u64::from(target_len))
        .min(u64::from(source_len.saturating_sub(1))) as u32
}

pub(super) fn intermediate_sample_size(
    source_size: Size<u32>,
    fitted_target: ImageRenderSize,
) -> ImageRenderSize {
    let oversample_limit = 2u32;
    let width = fitted_target
        .width
        .saturating_mul(oversample_limit)
        .min(source_size.width.max(1));
    let height = fitted_target
        .height
        .saturating_mul(oversample_limit)
        .min(source_size.height.max(1));
    ImageRenderSize {
        width: width.max(fitted_target.width).max(1),
        height: height.max(fitted_target.height).max(1),
    }
}

pub(super) fn rgba_image_from_bgra(bytes: Vec<u8>, size: ImageRenderSize) -> Result<RgbaImage> {
    let mut rgba = bytes;
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    RgbaImage::from_raw(size.width, size.height, rgba)
        .ok_or_else(|| anyhow::anyhow!("decoded image buffer dimensions were invalid"))
}
