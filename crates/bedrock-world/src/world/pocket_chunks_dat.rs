//! Pre-LevelDB Minecraft Pocket Edition `chunks.dat` conversion boundaries.
//!
//! Reading a Pocket world is lossless through [`crate::world::BedrockWorld::open_auto_blocking`].
//! Converting the historical terrain core into a later LevelDB `LegacyTerrain` record is a different
//! operation: the later record contains 256 persisted biome/RGB samples that old `chunks.dat` does not
//! necessarily contain. This module refuses to invent those missing bytes.

use super::pocket_world_storage::PocketWorldStorage;
use crate::chunk::{BedrockDbKey, ChunkRecordTag, LegacyTerrain};
use crate::storage::{StorageBatch, StorageReadOptions, StorageVisitorControl, WorldStorage};
use crate::error::{BedrockWorldError, Result};
use bytes::Bytes;
use std::path::Path;

/// Settings for explicitly copying Pocket terrain records into a LevelDB-compatible target.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PocketChunksDatImportOptions {
    /// Allows replacing an existing target `LegacyTerrain` record.
    pub overwrite_existing: bool,
}

/// Preflight information for converting one Pocket `chunks.dat` container to LevelDB records.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PocketChunksDatImportCheck {
    /// Number of historical terrain records found.
    pub terrain_records: usize,
    /// Records that already contain the complete later LevelDB biome/RGB tail.
    pub leveldb_complete_records: usize,
    /// Records containing only the older 82,176-byte terrain core.
    pub missing_biome_records: usize,
    /// Total source terrain bytes inspected.
    pub source_bytes: usize,
}

impl PocketChunksDatImportCheck {
    /// Returns whether every terrain record can be copied as a complete LevelDB `LegacyTerrain`
    /// record without synthesising missing biome/RGB data.
    #[must_use]
    pub const fn is_exact_leveldb_copy(&self) -> bool {
        self.missing_biome_records == 0
    }
}

/// Result of one exact Pocket terrain copy into LevelDB-compatible `LegacyTerrain` records.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PocketChunksDatImportReport {
    /// Number of complete `LegacyTerrain` records copied.
    pub terrain_records: usize,
    /// Number of value bytes copied without modification.
    pub bytes_copied: usize,
}

/// Inspects whether a pre-LevelDB Pocket `chunks.dat` can be copied into later `LegacyTerrain`
/// records without inventing biome/RGB bytes.
pub fn check_pocket_chunks_dat_leveldb_import_blocking(
    source_world_path: impl AsRef<Path>,
) -> Result<PocketChunksDatImportCheck> {
    let source = PocketWorldStorage::open(source_world_path)?;
    let mut report = PocketChunksDatImportCheck::default();
    source.for_each_entry(StorageReadOptions::default(), &mut |key, value| {
        let is_legacy_terrain = matches!(
            BedrockDbKey::decode(key),
            BedrockDbKey::Chunk(chunk) if chunk.tag == ChunkRecordTag::LegacyTerrain
        );
        if !is_legacy_terrain {
            return Ok(StorageVisitorControl::Continue);
        }

        let terrain = LegacyTerrain::parse(value.clone())?;
        report.terrain_records = report.terrain_records.saturating_add(1);
        report.source_bytes = report.source_bytes.saturating_add(value.len());
        if terrain.has_biome_samples() {
            report.leveldb_complete_records = report.leveldb_complete_records.saturating_add(1);
        } else {
            report.missing_biome_records = report.missing_biome_records.saturating_add(1);
        }
        Ok(StorageVisitorControl::Continue)
    })?;
    Ok(report)
}

/// Copies only already-complete Pocket terrain records into a writable LevelDB-compatible target.
///
/// Most genuinely pre-LevelDB Pocket worlds contain the 82,176-byte terrain core and therefore lack
/// the 1,024-byte biome/RGB tail required by later `LegacyTerrain`. Such a world remains fully
/// readable through the Pocket backend, but this conversion returns an error before touching the
/// target. A caller that wants a later game-compatible world must obtain or deliberately choose the
/// missing biome data through a separate explicit process; this library does not fabricate it.
///
/// When every source record is complete, all collisions are checked first and all records are written
/// through one [`WorldStorage::write_batch`] call.
pub fn import_pocket_chunks_dat_records_blocking(
    source_world_path: impl AsRef<Path>,
    target: &dyn WorldStorage,
    options: PocketChunksDatImportOptions,
) -> Result<PocketChunksDatImportReport> {
    let source = PocketWorldStorage::open(source_world_path)?;
    let mut records = Vec::<(Bytes, Bytes)>::new();
    let mut missing_biome_records = 0usize;
    let mut source_terrain_records = 0usize;

    source.for_each_entry(StorageReadOptions::default(), &mut |key, value| {
        let is_legacy_terrain = matches!(
            BedrockDbKey::decode(key),
            BedrockDbKey::Chunk(chunk) if chunk.tag == ChunkRecordTag::LegacyTerrain
        );
        if !is_legacy_terrain {
            return Ok(StorageVisitorControl::Continue);
        }

        source_terrain_records = source_terrain_records.saturating_add(1);
        let terrain = LegacyTerrain::parse(value.clone())?;
        if !terrain.has_biome_samples() {
            missing_biome_records = missing_biome_records.saturating_add(1);
            return Ok(StorageVisitorControl::Continue);
        }
        records.push((Bytes::copy_from_slice(key), value.clone()));
        Ok(StorageVisitorControl::Continue)
    })?;

    if missing_biome_records != 0 {
        return Err(BedrockWorldError::Validation(format!(
            "cannot copy Pocket chunks.dat to LevelDB exactly: {missing_biome_records} of {source_terrain_records} terrain records contain only the historical 82,176-byte core and have no persisted biome/RGB tail; source world was not modified and target was not written"
        )));
    }

    if !options.overwrite_existing {
        for (key, _) in &records {
            if target.get(key.as_ref())?.is_some() {
                return Err(BedrockWorldError::Validation(format!(
                    "Pocket chunks.dat import target already contains key {:02x?}",
                    key.as_ref()
                )));
            }
        }
    }

    let mut batch = StorageBatch::new();
    let mut report = PocketChunksDatImportReport::default();
    for (key, value) in records {
        report.terrain_records = report.terrain_records.saturating_add(1);
        report.bytes_copied = report.bytes_copied.saturating_add(value.len());
        batch.put(key, value);
    }
    if !batch.is_empty() {
        target.write_batch(&batch)?;
    }
    Ok(report)
}
