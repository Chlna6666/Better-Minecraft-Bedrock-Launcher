//! Explicit lossless `LegacyTerrain` record split for Minecraft Bedrock worlds.

use crate::chunk::{LegacyTerrainSplitReport, SubChunkVersion, stage_legacy_terrain_split};
use crate::database::StorageOp;
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
        if batch.is_empty() {
            return Ok(report);
        }

        let mut transaction = self.transaction();
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
        transaction.commit()?;
        Ok(report)
    }
}
