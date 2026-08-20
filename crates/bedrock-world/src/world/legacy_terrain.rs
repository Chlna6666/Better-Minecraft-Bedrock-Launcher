//! Explicit lossless `LegacyTerrain` record split/recombine operations for Minecraft Bedrock worlds.

use crate::chunk::{
    BlockState, ChunkKey, ChunkPos, ChunkRecordTag, LegacyTerrain, LegacyTerrainCombineReport,
    LegacyTerrainSplitReport, SubChunkVersion, stage_legacy_terrain_combine,
    stage_legacy_terrain_split,
};
use crate::database::{StorageBatch, StorageOp};
use crate::error::Result;
use crate::nbt::NbtTag;
use crate::world::{BedrockWorld, WorldStorageHandle};
use std::collections::BTreeMap;

impl<S> BedrockWorld<S>
where
    S: WorldStorageHandle,
{
    /// Losslessly separates every `LegacyTerrain` record into fixed-array SubChunks and
    /// `Data2DLegacy` in one world transaction.
    ///
    /// This is a physical Bedrock data conversion, not an implicit game-version upgrade. The caller
    /// explicitly selects V0 or V2-V7 for the resulting numeric SubChunks. Numeric block IDs/data,
    /// sky/block light, height values, biome IDs and saved biome RGB components are all preserved.
    /// Existing destination SubChunk or biome records make the operation fail before commit.
    pub fn split_legacy_terrain_blocking(
        &self,
        subchunk_version: SubChunkVersion,
    ) -> Result<LegacyTerrainSplitReport> {
        let (batch, report) = stage_legacy_terrain_split(self.storage(), subchunk_version)?;
        commit_legacy_terrain_batch(self, &batch)?;
        Ok(report)
    }

    /// Exactly recombines separated V0/V2-V7 SubChunks plus `Data2DLegacy` back into
    /// `LegacyTerrain` in one world transaction.
    ///
    /// This reverse write is accepted only when all eight Y=0..7 fixed-array SubChunks exist, each
    /// still contains sky/block light arrays, no SubChunk exists outside the 0..127 target height,
    /// every `Data2DLegacy` height fits the `LegacyTerrain` `u8` field, and no competing biome or
    /// existing `LegacyTerrain` record is present. Otherwise nothing is written.
    pub fn combine_legacy_terrain_blocking(&self) -> Result<LegacyTerrainCombineReport> {
        let (batch, report) = stage_legacy_terrain_combine(self.storage())?;
        commit_legacy_terrain_batch(self, &batch)?;
        Ok(report)
    }

    pub(crate) fn legacy_terrain_block_state_at(
        &self,
        pos: ChunkPos,
        local_x: u8,
        block_y: i32,
        local_z: u8,
    ) -> Result<Option<BlockState>> {
        let Ok(local_y) = u8::try_from(block_y) else {
            return Ok(None);
        };
        if local_y > 127 {
            return Ok(None);
        }
        let key = ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode();
        let Some(raw) = self.storage().get(&key)? else {
            return Ok(None);
        };
        let terrain = LegacyTerrain::parse(raw)?;
        let Some(id) = terrain.block_id_at(local_x, local_y, local_z) else {
            return Ok(None);
        };
        Ok(Some(legacy_numeric_block_state(
            id,
            terrain.block_data_at(local_x, local_y, local_z),
        )))
    }
}

pub(crate) fn legacy_numeric_block_state(id: u8, data: Option<u8>) -> BlockState {
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

fn commit_legacy_terrain_batch<S>(world: &BedrockWorld<S>, batch: &StorageBatch) -> Result<()>
where
    S: WorldStorageHandle,
{
    if batch.is_empty() {
        return Ok(());
    }
    let mut transaction = world.transaction();
    for op in batch.ops() {
        match op {
            StorageOp::Put { key, value } => {
                transaction.put_raw_key(key.clone(), value.clone());
            }
            StorageOp::Delete { key } => {
                transaction.delete_raw_key(key.clone());
            }
        }
    }
    transaction.commit()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::BlockPos;
    use crate::chunk::{Dimension, LegacyTerrainBuilder};
    use crate::database::{MemoryStorage, WorldStorage};
    use crate::world::BedrockWorldOpenOptions;

    #[test]
    fn direct_block_query_falls_back_to_legacy_terrain() {
        let storage = MemoryStorage::new();
        let pos = ChunkPos {
            x: 2,
            z: -3,
            dimension: Dimension::Overworld,
        };
        let mut terrain = LegacyTerrainBuilder::zeroed();
        terrain.set_block(4, 63, 5, 64, 7).unwrap();
        let terrain = terrain.build().unwrap();
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode(),
                terrain.raw(),
            )
            .unwrap();
        let world =
            BedrockWorld::from_typed_storage("memory", storage, BedrockWorldOpenOptions::default());

        let state = world
            .get_block_state_at_blocking(
                Dimension::Overworld,
                BlockPos {
                    x: pos.x * 16 + 4,
                    y: 63,
                    z: pos.z * 16 + 5,
                },
            )
            .unwrap()
            .unwrap();

        assert_eq!(state.name, "legacy:64");
        assert_eq!(state.state_integer("data").unwrap(), Some(7));
    }
}
