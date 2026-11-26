use core::fmt::Debug;
use std::ops::Sub;

use super::super::{Corner, Half, Point, Size};
use super::Bounds;

impl<T> Bounds<T>
where
    T: Clone + Debug + Default + PartialEq,
{
    /// Creates a new `Bounds` with the specified origin and size.
    ///
    /// # Arguments
    ///
    /// * `origin` - A `Point<T>` representing the origin of the bounds.
    /// * `size` - A `Size<T>` representing the size of the bounds.
    ///
    /// # Returns
    ///
    /// Returns a `Bounds<T>` that has the given origin and size.
    pub fn new(origin: Point<T>, size: Size<T>) -> Self {
        Bounds { origin, size }
    }
}

impl<T> Bounds<T>
where
    T: Sub<Output = T> + Clone + Debug + Default + PartialEq,
{
    /// Constructs a `Bounds` from two corner points: the top left and bottom right corners.
    ///
    /// This function calculates the origin and size of the `Bounds` based on the provided corner points.
    /// The origin is set to the top left corner, and the size is determined by the difference between
    /// the x and y coordinates of the bottom right and top left points.
    ///
    /// # Arguments
    ///
    /// * `top_left` - A `Point<T>` representing the top left corner of the rectangle.
    /// * `bottom_right` - A `Point<T>` representing the bottom right corner of the rectangle.
    ///
    /// # Returns
    ///
    /// Returns a `Bounds<T>` that encompasses the area defined by the two corner points.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::{Bounds, Point};
    /// let top_left = Point { x: 0, y: 0 };
    /// let bottom_right = Point { x: 10, y: 10 };
    /// let bounds = Bounds::from_corners(top_left, bottom_right);
    ///
    /// assert_eq!(bounds.origin, top_left);
    /// assert_eq!(bounds.size.width, 10);
    /// assert_eq!(bounds.size.height, 10);
    /// ```
    pub fn from_corners(top_left: Point<T>, bottom_right: Point<T>) -> Self {
        let origin = Point {
            x: top_left.x.clone(),
            y: top_left.y.clone(),
        };
        let size = Size {
            width: bottom_right.x - top_left.x,
            height: bottom_right.y - top_left.y,
        };
        Bounds { origin, size }
    }

    /// Constructs a `Bounds` from a corner point and size. The specified corner will be placed at
    /// the specified origin.
    pub fn from_corner_and_size(corner: Corner, origin: Point<T>, size: Size<T>) -> Bounds<T> {
        let origin = match corner {
            Corner::TopLeft => origin,
            Corner::TopRight => Point {
                x: origin.x - size.width.clone(),
                y: origin.y,
            },
            Corner::BottomLeft => Point {
                x: origin.x,
                y: origin.y - size.height.clone(),
            },
            Corner::BottomRight => Point {
                x: origin.x - size.width.clone(),
                y: origin.y - size.height.clone(),
            },
        };

        Bounds { origin, size }
    }
}

impl<T> Bounds<T>
where
    T: Sub<T, Output = T> + Half + Clone + Debug + Default + PartialEq,
{
    /// Creates a new bounds centered at the given point.
    pub fn centered_at(center: Point<T>, size: Size<T>) -> Self {
        let origin = Point {
            x: center.x - size.width.half(),
            y: center.y - size.height.half(),
        };
        Self::new(origin, size)
    }
}
