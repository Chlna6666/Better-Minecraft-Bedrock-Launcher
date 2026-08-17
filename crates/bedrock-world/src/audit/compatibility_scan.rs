//! Whole-world historical format capability scanning.
//!
//! Bedrock upgrades are not necessarily all-or-nothing: one database may contain old chunk keys,
//! modern chunk records, legacy inline entities, modern actor digests and unknown future records at
//! the same time. This scan therefore derives compatibility from the actual key/value population.

use crate::{
    ActorStorageModel, BedrockDbKey, ChunkCapabilities, ChunkKey, ChunkPos, ChunkRecord,
    ChunkRecordTag, CompatibilityLevel, Result, StorageReadOptions, StorageVisitorControl,
    SubChunkCodecKind, WorldCapabilities, WorldFormat, WorldStorage,
};
use bytes::Bytes;
use std::collections::{BTreeMap, BTreeSet};

/// Per-chunk compatibility summary produced by a whole-world scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkCompatibilitySummary {
    /// Chunk position including dimension.
    pub pos: ChunkPos,
    /// Capabilities inferred from the chunk's observed records.
    pub capabilities: ChunkCapabilities,
}

/// Aggregate compatibility information derived from one complete world storage scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldCompatibilityReport {
    /// Baseline capabilities inferred from the selected physical storage container.
    pub world: WorldCapabilities,
    /// Worst compatibility level observed across the scanned world.
    pub compatibility: CompatibilityLevel,
    /// Combined actor storage model observed in legacy chunk records and modern actor key spaces.
    pub actor_storage: ActorStorageModel,
    /// Number of raw storage records visited.
    pub records_scanned: usize,
    /// Number of unique chunk positions observed.
    pub chunks_scanned: usize,
    /// Number of chunks safe for exact structured round-trip by current codecs.
    pub exact_chunks: usize,
    /// Number of readable chunks containing preserved unknown/non-exact data.
    pub read_compatible_chunks: usize,
    /// Number of chunks requiring explicit historical migration before normal writes.
    pub migration_required_chunks: usize,
    /// Number of chunks containing a future/unknown subchunk format.
    pub unsupported_future_chunks: usize,
    /// Number of chunks classified as corrupt by the compatibility layer.
    pub corrupt_chunks: usize,
    /// Number of modern actor digest records (`digp...`) observed.
    pub actor_digest_records: usize,
    /// Number of modern actor payload records (`actorprefix...`) observed.
    pub actor_prefix_records: usize,
    /// Number of legacy inline `Entity` chunk records observed.
    pub legacy_entity_records: usize,
    /// Number of unknown chunk record tags retained for forward compatibility.
    pub unknown_chunk_records: usize,
    /// Number of non-chunk/raw keys not understood by the high-level classifier.
    pub unknown_storage_keys: usize,
    /// Counts of historical subchunk codec families by stable diagnostic label.
    pub subchunk_codecs: BTreeMap<String, usize>,
    /// Optional per-chunk summaries retained by the scan.
    pub chunks: Vec<ChunkCompatibilitySummary>,
}

impl WorldCompatibilityReport {
    /// Returns whether any scanned record requires an explicit migration before structured writes.
    #[must_use]
    pub const fn requires_migration(&self) -> bool {
        self.migration_required_chunks != 0
    }

    /// Returns whether any future/unknown chunk format was observed.
    #[must_use]
    pub const fn has_future_data(&self) -> bool {
        self.unsupported_future_chunks != 0
    }
}

/// Scans raw world storage once and derives world/chunk capability information.
///
/// This function does not mutate the database and deliberately inspects record shapes rather than
/// relying on one `level.dat` version field. Subchunk values are retained only as a one-byte version
/// probe while the scan is in progress, so large terrain payloads are not duplicated in memory.
pub fn scan_world_compatibility_blocking(
    storage: &dyn WorldStorage,
    format: WorldFormat,
    options: StorageReadOptions,
) -> Result<WorldCompatibilityReport> {
    let baseline = format.capabilities();
    let mut chunk_records = BTreeMap::<ChunkPos, Vec<ChunkRecord>>::new();
    let mut actor_storage = ActorStorageModel::Unknown;
    let mut actor_digest_records = 0usize;
    let mut actor_prefix_records = 0usize;
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
                actor_digest_records = actor_digest_records.saturating_add(1);
                actor_storage = actor_storage.merge(ActorStorageModel::ModernDigest);
            }
            BedrockDbKey::ActorPrefix { .. } => {
                actor_prefix_records = actor_prefix_records.saturating_add(1);
                actor_storage = actor_storage.merge(ActorStorageModel::ModernDigest);
            }
            BedrockDbKey::Unknown(_) => {
                // Unknown 9/10/13/14-byte chunk keys are still recoverable through ChunkKey so an
                // unrecognised tag contributes to the owning chunk instead of being lost globally.
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
    let mut migration_required_chunks = 0usize;
    let mut unsupported_future_chunks = 0usize;
    let mut corrupt_chunks = 0usize;
    let mut legacy_entity_records = 0usize;
    let mut unknown_chunk_records = 0usize;
    let mut subchunk_codecs = BTreeMap::<String, usize>::new();
    let mut chunks = Vec::with_capacity(chunk_records.len());

    for (pos, records) in chunk_records {
        let capabilities = ChunkCapabilities::inspect(&records);
        if capabilities.has_legacy_inline_entities {
            legacy_entity_records = legacy_entity_records.saturating_add(
                records
                    .iter()
                    .filter(|record| record.key.tag == ChunkRecordTag::Entity)
                    .count(),
            );
            actor_storage = actor_storage.merge(ActorStorageModel::LegacyInline);
        }
        unknown_chunk_records = unknown_chunk_records.saturating_add(
            records
                .iter()
                .filter(|record| matches!(record.key.tag, ChunkRecordTag::Unknown(_)))
                .count(),
        );
        for codec in &capabilities.subchunk_codecs {
            *subchunk_codecs.entry(codec_label(*codec)).or_default() += 1;
        }
        match capabilities.compatibility {
            CompatibilityLevel::Exact => exact_chunks = exact_chunks.saturating_add(1),
            CompatibilityLevel::ReadCompatible => {
                read_compatible_chunks = read_compatible_chunks.saturating_add(1);
            }
            CompatibilityLevel::MigrationRequired => {
                migration_required_chunks = migration_required_chunks.saturating_add(1);
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
        migration_required_chunks,
        unsupported_future_chunks,
        corrupt_chunks,
        actor_digest_records,
        actor_prefix_records,
        legacy_entity_records,
        unknown_chunk_records,
        unknown_storage_keys,
        subchunk_codecs,
        chunks,
    })
}

fn codec_label(codec: SubChunkCodecKind) -> String {
    match codec {
        SubChunkCodecKind::LegacyV0 => "legacy-v0".to_string(),
        SubChunkCodecKind::PalettedV1 => "paletted-v1".to_string(),
        SubChunkCodecKind::LegacyV2ToV7(version) => format!("legacy-v{version}"),
        SubChunkCodecKind::PalettedV8 => "paletted-v8".to_string(),
        SubChunkCodecKind::PalettedV9 => "paletted-v9".to_string(),
        SubChunkCodecKind::UnknownFuture(version) => format!("future-v{version}"),
        SubChunkCodecKind::UnknownLegacy(version) => format!("unknown-legacy-v{version}"),
        SubChunkCodecKind::Unknown => "unknown".to_string(),
    }
}

const fn compatibility_rank(value: CompatibilityLevel) -> u8 {
    match value {
        CompatibilityLevel::Exact => 0,
        CompatibilityLevel::ReadCompatible => 1,
        CompatibilityLevel::MigrationRequired => 2,
        CompatibilityLevel::UnsupportedFuture => 3,
        CompatibilityLevel::Corrupt => 4,
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
    use crate::{Dimension, MemoryStorage, StorageBatch};

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
        batch.put(Bytes::from_static(b"actorprefix\x00\x00\x00\x00\x00\x00\x00\x01"), Bytes::new());
        storage.write_batch(&batch).expect("seed storage");

        let report = scan_world_compatibility_blocking(
            &storage,
            WorldFormat::LevelDb,
            StorageReadOptions::default(),
        )
        .expect("scan compatibility");
        assert_eq!(report.actor_storage, ActorStorageModel::Mixed);
        assert_eq!(report.unsupported_future_chunks, 1);
        assert!(report.has_future_data());
    }
}
