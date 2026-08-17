//! Minecraft Bedrock `.mcstructure` codecs and world placement helpers.
//!
//! File codec and placement responsibilities have stable child-module entry points while placement
//! operations remain isolated under `mcstructure/operations.rs`.

mod operations;

pub mod codec;
pub mod placement;

pub use operations::*;
