use crate::{DevicePixels, Size, size};
use image::{Delay, Frame, RgbaImage};
use std::{sync::Arc, time::Duration};

use super::super::types::RenderImagePixelFormat;

#[derive(Clone)]
pub(crate) struct AnimatedFrame {
    pub(in crate::assets) sequence: usize,
    pub(in crate::assets) size: Size<DevicePixels>,
    pub(in crate::assets) delay: Delay,
    pub(in crate::assets) bytes: Arc<[u8]>,
    pub(in crate::assets) pixel_format: RenderImagePixelFormat,
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

    pub(crate) fn from_raw_pixel_bytes(
        sequence: usize,
        size: Size<DevicePixels>,
        pixel_format: RenderImagePixelFormat,
        bytes: impl Into<Arc<[u8]>>,
    ) -> Self {
        Self {
            sequence,
            size,
            delay: Delay::from_saturating_duration(Duration::ZERO),
            bytes: bytes.into(),
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

    pub(in crate::assets) fn byte_len(&self) -> usize {
        self.bytes.len()
    }
}
