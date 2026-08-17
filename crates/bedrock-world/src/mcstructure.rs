//! Minecraft Bedrock `.mcstructure` codecs and world placement helpers.
//!
//! Codec, model, and placement behavior currently share one cohesive implementation module. Child
//! modules are introduced only when they own implementation rather than acting as re-export facades.

mod operations;

pub use operations::*;
