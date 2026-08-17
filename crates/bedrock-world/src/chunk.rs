//! Minecraft Bedrock chunk data grouped by game-data responsibility.

mod level_chunk;

/// Bedrock world, chunk and block coordinates and dimension identities.
pub mod position;
/// Bedrock LevelDB chunk and world-record keys.
pub mod key;
/// Block-state palettes and packed block-storage helpers.
pub mod palette;
/// Historical numeric terrain and legacy subchunk representations.
pub mod legacy;
/// Versioned Minecraft Bedrock SubChunk payloads and decode policies.
pub mod subchunk;
/// Modern paletted subchunk write helpers shared by world editing and structure placement.
pub(crate) mod subchunk_write;

pub use key::*;
pub use legacy::*;
pub use level_chunk::{Chunk, ChunkRecord, EntityData};
pub use palette::*;
pub use position::*;
pub use subchunk::*;

#[cfg(test)]
mod tests;
