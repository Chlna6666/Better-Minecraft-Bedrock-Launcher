//! Minecraft Bedrock chunk records and SubChunk payloads.

mod block_entities;
mod encoding;
mod heightmap;
mod legacy_encoding;
mod legacy_extra;
pub(crate) mod legacy_terrain;
mod legacy_terrain_combine;
mod legacy_terrain_storage;
mod level;
pub mod subchunk;
/// Scheduled and random block-tick records stored with chunks.
pub mod ticks;

/// Bedrock LevelDB chunk and world-record keys.
pub mod key;
/// Historical `LegacyTerrain` and fixed-array SubChunk representations.
pub mod legacy;
/// Block-state palettes and packed block-storage helpers.
pub mod palette;
/// Bedrock world, chunk and block coordinates and dimension identities.
pub mod position;

pub use crate::scan::{ChunkEntry, ChunkValue, HardcodedSpawnArea, HardcodedSpawnAreaKind};
pub use block_entities::ChunkBlockEntities;
pub use heightmap::{ChunkHeightMap, ChunkHeightMapStatus};
pub use key::*;
pub use legacy::*;
pub use legacy_encoding::{LegacySubChunkBuilder, LegacySubChunkEncoding, LegacyTerrainBuilder};
pub use legacy_extra::{
    LegacyBlockExtraData, LegacyBlockExtraDataBuilder, LegacyBlockExtraDataEntries,
    LegacyBlockExtraDataEntry,
};
pub use legacy_terrain_combine::LegacyTerrainCombineReport;
pub use legacy_terrain_storage::LegacyTerrainSplitReport;
pub(crate) use legacy_terrain_storage::stage_legacy_terrain_split;
pub use level::{ChunkRecord, EntityData, LevelChunk};
pub use palette::*;
pub use position::*;
pub(crate) use subchunk::stage_paletted_subchunks_for_upgrade;
pub use subchunk::{
    NumericSubChunkDowngradeReport, NumericSubChunkUpgradeReport, SubChunk, SubChunkDecodeMode,
    SubChunkDowngradeWriteReport, SubChunkFormat,
    SubChunkStorageWriteReport, SubChunkUpgradeReport, SubChunkUpgradeWriteReport, SubChunkVersion,
    VisibleBlockStatesAt,
};
pub(crate) use subchunk::{stage_numeric_subchunk_upgrade, stage_subchunk_downgrade};

#[cfg(test)]
mod tests;
