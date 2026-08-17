//! World upgrade planning derived from observed records rather than a single global version.

use crate::chunk::ChunkPos;
use crate::integrity::{
    ActorStorageModel, CompatibilityLevel, WorldCompatibilityReport, WritePolicy,
};
use crate::upgrade::{ActorMigrationAction, classify_actor_migration};

/// A reason a world cannot be upgraded destructively without additional authoritative information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationBlocker {
    /// Future/unknown chunk data is present and must be preserved raw.
    FutureChunkData(ChunkPos),
    /// Corrupt chunk data must be repaired before migration.
    CorruptChunk(ChunkPos),
    /// Actor storage evidence cannot be reconciled automatically.
    ActorStorage,
}

/// One chunk selected for historical format upgrade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkMigrationTarget {
    /// Chunk position including dimension.
    pub pos: ChunkPos,
    /// Compatibility observed before upgrade.
    pub compatibility: CompatibilityLevel,
}

/// Deterministic upgrade plan built from a read-only compatibility scan.
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
    /// Builds an upgrade plan from a compatibility report without touching storage.
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
            && !matches!(report.actor_storage, ActorStorageModel::Unknown | ActorStorageModel::ModernDigest)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuse_policy_never_produces_executable_plan() {
        let report = WorldCompatibilityReport {
            world: crate::world::WorldFormat::LevelDb.capabilities(),
            compatibility: CompatibilityLevel::Exact,
            actor_storage: ActorStorageModel::ModernDigest,
            records_scanned: 0,
            chunks_scanned: 0,
            exact_chunks: 0,
            read_compatible_chunks: 0,
            migration_required_chunks: 0,
            unsupported_future_chunks: 0,
            corrupt_chunks: 0,
            actor_digest_records: 0,
            actor_prefix_records: 0,
            legacy_entity_records: 0,
            unknown_chunk_records: 0,
            unknown_storage_keys: 0,
            subchunk_codecs: Default::default(),
            chunks: Vec::new(),
        };
        assert!(!WorldMigrationPlan::from_report(&report, WritePolicy::Refuse).executable());
    }
}
