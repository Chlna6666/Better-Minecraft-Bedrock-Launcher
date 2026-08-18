//! Whole-world Bedrock data capability scanning.
//!
//! One world may contain records written by several game generations. The scan reports the actual
//! persisted data that exists; it does not decide that historical data should be upgraded.

use super::{ActorStorage, ChunkCapabilities, CompatibilityLevel, WorldCapabilities};
use crate::chunk::{BedrockDbKey, ChunkKey, ChunkPos, ChunkRecord, ChunkRecordTag, SubChunkVersion};
use crate::database::{StorageReadOptions, StorageVisitorControl, WorldStorage};
use crate::error::Result;
use crate::world::WorldFormat;
use bytes::Bytes;
use std::collections::BTreeMap;

/// Per-chunk data summary produced by a whole-world scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkCompatibilitySummary {
    /// Chunk position including dimension.
    pub pos: ChunkPos,
    /// Persisted Bedrock data observed in this chunk.
    pub capabilities: ChunkCapabilities,
}

/// Aggregate compatibility information derived from one complete world storage scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldCompatibilityReport {
    /// Baseline storage facts inferred from the selected physical world container.
    pub world: WorldCapabilities,
    /// Worst safety level observed across the scanned world.
    pub compatibility: CompatibilityLevel,
    /// Actor storage observed across chunk `Entity` and `digp`/`actorprefix` records.
    pub actor_storage: ActorStorage,
    /// Number of raw storage records visited.
    pub records_scanned: usize,
    /// Number of unique chunk positions observed.
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
    /// Number of unknown chunk record tags retained for forward compatibility.
    pub unknown_chunk_records: usize,
    /// Number of non-chunk/raw keys not understood by the high-level classifier.
    pub unknown_storage_keys: usize,
    /// Counts grouped by actual persisted SubChunk version.
    pub subchunk_versions: BTreeMap<String, usize>,
    /// Optional per-chunk summaries retained by the scan.
    pub chunks: Vec<ChunkCompatibilitySummary>,
}

impl WorldCompatibilityReport {
    /// Returns whether any future/unknown SubChunk representation was observed.
    #[must_use]
    pub const fn has_future_data(&self) -> bool {
        self.unsupported_future_chunks != 0
    }

    /// Returns whether any data must preserve its raw representation for safe writes.
    #[must_use]
    pub const fn requires_raw_preservation(&self) -> bool {
        self.read_compatible_chunks != 0 || self.unsupported_future_chunks != 0
    }
}

/// Scans raw world storage once and derives world/chunk data information.
pub fn scan_world_compatibility_blocking(
    storage: &dyn WorldStorage,
    format: WorldFormat,
    options: StorageReadOptions,
) -> Result<WorldCompatibilityReport> {
    let baseline = format.capabilities();
    let mut chunk_records = BTreeMap::<ChunkPos, Vec<ChunkRecord>>::new();
    let mut actor_storage = ActorStorage::Unknown;
    let mut digp_records = 0usize;
    let mut actorprefix_records = 0usize;
    let mut unknown_storage_keys = 0usize;
    let mut records_scanned = 0usize;

    storage.for_each_entry(options, &mut |raw_key, value| {
        records_scanned = records_scanned.saturating_add(1);
        match BedrockDbKey::decode(raw_key) {
            BedrockDbKey::Chunk(key) => {
                let retained = if key.tag == ChunkRecordTag::SubChunkPrefix {
                    value
                        .first()
                        .copied()
                        .map_or_else(Bytes::new, |version| Bytes::from(vec![version]))
                } else {
                    Bytes::new()
                };
                chunk_records
                    .entry(key.pos)
                    .or_default()
                    .push(ChunkRecord { key, value: retained });
            }
            BedrockDbKey::ActorDigest { .. } => {
                digp_records = digp_records.saturating_add(1);
                actor_storage = actor_storage.merge(ActorStorage::DigpActorprefix);
            }
            BedrockDbKey::ActorPrefix { .. } => {
                actorprefix_records = actorprefix_records.saturating_add(1);
                actor_storage = actor_storage.merge(ActorStorage::DigpActorprefix);
            }
            BedrockDbKey::Unknown(_) => {
                if let Ok(key) = ChunkKey::decode(raw_key) {
                    chunk_records
                        .entry(key.pos)
                        .or_default()
                        .push(ChunkRecord {
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
        let capabilities = ChunkCapabilities::inspect(&records);
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
            *subchunk_versions.entry(version_label(*version)).or_default() += 1;
        }
        if capabilities.has_unversioned_subchunk {
            *subchunk_versions.entry("unversioned".to_string()).or_default() += 1;
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
        chunks.push(ChunkCompatibilitySummary { pos, capabilities });
    }

    chunks.sort_by_key(|entry| (entry.pos.dimension.id(), entry.pos.x, entry.pos.z));
    let chunks_scanned = chunks.len();
    Ok(WorldCompatibilityReport {
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
    use crate::chunk::Dimension;
    use crate::database::{MemoryStorage, StorageBatch};

    #[test]
    fn mixed_actor_and_future_subchunk_are_reported() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: 1,
            z: 2,
            dimension: Dimension::Overworld,
        };
        let mut batch = StorageBatch::new();
        batch.put(ChunkKey::new(pos, ChunkRecordTag::Entity).encode(), Bytes::new());
        batch.put(ChunkKey::subchunk(pos, 0).encode(), Bytes::from_static(&[10, 1, 0]));
        batch.put(
            Bytes::from_static(b"actorprefix\x00\x00\x00\x00\x00\x00\x00\x01"),
            Bytes::new(),
        );
        storage.write_batch(&batch).expect("seed storage");

        let report = scan_world_compatibility_blocking(
            &storage,
            WorldFormat::LevelDb,
            StorageReadOptions::default(),
        )
        .expect("scan compatibility");
        assert_eq!(report.actor_storage, ActorStorage::Mixed);
        assert_eq!(report.unsupported_future_chunks, 1);
        assert!(report.has_future_data());
        assert_eq!(report.subchunk_versions.get("unknown-v10"), Some(&1));
    }
}
