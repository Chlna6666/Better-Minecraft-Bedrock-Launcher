//! Whole-world Minecraft Bedrock actor storage rewrites.
//!
//! `Entity` and `digp`/`actorprefix` are real Bedrock storage generations. This module preflights
//! every affected record before producing one batch so callers can switch actor storage atomically.

use crate::chunk::{BedrockDbKey, ChunkKey, ChunkPos, ChunkRecordTag};
use crate::database::{
    StorageBatch, StorageReadOptions, StorageVisitorControl, WorldStorage,
};
use crate::entity::{ActorDigestKey, ActorUid};
use crate::error::{BedrockWorldError, Result};
use crate::nbt::{NbtTag, parse_consecutive_root_nbt, serialize_root_nbt};
use crate::parsed::{encode_actor_digest_ids, parse_actor_digest_ids};
use bytes::Bytes;
use std::collections::{BTreeMap, BTreeSet};

/// Summary of an atomic whole-world actor storage rewrite.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ActorStorageRewriteReport {
    /// Number of chunk actor records rewritten.
    pub chunks: usize,
    /// Number of actors represented by the rewritten source records.
    pub actors: usize,
    /// `Entity` records written by a modern-to-legacy rewrite.
    pub entity_records_written: usize,
    /// `Entity` records deleted by a legacy-to-modern rewrite.
    pub entity_records_deleted: usize,
    /// `digp` records written by a legacy-to-modern rewrite.
    pub digp_records_written: usize,
    /// `digp` records deleted by a modern-to-legacy rewrite.
    pub digp_records_deleted: usize,
    /// `actorprefix` records created by a legacy-to-modern rewrite.
    pub actorprefix_records_written: usize,
    /// Referenced `actorprefix` records removed by a modern-to-legacy rewrite.
    pub actorprefix_records_deleted: usize,
    /// Existing `actorprefix` records reused because their NBT matched the inline actor exactly.
    pub actorprefix_records_reused: usize,
    /// Unreferenced `actorprefix` records retained because no `digp` record proves ownership.
    pub orphan_actorprefix_records_retained: usize,
}

/// Preflights every inline `Entity` record and stages one atomic rewrite to `digp`/`actorprefix`.
///
/// Existing modern records are merged only when they refer to the same chunk and contain identical
/// actor NBT. No storage mutation occurs until the returned batch is committed.
pub(crate) fn stage_world_entity_to_digp_actorprefix(
    storage: &dyn WorldStorage,
) -> Result<(StorageBatch, ActorStorageRewriteReport)> {
    let snapshot = scan_actor_storage(storage)?;
    if snapshot.entities.is_empty() {
        return Ok((StorageBatch::new(), ActorStorageRewriteReport::default()));
    }

    let mut digest_ids = BTreeMap::<ChunkPos, Vec<ActorUid>>::new();
    let mut actor_owner = BTreeMap::<ActorUid, ChunkPos>::new();
    for (pos, raw) in &snapshot.digests {
        let ids = parse_actor_digest_ids(raw)?;
        let mut local = BTreeSet::new();
        for uid in &ids {
            if !local.insert(*uid) {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "digp for chunk {pos:?} contains duplicate actor storage id {uid:?}"
                )));
            }
            if let Some(previous) = actor_owner.insert(*uid, *pos)
                && previous != *pos
            {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "actor storage id {uid:?} is referenced by both {previous:?} and {pos:?}"
                )));
            }
        }
        digest_ids.insert(*pos, ids);
    }

    let mut inline_roots = BTreeMap::<ActorUid, NbtTag>::new();
    let mut actor_count = 0usize;
    for (pos, raw) in &snapshot.entities {
        let roots = parse_consecutive_root_nbt(raw)?;
        if roots.is_empty() {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "Entity record for chunk {pos:?} is empty"
            )));
        }
        let ids = digest_ids.entry(*pos).or_default();
        let mut local = BTreeSet::new();
        for root in roots {
            let unique_id = actor_unique_id(&root).ok_or_else(|| {
                BedrockWorldError::CorruptWorld(format!(
                    "Entity actor in chunk {pos:?} has no integer UniqueID"
                ))
            })?;
            let uid = ActorUid::from_unique_id(unique_id);
            if !local.insert(uid) {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "Entity record for chunk {pos:?} contains duplicate UniqueID {unique_id}"
                )));
            }
            if let Some(previous) = actor_owner.get(&uid).copied()
                && previous != *pos
            {
                return Err(BedrockWorldError::ConcurrentWrite(format!(
                    "actor UniqueID {unique_id} belongs to {previous:?} and cannot also be written to {pos:?}"
                )));
            }
            actor_owner.insert(uid, *pos);
            if !ids.contains(&uid) {
                ids.push(uid);
            }
            if inline_roots.insert(uid, root).is_some() {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "actor UniqueID {unique_id} appears in multiple inline Entity records"
                )));
            }
            actor_count = actor_count.saturating_add(1);
        }
    }

    let target_uids = inline_roots.keys().copied().collect::<Vec<_>>();
    let actor_keys = target_uids
        .iter()
        .map(|uid| uid.storage_key())
        .collect::<Vec<_>>();
    let existing_actor_values = storage.get_many(&actor_keys)?;

    let mut batch = StorageBatch::new();
    let mut report = ActorStorageRewriteReport {
        chunks: snapshot.entities.len(),
        actors: actor_count,
        entity_records_deleted: snapshot.entities.len(),
        ..ActorStorageRewriteReport::default()
    };

    for (uid, existing) in target_uids.iter().copied().zip(existing_actor_values) {
        let root = inline_roots.get(&uid).ok_or_else(|| {
            BedrockWorldError::CorruptWorld(format!(
                "missing preflight inline actor root for storage id {uid:?}"
            ))
        })?;
        if let Some(existing) = existing {
            let existing_roots = parse_consecutive_root_nbt(&existing)?;
            if existing_roots.len() != 1 || existing_roots.first() != Some(root) {
                return Err(BedrockWorldError::ConcurrentWrite(format!(
                    "actorprefix collision for storage id {uid:?}"
                )));
            }
            report.actorprefix_records_reused =
                report.actorprefix_records_reused.saturating_add(1);
        } else {
            batch.put(uid.storage_key(), Bytes::from(serialize_root_nbt(root)?));
            report.actorprefix_records_written =
                report.actorprefix_records_written.saturating_add(1);
        }
    }

    for pos in snapshot.entities.keys().copied() {
        let ids = digest_ids.get(&pos).ok_or_else(|| {
            BedrockWorldError::CorruptWorld(format!(
                "missing staged digp actor list for chunk {pos:?}"
            ))
        })?;
        if ids.is_empty() {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "staged digp for chunk {pos:?} would be empty"
            )));
        }
        let encoded = encode_actor_digest_ids(ids);
        if snapshot.digests.get(&pos) != Some(&encoded) {
            batch.put(ActorDigestKey::new(pos).storage_key(), encoded);
            report.digp_records_written = report.digp_records_written.saturating_add(1);
        }
        batch.delete(ChunkKey::new(pos, ChunkRecordTag::Entity).encode());
    }

    Ok((batch, report))
}

/// Preflights every `digp` record and stages one atomic rewrite to chunk-scoped `Entity` records.
///
/// Every referenced `actorprefix` is deleted only because every `digp` record is converted in the
/// same batch. Unreferenced `actorprefix` records are retained and reported rather than guessed to be
/// safe to delete.
pub(crate) fn stage_world_digp_actorprefix_to_entity(
    storage: &dyn WorldStorage,
) -> Result<(StorageBatch, ActorStorageRewriteReport)> {
    let snapshot = scan_actor_storage(storage)?;
    if snapshot.digests.is_empty() {
        return Ok((StorageBatch::new(), ActorStorageRewriteReport::default()));
    }

    let mut digest_ids = BTreeMap::<ChunkPos, Vec<ActorUid>>::new();
    let mut actor_owner = BTreeMap::<ActorUid, ChunkPos>::new();
    let mut referenced = Vec::<ActorUid>::new();
    for (pos, raw) in &snapshot.digests {
        let ids = parse_actor_digest_ids(raw)?;
        if ids.is_empty() {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "digp for chunk {pos:?} is empty"
            )));
        }
        let mut local = BTreeSet::new();
        for uid in &ids {
            if !local.insert(*uid) {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "digp for chunk {pos:?} contains duplicate actor storage id {uid:?}"
                )));
            }
            if let Some(previous) = actor_owner.insert(*uid, *pos)
                && previous != *pos
            {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "actor storage id {uid:?} is referenced by both {previous:?} and {pos:?}"
                )));
            }
            referenced.push(*uid);
        }
        digest_ids.insert(*pos, ids);
    }

    let actor_keys = referenced
        .iter()
        .map(|uid| uid.storage_key())
        .collect::<Vec<_>>();
    let actor_values = storage.get_many(&actor_keys)?;
    let mut actor_roots = BTreeMap::<ActorUid, NbtTag>::new();
    for (uid, value) in referenced.iter().copied().zip(actor_values) {
        let value = value.ok_or_else(|| {
            BedrockWorldError::CorruptWorld(format!(
                "digp references missing actorprefix storage id {uid:?}"
            ))
        })?;
        let roots = parse_consecutive_root_nbt(&value)?;
        if roots.len() != 1 {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "actorprefix {uid:?} contains {} NBT roots instead of one",
                roots.len()
            )));
        }
        let root = roots.into_iter().next().ok_or_else(|| {
            BedrockWorldError::CorruptWorld(format!(
                "actorprefix {uid:?} unexpectedly contains no NBT root"
            ))
        })?;
        let unique_id = actor_unique_id(&root).ok_or_else(|| {
            BedrockWorldError::CorruptWorld(format!(
                "actorprefix {uid:?} has no integer UniqueID"
            ))
        })?;
        if ActorUid::from_unique_id(unique_id) != uid {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "actorprefix {uid:?} does not match NBT UniqueID {unique_id}"
            )));
        }
        actor_roots.insert(uid, root);
    }

    let mut batch = StorageBatch::new();
    let mut report = ActorStorageRewriteReport {
        chunks: snapshot.digests.len(),
        actors: referenced.len(),
        digp_records_deleted: snapshot.digests.len(),
        actorprefix_records_deleted: referenced.len(),
        orphan_actorprefix_records_retained: snapshot
            .actorprefix_records
            .saturating_sub(referenced.len()),
        ..ActorStorageRewriteReport::default()
    };

    for (pos, ids) in digest_ids {
        let existing_raw = snapshot.entities.get(&pos);
        let mut roots = if let Some(raw) = existing_raw {
            parse_consecutive_root_nbt(raw)?
        } else {
            Vec::new()
        };
        let mut existing_by_unique_id = BTreeMap::<i64, NbtTag>::new();
        for root in &roots {
            if let Some(unique_id) = actor_unique_id(root)
                && existing_by_unique_id.insert(unique_id, root.clone()).is_some()
            {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "Entity record for chunk {pos:?} contains duplicate UniqueID {unique_id}"
                )));
            }
        }

        for uid in ids {
            let root = actor_roots.get(&uid).ok_or_else(|| {
                BedrockWorldError::CorruptWorld(format!(
                    "missing preflight actor root for storage id {uid:?}"
                ))
            })?;
            let unique_id = actor_unique_id(root).ok_or_else(|| {
                BedrockWorldError::CorruptWorld(format!(
                    "preflight actor root for storage id {uid:?} lost its UniqueID"
                ))
            })?;
            if let Some(existing) = existing_by_unique_id.get(&unique_id) {
                if existing != root {
                    return Err(BedrockWorldError::ConcurrentWrite(format!(
                        "Entity actor UniqueID {unique_id} differs from actorprefix {uid:?}"
                    )));
                }
                continue;
            }
            existing_by_unique_id.insert(unique_id, root.clone());
            roots.push(root.clone());
        }

        let mut encoded = Vec::new();
        for root in roots {
            encoded.extend(serialize_root_nbt(&root)?);
        }
        let encoded = Bytes::from(encoded);
        if existing_raw != Some(&encoded) {
            batch.put(ChunkKey::new(pos, ChunkRecordTag::Entity).encode(), encoded);
            report.entity_records_written = report.entity_records_written.saturating_add(1);
        }
        batch.delete(ActorDigestKey::new(pos).storage_key());
    }
    for uid in referenced {
        batch.delete(uid.storage_key());
    }

    Ok((batch, report))
}

#[derive(Default)]
struct ActorStorageSnapshot {
    entities: BTreeMap<ChunkPos, Bytes>,
    digests: BTreeMap<ChunkPos, Bytes>,
    actorprefix_records: usize,
}

fn scan_actor_storage(storage: &dyn WorldStorage) -> Result<ActorStorageSnapshot> {
    let mut snapshot = ActorStorageSnapshot::default();
    storage.for_each_entry(StorageReadOptions::default(), &mut |raw_key, value| {
        match BedrockDbKey::decode(raw_key) {
            BedrockDbKey::Chunk(key) if key.tag == ChunkRecordTag::Entity => {
                snapshot.entities.insert(key.pos, value.clone());
            }
            BedrockDbKey::ActorDigest { pos } => {
                snapshot.digests.insert(pos, value.clone());
            }
            BedrockDbKey::ActorPrefix { .. } => {
                snapshot.actorprefix_records = snapshot.actorprefix_records.saturating_add(1);
            }
            _ => {}
        }
        Ok(StorageVisitorControl::Continue)
    })?;
    Ok(snapshot)
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

    fn actor(unique_id: i64, identifier: &str) -> NbtTag {
        NbtTag::Compound(IndexMap::from([
            ("UniqueID".to_string(), NbtTag::Long(unique_id)),
            (
                "identifier".to_string(),
                NbtTag::String(identifier.to_string()),
            ),
        ]))
    }

    fn entity_bytes(roots: &[NbtTag]) -> Bytes {
        let mut bytes = Vec::new();
        for root in roots {
            bytes.extend(serialize_root_nbt(root).expect("serialize actor"));
        }
        Bytes::from(bytes)
    }

    #[test]
    fn world_entity_to_digp_is_preflighted_before_one_batch() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: 4,
            z: -2,
            dimension: Dimension::Overworld,
        };
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Entity).encode(),
                &entity_bytes(&[actor(77, "minecraft:pig")]),
            )
            .expect("seed Entity");

        let (batch, report) =
            stage_world_entity_to_digp_actorprefix(&storage).expect("stage rewrite");
        assert!(
            storage
                .get(&ActorDigestKey::new(pos).storage_key())
                .expect("read before commit")
                .is_none()
        );
        assert_eq!(report.chunks, 1);
        assert_eq!(report.actors, 1);

        storage.write_batch(&batch).expect("commit rewrite");
        assert!(
            storage
                .get(&ChunkKey::new(pos, ChunkRecordTag::Entity).encode())
                .expect("read Entity")
                .is_none()
        );
        let uid = ActorUid::from_unique_id(77);
        assert!(
            storage
                .get(&ActorDigestKey::new(pos).storage_key())
                .expect("read digp")
                .is_some()
        );
        assert!(storage.get(&uid.storage_key()).expect("read actor").is_some());
    }

    #[test]
    fn duplicate_inline_unique_id_fails_without_mutation() {
        let storage = MemoryStorage::new();
        let first = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let second = ChunkPos {
            x: 1,
            z: 0,
            dimension: Dimension::Overworld,
        };
        for pos in [first, second] {
            storage
                .put(
                    &ChunkKey::new(pos, ChunkRecordTag::Entity).encode(),
                    &entity_bytes(&[actor(99, "minecraft:cow")]),
                )
                .expect("seed duplicate Entity");
        }

        assert!(stage_world_entity_to_digp_actorprefix(&storage).is_err());
        assert!(
            storage
                .get(&ActorDigestKey::new(first).storage_key())
                .expect("read digp")
                .is_none()
        );
        assert!(
            storage
                .get(&ActorDigestKey::new(second).storage_key())
                .expect("read digp")
                .is_none()
        );
    }

    #[test]
    fn world_digp_to_entity_deletes_only_referenced_actorprefix() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: -3,
            z: 8,
            dimension: Dimension::Nether,
        };
        let uid = ActorUid::from_unique_id(123);
        let orphan = ActorUid::from_unique_id(456);
        storage
            .put(
                &ActorDigestKey::new(pos).storage_key(),
                &encode_actor_digest_ids(&[uid]),
            )
            .expect("seed digp");
        storage
            .put(&uid.storage_key(), &entity_bytes(&[actor(123, "minecraft:pig")]))
            .expect("seed actorprefix");
        storage
            .put(
                &orphan.storage_key(),
                &entity_bytes(&[actor(456, "minecraft:cow")]),
            )
            .expect("seed orphan actorprefix");

        let (batch, report) =
            stage_world_digp_actorprefix_to_entity(&storage).expect("stage downgrade");
        assert_eq!(report.orphan_actorprefix_records_retained, 1);
        assert!(
            storage
                .get(&ChunkKey::new(pos, ChunkRecordTag::Entity).encode())
                .expect("read before commit")
                .is_none()
        );

        storage.write_batch(&batch).expect("commit downgrade");
        assert!(
            storage
                .get(&ChunkKey::new(pos, ChunkRecordTag::Entity).encode())
                .expect("read Entity")
                .is_some()
        );
        assert!(
            storage
                .get(&ActorDigestKey::new(pos).storage_key())
                .expect("read deleted digp")
                .is_none()
        );
        assert!(storage.get(&uid.storage_key()).expect("read actorprefix").is_none());
        assert!(
            storage
                .get(&orphan.storage_key())
                .expect("read orphan actorprefix")
                .is_some()
        );
    }
}
