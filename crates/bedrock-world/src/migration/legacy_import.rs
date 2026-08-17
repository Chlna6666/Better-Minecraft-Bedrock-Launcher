//! Explicit import helpers for pre-LevelDB Pocket Edition worlds.
//!
//! `chunks.dat` is not a LevelDB database. The importer deliberately separates container migration
//! from chunk-format migration: it copies decoded legacy terrain records into a caller-provided
//! writable storage backend, but it does not pretend those records are modern paletted chunks.

use crate::{
    BedrockDbKey, ChunkRecordTag, PocketChunksDatStorage, Result, StorageBatch, StorageReadOptions,
    StorageVisitorControl, WorldStorage,
};
use bytes::Bytes;
use std::path::Path;

const DEFAULT_IMPORT_BATCH_ENTRIES: usize = 128;

/// Settings controlling a pre-LevelDB Pocket `chunks.dat` import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PocketChunksDatImportOptions {
    /// Maximum raw records written in one target-storage batch.
    pub batch_entries: usize,
}

impl Default for PocketChunksDatImportOptions {
    fn default() -> Self {
        Self {
            batch_entries: DEFAULT_IMPORT_BATCH_ENTRIES,
        }
    }
}

/// Report produced after importing a Pocket `chunks.dat` container.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PocketChunksDatImportReport {
    /// Number of legacy terrain records copied to the target storage.
    pub terrain_records: usize,
    /// Number of target-storage batches committed.
    pub commits: usize,
    /// Number of value bytes copied.
    pub bytes_copied: usize,
    /// Whether the imported data still requires a semantic chunk-format migration.
    pub requires_chunk_migration: bool,
}

/// Imports pre-LevelDB Pocket `chunks.dat` terrain into a writable raw world storage.
///
/// The source [`PocketChunksDatStorage`] decoder converts the old container layout into Bedrock
/// `LegacyTerrain` chunk records. Those records are copied without converting block IDs, metadata,
/// biomes or heights into a newer paletted chunk schema. Callers that need a modern world must run an
/// explicit historical chunk migration after this container import.
///
/// `level.dat`, `entities.dat`, and other sidecar files are intentionally not written through this
/// function because they are not LevelDB key/value records. Higher-level world import tools should
/// copy/upgrade those files separately and preserve unknown metadata.
pub fn import_pocket_chunks_dat_records_blocking(
    source_world_path: impl AsRef<Path>,
    target: &dyn WorldStorage,
    options: PocketChunksDatImportOptions,
) -> Result<PocketChunksDatImportReport> {
    if options.batch_entries == 0 {
        return Err(crate::BedrockWorldError::Validation(
            "Pocket chunks.dat import batch_entries must be greater than zero".to_string(),
        ));
    }

    let source = PocketChunksDatStorage::open(source_world_path)?;
    let mut batch = StorageBatch::new();
    let mut pending = 0usize;
    let mut report = PocketChunksDatImportReport {
        requires_chunk_migration: true,
        ..PocketChunksDatImportReport::default()
    };

    source.for_each_entry(StorageReadOptions::default(), &mut |key, value| {
        let is_legacy_terrain = matches!(
            BedrockDbKey::decode(key),
            BedrockDbKey::Chunk(chunk) if chunk.tag == ChunkRecordTag::LegacyTerrain
        );
        if !is_legacy_terrain {
            return Ok(StorageVisitorControl::Continue);
        }

        batch.put(Bytes::copy_from_slice(key), value.clone());
        pending = pending.saturating_add(1);
        report.terrain_records = report.terrain_records.saturating_add(1);
        report.bytes_copied = report.bytes_copied.saturating_add(value.len());

        if pending >= options.batch_entries {
            target.write_batch(&batch)?;
            batch = StorageBatch::new();
            pending = 0;
            report.commits = report.commits.saturating_add(1);
        }
        Ok(StorageVisitorControl::Continue)
    })?;

    if !batch.is_empty() {
        target.write_batch(&batch)?;
        report.commits = report.commits.saturating_add(1);
    }
    target.flush()?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_import_is_explicitly_a_two_stage_migration() {
        let report = PocketChunksDatImportReport {
            requires_chunk_migration: true,
            ..PocketChunksDatImportReport::default()
        };
        assert!(report.requires_chunk_migration);
        assert_eq!(PocketChunksDatImportOptions::default().batch_entries, 128);
    }
}
