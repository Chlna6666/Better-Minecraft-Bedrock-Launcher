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

/// Writes one chunk's inline `Entity` actors as the exact `digp` plus `actorprefix` representation.
///
/// Actor order follows the consecutive source NBT roots. Existing target records are reusable only
/// when their bytes match the source-derived target exactly; extra UIDs in a pre-existing `digp` are
/// treated as a conflicting mixed-storage state rather than silently merged into the conversion.
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
    let mut seen = BTreeSet::<[u8; 8]>::new();
    let mut digest = Vec::with_capacity(roots.len().saturating_mul(8));
    let mut actor_values = Vec::<(Bytes, Bytes, i64)>::with_capacity(roots.len());

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
        if !seen.insert(raw_uid) {
            return Err(BedrockWorldError::Validation(format!(
                "Entity record contains duplicate UniqueID {unique_id}"
            )));
        }
        digest.extend_from_slice(&raw_uid);
        actor_values.push((
            uid.storage_key(),
            Bytes::from(serialize_root_nbt(&root)?),
            unique_id,
        ));
    }

    let digest_value = Bytes::from(digest);
    let existing_digest = storage.get(&digest_key)?;
    if let Some(existing) = &existing_digest
        && existing != &digest_value
    {
        return Err(BedrockWorldError::ConcurrentWrite(format!(
            "digp target for chunk ({}, {}, {}) already exists with different actor ids",
            pos.x,
            pos.z,
            pos.dimension.id()
        )));
    }

    let mut batch = StorageBatch::new();
    let mut report = ActorRecordWriteReport {
        actors: actor_values.len(),
        ..ActorRecordWriteReport::default()
    };
    for (actor_key, actor_value, unique_id) in actor_values {
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
    }

    if existing_digest.is_some() {
        report.reused = report.reused.saturating_add(1);
    } else {
        batch.put(digest_key, digest_value);
    }
    batch.delete(entity_key);
    storage.write_batch(&batch)?;
    Ok(report)
}

/// Writes one chunk's `digp` actor list back to its exact inline `Entity` record.
///
/// `actorprefix` records are retained because safe deletion requires a whole-world reference scan.
/// An empty digest converts to the absence of an `Entity` record, not an invalid zero-byte payload.
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

    let entity_key = ChunkKey::new(pos, ChunkRecordTag::Entity).encode();
    if digest.is_empty() {
        if let Some(existing) = storage.get(&entity_key)?
            && !existing.is_empty()
        {
            return Err(BedrockWorldError::ConcurrentWrite(
                "empty digp cannot replace an existing non-empty Entity record".to_string(),
            ));
        }
        let mut batch = StorageBatch::new();
        batch.delete(entity_key);
        batch.delete(digest_key);
        storage.write_batch(&batch)?;
        return Ok(ActorRecordWriteReport::default());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::Dimension;
    use crate::database::MemoryStorage;
    use indexmap::IndexMap;

    fn actor(unique_id: i64) -> NbtTag {
        NbtTag::Compound(IndexMap::from([(
            "UniqueID".to_string(),
            NbtTag::Long(unique_id),
        )]))
    }

    fn pos() -> ChunkPos {
        ChunkPos {
            x: 1,
            z: -2,
            dimension: Dimension::Overworld,
        }
    }

    #[test]
    fn entity_to_digp_preserves_source_order() {
        let storage = MemoryStorage::new();
        let pos = pos();
        let entity_key = ChunkKey::new(pos, ChunkRecordTag::Entity).encode();
        let mut raw = serialize_root_nbt(&actor(9)).unwrap();
        raw.extend_from_slice(&serialize_root_nbt(&actor(2)).unwrap());
        storage.put(entity_key.as_ref(), &raw).unwrap();

        write_digp_from_entity(&storage, pos).unwrap();
        let digest = storage
            .get(&ActorDigestKey::new(pos).storage_key())
            .unwrap()
            .unwrap();
        let mut expected = Vec::new();
        expected.extend_from_slice(&ActorUid::from_unique_id(9).raw_storage_bytes());
        expected.extend_from_slice(&ActorUid::from_unique_id(2).raw_storage_bytes());
        assert_eq!(digest.as_ref(), expected.as_slice());
    }

    #[test]
    fn conflicting_existing_digest_is_not_merged() {
        let storage = MemoryStorage::new();
        let pos = pos();
        let entity_key = ChunkKey::new(pos, ChunkRecordTag::Entity).encode();
        storage
            .put(entity_key.as_ref(), &serialize_root_nbt(&actor(9)).unwrap())
            .unwrap();
        storage
            .put(
                &ActorDigestKey::new(pos).storage_key(),
                &ActorUid::from_unique_id(99).raw_storage_bytes(),
            )
            .unwrap();

        assert!(write_digp_from_entity(&storage, pos).is_err());
        assert!(storage.get(entity_key.as_ref()).unwrap().is_some());
    }

    #[test]
    fn empty_digp_does_not_create_empty_entity_record() {
        let storage = MemoryStorage::new();
        let pos = pos();
        storage
            .put(&ActorDigestKey::new(pos).storage_key(), &[])
            .unwrap();
        write_entity_from_digp(&storage, pos).unwrap();
        let entity_key = ChunkKey::new(pos, ChunkRecordTag::Entity).encode();
        assert!(storage.get(entity_key.as_ref()).unwrap().is_none());
        assert!(
            storage
                .get(&ActorDigestKey::new(pos).storage_key())
                .unwrap()
                .is_none()
        );
    }
}
