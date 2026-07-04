use core::fmt::Debug;
use derive_more::{Add, AddAssign, Sub, SubAssign};
use refineable::Refineable;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    fmt::{self, Display},
    hash::Hash,
    ops::{Div, Mul, MulAssign, Sub as StdSub},
};

use super::{Along, Axis, Negate, Pixels, ScaledPixels};

/// Describes a location in a 2D cartesian space.
///
/// It holds two public fields, `x` and `y`, which represent the coordinates in the space.
/// The type `T` for the coordinates can be any type that implements `Default`, `Clone`, and `Debug`.
///
/// # Examples
///
/// ```
/// # use gpui::Point;
/// let point = Point { x: 10, y: 20 };
/// println!("{:?}", point); // Outputs: Point { x: 10, y: 20 }
/// ```
#[derive(
    Refineable,
    Default,
    Add,
    AddAssign,
    Sub,
    SubAssign,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    JsonSchema,
    Hash,
)]
#[refineable(Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[repr(C)]
pub struct Point<T: Clone + Debug + Default + PartialEq> {
    /// The x coordinate of the point.
    pub x: T,
    /// The y coordinate of the point.
    pub y: T,
}

/// Constructs a new `Point<T>` with the given x and y coordinates.
///
/// # Arguments
///
/// * `x` - The x coordinate of the point.
/// * `y` - The y coordinate of the point.
///
/// # Returns
///
/// Returns a `Point<T>` with the specified coordinates.
///
/// # Examples
///
/// ```
/// use gpui::point;
/// let p = point(10, 20);
/// assert_eq!(p.x, 10);
/// assert_eq!(p.y, 20);
/// ```
pub const fn point<T: Clone + Debug + Default + PartialEq>(x: T, y: T) -> Point<T> {
    Point { x, y }
}

impl<T: Clone + Debug + Default + PartialEq> Point<T> {
    /// Creates a new `Point` with the specified `x` and `y` coordinates.
    ///
    /// # Arguments
    ///
    /// * `x` - The horizontal coordinate of the point.
    /// * `y` - The vertical coordinate of the point.
    ///
    /// # Examples
    ///
    /// ```
    /// use gpui::Point;
    /// let p = Point::new(10, 20);
    /// assert_eq!(p.x, 10);
    /// assert_eq!(p.y, 20);
    /// ```
    pub const fn new(x: T, y: T) -> Self {
        Self { x, y }
    }

    /// Transforms the point to a `Point<U>` by applying the given function to both coordinates.
    ///
    /// This method allows for converting a `Point<T>` to a `Point<U>` by specifying a closure
    /// that defines how to convert between the two types. The closure is applied to both the `x`
    /// and `y` coordinates, resulting in a new point of the desired type.
    ///
    /// # Arguments
    ///
    /// * `f` - A closure that takes a value of type `T` and returns a value of type `U`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::Point;
    /// let p = Point { x: 3, y: 4 };
    /// let p_float = p.map(|coord| coord as f32);
    /// assert_eq!(p_float, Point { x: 3.0, y: 4.0 });
    /// ```
    #[must_use]
    pub fn map<U: Clone + Debug + Default + PartialEq>(&self, f: impl Fn(T) -> U) -> Point<U> {
        Point {
            x: f(self.x.clone()),
            y: f(self.y.clone()),
        }
    }
}

impl<T: Clone + Debug + Default + PartialEq> Along for Point<T> {
    type Unit = T;

    fn along(&self, axis: Axis) -> T {
        match axis {
            Axis::Horizontal => self.x.clone(),
            Axis::Vertical => self.y.clone(),
        }
    }

    fn apply_along(&self, axis: Axis, f: impl FnOnce(T) -> T) -> Point<T> {
        match axis {
            Axis::Horizontal => Point {
                x: f(self.x.clone()),
                y: self.y.clone(),
            },
            Axis::Vertical => Point {
                x: self.x.clone(),
                y: f(self.y.clone()),
            },
        }
    }
}

impl<T: Clone + Debug + Default + PartialEq + Negate> Negate for Point<T> {
    fn negate(self) -> Self {
        self.map(Negate::negate)
    }
}

impl Point<Pixels> {
    /// Scales the point by a given factor, which is typically derived from the resolution
    /// of a target display to ensure proper sizing of UI elements.
    ///
    /// # Arguments
    ///
    /// * `factor` - The scaling factor to apply to both the x and y coordinates.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::{Point, Pixels, ScaledPixels};
    /// let p = Point { x: Pixels::from(10.0), y: Pixels::from(20.0) };
    /// let scaled_p = p.scale(1.5);
    /// assert_eq!(scaled_p, Point { x: ScaledPixels::from(15.0), y: ScaledPixels::from(30.0) });
    /// ```
    pub fn scale(&self, factor: f32) -> Point<ScaledPixels> {
        Point {
            x: self.x.scale(factor),
            y: self.y.scale(factor),
        }
    }

    /// Calculates the Euclidean distance from the origin (0, 0) to this point.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::{Pixels, Point};
    /// let p = Point { x: Pixels::from(3.0), y: Pixels::from(4.0) };
    /// assert_eq!(p.magnitude(), 5.0);
    /// ```
    pub fn magnitude(&self) -> f64 {
        ((self.x.0.powi(2) + self.y.0.powi(2)) as f64).sqrt()
    }
}

impl<T> Point<T>
where
    T: StdSub<T, Output = T> + Clone + Debug + Default + PartialEq,
{
    /// Get the position of this point, relative to the given origin
    pub fn relative_to(&self, origin: &Point<T>) -> Point<T> {
        point(
            self.x.clone() - origin.x.clone(),
            self.y.clone() - origin.y.clone(),
        )
    }
}

impl<T, Rhs> Mul<Rhs> for Point<T>
where
    T: Mul<Rhs, Output = T> + Clone + Debug + Default + PartialEq,
    Rhs: Clone + Debug,
{
    type Output = Point<T>;

    fn mul(self, rhs: Rhs) -> Self::Output {
        Point {
            x: self.x * rhs.clone(),
            y: self.y * rhs,
        }
    }
}

impl<T, S> MulAssign<S> for Point<T>
where
    T: Mul<S, Output = T> + Clone + Debug + Default + PartialEq,
    S: Clone,
{
    fn mul_assign(&mut self, rhs: S) {
        self.x = self.x.clone() * rhs.clone();
        self.y = self.y.clone() * rhs;
    }
}

impl<T, S> Div<S> for Point<T>
where
    T: Div<S, Output = T> + Clone + Debug + Default + PartialEq,
    S: Clone,
{
    type Output = Self;

    fn div(self, rhs: S) -> Self::Output {
        Self {
            x: self.x / rhs.clone(),
            y: self.y / rhs,
        }
    }
}

impl<T> Point<T>
where
    T: PartialOrd + Clone + Debug + Default + PartialEq,
{
    /// Returns a new point with the maximum values of each dimension from `self` and `other`.
    ///
    /// # Arguments
    ///
    /// * `other` - A reference to another `Point` to compare with `self`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::Point;
    /// let p1 = Point { x: 3, y: 7 };
    /// let p2 = Point { x: 5, y: 2 };
    /// let max_point = p1.max(&p2);
    /// assert_eq!(max_point, Point { x: 5, y: 7 });
    /// ```
    pub fn max(&self, other: &Self) -> Self {
        Point {
            x: if self.x > other.x {
                self.x.clone()
            } else {
                other.x.clone()
            },
            y: if self.y > other.y {
                self.y.clone()
            } else {
                other.y.clone()
            },
        }
    }

    /// Returns a new point with the minimum values of each dimension from `self` and `other`.
    ///
    /// # Arguments
    ///
    /// * `other` - A reference to another `Point` to compare with `self`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::Point;
    /// let p1 = Point { x: 3, y: 7 };
    /// let p2 = Point { x: 5, y: 2 };
    /// let min_point = p1.min(&p2);
    /// assert_eq!(min_point, Point { x: 3, y: 2 });
    /// ```
    pub fn min(&self, other: &Self) -> Self {
        Point {
            x: if self.x <= other.x {
                self.x.clone()
            } else {
                other.x.clone()
            },
            y: if self.y <= other.y {
                self.y.clone()
            } else {
                other.y.clone()
            },
        }
    }

    /// Clamps the point to a specified range.
    ///
    /// Given a minimum point and a maximum point, this method constrains the current point
    /// such that its coordinates do not exceed the range defined by the minimum and maximum points.
    /// If the current point's coordinates are less than the minimum, they are set to the minimum.
    /// If they are greater than the maximum, they are set to the maximum.
    ///
    /// # Arguments
    ///
    /// * `min` - A reference to a `Point` representing the minimum allowable coordinates.
    /// * `max` - A reference to a `Point` representing the maximum allowable coordinates.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::Point;
    /// let p = Point { x: 10, y: 20 };
    /// let min = Point { x: 0, y: 5 };
    /// let max = Point { x: 15, y: 25 };
    /// let clamped_p = p.clamp(&min, &max);
    /// assert_eq!(clamped_p, Point { x: 10, y: 20 });
    ///
    /// let p_out_of_bounds = Point { x: -5, y: 30 };
    /// let clamped_p_out_of_bounds = p_out_of_bounds.clamp(&min, &max);
    /// assert_eq!(clamped_p_out_of_bounds, Point { x: 0, y: 25 });
    /// ```
    pub fn clamp(&self, min: &Self, max: &Self) -> Self {
        self.max(min).min(max)
    }
}

impl<T: Clone + Debug + Default + PartialEq> Clone for Point<T> {
    fn clone(&self) -> Self {
        Self {
            x: self.x.clone(),
            y: self.y.clone(),
        }
    }
}

impl<T: Clone + Debug + Default + PartialEq + Display> Display for Point<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {})", self.x, self.y)
    }
}
