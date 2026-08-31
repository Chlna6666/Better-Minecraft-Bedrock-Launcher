//! Whole-world Bedrock data capability scanning.
//!
//! One world may contain records written by several game generations. The scan reports the actual
//! persisted data that exists; it does not decide that historical data should be upgraded.

use super::{ActorStorage, ChunkCapabilities, CompatibilityLevel, WorldCapabilities};
use crate::chunk::{
    BedrockDbKey, ChunkKey, ChunkPos, ChunkRecord, ChunkRecordTag, LEGACY_TERRAIN_VALUE_LEN,
    POCKET_TERRAIN_VALUE_LEN, SubChunkVersion,
};
use crate::entity::parse_actor_digest_ids;
use crate::error::Result;
use crate::storage::{StorageReadOptions, StorageVisitorControl, WorldStorage};
use crate::world::WorldFormat;
use bytes::Bytes;
use std::collections::{BTreeMap, BTreeSet};

/// Per-chunk compatibility summary produced by a whole-world scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkSummary {
    /// Chunk position including dimension.
    pub pos: ChunkPos,
    /// Persisted Bedrock data observed in this chunk.
    pub capabilities: ChunkCapabilities,
    /// Exact `LegacyTerrain` payload length when one was observed for this chunk.
    ///
    /// `82_176` identifies a pre-LevelDB Pocket terrain core with no persisted biome/RGB tail;
    /// `83_200` identifies the complete LevelDB-era `LegacyTerrain` representation.
    pub legacy_terrain_payload_len: Option<usize>,
}

/// Aggregate compatibility information derived from one complete world storage scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilityReport {
    /// Baseline storage facts inferred from the selected physical world container.
    pub world: WorldCapabilities,
    /// Worst safety level observed across the scanned world.
    pub compatibility: CompatibilityLevel,
    /// Actor storage observed across chunk `Entity` and `digp`/`actorprefix` records.
    pub actor_storage: ActorStorage,
    /// Number of raw storage records visited.
    pub records_scanned: usize,
    /// Number of unique chunk positions observed, including chunks represented only by `digp`.
    pub chunks_scanned: usize,
    /// Number of chunks safe for same-representation structured round-trip.
    pub exact_chunks: usize,
    /// Number of readable chunks containing data that must retain raw representation for writes.
    pub read_compatible_chunks: usize,
    /// Number of chunks containing a future/unknown SubChunk version.
    pub unsupported_future_chunks: usize,
    /// Number of chunks classified as corrupt by this layer.
    pub corrupt_chunks: usize,
    /// Number of `digp...` records observed.
    pub digp_records: usize,
    /// Number of `actorprefix...` records observed.
    pub actorprefix_records: usize,
    /// Number of chunk-scoped `Entity` records observed.
    pub entity_records: usize,
    /// Number of malformed `digp` records that could not be decoded as actor-id lists.
    pub malformed_actor_digest_records: usize,
    /// Duplicate actor-id entries repeated inside one `digp` record.
    pub duplicate_actor_digest_entries: usize,
    /// Actor references whose `actorprefix` payload is missing.
    pub dangling_actor_references: usize,
    /// Actor payloads not referenced by any observed `digp` record.
    pub orphan_actorprefix_records: usize,
    /// Distinct actor ids referenced by more than one chunk digest.
    pub actor_ids_referenced_by_multiple_chunks: usize,
    /// Pre-LevelDB Pocket terrain core records with no persisted biome/RGB tail.
    pub pocket_terrain_core_records: usize,
    /// Complete LevelDB-era `LegacyTerrain` records including biome/RGB samples.
    pub complete_legacy_terrain_records: usize,
    /// `LegacyTerrain` records whose payload length matches neither known historical representation.
    pub malformed_legacy_terrain_records: usize,
    /// Number of unknown chunk record tags retained for forward compatibility.
    pub unknown_chunk_records: usize,
    /// Number of non-chunk/raw keys not understood by the high-level classifier.
    pub unknown_storage_keys: usize,
    /// Counts grouped by actual persisted SubChunk version.
    pub subchunk_versions: BTreeMap<String, usize>,
    /// Optional per-chunk summaries retained by the scan.
    pub chunks: Vec<ChunkSummary>,
}

impl CompatibilityReport {
    /// Returns whether any future/unknown SubChunk representation was observed.
    #[must_use]
    pub const fn has_future_data(&self) -> bool {
        self.unsupported_future_chunks != 0
    }

    /// Returns whether any data must preserve its raw representation for safe writes.
    #[must_use]
    pub const fn requires_raw_preservation(&self) -> bool {
        self.read_compatible_chunks != 0
            || self.unsupported_future_chunks != 0
            || self.orphan_actorprefix_records != 0
            || self.pocket_terrain_core_records != 0
            || self.unknown_storage_keys != 0
    }

    /// Returns whether actor index/payload relationships are structurally inconsistent.
    #[must_use]
    pub const fn has_actor_link_corruption(&self) -> bool {
        self.malformed_actor_digest_records != 0
            || self.duplicate_actor_digest_entries != 0
            || self.dangling_actor_references != 0
            || self.actor_ids_referenced_by_multiple_chunks != 0
    }
}

/// Scans raw world storage once and derives world/chunk compatibility information.
pub fn scan_compatibility(
    storage: &dyn WorldStorage,
    format: WorldFormat,
    options: StorageReadOptions,
) -> Result<CompatibilityReport> {
    let baseline = format.capabilities();
    let mut chunk_records = BTreeMap::<ChunkPos, Vec<ChunkRecord>>::new();
    let mut legacy_terrain_lengths = BTreeMap::<ChunkPos, usize>::new();
    let mut actor_storage = ActorStorage::Unknown;
    let mut actorprefix_ids = BTreeSet::<i64>::new();
    let mut actor_references = BTreeMap::<i64, (ChunkPos, usize, bool)>::new();
    let mut corrupt_actor_chunks = BTreeSet::<ChunkPos>::new();
    let mut digp_records = 0usize;
    let mut actorprefix_records = 0usize;
    let mut malformed_actor_digest_records = 0usize;
    let mut duplicate_actor_digest_entries = 0usize;
    let mut pocket_terrain_core_records = 0usize;
    let mut complete_legacy_terrain_records = 0usize;
    let mut malformed_legacy_terrain_records = 0usize;
    let mut unknown_storage_keys = 0usize;
    let mut records_scanned = 0usize;

    storage.for_each_prefix_ref(b"", options, &mut |entry| {
        let raw_key = entry.key;
        let value = entry.value;
        records_scanned = records_scanned.saturating_add(1);
        match BedrockDbKey::decode(raw_key) {
            BedrockDbKey::Chunk(key) => {
                if key.tag == ChunkRecordTag::LegacyTerrain {
                    legacy_terrain_lengths.insert(key.pos, value.len());
                    match value.len() {
                        POCKET_TERRAIN_VALUE_LEN => {
                            pocket_terrain_core_records =
                                pocket_terrain_core_records.saturating_add(1);
                        }
                        LEGACY_TERRAIN_VALUE_LEN => {
                            complete_legacy_terrain_records =
                                complete_legacy_terrain_records.saturating_add(1);
                        }
                        _ => {
                            malformed_legacy_terrain_records =
                                malformed_legacy_terrain_records.saturating_add(1);
                        }
                    }
                }
                let retained = if key.tag == ChunkRecordTag::SubChunkPrefix {
                    value
                        .first()
                        .copied()
                        .map_or_else(Bytes::new, |version| Bytes::from(vec![version]))
                } else {
                    Bytes::new()
                };
                chunk_records.entry(key.pos).or_default().push(ChunkRecord {
                    key,
                    value: retained,
                });
            }
            BedrockDbKey::ActorDigest { pos } => {
                digp_records = digp_records.saturating_add(1);
                actor_storage = actor_storage.merge(ActorStorage::DigpActorprefix);
                chunk_records.entry(pos).or_default();
                match parse_actor_digest_ids(value) {
                    Ok(ids) => {
                        let mut local = BTreeSet::<i64>::new();
                        for uid in ids {
                            let actor_id = uid.0;
                            if !local.insert(actor_id) {
                                duplicate_actor_digest_entries =
                                    duplicate_actor_digest_entries.saturating_add(1);
                                corrupt_actor_chunks.insert(pos);
                                continue;
                            }
                            match actor_references.get_mut(&actor_id) {
                                Some((first_pos, references, multiple_chunks)) => {
                                    *references = references.saturating_add(1);
                                    if *first_pos != pos {
                                        *multiple_chunks = true;
                                        corrupt_actor_chunks.insert(*first_pos);
                                        corrupt_actor_chunks.insert(pos);
                                    }
                                }
                                None => {
                                    actor_references.insert(actor_id, (pos, 1, false));
                                }
                            }
                        }
                    }
                    Err(_) => {
                        malformed_actor_digest_records =
                            malformed_actor_digest_records.saturating_add(1);
                        corrupt_actor_chunks.insert(pos);
                    }
                }
            }
            BedrockDbKey::ActorPrefix { actor_id } => {
                actorprefix_records = actorprefix_records.saturating_add(1);
                actorprefix_ids.insert(actor_id);
                actor_storage = actor_storage.merge(ActorStorage::DigpActorprefix);
            }
            BedrockDbKey::Unknown(_) => {
                if let Ok(key) = ChunkKey::decode(raw_key) {
                    chunk_records.entry(key.pos).or_default().push(ChunkRecord {
                        key,
                        value: Bytes::new(),
                    });
                } else {
                    unknown_storage_keys = unknown_storage_keys.saturating_add(1);
                }
            }
            _ => {}
        }
        Ok(StorageVisitorControl::Continue)
    })?;

    let mut dangling_actor_references = 0usize;
    let mut actor_ids_referenced_by_multiple_chunks = 0usize;
    for (actor_id, (first_pos, references, multiple_chunks)) in &actor_references {
        if !actorprefix_ids.contains(actor_id) {
            dangling_actor_references = dangling_actor_references.saturating_add(*references);
            corrupt_actor_chunks.insert(*first_pos);
        }
        if *multiple_chunks {
            actor_ids_referenced_by_multiple_chunks =
                actor_ids_referenced_by_multiple_chunks.saturating_add(1);
        }
    }
    let orphan_actorprefix_records = actorprefix_ids
        .iter()
        .filter(|actor_id| !actor_references.contains_key(actor_id))
        .count();

    let mut compatibility = baseline.compatibility;
    let mut exact_chunks = 0usize;
    let mut read_compatible_chunks = 0usize;
    let mut unsupported_future_chunks = 0usize;
    let mut corrupt_chunks = 0usize;
    let mut entity_records = 0usize;
    let mut unknown_chunk_records = 0usize;
    let mut subchunk_versions = BTreeMap::<String, usize>::new();
    let mut chunks = Vec::with_capacity(chunk_records.len());

    for (pos, records) in chunk_records {
        let mut capabilities = ChunkCapabilities::inspect(&records);
        let legacy_terrain_payload_len = legacy_terrain_lengths.get(&pos).copied();
        capabilities.apply_legacy_terrain_payload_len(legacy_terrain_payload_len);
        if corrupt_actor_chunks.contains(&pos) {
            capabilities.compatibility = CompatibilityLevel::Corrupt;
        }
        if capabilities.has_entity {
            entity_records = entity_records.saturating_add(
                records
                    .iter()
                    .filter(|record| record.key.tag == ChunkRecordTag::Entity)
                    .count(),
            );
            actor_storage = actor_storage.merge(ActorStorage::Entity);
        }
        unknown_chunk_records = unknown_chunk_records.saturating_add(
            records
                .iter()
                .filter(|record| matches!(record.key.tag, ChunkRecordTag::Unknown(_)))
                .count(),
        );
        for version in &capabilities.subchunk_versions {
            *subchunk_versions
                .entry(version_label(*version))
                .or_default() += 1;
        }
        if capabilities.has_unversioned_subchunk {
            *subchunk_versions
                .entry("unversioned".to_string())
                .or_default() += 1;
        }
        match capabilities.compatibility {
            CompatibilityLevel::Exact => exact_chunks = exact_chunks.saturating_add(1),
            CompatibilityLevel::ReadCompatible => {
                read_compatible_chunks = read_compatible_chunks.saturating_add(1);
            }
            CompatibilityLevel::UnsupportedFuture => {
                unsupported_future_chunks = unsupported_future_chunks.saturating_add(1);
            }
            CompatibilityLevel::Corrupt => corrupt_chunks = corrupt_chunks.saturating_add(1),
        }
        compatibility = worst_compatibility(compatibility, capabilities.compatibility);
        chunks.push(ChunkSummary {
            pos,
            capabilities,
            legacy_terrain_payload_len,
        });
    }

    if orphan_actorprefix_records != 0 || unknown_storage_keys != 0 {
        compatibility = worst_compatibility(compatibility, CompatibilityLevel::ReadCompatible);
    }
    if malformed_actor_digest_records != 0
        || duplicate_actor_digest_entries != 0
        || dangling_actor_references != 0
        || actor_ids_referenced_by_multiple_chunks != 0
        || malformed_legacy_terrain_records != 0
    {
        compatibility = CompatibilityLevel::Corrupt;
    }

    chunks.sort_by_key(|entry| (entry.pos.dimension.id(), entry.pos.x, entry.pos.z));
    let chunks_scanned = chunks.len();
    Ok(CompatibilityReport {
        world: baseline,
        compatibility,
        actor_storage,
        records_scanned,
        chunks_scanned,
        exact_chunks,
        read_compatible_chunks,
        unsupported_future_chunks,
        corrupt_chunks,
        digp_records,
        actorprefix_records,
        entity_records,
        malformed_actor_digest_records,
        duplicate_actor_digest_entries,
        dangling_actor_references,
        orphan_actorprefix_records,
        actor_ids_referenced_by_multiple_chunks,
        pocket_terrain_core_records,
        complete_legacy_terrain_records,
        malformed_legacy_terrain_records,
        unknown_chunk_records,
        unknown_storage_keys,
        subchunk_versions,
        chunks,
    })
}

fn version_label(version: SubChunkVersion) -> String {
    match version {
        SubChunkVersion::V0 => "v0".to_string(),
        SubChunkVersion::V1 => "v1".to_string(),
        SubChunkVersion::V2 => "v2".to_string(),
        SubChunkVersion::V3 => "v3".to_string(),
        SubChunkVersion::V4 => "v4".to_string(),
        SubChunkVersion::V5 => "v5".to_string(),
        SubChunkVersion::V6 => "v6".to_string(),
        SubChunkVersion::V7 => "v7".to_string(),
        SubChunkVersion::V8 => "v8".to_string(),
        SubChunkVersion::V9 => "v9".to_string(),
        SubChunkVersion::Unknown(version) => format!("unknown-v{version}"),
    }
}

const fn compatibility_rank(value: CompatibilityLevel) -> u8 {
    match value {
        CompatibilityLevel::Exact => 0,
        CompatibilityLevel::ReadCompatible => 1,
        CompatibilityLevel::UnsupportedFuture => 2,
        CompatibilityLevel::Corrupt => 3,
    }
}

const fn worst_compatibility(
    left: CompatibilityLevel,
    right: CompatibilityLevel,
) -> CompatibilityLevel {
    if compatibility_rank(left) >= compatibility_rank(right) {
        left
    } else {
        right
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{ActorUid, Dimension};
    use crate::entity::encode_actor_digest_ids;
    use crate::storage::{MemoryStorage, StorageBatch};

    #[test]
    fn mixed_actor_and_future_subchunk_are_reported() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: 1,
            z: 2,
            dimension: Dimension::Overworld,
        };
        let mut batch = StorageBatch::new();
        batch.put(
            ChunkKey::new(pos, ChunkRecordTag::Entity).encode(),
            Bytes::new(),
        );
        batch.put(
            ChunkKey::subchunk(pos, 0).encode(),
            Bytes::from_static(&[10, 1, 0]),
        );
        batch.put(
            Bytes::from_static(b"actorprefix\x00\x00\x00\x00\x00\x00\x00\x01"),
            Bytes::new(),
        );
        storage.write_batch(&batch).expect("seed storage");

        let report = scan_compatibility(
            &storage,
            WorldFormat::LevelDb,
            StorageReadOptions::default(),
        )
        .expect("scan compatibility");
        assert_eq!(report.actor_storage, ActorStorage::Mixed);
        assert_eq!(report.unsupported_future_chunks, 1);
        assert!(report.has_future_data());
        assert_eq!(report.orphan_actorprefix_records, 1);
        assert_eq!(report.subchunk_versions.get("unknown-v10"), Some(&1));
    }

    #[test]
    fn actor_link_corruption_is_reported_without_deep_nbt_scan() {
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
        let uid = ActorUid(42);
        let mut batch = StorageBatch::new();
        batch.put(
            crate::entity::ActorDigestKey::new(first).storage_key(),
            encode_actor_digest_ids(&[uid]),
        );
        batch.put(
            crate::entity::ActorDigestKey::new(second).storage_key(),
            encode_actor_digest_ids(&[uid]),
        );
        batch.put(uid.storage_key(), Bytes::from_static(b"actor"));
        storage.write_batch(&batch).expect("seed actor links");

        let report = scan_compatibility(
            &storage,
            WorldFormat::LevelDb,
            StorageReadOptions::default(),
        )
        .expect("scan actor compatibility");
        assert_eq!(report.compatibility, CompatibilityLevel::Corrupt);
        assert_eq!(report.actor_ids_referenced_by_multiple_chunks, 1);
        assert_eq!(report.dangling_actor_references, 0);
        assert_eq!(report.corrupt_chunks, 2);
    }

    #[test]
    fn pocket_terrain_core_is_distinct_from_complete_legacy_terrain() {
        let storage = MemoryStorage::new();
        let short_pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let full_pos = ChunkPos {
            x: 1,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let mut batch = StorageBatch::new();
        batch.put(
            ChunkKey::new(short_pos, ChunkRecordTag::LegacyTerrain).encode(),
            Bytes::from(vec![0; POCKET_TERRAIN_VALUE_LEN]),
        );
        batch.put(
            ChunkKey::new(full_pos, ChunkRecordTag::LegacyTerrain).encode(),
            Bytes::from(vec![0; LEGACY_TERRAIN_VALUE_LEN]),
        );
        storage.write_batch(&batch).expect("seed legacy terrain");

        let report = scan_compatibility(
            &storage,
            WorldFormat::LevelDbLegacyTerrain,
            StorageReadOptions::default(),
        )
        .expect("scan terrain compatibility");
        assert_eq!(report.pocket_terrain_core_records, 1);
        assert_eq!(report.complete_legacy_terrain_records, 1);
        assert_eq!(report.malformed_legacy_terrain_records, 0);
        assert_eq!(report.compatibility, CompatibilityLevel::ReadCompatible);

        let short = report
            .chunks
            .iter()
            .find(|entry| entry.pos == short_pos)
            .expect("short terrain chunk");
        assert_eq!(
            short.legacy_terrain_payload_len,
            Some(POCKET_TERRAIN_VALUE_LEN)
        );
        assert!(short.capabilities.has_pocket_terrain_core);
        assert!(!short.capabilities.has_complete_legacy_terrain);
        assert_eq!(
            short.capabilities.compatibility,
            CompatibilityLevel::ReadCompatible
        );

        let full = report
            .chunks
            .iter()
            .find(|entry| entry.pos == full_pos)
            .expect("complete legacy terrain chunk");
        assert_eq!(
            full.legacy_terrain_payload_len,
            Some(LEGACY_TERRAIN_VALUE_LEN)
        );
        assert!(!full.capabilities.has_pocket_terrain_core);
        assert!(full.capabilities.has_complete_legacy_terrain);
        assert_eq!(full.capabilities.compatibility, CompatibilityLevel::Exact);
    }
}
