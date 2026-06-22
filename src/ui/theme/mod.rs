// Theme scaffolding.
//
// We'll migrate hard-coded colors from the UI components into this module as the UI stabilizes.

pub mod colors;

pub use colors::{
    DarkColors, LightColors, ThemeColors, lerp_theme_colors, parse_hex_color_to_hsla,
};
