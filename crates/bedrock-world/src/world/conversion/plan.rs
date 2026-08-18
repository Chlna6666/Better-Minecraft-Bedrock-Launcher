//! Whole-world migration planning derived from observed records rather than a single global version.

use crate::chunk::ChunkPos;
use crate::entity::{ActorMigrationAction, classify_actor_migration};
use crate::integrity::{
    ActorStorageModel, CompatibilityLevel, WorldCompatibilityReport, WritePolicy,
};

/// A reason a world cannot be migrated destructively without additional authoritative information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationBlocker {
    /// Future/unknown chunk data is present and must be preserved raw.
    FutureChunkData(ChunkPos),
    /// Corrupt chunk data must be repaired before migration.
    CorruptChunk(ChunkPos),
    /// Actor storage evidence cannot be reconciled automatically.
    ActorStorage,
}

/// One chunk selected for historical format migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkMigrationTarget {
    /// Chunk position including dimension.
    pub pos: ChunkPos,
    /// Compatibility observed before migration.
    pub compatibility: CompatibilityLevel,
}

/// Deterministic migration plan built from a read-only compatibility scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldMigrationPlan {
    /// Requested mutation policy.
    pub policy: WritePolicy,
    /// Chunks requiring explicit conversion.
    pub chunks: Vec<ChunkMigrationTarget>,
    /// Actor storage conversion/reconciliation action.
    pub actor_action: ActorMigrationAction,
    /// Blocking conditions that prohibit execution.
    pub blockers: Vec<MigrationBlocker>,
}

impl WorldMigrationPlan {
    /// Builds a migration plan from a compatibility report without touching storage.
    #[must_use]
    pub fn from_report(report: &WorldCompatibilityReport, policy: WritePolicy) -> Self {
        let mut chunks = Vec::new();
        let mut blockers = Vec::new();
        for chunk in &report.chunks {
            match chunk.capabilities.compatibility {
                CompatibilityLevel::MigrationRequired | CompatibilityLevel::ReadCompatible => {
                    if matches!(policy, WritePolicy::Migrate) {
                        chunks.push(ChunkMigrationTarget {
                            pos: chunk.pos,
                            compatibility: chunk.capabilities.compatibility,
                        });
                    }
                }
                CompatibilityLevel::UnsupportedFuture => {
                    blockers.push(MigrationBlocker::FutureChunkData(chunk.pos));
                }
                CompatibilityLevel::Corrupt => {
                    blockers.push(MigrationBlocker::CorruptChunk(chunk.pos));
                }
                CompatibilityLevel::Exact => {}
            }
        }
        let actor_action = classify_actor_migration(report.actor_storage, policy);
        if matches!(actor_action, ActorMigrationAction::Refuse)
            && !matches!(
                report.actor_storage,
                ActorStorageModel::Unknown | ActorStorageModel::ModernDigest
            )
        {
            blockers.push(MigrationBlocker::ActorStorage);
        }
        Self {
            policy,
            chunks,
            actor_action,
            blockers,
        }
    }

    /// Returns whether this plan may proceed without violating preservation guarantees.
    #[must_use]
    pub fn executable(&self) -> bool {
        matches!(self.policy, WritePolicy::Migrate) && self.blockers.is_empty()
    }
}
