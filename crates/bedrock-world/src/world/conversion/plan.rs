//! Whole-world explicit conversion planning derived from observed Bedrock records.

use crate::chunk::ChunkPos;
use crate::entity::{
    ActorStorageConversion, ActorStorageTarget, classify_actor_storage_conversion,
};
use crate::integrity::{ActorStorageModel, CompatibilityLevel, WorldCompatibilityReport};

/// A reason an explicit whole-world conversion cannot proceed safely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversionBlocker {
    /// Future/unknown chunk data is present and must be preserved raw.
    FutureChunkData(ChunkPos),
    /// Corrupt chunk data must be repaired before conversion.
    CorruptChunk(ChunkPos),
    /// Actor storage evidence cannot be converted automatically to the requested target.
    ActorStorage,
}

/// One chunk selected for caller-requested historical format conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkConversionTarget {
    /// Chunk position including dimension.
    pub pos: ChunkPos,
    /// Compatibility observed before conversion.
    pub compatibility: CompatibilityLevel,
}

/// Deterministic explicit conversion plan built from a read-only compatibility scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldConversionPlan {
    /// Chunks containing non-exact data for which a domain conversion may be required.
    pub chunks: Vec<ChunkConversionTarget>,
    /// Requested actor-storage conversion.
    pub actor_conversion: ActorStorageConversion,
    /// Conditions that prohibit automatic execution of this plan.
    pub blockers: Vec<ConversionBlocker>,
}

impl WorldConversionPlan {
    /// Builds a conversion plan without touching storage.
    #[must_use]
    pub fn from_report(
        report: &WorldCompatibilityReport,
        actor_target: ActorStorageTarget,
    ) -> Self {
        let mut chunks = Vec::new();
        let mut blockers = Vec::new();
        for chunk in &report.chunks {
            match chunk.capabilities.compatibility {
                CompatibilityLevel::MigrationRequired | CompatibilityLevel::ReadCompatible => {
                    chunks.push(ChunkConversionTarget {
                        pos: chunk.pos,
                        compatibility: chunk.capabilities.compatibility,
                    });
                }
                CompatibilityLevel::UnsupportedFuture => {
                    blockers.push(ConversionBlocker::FutureChunkData(chunk.pos));
                }
                CompatibilityLevel::Corrupt => {
                    blockers.push(ConversionBlocker::CorruptChunk(chunk.pos));
                }
                CompatibilityLevel::Exact => {}
            }
        }
        let actor_conversion = classify_actor_storage_conversion(report.actor_storage, actor_target);
        if matches!(
            actor_conversion,
            ActorStorageConversion::ReconcileMixed | ActorStorageConversion::Unsupported
        ) && !matches!(report.actor_storage, ActorStorageModel::Unknown)
        {
            blockers.push(ConversionBlocker::ActorStorage);
        }
        Self {
            chunks,
            actor_conversion,
            blockers,
        }
    }

    /// Returns whether no blocker prevents execution.
    #[must_use]
    pub fn executable(&self) -> bool {
        self.blockers.is_empty()
    }
}
