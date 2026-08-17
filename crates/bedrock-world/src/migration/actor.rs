//! Actor-storage migration policy.
//!
//! Actor migration belongs to `bedrock-world` because `digp`, `actorprefix`, entity NBT and actor
//! identity are Minecraft semantics rather than LevelDB mechanics.

use crate::{ActorStorageModel, CompatibilityLevel, WritePolicy};

/// Required action for an observed actor storage population.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorMigrationAction {
    /// No actor-format conversion is required.
    None,
    /// Convert legacy inline chunk `Entity` records to modern digest/payload storage.
    InlineToDigest,
    /// Reconcile a mixed inline/digest population before destructive actor writes.
    ReconcileMixed,
    /// No safe actor migration can be selected from the available evidence.
    Refuse,
}

/// Classifies actor migration without mutating storage.
#[must_use]
pub const fn classify_actor_migration(
    storage: ActorStorageModel,
    policy: WritePolicy,
) -> ActorMigrationAction {
    if matches!(policy, WritePolicy::Refuse | WritePolicy::Preserve) {
        return match storage {
            ActorStorageModel::ModernDigest | ActorStorageModel::Unknown => ActorMigrationAction::None,
            ActorStorageModel::LegacyInline | ActorStorageModel::Mixed => ActorMigrationAction::Refuse,
        };
    }
    match storage {
        ActorStorageModel::Unknown | ActorStorageModel::ModernDigest => ActorMigrationAction::None,
        ActorStorageModel::LegacyInline => ActorMigrationAction::InlineToDigest,
        ActorStorageModel::Mixed => ActorMigrationAction::ReconcileMixed,
    }
}

/// Compatibility implied by an actor storage population.
#[must_use]
pub const fn actor_storage_compatibility(storage: ActorStorageModel) -> CompatibilityLevel {
    match storage {
        ActorStorageModel::ModernDigest => CompatibilityLevel::Exact,
        ActorStorageModel::LegacyInline | ActorStorageModel::Mixed => CompatibilityLevel::MigrationRequired,
        ActorStorageModel::Unknown => CompatibilityLevel::ReadCompatible,
    }
}
