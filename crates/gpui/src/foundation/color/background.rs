use super::{hsla::Hsla, rgba::Rgba};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt::{self, Display, Formatter};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[repr(C)]
pub(crate) enum BackgroundTag {
    Solid = 0,
    LinearGradient = 1,
    PatternSlash = 2,
}

/// A color space for color interpolation.
///
/// References:
/// - <https://developer.mozilla.org/en-US/docs/Web/CSS/color-interpolation-method>
/// - <https://www.w3.org/TR/css-color-4/#typedef-color-space>
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[repr(C)]
pub enum ColorSpace {
    #[default]
    /// The sRGB color space.
    Srgb = 0,
    /// The Oklab color space.
    Oklab = 1,
}

impl Display for ColorSpace {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            ColorSpace::Srgb => write!(f, "sRGB"),
            ColorSpace::Oklab => write!(f, "Oklab"),
        }
    }
}

/// A background color, which can be either a solid color or a linear gradient.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[repr(C)]
pub struct Background {
    pub(crate) tag: BackgroundTag,
    pub(crate) color_space: ColorSpace,
    pub(crate) solid: Hsla,
    pub(crate) gradient_angle_or_pattern_height: f32,
    pub(crate) colors: [LinearColorStop; 2],
    /// Padding for alignment for repr(C) layout.
    pad: u32,
}

impl fmt::Debug for Background {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.tag {
            BackgroundTag::Solid => write!(f, "Solid({:?})", self.solid),
            BackgroundTag::LinearGradient => {
                write!(
                    f,
                    "LinearGradient({}, {:?}, {:?})",
                    self.gradient_angle_or_pattern_height, self.colors[0], self.colors[1]
                )
            }
            BackgroundTag::PatternSlash => {
                write!(
                    f,
                    "PatternSlash({:?}, {})",
                    self.solid, self.gradient_angle_or_pattern_height
                )
            }
        }
    }
}

impl Eq for Background {}

impl Default for Background {
    fn default() -> Self {
        Self {
            tag: BackgroundTag::Solid,
            solid: Hsla::default(),
            color_space: ColorSpace::default(),
            gradient_angle_or_pattern_height: 0.0,
            colors: [LinearColorStop::default(), LinearColorStop::default()],
            pad: 0,
        }
    }
}

/// Creates a hash pattern background
pub fn pattern_slash(color: Hsla, width: f32, interval: f32) -> Background {
    let width_scaled = (width * 255.0) as u32;
    let interval_scaled = (interval * 255.0) as u32;
    let height = ((width_scaled * 0xFFFF) + interval_scaled) as f32;

    Background {
        tag: BackgroundTag::PatternSlash,
        solid: color,
        gradient_angle_or_pattern_height: height,
        ..Default::default()
    }
}

/// Creates a solid background color.
pub fn solid_background(color: impl Into<Hsla>) -> Background {
    Background {
        solid: color.into(),
        ..Default::default()
    }
}

/// Creates a LinearGradient background color.
///
/// The gradient line's angle of direction. A value of `0.` is equivalent to top; increasing values rotate clockwise from there.
///
/// The `angle` is in degrees value in the range 0.0 to 360.0.
///
/// <https://developer.mozilla.org/en-US/docs/Web/CSS/gradient/linear-gradient>
pub fn linear_gradient(
    angle: f32,
    from: impl Into<LinearColorStop>,
    to: impl Into<LinearColorStop>,
) -> Background {
    Background {
        tag: BackgroundTag::LinearGradient,
        gradient_angle_or_pattern_height: angle,
        colors: [from.into(), to.into()],
        ..Default::default()
    }
}

/// A color stop in a linear gradient.
///
/// <https://developer.mozilla.org/en-US/docs/Web/CSS/gradient/linear-gradient#linear-color-stop>
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[repr(C)]
pub struct LinearColorStop {
    /// The color of the color stop.
    pub color: Hsla,
    /// The percentage of the gradient, in the range 0.0 to 1.0.
    pub percentage: f32,
}

/// Creates a new linear color stop.
///
/// The percentage of the gradient, in the range 0.0 to 1.0.
pub fn linear_color_stop(color: impl Into<Hsla>, percentage: f32) -> LinearColorStop {
    LinearColorStop {
        color: color.into(),
        percentage,
    }
}

impl LinearColorStop {
    /// Returns a new color stop with the same color, but with a modified alpha value.
    pub fn opacity(&self, factor: f32) -> Self {
        Self {
            percentage: self.percentage,
            color: self.color.opacity(factor),
        }
    }
}

impl Background {
    /// Use specified color space for color interpolation.
    ///
    /// <https://developer.mozilla.org/en-US/docs/Web/CSS/color-interpolation-method>
    pub fn color_space(mut self, color_space: ColorSpace) -> Self {
        self.color_space = color_space;
        self
    }

    /// Returns a new background color with the same hue, saturation, and lightness, but with a modified alpha value.
    pub fn opacity(&self, factor: f32) -> Self {
        let mut background = *self;
        background.solid = background.solid.opacity(factor);
        background.colors = [
            self.colors[0].opacity(factor),
            self.colors[1].opacity(factor),
        ];
        background
    }

    /// Returns whether the background color is transparent.
    pub fn is_transparent(&self) -> bool {
        match self.tag {
            BackgroundTag::Solid => self.solid.is_transparent(),
            BackgroundTag::LinearGradient => self.colors.iter().all(|c| c.color.is_transparent()),
            BackgroundTag::PatternSlash => self.solid.is_transparent(),
        }
    }
}

impl From<Hsla> for Background {
    fn from(value: Hsla) -> Self {
        Background {
            tag: BackgroundTag::Solid,
            solid: value,
            ..Default::default()
        }
    }
}

impl From<Rgba> for Background {
    fn from(value: Rgba) -> Self {
        Background {
            tag: BackgroundTag::Solid,
            solid: Hsla::from(value),
            ..Default::default()
        }
    }
}
