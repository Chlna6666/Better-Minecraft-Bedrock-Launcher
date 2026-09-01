//! Minecraft Bedrock chunk records and SubChunk payloads.

mod encoding;
mod block_entities;
mod heightmap;
mod legacy_encoding;
mod legacy_extra;
mod legacy_terrain_combine;
pub(crate) mod legacy_terrain;
mod legacy_terrain_storage;
mod level_chunk;
mod subchunk;

/// Bedrock LevelDB chunk and world-record keys.
pub mod key;
/// Historical `LegacyTerrain` and fixed-array SubChunk representations.
pub mod legacy;
/// Block-state palettes and packed block-storage helpers.
pub mod palette;
/// Bedrock world, chunk and block coordinates and dimension identities.
pub mod position;

pub use crate::scan::{
    HardcodedSpawnAreaKind, ChunkEntry, ChunkValue,
    HardcodedSpawnArea,
};
pub use heightmap::{ChunkHeightMap, ChunkHeightMapStatus};
pub use block_entities::ChunkBlockEntities;
pub use key::*;
pub use legacy::*;
pub use legacy_encoding::{LegacySubChunkBuilder, LegacyTerrainBuilder};
pub use legacy_extra::{
    LegacyBlockExtraData, LegacyBlockExtraDataBuilder, LegacyBlockExtraDataEntries,
    LegacyBlockExtraDataEntry,
};
pub use legacy_terrain_combine::LegacyTerrainCombineReport;
pub(crate) use legacy_terrain_combine::stage_legacy_terrain_combine;
pub use legacy_terrain_storage::LegacyTerrainSplitReport;
pub(crate) use legacy_terrain_storage::stage_legacy_terrain_split;
pub use level_chunk::{ChunkRecord, EntityData, LevelChunk};
pub use palette::*;
pub use position::*;
pub(crate) use subchunk::stage_paletted_subchunks_for_upgrade;
pub use subchunk::{
    NumericSubChunkDowngradeReport, NumericSubChunkUpgradeReport, SubChunk, SubChunkDecodeMode,
    SubChunkHeightMapContribution, subchunk_height_map_contribution,
    SubChunkDowngradeWriteReport, SubChunkFormat, SubChunkStorageWriteReport, SubChunkUpgradeReport,
    SubChunkUpgradeWriteReport, SubChunkVersion, VisibleBlockStatesAt,
};
pub(crate) use subchunk::{
    stage_numeric_subchunk_upgrade, stage_subchunk_downgrade, stage_subchunks_as_version,
    stage_subchunks_for_exact_downgrade,
};

#[cfg(test)]
mod tests;
