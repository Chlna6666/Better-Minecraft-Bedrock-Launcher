//! Minecraft Bedrock chunk records and SubChunk payloads.

mod encoding;
mod legacy_encoding;
mod legacy_extra;
mod level_chunk;
mod subchunk;

/// Bedrock world, chunk and block coordinates and dimension identities.
pub mod position;
/// Bedrock LevelDB chunk and world-record keys.
pub mod key;
/// Block-state palettes and packed block-storage helpers.
pub mod palette;
/// Historical `LegacyTerrain` and fixed-array SubChunk representations.
pub mod legacy;
/// Actual persisted SubChunk V0-V9 values and same-version writes.
pub mod version;

pub use key::*;
pub use legacy::*;
pub use legacy_encoding::{LegacySubChunkBuilder, LegacyTerrainBuilder};
pub use legacy_extra::{
    LegacyBlockExtraData, LegacyBlockExtraDataBuilder, LegacyBlockExtraDataEntries,
    LegacyBlockExtraDataEntry,
};
pub use level_chunk::{Chunk, ChunkRecord, EntityData};
pub use palette::*;
pub use position::*;
pub use subchunk::{SubChunk, SubChunkDecodeMode, SubChunkFormat, VisibleBlockStatesAt};
pub use version::{SubChunkVersion, read_subchunk, write_subchunk_preserving_version};
pub use crate::parsed::{
    HardcodedSpawnAreaKind, ParsedChunkData, ParsedChunkRecord, ParsedChunkRecordValue,
    ParsedHardcodedSpawnArea, parse_chunk_records, parse_chunk_records_with_options,
};

/// Reads a SubChunk with full indices after detecting its persisted V0-V9 version byte.
pub fn parse_subchunk(y: i8, bytes: bytes::Bytes) -> crate::error::Result<SubChunk> {
    read_subchunk(y, bytes, SubChunkDecodeMode::FullIndices)
}

/// Reads a SubChunk with the requested index-retention mode after detecting its version byte.
pub fn parse_subchunk_with_mode(
    y: i8,
    bytes: bytes::Bytes,
    mode: SubChunkDecodeMode,
) -> crate::error::Result<SubChunk> {
    read_subchunk(y, bytes, mode)
}

#[cfg(test)]
mod tests;
