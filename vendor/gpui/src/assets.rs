use crate::{
    BackgroundExecutor, DevicePixels, ObjectFit, Result, SharedString, Size,
    performance_metrics::{record_render_image_created, record_render_image_dropped},
    size,
};
use crossbeam_queue::ArrayQueue;
use smallvec::SmallVec;

use image::{
    AnimationDecoder, ColorType, Delay, Frame, ImageDecoder as _, ImageDecoderRect as _,
    ImageFormat, Rgba, RgbaImage,
    codecs::{bmp::BmpDecoder, gif::GifDecoder, png::PngDecoder, webp::WebPDecoder},
};
use std::{
    borrow::Cow,
    fmt,
    hash::Hash,
    io::{BufReader, Cursor},
    mem::MaybeUninit,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicUsize, Ordering, Ordering::SeqCst},
    },
    time::Duration,
};

/// A source of assets for this app to use.
pub trait AssetSource: 'static + Send + Sync {
    /// Load the given asset from the source path.
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>>;

    /// List the assets at the given path.
    fn list(&self, path: &str) -> Result<Vec<SharedString>>;
}

impl AssetSource for () {
    fn load(&self, _path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(None)
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(vec![])
    }
}

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

/// Application-wide image pipeline resource limits and diagnostics thresholds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImagePipelineConfig {
    /// Controls animated image playback and GPU frame residency.
    pub animated: AnimatedImageConfig,
    /// Approximate maximum decoded image bytes retained by bounded caches by default.
    pub max_decoded_bytes: usize,
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
            slow_decode_threshold: std::time::Duration::from_millis(16),
            slow_upload_bytes: 8 * 1024 * 1024,
            slow_upload_threshold: std::time::Duration::from_millis(4),
        }
    }
}

/// A cached and processed image.
pub struct RenderImage {
    /// The ID associated with this image
    pub id: ImageId,
    /// The scale factor of this image on render.
    pub(crate) scale_factor: f32,
    compressed_byte_len: usize,
    decode_duration: Option<std::time::Duration>,
    data: RenderImageData,
}

enum RenderImageData {
    Resident(SmallVec<[AnimatedFrame; 1]>),
    Streaming(Arc<StreamingImageState>),
}

#[derive(Clone)]
pub(crate) struct AnimatedFrame {
    sequence: usize,
    size: Size<DevicePixels>,
    delay: Delay,
    bytes: Arc<[u8]>,
    pixel_format: RenderImagePixelFormat,
}

struct StreamingImageState {
    source: AnimatedImageSource,
    target: Option<ImageDecodeTarget>,
    first_frame: AnimatedFrame,
    queue: ArrayQueue<AnimatedFrame>,
    next_sequence: AtomicUsize,
    next_source_index: AtomicUsize,
    decode_task_running: AtomicBool,
    completed: AtomicBool,
}

#[derive(Clone)]
pub(crate) struct AnimatedImageSource {
    pub(crate) bytes: Arc<[u8]>,
    pub(crate) format: image::ImageFormat,
}

pub(crate) struct DecodedAnimation {
    pub(crate) first_frame: AnimatedFrame,
    pub(crate) remaining_frames: SmallVec<[AnimatedFrame; 8]>,
    pub(crate) is_complete: bool,
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

impl PartialEq for RenderImage {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for RenderImage {}

fn next_render_image_id() -> ImageId {
    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

    ImageId(NEXT_ID.fetch_add(1, SeqCst))
}

impl RenderImage {
    /// Create a new image from the given data.
    pub fn new(data: impl Into<SmallVec<[Frame; 1]>>) -> Self {
        Self {
            id: next_render_image_id(),
            scale_factor: 1.0,
            compressed_byte_len: 0,
            decode_duration: None,
            data: RenderImageData::Resident(
                data.into()
                    .into_iter()
                    .enumerate()
                    .map(|(sequence, frame)| AnimatedFrame::from_bgra_frame(sequence, frame))
                    .collect(),
            ),
        }
        .track_live_cpu_bytes()
    }

    /// Create a new image from RGBA frames without converting them to BGRA.
    pub fn from_rgba_frames(data: impl Into<SmallVec<[Frame; 1]>>) -> Self {
        Self {
            id: next_render_image_id(),
            scale_factor: 1.0,
            compressed_byte_len: 0,
            decode_duration: None,
            data: RenderImageData::Resident(
                data.into()
                    .into_iter()
                    .enumerate()
                    .map(|(sequence, frame)| {
                        AnimatedFrame::from_rgba_frame_without_conversion(sequence, frame)
                    })
                    .collect(),
            ),
        }
        .track_live_cpu_bytes()
    }

    /// Create a single-frame image from raw 4-byte-per-pixel data.
    pub fn from_raw_pixels(
        width: u32,
        height: u32,
        pixel_format: RenderImagePixelFormat,
        bytes: Vec<u8>,
    ) -> Result<Self> {
        let pixel_count = width
            .checked_mul(height)
            .ok_or_else(|| anyhow::anyhow!("image dimensions overflow: {width}x{height}"))?;
        let expected_len = usize::try_from(pixel_count)
            .map_err(|_| anyhow::anyhow!("image pixel count does not fit usize: {width}x{height}"))?
            .checked_mul(pixel_format.bytes_per_pixel())
            .ok_or_else(|| anyhow::anyhow!("image byte length overflow: {width}x{height}"))?;
        if bytes.len() != expected_len {
            return Err(anyhow::anyhow!(
                "image byte length mismatch: expected {expected_len}, got {}",
                bytes.len()
            ));
        }
        let frame = AnimatedFrame::from_raw_bytes(
            0,
            size(width.into(), height.into()),
            pixel_format,
            bytes,
        );
        Ok(Self::from_resident_frames(SmallVec::from_elem(frame, 1)))
    }

    pub(crate) fn from_resident_frames(data: impl Into<SmallVec<[AnimatedFrame; 1]>>) -> Self {
        Self {
            id: next_render_image_id(),
            scale_factor: 1.0,
            compressed_byte_len: 0,
            decode_duration: None,
            data: RenderImageData::Resident(data.into()),
        }
        .track_live_cpu_bytes()
    }

    pub(crate) fn streaming(
        source: AnimatedImageSource,
        first_frame: AnimatedFrame,
        queued_frames: SmallVec<[AnimatedFrame; 8]>,
        config: AnimatedImageConfig,
    ) -> Self {
        Self::streaming_with_target(source, None, first_frame, queued_frames, config)
    }

    fn streaming_with_target(
        source: AnimatedImageSource,
        target: Option<ImageDecodeTarget>,
        first_frame: AnimatedFrame,
        queued_frames: SmallVec<[AnimatedFrame; 8]>,
        config: AnimatedImageConfig,
    ) -> Self {
        let config = config.clamped();
        let queue = ArrayQueue::new(config.decode_ahead_frames);
        let mut next_source_index = first_frame.sequence().saturating_add(1);
        for frame in queued_frames {
            next_source_index = next_source_index.max(frame.sequence().saturating_add(1));
            if queue.push(frame).is_err() {
                break;
            }
        }
        let state = StreamingImageState {
            source,
            target,
            first_frame,
            queue,
            next_sequence: AtomicUsize::new(next_source_index),
            next_source_index: AtomicUsize::new(next_source_index),
            decode_task_running: AtomicBool::new(false),
            completed: AtomicBool::new(false),
        };

        Self {
            id: next_render_image_id(),
            scale_factor: 1.0,
            compressed_byte_len: 0,
            decode_duration: None,
            data: RenderImageData::Streaming(Arc::new(state)),
        }
        .track_live_cpu_bytes()
    }

    /// Set diagnostic metadata collected while loading this image.
    pub fn with_pipeline_metadata(
        mut self,
        compressed_byte_len: usize,
        decode_duration: std::time::Duration,
    ) -> Self {
        self.compressed_byte_len = compressed_byte_len;
        self.decode_duration = Some(decode_duration);
        self
    }

    pub(crate) fn with_scale_factor(mut self, scale_factor: f32) -> Self {
        self.scale_factor = if scale_factor.is_finite() && scale_factor > 0.0 {
            scale_factor
        } else {
            1.0
        };
        self
    }

    fn track_live_cpu_bytes(self) -> Self {
        record_render_image_created(self.decoded_byte_len());
        self
    }

    /// Convert this image into a byte slice.
    pub fn as_bytes(&self, frame_index: usize) -> Option<&[u8]> {
        match &self.data {
            RenderImageData::Resident(frames) => frames.get(frame_index).map(AnimatedFrame::bytes),
            RenderImageData::Streaming(state) => {
                (frame_index == 0).then(|| state.first_frame.bytes())
            }
        }
    }

    /// Return the pixel format of the retained frame.
    pub fn pixel_format(&self, frame_index: usize) -> Option<RenderImagePixelFormat> {
        self.frame(frame_index).map(|frame| frame.pixel_format)
    }

    /// Get the size of this image, in pixels.
    pub fn size(&self, frame_index: usize) -> Size<DevicePixels> {
        self.frame(frame_index)
            .map(|frame| frame.size)
            .unwrap_or_else(|| self.frame(0).map_or(Size::default(), |frame| frame.size))
    }

    /// Get the delay of this frame from the previous
    pub fn delay(&self, frame_index: usize) -> Delay {
        self.frame(frame_index)
            .map(|frame| frame.delay)
            .unwrap_or_else(|| Delay::from_saturating_duration(Duration::from_millis(16)))
    }

    /// Get the number of frames for this image.
    pub fn frame_count(&self) -> usize {
        match &self.data {
            RenderImageData::Resident(frames) => frames.len(),
            RenderImageData::Streaming(_) => usize::MAX,
        }
    }

    /// Returns true when this image has more than one decoded frame.
    pub fn is_animated(&self) -> bool {
        match &self.data {
            RenderImageData::Resident(frames) => frames.len() > 1,
            RenderImageData::Streaming(_) => true,
        }
    }

    /// Estimated decoded bytes for all retained frames.
    pub fn decoded_byte_len(&self) -> usize {
        match &self.data {
            RenderImageData::Resident(frames) => frames.iter().map(AnimatedFrame::byte_len).sum(),
            RenderImageData::Streaming(state) => state.first_frame.byte_len(),
        }
    }

    /// Estimated decoded bytes for one retained frame.
    pub fn frame_byte_len(&self, frame_index: usize) -> usize {
        let size = self.size(frame_index);
        let width: usize = size.width.into();
        let height: usize = size.height.into();
        width.saturating_mul(height).saturating_mul(4)
    }

    /// Number of bytes read from the compressed source, when known.
    pub fn compressed_byte_len(&self) -> usize {
        self.compressed_byte_len
    }

    /// Time spent decoding this image, when known.
    pub fn decode_duration(&self) -> Option<std::time::Duration> {
        self.decode_duration
    }

    pub(crate) fn gpu_frame_slot_for_frame(
        &self,
        frame_index: usize,
        config: AnimatedImageConfig,
    ) -> usize {
        if !self.is_animated() {
            return frame_index;
        }

        frame_index % config.clamped().max_gpu_frame_slots
    }

    pub(crate) fn frame(&self, frame_index: usize) -> Option<AnimatedFrame> {
        match &self.data {
            RenderImageData::Resident(frames) => frames.get(frame_index).cloned(),
            RenderImageData::Streaming(state) => {
                (frame_index == 0).then(|| state.first_frame.clone())
            }
        }
    }

    pub(crate) fn next_streaming_frame(
        &self,
        current_sequence: usize,
        executor: &BackgroundExecutor,
    ) -> Option<AnimatedFrame> {
        let RenderImageData::Streaming(state) = &self.data else {
            return None;
        };
        let mut next_frame = None;
        while let Some(frame) = state.queue.pop() {
            if frame.sequence > current_sequence {
                next_frame = Some(frame);
                break;
            }
        }
        state.ensure_decode_task(executor);
        next_frame
    }
}

impl Drop for RenderImage {
    fn drop(&mut self) {
        record_render_image_dropped(self.decoded_byte_len());
    }
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
            bytes: Arc::from(data.into_raw()),
            pixel_format: RenderImagePixelFormat::Bgra8,
        }
    }

    pub(crate) fn from_rgba_image(sequence: usize, data: RgbaImage) -> Self {
        let mut data = data;
        for pixel in data.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        Self::from_bgra_frame(sequence, Frame::new(data))
    }

    pub(crate) fn from_bgra_image(sequence: usize, data: RgbaImage) -> Self {
        Self::from_bgra_frame(sequence, Frame::new(data))
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
            bytes: Arc::from(bytes),
            pixel_format: RenderImagePixelFormat::Bgra8,
        }
    }

    pub(crate) fn from_raw_bytes(
        sequence: usize,
        size: Size<DevicePixels>,
        pixel_format: RenderImagePixelFormat,
        bytes: Vec<u8>,
    ) -> Self {
        Self {
            sequence,
            size,
            delay: Delay::from_saturating_duration(Duration::ZERO),
            bytes: Arc::from(bytes),
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
            bytes: Arc::from(data.into_raw()),
            pixel_format: RenderImagePixelFormat::Bgra8,
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
            bytes: Arc::from(data.into_raw()),
            pixel_format: RenderImagePixelFormat::Rgba8,
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
        self.bytes.as_ref()
    }

    pub(crate) fn pixel_format(&self) -> RenderImagePixelFormat {
        self.pixel_format
    }

    fn byte_len(&self) -> usize {
        self.bytes.len()
    }
}

impl StreamingImageState {
    fn ensure_decode_task(self: &Arc<Self>, executor: &BackgroundExecutor) {
        if self.completed.load(Ordering::Acquire) {
            return;
        }
        if self
            .decode_task_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }

        let state = Arc::downgrade(self);
        let executor = executor.clone();
        executor
            .spawn(async move {
                decode_streaming_frames(state);
            })
            .detach();
    }
}

fn decode_streaming_frames(state: Weak<StreamingImageState>) {
    loop {
        let Some(shared_state) = state.upgrade() else {
            break;
        };
        let source = shared_state.source.clone();
        let target = shared_state.target;
        let mut next_sequence = shared_state.next_sequence.load(Ordering::Acquire);
        let skipped_frames = shared_state.next_source_index.load(Ordering::Acquire);
        drop(shared_state);

        match push_streaming_cycle(&source, target, &state, &mut next_sequence, skipped_frames) {
            Ok(StreamCycle::Dropped | StreamCycle::Paused) => break,
            Ok(StreamCycle::Finished) => {
                if let Some(state) = state.upgrade() {
                    state.next_sequence.store(next_sequence, Ordering::Release);
                    state.next_source_index.store(0, Ordering::Release);
                }
            }
            Err(error) => {
                if let Some(state) = state.upgrade() {
                    log::debug!("animated image streaming decode failed: {error}");
                    state.completed.store(true, Ordering::Release);
                }
                break;
            }
        }
    }

    if let Some(state) = state.upgrade() {
        state.decode_task_running.store(false, Ordering::Release);
    }
}

enum StreamCycle {
    Finished,
    Paused,
    Dropped,
}

fn push_streaming_cycle(
    source: &AnimatedImageSource,
    target: Option<ImageDecodeTarget>,
    state: &Weak<StreamingImageState>,
    next_sequence: &mut usize,
    skipped_frames: usize,
) -> Result<StreamCycle> {
    match source.format {
        ImageFormat::Gif => {
            let decoder = GifDecoder::new(Cursor::new(source.bytes.as_ref()))?;
            push_streaming_frames(
                decoder.into_frames(),
                target,
                state,
                next_sequence,
                skipped_frames,
            )
        }
        ImageFormat::Png => {
            let decoder = PngDecoder::new(Cursor::new(source.bytes.as_ref()))?;
            if !decoder.is_apng()? {
                return Ok(StreamCycle::Finished);
            }
            push_streaming_frames(
                decoder.apng()?.into_frames(),
                target,
                state,
                next_sequence,
                skipped_frames,
            )
        }
        ImageFormat::WebP => {
            let mut decoder = WebPDecoder::new(Cursor::new(source.bytes.as_ref()))?;
            if !decoder.has_animation() {
                return Ok(StreamCycle::Finished);
            }
            let _ = decoder.set_background_color(Rgba([0, 0, 0, 0]));
            push_streaming_frames(
                decoder.into_frames(),
                target,
                state,
                next_sequence,
                skipped_frames,
            )
        }
        _ => Ok(StreamCycle::Finished),
    }
}

fn push_streaming_frames(
    frames: image::Frames<'_>,
    target: Option<ImageDecodeTarget>,
    state: &Weak<StreamingImageState>,
    next_sequence: &mut usize,
    skipped_frames: usize,
) -> Result<StreamCycle> {
    for (source_index, frame) in frames.enumerate() {
        if source_index < skipped_frames {
            continue;
        }

        let frame = AnimatedFrame::from_rgba_frame(*next_sequence, frame?);
        let frame = if let Some(target) = target {
            resample_bgra_frame_to_target(frame, target)?
        } else {
            frame
        };
        let Some(state) = state.upgrade() else {
            return Ok(StreamCycle::Dropped);
        };
        if state.queue.push(frame).is_err() {
            return Ok(StreamCycle::Paused);
        }
        *next_sequence = next_sequence.saturating_add(1);
        state.next_sequence.store(*next_sequence, Ordering::Release);
        state
            .next_source_index
            .store(source_index.saturating_add(1), Ordering::Release);
    }

    Ok(StreamCycle::Finished)
}

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

pub(crate) fn decode_animation_prefix(
    source: &AnimatedImageSource,
    max_resident_frames: usize,
    max_resident_bytes: usize,
) -> Result<DecodedAnimation> {
    match source.format {
        ImageFormat::Gif => {
            let decoder = GifDecoder::new(Cursor::new(source.bytes.as_ref()))?;
            decode_frames_prefix(
                decoder.into_frames(),
                max_resident_frames,
                max_resident_bytes,
            )
        }
        ImageFormat::Png => decode_png_prefix(source, max_resident_frames, max_resident_bytes),
        ImageFormat::WebP => decode_webp_prefix(source, max_resident_frames, max_resident_bytes),
        ImageFormat::Jpeg => Ok(single_frame_decoded(decode_static_jpeg_frame(
            source.bytes.as_ref(),
        )?)),
        ImageFormat::Bmp => Ok(single_frame_decoded(decode_static_bmp_frame(
            source.bytes.as_ref(),
        )?)),
        format => anyhow::bail!("unsupported GPUI image asset format: {format:?}"),
    }
}

fn decode_png_prefix(
    source: &AnimatedImageSource,
    max_resident_frames: usize,
    max_resident_bytes: usize,
) -> Result<DecodedAnimation> {
    let decoder = PngDecoder::new(Cursor::new(source.bytes.as_ref()))?;
    if decoder.is_apng()? {
        let decoded = decode_frames_prefix(
            decoder.apng()?.into_frames(),
            max_resident_frames,
            max_resident_bytes,
        )?;
        if decoded.first_frame.byte_len() > 0 {
            return Ok(decoded);
        }
    }

    Ok(single_frame_decoded(decode_static_png_frame(
        source.bytes.as_ref(),
    )?))
}

fn decode_webp_prefix(
    source: &AnimatedImageSource,
    max_resident_frames: usize,
    max_resident_bytes: usize,
) -> Result<DecodedAnimation> {
    let mut decoder = WebPDecoder::new(Cursor::new(source.bytes.as_ref()))?;
    if decoder.has_animation() {
        let _ = decoder.set_background_color(Rgba([0, 0, 0, 0]));
        return decode_frames_prefix(
            decoder.into_frames(),
            max_resident_frames,
            max_resident_bytes,
        );
    }

    Ok(single_frame_decoded(decode_static_webp_frame(
        source.bytes.as_ref(),
    )?))
}

fn single_frame_decoded(first_frame: AnimatedFrame) -> DecodedAnimation {
    DecodedAnimation {
        first_frame,
        remaining_frames: SmallVec::new(),
        is_complete: true,
    }
}

fn decode_webp_to_target(
    bytes: &[u8],
    target: ImageDecodeTarget,
    object_fit: ObjectFit,
) -> Result<(RenderImage, TargetImageDecodeMetadata)> {
    let original_size = decode_webp_dimensions(bytes)?;
    let fitted_target = fitted_target_size(original_size, target, object_fit);
    let sample_target = high_quality_intermediate_target(original_size, fitted_target);
    let (output, decoded_target, original_width, original_height, initial_decode_mode) =
        decode_webp_bgra(bytes, Some((sample_target, object_fit)))?;
    let (image, decode_mode) = if decoded_target == fitted_target {
        let frame = AnimatedFrame::from_bgra_bytes(0, decoded_target.size(), output);
        (
            RenderImage::from_resident_frames(SmallVec::from_elem(frame, 1)),
            initial_decode_mode,
        )
    } else {
        let rgba = bgra_bytes_to_rgba_image(output, decoded_target)?;
        let (rgba, decode_mode) = resize_rgba_to_target(rgba, fitted_target, initial_decode_mode)?;
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

fn decode_static_webp_frame(bytes: &[u8]) -> Result<AnimatedFrame> {
    let (output, target, _, _, _) = decode_webp_bgra(bytes, None)?;
    Ok(AnimatedFrame::from_bgra_bytes(0, target.size(), output))
}

fn decode_webp_bgra(
    bytes: &[u8],
    target: Option<(ImageDecodeTarget, ObjectFit)>,
) -> Result<(Vec<u8>, ImageDecodeTarget, u32, u32, &'static str)> {
    use libwebp_sys::{
        MODE_BGRA, VP8_STATUS_OK, WebPDecBuffer, WebPDecode, WebPDecoderConfig, WebPGetFeatures,
        WebPInitDecoderConfig, WebPRGBABuffer,
    };

    let mut config = MaybeUninit::<WebPDecoderConfig>::uninit();
    let init_ok = unsafe {
        // SAFETY: `config` points to valid writable storage for libwebp initialization.
        WebPInitDecoderConfig(config.as_mut_ptr())
    };
    anyhow::ensure!(
        init_ok != 0,
        "libwebp decoder configuration initialization failed"
    );
    let mut config = unsafe {
        // SAFETY: libwebp reported successful initialization above.
        config.assume_init()
    };

    let feature_status = unsafe {
        // SAFETY: `bytes` is a valid byte slice for the duration of this call and config.input is initialized.
        WebPGetFeatures(bytes.as_ptr(), bytes.len(), &mut config.input)
    };
    anyhow::ensure!(
        feature_status == VP8_STATUS_OK,
        "libwebp failed to read features: status {feature_status}"
    );
    anyhow::ensure!(
        config.input.has_animation == 0,
        "target-size animated WebP decode is not supported"
    );

    let original_width = u32::try_from(config.input.width)
        .ok()
        .filter(|width| *width > 0)
        .ok_or_else(|| anyhow::anyhow!("libwebp reported invalid source width"))?;
    let original_height = u32::try_from(config.input.height)
        .ok()
        .filter(|height| *height > 0)
        .ok_or_else(|| anyhow::anyhow!("libwebp reported invalid source height"))?;
    let source_size = size(original_width, original_height);
    let fitted_target = if let Some((target, object_fit)) = target {
        fitted_target_size(source_size, target, object_fit)
    } else {
        ImageDecodeTarget {
            width: original_width,
            height: original_height,
        }
    };
    let output_len = bgra_len(fitted_target)?;
    let mut output = vec![0; output_len];

    config.options.use_scaling =
        i32::from(fitted_target.width != original_width || fitted_target.height != original_height);
    config.options.scaled_width = fitted_target.width as i32;
    config.options.scaled_height = fitted_target.height as i32;
    config.output = WebPDecBuffer {
        colorspace: MODE_BGRA,
        width: fitted_target.width as i32,
        height: fitted_target.height as i32,
        is_external_memory: 1,
        u: libwebp_sys::__WebPDecBufferUnion {
            RGBA: WebPRGBABuffer {
                rgba: output.as_mut_ptr(),
                stride: fitted_target.width as i32 * 4,
                size: output.len(),
            },
        },
        pad: [0; 4],
        private_memory: std::ptr::null_mut(),
    };

    let status = unsafe {
        // SAFETY: config.output points at `output`, which is sized for scaled BGRA pixels and remains live.
        WebPDecode(bytes.as_ptr(), bytes.len(), &mut config)
    };
    anyhow::ensure!(
        status == VP8_STATUS_OK,
        "libwebp decode failed: status {status}"
    );

    Ok((
        output,
        fitted_target,
        original_width,
        original_height,
        if config.options.use_scaling != 0 {
            "webp_scaled_decode"
        } else {
            "webp_direct_decode"
        },
    ))
}

fn decode_webp_dimensions(bytes: &[u8]) -> Result<Size<u32>> {
    use libwebp_sys::{VP8_STATUS_OK, WebPBitstreamFeatures, WebPGetFeatures};

    let mut features = MaybeUninit::<WebPBitstreamFeatures>::uninit();
    let status = unsafe {
        // SAFETY: `features` points to writable storage and `bytes` lives for this call.
        WebPGetFeatures(bytes.as_ptr(), bytes.len(), features.as_mut_ptr())
    };
    anyhow::ensure!(
        status == VP8_STATUS_OK,
        "libwebp failed to read features: status {status}"
    );
    let features = unsafe {
        // SAFETY: libwebp reported successful feature parsing above.
        features.assume_init()
    };
    let width = u32::try_from(features.width)
        .ok()
        .filter(|width| *width > 0)
        .ok_or_else(|| anyhow::anyhow!("libwebp reported invalid source width"))?;
    let height = u32::try_from(features.height)
        .ok()
        .filter(|height| *height > 0)
        .ok_or_else(|| anyhow::anyhow!("libwebp reported invalid source height"))?;
    Ok(size(width, height))
}

fn decode_static_jpeg_frame(bytes: &[u8]) -> Result<AnimatedFrame> {
    let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(bytes));
    let pixels = decoder.decode()?;
    let info = decoder
        .info()
        .ok_or_else(|| anyhow::anyhow!("JPEG decoder did not report image dimensions"))?;
    let rgba = jpeg_pixels_to_rgba_image(&pixels, info)?;
    Ok(AnimatedFrame::from_rgba_image(0, rgba))
}

fn decode_jpeg_path_to_target(
    path: &std::path::Path,
    target: ImageDecodeTarget,
    object_fit: ObjectFit,
) -> Result<(RenderImage, TargetImageDecodeMetadata)> {
    let file = std::fs::File::open(path)?;
    let mut decoder = jpeg_decoder::Decoder::new(BufReader::new(file));
    decoder.read_info()?;
    let original_info = decoder
        .info()
        .ok_or_else(|| anyhow::anyhow!("JPEG decoder did not report image dimensions"))?;
    let original_size = size(
        u32::from(original_info.width),
        u32::from(original_info.height),
    );
    let fitted_target = fitted_target_size(original_size, target, object_fit);

    let requested_width = u16::try_from(fitted_target.width.min(u32::from(u16::MAX)))?;
    let requested_height = u16::try_from(fitted_target.height.min(u32::from(u16::MAX)))?;
    decoder.scale(requested_width.max(1), requested_height.max(1))?;
    let pixels = decoder.decode()?;
    let scaled_info = decoder
        .info()
        .ok_or_else(|| anyhow::anyhow!("JPEG decoder did not report scaled dimensions"))?;
    let rgba = jpeg_pixels_to_rgba_image(&pixels, scaled_info)?;
    let (rgba, decode_mode) = resize_rgba_to_target(rgba, fitted_target, "jpeg_scaled_decode")?;
    let frame = AnimatedFrame::from_rgba_image(0, rgba);
    let image = RenderImage::from_resident_frames(SmallVec::from_elem(frame, 1));

    Ok((
        image,
        TargetImageDecodeMetadata {
            original_width: original_size.width,
            original_height: original_size.height,
            target: fitted_target,
            decode_mode,
        },
    ))
}

fn decode_static_bmp_frame(bytes: &[u8]) -> Result<AnimatedFrame> {
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

fn decode_bmp_to_target(
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

fn decode_bmp_path_to_target(
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

fn decode_jpeg_to_target(
    bytes: &[u8],
    target: ImageDecodeTarget,
    object_fit: ObjectFit,
) -> Result<(RenderImage, TargetImageDecodeMetadata)> {
    let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(bytes));
    decoder.read_info()?;
    let original_info = decoder
        .info()
        .ok_or_else(|| anyhow::anyhow!("JPEG decoder did not report image dimensions"))?;
    let original_size = size(
        u32::from(original_info.width),
        u32::from(original_info.height),
    );
    let fitted_target = fitted_target_size(original_size, target, object_fit);

    let requested_width = u16::try_from(fitted_target.width.min(u32::from(u16::MAX)))?;
    let requested_height = u16::try_from(fitted_target.height.min(u32::from(u16::MAX)))?;
    decoder.scale(requested_width.max(1), requested_height.max(1))?;
    let pixels = decoder.decode()?;
    let scaled_info = decoder
        .info()
        .ok_or_else(|| anyhow::anyhow!("JPEG decoder did not report scaled dimensions"))?;
    let rgba = jpeg_pixels_to_rgba_image(&pixels, scaled_info)?;
    let (rgba, decode_mode) = resize_rgba_to_target(rgba, fitted_target, "jpeg_scaled_decode")?;
    let frame = AnimatedFrame::from_rgba_image(0, rgba);
    let image = RenderImage::from_resident_frames(SmallVec::from_elem(frame, 1));

    Ok((
        image,
        TargetImageDecodeMetadata {
            original_width: original_size.width,
            original_height: original_size.height,
            target: fitted_target,
            decode_mode,
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

fn decode_png_to_target(
    bytes: &[u8],
    config: AnimatedImageConfig,
    target: ImageDecodeTarget,
    object_fit: ObjectFit,
) -> Result<(RenderImage, TargetImageDecodeMetadata)> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info()?;
    let info = reader.info().clone();

    if info.animation_control.is_some() || info.interlaced {
        return decode_image_bytes_to_target_via_full_decode(
            bytes,
            ImageFormat::Png,
            config,
            target,
            object_fit,
        );
    }

    let original_size = size(info.width, info.height);
    let fitted_target = fitted_target_size(original_size, target, object_fit);
    let sample_target = high_quality_intermediate_target(original_size, fitted_target);
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
    let (image, decode_mode) = if sample_target == fitted_target {
        let frame = AnimatedFrame::from_bgra_bytes(0, sample_target.size(), output);
        (
            RenderImage::from_resident_frames(SmallVec::from_elem(frame, 1)),
            "png_row_sample_decode",
        )
    } else {
        let rgba = bgra_bytes_to_rgba_image(output, sample_target)?;
        let (rgba, decode_mode) =
            resize_rgba_to_target(rgba, fitted_target, "png_row_sample_decode")?;
        let frame = AnimatedFrame::from_rgba_image(0, rgba);
        (
            RenderImage::from_resident_frames(SmallVec::from_elem(frame, 1)),
            decode_mode,
        )
    };
    Ok((
        image,
        TargetImageDecodeMetadata {
            original_width: original_size.width,
            original_height: original_size.height,
            target: fitted_target,
            decode_mode,
        },
    ))
}

fn decode_png_path_to_target(
    path: &std::path::Path,
    config: AnimatedImageConfig,
    target: ImageDecodeTarget,
    object_fit: ObjectFit,
) -> Result<(RenderImage, TargetImageDecodeMetadata)> {
    let file = std::fs::File::open(path)?;
    let mut decoder = png::Decoder::new(BufReader::new(file));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info()?;
    let info = reader.info().clone();

    if info.animation_control.is_some() || info.interlaced {
        let bytes = std::fs::read(path)?;
        return decode_image_bytes_to_target_via_full_decode(
            &bytes,
            ImageFormat::Png,
            config,
            target,
            object_fit,
        );
    }

    let original_size = size(info.width, info.height);
    let fitted_target = fitted_target_size(original_size, target, object_fit);
    let sample_target = high_quality_intermediate_target(original_size, fitted_target);
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
    let (image, decode_mode) = if sample_target == fitted_target {
        let frame = AnimatedFrame::from_bgra_bytes(0, sample_target.size(), output);
        (
            RenderImage::from_resident_frames(SmallVec::from_elem(frame, 1)),
            "png_row_sample_decode",
        )
    } else {
        let rgba = bgra_bytes_to_rgba_image(output, sample_target)?;
        let (rgba, decode_mode) =
            resize_rgba_to_target(rgba, fitted_target, "png_row_sample_decode")?;
        let frame = AnimatedFrame::from_rgba_image(0, rgba);
        (
            RenderImage::from_resident_frames(SmallVec::from_elem(frame, 1)),
            decode_mode,
        )
    };
    Ok((
        image,
        TargetImageDecodeMetadata {
            original_width: original_size.width,
            original_height: original_size.height,
            target: fitted_target,
            decode_mode,
        },
    ))
}

fn source_axis_for_target(target_axis: u32, source_len: u32, target_len: u32) -> u32 {
    ((u64::from(target_axis) * u64::from(source_len)) / u64::from(target_len))
        .min(u64::from(source_len.saturating_sub(1))) as u32
}

fn high_quality_intermediate_target(
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

fn bgra_bytes_to_rgba_image(bytes: Vec<u8>, size: ImageDecodeTarget) -> Result<RgbaImage> {
    let mut rgba = bytes;
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    RgbaImage::from_raw(size.width, size.height, rgba)
        .ok_or_else(|| anyhow::anyhow!("decoded image buffer dimensions were invalid"))
}

fn sample_png_rows_to_bgra<R: std::io::BufRead + std::io::Seek>(
    reader: &mut png::Reader<R>,
    source_width: u32,
    source_height: u32,
    color_type: png::ColorType,
    sample_target: ImageDecodeTarget,
) -> Result<Vec<u8>> {
    let source_row_len = reader
        .output_line_size(source_width)
        .ok_or_else(|| anyhow::anyhow!("PNG row size overflowed"))?;
    let output_len = bgra_len(sample_target)?;
    let mut source_row = vec![0; source_row_len];
    let mut output = vec![0; output_len];
    let mut next_target_y = 0u32;

    for source_y in 0..source_height {
        if reader.read_row(&mut source_row)?.is_none() {
            break;
        }

        while next_target_y < sample_target.height
            && source_axis_for_target(next_target_y, source_height, sample_target.height)
                == source_y
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
        let source_x = source_axis_for_target(target_x, source_width, target_width) as usize;
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

fn decode_static_png_frame(bytes: &[u8]) -> Result<AnimatedFrame> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info()?;
    let output_len = reader
        .output_buffer_size()
        .ok_or_else(|| anyhow::anyhow!("PNG decoded buffer size overflowed"))?;
    let mut pixels = vec![0; output_len];
    let output_info = reader.next_frame(&mut pixels)?;
    let pixels = &pixels[..output_info.buffer_size()];
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
    pixels: &[u8],
    color_type: png::ColorType,
    width: u32,
    height: u32,
) -> Result<Vec<u8>> {
    let pixel_count = width as usize * height as usize;
    let mut bgra = Vec::with_capacity(pixel_count * 4);
    match color_type {
        png::ColorType::Grayscale => {
            for &luma in pixels {
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
            for pixel in pixels.chunks_exact(4) {
                bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
            }
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

fn decode_image_bytes_to_target_via_full_decode(
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

fn resample_bgra_frame_to_target(
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

fn resize_rgba_to_target(
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

fn bgra_len(target: ImageDecodeTarget) -> Result<usize> {
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

fn decode_frames_prefix(
    frames: image::Frames<'_>,
    max_resident_frames: usize,
    max_resident_bytes: usize,
) -> Result<DecodedAnimation> {
    let mut frames = frames.enumerate();
    let Some((_, first_frame)) = frames.next() else {
        return Err(anyhow::anyhow!("animated image did not contain any frames"));
    };
    let first_frame = AnimatedFrame::from_rgba_frame(0, first_frame?);
    let mut decoded_byte_len = first_frame.byte_len();
    let mut remaining_frames = SmallVec::<[AnimatedFrame; 8]>::new();

    for (sequence, frame) in frames {
        if remaining_frames.len() + 1 >= max_resident_frames
            || decoded_byte_len >= max_resident_bytes
        {
            return Ok(DecodedAnimation {
                first_frame,
                remaining_frames,
                is_complete: false,
            });
        }

        let frame = AnimatedFrame::from_rgba_frame(sequence, frame?);
        decoded_byte_len = decoded_byte_len.saturating_add(frame.byte_len());
        if decoded_byte_len > max_resident_bytes {
            return Ok(DecodedAnimation {
                first_frame,
                remaining_frames,
                is_complete: false,
            });
        }
        remaining_frames.push(frame);
    }

    Ok(DecodedAnimation {
        first_frame,
        remaining_frames,
        is_complete: true,
    })
}

/// A reserved placeholder for future platform video pipelines.
///
/// MP4 and other video containers should use a dedicated media primitive that can
/// integrate platform decoders and GPU textures instead of entering [`ImageFormat`].
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AnimatedMediaSource {
    /// A video or animated media file on disk.
    Path(std::path::PathBuf),
    /// A video or animated media URI.
    Uri(SharedString),
}

impl fmt::Debug for RenderImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImageData")
            .field("id", &self.id)
            .field("size", &self.size(0))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{
        ExtendedColorType, ImageBuffer, ImageEncoder as _, RgbaImage,
        codecs::gif::{GifEncoder, Repeat},
    };
    use rand::SeedableRng as _;
    use std::io::Cursor;

    fn frame(width: u32, height: u32) -> Frame {
        let image: RgbaImage = ImageBuffer::from_pixel(width, height, image::Rgba([0, 0, 0, 255]));
        Frame::new(image)
    }

    fn rgba_frame(color: [u8; 4]) -> Frame {
        Frame::from_parts(
            ImageBuffer::from_pixel(1, 1, image::Rgba(color)),
            0,
            0,
            Delay::from_saturating_duration(Duration::from_millis(20)),
        )
    }

    #[test]
    fn animated_frame_slots_are_bounded() {
        let image = RenderImage::new(vec![frame(2, 2), frame(2, 2), frame(2, 2), frame(2, 2)]);
        let config = AnimatedImageConfig {
            max_gpu_frame_slots: 3,
            ..AnimatedImageConfig::default()
        };

        assert_eq!(image.gpu_frame_slot_for_frame(0, config), 0);
        assert_eq!(image.gpu_frame_slot_for_frame(1, config), 1);
        assert_eq!(image.gpu_frame_slot_for_frame(2, config), 2);
        assert_eq!(image.gpu_frame_slot_for_frame(3, config), 0);
    }

    #[test]
    fn decoded_byte_len_counts_all_frames() {
        let image = RenderImage::new(vec![frame(2, 3), frame(4, 5)]);

        assert_eq!(image.frame_byte_len(0), 2 * 3 * 4);
        assert_eq!(image.frame_byte_len(1), 4 * 5 * 4);
        assert_eq!(image.decoded_byte_len(), (2 * 3 * 4) + (4 * 5 * 4));
    }

    #[test]
    fn raw_rgba_image_retains_rgba_bytes() {
        let pixels = vec![1, 2, 3, 255];
        let image =
            RenderImage::from_raw_pixels(1, 1, RenderImagePixelFormat::Rgba8, pixels.clone())
                .unwrap();

        assert_eq!(image.as_bytes(0).unwrap(), pixels);
        assert_eq!(image.pixel_format(0), Some(RenderImagePixelFormat::Rgba8));
    }

    #[test]
    fn new_image_keeps_existing_bgra_semantics() {
        let image = RenderImage::new(vec![rgba_frame([1, 2, 3, 255])]);

        assert_eq!(image.as_bytes(0).unwrap(), &[1, 2, 3, 255]);
        assert_eq!(image.pixel_format(0), Some(RenderImagePixelFormat::Bgra8));
    }

    #[test]
    fn animated_config_clamps_runtime_values() {
        let config = AnimatedImageConfig {
            play: false,
            max_gpu_frame_slots: 0,
            max_fps: 999.0,
            inactive_max_fps: 999.0,
            decode_ahead_frames: 1,
            max_resident_frames: 0,
            max_resident_bytes: 0,
        }
        .clamped();

        assert!(!config.play);
        assert_eq!(config.max_gpu_frame_slots, 1);
        assert_eq!(config.max_fps, 60.0);
        assert_eq!(config.inactive_max_fps, 30.0);
        assert_eq!(config.decode_ahead_frames, 2);
        assert_eq!(config.max_resident_frames, 1);
        assert_eq!(config.max_resident_bytes, 4);
    }

    #[test]
    fn cover_target_preserves_aspect_ratio() {
        let target = ImageDecodeTarget::new(800, 600).unwrap();
        let fitted = fitted_target_size(size(3840, 2160), target, ObjectFit::Cover);

        assert_eq!(fitted.width, 1067);
        assert_eq!(fitted.height, 600);
    }

    #[test]
    fn contain_target_preserves_aspect_ratio() {
        let target = ImageDecodeTarget::new(44, 44).unwrap();
        let fitted = fitted_target_size(size(1465, 1496), target, ObjectFit::Contain);

        assert_eq!(fitted.width, 44);
        assert_eq!(fitted.height, 44);
    }

    #[test]
    fn gif_decode_helper_keeps_multiple_bgra_frames() {
        let mut bytes = Vec::new();
        {
            let mut encoder = GifEncoder::new(&mut bytes);
            encoder.set_repeat(Repeat::Infinite).unwrap();
            encoder
                .encode_frames([rgba_frame([255, 0, 0, 255]), rgba_frame([0, 255, 0, 255])])
                .unwrap();
        }

        let image = decode_image_bytes(
            &bytes,
            ImageFormat::Gif,
            AnimatedImageConfig::default(),
            None,
        )
        .unwrap();

        assert!(image.is_animated());
        assert_eq!(image.frame_count(), 2);
        assert_eq!(image.as_bytes(0).unwrap(), &[0, 0, 255, 255]);
    }

    #[test]
    fn apng_decode_helper_keeps_multiple_frames() {
        let bytes = animated_png_bytes();
        let image = decode_image_bytes(
            &bytes,
            ImageFormat::Png,
            AnimatedImageConfig::default(),
            None,
        )
        .unwrap();

        assert!(image.is_animated());
        assert_eq!(image.frame_count(), 2);
    }

    #[test]
    fn static_png_is_not_treated_as_animation() {
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(&[255, 0, 0, 255], 1, 1, ExtendedColorType::Rgba8)
            .unwrap();

        let image = decode_image_bytes(
            &bytes,
            ImageFormat::Png,
            AnimatedImageConfig::default(),
            None,
        )
        .unwrap();

        assert!(!image.is_animated());
        assert_eq!(image.frame_count(), 1);
    }

    #[test]
    fn png_target_decode_uses_element_sized_resident_buffer() {
        let bytes = encoded_rgba_image(128, 96, |writer| {
            image::codecs::png::PngEncoder::new(writer).write_image(
                &solid_rgba_pixels(128, 96),
                128,
                96,
                ExtendedColorType::Rgba8,
            )
        });
        let target = ImageDecodeTarget::new(32, 24).unwrap();
        let (image, metadata) = decode_image_bytes_to_target(
            &bytes,
            ImageFormat::Png,
            AnimatedImageConfig::default(),
            target,
            ObjectFit::Fill,
        )
        .unwrap();

        assert_eq!(image.size(0), target.size());
        assert_eq!(image.decoded_byte_len(), 32 * 24 * 4);
        assert!(
            metadata.decode_mode == "png_row_sample_decode"
                || metadata.decode_mode == "decoder_scaled_then_resized"
        );
    }

    #[test]
    fn jpeg_target_decode_uses_scaled_decoder_before_resizing() {
        let bytes = encoded_rgba_image(128, 96, |writer| {
            image::codecs::jpeg::JpegEncoder::new_with_quality(writer, 90).write_image(
                &solid_rgb_pixels(128, 96),
                128,
                96,
                ExtendedColorType::Rgb8,
            )
        });
        let target = ImageDecodeTarget::new(32, 24).unwrap();
        let (image, metadata) = decode_image_bytes_to_target(
            &bytes,
            ImageFormat::Jpeg,
            AnimatedImageConfig::default(),
            target,
            ObjectFit::Fill,
        )
        .unwrap();

        assert_eq!(image.size(0), target.size());
        assert_eq!(image.decoded_byte_len(), 32 * 24 * 4);
        assert!(
            metadata.decode_mode == "jpeg_scaled_decode"
                || metadata.decode_mode == "decoder_scaled_then_resized"
        );
    }

    #[test]
    fn bmp_target_decode_samples_rows_without_retaining_original_size() {
        let bytes = encoded_rgba_image(128, 96, |writer| {
            image::codecs::bmp::BmpEncoder::new(writer).write_image(
                &solid_rgba_pixels(128, 96),
                128,
                96,
                ExtendedColorType::Rgba8,
            )
        });
        let target = ImageDecodeTarget::new(32, 24).unwrap();
        let (image, metadata) = decode_image_bytes_to_target(
            &bytes,
            ImageFormat::Bmp,
            AnimatedImageConfig::default(),
            target,
            ObjectFit::Fill,
        )
        .unwrap();

        assert_eq!(image.size(0), target.size());
        assert_eq!(image.decoded_byte_len(), 32 * 24 * 4);
        assert!(
            metadata.decode_mode == "bmp_rect_sample_decode"
                || metadata.decode_mode == "decoder_scaled_then_resized"
        );
    }

    #[test]
    fn target_decode_keeps_animated_png_playable_after_resize() {
        let bytes = animated_png_bytes_with_size(64, 64);
        let config = AnimatedImageConfig {
            max_resident_bytes: 4 * 4 * 4 * 2,
            ..AnimatedImageConfig::default()
        };
        let target = ImageDecodeTarget::new(4, 4).unwrap();
        let (image, metadata) =
            decode_image_bytes_to_target(&bytes, ImageFormat::Png, config, target, ObjectFit::Fill)
                .unwrap();

        assert!(image.is_animated());
        assert_eq!(image.frame_count(), 2);
        assert_eq!(image.size(0), target.size());
        assert_eq!(image.size(1), target.size());
        assert_eq!(image.decoded_byte_len(), 4 * 4 * 4 * 2);
        assert_eq!(metadata.decode_mode, "animated_frame_sample_decode");
    }

    #[test]
    fn target_decode_streams_large_animation_after_resize() {
        let bytes = animated_png_bytes_with_size(64, 64);
        let config = AnimatedImageConfig {
            max_resident_frames: 1,
            max_resident_bytes: 4 * 4 * 4,
            ..AnimatedImageConfig::default()
        };
        let target = ImageDecodeTarget::new(4, 4).unwrap();
        let (image, _) =
            decode_image_bytes_to_target(&bytes, ImageFormat::Png, config, target, ObjectFit::Fill)
                .unwrap();

        assert!(matches!(image.data, RenderImageData::Streaming(_)));
        assert!(image.is_animated());
        assert_eq!(image.frame_count(), usize::MAX);
        assert_eq!(image.size(0), target.size());
    }

    #[test]
    fn large_animation_enters_streaming_mode() {
        let bytes = animated_png_bytes();
        let config = AnimatedImageConfig {
            max_resident_frames: 1,
            max_resident_bytes: 4,
            ..AnimatedImageConfig::default()
        };
        let image = decode_image_bytes(&bytes, ImageFormat::Png, config, None).unwrap();

        assert!(matches!(image.data, RenderImageData::Resident(_)));

        let image = decode_image_bytes(
            &bytes,
            ImageFormat::Png,
            config,
            Some(BackgroundExecutor::new(std::sync::Arc::new(
                crate::TestDispatcher::new(rand::rngs::StdRng::seed_from_u64(1)),
            ))),
        )
        .unwrap();

        assert!(matches!(image.data, RenderImageData::Streaming(_)));
    }

    #[test]
    fn streaming_animation_decoded_byte_len_uses_resident_frames_only() {
        let bytes = animated_png_bytes();
        let config = AnimatedImageConfig {
            max_resident_frames: 1,
            max_resident_bytes: 4,
            ..AnimatedImageConfig::default()
        };
        let image = decode_image_bytes(
            &bytes,
            ImageFormat::Png,
            config,
            Some(BackgroundExecutor::new(std::sync::Arc::new(
                crate::TestDispatcher::new(rand::rngs::StdRng::seed_from_u64(2)),
            ))),
        )
        .unwrap();

        assert!(matches!(image.data, RenderImageData::Streaming(_)));
        assert_eq!(image.frame_count(), usize::MAX);
        assert_eq!(image.decoded_byte_len(), image.frame_byte_len(0));
    }

    #[test]
    fn streaming_animation_restarts_decoder_after_queue_pause() {
        let bytes = animated_png_bytes_with_frame_count(4);
        let config = AnimatedImageConfig {
            decode_ahead_frames: 2,
            max_resident_frames: 1,
            max_resident_bytes: 4,
            ..AnimatedImageConfig::default()
        };
        let executor = BackgroundExecutor::new(std::sync::Arc::new(crate::TestDispatcher::new(
            rand::rngs::StdRng::seed_from_u64(3),
        )));
        let image =
            decode_image_bytes(&bytes, ImageFormat::Png, config, Some(executor.clone())).unwrap();

        executor.run_until_parked();
        let frame = image.next_streaming_frame(0, &executor).unwrap();
        executor.run_until_parked();
        let frame = image
            .next_streaming_frame(frame.sequence(), &executor)
            .unwrap();
        let frame = image
            .next_streaming_frame(frame.sequence(), &executor)
            .unwrap();

        assert_eq!(frame.sequence(), 3);
    }

    fn animated_png_bytes() -> Vec<u8> {
        animated_png_bytes_with_size(1, 1)
    }

    fn animated_png_bytes_with_size(width: u32, height: u32) -> Vec<u8> {
        animated_png_bytes_with_size_and_frame_count(width, height, 2)
    }

    fn animated_png_bytes_with_frame_count(frame_count: u32) -> Vec<u8> {
        animated_png_bytes_with_size_and_frame_count(1, 1, frame_count)
    }

    fn animated_png_bytes_with_size_and_frame_count(
        width: u32,
        height: u32,
        frame_count: u32,
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(Cursor::new(&mut bytes), width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_animated(frame_count, 0).unwrap();
            encoder.set_frame_delay(20, 1000).unwrap();
            let mut writer = encoder.write_header().unwrap();
            writer
                .write_image_data(&solid_color_rgba_pixels(width, height, [255, 0, 0, 255]))
                .unwrap();
            for index in 1..frame_count {
                let color = if index % 2 == 0 {
                    [0, 0, 255, 255]
                } else {
                    [0, 255, 0, 255]
                };
                writer.set_frame_delay(20, 1000).unwrap();
                writer
                    .write_image_data(&solid_color_rgba_pixels(width, height, color))
                    .unwrap();
            }
            writer.finish().unwrap();
        }
        bytes
    }

    fn encoded_rgba_image(
        width: u32,
        height: u32,
        encode: impl FnOnce(&mut Vec<u8>) -> image::ImageResult<()>,
    ) -> Vec<u8> {
        let mut bytes = Vec::with_capacity((width * height) as usize);
        encode(&mut bytes).unwrap();
        bytes
    }

    fn solid_color_rgba_pixels(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
        color
            .into_iter()
            .cycle()
            .take(width as usize * height as usize * 4)
            .collect()
    }

    fn solid_rgba_pixels(width: u32, height: u32) -> Vec<u8> {
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                pixels.extend_from_slice(&[x as u8, y as u8, 192, 255]);
            }
        }
        pixels
    }

    fn solid_rgb_pixels(width: u32, height: u32) -> Vec<u8> {
        let mut pixels = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            for x in 0..width {
                pixels.extend_from_slice(&[x as u8, y as u8, 192]);
            }
        }
        pixels
    }
}
