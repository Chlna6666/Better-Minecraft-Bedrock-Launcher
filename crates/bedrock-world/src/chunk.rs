//! Bedrock chunk facade and compatibility re-exports.
//!
//! New code should prefer the responsibility-specific submodules. The root `chunk::*` exports remain
//! during the 0.6 transition so existing consumers do not need an all-at-once migration.

#[path = "chunk/impl.rs"]
mod implementation;

/// Chunk/world coordinate and semantic model types.
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
