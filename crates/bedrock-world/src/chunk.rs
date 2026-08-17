//! Minecraft Bedrock chunk data, keys, palettes and historical chunk formats.

// Chunk storage codec shared by the responsibility-specific public modules below.
// Keep this implementation private so callers use stable semantic paths such as
// `chunk::key`, `chunk::model`, and `chunk::subchunk` rather than an implementation module.
mod codec;

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
/// Modern paletted subchunk write helpers shared by world editing and structure placement.
pub(crate) mod subchunk_write;

pub use codec::*;
