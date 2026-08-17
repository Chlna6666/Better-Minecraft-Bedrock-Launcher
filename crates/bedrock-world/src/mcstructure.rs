//! Minecraft Bedrock `.mcstructure` codecs and world placement helpers.
//!
//! File codec and placement responsibilities have stable child-module entry points while the current
//! implementation is progressively split from `mcstructure/impl.rs`.

#[path = "mcstructure/impl.rs"]
mod implementation;

pub mod codec;
pub mod placement;

pub use implementation::*;
