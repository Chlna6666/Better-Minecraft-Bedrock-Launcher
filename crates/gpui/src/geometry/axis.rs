use serde::{Deserialize, Serialize};

/// Axis in a 2D cartesian space.
#[derive(Copy, Clone, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub enum Axis {
    /// The y axis, or up and down
    Vertical,
    /// The x axis, or left and right
    Horizontal,
}

impl Axis {
    /// Swap this axis to the opposite axis.
    pub fn invert(self) -> Self {
        match self {
            Axis::Vertical => Axis::Horizontal,
            Axis::Horizontal => Axis::Vertical,
        }
    }
}

/// A trait for accessing the given unit along a certain axis.
pub trait Along {
    /// The unit associated with this type
    type Unit;

    /// Returns the unit along the given axis.
    fn along(&self, axis: Axis) -> Self::Unit;

    /// Applies the given function to the unit along the given axis and returns a new value.
    fn apply_along(&self, axis: Axis, f: impl FnOnce(Self::Unit) -> Self::Unit) -> Self;
}
