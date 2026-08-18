//! Explicit Minecraft Bedrock SubChunk writes to historical numeric fixed-array storage.

use crate::block::LegacyNumericBlockStateTable;
use crate::chunk::{
    LegacyNumericSubChunkWriteReport, SubChunkVersion,
    stage_paletted_subchunks_as_legacy_numeric,
};
use crate::database::StorageOp;
use crate::error::Result;
use crate::world::{BedrockWorld, WorldStorageHandle};

impl<S> BedrockWorld<S>
where
    S: WorldStorageHandle,
{
    /// Writes paletted SubChunks as an explicitly selected V0/V2-V7 fixed-array numeric format.
    ///
    /// This method is intentionally independent of `GameVersion`: the caller selects the actual
    /// persisted SubChunk version and supplies the authoritative historical numeric table. Existing
    /// SubChunks already using the exact target remain byte-for-byte unchanged. Paletted sources are
    /// rewritten only when they have one storage layer and every semantic BlockState has exactly one
    /// `(numeric id, metadata)` representation that fits the fixed-array layout. Missing or ambiguous
    /// states, other historical source versions, `BlockExtraData` mixed with a paletted source, and
    /// additional paletted storage layers abort the complete operation before any database write.
    ///
    /// Because paletted source records do not contain the historical sky/block light arrays, newly
    /// written fixed-array values use the valid short form without those arrays rather than inventing
    /// lighting. Use this as a representation write; complete old-game downgrade still requires the
    /// corresponding biome, actor, item, level metadata and other version-specific checks.
    pub fn write_subchunks_as_legacy_numeric_blocking(
        &self,
        target: SubChunkVersion,
        numeric: &LegacyNumericBlockStateTable,
    ) -> Result<LegacyNumericSubChunkWriteReport> {
        let (batch, report) =
            stage_paletted_subchunks_as_legacy_numeric(self.storage(), target, numeric)?;
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
