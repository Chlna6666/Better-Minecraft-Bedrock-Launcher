use super::super::Stateful;
use super::element::*;
use crate::{AnyElement, Bounds, DevicePixels, ObjectFit, Pixels, Point, Size};

/// The style of an image element.
pub struct ImageStyle {
    pub(super) grayscale: bool,
    pub(super) object_fit: ObjectFit,
    pub(super) object_position: Option<Point<f32>>,
    pub(super) render_to_bounds: bool,
    pub(super) loading: Option<Box<dyn Fn() -> AnyElement>>,
    pub(super) fallback: Option<Box<dyn Fn() -> AnyElement>>,
}

impl Default for ImageStyle {
    fn default() -> Self {
        Self {
            grayscale: false,
            object_fit: ObjectFit::Contain,
            object_position: None,
            render_to_bounds: false,
            loading: None,
            fallback: None,
        }
    }
}

impl ImageStyle {
    pub(super) fn bounds(
        &self,
        bounds: Bounds<Pixels>,
        image_size: Size<DevicePixels>,
    ) -> Bounds<Pixels> {
        let mut fitted = self.object_fit.bounds(bounds, image_size);
        if let Some(position) = self.object_position {
            fitted.origin.x =
                bounds.origin.x + (bounds.size.width - fitted.size.width) * position.x;
            fitted.origin.y =
                bounds.origin.y + (bounds.size.height - fitted.size.height) * position.y;
        }
        fitted
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

    /// Align the fitted image within its element without changing layout or decoding.
    ///
    /// Each coordinate positions the unused (or cropped) space: 0 aligns to the
    /// leading edge, 0.5 centers, and 1 aligns to the trailing edge. For example,
    /// `point(0.5, 1.0)` keeps the bottom of a Cover image visible. When unset,
    /// the selected [`ObjectFit`] keeps its default alignment.
    fn object_position(mut self, position: Point<f32>) -> Self {
        self.image_style().object_position = Some(position);
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
    ///
    /// This explicitly enables bounds-aware decoding when the application uses
    /// [`ImageBoundsPolicy::Explicit`](crate::ImageBoundsPolicy::Explicit). Applications using
    /// [`ImageBoundsPolicy::Visible`](crate::ImageBoundsPolicy::Visible) get this behavior by
    /// default for resource images.
    fn render_to_bounds(mut self) -> Self {
        self.image_style().render_to_bounds = true;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{point, px, size};

    #[test]
    fn bottom_aligned_cover_preserves_bottom_edge_and_aspect_ratio() {
        let image_size = size(DevicePixels(3840), DevicePixels(2160));
        let style = ImageStyle {
            object_fit: ObjectFit::Cover,
            object_position: Some(point(0.5, 1.0)),
            ..ImageStyle::default()
        };
        for viewport in [size(px(1530.0), px(568.0)), size(px(600.0), px(900.0))] {
            let bounds = Bounds::new(point(px(10.0), px(60.0)), viewport);
            let fitted = style.bounds(bounds, image_size);
            assert!((fitted.bottom() - bounds.bottom()).0.abs() < 0.001);
            assert!((fitted.center().x - bounds.center().x).0.abs() < 0.001);
            assert!((fitted.size.width / fitted.size.height - 16.0 / 9.0).abs() < 0.001);
            assert!(fitted.size.width >= viewport.width);
            assert!(fitted.size.height >= viewport.height);
        }
    }
}
