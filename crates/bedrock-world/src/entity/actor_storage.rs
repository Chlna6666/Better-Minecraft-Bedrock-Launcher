//! Whole-world Minecraft Bedrock actor storage rewrites.
//!
//! `Entity` and `digp`/`actorprefix` are real Bedrock storage generations. This module preflights
//! every affected record before producing one batch so callers can switch actor storage atomically.
//! When both generations already exist for a chunk they must describe the same ordered actors and NBT;
//! the library never merges two conflicting representations into a third state.

use crate::chunk::{BedrockDbKey, ChunkKey, ChunkPos, ChunkRecordTag};
use crate::database::{StorageBatch, StorageReadOptions, StorageVisitorControl, WorldStorage};
use crate::entity::{ActorDigestKey, ActorUid};
use crate::error::{BedrockWorldError, Result};
use crate::nbt::{NbtTag, parse_consecutive_root_nbt, serialize_root_nbt};
use crate::parsed::{encode_actor_digest_ids, parse_actor_digest_ids};
use bytes::Bytes;
use std::collections::{BTreeMap, BTreeSet};

/// Summary of an atomic whole-world actor storage rewrite.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ActorStorageRewriteReport {
    /// Number of chunk actor records rewritten or validated for the target representation.
    pub chunks: usize,
    /// Number of actors represented by the rewritten source records.
    pub actors: usize,
    /// `Entity` records written by a modern-to-legacy rewrite.
    pub entity_records_written: usize,
    /// `Entity` records deleted by a legacy-to-modern rewrite or empty modern-to-legacy target.
    pub entity_records_deleted: usize,
    /// `digp` records written by a legacy-to-modern rewrite.
    pub digp_records_written: usize,
    /// `digp` records deleted by a modern-to-legacy rewrite.
    pub digp_records_deleted: usize,
    /// `actorprefix` records created by a legacy-to-modern rewrite.
    pub actorprefix_records_written: usize,
    /// Referenced `actorprefix` records removed by a modern-to-legacy rewrite.
    pub actorprefix_records_deleted: usize,
    /// Existing target records reused because their bytes matched exactly.
    pub actorprefix_records_reused: usize,
    /// Unreferenced `actorprefix` records retained because no `digp` record proves ownership.
    pub orphan_actorprefix_records_retained: usize,
}

/// Summary of correcting `actorprefix`/`digp` storage tokens from actor NBT `UniqueID` values.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ActorUidRepairReport {
    /// Actor payload keys whose token did not match the payload `UniqueID`.
    pub actorprefix_records_rekeyed: usize,
    /// Chunk digests updated to reference corrected tokens.
    pub digp_records_rewritten: usize,
    /// Correct actor payload keys that already existed with identical bytes.
    pub actorprefix_records_reused: usize,
    /// Payloads retained unchanged because they were not a valid single-root actor NBT record.
    pub unreadable_actorprefix_records_retained: usize,
}

/// Preflights and stages one atomic correction of actor storage tokens.
///
/// The NBT `UniqueID` is authoritative. A destination key collision aborts unless both payloads are
/// byte-identical, and every visible `digp` reference is rewritten before an obsolete key is deleted.
pub(crate) fn stage_actor_uid_repair(
    storage: &dyn WorldStorage,
) -> Result<(StorageBatch, ActorUidRepairReport)> {
    let mut actor_values = BTreeMap::<ActorUid, Bytes>::new();
    let mut digests = BTreeMap::<ChunkPos, Bytes>::new();
    storage.for_each_entry(StorageReadOptions::default(), &mut |raw_key, value| {
        match BedrockDbKey::decode(raw_key) {
            BedrockDbKey::ActorPrefix { actor_id } => {
                actor_values.insert(ActorUid(actor_id), value.clone());
            }
            BedrockDbKey::ActorDigest { pos } => {
                digests.insert(pos, value.clone());
            }
            _ => {}
        }
        Ok(StorageVisitorControl::Continue)
    })?;

    let mut replacements = BTreeMap::<ActorUid, ActorUid>::new();
    let mut report = ActorUidRepairReport::default();
    for (stored_uid, value) in &actor_values {
        let Ok(roots) = parse_consecutive_root_nbt(value) else {
            report.unreadable_actorprefix_records_retained = report
                .unreadable_actorprefix_records_retained
                .saturating_add(1);
            continue;
        };
        if roots.len() != 1 {
            report.unreadable_actorprefix_records_retained = report
                .unreadable_actorprefix_records_retained
                .saturating_add(1);
            continue;
        }
        let Some(unique_id) = roots.first().and_then(actor_unique_id) else {
            report.unreadable_actorprefix_records_retained = report
                .unreadable_actorprefix_records_retained
                .saturating_add(1);
            continue;
        };
        let expected_uid = ActorUid::from_unique_id(unique_id);
        if expected_uid != *stored_uid {
            replacements.insert(*stored_uid, expected_uid);
        }
    }

    let mut batch = StorageBatch::new();
    for (stored_uid, expected_uid) in &replacements {
        let value = actor_values
            .get(stored_uid)
            .expect("replacement source was scanned");
        match actor_values.get(expected_uid) {
            Some(existing) if existing != value => {
                return Err(BedrockWorldError::ConcurrentWrite(format!(
                    "correct actorprefix key {expected_uid:?} already contains different bytes"
                )));
            }
            Some(_) => {
                report.actorprefix_records_reused =
                    report.actorprefix_records_reused.saturating_add(1);
            }
            None => batch.put(expected_uid.storage_key(), value.clone()),
        }
        batch.delete(stored_uid.storage_key());
        report.actorprefix_records_rekeyed = report.actorprefix_records_rekeyed.saturating_add(1);
    }

    for (pos, raw_digest) in digests {
        let mut ids = parse_actor_digest_ids(&raw_digest)?;
        let mut changed = false;
        for uid in &mut ids {
            if let Some(expected_uid) = replacements.get(uid) {
                *uid = *expected_uid;
                changed = true;
            }
        }
        if changed {
            if ids.iter().copied().collect::<BTreeSet<_>>().len() != ids.len() {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "correcting actor UIDs would create a duplicate in digp {pos:?}"
                )));
            }
            batch.put(
                ActorDigestKey::new(pos).storage_key(),
                encode_actor_digest_ids(&ids),
            );
            report.digp_records_rewritten = report.digp_records_rewritten.saturating_add(1);
        }
    }

    Ok((batch, report))
}

/// Preflights every inline `Entity` record and stages one atomic rewrite to `digp`/`actorprefix`.
///
/// Existing modern records for a source chunk are accepted only when the `digp` UID sequence and every
/// referenced `actorprefix` NBT exactly match the inline source. Extra actors, reordered actors, or
/// differing actor NBT abort the entire rewrite before storage mutation.
pub(crate) fn stage_world_entity_to_digp_actorprefix(
    storage: &dyn WorldStorage,
) -> Result<(StorageBatch, ActorStorageRewriteReport)> {
    let snapshot = scan_actor_storage(storage)?;
    if snapshot.entities.is_empty() {
        return Ok((StorageBatch::new(), ActorStorageRewriteReport::default()));
    }

    let modern_owners = validate_digest_ownership(&snapshot.digests)?;
    let mut batch = StorageBatch::new();
    let mut report = ActorStorageRewriteReport {
        chunks: snapshot.entities.len(),
        entity_records_deleted: snapshot.entities.len(),
        ..ActorStorageRewriteReport::default()
    };
    let mut inline_owner = BTreeMap::<ActorUid, ChunkPos>::new();

    for (pos, raw) in &snapshot.entities {
        let roots = parse_consecutive_root_nbt(raw)?;
        if roots.is_empty() {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "Entity record for chunk {pos:?} is empty"
            )));
        }

        let mut source_ids = Vec::<ActorUid>::with_capacity(roots.len());
        let mut source_roots = Vec::<(ActorUid, NbtTag, i64)>::with_capacity(roots.len());
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
            if let Some(previous) = inline_owner.insert(uid, *pos)
                && previous != *pos
            {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "actor UniqueID {unique_id} appears in inline Entity records for both {previous:?} and {pos:?}"
                )));
            }
            if let Some(owner) = modern_owners.get(&uid)
                && *owner != *pos
            {
                return Err(BedrockWorldError::ConcurrentWrite(format!(
                    "actor UniqueID {unique_id} is already owned by modern digest {owner:?}, not source chunk {pos:?}"
                )));
            }
            source_ids.push(uid);
            source_roots.push((uid, root, unique_id));
        }
        report.actors = report.actors.saturating_add(source_ids.len());

        if let Some(existing_digest) = snapshot.digests.get(pos) {
            let existing_ids = parse_actor_digest_ids(existing_digest)?;
            if existing_ids != source_ids {
                return Err(BedrockWorldError::ConcurrentWrite(format!(
                    "mixed actor storage for chunk {pos:?} disagrees: Entity UniqueID order does not exactly match existing digp"
                )));
            }
        } else {
            batch.put(
                ActorDigestKey::new(*pos).storage_key(),
                encode_actor_digest_ids(&source_ids),
            );
            report.digp_records_written = report.digp_records_written.saturating_add(1);
        }

        let actor_keys = source_ids
            .iter()
            .map(|uid| uid.storage_key())
            .collect::<Vec<_>>();
        let existing_values = storage.get_many(&actor_keys)?;
        for ((uid, root, unique_id), existing) in source_roots.into_iter().zip(existing_values) {
            let encoded = Bytes::from(serialize_root_nbt(&root)?);
            if let Some(existing) = existing {
                if existing != encoded {
                    return Err(BedrockWorldError::ConcurrentWrite(format!(
                        "actorprefix collision for UniqueID {unique_id} ({uid:?})"
                    )));
                }
                report.actorprefix_records_reused =
                    report.actorprefix_records_reused.saturating_add(1);
            } else {
                batch.put(uid.storage_key(), encoded);
                report.actorprefix_records_written =
                    report.actorprefix_records_written.saturating_add(1);
            }
        }

        batch.delete(ChunkKey::new(*pos, ChunkRecordTag::Entity).encode());
    }

    Ok((batch, report))
}

/// Preflights every `digp` record and stages one atomic rewrite to chunk-scoped `Entity` records.
///
/// If an inline `Entity` already exists for a digest chunk, its complete consecutive-root byte stream
/// must already equal the stream derived from the digest in digest order. Existing extra inline actors
/// are not merged. Every referenced `actorprefix` is deleted only because every `digp` record is
/// converted in the same batch; unreferenced `actorprefix` records are retained.
pub(crate) fn stage_world_digp_actorprefix_to_entity(
    storage: &dyn WorldStorage,
) -> Result<(StorageBatch, ActorStorageRewriteReport)> {
    let snapshot = scan_actor_storage(storage)?;
    if snapshot.digests.is_empty() {
        return Ok((StorageBatch::new(), ActorStorageRewriteReport::default()));
    }

    let actor_owner = validate_digest_ownership(&snapshot.digests)?;
    let referenced = actor_owner.keys().copied().collect::<Vec<_>>();
    let actor_keys = referenced
        .iter()
        .map(|uid| uid.storage_key())
        .collect::<Vec<_>>();
    let actor_values = storage.get_many(&actor_keys)?;
    let mut actor_bytes = BTreeMap::<ActorUid, Bytes>::new();

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
        let root = roots.first().ok_or_else(|| {
            BedrockWorldError::CorruptWorld(format!(
                "actorprefix {uid:?} unexpectedly contains no NBT root"
            ))
        })?;
        let unique_id = actor_unique_id(root).ok_or_else(|| {
            BedrockWorldError::CorruptWorld(format!("actorprefix {uid:?} has no integer UniqueID"))
        })?;
        if ActorUid::from_unique_id(unique_id) != uid {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "actorprefix {uid:?} does not match NBT UniqueID {unique_id}"
            )));
        }
        actor_bytes.insert(uid, value);
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

    for (pos, raw_digest) in &snapshot.digests {
        let ids = parse_actor_digest_ids(raw_digest)?;
        let mut encoded = Vec::new();
        for uid in &ids {
            let actor = actor_bytes.get(uid).ok_or_else(|| {
                BedrockWorldError::CorruptWorld(format!(
                    "missing preflight actorprefix bytes for storage id {uid:?}"
                ))
            })?;
            encoded.extend_from_slice(actor);
        }
        let target = Bytes::from(encoded);
        let entity_key = ChunkKey::new(*pos, ChunkRecordTag::Entity).encode();

        match (target.is_empty(), snapshot.entities.get(pos)) {
            (true, Some(existing)) if !existing.is_empty() => {
                return Err(BedrockWorldError::ConcurrentWrite(format!(
                    "empty digp for chunk {pos:?} conflicts with an existing non-empty Entity record"
                )));
            }
            (true, Some(_)) => {
                batch.delete(entity_key);
                report.entity_records_deleted = report.entity_records_deleted.saturating_add(1);
            }
            (true, None) => {}
            (false, Some(existing)) if existing != &target => {
                return Err(BedrockWorldError::ConcurrentWrite(format!(
                    "mixed actor storage for chunk {pos:?} disagrees: existing Entity bytes do not exactly match digp/actorprefix"
                )));
            }
            (false, Some(_)) => {}
            (false, None) => {
                batch.put(entity_key, target);
                report.entity_records_written = report.entity_records_written.saturating_add(1);
            }
        }
        batch.delete(ActorDigestKey::new(*pos).storage_key());
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
                if snapshot.entities.insert(key.pos, value.clone()).is_some() {
                    return Err(BedrockWorldError::CorruptWorld(format!(
                        "multiple visible Entity records decode to chunk {:?}",
                        key.pos
                    )));
                }
            }
            BedrockDbKey::ActorDigest { pos } => {
                if snapshot.digests.insert(pos, value.clone()).is_some() {
                    return Err(BedrockWorldError::CorruptWorld(format!(
                        "multiple visible digp records decode to chunk {pos:?}"
                    )));
                }
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

fn validate_digest_ownership(
    digests: &BTreeMap<ChunkPos, Bytes>,
) -> Result<BTreeMap<ActorUid, ChunkPos>> {
    let mut actor_owner = BTreeMap::<ActorUid, ChunkPos>::new();
    for (pos, raw) in digests {
        let ids = parse_actor_digest_ids(raw)?;
        let mut local = BTreeSet::new();
        for uid in ids {
            if !local.insert(uid) {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "digp for chunk {pos:?} contains duplicate actor storage id {uid:?}"
                )));
            }
            if let Some(previous) = actor_owner.insert(uid, *pos)
                && previous != *pos
            {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "actor storage id {uid:?} is referenced by both {previous:?} and {pos:?}"
                )));
            }
        }
    }
    Ok(actor_owner)
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

    fn overworld(x: i32, z: i32) -> ChunkPos {
        ChunkPos {
            x,
            z,
            dimension: Dimension::Overworld,
        }
    }

    #[test]
    fn world_entity_to_digp_is_preflighted_before_one_batch() {
        let storage = MemoryStorage::new();
        let pos = overworld(4, -2);
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
        assert!(
            storage
                .get(&uid.storage_key())
                .expect("read actor")
                .is_some()
        );
    }

    #[test]
    fn mixed_entity_and_digest_must_match_exactly() {
        let storage = MemoryStorage::new();
        let pos = overworld(0, 0);
        let uid = ActorUid::from_unique_id(1);
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Entity).encode(),
                &entity_bytes(&[actor(1, "minecraft:pig")]),
            )
            .unwrap();
        storage
            .put(
                &ActorDigestKey::new(pos).storage_key(),
                &encode_actor_digest_ids(&[uid, ActorUid::from_unique_id(2)]),
            )
            .unwrap();

        assert!(stage_world_entity_to_digp_actorprefix(&storage).is_err());
        assert!(
            storage
                .get(&ChunkKey::new(pos, ChunkRecordTag::Entity).encode())
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn duplicate_inline_unique_id_fails_without_mutation() {
        let storage = MemoryStorage::new();
        let first = overworld(0, 0);
        let second = overworld(1, 0);
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
            .put(
                &uid.storage_key(),
                &entity_bytes(&[actor(123, "minecraft:pig")]),
            )
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
        assert!(
            storage
                .get(&uid.storage_key())
                .expect("read actorprefix")
                .is_none()
        );
        assert!(
            storage
                .get(&orphan.storage_key())
                .expect("read orphan actorprefix")
                .is_some()
        );
    }

    #[test]
    fn actor_uid_repair_rekeys_payload_and_every_digest_reference_atomically() {
        let storage = MemoryStorage::new();
        let unique_id = -206_158_405_104_i64;
        let corrected = ActorUid::from_unique_id(unique_id);
        let unique = unique_id as u64;
        let old_high = 0xffff_ffff_u32.wrapping_sub((unique >> 32) as u32);
        let old_storage = (u64::from(old_high) << 32) | (unique & 0xffff_ffff);
        let obsolete = ActorUid(i64::from_le_bytes(old_storage.to_be_bytes()));
        let first = overworld(1, 0);
        let second = overworld(2, 0);
        let payload = entity_bytes(&[actor(unique_id, "minecraft:pig")]);
        storage.put(&obsolete.storage_key(), &payload).unwrap();
        for pos in [first, second] {
            storage
                .put(
                    &ActorDigestKey::new(pos).storage_key(),
                    &encode_actor_digest_ids(&[obsolete]),
                )
                .unwrap();
        }

        let (batch, report) = stage_actor_uid_repair(&storage).expect("stage UID repair");
        assert_eq!(report.actorprefix_records_rekeyed, 1);
        assert_eq!(report.digp_records_rewritten, 2);
        storage.write_batch(&batch).expect("commit UID repair");

        assert!(storage.get(&obsolete.storage_key()).unwrap().is_none());
        assert_eq!(
            storage.get(&corrected.storage_key()).unwrap(),
            Some(payload)
        );
        for pos in [first, second] {
            let digest = storage
                .get(&ActorDigestKey::new(pos).storage_key())
                .unwrap()
                .unwrap();
            assert_eq!(parse_actor_digest_ids(&digest).unwrap(), vec![corrected]);
        }
    }

    #[test]
    fn mixed_modern_and_entity_target_must_match_exact_bytes() {
        let storage = MemoryStorage::new();
        let pos = overworld(2, 3);
        let uid = ActorUid::from_unique_id(5);
        storage
            .put(
                &ActorDigestKey::new(pos).storage_key(),
                &encode_actor_digest_ids(&[uid]),
            )
            .unwrap();
        storage
            .put(
                &uid.storage_key(),
                &entity_bytes(&[actor(5, "minecraft:pig")]),
            )
            .unwrap();
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Entity).encode(),
                &entity_bytes(&[actor(5, "minecraft:pig"), actor(6, "minecraft:cow")]),
            )
            .unwrap();

        assert!(stage_world_digp_actorprefix_to_entity(&storage).is_err());
        assert!(storage.get(&uid.storage_key()).unwrap().is_some());
        assert!(
            storage
                .get(&ActorDigestKey::new(pos).storage_key())
                .unwrap()
                .is_some()
        );
    }
}
