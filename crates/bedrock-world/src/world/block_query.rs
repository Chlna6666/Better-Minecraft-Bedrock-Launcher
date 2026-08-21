//! Batched authoritative block-state queries over Bedrock SubChunk and LegacyTerrain storage.
//!
//! Exact SubChunk keys are deduplicated, fetched in one storage batch and decoded once per unique
//! SubChunk. The borrowed visitor API exposes palette-owned `BlockState` values directly, avoiding a
//! deep `BlockState` clone for every queried position. Historical `LegacyTerrain` is fetched once per
//! relevant chunk as an exact fallback; surface hints are never used by this path.

use super::{BedrockWorld, WorldStorageHandle};
use crate::chunk::{
    BlockPos, BlockState, ChunkKey, ChunkPos, ChunkRecordTag, Dimension, LegacyTerrain, SubChunk,
    SubChunkDecodeMode, parse_subchunk_with_mode,
};
use crate::database::StorageKeyBatchBuilder;
use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use std::collections::BTreeMap;

const MAX_ENCODED_CHUNK_KEY_BYTES: usize = 14;

/// One owned exact block-state query result.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockStateQueryResult {
    /// Absolute queried block position.
    pub pos: BlockPos,
    /// Semantic state when persisted terrain contains a block at this position.
    ///
    /// `None` represents an absent/all-air SubChunk record rather than a storage error.
    pub state: Option<BlockState>,
}

/// Borrowed exact block-state result passed to batched visitors.
///
/// Modern states borrow directly from the decoded SubChunk palette and are valid only for the
/// duration of the visitor call. Legacy numeric states are materialized once for that call.
#[derive(Debug, Clone, Copy)]
pub struct BlockStateQueryRef<'a> {
    /// Absolute queried block position.
    pub pos: BlockPos,
    /// Authoritative state at the queried position, when present.
    pub state: Option<&'a BlockState>,
}

/// Visitor control for authoritative block-state batch queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockStateQueryControl {
    /// Continue with the next input position.
    Continue,
    /// Stop visiting results without treating the decision as an error.
    Stop,
}

/// Diagnostics for one authoritative block-state batch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BlockStateBatchStats {
    /// Number of input positions.
    pub positions: usize,
    /// Number of unique exact SubChunk records requested.
    pub unique_subchunks: usize,
    /// Number of unique legacy terrain fallback records requested.
    pub unique_legacy_chunks: usize,
    /// Number of exact storage keys issued in the single batch.
    pub storage_keys: usize,
    /// Number of SubChunk payloads actually decoded.
    pub subchunks_decoded: usize,
    /// Number of LegacyTerrain payloads actually decoded.
    pub legacy_terrain_decoded: usize,
    /// Whether the result visitor requested early termination.
    pub stopped: bool,
}

impl<S> BedrockWorld<S>
where
    S: WorldStorageHandle,
{
    /// Visits many authoritative block states in one dimension while preserving input order.
    ///
    /// Modern V0-V9 SubChunks are fetched by exact key and decoded with
    /// [`SubChunkDecodeMode::FullIndices`] exactly once per unique SubChunk. This path never uses
    /// `ExactSurfaceSubchunkPolicy::HintThenVerify`, height-map hints, surface projection, or any
    /// other render approximation. It is suitable for server-side authoritative reads.
    ///
    /// Modern `BlockState` values are borrowed directly from their decoded palettes, so callers that
    /// can consume each result immediately avoid per-position `String`/NBT-map clones.
    pub fn for_each_block_state_at_blocking<F>(
        &self,
        dimension: Dimension,
        positions: impl IntoIterator<Item = BlockPos>,
        mut visitor: F,
    ) -> Result<BlockStateBatchStats>
    where
        F: for<'a> FnMut(BlockStateQueryRef<'a>) -> Result<BlockStateQueryControl>,
    {
        let positions = positions.into_iter().collect::<Vec<_>>();
        self.visit_block_states_at_blocking(dimension, &positions, &mut visitor)
    }

    /// Reads many exact block states in one dimension while preserving input order.
    ///
    /// This owned compatibility API is implemented on top of
    /// [`BedrockWorld::for_each_block_state_at_blocking`]. Server and renderer hot paths should use
    /// the borrowed visitor form when they do not need to retain full owned states.
    pub fn get_block_states_at_blocking(
        &self,
        dimension: Dimension,
        positions: impl IntoIterator<Item = BlockPos>,
    ) -> Result<Vec<BlockStateQueryResult>> {
        let positions = positions.into_iter().collect::<Vec<_>>();
        let mut results = Vec::with_capacity(positions.len());
        self.visit_block_states_at_blocking(dimension, &positions, &mut |entry| {
            results.push(BlockStateQueryResult {
                pos: entry.pos,
                state: entry.state.cloned(),
            });
            Ok(BlockStateQueryControl::Continue)
        })?;
        Ok(results)
    }

    fn visit_block_states_at_blocking<F>(
        &self,
        dimension: Dimension,
        positions: &[BlockPos],
        visitor: &mut F,
    ) -> Result<BlockStateBatchStats>
    where
        F: for<'a> FnMut(BlockStateQueryRef<'a>) -> Result<BlockStateQueryControl>,
    {
        if positions.is_empty() {
            return Ok(BlockStateBatchStats::default());
        }

        // Flat Vec + sort/dedup avoids one tree/hash node allocation per unique SubChunk.
        let mut subchunk_order = Vec::<(ChunkPos, i8)>::with_capacity(positions.len());
        let mut legacy_order = Vec::<ChunkPos>::with_capacity(positions.len().min(256));
        for &block_pos in positions {
            let chunk_pos = block_pos.to_chunk_pos(dimension);
            if let Ok(subchunk_y) = i8::try_from(block_pos.y.div_euclid(16)) {
                subchunk_order.push((chunk_pos, subchunk_y));
            }
            if (0..=127).contains(&block_pos.y) {
                legacy_order.push(chunk_pos);
            }
        }
        subchunk_order.sort_unstable();
        subchunk_order.dedup();
        legacy_order.sort_unstable();
        legacy_order.dedup();

        let key_count = subchunk_order.len().saturating_add(legacy_order.len());
        let mut keys = StorageKeyBatchBuilder::with_capacity(
            key_count.saturating_mul(MAX_ENCODED_CHUNK_KEY_BYTES),
            key_count,
        );
        for &(chunk_pos, subchunk_y) in &subchunk_order {
            let encoded = ChunkKey::subchunk(chunk_pos, subchunk_y).encode_inline();
            keys.push(encoded.as_bytes());
        }
        let legacy_base = keys.len();
        for &chunk_pos in &legacy_order {
            let encoded = ChunkKey::new(chunk_pos, ChunkRecordTag::LegacyTerrain).encode_inline();
            keys.push(encoded.as_bytes());
        }
        let keys = keys.finish();

        let mut values = self.storage().get_many(keys.keys())?;
        if values.len() != keys.len() {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "batch block-state read returned {} values for {} exact keys",
                values.len(),
                keys.len()
            )));
        }

        // Keep decoded records aligned with their sorted keys. Binary search is cache-friendly for
        // the small number of SubChunks in a sparse authoritative batch and avoids HashMap nodes.
        let mut subchunks = Vec::<Option<SubChunk>>::with_capacity(subchunk_order.len());
        let mut subchunks_decoded = 0usize;
        for (index, &(_, subchunk_y)) in subchunk_order.iter().enumerate() {
            let parsed = values
                .get_mut(index)
                .and_then(Option::take)
                .map(|bytes| {
                    subchunks_decoded = subchunks_decoded.saturating_add(1);
                    parse_subchunk_with_mode(subchunk_y, bytes, SubChunkDecodeMode::FullIndices)
                })
                .transpose()?;
            subchunks.push(parsed);
        }

        let mut legacy_terrain = Vec::<Option<LegacyTerrain>>::with_capacity(legacy_order.len());
        let mut legacy_terrain_decoded = 0usize;
        for offset in 0..legacy_order.len() {
            let parsed = values
                .get_mut(legacy_base.saturating_add(offset))
                .and_then(Option::take)
                .map(|bytes| {
                    legacy_terrain_decoded = legacy_terrain_decoded.saturating_add(1);
                    LegacyTerrain::parse(bytes)
                })
                .transpose()?;
            legacy_terrain.push(parsed);
        }

        let mut stats = BlockStateBatchStats {
            positions: positions.len(),
            unique_subchunks: subchunk_order.len(),
            unique_legacy_chunks: legacy_order.len(),
            storage_keys: key_count,
            subchunks_decoded,
            legacy_terrain_decoded,
            stopped: false,
        };

        for &block_pos in positions {
            let chunk_pos = block_pos.to_chunk_pos(dimension);
            let (local_x, _, local_z) = block_pos.in_chunk_offset();
            let local_y = u8::try_from(block_pos.y.rem_euclid(16)).map_err(|_| {
                BedrockWorldError::Validation(format!(
                    "block y={} has invalid local SubChunk offset",
                    block_pos.y
                ))
            })?;

            let mut resolved = ResolvedBlockState::Missing;
            if let Ok(subchunk_y) = i8::try_from(block_pos.y.div_euclid(16)) {
                if let Ok(index) = subchunk_order.binary_search(&(chunk_pos, subchunk_y)) {
                    if let Some(subchunk) = subchunks.get(index).and_then(Option::as_ref) {
                        if let Some(block_state) = subchunk.block_state_at(local_x, local_y, local_z)
                        {
                            resolved = ResolvedBlockState::Borrowed(block_state);
                        } else if let Some(id) =
                            subchunk.legacy_block_id_at(local_x, local_y, local_z)
                        {
                            resolved = ResolvedBlockState::Owned(legacy_block_state(
                                id,
                                subchunk.legacy_block_data_at(local_x, local_y, local_z),
                            ));
                        }
                    }
                }
            }

            if resolved.is_missing() && (0..=127).contains(&block_pos.y) {
                if let Ok(index) = legacy_order.binary_search(&chunk_pos) {
                    if let Some(terrain) = legacy_terrain.get(index).and_then(Option::as_ref) {
                        let y = u8::try_from(block_pos.y).map_err(|_| {
                            BedrockWorldError::Validation(format!(
                                "legacy block y={} is outside 0..127",
                                block_pos.y
                            ))
                        })?;
                        if let Some(id) = terrain.block_id_at(local_x, y, local_z) {
                            resolved = ResolvedBlockState::Owned(legacy_block_state(
                                id,
                                terrain.block_data_at(local_x, y, local_z),
                            ));
                        }
                    }
                }
            }

            if visitor(BlockStateQueryRef {
                pos: block_pos,
                state: resolved.as_ref(),
            })? == BlockStateQueryControl::Stop
            {
                stats.stopped = true;
                break;
            }
        }
        Ok(stats)
    }
}

enum ResolvedBlockState<'a> {
    Missing,
    Borrowed(&'a BlockState),
    Owned(BlockState),
}

impl ResolvedBlockState<'_> {
    fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }

    fn as_ref(&self) -> Option<&BlockState> {
        match self {
            Self::Missing => None,
            Self::Borrowed(state) => Some(state),
            Self::Owned(state) => Some(state),
        }
    }
}

fn legacy_block_state(id: u8, data: Option<u8>) -> BlockState {
    let mut states = BTreeMap::new();
    if let Some(data) = data {
        states.insert("data".to_string(), NbtTag::Byte(data as i8));
    }
    BlockState {
        name: format!("legacy:{id}"),
        states,
        version: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::LEGACY_TERRAIN_VALUE_LEN;
    use crate::database::{MemoryStorage, WorldStorage};
    use crate::world::{BedrockWorldOpenOptions, WorldFormatHint};

    #[test]
    fn empty_batch_is_empty() {
        let world = BedrockWorld::from_typed_storage(
            "memory-world",
            MemoryStorage::new(),
            BedrockWorldOpenOptions {
                read_only: false,
                format: WorldFormatHint::LevelDb,
            },
        );
        assert!(
            world
                .get_block_states_at_blocking(Dimension::Overworld, [])
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn legacy_terrain_batch_preserves_input_order() {
        let storage = MemoryStorage::new();
        let chunk_pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let payload = vec![0_u8; LEGACY_TERRAIN_VALUE_LEN];
        storage
            .put(
                &ChunkKey::new(chunk_pos, ChunkRecordTag::LegacyTerrain).encode(),
                &payload,
            )
            .expect("legacy terrain");
        let world = BedrockWorld::from_typed_storage(
            "memory-world",
            storage,
            BedrockWorldOpenOptions {
                read_only: false,
                format: WorldFormatHint::LevelDb,
            },
        );
        let input = [
            BlockPos { x: 1, y: 5, z: 2 },
            BlockPos { x: 3, y: 7, z: 4 },
        ];
        let results = world
            .get_block_states_at_blocking(Dimension::Overworld, input)
            .expect("query");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].pos, input[0]);
        assert_eq!(results[1].pos, input[1]);
        assert_eq!(
            results[0].state.as_ref().map(|state| state.name.as_str()),
            Some("legacy:0")
        );
    }

    #[test]
    fn borrowed_batch_stops_without_cloning_owned_results() {
        let storage = MemoryStorage::new();
        let chunk_pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        storage
            .put(
                &ChunkKey::new(chunk_pos, ChunkRecordTag::LegacyTerrain).encode(),
                &vec![0_u8; LEGACY_TERRAIN_VALUE_LEN],
            )
            .expect("legacy terrain");
        let world = BedrockWorld::from_typed_storage(
            "memory-world",
            storage,
            BedrockWorldOpenOptions {
                read_only: false,
                format: WorldFormatHint::LevelDb,
            },
        );
        let mut visited = 0usize;
        let stats = world
            .for_each_block_state_at_blocking(
                Dimension::Overworld,
                [
                    BlockPos { x: 1, y: 5, z: 2 },
                    BlockPos { x: 3, y: 7, z: 4 },
                ],
                |entry| {
                    visited = visited.saturating_add(1);
                    assert!(entry.state.is_some());
                    Ok(BlockStateQueryControl::Stop)
                },
            )
            .expect("borrowed query");
        assert_eq!(visited, 1);
        assert!(stats.stopped);
        assert_eq!(stats.unique_legacy_chunks, 1);
    }
}
