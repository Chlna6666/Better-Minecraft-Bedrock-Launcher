//! Typed batch access to persisted Bedrock chunk height maps.

use crate::surface::{ChunkDataRequest, ChunkLoadOptions, WorldThreadingOptions};
use crate::world::{World, StorageBackend};
use crate::chunk::ChunkPos;
use crate::error::{BedrockWorldError, Result};

/// Availability of one persisted chunk height map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkHeightMapStatus {
    /// The chunk position does not exist in persistent world storage.
    Missing,
    /// Persisted chunk records exist, but no supported height-map representation was available.
    Unavailable,
    /// Absolute world-Y height values in `[z][x]` order.
    Data([[Option<i16>; 16]; 16]),
}

/// Typed height-map result for one Bedrock chunk position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkHeightMap {
    /// Queried chunk position.
    pub pos: ChunkPos,
    /// Persisted height-map availability and values.
    pub status: ChunkHeightMapStatus,
}

impl ChunkHeightMap {
    /// Returns the height-map values when available.
    #[must_use]
    pub const fn data(&self) -> Option<&[[Option<i16>; 16]; 16]> {
        match &self.status {
            ChunkHeightMapStatus::Data(values) => Some(values),
            ChunkHeightMapStatus::Missing | ChunkHeightMapStatus::Unavailable => None,
        }
    }

    /// Returns whether the chunk itself is present in persistent storage.
    #[must_use]
    pub const fn is_chunk_saved(&self) -> bool {
        !matches!(self.status, ChunkHeightMapStatus::Missing)
    }
}

impl<S> World<S>
where
    S: StorageBackend,
{
    /// Reads height maps with an explicit world-threading policy while preserving input order.
    ///
    /// Data3D/Data2D/legacy decoding is delegated to the existing composable chunk loader. Only
    /// positions whose height map is unavailable pay a persisted-presence check, keeping the common
    /// modern-world path to one batched read/decode operation.
    pub fn chunk_height_maps(
        &self,
        positions: impl IntoIterator<Item = ChunkPos>,
        threading: WorldThreadingOptions,
    ) -> Result<Vec<ChunkHeightMap>> {
        let positions = positions.into_iter().collect::<Vec<_>>();
        if positions.is_empty() {
            return Ok(Vec::new());
        }

        let mut options = ChunkLoadOptions::for_data_request(ChunkDataRequest::new().height_map());
        options.threading = threading;
        let chunks = self.query_chunk_data_many(positions.iter().copied(), options)?;
        if chunks.len() != positions.len() {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "height-map query returned {} chunks for {} positions",
                chunks.len(),
                positions.len()
            )));
        }

        let unresolved = chunks
            .iter()
            .filter(|chunk| chunk.height_map.is_none())
            .map(|chunk| chunk.pos)
            .collect::<Vec<_>>();
        let unresolved_chunks = self.chunk_keys_exist(unresolved.iter().copied(), true)?;
        let mut unresolved_chunks = unresolved_chunks.into_iter();

        let mut results = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            let status = if let Some(values) = chunk.height_map {
                ChunkHeightMapStatus::Data(values)
            } else {
                let exists = unresolved_chunks.next().ok_or_else(|| {
                    BedrockWorldError::CorruptWorld(
                        "height-map chunk results lost input alignment".to_string(),
                    )
                })?;
                if exists {
                    ChunkHeightMapStatus::Unavailable
                } else {
                    ChunkHeightMapStatus::Missing
                }
            };
            results.push(ChunkHeightMap {
                pos: chunk.pos,
                status,
            });
        }
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{ChunkKey, ChunkRecordTag, Dimension};
    use crate::storage::{MemoryStorage, WorldStorage};
    use crate::world::{OpenOptions, WorldFormatHint};

    fn world(storage: MemoryStorage) -> World<MemoryStorage> {
        World::from_typed_storage(
            "memory-world",
            storage,
            OpenOptions {
                read_only: false,
                format: WorldFormatHint::LevelDb,
            },
        )
    }

    #[test]
    fn missing_chunk_is_not_conflated_with_unavailable_height_map() {
        let pos = ChunkPos {
            x: 4,
            z: -2,
            dimension: Dimension::Overworld,
        };
        let result = world(MemoryStorage::new())
            .chunk_height_maps([pos], WorldThreadingOptions::Auto)
            .expect("height map");
        assert_eq!(result[0].status, ChunkHeightMapStatus::Missing);
    }

    #[test]
    fn existing_sparse_chunk_reports_unavailable() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: 1,
            z: 2,
            dimension: Dimension::Overworld,
        };
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::FinalizedState).encode(),
                &2_i32.to_le_bytes(),
            )
            .expect("anchor");
        let result = world(storage)
            .chunk_height_maps([pos], WorldThreadingOptions::Auto)
            .expect("height map");
        assert_eq!(result[0].status, ChunkHeightMapStatus::Unavailable);
    }
}
