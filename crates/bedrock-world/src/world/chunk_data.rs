//! Semantic helpers for composable chunk-data query results.

use super::ChunkData;

impl ChunkData {
    /// Returns whether this particular [`super::ChunkDataRequest`] produced enough requested data to
    /// satisfy the chunk-data loader.
    ///
    /// This is the semantic accessor for the historical `is_loaded` field. It does **not** mean that
    /// the chunk position is absent from persistent storage when false. For example, an exact request
    /// for an all-air SubChunk can legitimately be unsatisfied while the surrounding chunk column is
    /// fully persisted. Use [`super::BedrockWorld::chunk_presence_blocking`] or
    /// [`super::BedrockWorld::chunk_presence_many_blocking`] when persisted existence is required.
    #[must_use]
    pub const fn request_satisfied(&self) -> bool {
        self.is_loaded
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{ChunkPos, ChunkVersion, Dimension};
    use crate::world::ChunkData;
    use std::collections::BTreeMap;

    #[test]
    fn request_satisfied_is_not_a_persistence_alias() {
        let data = ChunkData {
            pos: ChunkPos {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
            is_loaded: false,
            height_map: None,
            legacy_biomes: None,
            legacy_biome_colors: None,
            biome_data: BTreeMap::new(),
            subchunks: BTreeMap::new(),
            block_entities: Vec::new(),
            legacy_terrain: None,
            column_samples: None,
            version: ChunkVersion::New,
        };
        assert!(!data.request_satisfied());
    }
}
