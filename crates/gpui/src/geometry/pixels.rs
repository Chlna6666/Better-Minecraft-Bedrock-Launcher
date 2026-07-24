use anyhow::Context as _;
use derive_more::{Add, AddAssign, Div, DivAssign, Neg, Sub, SubAssign};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    cmp,
    fmt::{self, Debug, Display},
    ops::{Div as StdDiv, Mul as StdMul, MulAssign},
};

use super::ScaledPixels;

/// Represents a length in pixels, the base unit of measurement in the UI framework.
///
/// `Pixels` is a value type that represents an absolute length in pixels, which is used
/// for specifying sizes, positions, and distances in the UI. It is the fundamental unit
/// of measurement for all visual elements and layout calculations.
///
/// The inner value is an `f32`, allowing for sub-pixel precision which can be useful for
/// anti-aliasing and animations. However, when applied to actual pixel grids, the value
/// is typically rounded to the nearest integer.
///
/// # Examples
///
/// ```
/// use gpui::{Pixels, ScaledPixels};
///
/// // Define a length of 10 pixels
/// let length = Pixels::from(10.0);
///
/// // Define a length and scale it by a factor of 2
/// let scaled_length = length.scale(2.0);
/// assert_eq!(scaled_length, ScaledPixels::from(20.0));
/// ```
#[derive(
    Clone,
    Copy,
    Default,
    Add,
    AddAssign,
    Sub,
    SubAssign,
    Neg,
    Div,
    DivAssign,
    PartialEq,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[repr(transparent)]
pub struct Pixels(pub(crate) f32);

impl StdDiv for Pixels {
    type Output = f32;

    fn div(self, rhs: Self) -> Self::Output {
        self.0 / rhs.0
    }
}

impl std::ops::DivAssign for Pixels {
    fn div_assign(&mut self, rhs: Self) {
        *self = Self(self.0 / rhs.0);
    }
}

impl std::ops::RemAssign for Pixels {
    fn rem_assign(&mut self, rhs: Self) {
        self.0 %= rhs.0;
    }
}

impl std::ops::Rem for Pixels {
    type Output = Self;

    fn rem(self, rhs: Self) -> Self {
        Self(self.0 % rhs.0)
    }
}

impl StdMul<f32> for Pixels {
    type Output = Self;

    fn mul(self, rhs: f32) -> Self {
        Self(self.0 * rhs)
    }
}

impl StdMul<Pixels> for f32 {
    type Output = Pixels;

    fn mul(self, rhs: Pixels) -> Self::Output {
        rhs * self
    }
}

impl StdMul<usize> for Pixels {
    type Output = Self;

    fn mul(self, rhs: usize) -> Self {
        self * (rhs as f32)
    }
}

impl StdMul<Pixels> for usize {
    type Output = Pixels;

    fn mul(self, rhs: Pixels) -> Pixels {
        rhs * self
    }
}

impl MulAssign<f32> for Pixels {
    fn mul_assign(&mut self, rhs: f32) {
        self.0 *= rhs;
    }
}

impl Display for Pixels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}px", self.0)
    }
}

impl Debug for Pixels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(self, f)
    }
}

impl TryFrom<&'_ str> for Pixels {
    type Error = anyhow::Error;

    fn try_from(value: &'_ str) -> Result<Self, Self::Error> {
        value
            .strip_suffix("px")
            .context("expected 'px' suffix")
            .and_then(|number| Ok(number.parse()?))
            .map(Self)
    }
}

impl Pixels {
    /// Represents zero pixels.
    pub const ZERO: Pixels = Pixels(0.0);
    /// The maximum value that can be represented by `Pixels`.
    pub const MAX: Pixels = Pixels(f32::MAX);
    /// The minimum value that can be represented by `Pixels`.
    pub const MIN: Pixels = Pixels(f32::MIN);

    /// Floors the `Pixels` value to the nearest whole number.
    ///
    /// # Returns
    ///
    /// Returns a new `Pixels` instance with the floored value.
    pub fn floor(&self) -> Self {
        Self(self.0.floor())
    }

    /// Rounds the `Pixels` value to the nearest whole number.
    ///
    /// # Returns
    ///
    /// Returns a new `Pixels` instance with the rounded value.
    pub fn round(&self) -> Self {
        Self(self.0.round())
    }

    /// Returns the ceiling of the `Pixels` value to the nearest whole number.
    ///
    /// # Returns
    ///
    /// Returns a new `Pixels` instance with the ceiling value.
    pub fn ceil(&self) -> Self {
        Self(self.0.ceil())
    }

    /// Scales the `Pixels` value by a given factor, producing `ScaledPixels`.
    ///
    /// This method is used when adjusting pixel values for display scaling factors,
    /// such as high DPI (dots per inch) or Retina displays, where the pixel density is higher and
    /// thus requires scaling to maintain visual consistency and readability.
    ///
    /// The resulting `ScaledPixels` represent the scaled value which can be used for rendering
    /// calculations where display scaling is considered.
    #[must_use]
    pub fn scale(&self, factor: f32) -> ScaledPixels {
        ScaledPixels(self.0 * factor)
    }

    /// Raises the `Pixels` value to a given power.
    ///
    /// # Arguments
    ///
    /// * `exponent` - The exponent to raise the `Pixels` value by.
    ///
    /// # Returns
    ///
    /// Returns a new `Pixels` instance with the value raised to the given exponent.
    pub fn pow(&self, exponent: f32) -> Self {
        Self(self.0.powf(exponent))
    }

    /// Returns the absolute value of the `Pixels`.
    ///
    /// # Returns
    ///
    /// A new `Pixels` instance with the absolute value of the original `Pixels`.
    pub fn abs(&self) -> Self {
        Self(self.0.abs())
    }

    /// Returns the sign of the `Pixels` value.
    ///
    /// # Returns
    ///
    /// Returns:
    /// * `1.0` if the value is positive
    /// * `-1.0` if the value is negative
    pub fn signum(&self) -> f32 {
        self.0.signum()
    }

    /// Returns the f64 value of `Pixels`.
    ///
    /// # Returns
    ///
    /// A f64 value of the `Pixels`.
    pub fn to_f64(self) -> f64 {
        self.0 as f64
    }
}

impl Eq for Pixels {}

impl PartialOrd for Pixels {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Pixels {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl std::hash::Hash for Pixels {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.to_bits().hash(state);
    }
}

impl From<f64> for Pixels {
    fn from(pixels: f64) -> Self {
        Pixels(pixels as f32)
    }
}

impl From<f32> for Pixels {
    fn from(pixels: f32) -> Self {
        Pixels(pixels)
    }
}

impl From<Pixels> for f32 {
    fn from(pixels: Pixels) -> Self {
        pixels.0
    }
}

impl From<&Pixels> for f32 {
    fn from(pixels: &Pixels) -> Self {
        pixels.0
    }
}

impl From<Pixels> for f64 {
    fn from(pixels: Pixels) -> Self {
        pixels.0 as f64
    }
}

impl From<Pixels> for u32 {
    fn from(pixels: Pixels) -> Self {
        pixels.0 as u32
    }
}

impl From<&Pixels> for u32 {
    fn from(pixels: &Pixels) -> Self {
        pixels.0 as u32
    }
}

impl From<u32> for Pixels {
    fn from(pixels: u32) -> Self {
        Pixels(pixels as f32)
    }
}

impl From<Pixels> for usize {
    fn from(pixels: Pixels) -> Self {
        pixels.0 as usize
    }
}

impl From<usize> for Pixels {
    fn from(pixels: usize) -> Self {
        Pixels(pixels as f32)
    }
}
