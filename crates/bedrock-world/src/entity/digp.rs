//! Minecraft Bedrock `digp<ChunkKey>` and `actorprefix<ActorUid>` actor records.

use crate::chunk::{ChunkKey, ChunkPos, ChunkRecordTag};
use crate::database::{StorageBatch, WorldStorage};
use crate::entity::{ActorDigestKey, ActorUid};
use crate::error::{BedrockWorldError, Result};
use crate::nbt::{NbtTag, parse_consecutive_root_nbt, serialize_root_nbt};
use bytes::Bytes;
use std::collections::BTreeSet;

/// Summary of writing one chunk between `Entity` and `digp`/`actorprefix` representations.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ActorRecordWriteReport {
    /// Number of actors written or represented by the target record.
    pub actors: usize,
    /// Number of already-existing target records whose bytes matched exactly.
    pub reused: usize,
    /// Number of `actorprefix` payloads intentionally retained after writing an `Entity` record.
    pub retained_actorprefix: usize,
}

/// Writes one chunk's inline `Entity` actors as `digp` plus `actorprefix` records.
pub fn write_digp_from_entity(
    storage: &dyn WorldStorage,
    pos: ChunkPos,
) -> Result<ActorRecordWriteReport> {
    let entity_key = ChunkKey::new(pos, ChunkRecordTag::Entity).encode();
    let Some(entity_payload) = storage.get(&entity_key)? else {
        return Ok(ActorRecordWriteReport::default());
    };
    let roots = parse_consecutive_root_nbt(entity_payload.as_ref())?;
    if roots.is_empty() {
        return Err(BedrockWorldError::CorruptWorld(format!(
            "Entity record for chunk ({}, {}, {}) is empty",
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
                "digp for chunk ({}, {}, {}) has invalid byte length {}",
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
    let mut report = ActorRecordWriteReport::default();
    for root in roots {
        let unique_id = actor_unique_id(&root).ok_or_else(|| {
            BedrockWorldError::Validation(format!(
                "Entity actor in chunk ({}, {}, {}) has no integer UniqueID",
                pos.x,
                pos.z,
                pos.dimension.id()
            ))
        })?;
        let uid = ActorUid::from_unique_id(unique_id);
        let raw_uid = uid.raw_storage_bytes();
        if !digest_ids.insert(raw_uid) && existing_digest.is_none() {
            return Err(BedrockWorldError::Validation(format!(
                "Entity record contains duplicate UniqueID {unique_id}"
            )));
        }
        let actor_value = Bytes::from(serialize_root_nbt(&root)?);
        let actor_key = uid.storage_key();
        if let Some(existing) = storage.get(&actor_key)? {
            if existing != actor_value {
                return Err(BedrockWorldError::ConcurrentWrite(format!(
                    "actorprefix collision for UniqueID {unique_id}"
                )));
            }
            report.reused = report.reused.saturating_add(1);
        } else {
            batch.put(actor_key, actor_value);
        }
        report.actors = report.actors.saturating_add(1);
    }

    let mut digest = Vec::with_capacity(digest_ids.len() * 8);
    for uid in digest_ids {
        digest.extend_from_slice(&uid);
    }
    batch.put(digest_key, Bytes::from(digest));
    batch.delete(entity_key);
    storage.write_batch(&batch)?;
    Ok(report)
}

/// Writes one chunk's `digp` actor list back to its inline `Entity` record.
///
/// `actorprefix` records are retained because safe deletion requires a whole-world reference scan.
pub fn write_entity_from_digp(
    storage: &dyn WorldStorage,
    pos: ChunkPos,
) -> Result<ActorRecordWriteReport> {
    let digest_key = ActorDigestKey::new(pos).storage_key();
    let Some(digest) = storage.get(&digest_key)? else {
        return Ok(ActorRecordWriteReport::default());
    };
    if digest.len() % 8 != 0 {
        return Err(BedrockWorldError::CorruptWorld(format!(
            "digp for chunk ({}, {}, {}) has invalid byte length {}",
            pos.x,
            pos.z,
            pos.dimension.id(),
            digest.len()
        )));
    }

    let mut entity = Vec::new();
    let mut seen = BTreeSet::<[u8; 8]>::new();
    let mut report = ActorRecordWriteReport::default();
    for raw in digest.chunks_exact(8) {
        let mut uid_bytes = [0_u8; 8];
        uid_bytes.copy_from_slice(raw);
        if !seen.insert(uid_bytes) {
            return Err(BedrockWorldError::CorruptWorld(
                "digp contains duplicate actor storage ids".to_string(),
            ));
        }
        let uid = ActorUid(i64::from_le_bytes(uid_bytes));
        let actor = storage.get(&uid.storage_key())?.ok_or_else(|| {
            BedrockWorldError::CorruptWorld(format!(
                "digp references missing actorprefix {:?}", uid_bytes
            ))
        })?;
        let roots = parse_consecutive_root_nbt(actor.as_ref())?;
        if roots.len() != 1 {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "actorprefix {:?} contains {} NBT roots instead of one",
                uid_bytes,
                roots.len()
            )));
        }
        entity.extend_from_slice(actor.as_ref());
        report.actors = report.actors.saturating_add(1);
        report.retained_actorprefix = report.retained_actorprefix.saturating_add(1);
    }

    let entity_key = ChunkKey::new(pos, ChunkRecordTag::Entity).encode();
    let entity = Bytes::from(entity);
    let mut batch = StorageBatch::new();
    if let Some(existing) = storage.get(&entity_key)? {
        if existing != entity {
            return Err(BedrockWorldError::ConcurrentWrite(
                "Entity target already exists with different bytes".to_string(),
            ));
        }
        report.reused = report.reused.saturating_add(1);
    } else {
        batch.put(entity_key, entity);
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
