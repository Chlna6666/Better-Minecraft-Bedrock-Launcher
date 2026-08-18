//! Minecraft Bedrock chunk data grouped by persisted game-data responsibility.

mod encoding;
mod legacy_encoding;
mod legacy_extra;
mod legacy_numeric_resolver;
mod level_chunk;
mod pinned_migration;
mod subchunk;

/// Bedrock world, chunk and block coordinates and dimension identities.
pub mod position;
/// Bedrock LevelDB chunk and world-record keys.
pub mod key;
/// Block-state palettes and packed block-storage helpers.
pub mod palette;
/// Historical numeric terrain and legacy subchunk representations.
pub mod legacy;
/// Actual persisted SubChunk versions, conservative reads and version-preserving writes.
pub mod version;
/// Historical chunk decoding and explicit conversion to target paletted formats.
pub mod migration;

pub use key::*;
pub use legacy::*;
pub use legacy_encoding::{LegacySubChunkBuilder, LegacyTerrainBuilder};
pub use legacy_extra::{
    LegacyBlockExtraData, LegacyBlockExtraDataBuilder, LegacyBlockExtraDataEntries,
    LegacyBlockExtraDataEntry,
};
pub use level_chunk::{Chunk, ChunkRecord, EntityData};
pub use migration::{
    HistoricalChunkMigrationOptions, HistoricalChunkMigrationReport, LegacyBlockMapping,
    LegacyBlockReference, LegacyBlockResolver, LegacyBlockSource, ResolvedHistoricalSubChunk,
    ResolvedLegacyTerrain, resolve_legacy_subchunk, resolve_legacy_terrain,
};
pub use palette::*;
pub use pinned_migration::migrate_historical_chunk_with_pinned_bundle_blocking;
pub use position::*;
pub use subchunk::{SubChunk, SubChunkDecodeMode, SubChunkFormat, VisibleBlockStatesAt};
pub use version::{SubChunkVersion, read_subchunk, write_subchunk_preserving_version};
pub use crate::parsed::{
    HardcodedSpawnAreaKind, ParsedChunkData, ParsedChunkRecord, ParsedChunkRecordValue,
    ParsedHardcodedSpawnArea, parse_chunk_records, parse_chunk_records_with_options,
};

/// Reads a SubChunk with full indices after automatically detecting its persisted V0-V9 version.
pub fn parse_subchunk(y: i8, bytes: bytes::Bytes) -> crate::error::Result<SubChunk> {
    read_subchunk(y, bytes, SubChunkDecodeMode::FullIndices)
}

/// Reads a SubChunk with the requested index-retention mode after automatically detecting its
/// persisted version. Unknown versions are retained raw rather than interpreted as a known format.
pub fn parse_subchunk_with_mode(
    y: i8,
    bytes: bytes::Bytes,
    mode: SubChunkDecodeMode,
) -> crate::error::Result<SubChunk> {
    read_subchunk(y, bytes, mode)
}

#[cfg(test)]
mod tests;
