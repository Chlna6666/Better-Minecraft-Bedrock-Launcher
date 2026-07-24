use super::rgba::Rgba;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{
    borrow::Cow,
    fmt::{Display, Formatter},
    hash::{Hash, Hasher},
};

/// An HSLA color
#[derive(Default, Copy, Clone, Debug)]
#[repr(C)]
pub struct Hsla {
    /// Hue, in a range from 0 to 1
    pub h: f32,

    /// Saturation, in a range from 0 to 1
    pub s: f32,

    /// Lightness, in a range from 0 to 1
    pub l: f32,

    /// Alpha, in a range from 0 to 1
    pub a: f32,
}

impl PartialEq for Hsla {
    fn eq(&self, other: &Self) -> bool {
        self.h
            .total_cmp(&other.h)
            .then(self.s.total_cmp(&other.s))
            .then(self.l.total_cmp(&other.l).then(self.a.total_cmp(&other.a)))
            .is_eq()
    }
}

impl PartialOrd for Hsla {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Hsla {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.h
            .total_cmp(&other.h)
            .then(self.s.total_cmp(&other.s))
            .then(self.l.total_cmp(&other.l).then(self.a.total_cmp(&other.a)))
    }
}

impl Eq for Hsla {}

impl Hash for Hsla {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u32(u32::from_be_bytes(self.h.to_be_bytes()));
        state.write_u32(u32::from_be_bytes(self.s.to_be_bytes()));
        state.write_u32(u32::from_be_bytes(self.l.to_be_bytes()));
        state.write_u32(u32::from_be_bytes(self.a.to_be_bytes()));
    }
}

impl Display for Hsla {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "hsla({:.2}, {:.2}%, {:.2}%, {:.2})",
            self.h * 360.,
            self.s * 100.,
            self.l * 100.,
            self.a
        )
    }
}

/// Construct an [`Hsla`] object from plain values
pub fn hsla(h: f32, s: f32, l: f32, a: f32) -> Hsla {
    Hsla {
        h: h.clamp(0., 1.),
        s: s.clamp(0., 1.),
        l: l.clamp(0., 1.),
        a: a.clamp(0., 1.),
    }
}

/// Pure black in [`Hsla`]
pub const fn black() -> Hsla {
    Hsla {
        h: 0.,
        s: 0.,
        l: 0.,
        a: 1.,
    }
}

/// Transparent black in [`Hsla`]
pub const fn transparent_black() -> Hsla {
    Hsla {
        h: 0.,
        s: 0.,
        l: 0.,
        a: 0.,
    }
}

/// Transparent white in [`Hsla`]
pub const fn transparent_white() -> Hsla {
    Hsla {
        h: 0.,
        s: 0.,
        l: 1.,
        a: 0.,
    }
}

/// Opaque grey in [`Hsla`], values will be clamped to the range [0, 1]
pub fn opaque_grey(lightness: f32, opacity: f32) -> Hsla {
    Hsla {
        h: 0.,
        s: 0.,
        l: lightness.clamp(0., 1.),
        a: opacity.clamp(0., 1.),
    }
}

/// Pure white in [`Hsla`]
pub const fn white() -> Hsla {
    Hsla {
        h: 0.,
        s: 0.,
        l: 1.,
        a: 1.,
    }
}

/// The color red in [`Hsla`]
pub const fn red() -> Hsla {
    Hsla {
        h: 0.,
        s: 1.,
        l: 0.5,
        a: 1.,
    }
}

/// The color blue in [`Hsla`]
pub const fn blue() -> Hsla {
    Hsla {
        h: 0.6666666667,
        s: 1.,
        l: 0.5,
        a: 1.,
    }
}

/// The color green in [`Hsla`]
pub const fn green() -> Hsla {
    Hsla {
        h: 0.3333333333,
        s: 1.,
        l: 0.25,
        a: 1.,
    }
}

/// The color yellow in [`Hsla`]
pub const fn yellow() -> Hsla {
    Hsla {
        h: 0.1666666667,
        s: 1.,
        l: 0.5,
        a: 1.,
    }
}

impl Hsla {
    /// Converts this HSLA color to an RGBA color.
    pub fn to_rgb(self) -> Rgba {
        self.into()
    }

    /// The color red
    pub const fn red() -> Self {
        red()
    }

    /// The color green
    pub const fn green() -> Self {
        green()
    }

    /// The color blue
    pub const fn blue() -> Self {
        blue()
    }

    /// The color black
    pub const fn black() -> Self {
        black()
    }

    /// The color white
    pub const fn white() -> Self {
        white()
    }

    /// The color transparent black
    pub const fn transparent_black() -> Self {
        transparent_black()
    }

    /// Returns true if the HSLA color is fully transparent, false otherwise.
    pub fn is_transparent(&self) -> bool {
        self.a == 0.0
    }

    /// Returns true if the HSLA color is fully opaque, false otherwise.
    pub fn is_opaque(&self) -> bool {
        self.a == 1.0
    }

    /// Blends `other` on top of `self` based on `other`'s alpha value. The resulting color is a combination of `self`'s and `other`'s colors.
    ///
    /// If `other`'s alpha value is 1.0 or greater, `other` color is fully opaque, thus `other` is returned as the output color.
    /// If `other`'s alpha value is 0.0 or less, `other` color is fully transparent, thus `self` is returned as the output color.
    /// Else, the output color is calculated as a blend of `self` and `other` based on their weighted alpha values.
    ///
    /// Assumptions:
    /// - Alpha values are contained in the range [0, 1], with 1 as fully opaque and 0 as fully transparent.
    /// - The relative contributions of `self` and `other` is based on `self`'s alpha value (`self.a`) and `other`'s  alpha value (`other.a`), `self` contributing `self.a * (1.0 - other.a)` and `other` contributing its own alpha value.
    /// - RGB color components are contained in the range [0, 1].
    /// - If `self` and `other` colors are out of the valid range, the blend operation's output and behavior is undefined.
    pub fn blend(self, other: Hsla) -> Hsla {
        let alpha = other.a;

        if alpha >= 1.0 {
            other
        } else if alpha <= 0.0 {
            self
        } else {
            let converted_self = Rgba::from(self);
            let converted_other = Rgba::from(other);
            let blended_rgb = converted_self.blend(converted_other);
            Hsla::from(blended_rgb)
        }
    }

    /// Returns a new HSLA color with the same hue, and lightness, but with no saturation.
    pub fn grayscale(&self) -> Self {
        Hsla {
            h: self.h,
            s: 0.,
            l: self.l,
            a: self.a,
        }
    }

    /// Fade out the color by a given factor. This factor should be between 0.0 and 1.0.
    /// Where 0.0 will leave the color unchanged, and 1.0 will completely fade out the color.
    pub fn fade_out(&mut self, factor: f32) {
        self.a *= 1.0 - factor.clamp(0., 1.);
    }

    /// Multiplies the alpha value of the color by a given factor
    /// and returns a new HSLA color.
    ///
    /// Useful for transforming colors with dynamic opacity,
    /// like a color from an external source.
    ///
    /// Example:
    /// ```
    /// let color = gpui::red();
    /// let faded_color = color.opacity(0.5);
    /// assert_eq!(faded_color.a, 0.5);
    /// ```
    ///
    /// This will return a red color with half the opacity.
    ///
    /// Example:
    /// ```
    /// use gpui::hsla;
    /// let color = hsla(0.7, 1.0, 0.5, 0.7); // A saturated blue
    /// let faded_color = color.opacity(0.16);
    /// assert!((faded_color.a - 0.112).abs() < 1e-6);
    /// ```
    ///
    /// This will return a blue color with around ~10% opacity,
    /// suitable for an element's hover or selected state.
    ///
    pub fn opacity(&self, factor: f32) -> Self {
        Hsla {
            h: self.h,
            s: self.s,
            l: self.l,
            a: self.a * factor.clamp(0., 1.),
        }
    }

    /// Returns a new HSLA color with the same hue, saturation,
    /// and lightness, but with a new alpha value.
    ///
    /// Example:
    /// ```
    /// let color = gpui::red();
    /// let red_color = color.alpha(0.25);
    /// assert_eq!(red_color.a, 0.25);
    /// ```
    ///
    /// This will return a red color with half the opacity.
    ///
    /// Example:
    /// ```
    /// use gpui::hsla;
    /// let color = hsla(0.7, 1.0, 0.5, 0.7); // A saturated blue
    /// let faded_color = color.alpha(0.25);
    /// assert_eq!(faded_color.a, 0.25);
    /// ```
    ///
    /// This will return a blue color with 25% opacity.
    pub fn alpha(&self, a: f32) -> Self {
        Hsla {
            h: self.h,
            s: self.s,
            l: self.l,
            a: a.clamp(0., 1.),
        }
    }
}

impl From<Rgba> for Hsla {
    fn from(color: Rgba) -> Self {
        let r = color.r;
        let g = color.g;
        let b = color.b;

        let max = r.max(g.max(b));
        let min = r.min(g.min(b));
        let delta = max - min;

        let l = (max + min) / 2.0;
        let s = if l == 0.0 || l == 1.0 {
            0.0
        } else if l < 0.5 {
            delta / (2.0 * l)
        } else {
            delta / (2.0 - 2.0 * l)
        };

        let h = if delta == 0.0 {
            0.0
        } else if max == r {
            ((g - b) / delta).rem_euclid(6.0) / 6.0
        } else if max == g {
            ((b - r) / delta + 2.0) / 6.0
        } else {
            ((r - g) / delta + 4.0) / 6.0
        };

        Hsla {
            h,
            s,
            l,
            a: color.a,
        }
    }
}

impl JsonSchema for Hsla {
    fn schema_name() -> Cow<'static, str> {
        Rgba::schema_name()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        Rgba::json_schema(generator)
    }
}

impl Serialize for Hsla {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Rgba::from(*self).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Hsla {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(Rgba::deserialize(deserializer)?.into())
    }
}
