use derive_more::{Add, AddAssign, Div, DivAssign, Sub, SubAssign};
use std::{
    cmp,
    fmt::{self, Debug},
    ops::{Div as StdDiv, Mul as StdMul, MulAssign},
};

use super::DevicePixels;

/// Represents scaled pixels that take into account the device's scale factor.
///
/// `ScaledPixels` are used to ensure that UI elements appear at the correct size on devices
/// with different pixel densities. When a device has a higher scale factor (such as Retina displays),
/// a single logical pixel may correspond to multiple physical pixels. By using `ScaledPixels`,
/// dimensions and positions can be specified in a way that scales appropriately across different
/// display resolutions.
#[derive(Clone, Copy, Default, Add, AddAssign, Sub, SubAssign, Div, DivAssign, PartialEq)]
#[repr(transparent)]
pub struct ScaledPixels(pub(crate) f32);

impl ScaledPixels {
    /// Floors the `ScaledPixels` value to the nearest whole number.
    ///
    /// # Returns
    ///
    /// Returns a new `ScaledPixels` instance with the floored value.
    pub fn floor(&self) -> Self {
        Self(self.0.floor())
    }

    /// Rounds the `ScaledPixels` value to the nearest whole number.
    ///
    /// # Returns
    ///
    /// Returns a new `ScaledPixels` instance with the rounded value.
    pub fn round(&self) -> Self {
        Self(self.0.round())
    }

    /// Ceils the `ScaledPixels` value to the nearest whole number.
    ///
    /// # Returns
    ///
    /// Returns a new `ScaledPixels` instance with the ceiled value.
    pub fn ceil(&self) -> Self {
        Self(self.0.ceil())
    }
}

impl Eq for ScaledPixels {}

impl PartialOrd for ScaledPixels {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScaledPixels {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl Debug for ScaledPixels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}px (scaled)", self.0)
    }
}

impl From<ScaledPixels> for DevicePixels {
    fn from(scaled: ScaledPixels) -> Self {
        DevicePixels(scaled.0.ceil() as i32)
    }
}

impl From<DevicePixels> for ScaledPixels {
    fn from(device: DevicePixels) -> Self {
        ScaledPixels(device.0 as f32)
    }
}

impl From<ScaledPixels> for f64 {
    fn from(scaled_pixels: ScaledPixels) -> Self {
        scaled_pixels.0 as f64
    }
}

impl From<ScaledPixels> for u32 {
    fn from(pixels: ScaledPixels) -> Self {
        pixels.0 as u32
    }
}

impl From<f32> for ScaledPixels {
    fn from(pixels: f32) -> Self {
        Self(pixels)
    }
}

impl StdDiv for ScaledPixels {
    type Output = f32;

    fn div(self, rhs: Self) -> Self::Output {
        self.0 / rhs.0
    }
}

impl std::ops::DivAssign for ScaledPixels {
    fn div_assign(&mut self, rhs: Self) {
        *self = Self(self.0 / rhs.0);
    }
}

impl std::ops::RemAssign for ScaledPixels {
    fn rem_assign(&mut self, rhs: Self) {
        self.0 %= rhs.0;
    }
}

impl std::ops::Rem for ScaledPixels {
    type Output = Self;

    fn rem(self, rhs: Self) -> Self {
        Self(self.0 % rhs.0)
    }
}

impl StdMul<f32> for ScaledPixels {
    type Output = Self;

    fn mul(self, rhs: f32) -> Self {
        Self(self.0 * rhs)
    }
}

impl StdMul<ScaledPixels> for f32 {
    type Output = ScaledPixels;

    fn mul(self, rhs: ScaledPixels) -> Self::Output {
        rhs * self
    }
}

impl StdMul<usize> for ScaledPixels {
    type Output = Self;

    fn mul(self, rhs: usize) -> Self {
        self * (rhs as f32)
    }
}

impl StdMul<ScaledPixels> for usize {
    type Output = ScaledPixels;

    fn mul(self, rhs: ScaledPixels) -> ScaledPixels {
        rhs * self
    }
}

impl MulAssign<f32> for ScaledPixels {
    fn mul_assign(&mut self, rhs: f32) {
        self.0 *= rhs;
    }
}
