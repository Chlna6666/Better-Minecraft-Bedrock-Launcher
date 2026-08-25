use super::animation_stream::{release_animation_queue_bytes, reserve_animation_queue_bytes};
use super::{AnimatedFrame, AnimatedImageConfig, AnimationStream, EncodedImage, ImageRenderSize};
use crate::{DevicePixels, Result, Size, size};
use image::{Delay, Frame};
use linked_hash_map::LinkedHashMap;
use parking_lot::Mutex;
use smallvec::SmallVec;
use std::{
    any::TypeId,
    fmt,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicUsize, Ordering, Ordering::SeqCst},
        mpsc::{TryRecvError, sync_channel},
    },
    time::Duration,
};

const MAX_INTERNED_IMAGE_IDS: usize = 8_192;

/// A unique identifier for the image cache.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ImageId(pub usize);

/// Pixel format used by image frames uploaded to the renderer.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum ImagePixelFormat {
    /// Blue, green, red, alpha byte order.
    Bgra8,
    /// Red, green, blue, alpha byte order.
    Rgba8,
}

impl ImagePixelFormat {
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
    pub(crate) pixel_format: ImagePixelFormat,
}

/// A cached and processed image.
pub struct RenderImage {
    /// The ID associated with this image
    pub id: ImageId,
    /// The scale factor of this image on render.
    pub(crate) scale_factor: f32,
    compressed_byte_len: usize,
    processing_duration: Option<std::time::Duration>,
    pub(in crate::assets) storage: RenderImageStorage,
}

pub(in crate::assets) enum RenderImageStorage {
    Resident(SmallVec<[AnimatedFrame; 1]>),
    Streaming(Arc<AnimationStream>),
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

/// Returns a stable [`ImageId`] for a repeatable decode source.
///
/// Asset loaders that can re-decode the same source with the same decode parameters use this to
/// keep the id stable across evictions of the decoded image. Because atlas tiles are keyed by
/// `RenderImageParams { image_id, .. }` and retained scenes keep tiles resident after
/// `drop_image`, a re-decode with a fresh id would allocate a new tile on every
/// trim/redecode cycle until the atlas budget is exhausted. Reusing the id lets the re-decoded
/// image (which is pixel-identical, since the source and decode parameters match) hit the
/// existing tile instead.
///
/// The least recently used mappings are evicted after a fixed limit. Atlas residency is already
/// independently bounded, so an evicted source may receive a new id without making this CPU-side
/// metadata table unbounded. Manually constructed [`RenderImage`]s keep unique auto-incremented
/// ids.
pub(crate) fn interned_render_image_id(loader: TypeId, source_hash: u64) -> ImageId {
    static INTERNED: OnceLock<Mutex<LinkedHashMap<(TypeId, u64), ImageId>>> = OnceLock::new();

    let key = (loader, source_hash);
    let mut interned = INTERNED.get_or_init(Default::default).lock();
    if let Some(image_id) = interned.get_refresh(&key).copied() {
        return image_id;
    }
    if interned.len() >= MAX_INTERNED_IMAGE_IDS {
        interned.pop_front();
    }
    let image_id = next_render_image_id();
    interned.insert(key, image_id);
    image_id
}

impl RenderImage {
    /// Create a new image from the given data.
    pub fn new(frames: impl Into<SmallVec<[Frame; 1]>>) -> Self {
        Self {
            id: next_render_image_id(),
            scale_factor: 1.0,
            compressed_byte_len: 0,
            processing_duration: None,
            storage: RenderImageStorage::Resident(
                frames
                    .into()
                    .into_iter()
                    .enumerate()
                    .map(|(sequence, frame)| AnimatedFrame::from_bgra_frame(sequence, frame))
                    .collect(),
            ),
        }
    }

    /// Create a new image from RGBA frames without converting them to BGRA.
    pub fn from_rgba_frames(frames: impl Into<SmallVec<[Frame; 1]>>) -> Self {
        Self {
            id: next_render_image_id(),
            scale_factor: 1.0,
            compressed_byte_len: 0,
            processing_duration: None,
            storage: RenderImageStorage::Resident(
                frames
                    .into()
                    .into_iter()
                    .enumerate()
                    .map(|(sequence, frame)| {
                        AnimatedFrame::from_rgba_frame_without_conversion(sequence, frame)
                    })
                    .collect(),
            ),
        }
    }

    /// Create a single-frame image from raw 4-byte-per-pixel data.
    pub fn from_raw_pixels(
        width: u32,
        height: u32,
        pixel_format: ImagePixelFormat,
        bytes: Vec<u8>,
    ) -> Result<Self> {
        Self::from_raw_pixel_bytes(width, height, pixel_format, bytes)
    }

    /// Create a single-frame image from shared raw pixel bytes.
    ///
    /// This constructor keeps an already shared pixel buffer without copying it.
    /// Use [`RenderImage::from_raw_pixels`] when the source is an owned `Vec<u8>`.
    pub fn from_raw_pixel_bytes(
        width: u32,
        height: u32,
        pixel_format: ImagePixelFormat,
        bytes: impl Into<Arc<[u8]>>,
    ) -> Result<Self> {
        let bytes = bytes.into();
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
        let frame = AnimatedFrame::from_raw_pixel_bytes(
            0,
            size(width.into(), height.into()),
            pixel_format,
            bytes,
        );
        Ok(Self::from_resident_frames(SmallVec::from_elem(frame, 1)))
    }

    pub(crate) fn from_resident_frames(frames: impl Into<SmallVec<[AnimatedFrame; 1]>>) -> Self {
        Self {
            id: next_render_image_id(),
            scale_factor: 1.0,
            compressed_byte_len: 0,
            processing_duration: None,
            storage: RenderImageStorage::Resident(frames.into()),
        }
    }

    pub(crate) fn streaming(
        source: EncodedImage,
        first_frame: AnimatedFrame,
        queued_frames: SmallVec<[AnimatedFrame; 8]>,
        config: AnimatedImageConfig,
    ) -> Self {
        Self::streaming_with_target(source, None, first_frame, queued_frames, config)
    }

    pub(in crate::assets) fn streaming_with_target(
        source: EncodedImage,
        target: Option<ImageRenderSize>,
        first_frame: AnimatedFrame,
        queued_frames: SmallVec<[AnimatedFrame; 8]>,
        config: AnimatedImageConfig,
    ) -> Self {
        let config = config.clamped();
        let (queue_sender, queue_receiver) = sync_channel(config.prefetch_frames);
        let mut next_source_index = first_frame.sequence().saturating_add(1);
        let mut queued_byte_len = 0usize;
        let mut queued_frame_count = 0usize;
        for frame in queued_frames {
            let next_frame_index = frame.sequence().saturating_add(1);
            let frame_byte_len = frame.byte_len();
            if queued_byte_len != 0
                && queued_byte_len.saturating_add(frame_byte_len) > config.prefetch_byte_limit
            {
                break;
            }
            if !reserve_animation_queue_bytes(frame_byte_len) {
                break;
            }
            if queue_sender.try_send(frame).is_err() {
                release_animation_queue_bytes(frame_byte_len);
                break;
            }
            next_source_index = next_source_index.max(next_frame_index);
            queued_byte_len = queued_byte_len.saturating_add(frame_byte_len);
            queued_frame_count = queued_frame_count.saturating_add(1);
        }
        let state = AnimationStream {
            source,
            target,
            first_frame,
            queue_sender,
            queue_receiver: Mutex::new(queue_receiver),
            next_sequence: next_source_index,
            next_source_index,
            prefetch_frames: config.prefetch_frames,
            prefetch_byte_limit: config.prefetch_byte_limit,
            queued_frame_count: AtomicUsize::new(queued_frame_count),
            queued_byte_len: AtomicUsize::new(queued_byte_len),
            delivered_byte_len: AtomicUsize::new(0),
            stream_task_running: AtomicBool::new(false),
            completed: AtomicBool::new(false),
            worker_index: AtomicUsize::new(usize::MAX),
        };

        Self {
            id: next_render_image_id(),
            scale_factor: 1.0,
            compressed_byte_len: 0,
            processing_duration: None,
            storage: RenderImageStorage::Streaming(Arc::new(state)),
        }
    }

    /// Set diagnostic metadata collected while loading this image.
    pub fn with_processing_metrics(
        mut self,
        compressed_byte_len: usize,
        processing_duration: std::time::Duration,
    ) -> Self {
        self.compressed_byte_len = compressed_byte_len;
        self.processing_duration = Some(processing_duration);
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

    /// Convert this image into a byte slice.
    pub fn as_bytes(&self, frame_index: usize) -> Option<&[u8]> {
        match &self.storage {
            RenderImageStorage::Resident(frames) => {
                frames.get(frame_index).map(AnimatedFrame::bytes)
            }
            RenderImageStorage::Streaming(state) => {
                (frame_index == 0).then(|| state.first_frame.bytes())
            }
        }
    }

    /// Return the pixel format of the retained frame.
    pub fn pixel_format(&self, frame_index: usize) -> Option<ImagePixelFormat> {
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
        match &self.storage {
            RenderImageStorage::Resident(frames) => frames.len(),
            RenderImageStorage::Streaming(_) => usize::MAX,
        }
    }

    /// Returns true when this image has more than one decoded frame.
    pub fn is_animated(&self) -> bool {
        match &self.storage {
            RenderImageStorage::Resident(frames) => frames.len() > 1,
            RenderImageStorage::Streaming(_) => true,
        }
    }

    /// Estimated decoded bytes for all retained frames.
    pub fn resident_byte_len(&self) -> usize {
        match &self.storage {
            RenderImageStorage::Resident(frames) => {
                frames.iter().map(AnimatedFrame::byte_len).sum()
            }
            RenderImageStorage::Streaming(state) => state
                .first_frame
                .byte_len()
                .saturating_add(state.queued_byte_len.load(Ordering::Relaxed))
                .saturating_add(state.delivered_byte_len.load(Ordering::Relaxed)),
        }
    }

    pub(crate) fn cache_cost_byte_len(&self) -> usize {
        let decoded_bytes = self.resident_byte_len();
        match &self.storage {
            RenderImageStorage::Resident(_) => decoded_bytes,
            RenderImageStorage::Streaming(state) => {
                decoded_bytes.saturating_add(state.source.bytes.len())
            }
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
    pub fn processing_duration(&self) -> Option<std::time::Duration> {
        self.processing_duration
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
        match &self.storage {
            RenderImageStorage::Resident(frames) => frames.get(frame_index).cloned(),
            RenderImageStorage::Streaming(state) => {
                (frame_index == 0).then(|| state.first_frame.clone())
            }
        }
    }

    pub(crate) fn next_streaming_frame(&self, current_sequence: usize) -> Option<AnimatedFrame> {
        let RenderImageStorage::Streaming(state) = &self.storage else {
            return None;
        };
        let mut next_frame = None;
        let mut stale_frame_count = 0usize;
        {
            let queue_receiver = state.queue_receiver.lock();
            loop {
                match queue_receiver.try_recv() {
                    Ok(frame) if frame.sequence > current_sequence => {
                        let frame_byte_len = frame.byte_len();
                        state.release_queued_frame(frame_byte_len);
                        state
                            .delivered_byte_len
                            .store(frame_byte_len, Ordering::Relaxed);
                        next_frame = Some(frame);
                        break;
                    }
                    Ok(frame) => {
                        state.release_queued_frame(frame.byte_len());
                        stale_frame_count = stale_frame_count.saturating_add(1);
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        state.completed.store(true, SeqCst);
                        break;
                    }
                }
            }
        }
        crate::diagnostics::performance_metrics::record_animation_stale_frame_count(
            stale_frame_count,
        );
        state.ensure_stream_task();
        next_frame
    }
}

impl fmt::Debug for RenderImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RenderImage")
            .field("id", &self.id)
            .field("size", &self.size(0))
            .finish()
    }
}
