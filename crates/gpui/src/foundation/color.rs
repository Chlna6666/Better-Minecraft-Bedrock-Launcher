mod background;
mod batch;
mod hsla;
mod premultiplied;
mod rgba;

#[cfg(test)]
mod tests;

pub(crate) use background::BackgroundTag;
pub use background::{
    Background, ColorSpace, LinearColorStop, linear_color_stop, linear_gradient, pattern_slash,
    solid_background,
};
pub use batch::{hsla_to_rgba_batch, lerp_hsla_batch, rgba_to_hsla_batch};
pub use hsla::{
    Hsla, black, blue, green, hsla, opaque_grey, red, transparent_black, transparent_white, white,
    yellow,
};
pub(crate) use premultiplied::swap_rgba_pa_to_bgra_buffer;
pub(crate) use rgba::swap_rgba_pa_to_bgra;
pub use rgba::{Rgba, rgb, rgba};
