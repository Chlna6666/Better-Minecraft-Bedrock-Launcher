//! Linear-light shading with one sRGB decode/encode boundary.

use std::sync::LazyLock;

use super::RgbaColor;

const ENCODE_STEPS: usize = 65_535;

static LINEAR: LazyLock<[f32; 256]> = LazyLock::new(|| {
    std::array::from_fn(|channel| {
        let encoded = channel as f32 / 255.0;
        if encoded <= 0.04045 {
            encoded / 12.92
        } else {
            ((encoded + 0.055) / 1.055).powf(2.4)
        }
    })
});

static ENCODED: LazyLock<[u8; ENCODE_STEPS + 1]> = LazyLock::new(|| {
    std::array::from_fn(|index| {
        let linear = index as f32 / ENCODE_STEPS as f32;
        let encoded = if linear <= 0.0031308 {
            linear * 12.92
        } else {
            1.055 * linear.powf(1.0 / 2.4) - 0.055
        };
        (encoded * 255.0).round() as u8
    })
});

impl RgbaColor {
    /// Scales RGB radiance in linear-light sRGB, then encodes it back to 8-bit sRGB.
    ///
    /// `multiplier` is a finite, nonnegative light intensity: `1.0` preserves the
    /// original color exactly, `0.0` makes RGB black, and larger values brighten it.
    /// Values outside the display gamut are clipped per channel. Alpha is preserved;
    /// this does not composite a background, change opacity, or modify a world.
    /// Unlike mixing highlights toward white, this preserves linear RGB ratios
    /// until gamut clipping or 8-bit rounding occurs. Shared lookup tables avoid
    /// evaluating transfer-function powers for every rendered pixel.
    #[must_use]
    pub fn shade(self, multiplier: f32) -> Self {
        if multiplier == 1.0 {
            return self;
        }
        let channel = |encoded: u8| {
            let linear = (LINEAR[usize::from(encoded)] * multiplier).clamp(0.0, 1.0);
            ENCODED[(linear * ENCODE_STEPS as f32).round() as usize]
        };
        Self::new(
            channel(self.red),
            channel(self.green),
            channel(self.blue),
            self.alpha,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_tables_roundtrip_every_channel() {
        for value in 0..=u8::MAX {
            let index = (LINEAR[usize::from(value)] * ENCODE_STEPS as f32).round() as usize;
            assert_eq!(ENCODED[index], value);
        }
    }

    #[test]
    fn shading_does_not_mix_colored_highlights_with_white() {
        for value in 0..=u8::MAX {
            for multiplier in [0.0, 0.2, 0.56, 1.0, 1.5] {
                let color = RgbaColor::new(value, 0, 0, 37).shade(multiplier);
                assert_eq!(color.green, 0);
                assert_eq!(color.blue, 0);
                assert_eq!(color.alpha, 37);
                if multiplier == 1.0 {
                    assert_eq!(color.red, value);
                }
            }
        }
        assert_eq!(RgbaColor::new(128, 128, 128, 255).shade(0.5).red, 92);
    }
}
