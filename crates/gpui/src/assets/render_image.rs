mod frame;
mod image;
mod source;
mod streaming;

pub(crate) use frame::AnimatedFrame;
pub use image::RenderImage;
pub(super) use image::RenderImageData;
pub(crate) use source::{AnimatedImageSource, DecodedAnimation};
