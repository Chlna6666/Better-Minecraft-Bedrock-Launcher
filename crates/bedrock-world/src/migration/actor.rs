//! Actor-storage migration between historical inline entity records and modern digest storage.
//!
//! Actor migration belongs to `bedrock-world`: `Entity`, `digp`, `actorprefix`, actor `UniqueID` and
//! Bedrock NBT are Minecraft world semantics rather than LevelDB mechanics.

use crate::audit::{ActorStorageModel, CompatibilityLevel, WritePolicy};
use crate::codec::nbt::{NbtTag, parse_consecutive_root_nbt, serialize_root_nbt};
use crate::error::{BedrockWorldError, Result};
use crate::model::{ActorDigestKey, ActorUid, ChunkKey, ChunkPos, ChunkRecordTag};
use crate::storage::{StorageBatch, WorldStorage};
use bytes::Bytes;
use std::collections::BTreeSet;

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

/// Result of converting one legacy inline actor record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ActorMigrationReport {
    /// Number of inline actor NBT roots converted.
    pub actors_migrated: usize,
    /// Number of existing actorprefix payloads that already matched exactly.
    pub actor_payloads_reused: usize,
    /// Whether an existing digest was merged rather than created from scratch.
    pub merged_existing_digest: bool,
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

/// Converts one chunk's legacy inline `Entity` payload to modern `digp`/`actorprefix` storage.
///
/// The operation is conservative and atomic through [`WorldStorage::write_batch`]. Existing modern
/// actor payloads are reused only when their bytes exactly match. Conflicting payloads, duplicate
/// actor ids and actors without a usable `UniqueID` abort migration without deleting legacy data.
pub fn migrate_inline_actor_chunk_blocking(
    storage: &dyn WorldStorage,
    pos: ChunkPos,
) -> Result<ActorMigrationReport> {
    let legacy_key = ChunkKey::new(pos, ChunkRecordTag::Entity).encode();
    let Some(legacy_payload) = storage.get(&legacy_key)? else {
        return Ok(ActorMigrationReport::default());
    };
    let roots = parse_consecutive_root_nbt(legacy_payload.as_ref())?;
    if roots.is_empty() {
        return Err(BedrockWorldError::CorruptWorld(format!(
            "legacy Entity record for chunk ({}, {}, {}) is empty",
            pos.x,
            pos.z,
            pos.dimension.id()
        )));
    }

    let digest_key = ActorDigestKey::new(pos).storage_key();
    let existing_digest = storage.get(&digest_key)?;
    let mut digest_ids = BTreeSet::<[u8; 8]>::new();
    if let Some(existing) = &existing_digest {
        if existing.len() % 8 != 0 {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "actor digest for chunk ({}, {}, {}) has invalid byte length {}",
                pos.x,
                pos.z,
                pos.dimension.id(),
                existing.len()
            )));
        }
        for raw in existing.chunks_exact(8) {
            let mut uid = [0_u8; 8];
            uid.copy_from_slice(raw);
            digest_ids.insert(uid);
        }
    }

    let mut batch = StorageBatch::new();
    let mut report = ActorMigrationReport {
        merged_existing_digest: existing_digest.is_some(),
        ..ActorMigrationReport::default()
    };

    for root in roots {
        let unique_id = actor_unique_id(&root).ok_or_else(|| {
            BedrockWorldError::Validation(format!(
                "legacy actor in chunk ({}, {}, {}) has no integer UniqueID",
                pos.x,
                pos.z,
                pos.dimension.id()
            ))
        })?;
        let uid = ActorUid::from_unique_id(unique_id);
        let raw_uid = uid.raw_storage_bytes();
        if !digest_ids.insert(raw_uid) && existing_digest.is_none() {
            return Err(BedrockWorldError::Validation(format!(
                "legacy actor record contains duplicate UniqueID {unique_id}"
            )));
        }
        let actor_value = Bytes::from(serialize_root_nbt(&root)?);
        let actor_key = uid.storage_key();
        if let Some(existing) = storage.get(&actor_key)? {
            if existing != actor_value {
                return Err(BedrockWorldError::ConcurrentWrite(format!(
                    "actorprefix collision while migrating UniqueID {unique_id}"
                )));
            }
            report.actor_payloads_reused = report.actor_payloads_reused.saturating_add(1);
        } else {
            batch.put(actor_key, actor_value);
        }
        report.actors_migrated = report.actors_migrated.saturating_add(1);
    }

    let mut digest = Vec::with_capacity(digest_ids.len() * 8);
    for uid in digest_ids {
        digest.extend_from_slice(&uid);
    }
    batch.put(digest_key, Bytes::from(digest));
    batch.delete(legacy_key);
    storage.write_batch(&batch)?;
    Ok(report)
}

fn actor_unique_id(root: &NbtTag) -> Option<i64> {
    let NbtTag::Compound(values) = root else {
        return None;
    };
    match values.get("UniqueID").or_else(|| values.get("uniqueID"))? {
        NbtTag::Long(value) => Some(*value),
        NbtTag::Int(value) => Some(i64::from(*value)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::nbt::NbtTag;
    use crate::model::Dimension;
    use crate::storage::{MemoryStorage, WorldStorage};
    use indexmap::IndexMap;

    #[test]
    fn inline_actor_migration_writes_digest_and_removes_legacy_record() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos { x: 1, z: -2, dimension: Dimension::Overworld };
        let root = NbtTag::Compound(IndexMap::from([
            ("identifier".to_string(), NbtTag::String("minecraft:pig".to_string())),
            ("UniqueID".to_string(), NbtTag::Long(0x0000_0002_1234_5678_i64)),
        ]));
        let legacy_key = ChunkKey::new(pos, ChunkRecordTag::Entity).encode();
        storage.put(&legacy_key, &serialize_root_nbt(&root).unwrap()).unwrap();

        let report = migrate_inline_actor_chunk_blocking(&storage, pos).unwrap();
        assert_eq!(report.actors_migrated, 1);
        assert!(storage.get(&legacy_key).unwrap().is_none());
        let uid = ActorUid::from_unique_id(0x0000_0002_1234_5678_i64);
        assert!(storage.get(&uid.storage_key()).unwrap().is_some());
        assert_eq!(
            storage.get(&ActorDigestKey::new(pos).storage_key()).unwrap().unwrap().as_ref(),
            uid.raw_storage_bytes().as_slice()
        );
    }
}
