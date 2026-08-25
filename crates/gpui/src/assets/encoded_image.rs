use super::animated_image::{AnimatedImageFrames, initial_frames};
use super::render_image::{RenderImage, RenderImageStorage};
use super::{
    AnimatedFrame, AnimatedImageConfig, ImageRenderInfo, ImageRenderSize, bmp, jpeg, png, resample,
    webp,
};
use crate::{ObjectFit, Result};
use image::ImageFormat;
use smallvec::SmallVec;
use std::sync::Arc;

/// Encoded raster image bytes together with their container format.
#[derive(Clone)]
pub struct EncodedImage {
    pub(in crate::assets) bytes: Arc<[u8]>,
    pub(in crate::assets) format: ImageFormat,
}

impl EncodedImage {
    /// Creates an encoded image source without copying already shared bytes.
    pub fn new(format: ImageFormat, bytes: impl Into<Arc<[u8]>>) -> Self {
        Self {
            bytes: bytes.into(),
            format,
        }
    }

    /// Produces a renderable image, retaining animation frames according to `config`.
    ///
    /// Animations that exceed the resident limits continue through GPUI's bounded animation
    /// worker pool; callers do not provide or own a background executor.
    pub fn render(self, config: AnimatedImageConfig) -> Result<RenderImage> {
        let config = config.clamped();
        let AnimatedImageFrames {
            first_frame,
            remaining_frames,
            is_complete,
        } = initial_frames(&self, config.max_resident_frames, config.max_resident_bytes)?;

        let image = if !is_complete {
            let image = RenderImage::streaming(self, first_frame, remaining_frames, config);
            if let RenderImageStorage::Streaming(state) = &image.storage {
                state.ensure_stream_task();
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

    /// Produces a renderable image fitted to a device-pixel target.
    pub fn render_sized(
        self,
        target: ImageRenderSize,
        object_fit: ObjectFit,
        config: AnimatedImageConfig,
    ) -> Result<(RenderImage, ImageRenderInfo)> {
        match self.format {
            ImageFormat::Jpeg => jpeg::render_sized(&self.bytes, target, object_fit),
            ImageFormat::Png => png::render_sized(&self.bytes, config, target, object_fit),
            ImageFormat::WebP => match webp::render_sized(&self.bytes, target, object_fit) {
                Ok(Some(image)) => Ok(image),
                Ok(None) | Err(_) => resample::render_sized(self, config, target, object_fit),
            },
            ImageFormat::Bmp => bmp::render_sized(&self.bytes, target, object_fit),
            _ => resample::render_sized(self, config, target, object_fit),
        }
    }
}
