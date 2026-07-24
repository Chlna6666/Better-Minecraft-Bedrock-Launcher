use core::fmt::Debug;
use refineable::Refineable;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    cmp,
    ops::{Div, Mul, MulAssign},
};

use super::{AbsoluteLength, Axis, Pixels, ScaledPixels, Size};

/// Identifies a corner of a 2d box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corner {
    /// The top left corner
    TopLeft,
    /// The top right corner
    TopRight,
    /// The bottom left corner
    BottomLeft,
    /// The bottom right corner
    BottomRight,
}

impl Corner {
    /// Returns the directly opposite corner.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::Corner;
    /// assert_eq!(Corner::TopLeft.opposite_corner(), Corner::BottomRight);
    /// ```
    #[must_use]
    pub fn opposite_corner(self) -> Self {
        match self {
            Corner::TopLeft => Corner::BottomRight,
            Corner::TopRight => Corner::BottomLeft,
            Corner::BottomLeft => Corner::TopRight,
            Corner::BottomRight => Corner::TopLeft,
        }
    }

    /// Returns the corner across from this corner, moving along the specified axis.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::{Axis, Corner};
    /// let result = Corner::TopLeft.other_side_corner_along(Axis::Horizontal);
    /// assert_eq!(result, Corner::TopRight);
    /// ```
    #[must_use]
    pub fn other_side_corner_along(self, axis: Axis) -> Self {
        match axis {
            Axis::Vertical => match self {
                Corner::TopLeft => Corner::BottomLeft,
                Corner::TopRight => Corner::BottomRight,
                Corner::BottomLeft => Corner::TopLeft,
                Corner::BottomRight => Corner::TopRight,
            },
            Axis::Horizontal => match self {
                Corner::TopLeft => Corner::TopRight,
                Corner::TopRight => Corner::TopLeft,
                Corner::BottomLeft => Corner::BottomRight,
                Corner::BottomRight => Corner::BottomLeft,
            },
        }
    }
}

/// Represents the corners of a box in a 2D space, such as border radius.
///
/// Each field represents the size of the corner on one side of the box: `top_left`, `top_right`, `bottom_right`, and `bottom_left`.
#[derive(Refineable, Clone, Default, Debug, Eq, PartialEq)]
#[refineable(Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[repr(C)]
pub struct Corners<T: Clone + Debug + Default + PartialEq> {
    /// The value associated with the top left corner.
    pub top_left: T,
    /// The value associated with the top right corner.
    pub top_right: T,
    /// The value associated with the bottom right corner.
    pub bottom_right: T,
    /// The value associated with the bottom left corner.
    pub bottom_left: T,
}

impl<T> Corners<T>
where
    T: Clone + Debug + Default + PartialEq,
{
    /// Constructs `Corners` where all sides are set to the same specified value.
    ///
    /// This function creates a `Corners` instance with the `top_left`, `top_right`, `bottom_right`, and `bottom_left` fields all initialized
    /// to the same value provided as an argument. This is useful when you want to have uniform corners around a box,
    /// such as a uniform border radius on a rectangle.
    ///
    /// # Arguments
    ///
    /// * `value` - The value to set for all four corners.
    ///
    /// # Returns
    ///
    /// An `Corners` instance with all corners set to the given value.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::Corners;
    /// let uniform_corners = Corners::all(5.0);
    /// assert_eq!(uniform_corners.top_left, 5.0);
    /// assert_eq!(uniform_corners.top_right, 5.0);
    /// assert_eq!(uniform_corners.bottom_right, 5.0);
    /// assert_eq!(uniform_corners.bottom_left, 5.0);
    /// ```
    pub fn all(value: T) -> Self {
        Self {
            top_left: value.clone(),
            top_right: value.clone(),
            bottom_right: value.clone(),
            bottom_left: value,
        }
    }

    /// Returns the requested corner.
    ///
    /// # Returns
    ///
    /// A `Point<T>` representing the corner requested by the parameter.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::{Corner, Corners};
    /// let corners = Corners {
    ///     top_left: 1,
    ///     top_right: 2,
    ///     bottom_left: 3,
    ///     bottom_right: 4
    /// };
    /// assert_eq!(corners.corner(Corner::BottomLeft), 3);
    /// ```
    #[must_use]
    pub fn corner(&self, corner: Corner) -> T {
        match corner {
            Corner::TopLeft => self.top_left.clone(),
            Corner::TopRight => self.top_right.clone(),
            Corner::BottomLeft => self.bottom_left.clone(),
            Corner::BottomRight => self.bottom_right.clone(),
        }
    }
}

impl Corners<AbsoluteLength> {
    /// Converts the `AbsoluteLength` to `Pixels` based on the provided rem size.
    ///
    /// # Arguments
    ///
    /// * `rem_size` - The size of one REM unit in pixels, used for conversion if the `AbsoluteLength` is in REMs.
    ///
    /// # Returns
    ///
    /// Returns a `Corners<Pixels>` instance with each corner's length converted to pixels.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::{Corners, AbsoluteLength, Pixels, Rems, Size};
    /// let corners = Corners {
    ///     top_left: AbsoluteLength::Pixels(Pixels::from(15.0)),
    ///     top_right: AbsoluteLength::Rems(Rems(1.0)),
    ///     bottom_right: AbsoluteLength::Pixels(Pixels::from(30.0)),
    ///     bottom_left: AbsoluteLength::Rems(Rems(2.0)),
    /// };
    /// let rem_size = Pixels::from(16.0);
    /// let corners_in_pixels = corners.to_pixels(rem_size);
    ///
    /// assert_eq!(corners_in_pixels.top_left, Pixels::from(15.0));
    /// assert_eq!(corners_in_pixels.top_right, Pixels::from(16.0)); // 1 rem converted to pixels
    /// assert_eq!(corners_in_pixels.bottom_right, Pixels::from(30.0));
    /// assert_eq!(corners_in_pixels.bottom_left, Pixels::from(32.0)); // 2 rems converted to pixels
    /// ```
    pub fn to_pixels(self, rem_size: Pixels) -> Corners<Pixels> {
        Corners {
            top_left: self.top_left.to_pixels(rem_size),
            top_right: self.top_right.to_pixels(rem_size),
            bottom_right: self.bottom_right.to_pixels(rem_size),
            bottom_left: self.bottom_left.to_pixels(rem_size),
        }
    }
}

impl Corners<Pixels> {
    /// Scales the `Corners<Pixels>` by a given factor, returning `Corners<ScaledPixels>`.
    ///
    /// This method is typically used for adjusting the corner sizes for different display densities or scaling factors.
    ///
    /// # Arguments
    ///
    /// * `factor` - The scaling factor to apply to each corner.
    ///
    /// # Returns
    ///
    /// Returns a new `Corners<ScaledPixels>` where each corner is the result of scaling the original corner by the given factor.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::{Corners, Pixels, ScaledPixels};
    /// let corners = Corners {
    ///     top_left: Pixels::from(10.0),
    ///     top_right: Pixels::from(20.0),
    ///     bottom_right: Pixels::from(30.0),
    ///     bottom_left: Pixels::from(40.0),
    /// };
    /// let scaled_corners = corners.scale(2.0);
    /// assert_eq!(scaled_corners.top_left, ScaledPixels::from(20.0));
    /// assert_eq!(scaled_corners.top_right, ScaledPixels::from(40.0));
    /// assert_eq!(scaled_corners.bottom_right, ScaledPixels::from(60.0));
    /// assert_eq!(scaled_corners.bottom_left, ScaledPixels::from(80.0));
    /// ```
    #[must_use]
    pub fn scale(&self, factor: f32) -> Corners<ScaledPixels> {
        Corners {
            top_left: self.top_left.scale(factor),
            top_right: self.top_right.scale(factor),
            bottom_right: self.bottom_right.scale(factor),
            bottom_left: self.bottom_left.scale(factor),
        }
    }

    /// Returns the maximum value of any corner.
    ///
    /// # Returns
    ///
    /// The maximum `Pixels` value among all four corners.
    #[must_use]
    pub fn max(&self) -> Pixels {
        self.top_left
            .max(self.top_right)
            .max(self.bottom_right)
            .max(self.bottom_left)
    }
}

impl<T: Div<f32, Output = T> + Ord + Clone + Debug + Default + PartialEq> Corners<T> {
    /// Clamps corner radii to be less than or equal to half the shortest side of a quad.
    ///
    /// # Arguments
    ///
    /// * `size` - The size of the quad which limits the size of the corner radii.
    ///
    /// # Returns
    ///
    /// Corner radii values clamped to fit.
    #[must_use]
    pub fn clamp_radii_for_quad_size(self, size: Size<T>) -> Corners<T> {
        let max = cmp::min(size.width, size.height) / 2.;
        Corners {
            top_left: cmp::min(self.top_left, max.clone()),
            top_right: cmp::min(self.top_right, max.clone()),
            bottom_right: cmp::min(self.bottom_right, max.clone()),
            bottom_left: cmp::min(self.bottom_left, max),
        }
    }
}

impl<T: Clone + Debug + Default + PartialEq> Corners<T> {
    /// Applies a function to each field of the `Corners`, producing a new `Corners<U>`.
    ///
    /// This method allows for converting a `Corners<T>` to a `Corners<U>` by specifying a closure
    /// that defines how to convert between the two types. The closure is applied to each field
    /// (`top_left`, `top_right`, `bottom_right`, `bottom_left`), resulting in new corners of the desired type.
    ///
    /// # Arguments
    ///
    /// * `f` - A closure that takes a reference to a value of type `T` and returns a value of type `U`.
    ///
    /// # Returns
    ///
    /// Returns a new `Corners<U>` with each field mapped by the provided function.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::{Corners, Pixels, Rems};
    /// let corners = Corners {
    ///     top_left: Pixels::from(10.0),
    ///     top_right: Pixels::from(20.0),
    ///     bottom_right: Pixels::from(30.0),
    ///     bottom_left: Pixels::from(40.0),
    /// };
    /// let corners_in_rems = corners.map(|&px| Rems(f32::from(px) / 16.0));
    /// assert_eq!(corners_in_rems, Corners {
    ///     top_left: Rems(0.625),
    ///     top_right: Rems(1.25),
    ///     bottom_right: Rems(1.875),
    ///     bottom_left: Rems(2.5),
    /// });
    /// ```
    #[must_use]
    pub fn map<U>(&self, f: impl Fn(&T) -> U) -> Corners<U>
    where
        U: Clone + Debug + Default + PartialEq,
    {
        Corners {
            top_left: f(&self.top_left),
            top_right: f(&self.top_right),
            bottom_right: f(&self.bottom_right),
            bottom_left: f(&self.bottom_left),
        }
    }
}

impl<T> Mul for Corners<T>
where
    T: Mul<Output = T> + Clone + Debug + Default + PartialEq,
{
    type Output = Self;

    fn mul(self, rhs: Self) -> Self::Output {
        Self {
            top_left: self.top_left.clone() * rhs.top_left,
            top_right: self.top_right.clone() * rhs.top_right,
            bottom_right: self.bottom_right.clone() * rhs.bottom_right,
            bottom_left: self.bottom_left * rhs.bottom_left,
        }
    }
}

impl<T, S> MulAssign<S> for Corners<T>
where
    T: Mul<S, Output = T> + Clone + Debug + Default + PartialEq,
    S: Clone,
{
    fn mul_assign(&mut self, rhs: S) {
        self.top_left = self.top_left.clone() * rhs.clone();
        self.top_right = self.top_right.clone() * rhs.clone();
        self.bottom_right = self.bottom_right.clone() * rhs.clone();
        self.bottom_left = self.bottom_left.clone() * rhs;
    }
}

impl<T> Copy for Corners<T> where T: Copy + Clone + Debug + Default + PartialEq {}

impl From<f32> for Corners<Pixels> {
    fn from(value: f32) -> Self {
        Corners {
            top_left: value.into(),
            top_right: value.into(),
            bottom_right: value.into(),
            bottom_left: value.into(),
        }
    }
}

impl From<Pixels> for Corners<Pixels> {
    fn from(value: Pixels) -> Self {
        Corners {
            top_left: value,
            top_right: value,
            bottom_right: value,
            bottom_left: value,
        }
    }
}
