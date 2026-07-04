use core::fmt::Debug;

use super::{
    AbsoluteLength, Bounds, Corners, DefiniteLength, DevicePixels, Length, Pixels, Point, Rems,
    ScaledPixels, Size,
};

/// Provides a trait for types that can calculate half of their value.
///
/// The `Half` trait is used for types that can be evenly divided, returning a new instance of the same type
/// representing half of the original value. This is commonly used for types that represent measurements or sizes,
/// such as lengths or pixels, where halving is a frequent operation during layout calculations or animations.
pub trait Half {
    /// Returns half of the current value.
    ///
    /// # Returns
    ///
    /// A new instance of the implementing type, representing half of the original value.
    fn half(&self) -> Self;
}

impl Half for i32 {
    fn half(&self) -> Self {
        self / 2
    }
}

impl Half for f32 {
    fn half(&self) -> Self {
        self / 2.
    }
}

impl Half for DevicePixels {
    fn half(&self) -> Self {
        Self(self.0 / 2)
    }
}

impl Half for ScaledPixels {
    fn half(&self) -> Self {
        Self(self.0 / 2.)
    }
}

impl Half for Pixels {
    fn half(&self) -> Self {
        Self(self.0 / 2.)
    }
}

impl Half for Rems {
    fn half(&self) -> Self {
        Self(self.0 / 2.)
    }
}

/// Provides a trait for types that can negate their values.
pub trait Negate {
    /// Returns the negation of the given value
    fn negate(self) -> Self;
}

impl Negate for i32 {
    fn negate(self) -> Self {
        -self
    }
}

impl Negate for f32 {
    fn negate(self) -> Self {
        -self
    }
}

impl Negate for DevicePixels {
    fn negate(self) -> Self {
        Self(-self.0)
    }
}

impl Negate for ScaledPixels {
    fn negate(self) -> Self {
        Self(-self.0)
    }
}

impl Negate for Pixels {
    fn negate(self) -> Self {
        Self(-self.0)
    }
}

impl Negate for Rems {
    fn negate(self) -> Self {
        Self(-self.0)
    }
}

/// A trait for checking if a value is zero.
///
/// This trait provides a method to determine if a value is considered to be zero.
/// It is implemented for various numeric and length-related types where the concept
/// of zero is applicable. This can be useful for comparisons, optimizations, or
/// determining if an operation has a neutral effect.
pub trait IsZero {
    /// Determines if the value is zero.
    ///
    /// # Returns
    ///
    /// Returns `true` if the value is zero, `false` otherwise.
    fn is_zero(&self) -> bool;
}

impl IsZero for DevicePixels {
    fn is_zero(&self) -> bool {
        self.0 == 0
    }
}

impl IsZero for ScaledPixels {
    fn is_zero(&self) -> bool {
        self.0 == 0.
    }
}

impl IsZero for Pixels {
    fn is_zero(&self) -> bool {
        self.0 == 0.
    }
}

impl IsZero for Rems {
    fn is_zero(&self) -> bool {
        self.0 == 0.
    }
}

impl IsZero for AbsoluteLength {
    fn is_zero(&self) -> bool {
        match self {
            AbsoluteLength::Pixels(pixels) => pixels.is_zero(),
            AbsoluteLength::Rems(rems) => rems.is_zero(),
        }
    }
}

impl IsZero for DefiniteLength {
    fn is_zero(&self) -> bool {
        match self {
            DefiniteLength::Absolute(length) => length.is_zero(),
            DefiniteLength::Fraction(fraction) => *fraction == 0.,
        }
    }
}

impl IsZero for Length {
    fn is_zero(&self) -> bool {
        match self {
            Length::Definite(length) => length.is_zero(),
            Length::Auto => false,
        }
    }
}

impl<T: IsZero + Clone + Debug + Default + PartialEq> IsZero for Point<T> {
    fn is_zero(&self) -> bool {
        self.x.is_zero() && self.y.is_zero()
    }
}

impl<T> IsZero for Size<T>
where
    T: IsZero + Clone + Debug + Default + PartialEq,
{
    fn is_zero(&self) -> bool {
        self.width.is_zero() || self.height.is_zero()
    }
}

impl<T: IsZero + Clone + Debug + Default + PartialEq> IsZero for Bounds<T> {
    fn is_zero(&self) -> bool {
        self.size.is_zero()
    }
}

impl<T> IsZero for Corners<T>
where
    T: IsZero + Clone + Debug + Default + PartialEq,
{
    fn is_zero(&self) -> bool {
        self.top_left.is_zero()
            && self.top_right.is_zero()
            && self.bottom_right.is_zero()
            && self.bottom_left.is_zero()
    }
}
