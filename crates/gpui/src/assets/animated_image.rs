use super::{BitmapBytes, EncodedImage, ImagePixelFormat, bmp, jpeg, png, webp};
use crate::{DevicePixels, Result, Size, size};
use image::{
    AnimationDecoder, Delay, Frame, ImageFormat, Rgba, RgbaImage,
    codecs::{gif::GifDecoder, png::PngDecoder, webp::WebPDecoder},
};
use smallvec::SmallVec;
use std::{io::Cursor, sync::Arc, time::Duration};

const DEFAULT_ANIMATED_IMAGE_MAX_FPS: f32 = 90.0;
const DEFAULT_INACTIVE_ANIMATED_IMAGE_MAX_FPS: f32 = 4.0;
const MAX_CONFIGURABLE_ANIMATED_IMAGE_FPS: f32 = 1_000.0;
const MAX_CONFIGURABLE_INACTIVE_IMAGE_FPS: f32 = 240.0;

/// Runtime controls for animated images and GPU frame residency.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimatedImageConfig {
    /// Whether images with more than one frame should play.
    pub play: bool,
    /// Maximum frames per image resident in GPU atlas slots.
    pub max_gpu_frame_slots: usize,
    /// Maximum active playback rate.
    pub max_fps: f32,
    /// Maximum playback rate while the window is inactive.
    pub inactive_max_fps: f32,
    /// Number of frames queued ahead of playback.
    pub prefetch_frames: usize,
    /// Maximum frames retained fully resident before switching to streaming decode.
    pub max_resident_frames: usize,
}

impl Default for AnimatedImageConfig {
    fn default() -> Self {
        Self {
            play: true,
            max_gpu_frame_slots: 3,
            max_fps: DEFAULT_ANIMATED_IMAGE_MAX_FPS,
            inactive_max_fps: DEFAULT_INACTIVE_ANIMATED_IMAGE_MAX_FPS,
            prefetch_frames: 12,
            max_resident_frames: 512,
        }
    }
}

impl AnimatedImageConfig {
    pub(crate) fn clamped(self) -> Self {
        Self {
            play: self.play,
            max_gpu_frame_slots: self.max_gpu_frame_slots.max(1),
            max_fps: clamp_frame_rate(
                self.max_fps,
                1.0,
                MAX_CONFIGURABLE_ANIMATED_IMAGE_FPS,
                DEFAULT_ANIMATED_IMAGE_MAX_FPS,
            ),
            inactive_max_fps: clamp_frame_rate(
                self.inactive_max_fps,
                0.25,
                MAX_CONFIGURABLE_INACTIVE_IMAGE_FPS,
                DEFAULT_INACTIVE_ANIMATED_IMAGE_MAX_FPS,
            ),
            prefetch_frames: self.prefetch_frames.clamp(2, 64),
            max_resident_frames: self.max_resident_frames.max(1),
        }
    }

    pub(crate) fn minimum_frame_duration(self) -> Duration {
        Duration::from_secs_f32(1.0 / self.clamped().max_fps)
    }

    pub(crate) fn inactive_minimum_frame_duration(self) -> Duration {
        Duration::from_secs_f32(1.0 / self.clamped().inactive_max_fps)
    }
}

fn clamp_frame_rate(value: f32, minimum: f32, maximum: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(minimum, maximum)
    } else {
        fallback
    }
}

#[derive(Clone)]
pub(crate) struct AnimatedFrame {
    pub(in crate::assets) sequence: usize,
    pub(in crate::assets) size: Size<DevicePixels>,
    pub(in crate::assets) delay: Delay,
    pub(in crate::assets) bytes: Arc<BitmapBytes>,
    pub(in crate::assets) pixel_format: ImagePixelFormat,
}

impl AnimatedFrame {
    pub(crate) fn from_bgra_frame(sequence: usize, frame: Frame) -> Self {
        let delay = frame.delay();
        let data = frame.into_buffer();
        let (width, height) = data.dimensions();
        Self {
            sequence,
            size: size(width.into(), height.into()),
            delay,
            bytes: BitmapBytes::from_vec(data.into_raw()),
            pixel_format: ImagePixelFormat::Bgra8,
        }
    }

    pub(crate) fn from_rgba_image(sequence: usize, mut image: RgbaImage) -> Self {
        for pixel in image.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        Self::from_bgra_frame(sequence, Frame::new(image))
    }

    pub(crate) fn from_bgra_image(sequence: usize, image: RgbaImage) -> Self {
        Self::from_bgra_frame(sequence, Frame::new(image))
    }

    pub(crate) fn from_bgra_bytes(
        sequence: usize,
        size: Size<DevicePixels>,
        bytes: Vec<u8>,
    ) -> Self {
        Self {
            sequence,
            size,
            delay: Delay::from_saturating_duration(Duration::ZERO),
            bytes: BitmapBytes::from_vec(bytes),
            pixel_format: ImagePixelFormat::Bgra8,
        }
    }

    pub(crate) fn from_raw_pixel_bytes(
        sequence: usize,
        size: Size<DevicePixels>,
        pixel_format: ImagePixelFormat,
        bytes: impl Into<Arc<[u8]>>,
    ) -> Self {
        Self {
            sequence,
            size,
            delay: Delay::from_saturating_duration(Duration::ZERO),
            bytes: BitmapBytes::from_shared(bytes.into()),
            pixel_format,
        }
    }

    pub(crate) fn from_rgba_frame(sequence: usize, frame: Frame) -> Self {
        let delay = frame.delay();
        let mut data = frame.into_buffer();
        for pixel in data.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        let (width, height) = data.dimensions();
        Self {
            sequence,
            size: size(width.into(), height.into()),
            delay,
            bytes: BitmapBytes::from_vec(data.into_raw()),
            pixel_format: ImagePixelFormat::Bgra8,
        }
    }

    pub(crate) fn from_rgba_frame_without_conversion(sequence: usize, frame: Frame) -> Self {
        let delay = frame.delay();
        let data = frame.into_buffer();
        let (width, height) = data.dimensions();
        Self {
            sequence,
            size: size(width.into(), height.into()),
            delay,
            bytes: BitmapBytes::from_vec(data.into_raw()),
            pixel_format: ImagePixelFormat::Rgba8,
        }
    }

    pub(crate) fn sequence(&self) -> usize {
        self.sequence
    }
    pub(crate) fn size(&self) -> Size<DevicePixels> {
        self.size
    }
    pub(crate) fn delay(&self) -> Delay {
        self.delay
    }
    pub(crate) fn bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }
    pub(crate) fn pixel_format(&self) -> ImagePixelFormat {
        self.pixel_format
    }
    pub(in crate::assets) fn byte_len(&self) -> usize {
        self.bytes.as_slice().len()
    }
}

pub(crate) struct AnimatedImageFrames {
    pub(crate) first_frame: AnimatedFrame,
    pub(crate) remaining_frames: SmallVec<[AnimatedFrame; 8]>,
    pub(crate) is_complete: bool,
}

pub(super) fn initial_frames(
    source: &EncodedImage,
    max_resident_frames: usize,
) -> Result<AnimatedImageFrames> {
    match source.format {
        ImageFormat::Gif => {
            let decoder = GifDecoder::new(Cursor::new(source.bytes.as_ref()))?;
            resident_frames(decoder.into_frames(), max_resident_frames)
        }
        ImageFormat::Png => png_initial_frames(source, max_resident_frames),
        ImageFormat::WebP => webp_initial_frames(source, max_resident_frames),
        ImageFormat::Jpeg => Ok(from_single_frame(jpeg::frame(source.bytes.as_ref())?)),
        ImageFormat::Bmp => Ok(from_single_frame(bmp::frame(source.bytes.as_ref())?)),
        format => anyhow::bail!("unsupported GPUI image asset format: {format:?}"),
    }
}

fn png_initial_frames(
    source: &EncodedImage,
    max_resident_frames: usize,
) -> Result<AnimatedImageFrames> {
    let decoder = PngDecoder::new(Cursor::new(source.bytes.as_ref()))?;
    if decoder.is_apng()? {
        let decoded = resident_frames(decoder.apng()?.into_frames(), max_resident_frames)?;
        if decoded.first_frame.byte_len() > 0 {
            return Ok(decoded);
        }
    }

    Ok(from_single_frame(png::frame(source.bytes.as_ref())?))
}

fn webp_initial_frames(
    source: &EncodedImage,
    max_resident_frames: usize,
) -> Result<AnimatedImageFrames> {
    let mut decoder = WebPDecoder::new(Cursor::new(source.bytes.as_ref()))?;
    if decoder.has_animation() {
        let _ = decoder.set_background_color(Rgba([0, 0, 0, 0]));
        return resident_frames(decoder.into_frames(), max_resident_frames);
    }

    Ok(from_single_frame(webp::frame(source.bytes.as_ref())?))
}

fn from_single_frame(first_frame: AnimatedFrame) -> AnimatedImageFrames {
    AnimatedImageFrames {
        first_frame,
        remaining_frames: SmallVec::new(),
        is_complete: true,
    }
}

fn resident_frames(
    frames: image::Frames<'_>,
    max_resident_frames: usize,
) -> Result<AnimatedImageFrames> {
    let mut frames = frames.enumerate();
    let Some((_, first_frame)) = frames.next() else {
        return Err(anyhow::anyhow!("animated image did not contain any frames"));
    };
    let first_frame = AnimatedFrame::from_rgba_frame(0, first_frame?);
    let mut remaining_frames = SmallVec::<[AnimatedFrame; 8]>::new();

    for (sequence, frame) in frames {
        if remaining_frames.len() + 1 >= max_resident_frames {
            return Ok(AnimatedImageFrames {
                first_frame,
                remaining_frames,
                is_complete: false,
            });
        }
        remaining_frames.push(AnimatedFrame::from_rgba_frame(sequence, frame?));
    }

    Ok(AnimatedImageFrames {
        first_frame,
        remaining_frames,
        is_complete: true,
    })
}
