use core::fmt::Debug;
use refineable::Refineable;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::ops::{Mul, MulAssign};

use super::{AbsoluteLength, DefiniteLength, Length, Pixels, ScaledPixels, Size, px};

/// Represents the edges of a box in a 2D space, such as padding or margin.
///
/// Each field represents the size of the edge on one side of the box: `top`, `right`, `bottom`, and `left`.
///
/// # Examples
///
/// ```
/// # use gpui::Edges;
/// let edges = Edges {
///     top: 10.0,
///     right: 20.0,
///     bottom: 30.0,
///     left: 40.0,
/// };
///
/// assert_eq!(edges.top, 10.0);
/// assert_eq!(edges.right, 20.0);
/// assert_eq!(edges.bottom, 30.0);
/// assert_eq!(edges.left, 40.0);
/// ```
#[derive(Refineable, Clone, Default, Debug, Eq, PartialEq)]
#[refineable(Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[repr(C)]
pub struct Edges<T: Clone + Debug + Default + PartialEq> {
    /// The size of the top edge.
    pub top: T,
    /// The size of the right edge.
    pub right: T,
    /// The size of the bottom edge.
    pub bottom: T,
    /// The size of the left edge.
    pub left: T,
}

impl<T> Mul for Edges<T>
where
    T: Mul<Output = T> + Clone + Debug + Default + PartialEq,
{
    type Output = Self;

    fn mul(self, rhs: Self) -> Self::Output {
        Self {
            top: self.top.clone() * rhs.top,
            right: self.right.clone() * rhs.right,
            bottom: self.bottom.clone() * rhs.bottom,
            left: self.left * rhs.left,
        }
    }
}

impl<T, S> MulAssign<S> for Edges<T>
where
    T: Mul<S, Output = T> + Clone + Debug + Default + PartialEq,
    S: Clone,
{
    fn mul_assign(&mut self, rhs: S) {
        self.top = self.top.clone() * rhs.clone();
        self.right = self.right.clone() * rhs.clone();
        self.bottom = self.bottom.clone() * rhs.clone();
        self.left = self.left.clone() * rhs;
    }
}

impl<T: Clone + Debug + Default + PartialEq + Copy> Copy for Edges<T> {}

impl<T: Clone + Debug + Default + PartialEq> Edges<T> {
    /// Constructs `Edges` where all sides are set to the same specified value.
    ///
    /// This function creates an `Edges` instance with the `top`, `right`, `bottom`, and `left` fields all initialized
    /// to the same value provided as an argument. This is useful when you want to have uniform edges around a box,
    /// such as padding or margin with the same size on all sides.
    ///
    /// # Arguments
    ///
    /// * `value` - The value to set for all four sides of the edges.
    ///
    /// # Returns
    ///
    /// An `Edges` instance with all sides set to the given value.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::Edges;
    /// let uniform_edges = Edges::all(10.0);
    /// assert_eq!(uniform_edges.top, 10.0);
    /// assert_eq!(uniform_edges.right, 10.0);
    /// assert_eq!(uniform_edges.bottom, 10.0);
    /// assert_eq!(uniform_edges.left, 10.0);
    /// ```
    pub fn all(value: T) -> Self {
        Self {
            top: value.clone(),
            right: value.clone(),
            bottom: value.clone(),
            left: value,
        }
    }

    /// Applies a function to each field of the `Edges`, producing a new `Edges<U>`.
    ///
    /// This method allows for converting an `Edges<T>` to an `Edges<U>` by specifying a closure
    /// that defines how to convert between the two types. The closure is applied to each field
    /// (`top`, `right`, `bottom`, `left`), resulting in new edges of the desired type.
    ///
    /// # Arguments
    ///
    /// * `f` - A closure that takes a reference to a value of type `T` and returns a value of type `U`.
    ///
    /// # Returns
    ///
    /// Returns a new `Edges<U>` with each field mapped by the provided function.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::Edges;
    /// let edges = Edges { top: 10, right: 20, bottom: 30, left: 40 };
    /// let edges_float = edges.map(|&value| value as f32 * 1.1);
    /// assert_eq!(edges_float, Edges { top: 11.0, right: 22.0, bottom: 33.0, left: 44.0 });
    /// ```
    pub fn map<U>(&self, f: impl Fn(&T) -> U) -> Edges<U>
    where
        U: Clone + Debug + Default + PartialEq,
    {
        Edges {
            top: f(&self.top),
            right: f(&self.right),
            bottom: f(&self.bottom),
            left: f(&self.left),
        }
    }

    /// Checks if any of the edges satisfy a given predicate.
    ///
    /// This method applies a predicate function to each field of the `Edges` and returns `true` if any field satisfies the predicate.
    ///
    /// # Arguments
    ///
    /// * `predicate` - A closure that takes a reference to a value of type `T` and returns a `bool`.
    ///
    /// # Returns
    ///
    /// Returns `true` if the predicate returns `true` for any of the edge values, `false` otherwise.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::Edges;
    /// let edges = Edges {
    ///     top: 10,
    ///     right: 0,
    ///     bottom: 5,
    ///     left: 0,
    /// };
    ///
    /// assert!(edges.any(|value| *value == 0));
    /// assert!(edges.any(|value| *value > 0));
    /// assert!(!edges.any(|value| *value > 10));
    /// ```
    pub fn any<F: Fn(&T) -> bool>(&self, predicate: F) -> bool {
        predicate(&self.top)
            || predicate(&self.right)
            || predicate(&self.bottom)
            || predicate(&self.left)
    }
}

impl Edges<Length> {
    /// Sets the edges of the `Edges` struct to `auto`, which is a special value that allows the layout engine to automatically determine the size of the edges.
    ///
    /// This is typically used in layout contexts where the exact size of the edges is not important, or when the size should be calculated based on the content or container.
    ///
    /// # Returns
    ///
    /// Returns an `Edges<Length>` with all edges set to `Length::Auto`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::{Edges, Length};
    /// let auto_edges = Edges::auto();
    /// assert_eq!(auto_edges.top, Length::Auto);
    /// assert_eq!(auto_edges.right, Length::Auto);
    /// assert_eq!(auto_edges.bottom, Length::Auto);
    /// assert_eq!(auto_edges.left, Length::Auto);
    /// ```
    pub fn auto() -> Self {
        Self {
            top: Length::Auto,
            right: Length::Auto,
            bottom: Length::Auto,
            left: Length::Auto,
        }
    }

    /// Sets the edges of the `Edges` struct to zero, which means no size or thickness.
    ///
    /// This is typically used when you want to specify that a box (like a padding or margin area)
    /// should have no edges, effectively making it non-existent or invisible in layout calculations.
    ///
    /// # Returns
    ///
    /// Returns an `Edges<Length>` with all edges set to zero length.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::{DefiniteLength, Edges, Length, Pixels};
    /// let no_edges = Edges::<Length>::zero();
    /// assert_eq!(no_edges.top, Length::Definite(DefiniteLength::from(Pixels::ZERO)));
    /// assert_eq!(no_edges.right, Length::Definite(DefiniteLength::from(Pixels::ZERO)));
    /// assert_eq!(no_edges.bottom, Length::Definite(DefiniteLength::from(Pixels::ZERO)));
    /// assert_eq!(no_edges.left, Length::Definite(DefiniteLength::from(Pixels::ZERO)));
    /// ```
    pub fn zero() -> Self {
        Self {
            top: px(0.).into(),
            right: px(0.).into(),
            bottom: px(0.).into(),
            left: px(0.).into(),
        }
    }
}

impl Edges<DefiniteLength> {
    /// Sets the edges of the `Edges` struct to zero, which means no size or thickness.
    ///
    /// This is typically used when you want to specify that a box (like a padding or margin area)
    /// should have no edges, effectively making it non-existent or invisible in layout calculations.
    ///
    /// # Returns
    ///
    /// Returns an `Edges<DefiniteLength>` with all edges set to zero length.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::{px, DefiniteLength, Edges};
    /// let no_edges = Edges::<DefiniteLength>::zero();
    /// assert_eq!(no_edges.top, DefiniteLength::from(px(0.)));
    /// assert_eq!(no_edges.right, DefiniteLength::from(px(0.)));
    /// assert_eq!(no_edges.bottom, DefiniteLength::from(px(0.)));
    /// assert_eq!(no_edges.left, DefiniteLength::from(px(0.)));
    /// ```
    pub fn zero() -> Self {
        Self {
            top: px(0.).into(),
            right: px(0.).into(),
            bottom: px(0.).into(),
            left: px(0.).into(),
        }
    }

    /// Converts the `DefiniteLength` to `Pixels` based on the parent size and the REM size.
    ///
    /// This method allows for a `DefiniteLength` value to be converted into pixels, taking into account
    /// the size of the parent element (for percentage-based lengths) and the size of a rem unit (for rem-based lengths).
    ///
    /// # Arguments
    ///
    /// * `parent_size` - `Size<AbsoluteLength>` representing the size of the parent element.
    /// * `rem_size` - `Pixels` representing the size of one REM unit.
    ///
    /// # Returns
    ///
    /// Returns an `Edges<Pixels>` representing the edges with lengths converted to pixels.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::{Edges, DefiniteLength, px, AbsoluteLength, rems, Size};
    /// let edges = Edges {
    ///     top: DefiniteLength::Absolute(AbsoluteLength::Pixels(px(10.0))),
    ///     right: DefiniteLength::Fraction(0.5),
    ///     bottom: DefiniteLength::Absolute(AbsoluteLength::Rems(rems(2.0))),
    ///     left: DefiniteLength::Fraction(0.25),
    /// };
    /// let parent_size = Size {
    ///     width: AbsoluteLength::Pixels(px(200.0)),
    ///     height: AbsoluteLength::Pixels(px(100.0)),
    /// };
    /// let rem_size = px(16.0);
    /// let edges_in_pixels = edges.to_pixels(parent_size, rem_size);
    ///
    /// assert_eq!(edges_in_pixels.top, px(10.0)); // Absolute length in pixels
    /// assert_eq!(edges_in_pixels.right, px(100.0)); // 50% of parent width
    /// assert_eq!(edges_in_pixels.bottom, px(32.0)); // 2 rems
    /// assert_eq!(edges_in_pixels.left, px(50.0)); // 25% of parent width
    /// ```
    pub fn to_pixels(self, parent_size: Size<AbsoluteLength>, rem_size: Pixels) -> Edges<Pixels> {
        Edges {
            top: self.top.to_pixels(parent_size.height, rem_size),
            right: self.right.to_pixels(parent_size.width, rem_size),
            bottom: self.bottom.to_pixels(parent_size.height, rem_size),
            left: self.left.to_pixels(parent_size.width, rem_size),
        }
    }
}

impl Edges<AbsoluteLength> {
    /// Sets the edges of the `Edges` struct to zero, which means no size or thickness.
    ///
    /// This is typically used when you want to specify that a box (like a padding or margin area)
    /// should have no edges, effectively making it non-existent or invisible in layout calculations.
    ///
    /// # Returns
    ///
    /// Returns an `Edges<AbsoluteLength>` with all edges set to zero length.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::{AbsoluteLength, Edges, Pixels};
    /// let no_edges = Edges::<AbsoluteLength>::zero();
    /// assert_eq!(no_edges.top, AbsoluteLength::Pixels(Pixels::ZERO));
    /// assert_eq!(no_edges.right, AbsoluteLength::Pixels(Pixels::ZERO));
    /// assert_eq!(no_edges.bottom, AbsoluteLength::Pixels(Pixels::ZERO));
    /// assert_eq!(no_edges.left, AbsoluteLength::Pixels(Pixels::ZERO));
    /// ```
    pub fn zero() -> Self {
        Self {
            top: px(0.).into(),
            right: px(0.).into(),
            bottom: px(0.).into(),
            left: px(0.).into(),
        }
    }

    /// Converts the `AbsoluteLength` to `Pixels` based on the `rem_size`.
    ///
    /// If the `AbsoluteLength` is already in pixels, it simply returns the corresponding `Pixels` value.
    /// If the `AbsoluteLength` is in rems, it multiplies the number of rems by the `rem_size` to convert it to pixels.
    ///
    /// # Arguments
    ///
    /// * `rem_size` - The size of one rem unit in pixels.
    ///
    /// # Returns
    ///
    /// Returns an `Edges<Pixels>` representing the edges with lengths converted to pixels.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::{Edges, AbsoluteLength, Pixels, px, rems};
    /// let edges = Edges {
    ///     top: AbsoluteLength::Pixels(px(10.0)),
    ///     right: AbsoluteLength::Rems(rems(1.0)),
    ///     bottom: AbsoluteLength::Pixels(px(20.0)),
    ///     left: AbsoluteLength::Rems(rems(2.0)),
    /// };
    /// let rem_size = px(16.0);
    /// let edges_in_pixels = edges.to_pixels(rem_size);
    ///
    /// assert_eq!(edges_in_pixels.top, px(10.0)); // Already in pixels
    /// assert_eq!(edges_in_pixels.right, px(16.0)); // 1 rem converted to pixels
    /// assert_eq!(edges_in_pixels.bottom, px(20.0)); // Already in pixels
    /// assert_eq!(edges_in_pixels.left, px(32.0)); // 2 rems converted to pixels
    /// ```
    pub fn to_pixels(self, rem_size: Pixels) -> Edges<Pixels> {
        Edges {
            top: self.top.to_pixels(rem_size),
            right: self.right.to_pixels(rem_size),
            bottom: self.bottom.to_pixels(rem_size),
            left: self.left.to_pixels(rem_size),
        }
    }
}

impl Edges<Pixels> {
    /// Scales the `Edges<Pixels>` by a given factor, returning `Edges<ScaledPixels>`.
    ///
    /// This method is typically used for adjusting the edge sizes for different display densities or scaling factors.
    ///
    /// # Arguments
    ///
    /// * `factor` - The scaling factor to apply to each edge.
    ///
    /// # Returns
    ///
    /// Returns a new `Edges<ScaledPixels>` where each edge is the result of scaling the original edge by the given factor.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::{Edges, Pixels, ScaledPixels};
    /// let edges = Edges {
    ///     top: Pixels::from(10.0),
    ///     right: Pixels::from(20.0),
    ///     bottom: Pixels::from(30.0),
    ///     left: Pixels::from(40.0),
    /// };
    /// let scaled_edges = edges.scale(2.0);
    /// assert_eq!(scaled_edges.top, ScaledPixels::from(20.0));
    /// assert_eq!(scaled_edges.right, ScaledPixels::from(40.0));
    /// assert_eq!(scaled_edges.bottom, ScaledPixels::from(60.0));
    /// assert_eq!(scaled_edges.left, ScaledPixels::from(80.0));
    /// ```
    pub fn scale(&self, factor: f32) -> Edges<ScaledPixels> {
        Edges {
            top: self.top.scale(factor),
            right: self.right.scale(factor),
            bottom: self.bottom.scale(factor),
            left: self.left.scale(factor),
        }
    }

    /// Returns the maximum value of any edge.
    ///
    /// # Returns
    ///
    /// The maximum `Pixels` value among all four edges.
    pub fn max(&self) -> Pixels {
        self.top.max(self.right).max(self.bottom).max(self.left)
    }
}

impl From<f32> for Edges<Pixels> {
    fn from(value: f32) -> Self {
        let value: Pixels = value.into();
        value.into()
    }
}

impl From<Pixels> for Edges<Pixels> {
    fn from(value: Pixels) -> Self {
        Edges {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }
}
