use core::fmt::Debug;
use refineable::Refineable;
use serde::{Deserialize, Serialize};
use std::hash::Hash;

use super::super::{Point, Size};

/// Represents a rectangular area in a 2D space with an origin point and a size.
///
/// The `Bounds` struct is generic over a type `T` which represents the type of the coordinate system.
/// The origin is represented as a `Point<T>` which defines the top left corner of the rectangle,
/// and the size is represented as a `Size<T>` which defines the width and height of the rectangle.
///
/// # Examples
///
/// ```
/// # use gpui::{Bounds, Point, Size};
/// let origin = Point { x: 0, y: 0 };
/// let size = Size { width: 10, height: 20 };
/// let bounds = Bounds::new(origin, size);
///
/// assert_eq!(bounds.origin, origin);
/// assert_eq!(bounds.size, size);
/// ```
#[derive(Refineable, Clone, Default, Debug, Eq, PartialEq, Serialize, Deserialize, Hash)]
#[refineable(Debug)]
#[repr(C)]
pub struct Bounds<T: Clone + Debug + Default + PartialEq> {
    /// The origin point of this area.
    pub origin: Point<T>,
    /// The size of the rectangle.
    pub size: Size<T>,
}

/// Create a bounds with the given origin and size
pub fn bounds<T: Clone + Debug + Default + PartialEq>(
    origin: Point<T>,
    size: Size<T>,
) -> Bounds<T> {
    Bounds { origin, size }
}

impl<T: Copy + Clone + Debug + Default + PartialEq> Copy for Bounds<T> {}
