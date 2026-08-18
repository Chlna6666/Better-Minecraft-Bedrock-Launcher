//! Explicit Minecraft Bedrock world upgrade planning and independently safe upgrade steps.
//!
//! Ordinary world reads/writes never call this code. Upgrade is a separate operation and its plan is
//! derived from the actual persisted data present in the world folder.

use crate::chunk::SubChunkVersion;
use crate::database::StorageOp;
use crate::entity::{ActorStorageRewriteReport, stage_world_entity_to_digp_actorprefix};
use crate::error::{BedrockWorldError, Result};
use crate::player::{SavedItemKind, read_level_dat_player};
use crate::version::GameVersion;
use crate::world::{BedrockWorld, SubChunkVersionCount, WorldFormat, WorldStorageHandle, WorldVersions};
use std::cmp::Ordering;

/// One concrete data rewrite required to upgrade a world to the requested Bedrock release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpgradeAction {
    /// Update version-bearing `level.dat` data after all world data has been rewritten successfully.
    LevelDat,
    /// Rewrite `LegacyTerrain` into the target terrain representation.
    LegacyTerrain {
        /// Number of `LegacyTerrain` records found.
        records: usize,
    },
    /// Rewrite persisted SubChunks to the target SubChunk version.
    SubChunks {
        /// Actual source SubChunk versions and counts.
        source: Vec<SubChunkVersionCount>,
        /// Target SubChunk version selected for the requested Bedrock release.
        target: SubChunkVersion,
    },
    /// Rewrite `Data2D`/`Data2DLegacy` biome data to `Data3D`.
    Data2DToData3D {
        /// Number of 2D biome records found.
        records: usize,
    },
    /// Rewrite chunk-scoped `Entity` actor data to `digp`/`actorprefix` storage.
    EntityToDigpActorprefix {
        /// Number of chunk `Entity` records found.
        records: usize,
    },
    /// Rewrite historical saved-item identities/BlockStates in player records.
    PlayerSavedItems {
        /// Number of player records containing historical or mixed saved-item forms.
        players: usize,
    },
}

/// Reason an upgrade target cannot be selected safely from the persisted evidence alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpgradeIssue {
    /// `level.dat` did not contain a usable last-opened Bedrock version.
    MissingSourceGameVersion,
    /// The requested target is not newer than the persisted source version.
    TargetIsNotNewer,
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
    /// The pre-LevelDB `chunks.dat` backend is currently read-only.
    PocketChunksDatWriteNotImplemented,
    /// The target sits in the historical Caves & Cliffs experimental range where game version alone
    /// does not uniquely select V8 versus V9 persisted SubChunks.
    ExperimentalSubChunkTarget,
}

/// Complete, non-mutating upgrade plan for one world folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpgradePlan {
    /// Persisted source game version from `level.dat`, when present.
    pub source: Option<GameVersion>,
    /// Requested target Minecraft Bedrock version.
    pub target: GameVersion,
    /// Version/data evidence collected from the world.
    pub world: WorldVersions,
    /// Concrete rewrites required by the target.
    pub actions: Vec<UpgradeAction>,
    /// Conditions that prevent unambiguous target selection.
    pub issues: Vec<UpgradeIssue>,
}

impl UpgradePlan {
    /// Returns whether the requested target and persisted version evidence are unambiguous.
    #[must_use]
    pub fn is_structurally_valid(&self) -> bool {
        self.issues.is_empty()
    }
}

impl<S> BedrockWorld<S>
where
    S: WorldStorageHandle,
{
    /// Builds an explicit upgrade plan without modifying the world.
    pub fn upgrade_plan_blocking(&self, target: GameVersion) -> Result<UpgradePlan> {
        let versions = self.versions_blocking()?;
        let source = versions.game_version().cloned();
        let mut actions = Vec::new();
        let mut issues = Vec::new();

        match source.as_ref() {
            Some(source) if compare_game_versions(&target, source) != Ordering::Greater => {
                issues.push(UpgradeIssue::TargetIsNotNewer);
            }
            Some(_) => {}
            None => issues.push(UpgradeIssue::MissingSourceGameVersion),
        }

        if versions.world_format == WorldFormat::PocketChunksDat {
            issues.push(UpgradeIssue::PocketChunksDatWriteNotImplemented);
        }
        if versions.unversioned_subchunks != 0 {
            issues.push(UpgradeIssue::UnversionedSubChunks {
                records: versions.unversioned_subchunks,
            });
        }
        for entry in &versions.subchunks {
            if let SubChunkVersion::Unknown(version) = entry.version {
                issues.push(UpgradeIssue::UnknownSubChunkVersion {
                    version,
                    records: entry.records,
                });
            }
        }

        let target_subchunk = target_subchunk_version(&target);
        if target_subchunk.is_none() && game_at_least(&target, &[1, 16, 230, 50]) {
            issues.push(UpgradeIssue::ExperimentalSubChunkTarget);
        }

        if versions.legacy_terrain_records != 0 {
            actions.push(UpgradeAction::LegacyTerrain {
                records: versions.legacy_terrain_records,
            });
        }
        if let Some(target_subchunk) = target_subchunk {
            if versions
                .subchunks
                .iter()
                .any(|entry| entry.version != target_subchunk)
            {
                actions.push(UpgradeAction::SubChunks {
                    source: versions.subchunks.clone(),
                    target: target_subchunk,
                });
            }
        }
        if game_at_least(&target, &[1, 18, 0]) {
            let records = versions
                .data2d_records
                .saturating_add(versions.data2d_legacy_records);
            if records != 0 {
                actions.push(UpgradeAction::Data2DToData3D { records });
            }
        }
        if game_at_least(&target, &[1, 18, 30]) && versions.entity_records != 0 {
            actions.push(UpgradeAction::EntityToDigpActorprefix {
                records: versions.entity_records,
            });
        }

        let historical_players = count_historical_player_items(self)?;
        if historical_players != 0 {
            actions.push(UpgradeAction::PlayerSavedItems {
                players: historical_players,
            });
        }
        actions.push(UpgradeAction::LevelDat);

        Ok(UpgradePlan {
            source,
            target,
            world: versions,
            actions,
            issues,
        })
    }

    /// Rewrites only the world's actor storage from chunk `Entity` records to `digp`/`actorprefix`.
    ///
    /// This is an independently safe upgrade step, not a full-world upgrade. It first rebuilds the
    /// current plan, rejects only direction/backend issues relevant to Actor storage, preflights all
    /// actor references, and commits the complete Actor rewrite as one storage batch. SubChunk or
    /// biome issues do not block this operation because they are not touched.
    pub fn upgrade_actor_storage_blocking(
        &self,
        target: GameVersion,
    ) -> Result<ActorStorageRewriteReport> {
        let plan = self.upgrade_plan_blocking(target)?;
        if let Some(issue) = plan.issues.iter().find(|issue| {
            matches!(
                issue,
                UpgradeIssue::MissingSourceGameVersion
                    | UpgradeIssue::TargetIsNotNewer
                    | UpgradeIssue::PocketChunksDatWriteNotImplemented
            )
        }) {
            return Err(BedrockWorldError::Validation(format!(
                "actor storage upgrade cannot run: {issue:?}"
            )));
        }
        if !plan
            .actions
            .iter()
            .any(|action| matches!(action, UpgradeAction::EntityToDigpActorprefix { .. }))
        {
            return Ok(ActorStorageRewriteReport::default());
        }

        let (batch, report) = stage_world_entity_to_digp_actorprefix(self.storage())?;
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

fn count_historical_player_items<S>(world: &BedrockWorld<S>) -> Result<usize>
where
    S: WorldStorageHandle,
{
    let mut count = 0usize;
    for id in world.list_players_blocking()? {
        if let Some(player) = world.get_player_blocking(&id)?
            && matches!(
                player.saved_items,
                SavedItemKind::LegacyNumeric | SavedItemKind::Mixed
            )
        {
            count = count.saturating_add(1);
        }
    }
    let level = world.read_level_dat_blocking()?;
    if let Some(player) = read_level_dat_player(&level)?
        && matches!(
            player.saved_items,
            SavedItemKind::LegacyNumeric | SavedItemKind::Mixed
        )
    {
        count = count.saturating_add(1);
    }
    Ok(count)
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
    fn stable_target_subchunk_versions_are_selected_explicitly() {
        assert_eq!(
            target_subchunk_version(&GameVersion::new(vec![1, 18, 0, 20]).unwrap()),
            Some(SubChunkVersion::V9)
        );
        assert_eq!(
            target_subchunk_version(&GameVersion::new(vec![1, 17, 40]).unwrap()),
            None
        );
        assert_eq!(
            target_subchunk_version(&GameVersion::new(vec![1, 2, 14, 2]).unwrap()),
            Some(SubChunkVersion::V8)
        );
        assert_eq!(
            target_subchunk_version(&GameVersion::new(vec![1, 2, 13]).unwrap()),
            Some(SubChunkVersion::V1)
        );
    }
}
