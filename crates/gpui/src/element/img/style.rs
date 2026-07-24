use super::super::Stateful;
use super::element::*;
use crate::{AnyElement, ObjectFit};

/// The style of an image element.
pub struct ImageStyle {
    pub(super) grayscale: bool,
    pub(super) object_fit: ObjectFit,
    pub(super) decode_to_bounds: bool,
    pub(super) loading: Option<Box<dyn Fn() -> AnyElement>>,
    pub(super) fallback: Option<Box<dyn Fn() -> AnyElement>>,
}

impl Default for ImageStyle {
    fn default() -> Self {
        Self {
            grayscale: false,
            object_fit: ObjectFit::Contain,
            decode_to_bounds: false,
            loading: None,
            fallback: None,
        }
    }
}

/// Per-image animation playback settings.
///
/// This allows individual image elements to pause animated media or cap playback
/// without changing the application-wide image pipeline defaults.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImageAnimationPolicy {
    /// Whether decoded animations should advance beyond their first frame.
    pub play: bool,
    /// Optional maximum playback rate while the window is active.
    pub max_fps: Option<f32>,
    /// Optional maximum playback rate while the window is inactive.
    pub inactive_max_fps: Option<f32>,
}

impl Default for ImageAnimationPolicy {
    fn default() -> Self {
        Self {
            play: true,
            max_fps: None,
            inactive_max_fps: None,
        }
    }
}

impl ImageAnimationPolicy {
    /// Use the application-wide animation settings.
    pub const fn inherit() -> Self {
        Self {
            play: true,
            max_fps: None,
            inactive_max_fps: None,
        }
    }

    /// Paint the first frame of animated images without requesting animation frames.
    pub const fn paused() -> Self {
        Self {
            play: false,
            max_fps: None,
            inactive_max_fps: None,
        }
    }

    /// Play animated images with an optional active-window frame-rate cap.
    pub const fn playing(max_fps: f32) -> Self {
        Self {
            play: true,
            max_fps: Some(max_fps),
            inactive_max_fps: None,
        }
    }

    pub(super) fn apply_to(
        self,
        mut config: crate::AnimatedImageConfig,
    ) -> crate::AnimatedImageConfig {
        config.play = config.play && self.play;
        if let Some(max_fps) = self.max_fps {
            config.max_fps = max_fps;
        }
        if let Some(inactive_max_fps) = self.inactive_max_fps {
            config.inactive_max_fps = inactive_max_fps;
        }
        config
    }
}

/// Style an image element.
pub trait StyledImage: Sized {
    /// Get a mutable [ImageStyle] from the element.
    fn image_style(&mut self) -> &mut ImageStyle;

    /// Set the image to be displayed in grayscale.
    fn grayscale(mut self, grayscale: bool) -> Self {
        self.image_style().grayscale = grayscale;
        self
    }

    /// Set the object fit for the image.
    fn object_fit(mut self, object_fit: ObjectFit) -> Self {
        self.image_style().object_fit = object_fit;
        self
    }

    /// Set the object fit for the image.
    fn with_fallback(mut self, fallback: impl Fn() -> AnyElement + 'static) -> Self {
        self.image_style().fallback = Some(Box::new(fallback));
        self
    }

    /// Set the object fit for the image.
    fn with_loading(mut self, loading: impl Fn() -> AnyElement + 'static) -> Self {
        self.image_style().loading = Some(Box::new(loading));
        self
    }

    /// Decode resource images for the actual element bounds in device pixels.
    fn decode_to_bounds(mut self) -> Self {
        self.image_style().decode_to_bounds = true;
        self
    }
}

impl StyledImage for Img {
    fn image_style(&mut self) -> &mut ImageStyle {
        &mut self.style
    }
}

impl StyledImage for Stateful<Img> {
    fn image_style(&mut self) -> &mut ImageStyle {
        &mut self.element.style
    }
}
