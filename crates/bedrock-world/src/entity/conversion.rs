//! Explicit conversion between Minecraft Bedrock actor-storage representations.
//!
//! Historical inline `Entity` records and modern `digp`/`actorprefix` storage are both supported
//! persisted representations. Normal reads do not convert either form.

use crate::chunk::{ChunkKey, ChunkPos, ChunkRecordTag};
use crate::database::{StorageBatch, WorldStorage};
use crate::entity::{ActorDigestKey, ActorUid};
use crate::error::{BedrockWorldError, Result};
use crate::integrity::{ActorStorageModel, CompatibilityLevel};
use crate::nbt::{NbtTag, parse_consecutive_root_nbt, serialize_root_nbt};
use crate::version::ConversionCompatibility;
use bytes::Bytes;
use std::collections::BTreeSet;

/// Explicit target actor-storage representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorStorageTarget {
    /// Consecutive actor NBT roots stored in the chunk `Entity` record.
    Inline,
    /// `digp<ChunkKey>` digest plus `actorprefix<ActorUid>` payload records.
    Digest,
}

/// Conversion selected from observed actor storage and an explicit target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActorStorageConversion {
    /// Source already uses the requested storage representation.
    None,
    /// Convert inline chunk actors to digest/payload storage.
    InlineToDigest,
    /// Convert one chunk digest back to an inline `Entity` record.
    DigestToInline,
    /// Both representations are present and require caller-directed reconciliation.
    ReconcileMixed,
    /// Source storage cannot be established safely.
    Unsupported,
}

/// Result of one explicit actor-storage conversion.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ActorStorageConversionReport {
    /// Number of actor payloads represented in the target chunk storage.
    pub actors_converted: usize,
    /// Number of target payload/records that already matched exactly.
    pub target_records_reused: usize,
    /// Number of actorprefix payloads intentionally retained during digest -> inline conversion.
    ///
    /// Actor payload records can outlive an individual digest reference, so this converter never
    /// deletes them without a whole-world reference analysis.
    pub actorprefix_payloads_retained: usize,
}

/// Classifies a requested actor-storage conversion without mutating storage.
#[must_use]
pub const fn classify_actor_storage_conversion(
    source: ActorStorageModel,
    target: ActorStorageTarget,
) -> ActorStorageConversion {
    match (source, target) {
        (ActorStorageModel::LegacyInline, ActorStorageTarget::Inline)
        | (ActorStorageModel::ModernDigest, ActorStorageTarget::Digest) => ActorStorageConversion::None,
        (ActorStorageModel::LegacyInline, ActorStorageTarget::Digest) => {
            ActorStorageConversion::InlineToDigest
        }
        (ActorStorageModel::ModernDigest, ActorStorageTarget::Inline) => {
            ActorStorageConversion::DigestToInline
        }
        (ActorStorageModel::Mixed, _) => ActorStorageConversion::ReconcileMixed,
        (ActorStorageModel::Unknown, _) => ActorStorageConversion::Unsupported,
    }
}

/// Reports semantic conversion support for actor-storage representations.
#[must_use]
pub const fn actor_storage_conversion_compatibility(
    source: ActorStorageModel,
    target: ActorStorageTarget,
) -> ConversionCompatibility {
    match classify_actor_storage_conversion(source, target) {
        ActorStorageConversion::None
        | ActorStorageConversion::InlineToDigest
        | ActorStorageConversion::DigestToInline => ConversionCompatibility::Lossless,
        ActorStorageConversion::ReconcileMixed | ActorStorageConversion::Unsupported => {
            ConversionCompatibility::Unsupported
        }
    }
}

/// Compatibility implied by an observed actor storage population.
///
/// Both known Bedrock actor-storage generations are first-class readable representations.
#[must_use]
pub const fn actor_storage_compatibility(storage: ActorStorageModel) -> CompatibilityLevel {
    match storage {
        ActorStorageModel::LegacyInline | ActorStorageModel::ModernDigest => CompatibilityLevel::Exact,
        ActorStorageModel::Mixed | ActorStorageModel::Unknown => CompatibilityLevel::ReadCompatible,
    }
}

/// Converts one chunk's inline `Entity` payload to `digp`/`actorprefix` storage.
///
/// Existing actor payloads are reused only when bytes match exactly. Conflicts or actors without a
/// usable `UniqueID` abort before the legacy chunk record is removed.
pub fn convert_inline_actor_chunk_to_digest_blocking(
    storage: &dyn WorldStorage,
    pos: ChunkPos,
) -> Result<ActorStorageConversionReport> {
    let inline_key = ChunkKey::new(pos, ChunkRecordTag::Entity).encode();
    let Some(inline_payload) = storage.get(&inline_key)? else {
        return Ok(ActorStorageConversionReport::default());
    };
    let roots = parse_consecutive_root_nbt(inline_payload.as_ref())?;
    if roots.is_empty() {
        return Err(BedrockWorldError::CorruptWorld(format!(
            "inline Entity record for chunk ({}, {}, {}) is empty",
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
    let mut report = ActorStorageConversionReport::default();
    for root in roots {
        let unique_id = actor_unique_id(&root).ok_or_else(|| {
            BedrockWorldError::Validation(format!(
                "inline actor in chunk ({}, {}, {}) has no integer UniqueID",
                pos.x,
                pos.z,
                pos.dimension.id()
            ))
        })?;
        let uid = ActorUid::from_unique_id(unique_id);
        let raw_uid = uid.raw_storage_bytes();
        if !digest_ids.insert(raw_uid) && existing_digest.is_none() {
            return Err(BedrockWorldError::Validation(format!(
                "inline actor record contains duplicate UniqueID {unique_id}"
            )));
        }
        let actor_value = Bytes::from(serialize_root_nbt(&root)?);
        let actor_key = uid.storage_key();
        if let Some(existing) = storage.get(&actor_key)? {
            if existing != actor_value {
                return Err(BedrockWorldError::ConcurrentWrite(format!(
                    "actorprefix collision while converting UniqueID {unique_id}"
                )));
            }
            report.target_records_reused = report.target_records_reused.saturating_add(1);
        } else {
            batch.put(actor_key, actor_value);
        }
        report.actors_converted = report.actors_converted.saturating_add(1);
    }

    let mut digest = Vec::with_capacity(digest_ids.len() * 8);
    for uid in digest_ids {
        digest.extend_from_slice(&uid);
    }
    batch.put(digest_key, Bytes::from(digest));
    batch.delete(inline_key);
    storage.write_batch(&batch)?;
    Ok(report)
}

/// Converts one chunk's `digp` references back to a consecutive inline `Entity` record.
///
/// `actorprefix` payloads are retained because deleting them safely requires proving that no other
/// digest references them. The chunk digest itself is removed after the inline target record is
/// validated/staged.
pub fn convert_digest_actor_chunk_to_inline_blocking(
    storage: &dyn WorldStorage,
    pos: ChunkPos,
) -> Result<ActorStorageConversionReport> {
    let digest_key = ActorDigestKey::new(pos).storage_key();
    let Some(digest) = storage.get(&digest_key)? else {
        return Ok(ActorStorageConversionReport::default());
    };
    if digest.len() % 8 != 0 {
        return Err(BedrockWorldError::CorruptWorld(format!(
            "actor digest for chunk ({}, {}, {}) has invalid byte length {}",
            pos.x,
            pos.z,
            pos.dimension.id(),
            digest.len()
        )));
    }

    let mut inline = Vec::new();
    let mut seen = BTreeSet::<[u8; 8]>::new();
    let mut report = ActorStorageConversionReport::default();
    for raw in digest.chunks_exact(8) {
        let mut uid_bytes = [0_u8; 8];
        uid_bytes.copy_from_slice(raw);
        if !seen.insert(uid_bytes) {
            return Err(BedrockWorldError::CorruptWorld(
                "actor digest contains duplicate actor storage ids".to_string(),
            ));
        }
        let uid = ActorUid(i64::from_le_bytes(uid_bytes));
        let actor_key = uid.storage_key();
        let actor = storage.get(&actor_key)?.ok_or_else(|| {
            BedrockWorldError::CorruptWorld(format!(
                "actor digest references missing actor payload {:?}", uid_bytes
            ))
        })?;
        let roots = parse_consecutive_root_nbt(actor.as_ref())?;
        if roots.len() != 1 {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "actorprefix payload {:?} contains {} NBT roots instead of one",
                uid_bytes,
                roots.len()
            )));
        }
        inline.extend_from_slice(actor.as_ref());
        report.actors_converted = report.actors_converted.saturating_add(1);
        report.actorprefix_payloads_retained =
            report.actorprefix_payloads_retained.saturating_add(1);
    }

    let inline_key = ChunkKey::new(pos, ChunkRecordTag::Entity).encode();
    let inline = Bytes::from(inline);
    let mut batch = StorageBatch::new();
    if let Some(existing) = storage.get(&inline_key)? {
        if existing != inline {
            return Err(BedrockWorldError::ConcurrentWrite(
                "inline Entity target already exists with different bytes".to_string(),
            ));
        }
        report.target_records_reused = report.target_records_reused.saturating_add(1);
    } else {
        batch.put(inline_key, inline);
    }
    batch.delete(digest_key);
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
    use crate::chunk::Dimension;
    use crate::database::{MemoryStorage, WorldStorage};
    use indexmap::IndexMap;

    #[test]
    fn inline_and_digest_conversion_roundtrip_actor_payload() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos { x: 1, z: -2, dimension: Dimension::Overworld };
        let root = NbtTag::Compound(IndexMap::from([
            ("identifier".to_string(), NbtTag::String("minecraft:pig".to_string())),
            ("UniqueID".to_string(), NbtTag::Long(0x0000_0002_1234_5678_i64)),
        ]));
        let inline_key = ChunkKey::new(pos, ChunkRecordTag::Entity).encode();
        let original = Bytes::from(serialize_root_nbt(&root).unwrap());
        storage.put(&inline_key, &original).unwrap();

        convert_inline_actor_chunk_to_digest_blocking(&storage, pos).unwrap();
        assert!(storage.get(&inline_key).unwrap().is_none());
        convert_digest_actor_chunk_to_inline_blocking(&storage, pos).unwrap();
        assert_eq!(storage.get(&inline_key).unwrap(), Some(original));
    }
}
