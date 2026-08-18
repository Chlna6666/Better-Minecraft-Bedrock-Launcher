//! Minecraft Bedrock chunk records and SubChunk payloads.

mod encoding;
mod legacy_encoding;
mod legacy_extra;
mod legacy_subchunk_upgrade;
mod level_chunk;
mod subchunk;
mod subchunk_storage;
mod subchunk_upgrade;
#[path = "chunk/subchunk/v0.rs"]
mod subchunk_v0;
#[path = "chunk/subchunk/v1.rs"]
mod subchunk_v1;
#[path = "chunk/subchunk/v2_v7.rs"]
mod subchunk_v2_v7;
#[path = "chunk/subchunk/v8.rs"]
mod subchunk_v8;
#[path = "chunk/subchunk/v9.rs"]
mod subchunk_v9;

/// Bedrock world, chunk and block coordinates and dimension identities.
pub mod position;
/// Bedrock LevelDB chunk and world-record keys.
pub mod key;
/// Block-state palettes and packed block-storage helpers.
pub mod palette;
/// Historical `LegacyTerrain` and fixed-array SubChunk representations.
pub mod legacy;
/// Minecraft Bedrock SubChunk version byte values.
pub mod version;

pub use key::*;
pub use legacy::*;
pub use legacy_encoding::{LegacySubChunkBuilder, LegacyTerrainBuilder};
pub use legacy_extra::{
    LegacyBlockExtraData, LegacyBlockExtraDataBuilder, LegacyBlockExtraDataEntries,
    LegacyBlockExtraDataEntry,
};
pub use legacy_subchunk_upgrade::LegacySubChunkUpgradeWriteReport;
pub(crate) use legacy_subchunk_upgrade::stage_legacy_subchunks_for_upgrade;
pub use level_chunk::{Chunk, ChunkRecord, EntityData};
pub use palette::*;
pub use position::*;
pub use subchunk::{SubChunk, SubChunkDecodeMode, SubChunkFormat, VisibleBlockStatesAt};
pub use subchunk_storage::{SubChunkDowngradeWriteReport, SubChunkStorageWriteReport};
pub(crate) use subchunk_storage::{
    stage_subchunks_as_version, stage_subchunks_for_exact_downgrade,
};
pub use subchunk_upgrade::SubChunkUpgradeWriteReport;
pub(crate) use subchunk_upgrade::stage_paletted_subchunks_for_upgrade;
pub use version::SubChunkVersion;
pub use crate::parsed::{
    HardcodedSpawnAreaKind, ParsedChunkData, ParsedChunkRecord, ParsedChunkRecordValue,
    ParsedHardcodedSpawnArea, parse_chunk_records, parse_chunk_records_with_options,
};

/// Reads a SubChunk with full block indices after automatically detecting V0-V9.
pub fn parse_subchunk(y: i8, bytes: bytes::Bytes) -> crate::error::Result<SubChunk> {
    SubChunk::read(y, bytes, SubChunkDecodeMode::FullIndices)
}

/// Reads a SubChunk with the requested block-index retention after automatically detecting V0-V9.
pub fn parse_subchunk_with_mode(
    y: i8,
    bytes: bytes::Bytes,
    mode: SubChunkDecodeMode,
) -> crate::error::Result<SubChunk> {
    SubChunk::read(y, bytes, mode)
}

#[cfg(test)]
mod tests;
