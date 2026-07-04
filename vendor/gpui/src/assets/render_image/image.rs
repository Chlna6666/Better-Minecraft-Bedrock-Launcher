use super::{frame::AnimatedFrame, source::AnimatedImageSource, streaming::StreamingImageState};
use crate::{BackgroundExecutor, DevicePixels, Result, Size, size};
use crossbeam_queue::ArrayQueue;
use image::{Delay, Frame};
use smallvec::SmallVec;
use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst},
    },
    time::Duration,
};

use super::super::types::*;

/// A cached and processed image.
pub struct RenderImage {
    /// The ID associated with this image
    pub id: ImageId,
    /// The scale factor of this image on render.
    pub(crate) scale_factor: f32,
    compressed_byte_len: usize,
    decode_duration: Option<std::time::Duration>,
    pub(in crate::assets) data: RenderImageData,
}

pub(in crate::assets) enum RenderImageData {
    Resident(SmallVec<[AnimatedFrame; 1]>),
    Streaming(Arc<StreamingImageState>),
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
    }

    /// Create a single-frame image from raw 4-byte-per-pixel data.
    pub fn from_raw_pixels(
        width: u32,
        height: u32,
        pixel_format: RenderImagePixelFormat,
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
        pixel_format: RenderImagePixelFormat,
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

    pub(crate) fn from_resident_frames(data: impl Into<SmallVec<[AnimatedFrame; 1]>>) -> Self {
        Self {
            id: next_render_image_id(),
            scale_factor: 1.0,
            compressed_byte_len: 0,
            decode_duration: None,
            data: RenderImageData::Resident(data.into()),
        }
    }

    pub(crate) fn streaming(
        source: AnimatedImageSource,
        first_frame: AnimatedFrame,
        queued_frames: SmallVec<[AnimatedFrame; 8]>,
        config: AnimatedImageConfig,
    ) -> Self {
        Self::streaming_with_target(source, None, first_frame, queued_frames, config)
    }

    pub(in crate::assets) fn streaming_with_target(
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

impl fmt::Debug for RenderImage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImageData")
            .field("id", &self.id)
            .field("size", &self.size(0))
            .finish()
    }
}
