use core::fmt::Debug;
use derive_more::Div;
use refineable::Refineable;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    fmt::{self, Display},
    hash::Hash,
    ops::{Add, Mul, MulAssign, Sub},
};

use super::{
    AbsoluteLength, Along, Axis, DefiniteLength, Half, Length, Pixels, Point, ScaledPixels,
    relative,
};

/// A structure representing a two-dimensional size with width and height in a given unit.
///
/// This struct is generic over the type `T`, which can be any type that implements `Clone`, `Default`, and `Debug`.
/// It is commonly used to specify dimensions for elements in a UI, such as a window or element.
#[derive(Refineable, Default, Clone, Copy, PartialEq, Div, Hash, Serialize, Deserialize)]
#[refineable(Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[repr(C)]
pub struct Size<T: Clone + Debug + Default + PartialEq> {
    /// The width component of the size.
    pub width: T,
    /// The height component of the size.
    pub height: T,
}

impl<T: Clone + Debug + Default + PartialEq> Size<T> {
    /// Create a new Size, a synonym for [`size`]
    pub fn new(width: T, height: T) -> Self {
        size(width, height)
    }
}

/// Constructs a new `Size<T>` with the provided width and height.
///
/// # Arguments
///
/// * `width` - The width component of the `Size`.
/// * `height` - The height component of the `Size`.
///
/// # Examples
///
/// ```
/// use gpui::size;
/// let my_size = size(10, 20);
/// assert_eq!(my_size.width, 10);
/// assert_eq!(my_size.height, 20);
/// ```
pub const fn size<T>(width: T, height: T) -> Size<T>
where
    T: Clone + Debug + Default + PartialEq,
{
    Size { width, height }
}

impl<T> Size<T>
where
    T: Clone + Debug + Default + PartialEq,
{
    /// Applies a function to the width and height of the size, producing a new `Size<U>`.
    ///
    /// This method allows for converting a `Size<T>` to a `Size<U>` by specifying a closure
    /// that defines how to convert between the two types. The closure is applied to both the `width`
    /// and `height`, resulting in a new size of the desired type.
    ///
    /// # Arguments
    ///
    /// * `f` - A closure that takes a value of type `T` and returns a value of type `U`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::Size;
    /// let my_size = Size { width: 10, height: 20 };
    /// let my_new_size = my_size.map(|dimension| dimension as f32 * 1.5);
    /// assert_eq!(my_new_size, Size { width: 15.0, height: 30.0 });
    /// ```
    pub fn map<U>(&self, f: impl Fn(T) -> U) -> Size<U>
    where
        U: Clone + Debug + Default + PartialEq,
    {
        Size {
            width: f(self.width.clone()),
            height: f(self.height.clone()),
        }
    }
}

impl<T> Size<T>
where
    T: Clone + Debug + Default + PartialEq + Half,
{
    /// Compute the center point of the size.g
    pub fn center(&self) -> Point<T> {
        Point {
            x: self.width.half(),
            y: self.height.half(),
        }
    }
}

impl Size<Pixels> {
    /// Scales the size by a given factor.
    ///
    /// This method multiplies both the width and height by the provided scaling factor,
    /// resulting in a new `Size<ScaledPixels>` that is proportionally larger or smaller
    /// depending on the factor.
    ///
    /// # Arguments
    ///
    /// * `factor` - The scaling factor to apply to the width and height.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::{Size, Pixels, ScaledPixels};
    /// let size = Size { width: Pixels::from(100.0), height: Pixels::from(50.0) };
    /// let scaled_size = size.scale(2.0);
    /// assert_eq!(scaled_size, Size { width: ScaledPixels::from(200.0), height: ScaledPixels::from(100.0) });
    /// ```
    pub fn scale(&self, factor: f32) -> Size<ScaledPixels> {
        Size {
            width: self.width.scale(factor),
            height: self.height.scale(factor),
        }
    }
}

impl<T> Along for Size<T>
where
    T: Clone + Debug + Default + PartialEq,
{
    type Unit = T;

    fn along(&self, axis: Axis) -> T {
        match axis {
            Axis::Horizontal => self.width.clone(),
            Axis::Vertical => self.height.clone(),
        }
    }

    /// Returns the value of this size along the given axis.
    fn apply_along(&self, axis: Axis, f: impl FnOnce(T) -> T) -> Self {
        match axis {
            Axis::Horizontal => Size {
                width: f(self.width.clone()),
                height: self.height.clone(),
            },
            Axis::Vertical => Size {
                width: self.width.clone(),
                height: f(self.height.clone()),
            },
        }
    }
}

impl<T> Size<T>
where
    T: PartialOrd + Clone + Debug + Default + PartialEq,
{
    /// Returns a new `Size` with the maximum width and height from `self` and `other`.
    ///
    /// # Arguments
    ///
    /// * `other` - A reference to another `Size` to compare with `self`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::Size;
    /// let size1 = Size { width: 30, height: 40 };
    /// let size2 = Size { width: 50, height: 20 };
    /// let max_size = size1.max(&size2);
    /// assert_eq!(max_size, Size { width: 50, height: 40 });
    /// ```
    pub fn max(&self, other: &Self) -> Self {
        Size {
            width: if self.width >= other.width {
                self.width.clone()
            } else {
                other.width.clone()
            },
            height: if self.height >= other.height {
                self.height.clone()
            } else {
                other.height.clone()
            },
        }
    }

    /// Returns a new `Size` with the minimum width and height from `self` and `other`.
    ///
    /// # Arguments
    ///
    /// * `other` - A reference to another `Size` to compare with `self`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::Size;
    /// let size1 = Size { width: 30, height: 40 };
    /// let size2 = Size { width: 50, height: 20 };
    /// let min_size = size1.min(&size2);
    /// assert_eq!(min_size, Size { width: 30, height: 20 });
    /// ```
    pub fn min(&self, other: &Self) -> Self {
        Size {
            width: if self.width >= other.width {
                other.width.clone()
            } else {
                self.width.clone()
            },
            height: if self.height >= other.height {
                other.height.clone()
            } else {
                self.height.clone()
            },
        }
    }
}

impl<T> Sub for Size<T>
where
    T: Sub<Output = T> + Clone + Debug + Default + PartialEq,
{
    type Output = Size<T>;

    fn sub(self, rhs: Self) -> Self::Output {
        Size {
            width: self.width - rhs.width,
            height: self.height - rhs.height,
        }
    }
}

impl<T> Add for Size<T>
where
    T: Add<Output = T> + Clone + Debug + Default + PartialEq,
{
    type Output = Size<T>;

    fn add(self, rhs: Self) -> Self::Output {
        Size {
            width: self.width + rhs.width,
            height: self.height + rhs.height,
        }
    }
}

impl<T, Rhs> Mul<Rhs> for Size<T>
where
    T: Mul<Rhs, Output = Rhs> + Clone + Debug + Default + PartialEq,
    Rhs: Clone + Debug + Default + PartialEq,
{
    type Output = Size<Rhs>;

    fn mul(self, rhs: Rhs) -> Self::Output {
        Size {
            width: self.width * rhs.clone(),
            height: self.height * rhs,
        }
    }
}

impl<T, S> MulAssign<S> for Size<T>
where
    T: Mul<S, Output = T> + Clone + Debug + Default + PartialEq,
    S: Clone,
{
    fn mul_assign(&mut self, rhs: S) {
        self.width = self.width.clone() * rhs.clone();
        self.height = self.height.clone() * rhs;
    }
}

impl<T> Eq for Size<T> where T: Eq + Clone + Debug + Default + PartialEq {}

impl<T> Debug for Size<T>
where
    T: Clone + Debug + Default + PartialEq,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Size {{ {:?} × {:?} }}", self.width, self.height)
    }
}

impl<T: Clone + Debug + Default + PartialEq + Display> Display for Size<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} × {}", self.width, self.height)
    }
}

impl<T: Clone + Debug + Default + PartialEq> From<Point<T>> for Size<T> {
    fn from(point: Point<T>) -> Self {
        Self {
            width: point.x,
            height: point.y,
        }
    }
}

impl From<Size<Pixels>> for Size<DefiniteLength> {
    fn from(size: Size<Pixels>) -> Self {
        Size {
            width: size.width.into(),
            height: size.height.into(),
        }
    }
}

impl From<Size<Pixels>> for Size<AbsoluteLength> {
    fn from(size: Size<Pixels>) -> Self {
        Size {
            width: size.width.into(),
            height: size.height.into(),
        }
    }
}

impl Size<Length> {
    /// Returns a `Size` with both width and height set to fill the available space.
    ///
    /// This function creates a `Size` instance where both the width and height are set to `Length::Definite(DefiniteLength::Fraction(1.0))`,
    /// which represents 100% of the available space in both dimensions.
    ///
    /// # Returns
    ///
    /// A `Size<Length>` that will fill the available space when used in a layout.
    pub fn full() -> Self {
        Self {
            width: relative(1.).into(),
            height: relative(1.).into(),
        }
    }
}

impl Size<Length> {
    /// Returns a `Size` with both width and height set to `auto`, which allows the layout engine to determine the size.
    ///
    /// This function creates a `Size` instance where both the width and height are set to `Length::Auto`,
    /// indicating that their size should be computed based on the layout context, such as the content size or
    /// available space.
    ///
    /// # Returns
    ///
    /// A `Size<Length>` with width and height set to `Length::Auto`.
    pub fn auto() -> Self {
        Self {
            width: Length::Auto,
            height: Length::Auto,
        }
    }
}
