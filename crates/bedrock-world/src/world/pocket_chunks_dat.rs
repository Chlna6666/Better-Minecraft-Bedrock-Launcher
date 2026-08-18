//! Pre-LevelDB Minecraft Pocket Edition `chunks.dat` records.

use crate::chunk::{BedrockDbKey, ChunkRecordTag};
use crate::database::{
    PocketChunksDatStorage, StorageBatch, StorageReadOptions, StorageVisitorControl, WorldStorage,
};
use crate::error::{BedrockWorldError, Result};
use bytes::Bytes;
use std::path::Path;

const DEFAULT_IMPORT_BATCH_ENTRIES: usize = 128;

/// Settings for copying Pocket Edition `chunks.dat` records into a Bedrock world database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PocketChunksDatImportOptions {
    /// Maximum raw records written in one target batch.
    pub batch_entries: usize,
}

impl Default for PocketChunksDatImportOptions {
    fn default() -> Self {
        Self {
            batch_entries: DEFAULT_IMPORT_BATCH_ENTRIES,
        }
    }
}

/// Result of copying one Pocket Edition `chunks.dat` container.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PocketChunksDatImportReport {
    /// Number of `LegacyTerrain` records copied.
    pub terrain_records: usize,
    /// Number of target database batches committed.
    pub commits: usize,
    /// Number of value bytes copied.
    pub bytes_copied: usize,
    /// Whether copied records remain `LegacyTerrain` exactly as read from the source container.
    pub legacy_terrain_retained: bool,
}

/// Copies Pocket Edition `chunks.dat` terrain into a writable raw Bedrock database.
///
/// The source container is decoded into `LegacyTerrain` records and those records are copied without
/// changing block IDs, metadata, biome columns or heights. `level.dat`, `entities.dat` and other
/// sidecar files are intentionally outside this function.
pub fn import_pocket_chunks_dat_records_blocking(
    source_world_path: impl AsRef<Path>,
    target: &dyn WorldStorage,
    options: PocketChunksDatImportOptions,
) -> Result<PocketChunksDatImportReport> {
    if options.batch_entries == 0 {
        return Err(BedrockWorldError::Validation(
            "Pocket chunks.dat import batch_entries must be greater than zero".to_string(),
        ));
    }

    let source = PocketChunksDatStorage::open(source_world_path)?;
    let mut batch = StorageBatch::new();
    let mut pending = 0usize;
    let mut report = PocketChunksDatImportReport {
        legacy_terrain_retained: true,
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
    Ok(report)
}
