//! Explicit Minecraft Bedrock SubChunk writes to historical numeric fixed-array storage.

use crate::block::LegacyNumericBlockUpgradeTable;
use crate::chunk::{
    LegacyNumericSubChunkWriteReport, SubChunkVersion, stage_paletted_subchunks_as_legacy_numeric,
};
use crate::storage::StorageOp;
use crate::error::Result;
use crate::world::{BedrockWorld, WorldStorageHandle};

impl<S> BedrockWorld<S>
where
    S: WorldStorageHandle,
{
    /// Writes paletted SubChunks as an explicitly selected V0/V2-V7 fixed-array numeric format.
    ///
    /// This method is intentionally independent of `GameVersion`: the caller selects the actual
    /// persisted SubChunk version and supplies a forward-verified historical numeric table built from
    /// [`crate::block::LegacyNumericBlockUpgradeTable::build`]. Existing SubChunks already using the
    /// exact target remain byte-for-byte unchanged. Paletted sources are rewritten only when every
    /// modern semantic BlockState has exactly one historical `(numeric id, metadata)` candidate after
    /// authoritative BlockState upgrade verification.
    ///
    /// Primary metadata must fit four bits. An optional second paletted storage is written as
    /// `BlockExtraData`, where metadata may use the full historical u8. Missing or ambiguous states,
    /// another historical source version, an existing `BlockExtraData` beside a paletted source, a
    /// third storage layer, or data outside the historical vertical range aborts the complete operation
    /// before any database write.
    ///
    /// Because paletted source records do not contain the historical sky/block light arrays, newly
    /// written fixed-array values use the valid short form without those arrays rather than inventing
    /// lighting. Complete old-game downgrade still requires the corresponding biome, actor, item,
    /// `level.dat` and other version-specific checks.
    pub fn write_subchunks_as_legacy_numeric_blocking(
        &self,
        target: SubChunkVersion,
        numeric: &LegacyNumericBlockUpgradeTable,
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
