//! Historical chunk migration backed by a verified pinned BlockState resource bundle.

use crate::block::{BlockState, PinnedBlockMigrationBundle};
use crate::chunk::migration::{
    HistoricalChunkMigrationOptions, HistoricalChunkMigrationReport,
    migrate_historical_chunk_blocking,
};
use crate::chunk::ChunkPos;
use crate::database::WorldStorage;
use crate::error::{BedrockWorldError, Result};

/// Migrates one historical chunk with a previously verified pinned BlockState migration bundle.
///
/// The bundle chooses the historical numeric ID/meta table appropriate for its target schema before
/// entering the complete versioned schema chain. `target_palette_contains` remains caller-owned
/// because an upgrade corpus does not describe the full runtime block palette of a particular
/// Minecraft build.
pub fn migrate_historical_chunk_with_pinned_bundle_blocking(
    storage: &dyn WorldStorage,
    pos: ChunkPos,
    bundle: &PinnedBlockMigrationBundle,
    target_palette_contains: &dyn Fn(&BlockState) -> bool,
    options: HistoricalChunkMigrationOptions,
) -> Result<HistoricalChunkMigrationReport> {
    if options.target_block_state_version != bundle.target_block_state_version() {
        return Err(BedrockWorldError::Validation(format!(
            "pinned migration bundle outputs BlockState version {}, but migration targets {}",
            bundle.target_block_state_version(),
            options.target_block_state_version
        )));
    }
    migrate_historical_chunk_blocking(
        storage,
        pos,
        bundle.legacy_numeric_for_target(),
        bundle.catalog(),
        target_palette_contains,
        options,
    )
}
