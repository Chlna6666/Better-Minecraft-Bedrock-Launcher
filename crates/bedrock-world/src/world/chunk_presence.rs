//! Fast and exhaustive persisted chunk-presence queries.
//!
//! `ChunkData::is_loaded` describes whether a particular data request yielded enough records; it is
//! not a persisted-existence test. These APIs keep those semantics separate and provide a batched
//! exact-key path for hot callers plus an optional prefix-scan fallback for malformed/sparse worlds.

use super::{BedrockWorld, WorldStorageHandle};
use crate::chunk::{ChunkKey, ChunkPos, ChunkRecordTag};
use crate::database::{StorageKeyBatchBuilder, StorageReadOptions, StorageVisitorControl};
use crate::error::Result;

const CHUNK_PRESENCE_ANCHORS: [ChunkRecordTag; 8] = [
    ChunkRecordTag::Version,
    ChunkRecordTag::VersionOld,
    ChunkRecordTag::LegacyVersion,
    ChunkRecordTag::Data3D,
    ChunkRecordTag::Data2D,
    ChunkRecordTag::Data2DLegacy,
    ChunkRecordTag::LegacyTerrain,
    ChunkRecordTag::FinalizedState,
];
const MAX_ENCODED_CHUNK_KEY_BYTES: usize = 14;

/// How aggressively a persisted chunk-presence query verifies unresolved positions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ChunkPresenceMode {
    /// Check canonical modern and historical anchor records using one exact batch read.
    ///
    /// This is the preferred hot-path mode for valid Bedrock worlds and generated chunks.
    #[default]
    CanonicalRecords,
    /// Fall back to an early-terminating chunk-prefix key scan when no canonical anchor exists.
    ///
    /// This detects sparse, partially corrupted or future-format chunks that contain only other
    /// chunk-scoped records. It is intended for validation/repair paths rather than tight loops.
    AnyRecord,
}

/// Persisted presence result for one Bedrock chunk position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkPresence {
    /// Queried chunk position.
    pub pos: ChunkPos,
    /// Whether persisted chunk-scoped data exists under the selected verification mode.
    pub exists: bool,
    /// First canonical/fallback chunk record tag that proved presence, when available.
    pub anchor: Option<ChunkRecordTag>,
}

impl ChunkPresence {
    /// Returns whether this query found persisted chunk data.
    #[must_use]
    pub const fn exists(self) -> bool {
        self.exists
    }
}

impl<S> BedrockWorld<S>
where
    S: WorldStorageHandle,
{
    /// Checks one persisted chunk position.
    pub fn chunk_presence_blocking(
        &self,
        pos: ChunkPos,
        mode: ChunkPresenceMode,
    ) -> Result<ChunkPresence> {
        let mut result = self.chunk_presence_many_blocking([pos], mode)?;
        Ok(result.pop().unwrap_or(ChunkPresence {
            pos,
            exists: false,
            anchor: None,
        }))
    }

    /// Checks whether one persisted chunk position exists through canonical anchor records.
    pub fn chunk_exists_blocking(&self, pos: ChunkPos) -> Result<bool> {
        Ok(self
            .chunk_presence_blocking(pos, ChunkPresenceMode::CanonicalRecords)?
            .exists)
    }

    /// Checks persisted presence for many chunk positions while preserving input order.
    ///
    /// Canonical checks are issued as one exact storage batch. Every encoded Bedrock key is copied
    /// into one contiguous shared backing buffer, avoiding one tiny heap allocation per 9-14 byte
    /// key while preserving the backend's table-grouped `get_many` path.
    pub fn chunk_presence_many_blocking(
        &self,
        positions: impl IntoIterator<Item = ChunkPos>,
        mode: ChunkPresenceMode,
    ) -> Result<Vec<ChunkPresence>> {
        let positions = positions.into_iter().collect::<Vec<_>>();
        if positions.is_empty() {
            return Ok(Vec::new());
        }

        let key_count = positions.len().saturating_mul(CHUNK_PRESENCE_ANCHORS.len());
        let mut keys = StorageKeyBatchBuilder::with_capacity(
            key_count.saturating_mul(MAX_ENCODED_CHUNK_KEY_BYTES),
            key_count,
        );
        for &pos in &positions {
            for tag in CHUNK_PRESENCE_ANCHORS {
                let encoded = ChunkKey::new(pos, tag).encode_inline();
                keys.push(encoded.as_bytes());
            }
        }
        let keys = keys.finish();
        let values = self.storage().get_many(keys.keys())?;

        let mut results = Vec::with_capacity(positions.len());
        for (position_index, &pos) in positions.iter().enumerate() {
            let base = position_index.saturating_mul(CHUNK_PRESENCE_ANCHORS.len());
            let anchor =
                CHUNK_PRESENCE_ANCHORS
                    .iter()
                    .enumerate()
                    .find_map(|(anchor_index, tag)| {
                        values
                            .get(base.saturating_add(anchor_index))
                            .and_then(Option::as_ref)
                            .map(|_| *tag)
                    });
            results.push(ChunkPresence {
                pos,
                exists: anchor.is_some(),
                anchor,
            });
        }

        if mode == ChunkPresenceMode::AnyRecord {
            for result in results.iter_mut().filter(|result| !result.exists) {
                let anchor_key = ChunkKey::new(result.pos, ChunkRecordTag::Version).encode_inline();
                let prefix_len = anchor_key.len().saturating_sub(1);
                let prefix = &anchor_key.as_bytes()[..prefix_len];
                let mut fallback_anchor = None;
                self.storage().for_each_prefix_key(
                    prefix,
                    StorageReadOptions::default(),
                    &mut |key| {
                        if let Ok(decoded) = ChunkKey::decode(key) {
                            if decoded.pos == result.pos {
                                fallback_anchor = Some(decoded.tag);
                                return Ok(StorageVisitorControl::Stop);
                            }
                        }
                        Ok(StorageVisitorControl::Continue)
                    },
                )?;
                if let Some(anchor) = fallback_anchor {
                    result.exists = true;
                    result.anchor = Some(anchor);
                }
            }
        }

        Ok(results)
    }

    /// Checks whether many canonical Bedrock chunks exist while preserving input order.
    pub fn chunks_exist_blocking(
        &self,
        positions: impl IntoIterator<Item = ChunkPos>,
    ) -> Result<Vec<bool>> {
        self.chunk_presence_many_blocking(positions, ChunkPresenceMode::CanonicalRecords)
            .map(|results| results.into_iter().map(|result| result.exists).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::Dimension;
    use crate::database::{MemoryStorage, WorldStorage};
    use crate::world::{BedrockWorldOpenOptions, WorldFormat, WorldFormatHint};
    use std::path::PathBuf;

    fn pos(x: i32, z: i32) -> ChunkPos {
        ChunkPos {
            x,
            z,
            dimension: Dimension::Overworld,
        }
    }

    fn world(storage: MemoryStorage) -> BedrockWorld<MemoryStorage> {
        BedrockWorld::from_typed_storage_with_format(
            PathBuf::from("memory-world"),
            storage,
            BedrockWorldOpenOptions {
                read_only: false,
                format: WorldFormatHint::LevelDb,
            },
            WorldFormat::LevelDb,
        )
    }

    #[test]
    fn canonical_presence_recognizes_modern_and_legacy_anchors() {
        let storage = MemoryStorage::new();
        let modern = pos(2, -3);
        let legacy = pos(-5, 7);
        storage
            .put(
                &ChunkKey::new(modern, ChunkRecordTag::Version).encode(),
                &[40],
            )
            .expect("modern anchor");
        storage
            .put(
                &ChunkKey::new(legacy, ChunkRecordTag::LegacyTerrain).encode(),
                &[1, 2, 3],
            )
            .expect("legacy anchor");
        let world = world(storage);
        assert_eq!(
            world.chunks_exist_blocking([modern, legacy]).unwrap(),
            vec![true, true]
        );
    }

    #[test]
    fn any_record_finds_sparse_subchunk_only_column() {
        let storage = MemoryStorage::new();
        let pos = pos(1, 1);
        storage
            .put(&ChunkKey::subchunk(pos, 4).encode(), &[9, 0])
            .expect("subchunk");
        let world = world(storage);
        assert!(!world.chunk_exists_blocking(pos).unwrap());
        assert!(
            world
                .chunk_presence_blocking(pos, ChunkPresenceMode::AnyRecord)
                .unwrap()
                .exists
        );
    }

    #[test]
    fn missing_chunk_stays_missing() {
        let world = world(MemoryStorage::new());
        let pos = pos(9, 9);
        assert!(
            !world
                .chunk_presence_blocking(pos, ChunkPresenceMode::AnyRecord)
                .unwrap()
                .exists
        );
    }
}
