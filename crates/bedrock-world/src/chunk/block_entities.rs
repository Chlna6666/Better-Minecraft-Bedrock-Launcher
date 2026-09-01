//! Batched typed access to Bedrock chunk BlockEntity records.

use crate::surface::{ChunkBlockEntity, ChunkDataRequest, ChunkLoadOptions, WorldThreadingOptions};
use crate::world::{World, StorageBackend};
use crate::chunk::ChunkPos;
use crate::error::{BedrockWorldError, Result};

/// BlockEntity records loaded for one Bedrock chunk position.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkBlockEntities {
    /// Queried chunk position.
    pub pos: ChunkPos,
    /// Parsed BlockEntity records in persisted record order.
    pub entities: Vec<ChunkBlockEntity>,
}

impl ChunkBlockEntities {
    /// Finds the first BlockEntity at an absolute block position.
    #[must_use]
    pub fn at(&self, x: i32, y: i32, z: i32) -> Option<&ChunkBlockEntity> {
        self.entities
            .iter()
            .find(|entity| entity.position == Some([x, y, z]))
    }
}

impl<S> World<S>
where
    S: StorageBackend,
{
    /// Reads BlockEntity records with an explicit world-threading policy.
    ///
    /// Only BlockEntity exact records are requested; SubChunk block indices and biome payloads are
    /// not decoded merely to inspect tile data.
    pub fn chunk_block_entities(
        &self,
        positions: impl IntoIterator<Item = ChunkPos>,
        threading: WorldThreadingOptions,
    ) -> Result<Vec<ChunkBlockEntities>> {
        let positions = positions.into_iter().collect::<Vec<_>>();
        if positions.is_empty() {
            return Ok(Vec::new());
        }

        let mut options =
            ChunkLoadOptions::for_data_request(ChunkDataRequest::new().block_entities());
        options.threading = threading;
        let chunks = self.query_chunk_data_many(positions.iter().copied(), options)?;
        if chunks.len() != positions.len() {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "BlockEntity query returned {} chunks for {} positions",
                chunks.len(),
                positions.len()
            )));
        }

        Ok(chunks
            .into_iter()
            .map(|chunk| ChunkBlockEntities {
                pos: chunk.pos,
                entities: chunk.block_entities,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::Dimension;
    use crate::storage::MemoryStorage;
    use crate::world::{OpenOptions, WorldFormatHint};

    #[test]
    fn empty_block_entity_query_stays_empty() {
        let world = World::from_typed_storage(
            "memory-world",
            MemoryStorage::new(),
            OpenOptions {
                read_only: false,
                format: WorldFormatHint::LevelDb,
            },
        );
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let result = world
            .chunk_block_entities([pos], WorldThreadingOptions::Auto)
            .expect("query BlockEntity");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].pos, pos);
        assert!(result[0].entities.is_empty());
    }
}
