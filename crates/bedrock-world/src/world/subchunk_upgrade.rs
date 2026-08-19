//! Atomic Minecraft Bedrock SubChunk upgrade across legacy numeric and paletted generations.

use crate::block::{BlockUpgradeData, VanillaBlockStatePalette};
use crate::chunk::{
    LegacySubChunkUpgradeWriteReport, SubChunkUpgradeWriteReport, SubChunkVersion,
    stage_legacy_subchunks_for_upgrade, stage_paletted_subchunks_for_upgrade,
};
use crate::database::{StorageBatch, StorageOp};
use crate::error::{BedrockWorldError, Result};
use crate::version::GameVersion;
use crate::world::{BedrockWorld, WorldFormat, WorldStorageHandle};
use std::cmp::Ordering;

/// Result of one atomic Minecraft Bedrock SubChunk upgrade over legacy numeric and paletted records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BedrockWorldSubChunkUpgradeReport {
    /// Target persisted SubChunk version selected for the requested Bedrock game version.
    pub target: SubChunkVersion,
    /// Legacy V0/V2-V7 numeric SubChunks converted through the authoritative numeric table.
    ///
    /// For V8/V9 targets this report also includes historical `BlockExtraData` second-layer merges.
    pub legacy: LegacySubChunkUpgradeWriteReport,
    /// Existing paletted SubChunks upgraded through the authoritative BlockState schema rules.
    pub paletted: SubChunkUpgradeWriteReport,
}

impl BedrockWorldSubChunkUpgradeReport {
    /// Returns the total number of SubChunk records staged for rewriting.
    #[must_use]
    pub const fn rewritten_records(&self) -> usize {
        self.legacy.records.saturating_add(self.paletted.rewritten)
    }

    /// Returns the total encoded value bytes staged before the atomic commit.
    #[must_use]
    pub const fn staged_bytes(&self) -> usize {
        self.legacy
            .staged_bytes
            .saturating_add(self.paletted.staged_bytes)
    }
}

impl<S> BedrockWorld<S>
where
    S: WorldStorageHandle,
{
    /// Upgrades all supported SubChunk generations to one target Bedrock version in one transaction.
    ///
    /// Source direction and persisted SubChunk evidence are checked directly for this operation rather
    /// than through a synthetic whole-world upgrade plan. Legacy V0/V2-V7 numeric blocks are resolved
    /// through [`BlockUpgradeData`], while already-paletted SubChunks use the authoritative BlockState
    /// schema chain. Both paths validate every resulting BlockState against the exact target-game
    /// vanilla palette.
    ///
    /// For V8/V9 targets, historical chunk-scoped `BlockExtraData` (`0x34`) entries are assigned by
    /// persisted full Y coordinate to the matching legacy SubChunk and become a second paletted storage
    /// layer. The original `BlockExtraData` record is deleted only after every entry converts.
    ///
    /// Only after both legacy and paletted staging complete successfully are their disjoint changes
    /// committed through one [`crate::world::WorldTransaction`]. `LegacyTerrain`, biomes, actors,
    /// player items and `level.dat` are deliberately outside this concrete operation.
    pub fn upgrade_bedrock_subchunks_blocking(
        &self,
        target: GameVersion,
        upgrade_data: &BlockUpgradeData,
        target_palette: &VanillaBlockStatePalette,
    ) -> Result<BedrockWorldSubChunkUpgradeReport> {
        validate_upgrade_source(self, &target)?;
        validate_target_data(&target, upgrade_data, target_palette)?;

        let target_version = target_subchunk_version(&target).ok_or_else(|| {
            BedrockWorldError::Validation(format!(
                "Bedrock {target} does not select one unambiguous SubChunk version"
            ))
        })?;
        if !matches!(
            target_version,
            SubChunkVersion::V1 | SubChunkVersion::V8 | SubChunkVersion::V9
        ) {
            return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
                "authoritative numeric-to-BlockState upgrade requires a paletted V1/V8/V9 target, Bedrock {target} selects {target_version:?}"
            )));
        }

        let (legacy_batch, legacy) = stage_legacy_subchunks_for_upgrade(
            self.storage(),
            target_version,
            upgrade_data.legacy_numeric_blocks(),
            upgrade_data.block_states(),
            target_palette,
        )?;
        let (paletted_batch, paletted) = stage_paletted_subchunks_for_upgrade(
            self.storage(),
            target_version,
            upgrade_data.block_states(),
            target_palette,
        )?;

        commit_subchunk_upgrade(self, [&legacy_batch, &paletted_batch])?;
        Ok(BedrockWorldSubChunkUpgradeReport {
            target: target_version,
            legacy,
            paletted,
        })
    }
}

fn validate_upgrade_source<S>(world: &BedrockWorld<S>, target: &GameVersion) -> Result<()>
where
    S: WorldStorageHandle,
{
    let versions = world.versions_blocking()?;
    if versions.world_format == WorldFormat::PocketChunksDat {
        return Err(BedrockWorldError::ReadOnly);
    }
    let source = versions.game_version().ok_or_else(|| {
        BedrockWorldError::Validation(
            "SubChunk upgrade requires level.dat LastOpenedWithVersion evidence".to_string(),
        )
    })?;
    if compare_components(target.components(), source.components()) != Ordering::Greater {
        return Err(BedrockWorldError::Validation(format!(
            "SubChunk upgrade target {target} must be newer than persisted source {source}"
        )));
    }
    if versions.unversioned_subchunks != 0 {
        return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
            "{} SubChunk records have no readable version byte",
            versions.unversioned_subchunks
        )));
    }
    if let Some(entry) = versions
        .subchunks
        .iter()
        .find(|entry| matches!(entry.version, SubChunkVersion::Unknown(_)))
    {
        return Err(BedrockWorldError::UnsupportedChunkFormat(format!(
            "cannot upgrade unknown persisted SubChunk version {:?} ({} records)",
            entry.version, entry.records
        )));
    }
    Ok(())
}

fn validate_target_data(
    target: &GameVersion,
    upgrade_data: &BlockUpgradeData,
    target_palette: &VanillaBlockStatePalette,
) -> Result<()> {
    if target_palette.game_version() != target {
        return Err(BedrockWorldError::Validation(format!(
            "target vanilla BlockState palette is for Bedrock {}, requested upgrade target is {target}",
            target_palette.game_version()
        )));
    }
    if upgrade_data.target_block_state_version() != target_palette.storage_version().raw() {
        return Err(BedrockWorldError::Validation(format!(
            "block upgrade data ends at {}, target Bedrock {target} palette uses {}",
            upgrade_data.target_block_state_version(),
            target_palette.storage_version().raw()
        )));
    }
    Ok(())
}

fn commit_subchunk_upgrade<S>(
    world: &BedrockWorld<S>,
    batches: [&StorageBatch; 2],
) -> Result<()>
where
    S: WorldStorageHandle,
{
    if batches.iter().all(|batch| batch.is_empty()) {
        return Ok(());
    }

    let mut transaction = world.transaction();
    for batch in batches {
        for op in batch.ops() {
            match op {
                StorageOp::Put { key, value } => {
                    transaction.put_raw_key(key.clone(), value.clone());
                }
                StorageOp::Delete { key } => {
                    transaction.delete_raw_key(key.clone());
                }
            }
        }
    }
    transaction.commit()
}

fn target_subchunk_version(target: &GameVersion) -> Option<SubChunkVersion> {
    if game_at_least(target, &[1, 18, 0, 20]) {
        Some(SubChunkVersion::V9)
    } else if game_at_least(target, &[1, 16, 230, 50]) {
        // Experimental Caves & Cliffs builds used non-unique storage transitions; exact persisted
        // evidence is required rather than guessing V8 versus V9 from a broad version interval.
        None
    } else if game_at_least(target, &[1, 2, 14, 2]) {
        Some(SubChunkVersion::V8)
    } else if game_at_least(target, &[1, 2, 13]) {
        Some(SubChunkVersion::V1)
    } else if game_at_least(target, &[0, 17, 0, 1]) {
        Some(SubChunkVersion::V0)
    } else {
        None
    }
}

fn game_at_least(version: &GameVersion, minimum: &[i32]) -> bool {
    compare_components(version.components(), minimum) != Ordering::Less
}

fn compare_components(left: &[i32], right: &[i32]) -> Ordering {
    let len = left.len().max(right.len());
    for index in 0..len {
        let left = left.get(index).copied().unwrap_or(0);
        let right = right.get(index).copied().unwrap_or(0);
        match left.cmp(&right) {
            Ordering::Equal => {}
            other => return other,
        }
    }
    Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_paletted_targets_are_selected() {
        assert_eq!(
            target_subchunk_version(&GameVersion::new(vec![1, 18, 0, 20]).unwrap()),
            Some(SubChunkVersion::V9)
        );
        assert_eq!(
            target_subchunk_version(&GameVersion::new(vec![1, 10, 0, 0]).unwrap()),
            Some(SubChunkVersion::V8)
        );
        assert_eq!(
            target_subchunk_version(&GameVersion::new(vec![1, 17, 40]).unwrap()),
            None
        );
    }
}
