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

/// Compressed image bytes shared by resource loading and image decoding.
///
/// Static assets keep their original storage; file and network payloads share owned storage.
/// Cloning this value never copies the compressed payload.
#[derive(Clone)]
pub enum CompressedImageBytes {
    /// Statically embedded image bytes borrowed directly from the asset source.
    Static(&'static [u8]),
    /// Shared owned bytes retained for file or network-backed image resources.
    Shared(Arc<[u8]>),
}

impl CompressedImageBytes {
    /// Borrows the compressed payload without copying it.
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Static(bytes) => bytes,
            Self::Shared(bytes) => bytes.as_ref(),
        }
    }

    /// Returns the compressed payload size, including statically borrowed bytes.
    pub fn len(&self) -> usize {
        self.as_bytes().len()
    }

    /// Returns whether the compressed payload is empty.
    pub fn is_empty(&self) -> bool {
        self.as_bytes().is_empty()
    }
}

impl AsRef<[u8]> for CompressedImageBytes {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl From<Arc<[u8]>> for CompressedImageBytes {
    fn from(bytes: Arc<[u8]>) -> Self {
        Self::Shared(bytes)
    }
}

impl From<Vec<u8>> for CompressedImageBytes {
    fn from(bytes: Vec<u8>) -> Self {
        Self::Shared(bytes.into())
    }
}

impl From<&'static [u8]> for CompressedImageBytes {
    fn from(bytes: &'static [u8]) -> Self {
        Self::Static(bytes)
    }
}

/// Encoded raster image bytes together with their container format.
#[derive(Clone)]
pub struct EncodedImage {
    pub(in crate::assets) bytes: CompressedImageBytes,
    pub(in crate::assets) format: ImageFormat,
}

impl EncodedImage {
    /// Creates an encoded image source, preserving static or shared compressed storage.
    ///
    /// Static slices and existing shared buffers are not copied. Owned vectors are converted
    /// into shared storage once, so subsequent loads and animation workers reuse the payload.
    pub fn new(format: ImageFormat, bytes: impl Into<CompressedImageBytes>) -> Self {
        Self {
            bytes: bytes.into(),
            format,
        }
    }

    /// Produces a renderable image, retaining animation frames according to `config`.
    ///
    /// Animations that exceed the resident frame count continue through GPUI's bounded animation
    /// worker pool; callers do not provide or own a background executor.
    pub fn render(self, config: AnimatedImageConfig) -> Result<RenderImage> {
        let config = config.clamped();
        let AnimatedImageFrames {
            first_frame,
            remaining_frames,
            is_complete,
        } = initial_frames(&self, config.max_resident_frames)?;

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
            ImageFormat::Jpeg => jpeg::render_sized(self.bytes.as_bytes(), target, object_fit),
            ImageFormat::Png => png::render_sized(self, config, target, object_fit),
            ImageFormat::WebP => {
                match webp::render_sized(self.bytes.as_bytes(), target, object_fit) {
                    Ok(Some(image)) => Ok(image),
                    Ok(None) | Err(_) => resample::render_sized(self, config, target, object_fit),
                }
            }
            ImageFormat::Bmp => bmp::render_sized(self.bytes.as_bytes(), target, object_fit),
            _ => resample::render_sized(self, config, target, object_fit),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoded_image_preserves_static_payload_storage() {
        static BYTES: &[u8] = &[1, 2, 3, 4];
        let source = EncodedImage::new(ImageFormat::Png, BYTES);
        assert_eq!(source.bytes.as_bytes().as_ptr(), BYTES.as_ptr());
        assert_eq!(source.clone().bytes.as_bytes().as_ptr(), BYTES.as_ptr());
    }

    #[test]
    fn encoded_image_preserves_shared_payload_storage() {
        let bytes: Arc<[u8]> = Arc::from(vec![1, 2, 3, 4]);
        let source = EncodedImage::new(ImageFormat::Png, bytes.clone());
        assert_eq!(source.bytes.as_bytes().as_ptr(), bytes.as_ptr());
        assert_eq!(source.clone().bytes.as_bytes().as_ptr(), bytes.as_ptr());
    }
}
