use derive_more::{Add, AddAssign, Div, Sub, SubAssign};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Represents physical pixels on the display.
///
/// `DevicePixels` is a unit of measurement that refers to the actual pixels on a device's screen.
/// This type is used when precise pixel manipulation is required, such as rendering graphics or
/// interfacing with hardware that operates on the pixel level. Unlike logical pixels that may be
/// affected by the device's scale factor, `DevicePixels` always correspond to real pixels on the
/// display.
#[derive(
    Add,
    AddAssign,
    Clone,
    Copy,
    Default,
    Div,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    Sub,
    SubAssign,
    Serialize,
    Deserialize,
)]
#[repr(transparent)]
pub struct DevicePixels(pub i32);

impl DevicePixels {
    /// Converts the `DevicePixels` value to the number of bytes needed to represent it in memory.
    ///
    /// This function is useful when working with graphical data that needs to be stored in a buffer,
    /// such as images or framebuffers, where each pixel may be represented by a specific number of bytes.
    ///
    /// # Arguments
    ///
    /// * `bytes_per_pixel` - The number of bytes used to represent a single pixel.
    ///
    /// # Returns
    ///
    /// The number of bytes required to represent the `DevicePixels` value in memory.
    ///
    /// # Examples
    ///
    /// ```
    /// # use gpui::DevicePixels;
    /// let pixels = DevicePixels(10); // 10 device pixels
    /// let bytes_per_pixel = 4; // Assume each pixel is represented by 4 bytes (e.g., RGBA)
    /// let total_bytes = pixels.to_bytes(bytes_per_pixel);
    /// assert_eq!(total_bytes, 40); // 10 pixels * 4 bytes/pixel = 40 bytes
    /// ```
    pub fn to_bytes(self, bytes_per_pixel: u8) -> u32 {
        self.0 as u32 * bytes_per_pixel as u32
    }
}

impl fmt::Debug for DevicePixels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} px (device)", self.0)
    }
}

impl From<DevicePixels> for i32 {
    fn from(device_pixels: DevicePixels) -> Self {
        device_pixels.0
    }
}

impl From<i32> for DevicePixels {
    fn from(device_pixels: i32) -> Self {
        DevicePixels(device_pixels)
    }
}

impl From<u32> for DevicePixels {
    fn from(device_pixels: u32) -> Self {
        DevicePixels(device_pixels as i32)
    }
}

impl From<DevicePixels> for u32 {
    fn from(device_pixels: DevicePixels) -> Self {
        device_pixels.0 as u32
    }
}

impl From<DevicePixels> for u64 {
    fn from(device_pixels: DevicePixels) -> Self {
        device_pixels.0 as u64
    }
}

impl From<u64> for DevicePixels {
    fn from(device_pixels: u64) -> Self {
        DevicePixels(device_pixels as i32)
    }
}

impl From<DevicePixels> for usize {
    fn from(device_pixels: DevicePixels) -> Self {
        device_pixels.0 as usize
    }
}

impl From<usize> for DevicePixels {
    fn from(device_pixels: usize) -> Self {
        DevicePixels(device_pixels as i32)
    }
}
