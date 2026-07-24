use core::fmt::Debug;
use std::ops::{Add, Neg, Sub};

use super::super::{Edges, Half, Point, point, size};
use super::Bounds;

impl<T> Bounds<T>
where
    T: PartialOrd + Add<T, Output = T> + Clone + Debug + Default + PartialEq,
{
    /// Checks if this `Bounds` intersects with another `Bounds`.
    ///
    /// Two `Bounds` instances intersect if they overlap in the 2D space they occupy.
    /// This method checks if there is any overlapping area between the two bounds.
    ///
    /// # Arguments
    ///
    /// * `other` - A reference to another `Bounds` to check for intersection with.
    ///
    /// # Returns
    ///
    /// Returns `true` if there is any intersection between the two bounds, `false` otherwise.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::{Bounds, Point, Size};
    /// let bounds1 = Bounds {
    ///     origin: Point { x: 0, y: 0 },
    ///     size: Size { width: 10, height: 10 },
    /// };
    /// let bounds2 = Bounds {
    ///     origin: Point { x: 5, y: 5 },
    ///     size: Size { width: 10, height: 10 },
    /// };
    /// let bounds3 = Bounds {
    ///     origin: Point { x: 20, y: 20 },
    ///     size: Size { width: 10, height: 10 },
    /// };
    ///
    /// assert_eq!(bounds1.intersects(&bounds2), true); // Overlapping bounds
    /// assert_eq!(bounds1.intersects(&bounds3), false); // Non-overlapping bounds
    /// ```
    pub fn intersects(&self, other: &Bounds<T>) -> bool {
        let my_lower_right = self.bottom_right();
        let their_lower_right = other.bottom_right();

        self.origin.x < their_lower_right.x
            && my_lower_right.x > other.origin.x
            && self.origin.y < their_lower_right.y
            && my_lower_right.y > other.origin.y
    }
}

impl<T> Bounds<T>
where
    T: Add<T, Output = T> + Half + Clone + Debug + Default + PartialEq,
{
    /// Returns the center point of the bounds.
    ///
    /// Calculates the center by taking the origin's x and y coordinates and adding half the width and height
    /// of the bounds, respectively. The center is represented as a `Point<T>` where `T` is the type of the
    /// coordinate system.
    ///
    /// # Returns
    ///
    /// A `Point<T>` representing the center of the bounds.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::{Bounds, Point, Size};
    /// let bounds = Bounds {
    ///     origin: Point { x: 0, y: 0 },
    ///     size: Size { width: 10, height: 20 },
    /// };
    /// let center = bounds.center();
    /// assert_eq!(center, Point { x: 5, y: 10 });
    /// ```
    pub fn center(&self) -> Point<T> {
        Point {
            x: self.origin.x.clone() + self.size.width.clone().half(),
            y: self.origin.y.clone() + self.size.height.clone().half(),
        }
    }
}

impl<T> Bounds<T>
where
    T: Add<T, Output = T> + Clone + Debug + Default + PartialEq,
{
    /// Calculates the half perimeter of a rectangle defined by the bounds.
    ///
    /// The half perimeter is calculated as the sum of the width and the height of the rectangle.
    /// This method is generic over the type `T` which must implement the `Sub` trait to allow
    /// calculation of the width and height from the bounds' origin and size, as well as the `Add` trait
    /// to sum the width and height for the half perimeter.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::{Bounds, Point, Size};
    /// let bounds = Bounds {
    ///     origin: Point { x: 0, y: 0 },
    ///     size: Size { width: 10, height: 20 },
    /// };
    /// let half_perimeter = bounds.half_perimeter();
    /// assert_eq!(half_perimeter, 30);
    /// ```
    pub fn half_perimeter(&self) -> T {
        self.size.width.clone() + self.size.height.clone()
    }
}

impl<T> Bounds<T>
where
    T: Add<T, Output = T> + Sub<Output = T> + Clone + Debug + Default + PartialEq,
{
    /// Dilates the bounds by a specified amount in all directions.
    ///
    /// This method expands the bounds by the given `amount`, increasing the size
    /// and adjusting the origin so that the bounds grow outwards equally in all directions.
    /// The resulting bounds will have its width and height increased by twice the `amount`
    /// (since it grows in both directions), and the origin will be moved by `-amount`
    /// in both the x and y directions.
    ///
    /// # Arguments
    ///
    /// * `amount` - The amount by which to dilate the bounds.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::{Bounds, Point, Size};
    /// let mut bounds = Bounds {
    ///     origin: Point { x: 10, y: 10 },
    ///     size: Size { width: 10, height: 10 },
    /// };
    /// let expanded_bounds = bounds.dilate(5);
    /// assert_eq!(expanded_bounds, Bounds {
    ///     origin: Point { x: 5, y: 5 },
    ///     size: Size { width: 20, height: 20 },
    /// });
    /// ```
    #[must_use]
    pub fn dilate(&self, amount: T) -> Bounds<T> {
        let double_amount = amount.clone() + amount.clone();
        Bounds {
            origin: self.origin.clone() - point(amount.clone(), amount),
            size: self.size.clone() + size(double_amount.clone(), double_amount),
        }
    }

    /// Extends the bounds different amounts in each direction.
    #[must_use]
    pub fn extend(&self, amount: Edges<T>) -> Bounds<T> {
        Bounds {
            origin: self.origin.clone() - point(amount.left.clone(), amount.top.clone()),
            size: self.size.clone()
                + size(
                    amount.left.clone() + amount.right.clone(),
                    amount.top.clone() + amount.bottom,
                ),
        }
    }
}

impl<T> Bounds<T>
where
    T: Add<T, Output = T>
        + Sub<T, Output = T>
        + Neg<Output = T>
        + Clone
        + Debug
        + Default
        + PartialEq,
{
    /// Inset the bounds by a specified amount. Equivalent to `dilate` with the amount negated.
    ///
    /// Note that this may panic if T does not support negative values.
    pub fn inset(&self, amount: T) -> Self {
        self.dilate(-amount)
    }
}

impl<T: PartialOrd + Add<T, Output = T> + Sub<Output = T> + Clone + Debug + Default + PartialEq>
    Bounds<T>
{
    /// Calculates the intersection of two `Bounds` objects.
    ///
    /// This method computes the overlapping region of two `Bounds`. If the bounds do not intersect,
    /// the resulting `Bounds` will have a size with width and height of zero.
    ///
    /// # Arguments
    ///
    /// * `other` - A reference to another `Bounds` to intersect with.
    ///
    /// # Returns
    ///
    /// Returns a `Bounds` representing the intersection area. If there is no intersection,
    /// the returned `Bounds` will have a size with width and height of zero.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::{Bounds, Point, Size};
    /// let bounds1 = Bounds {
    ///     origin: Point { x: 0, y: 0 },
    ///     size: Size { width: 10, height: 10 },
    /// };
    /// let bounds2 = Bounds {
    ///     origin: Point { x: 5, y: 5 },
    ///     size: Size { width: 10, height: 10 },
    /// };
    /// let intersection = bounds1.intersect(&bounds2);
    ///
    /// assert_eq!(intersection, Bounds {
    ///     origin: Point { x: 5, y: 5 },
    ///     size: Size { width: 5, height: 5 },
    /// });
    /// ```
    pub fn intersect(&self, other: &Self) -> Self {
        let upper_left = self.origin.max(&other.origin);
        let bottom_right = self.bottom_right().min(&other.bottom_right());
        Self::from_corners(upper_left, bottom_right)
    }

    /// Computes the union of two `Bounds`.
    ///
    /// This method calculates the smallest `Bounds` that contains both the current `Bounds` and the `other` `Bounds`.
    /// The resulting `Bounds` will have an origin that is the minimum of the origins of the two `Bounds`,
    /// and a size that encompasses the furthest extents of both `Bounds`.
    ///
    /// # Arguments
    ///
    /// * `other` - A reference to another `Bounds` to create a union with.
    ///
    /// # Returns
    ///
    /// Returns a `Bounds` representing the union of the two `Bounds`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::{Bounds, Point, Size};
    /// let bounds1 = Bounds {
    ///     origin: Point { x: 0, y: 0 },
    ///     size: Size { width: 10, height: 10 },
    /// };
    /// let bounds2 = Bounds {
    ///     origin: Point { x: 5, y: 5 },
    ///     size: Size { width: 15, height: 15 },
    /// };
    /// let union_bounds = bounds1.union(&bounds2);
    ///
    /// assert_eq!(union_bounds, Bounds {
    ///     origin: Point { x: 0, y: 0 },
    ///     size: Size { width: 20, height: 20 },
    /// });
    /// ```
    pub fn union(&self, other: &Self) -> Self {
        let top_left = self.origin.min(&other.origin);
        let bottom_right = self.bottom_right().max(&other.bottom_right());
        Bounds::from_corners(top_left, bottom_right)
    }
}

impl<T> Bounds<T>
where
    T: Add<T, Output = T> + Sub<T, Output = T> + Clone + Debug + Default + PartialEq,
{
    /// Computes the space available within outer bounds.
    pub fn space_within(&self, outer: &Self) -> Edges<T> {
        Edges {
            top: self.top() - outer.top(),
            right: outer.right() - self.right(),
            bottom: outer.bottom() - self.bottom(),
            left: self.left() - outer.left(),
        }
    }
}
