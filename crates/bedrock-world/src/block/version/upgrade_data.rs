//! Ready-to-use authoritative block upgrade data for one target Minecraft Bedrock palette.

use super::{
    AuthoritativeBlockStateCatalog, LegacyNumericBlockStateTable, PinnedBlockMigrationBundle,
    VanillaBlockStatePalette, load_pinned_block_migration_bundle_for_target_from_dir,
};
use crate::error::{BedrockWorldError, Result};
use std::path::Path;

/// Verified BlockState schema rules plus the historical numeric block table selected for one target.
///
/// This is the developer-facing block upgrade input. The older internal `MigrationBundle` name is not
/// exposed through the block-domain public API. Data loaded here is pinned, Git-blob verified and
/// bound to the target palette's persisted BlockState storage version.
#[derive(Debug)]
pub struct BlockUpgradeData {
    inner: PinnedBlockMigrationBundle,
}

impl BlockUpgradeData {
    /// Returns the authoritative ordered BlockState upgrade rules for this target.
    #[must_use]
    pub const fn block_states(&self) -> &AuthoritativeBlockStateCatalog {
        self.inner.catalog()
    }

    /// Returns the historical numeric ID/metadata table selected for this target BlockState version.
    #[must_use]
    pub fn legacy_numeric_blocks(&self) -> &LegacyNumericBlockStateTable {
        self.inner.legacy_numeric_for_target()
    }

    /// Returns the exact persisted BlockState version produced by these upgrade rules.
    #[must_use]
    pub const fn target_block_state_version(&self) -> i32 {
        self.inner.target_block_state_version()
    }
}

/// Loads the pinned authoritative block upgrade corpus for one real target-game vanilla palette.
///
/// The target palette supplies the exact persisted BlockState endpoint. The pinned schema loader
/// refuses targets that are not represented by a real schema endpoint in the verified corpus.
pub fn load_pinned_block_upgrade_data_for_palette(
    root: impl AsRef<Path>,
    target_palette: &VanillaBlockStatePalette,
) -> Result<BlockUpgradeData> {
    let inner = load_pinned_block_migration_bundle_for_target_from_dir(
        root,
        target_palette.storage_version(),
    )?;
    if inner.target_block_state_version() != target_palette.storage_version().raw() {
        return Err(BedrockWorldError::Validation(format!(
            "pinned block upgrade data ends at {}, target Bedrock {} palette uses {}",
            inner.target_block_state_version(),
            target_palette.game_version(),
            target_palette.storage_version().raw()
        )));
    }
    Ok(BlockUpgradeData { inner })
}
