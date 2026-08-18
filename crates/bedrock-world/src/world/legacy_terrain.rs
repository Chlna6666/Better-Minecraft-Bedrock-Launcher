//! Explicit lossless `LegacyTerrain` record split/recombine operations for Minecraft Bedrock worlds.

use crate::chunk::{
    LegacyTerrainCombineReport, LegacyTerrainSplitReport, SubChunkVersion,
    stage_legacy_terrain_combine, stage_legacy_terrain_split,
};
use crate::database::{StorageBatch, StorageOp};
use crate::error::Result;
use crate::world::{BedrockWorld, WorldStorageHandle};

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
