#[path = "length_serialization.rs"]
mod length_serialization;

use derive_more::{Div, Mul, Neg, Sub};
use std::{
    fmt::{self, Debug, Display},
    hash::{Hash, Hasher},
    ops::Mul as StdMul,
};

use super::{Pixels, px};

/// Represents a length in rems, a unit based on the font-size of the window, which can be assigned with [`Window::set_rem_size`][set_rem_size].
///
/// Rems are used for defining lengths that are scalable and consistent across different UI elements.
/// The value of `1rem` is typically equal to the font-size of the root element (often the `<html>` element in browsers),
/// making it a flexible unit that adapts to the user's text size preferences. In this framework, `rems` serve a similar
/// purpose, allowing for scalable and accessible design that can adjust to different display settings or user preferences.
///
/// For example, if the root element's font-size is `16px`, then `1rem` equals `16px`. A length of `2rems` would then be `32px`.
///
/// [set_rem_size]: crate::Window::set_rem_size
#[derive(Clone, Copy, Default, Sub, Mul, Div, Neg, PartialEq)]
pub struct Rems(pub f32);

impl Rems {
    /// Convert this Rem value to pixels.
    pub fn to_pixels(self, rem_size: Pixels) -> Pixels {
        self * rem_size
    }
}

impl StdMul<Pixels> for Rems {
    type Output = Pixels;

    fn mul(self, other: Pixels) -> Pixels {
        Pixels(self.0 * other.0)
    }
}

impl Display for Rems {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}rem", self.0)
    }
}

impl Debug for Rems {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(self, f)
    }
}

impl Hash for Rems {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u32(u32::from_be_bytes(self.0.to_be_bytes()));
    }
}

/// Represents an absolute length in pixels or rems.
///
/// `AbsoluteLength` can be either a fixed number of pixels, which is an absolute measurement not
/// affected by the current font size, or a number of rems, which is relative to the font size of
/// the root element. It is used for specifying dimensions that are either independent of or
/// related to the typographic scale.
#[derive(Clone, Copy, Neg, PartialEq, Hash)]
pub enum AbsoluteLength {
    /// A length in pixels.
    Pixels(Pixels),
    /// A length in rems.
    Rems(Rems),
}

impl AbsoluteLength {
    /// Checks if the absolute length is zero.
    pub fn is_zero(&self) -> bool {
        match self {
            AbsoluteLength::Pixels(px) => px.0 == 0.0,
            AbsoluteLength::Rems(rems) => rems.0 == 0.0,
        }
    }
}

impl From<Pixels> for AbsoluteLength {
    fn from(pixels: Pixels) -> Self {
        AbsoluteLength::Pixels(pixels)
    }
}

impl From<Rems> for AbsoluteLength {
    fn from(rems: Rems) -> Self {
        AbsoluteLength::Rems(rems)
    }
}

impl AbsoluteLength {
    /// Converts an `AbsoluteLength` to `Pixels` based on a given `rem_size`.
    ///
    /// # Arguments
    ///
    /// * `rem_size` - The size of one rem in pixels.
    ///
    /// # Returns
    ///
    /// Returns the `AbsoluteLength` as `Pixels`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::{AbsoluteLength, Pixels, Rems};
    /// let length_in_pixels = AbsoluteLength::Pixels(Pixels::from(42.0));
    /// let length_in_rems = AbsoluteLength::Rems(Rems(2.0));
    /// let rem_size = Pixels::from(16.0);
    ///
    /// assert_eq!(length_in_pixels.to_pixels(rem_size), Pixels::from(42.0));
    /// assert_eq!(length_in_rems.to_pixels(rem_size), Pixels::from(32.0));
    /// ```
    pub fn to_pixels(self, rem_size: Pixels) -> Pixels {
        match self {
            AbsoluteLength::Pixels(pixels) => pixels,
            AbsoluteLength::Rems(rems) => rems.to_pixels(rem_size),
        }
    }

    /// Converts an `AbsoluteLength` to `Rems` based on a given `rem_size`.
    ///
    /// # Arguments
    ///
    /// * `rem_size` - The size of one rem in pixels.
    ///
    /// # Returns
    ///
    /// Returns the `AbsoluteLength` as `Pixels`.
    pub fn to_rems(self, rem_size: Pixels) -> Rems {
        match self {
            AbsoluteLength::Pixels(pixels) => Rems(pixels.0 / rem_size.0),
            AbsoluteLength::Rems(rems) => rems,
        }
    }
}

impl Default for AbsoluteLength {
    fn default() -> Self {
        px(0.).into()
    }
}

impl Display for AbsoluteLength {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Pixels(pixels) => write!(f, "{pixels}"),
            Self::Rems(rems) => write!(f, "{rems}"),
        }
    }
}

impl Debug for AbsoluteLength {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(self, f)
    }
}

/// A non-auto length that can be defined in pixels, rems, or percent of parent.
///
/// This enum represents lengths that have a specific value, as opposed to lengths that are automatically
/// determined by the context. It includes absolute lengths in pixels or rems, and relative lengths as a
/// fraction of the parent's size.
#[derive(Clone, Copy, Neg, PartialEq)]
pub enum DefiniteLength {
    /// An absolute length specified in pixels or rems.
    Absolute(AbsoluteLength),
    /// A relative length specified as a fraction of the parent's size, between 0 and 1.
    Fraction(f32),
}

impl DefiniteLength {
    /// Converts the `DefiniteLength` to `Pixels` based on a given `base_size` and `rem_size`.
    ///
    /// If the `DefiniteLength` is an absolute length, it will be directly converted to `Pixels`.
    /// If it is a fraction, the fraction will be multiplied by the `base_size` to get the length in pixels.
    ///
    /// # Arguments
    ///
    /// * `base_size` - The base size in `AbsoluteLength` to which the fraction will be applied.
    /// * `rem_size` - The size of one rem in pixels, used to convert rems to pixels.
    ///
    /// # Returns
    ///
    /// Returns the `DefiniteLength` as `Pixels`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::{DefiniteLength, AbsoluteLength, Pixels, px, rems};
    /// let length_in_pixels = DefiniteLength::Absolute(AbsoluteLength::Pixels(px(42.0)));
    /// let length_in_rems = DefiniteLength::Absolute(AbsoluteLength::Rems(rems(2.0)));
    /// let length_as_fraction = DefiniteLength::Fraction(0.5);
    /// let base_size = AbsoluteLength::Pixels(px(100.0));
    /// let rem_size = px(16.0);
    ///
    /// assert_eq!(length_in_pixels.to_pixels(base_size, rem_size), Pixels::from(42.0));
    /// assert_eq!(length_in_rems.to_pixels(base_size, rem_size), Pixels::from(32.0));
    /// assert_eq!(length_as_fraction.to_pixels(base_size, rem_size), Pixels::from(50.0));
    /// ```
    pub fn to_pixels(self, base_size: AbsoluteLength, rem_size: Pixels) -> Pixels {
        match self {
            DefiniteLength::Absolute(size) => size.to_pixels(rem_size),
            DefiniteLength::Fraction(fraction) => match base_size {
                AbsoluteLength::Pixels(px) => px * fraction,
                AbsoluteLength::Rems(rems) => rems * rem_size * fraction,
            },
        }
    }
}

impl Debug for DefiniteLength {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(self, f)
    }
}

impl Display for DefiniteLength {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DefiniteLength::Absolute(length) => write!(f, "{length}"),
            DefiniteLength::Fraction(fraction) => write!(f, "{}%", (fraction * 100.0) as i32),
        }
    }
}

impl From<Pixels> for DefiniteLength {
    fn from(pixels: Pixels) -> Self {
        Self::Absolute(pixels.into())
    }
}

impl From<Rems> for DefiniteLength {
    fn from(rems: Rems) -> Self {
        Self::Absolute(rems.into())
    }
}

impl From<AbsoluteLength> for DefiniteLength {
    fn from(length: AbsoluteLength) -> Self {
        Self::Absolute(length)
    }
}

impl Default for DefiniteLength {
    fn default() -> Self {
        Self::Absolute(AbsoluteLength::default())
    }
}

/// A length that can be defined in pixels, rems, percent of parent, or auto.
#[derive(Clone, Copy, PartialEq)]
pub enum Length {
    /// A definite length specified either in pixels, rems, or as a fraction of the parent's size.
    Definite(DefiniteLength),
    /// An automatic length that is determined by the context in which it is used.
    Auto,
}

impl Debug for Length {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(self, f)
    }
}

impl Display for Length {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Length::Definite(definite_length) => write!(f, "{}", definite_length),
            Length::Auto => write!(f, "auto"),
        }
    }
}

/// Constructs a `DefiniteLength` representing a relative fraction of a parent size.
///
/// This function creates a `DefiniteLength` that is a specified fraction of a parent's dimension.
/// The fraction should be a floating-point number between 0.0 and 1.0, where 1.0 represents 100% of the parent's size.
///
/// # Arguments
///
/// * `fraction` - The fraction of the parent's size, between 0.0 and 1.0.
///
/// # Returns
///
/// A `DefiniteLength` representing the relative length as a fraction of the parent's size.
pub const fn relative(fraction: f32) -> DefiniteLength {
    DefiniteLength::Fraction(fraction)
}

/// Returns the Golden Ratio, i.e. `~(1.0 + sqrt(5.0)) / 2.0`.
pub fn phi() -> DefiniteLength {
    relative(1.618_034)
}

/// Constructs a `Rems` value representing a length in rems.
///
/// # Arguments
///
/// * `rems` - The number of rems for the length.
///
/// # Returns
///
/// A `Rems` representing the specified number of rems.
pub fn rems(rems: f32) -> Rems {
    Rems(rems)
}

/// Returns a `Length` representing an automatic length.
///
/// The `auto` length is often used in layout calculations where the length should be determined
/// by the layout context itself rather than being explicitly set. This is commonly used in CSS
/// for properties like `width`, `height`, `margin`, `padding`, etc., where `auto` can be used
/// to instruct the layout engine to calculate the size based on other factors like the size of the
/// container or the intrinsic size of the content.
///
/// # Returns
///
/// A `Length` variant set to `Auto`.
pub fn auto() -> Length {
    Length::Auto
}

impl From<Pixels> for Length {
    fn from(pixels: Pixels) -> Self {
        Self::Definite(pixels.into())
    }
}

impl From<Rems> for Length {
    fn from(rems: Rems) -> Self {
        Self::Definite(rems.into())
    }
}

impl From<DefiniteLength> for Length {
    fn from(length: DefiniteLength) -> Self {
        Self::Definite(length)
    }
}

impl From<AbsoluteLength> for Length {
    fn from(length: AbsoluteLength) -> Self {
        Self::Definite(length.into())
    }
}

impl Default for Length {
    fn default() -> Self {
        Self::Definite(DefiniteLength::default())
    }
}

impl From<()> for Length {
    fn from(_: ()) -> Self {
        Self::Definite(DefiniteLength::default())
    }
}
