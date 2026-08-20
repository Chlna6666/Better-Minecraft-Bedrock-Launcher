//! High-level lazy world access built on top of the storage layer.
//!
//! The methods in this module are intentionally split into blocking and async
//! forms. Blocking methods are the canonical implementation and are appropriate
//! for CLI tools, background worker threads, and tests. Async methods are thin
//! wrappers that offload the same work with `tokio::task::spawn_blocking`.

use crate::chunk::{
    ActorDigestKey, ActorUid, BedrockDbKey, BedrockDbKeyKind, BlockPos, BlockState,
    BlockStatePaletteEntry, Chunk, ChunkKey, ChunkPos, ChunkRecord, ChunkRecordTag, ChunkVersion,
    GlobalRecordKind, LegacyBiomeSample, LegacyTerrain, MapRecordId, SubChunk, SubChunkDecodeMode,
    block_storage_index, parse_subchunk_with_mode,
};
use crate::entity::{ActorOwnershipIndex, ActorUidRepairReport, stage_actor_uid_repair};
use crate::error::{BedrockWorldError, Result};
use crate::level_dat::{LevelDatDocument, read_level_dat_document, write_level_dat_document};
use crate::nbt::{NbtTag, parse_consecutive_root_nbt, parse_root_nbt, serialize_root_nbt};
use crate::parsed::{
    ActorRecord, ActorSource, Biome3d, BlockEntityRecord, HeightMap2d, ItemStack, ParsedBiomeData,
    ParsedBiomeStorage, ParsedBlockEntity, ParsedChunkData, ParsedDbEntry, ParsedDbValue,
    ParsedEntity, ParsedGlobalData, ParsedHardcodedSpawnArea, ParsedMapData, ParsedVillageData,
    ParsedWorld, WorldParseOptions, WorldParseReport, collect_item_stacks, encode_actor_digest_ids,
    encode_consecutive_roots, encode_global_record, encode_hardcoded_spawn_area_records,
    encode_map_record, parse_actor_digest_ids, parse_block_entities_from_value,
    parse_chunk_records, parse_chunk_records_with_options, parse_data2d_legacy, parse_data3d,
    parse_entities_from_value, parse_global_record, parse_global_storage_entries,
    parse_hardcoded_spawn_area_records, parse_legacy_data2d, parse_map_record, parse_world_storage,
};
use crate::player::{PlayerData, PlayerId};
use crate::storage::backend::BedrockLevelDbStorage;
use crate::storage::{
    PocketChunksDatStorage, StorageBatch, StorageCachePolicy, StorageCancelFlag, StorageOp,
    StorageProgressSink, StorageReadOptions, StorageScanMode, StorageThreadingOptions,
    StorageVisitorControl, WorldStorage,
};
pub use crate::surface::{TerrainSurfaceRole, terrain_surface_overlay_alpha, terrain_surface_role};
use bytes::Bytes;
use rayon::{ThreadPoolBuilder, prelude::*};
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

/// Options used when opening or constructing a [`BedrockWorld`].
#[derive(Debug, Clone)]
pub struct BedrockWorldOpenOptions {
    /// Reject mutating operations when set.
    pub read_only: bool,
    /// Preferred world storage format. [`WorldFormatHint::Auto`] detects the
    /// backend from `db/CURRENT` and old `chunks.dat` files.
    pub format: WorldFormatHint,
}

impl Default for BedrockWorldOpenOptions {
    fn default() -> Self {
        Self {
            read_only: true,
            format: WorldFormatHint::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
/// Preferred storage format selection used when opening a world.
pub enum WorldFormatHint {
    #[default]
    /// Automatically choose the appropriate mode.
    Auto,
    /// Modern Bedrock `LevelDB` world.
    LevelDb,
    /// Pre-`LevelDB` Pocket Edition `chunks.dat` world.
    PocketChunksDat,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
/// Detected world storage format.
pub enum WorldFormat {
    #[default]
    /// Modern Bedrock `LevelDB` world.
    LevelDb,
    /// Old `LevelDB` world using `LegacyTerrain` records.
    LevelDbLegacyTerrain,
    /// Pre-`LevelDB` Pocket Edition `chunks.dat` world.
    PocketChunksDat,
}

/// Lazy handle to a Minecraft Bedrock world folder.
///
/// A handle stores the world path and a storage backend. It does not scan or
/// parse the database until a query method is called.
pub struct BedrockWorld<S = Arc<dyn WorldStorage>> {
    pub(super) path: PathBuf,
    pub(super) options: BedrockWorldOpenOptions,
    pub(super) storage: S,
    pub(super) format: WorldFormat,
}

/// Storage handle accepted by generic [`BedrockWorld`] methods.
pub trait WorldStorageHandle: Clone + Send + Sync + 'static {
    /// Returns the raw storage backend behind this handle.
    fn storage(&self) -> &dyn WorldStorage;
}

impl<T> WorldStorageHandle for T
where
    T: WorldStorage + Clone + Send + Sync + 'static,
{
    fn storage(&self) -> &dyn WorldStorage {
        self
    }
}

impl<T> WorldStorageHandle for Arc<T>
where
    T: WorldStorage + 'static,
{
    fn storage(&self) -> &dyn WorldStorage {
        self.as_ref()
    }
}

impl WorldStorageHandle for Arc<dyn WorldStorage> {
    fn storage(&self) -> &dyn WorldStorage {
        self.as_ref()
    }
}

#[cfg(feature = "async")]
mod async_world;
mod chunk_queries;
mod query_types;
mod world_records;

pub use query_types::*;
use query_types::{
    ChunkDecodeTiming, RawChunkData, RenderRecordKind, RenderRecordRequest, world_executor,
};

impl BedrockWorld<Arc<dyn WorldStorage>> {
    /// Opens a world on the calling thread with automatic format detection.
    pub fn open_blocking(path: impl AsRef<Path>, options: BedrockWorldOpenOptions) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let format = detect_world_format(&path, options.format)?;
        let storage: Arc<dyn WorldStorage> = match format {
            WorldFormat::LevelDb | WorldFormat::LevelDbLegacyTerrain => {
                let db_path = path.join("db");
                if options.read_only {
                    Arc::new(BedrockLevelDbStorage::open_read_only(db_path)?)
                } else {
                    Arc::new(BedrockLevelDbStorage::open(db_path)?)
                }
            }
            WorldFormat::PocketChunksDat => {
                if !options.read_only {
                    log::warn!(
                        "opening legacy chunks.dat world as read-only despite read_only=false"
                    );
                }
                Arc::new(PocketChunksDatStorage::open(&path)?)
            }
        };
        log::debug!(
            "opened Bedrock world (path={}, format={:?}, read_only={})",
            path.display(),
            format,
            options.read_only
        );
        Ok(Self {
            path,
            options,
            storage,
            format,
        })
    }

    #[cfg(feature = "async")]
    /// Opens a world on a blocking worker thread and returns an async handle.
    pub async fn open(path: impl AsRef<Path>, options: BedrockWorldOpenOptions) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        tokio::task::spawn_blocking(move || Self::open_blocking(path, options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[must_use]
    /// Creates a world handle from an already-open storage backend.
    pub fn from_storage(
        path: impl Into<PathBuf>,
        storage: Arc<dyn WorldStorage>,
        options: BedrockWorldOpenOptions,
    ) -> Self {
        Self {
            path: path.into(),
            options,
            storage,
            format: WorldFormat::LevelDb,
        }
    }

    #[must_use]
    /// Creates a world handle from an already-open storage backend and explicit format.
    pub fn from_storage_with_format(
        path: impl Into<PathBuf>,
        storage: Arc<dyn WorldStorage>,
        options: BedrockWorldOpenOptions,
        format: WorldFormat,
    ) -> Self {
        Self {
            path: path.into(),
            options,
            storage,
            format,
        }
    }
}

impl BedrockWorld<BedrockLevelDbStorage> {
    /// Opens a world with a concrete `BedrockLevelDbStorage` backend on the calling thread.
    pub fn open_typed_blocking(
        path: impl AsRef<Path>,
        options: BedrockWorldOpenOptions,
    ) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let format = detect_world_format(&path, options.format)?;
        match format {
            WorldFormat::LevelDb | WorldFormat::LevelDbLegacyTerrain => {
                let db_path = path.join("db");
                let storage = if options.read_only {
                    BedrockLevelDbStorage::open_read_only(db_path)?
                } else {
                    BedrockLevelDbStorage::open(db_path)?
                };
                Ok(Self {
                    path,
                    options,
                    storage,
                    format,
                })
            }
            WorldFormat::PocketChunksDat => Err(BedrockWorldError::UnsupportedChunkFormat(
                "typed LevelDB open does not support legacy chunks.dat worlds".to_string(),
            )),
        }
    }
}

impl<S> BedrockWorld<S>
where
    S: WorldStorageHandle,
{
    #[must_use]
    /// Creates a world handle from a concrete storage backend.
    pub fn from_typed_storage(
        path: impl Into<PathBuf>,
        storage: S,
        options: BedrockWorldOpenOptions,
    ) -> Self {
        Self {
            path: path.into(),
            options,
            storage,
            format: WorldFormat::LevelDb,
        }
    }

    #[must_use]
    /// Creates a world handle from a concrete storage backend and explicit format.
    pub fn from_typed_storage_with_format(
        path: impl Into<PathBuf>,
        storage: S,
        options: BedrockWorldOpenOptions,
        format: WorldFormat,
    ) -> Self {
        Self {
            path: path.into(),
            options,
            storage,
            format,
        }
    }

    #[must_use]
    /// Returns the underlying raw storage backend.
    pub fn storage(&self) -> &dyn WorldStorage {
        self.storage.storage()
    }

    /// Returns the concrete storage handle used by this world.
    pub const fn storage_backend(&self) -> &S {
        &self.storage
    }

    #[must_use]
    /// Returns the world folder path.
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    /// Returns the detected world storage format.
    pub const fn format(&self) -> WorldFormat {
        self.format
    }

    /// Read level dat blocking.
    pub fn read_level_dat_blocking(&self) -> Result<LevelDatDocument> {
        read_level_dat_document(&self.path.join("level.dat"))
    }

    /// Write level dat blocking.
    pub fn write_level_dat_blocking(&self, document: &LevelDatDocument) -> Result<()> {
        self.ensure_writable()?;
        write_level_dat_document(&self.path.join("level.dat"), document)
    }

    /// Compact the underlying world storage after writes.
    pub fn compact_storage_blocking(&self) -> Result<()> {
        self.ensure_writable()?;
        self.storage().compact()
    }

    /// Corrects actor storage tokens from each payload's authoritative NBT `UniqueID`.
    pub fn repair_actor_uids_blocking(&self) -> Result<ActorUidRepairReport> {
        self.ensure_writable()?;
        let (batch, report) = stage_actor_uid_repair(self.storage())?;
        if !batch.is_empty() {
            self.storage().write_batch(&batch)?;
        }
        Ok(report)
    }

    // Chunk and terrain queries are implemented in chunk_queries.
    /// Put raw record blocking.
    pub fn put_raw_record_blocking(&self, key: &ChunkKey, value: &[u8]) -> Result<()> {
        self.ensure_writable()?;
        self.storage().put(&key.encode(), value)
    }

    /// Delete raw record blocking.
    pub fn delete_raw_record_blocking(&self, key: &ChunkKey) -> Result<()> {
        self.ensure_writable()?;
        self.storage().delete(&key.encode())
    }

    #[must_use]
    /// Starts a buffered world transaction.
    pub fn transaction(&self) -> WorldTransaction<'_, S> {
        WorldTransaction {
            storage: &self.storage,
            batch: StorageBatch::new(),
            read_only: self.options.read_only,
            actor_ownership: None,
        }
    }

    pub(super) fn ensure_writable(&self) -> Result<()> {
        if self.options.read_only {
            return Err(BedrockWorldError::ReadOnly);
        }
        Ok(())
    }
}

/// Batched raw record and player writes for a [`BedrockWorld`].
mod transaction;

pub use transaction::WorldTransaction;

mod render_helpers;

use render_helpers::*;

fn detect_world_format(path: &Path, hint: WorldFormatHint) -> Result<WorldFormat> {
    match hint {
        WorldFormatHint::Auto => {
            if path.join("db").join("CURRENT").is_file() {
                return Ok(detect_leveldb_world_format(path));
            }
            if path.join("chunks.dat").is_file() {
                return Ok(WorldFormat::PocketChunksDat);
            }
            Err(BedrockWorldError::Validation(format!(
                "could not detect Bedrock world storage at {}; expected db/CURRENT or chunks.dat",
                path.display()
            )))
        }
        WorldFormatHint::LevelDb => {
            let current = path.join("db").join("CURRENT");
            if !current.is_file() {
                return Err(BedrockWorldError::Validation(format!(
                    "LevelDB world missing {}",
                    current.display()
                )));
            }
            Ok(detect_leveldb_world_format(path))
        }
        WorldFormatHint::PocketChunksDat => {
            let chunks = path.join("chunks.dat");
            if !chunks.is_file() {
                return Err(BedrockWorldError::Validation(format!(
                    "Pocket chunks.dat world missing {}",
                    chunks.display()
                )));
            }
            Ok(WorldFormat::PocketChunksDat)
        }
    }
}

fn detect_leveldb_world_format(path: &Path) -> WorldFormat {
    let Ok(document) = read_level_dat_document(&path.join("level.dat")) else {
        return WorldFormat::LevelDb;
    };
    let NbtTag::Compound(root) = &document.root else {
        return WorldFormat::LevelDb;
    };
    let storage_version = nbt_int_field(root, "StorageVersion");
    let network_version = nbt_int_field(root, "NetworkVersion");
    if storage_version.is_some_and(|version| version <= 4)
        || network_version.is_some_and(|version| version <= 91)
    {
        WorldFormat::LevelDbLegacyTerrain
    } else {
        WorldFormat::LevelDb
    }
}

mod terrain_helpers;

use terrain_helpers::*;

#[cfg(test)]
mod tests;
