use crate::{DevicePixels, SharedString, Size, size};
use std::time::Duration;

/// A unique identifier for the image cache
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ImageId(pub usize);

/// Pixel format used by decoded image frames uploaded to the renderer.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum RenderImagePixelFormat {
    /// Blue, green, red, alpha byte order.
    Bgra8,
    /// Red, green, blue, alpha byte order.
    Rgba8,
}

impl RenderImagePixelFormat {
    pub(crate) const fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Bgra8 | Self::Rgba8 => 4,
        }
    }
}

#[derive(PartialEq, Eq, Hash, Clone)]
pub(crate) struct RenderImageParams {
    pub(crate) image_id: ImageId,
    pub(crate) frame_slot: usize,
    pub(crate) pixel_format: RenderImagePixelFormat,
}

/// Runtime controls for animated image decoding and GPU frame residency.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimatedImageConfig {
    /// Whether images with more than one decoded frame should play.
    pub play: bool,
    /// Maximum frames per image that should be resident in GPU atlas slots.
    pub max_gpu_frame_slots: usize,
    /// Maximum playback rate for animated images.
    pub max_fps: f32,
    /// Maximum playback rate for animated images while their window is inactive.
    pub inactive_max_fps: f32,
    /// Number of decoded frames to keep queued ahead of playback.
    pub decode_ahead_frames: usize,
    /// Maximum frames to retain fully resident for small animated images.
    pub max_resident_frames: usize,
    /// Maximum decoded bytes to retain fully resident for small animated images.
    pub max_resident_bytes: usize,
}

impl Default for AnimatedImageConfig {
    fn default() -> Self {
        Self {
            play: true,
            max_gpu_frame_slots: 3,
            max_fps: 60.0,
            inactive_max_fps: 4.0,
            decode_ahead_frames: 12,
            max_resident_frames: 512,
            max_resident_bytes: 32 * 1024 * 1024,
        }
    }
}

impl AnimatedImageConfig {
    pub(crate) fn clamped(self) -> Self {
        Self {
            play: self.play,
            max_gpu_frame_slots: self.max_gpu_frame_slots.max(1),
            max_fps: self.max_fps.clamp(1.0, 60.0),
            inactive_max_fps: self.inactive_max_fps.clamp(0.25, 30.0),
            decode_ahead_frames: self.decode_ahead_frames.clamp(2, 64),
            max_resident_frames: self.max_resident_frames.max(1),
            max_resident_bytes: self.max_resident_bytes.max(4),
        }
    }

    pub(crate) fn minimum_frame_duration(self) -> Duration {
        Duration::from_secs_f32(1.0 / self.clamped().max_fps)
    }

    pub(crate) fn inactive_minimum_frame_duration(self) -> Duration {
        Duration::from_secs_f32(1.0 / self.clamped().inactive_max_fps)
    }
}

/// Controls when bounds-aware image decoding is allowed to start.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ImageDecodePolicy {
    /// Decode when the element explicitly requests bounds-aware decoding.
    #[default]
    Explicit,
    /// Decode bounds-aware images only when their visual bounds intersect the viewport.
    Visible,
}

/// Requested severity for an application image-memory trim.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ImageMemoryTrimLevel {
    /// Release temporary staging allocations.
    Light,
    /// Release unused decoded bitmaps while keeping compressed bytes.
    #[default]
    Moderate,
    /// Release all unused decoded image state while keeping the bounded byte cache.
    Aggressive,
}

/// Application-wide image pipeline resource limits and diagnostics thresholds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImagePipelineConfig {
    /// Controls animated image playback and GPU frame residency.
    pub animated: AnimatedImageConfig,
    /// Approximate maximum decoded image bytes retained by bounded caches by default.
    pub max_decoded_bytes: usize,
    /// Maximum compressed image bytes retained by the shared in-memory cache.
    pub max_compressed_bytes: usize,
    /// Maximum bytes retained by the reusable decoded bitmap pool.
    pub bitmap_pool_bytes: usize,
    /// Maximum capacity of one reusable bitmap buffer.
    pub bitmap_pool_max_buffer_bytes: usize,
    /// Maximum aggregate GPU atlas bytes allocated for image tiles.
    pub max_atlas_bytes: usize,
    /// Maximum number of GPU atlas textures allocated for image tiles.
    pub max_atlas_textures: usize,
    /// Controls whether bounds-aware decode is gated by visibility.
    pub decode_policy: ImageDecodePolicy,
    /// Trim image staging and bitmap pool when a window loses activation.
    pub trim_memory_on_hidden: bool,
    /// Log a slow image decode when this duration is exceeded.
    pub slow_decode_threshold: std::time::Duration,
    /// Log a slow atlas upload when this byte threshold is exceeded.
    pub slow_upload_bytes: usize,
    /// Log a slow atlas upload when this duration is exceeded.
    pub slow_upload_threshold: std::time::Duration,
}

impl Default for ImagePipelineConfig {
    fn default() -> Self {
        Self {
            animated: AnimatedImageConfig::default(),
            max_decoded_bytes: 128 * 1024 * 1024,
            max_compressed_bytes: 64 * 1024 * 1024,
            bitmap_pool_bytes: 64 * 1024 * 1024,
            bitmap_pool_max_buffer_bytes: 16 * 1024 * 1024,
            max_atlas_bytes: 256 * 1024 * 1024,
            max_atlas_textures: 32,
            decode_policy: ImageDecodePolicy::Explicit,
            trim_memory_on_hidden: false,
            slow_decode_threshold: std::time::Duration::from_millis(16),
            slow_upload_bytes: 8 * 1024 * 1024,
            slow_upload_threshold: std::time::Duration::from_millis(4),
        }
    }
}

/// Device-pixel target for size-aware image decoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ImageDecodeTarget {
    /// Requested width in device pixels.
    pub width: u32,
    /// Requested height in device pixels.
    pub height: u32,
}

impl ImageDecodeTarget {
    /// Creates a target size when both dimensions are non-zero.
    pub fn new(width: u32, height: u32) -> Option<Self> {
        (width > 0 && height > 0).then_some(Self { width, height })
    }

    pub(crate) fn size(self) -> Size<DevicePixels> {
        size(self.width.into(), self.height.into())
    }
}

/// Metadata for a target-size image decode.
#[derive(Clone, Debug)]
pub struct TargetImageDecodeMetadata {
    /// Original source width, when available.
    pub original_width: u32,
    /// Original source height, when available.
    pub original_height: u32,
    /// Requested decode target.
    pub target: ImageDecodeTarget,
    /// Decode implementation used.
    pub decode_mode: &'static str,
}

/// A reserved placeholder for future platform video pipelines.
///
/// MP4 and other video containers should use a dedicated media primitive that can
/// integrate platform decoders and GPU textures instead of entering `ImageFormat`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AnimatedMediaSource {
    /// A video or animated media file on disk.
    Path(std::path::PathBuf),
    /// A video or animated media URI.
    Uri(SharedString),
}
