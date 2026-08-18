//! Explicit historical chunk conversion backed by verified BlockState version data.

use crate::block::{BlockState, PinnedBlockMigrationBundle};
use crate::chunk::conversion::{
    HistoricalChunkMigrationOptions, HistoricalChunkMigrationReport,
    migrate_historical_chunk_blocking,
};
use crate::chunk::ChunkPos;
use crate::database::WorldStorage;
use crate::error::{BedrockWorldError, Result};

/// Explicitly converts one historical chunk with a verified pinned BlockState data bundle.
///
/// Normal chunk reads and writes do not call this function. The bundle chooses the historical
/// numeric ID/meta table appropriate for the requested target representation before entering the
/// authoritative BlockState conversion chain.
pub fn migrate_historical_chunk_with_pinned_bundle_blocking(
    storage: &dyn WorldStorage,
    pos: ChunkPos,
    bundle: &PinnedBlockMigrationBundle,
    target_palette_contains: &dyn Fn(&BlockState) -> bool,
    options: HistoricalChunkMigrationOptions,
) -> Result<HistoricalChunkMigrationReport> {
    if options.target_block_state_version != bundle.target_block_state_version() {
        return Err(BedrockWorldError::Validation(format!(
            "pinned BlockState data outputs version {}, but conversion targets {}",
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
