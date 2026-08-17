//! Minecraft Bedrock chunk data, keys, palettes and historical chunk formats.

#[path = "chunk/impl.rs"]
mod implementation;

/// Chunk/world coordinate and semantic chunk types.
pub mod model;
/// Bedrock world database key codecs and key classifications.
pub mod key;
/// Block-state palettes and packed block-storage helpers.
pub mod palette;
/// Historical numeric terrain and legacy subchunk representations.
pub mod legacy;
/// Versioned subchunk payload models and decode policies.
pub mod subchunk;

pub use implementation::*;
