use crate::{Bounds, Corners, Pixels, ScaledPixels};
use std::fmt::Debug;

/// Indicates which region of the window is visible. Content falling outside of this mask will not be
/// rendered. Corner radii describe an axis-aligned rounded rectangle.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct ContentMask<P: Clone + Debug + Default + PartialEq> {
    /// The bounds
    pub bounds: Bounds<P>,
    /// Bounds associated with `corner_radii`.
    pub corner_bounds: Bounds<P>,
    /// The corner radii of the visible region.
    pub corner_radii: Corners<P>,
}

impl<P: Clone + Debug + Default + PartialEq> ContentMask<P> {
    /// Creates a rectangular content mask.
    pub fn new(bounds: Bounds<P>) -> Self {
        Self {
            bounds,
            corner_bounds: Bounds::default(),
            corner_radii: Corners::default(),
        }
    }
}

impl ContentMask<Pixels> {
    /// Scale the content mask's pixel units by the given scaling factor.
    pub fn scale(&self, factor: f32) -> ContentMask<ScaledPixels> {
        ContentMask {
            bounds: self.bounds.scale(factor),
            corner_bounds: self.corner_bounds.scale(factor),
            corner_radii: self.corner_radii.scale(factor),
        }
    }

    /// Intersect the content mask with the given content mask.
    pub fn intersect(&self, other: &Self) -> Self {
        let bounds = self.bounds.intersect(&other.bounds);
        let (corner_bounds, corner_radii) = if self.corner_radii == Corners::default() {
            (other.corner_bounds, other.corner_radii)
        } else if other.corner_radii == Corners::default() || bounds == self.bounds {
            (self.corner_bounds, self.corner_radii)
        } else {
            (other.corner_bounds, other.corner_radii)
        };
        ContentMask {
            bounds,
            corner_bounds,
            corner_radii,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bounds, point, px, size};

    #[test]
    fn scale_preserves_rounded_clip_geometry() {
        let mask = ContentMask {
            bounds: bounds(point(px(2.0), px(3.0)), size(px(20.0), px(10.0))),
            corner_bounds: bounds(point(px(2.0), px(3.0)), size(px(20.0), px(10.0))),
            corner_radii: Corners::from(px(4.0)),
        };

        let scaled = mask.scale(1.5);

        assert_eq!(scaled.bounds.origin.x, ScaledPixels(3.0));
        assert_eq!(scaled.bounds.origin.y, ScaledPixels(4.5));
        assert_eq!(scaled.corner_bounds.origin.x, ScaledPixels(3.0));
        assert_eq!(scaled.corner_radii.top_left, ScaledPixels(6.0));
        assert_eq!(scaled.corner_radii.bottom_right, ScaledPixels(6.0));
    }

    #[test]
    fn intersection_keeps_radii_of_tighter_mask() {
        let outer = ContentMask {
            bounds: bounds(point(px(0.0), px(0.0)), size(px(100.0), px(100.0))),
            corner_bounds: bounds(point(px(0.0), px(0.0)), size(px(100.0), px(100.0))),
            corner_radii: Corners::from(px(12.0)),
        };
        let inner = ContentMask {
            bounds: bounds(point(px(20.0), px(20.0)), size(px(40.0), px(40.0))),
            corner_bounds: bounds(point(px(20.0), px(20.0)), size(px(40.0), px(40.0))),
            corner_radii: Corners::from(px(8.0)),
        };

        assert_eq!(outer.intersect(&inner), inner);
    }
}
