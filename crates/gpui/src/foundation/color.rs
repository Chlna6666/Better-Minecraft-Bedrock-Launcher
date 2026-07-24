mod background;
mod hsla;
mod rgba;

#[cfg(test)]
mod tests;

pub(crate) use background::BackgroundTag;
pub use background::{
    Background, ColorSpace, LinearColorStop, linear_color_stop, linear_gradient, pattern_slash,
    solid_background,
};
pub use hsla::{
    Hsla, black, blue, green, hsla, opaque_grey, red, transparent_black, transparent_white, white,
    yellow,
};
pub(crate) use rgba::swap_rgba_pa_to_bgra;
pub use rgba::{Rgba, rgb, rgba};
