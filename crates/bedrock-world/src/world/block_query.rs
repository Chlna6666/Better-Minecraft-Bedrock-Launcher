//! Batched exact block-state queries over Bedrock SubChunk and LegacyTerrain storage.
//!
//! This path is intended for callers that need many sparse block states without loading complete
//! chunk columns. Exact SubChunk keys are deduplicated, fetched in one storage batch and decoded once
//! per SubChunk. Historical `LegacyTerrain` is fetched once per relevant chunk as a fallback.

use super::{BedrockWorld, WorldStorageHandle};
use crate::chunk::{
    BlockPos, BlockState, ChunkKey, ChunkPos, ChunkRecordTag, Dimension, LegacyTerrain,
    SubChunkDecodeMode, parse_subchunk_with_mode,
};
use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use bytes::Bytes;
use std::collections::{BTreeMap, HashMap};

/// One exact block-state query result.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockStateQueryResult {
    /// Absolute queried block position.
    pub pos: BlockPos,
    /// Semantic state when persisted terrain contains a block at this position.
    ///
    /// `None` represents an absent/all-air SubChunk record rather than a storage error.
    pub state: Option<BlockState>,
}

impl<S> BedrockWorld<S>
where
    S: WorldStorageHandle,
{
    /// Reads many exact block states in one dimension while preserving input order.
    ///
    /// Modern V0-V9 SubChunks are decoded with full indices only once per unique SubChunk. Old
    /// LevelDB `LegacyTerrain` chunks are resolved through one exact fallback record per chunk.
    pub fn get_block_states_at_blocking(
        &self,
        dimension: Dimension,
        positions: impl IntoIterator<Item = BlockPos>,
    ) -> Result<Vec<BlockStateQueryResult>> {
        let positions = positions.into_iter().collect::<Vec<_>>();
        if positions.is_empty() {
            return Ok(Vec::new());
        }

        let mut subchunk_keys = BTreeMap::<(ChunkPos, i8), usize>::new();
        let mut legacy_keys = BTreeMap::<ChunkPos, usize>::new();
        for &block_pos in &positions {
            let chunk_pos = block_pos.to_chunk_pos(dimension);
            let subchunk_y_i32 = block_pos.y.div_euclid(16);
            if let Ok(subchunk_y) = i8::try_from(subchunk_y_i32) {
                let next = subchunk_keys.len();
                subchunk_keys.entry((chunk_pos, subchunk_y)).or_insert(next);
            }
            if (0..=127).contains(&block_pos.y) {
                let next = legacy_keys.len();
                legacy_keys.entry(chunk_pos).or_insert(next);
            }
        }

        let mut keys = Vec::<Bytes>::with_capacity(subchunk_keys.len() + legacy_keys.len());
        let mut subchunk_order = Vec::with_capacity(subchunk_keys.len());
        for &(chunk_pos, subchunk_y) in subchunk_keys.keys() {
            subchunk_order.push((chunk_pos, subchunk_y));
            keys.push(ChunkKey::subchunk(chunk_pos, subchunk_y).encode());
        }
        let legacy_base = keys.len();
        let mut legacy_order = Vec::with_capacity(legacy_keys.len());
        for &chunk_pos in legacy_keys.keys() {
            legacy_order.push(chunk_pos);
            keys.push(ChunkKey::new(chunk_pos, ChunkRecordTag::LegacyTerrain).encode());
        }

        let values = self.storage().get_many(&keys)?;
        if values.len() != keys.len() {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "batch block-state read returned {} values for {} exact keys",
                values.len(),
                keys.len()
            )));
        }

        let mut subchunks = HashMap::with_capacity(subchunk_order.len());
        for (index, (chunk_pos, subchunk_y)) in subchunk_order.into_iter().enumerate() {
            if let Some(bytes) = values.get(index).and_then(Option::as_ref) {
                let parsed = parse_subchunk_with_mode(
                    subchunk_y,
                    bytes.clone(),
                    SubChunkDecodeMode::FullIndices,
                )?;
                subchunks.insert((chunk_pos, subchunk_y), parsed);
            }
        }

        let mut legacy_terrain = HashMap::with_capacity(legacy_order.len());
        for (offset, chunk_pos) in legacy_order.into_iter().enumerate() {
            if let Some(bytes) = values
                .get(legacy_base.saturating_add(offset))
                .and_then(Option::as_ref)
            {
                legacy_terrain.insert(chunk_pos, LegacyTerrain::parse(bytes.clone())?);
            }
        }

        let mut results = Vec::with_capacity(positions.len());
        for block_pos in positions {
            let chunk_pos = block_pos.to_chunk_pos(dimension);
            let (local_x, _, local_z) = block_pos.in_chunk_offset();
            let subchunk_y_i32 = block_pos.y.div_euclid(16);
            let local_y = u8::try_from(block_pos.y.rem_euclid(16)).map_err(|_| {
                BedrockWorldError::Validation(format!(
                    "block y={} has invalid local SubChunk offset",
                    block_pos.y
                ))
            })?;

            let mut state = None;
            if let Ok(subchunk_y) = i8::try_from(subchunk_y_i32) {
                if let Some(subchunk) = subchunks.get(&(chunk_pos, subchunk_y)) {
                    if let Some(block_state) = subchunk.block_state_at(local_x, local_y, local_z) {
                        state = Some(block_state.clone());
                    } else if let Some(id) = subchunk.legacy_block_id_at(local_x, local_y, local_z) {
                        state = Some(legacy_block_state(
                            id,
                            subchunk.legacy_block_data_at(local_x, local_y, local_z),
                        ));
                    }
                }
            }

            if state.is_none() && (0..=127).contains(&block_pos.y) {
                if let Some(terrain) = legacy_terrain.get(&chunk_pos) {
                    let y = u8::try_from(block_pos.y).map_err(|_| {
                        BedrockWorldError::Validation(format!(
                            "legacy block y={} is outside 0..127",
                            block_pos.y
                        ))
                    })?;
                    if let Some(id) = terrain.block_id_at(local_x, y, local_z) {
                        state = Some(legacy_block_state(
                            id,
                            terrain.block_data_at(local_x, y, local_z),
                        ));
                    }
                }
            }

            results.push(BlockStateQueryResult {
                pos: block_pos,
                state,
            });
        }
        Ok(results)
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
    use crate::database::{MemoryStorage, WorldStorage};
    use crate::world::{BedrockWorldOpenOptions, WorldFormatHint};

    #[test]
    fn empty_batch_is_allocation_bounded_and_empty() {
        let world = BedrockWorld::from_typed_storage(
            "memory-world",
            MemoryStorage::new(),
            BedrockWorldOpenOptions {
                read_only: false,
                format: WorldFormatHint::LevelDb,
            },
        );
        assert!(world
            .get_block_states_at_blocking(Dimension::Overworld, [])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn legacy_terrain_batch_preserves_input_order() {
        let storage = MemoryStorage::new();
        let chunk_pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        // A zero-filled legacy terrain payload is valid enough for exact old-block lookup and maps
        // to legacy block id/data zero across the column.
        let payload = vec![0_u8; 32_768 + 16_384 + 16_384 + 256];
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
        assert_eq!(results[0].state.as_ref().map(|state| state.name.as_str()), Some("legacy:0"));
    }
}
