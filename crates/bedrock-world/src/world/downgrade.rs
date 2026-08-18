//! Explicit Minecraft Bedrock world downgrade planning and independently safe downgrade steps.
//!
//! Downgrade has its own rules and loss analysis. It does not run upgrade logic backwards.

use crate::chunk::SubChunkVersion;
use crate::database::StorageOp;
use crate::entity::{ActorStorageRewriteReport, stage_world_digp_actorprefix_to_entity};
use crate::error::{BedrockWorldError, Result};
use crate::player::{SavedItemKind, read_level_dat_player};
use crate::version::GameVersion;
use crate::world::{BedrockWorld, SubChunkVersionCount, WorldStorageHandle, WorldVersions};
use std::cmp::Ordering;

/// One concrete data rewrite required to downgrade a world to the requested Bedrock release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DowngradeAction {
    /// Update version-bearing `level.dat` data only after all other records have been rewritten.
    LevelDat,
    /// Rewrite SubChunks to the target persisted version.
    SubChunks {
        /// Actual source SubChunk versions and counts.
        source: Vec<SubChunkVersionCount>,
        /// Target SubChunk version selected for the requested Bedrock release.
        target: SubChunkVersion,
    },
    /// Rewrite SubChunk terrain into pre-SubChunk `LegacyTerrain`.
    SubChunksToLegacyTerrain {
        /// Number of SubChunk records that must be collapsed into legacy chunk terrain.
        records: usize,
    },
    /// Collapse `Data3D` biome data to the older 2D representation.
    Data3DToData2D {
        /// Number of `Data3D` records found.
        records: usize,
    },
    /// Rewrite `digp`/`actorprefix` actors to chunk-scoped `Entity` records.
    DigpActorprefixToEntity {
        /// Number of `digp` records found.
        digp_records: usize,
        /// Number of `actorprefix` records found.
        actorprefix_records: usize,
    },
    /// Rewrite player saved-item representation for an older release.
    PlayerSavedItems {
        /// Number of player records requiring item changes.
        players: usize,
    },
}

/// Data that cannot be represented identically in the requested older release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DowngradeLoss {
    /// 3D biome sections must be collapsed into a 2D biome map.
    ThreeDimensionalBiomes {
        /// Number of affected `Data3D` records.
        records: usize,
    },
    /// Target block availability/state validation needs the target release palette before execution.
    TargetBlockPaletteRequired,
    /// A classic numeric target requires authoritative reverse block ID/meta data.
    LegacyNumericBlocksRequired,
    /// The target item generation requires older item ID/meta representation.
    HistoricalSavedItems {
        /// Number of affected player records.
        players: usize,
    },
}

/// Reason a downgrade target cannot be selected or inspected safely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DowngradeIssue {
    /// `level.dat` did not contain a usable last-opened Bedrock version.
    MissingSourceGameVersion,
    /// The requested target is not older than the persisted source version.
    TargetIsNotOlder,
    /// A SubChunk record had no readable version byte.
    UnversionedSubChunks {
        /// Number of affected records.
        records: usize,
    },
    /// A SubChunk version newer than the library knows was found.
    UnknownSubChunkVersion {
        /// Unknown persisted version byte.
        version: u8,
        /// Number of affected records.
        records: usize,
    },
    /// The target is in the historical Caves & Cliffs experimental range where game version alone
    /// does not uniquely select V8 versus V9 persisted SubChunks.
    ExperimentalSubChunkTarget,
}

/// Complete, non-mutating downgrade plan for one world folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DowngradePlan {
    /// Persisted source game version from `level.dat`, when present.
    pub source: Option<GameVersion>,
    /// Requested target Minecraft Bedrock version.
    pub target: GameVersion,
    /// Version/data evidence collected from the world.
    pub world: WorldVersions,
    /// Concrete rewrites required by the target.
    pub actions: Vec<DowngradeAction>,
    /// Known representation losses or external target data required by execution.
    pub losses: Vec<DowngradeLoss>,
    /// Conditions that prevent unambiguous target selection.
    pub issues: Vec<DowngradeIssue>,
}

impl DowngradePlan {
    /// Returns whether target selection is unambiguous.
    #[must_use]
    pub fn is_structurally_valid(&self) -> bool {
        self.issues.is_empty()
    }

    /// Returns whether the plan has any known lossy or target-palette-dependent operation.
    #[must_use]
    pub fn is_lossless(&self) -> bool {
        self.losses.is_empty()
    }
}

impl<S> BedrockWorld<S>
where
    S: WorldStorageHandle,
{
    /// Builds an explicit downgrade plan without modifying the world.
    pub fn downgrade_plan_blocking(&self, target: GameVersion) -> Result<DowngradePlan> {
        let versions = self.versions_blocking()?;
        let source = versions.game_version().cloned();
        let mut actions = Vec::new();
        let mut losses = Vec::new();
        let mut issues = Vec::new();

        match source.as_ref() {
            Some(source) if compare_game_versions(&target, source) != Ordering::Less => {
                issues.push(DowngradeIssue::TargetIsNotOlder);
            }
            Some(_) => {}
            None => issues.push(DowngradeIssue::MissingSourceGameVersion),
        }

        if versions.unversioned_subchunks != 0 {
            issues.push(DowngradeIssue::UnversionedSubChunks {
                records: versions.unversioned_subchunks,
            });
        }
        for entry in &versions.subchunks {
            if let SubChunkVersion::Unknown(version) = entry.version {
                issues.push(DowngradeIssue::UnknownSubChunkVersion {
                    version,
                    records: entry.records,
                });
            }
        }

        let target_subchunk = target_subchunk_version(&target);
        if target_subchunk.is_none() && game_at_least(&target, &[1, 16, 230, 50]) {
            issues.push(DowngradeIssue::ExperimentalSubChunkTarget);
        }

        if let Some(target_subchunk) = target_subchunk {
            if versions
                .subchunks
                .iter()
                .any(|entry| entry.version != target_subchunk)
            {
                actions.push(DowngradeAction::SubChunks {
                    source: versions.subchunks.clone(),
                    target: target_subchunk,
                });
                losses.push(DowngradeLoss::TargetBlockPaletteRequired);
                if target_subchunk == SubChunkVersion::V0 {
                    losses.push(DowngradeLoss::LegacyNumericBlocksRequired);
                }
            }
        } else if !game_at_least(&target, &[0, 17, 0, 1]) {
            let records = versions
                .subchunks
                .iter()
                .map(|entry| entry.records)
                .sum::<usize>();
            if records != 0 {
                actions.push(DowngradeAction::SubChunksToLegacyTerrain { records });
                losses.push(DowngradeLoss::TargetBlockPaletteRequired);
                losses.push(DowngradeLoss::LegacyNumericBlocksRequired);
            }
        }

        if !game_at_least(&target, &[1, 18, 0]) && versions.data3d_records != 0 {
            actions.push(DowngradeAction::Data3DToData2D {
                records: versions.data3d_records,
            });
            losses.push(DowngradeLoss::ThreeDimensionalBiomes {
                records: versions.data3d_records,
            });
        }
        if !game_at_least(&target, &[1, 18, 30])
            && (versions.digp_records != 0 || versions.actorprefix_records != 0)
        {
            actions.push(DowngradeAction::DigpActorprefixToEntity {
                digp_records: versions.digp_records,
                actorprefix_records: versions.actorprefix_records,
            });
        }

        let historical_players = count_players_needing_item_downgrade(self, &target)?;
        if historical_players != 0 {
            actions.push(DowngradeAction::PlayerSavedItems {
                players: historical_players,
            });
            losses.push(DowngradeLoss::HistoricalSavedItems {
                players: historical_players,
            });
        }
        actions.push(DowngradeAction::LevelDat);

        Ok(DowngradePlan {
            source,
            target,
            world: versions,
            actions,
            losses,
            issues,
        })
    }

    /// Rewrites only the world's actor storage from `digp`/`actorprefix` to chunk `Entity` records.
    ///
    /// This is an independently safe downgrade step, not a full-world downgrade. It validates the
    /// requested direction, preflights every `digp` reference, stages all `Entity` writes and removes
    /// only actorprefix records proven to be referenced by the converted digests. Orphan actorprefix
    /// records are retained and reported. SubChunk/biome downgrade losses do not block this step.
    pub fn downgrade_actor_storage_blocking(
        &self,
        target: GameVersion,
    ) -> Result<ActorStorageRewriteReport> {
        let plan = self.downgrade_plan_blocking(target)?;
        if let Some(issue) = plan.issues.iter().find(|issue| {
            matches!(
                issue,
                DowngradeIssue::MissingSourceGameVersion | DowngradeIssue::TargetIsNotOlder
            )
        }) {
            return Err(BedrockWorldError::Validation(format!(
                "actor storage downgrade cannot run: {issue:?}"
            )));
        }
        if !plan
            .actions
            .iter()
            .any(|action| matches!(action, DowngradeAction::DigpActorprefixToEntity { .. }))
        {
            return Ok(ActorStorageRewriteReport::default());
        }

        let (batch, report) = stage_world_digp_actorprefix_to_entity(self.storage())?;
        commit_actor_storage_batch(self, &batch)?;
        Ok(report)
    }
}

fn commit_actor_storage_batch<S>(world: &BedrockWorld<S>, batch: &crate::database::StorageBatch) -> Result<()>
where
    S: WorldStorageHandle,
{
    if batch.is_empty() {
        return Ok(());
    }
    let mut transaction = world.transaction();
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
    transaction.commit()
}

fn count_players_needing_item_downgrade<S>(
    world: &BedrockWorld<S>,
    target: &GameVersion,
) -> Result<usize>
where
    S: WorldStorageHandle,
{
    let mut count = 0usize;
    for id in world.list_players_blocking()? {
        if let Some(player) = world.get_player_blocking(&id)?
            && saved_items_need_downgrade(player.saved_items, target)
        {
            count = count.saturating_add(1);
        }
    }
    let level = world.read_level_dat_blocking()?;
    if let Some(player) = read_level_dat_player(&level)?
        && saved_items_need_downgrade(player.saved_items, target)
    {
        count = count.saturating_add(1);
    }
    Ok(count)
}

fn saved_items_need_downgrade(kind: SavedItemKind, target: &GameVersion) -> bool {
    if !game_at_least(target, &[1, 6, 0]) {
        matches!(
            kind,
            SavedItemKind::Named | SavedItemKind::NamedBlockState | SavedItemKind::Mixed
        )
    } else if !game_at_least(target, &[1, 9, 0]) {
        matches!(kind, SavedItemKind::NamedBlockState | SavedItemKind::Mixed)
    } else {
        false
    }
}

fn target_subchunk_version(target: &GameVersion) -> Option<SubChunkVersion> {
    if game_at_least(target, &[1, 18, 0, 20]) {
        Some(SubChunkVersion::V9)
    } else if game_at_least(target, &[1, 16, 230, 50]) {
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

fn compare_game_versions(left: &GameVersion, right: &GameVersion) -> Ordering {
    compare_components(left.components(), right.components())
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
    fn old_item_targets_are_not_treated_as_reverse_upgrade() {
        let target = GameVersion::new(vec![1, 5, 0]).unwrap();
        assert!(saved_items_need_downgrade(SavedItemKind::Named, &target));
        assert!(saved_items_need_downgrade(
            SavedItemKind::NamedBlockState,
            &target
        ));
        assert!(!saved_items_need_downgrade(
            SavedItemKind::LegacyNumeric,
            &target
        ));
    }
}
