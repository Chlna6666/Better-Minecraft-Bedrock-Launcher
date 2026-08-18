//! Minecraft Bedrock chunk data grouped by game-data responsibility.

mod encoding;
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
/// Historical chunk decoding and explicit conversion to target paletted formats.
pub mod migration;

pub use key::*;
pub use legacy::*;
pub use level_chunk::{Chunk, ChunkRecord, EntityData};
pub use migration::{
    HistoricalChunkMigrationOptions, HistoricalChunkMigrationReport, LegacyBlockMapping,
    LegacyBlockReference, LegacyBlockResolver, LegacyBlockSource, ResolvedHistoricalSubChunk,
    ResolvedLegacyTerrain, resolve_legacy_subchunk, resolve_legacy_terrain,
};
pub use palette::*;
pub use position::*;
pub use subchunk::*;
pub use crate::parsed::{
    HardcodedSpawnAreaKind, ParsedChunkData, ParsedChunkRecord, ParsedChunkRecordValue,
    ParsedHardcodedSpawnArea, parse_chunk_records, parse_chunk_records_with_options,
};

#[cfg(test)]
mod tests;
