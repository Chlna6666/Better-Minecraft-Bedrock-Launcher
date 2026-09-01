//! High-level lazy world access built on top of the storage layer.
//!
//! Synchronous operations are the canonical implementation. Async APIs are adapters that offload
//! blocking storage work without changing Minecraft Bedrock persistence semantics. Transactions for
//! independently opened worlds at the same path share one in-process mutation lock so source
//! validation and the LevelDB batch commit cannot race with each other.

mod create;
/// Filesystem discovery for Minecraft Bedrock world folders.
pub mod discover;
mod migration;

use crate::pocket::PocketWorldStorage;

use crate::chunk::{
    ActorDigestKey, ActorUid, BedrockDbKey, BedrockDbKeyKind, BlockPos, BlockState,
    BlockStatePaletteEntry, ChunkKey, ChunkPos, ChunkRecord, ChunkRecordTag, ChunkVersion,
    GlobalRecordKind, LegacyBiomeSample, LegacyTerrain, MapItemId, SubChunk, SubChunkDecodeMode,
    LevelChunk, block_storage_index,
};
use crate::entity::{ActorOwnershipIndex, ActorUidRepairReport, stage_actor_uid_repair};
use crate::error::{BedrockWorldError, Result};
use crate::level_dat::{LevelDatDocument, read_level_dat_document, write_level_dat_document};
use crate::nbt::{NbtTag, parse_consecutive_root_nbt, parse_root_nbt, serialize_root_nbt};
use crate::scan::{
    ActorRecord, ActorSource, Biome3d, BlockEntityRecord, HeightMap2d, ItemStack, BiomeData,
    BiomeStorage, BlockEntity,
    Actor, Global, HardcodedSpawnArea,
    WorldScan, collect_item_stacks, encode_actor_ids, encode_consecutive_roots,
    encode_global, encode_hardcoded_spawn_areas, encode_map_item,
    decode_actor_ids, parse_block_entities_from_value,
    parse_data2d_legacy, parse_data3d, parse_entities_from_value,
    decode_global, decode_hardcoded_spawn_areas,
    parse_legacy_data2d, decode_map_item, scan_storage,
};
use crate::map_item::SavedData;
use crate::player::{PlayerData, PlayerId};
use crate::village::Entry;
use crate::storage::backend::BedrockLevelDbStorage;
use crate::storage::{
    StorageBatch, StorageCachePolicy, StorageCancelFlag, StorageOp, StorageProgressSink,
    StorageReadOptions, StorageScanMode, StorageThreadingOptions, StorageVisitorControl,
    WorldStorage,
};
use crate::surface::*;
use bytes::Bytes;
use rayon::{ThreadPoolBuilder, prelude::*};
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::Instant;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

/// Options used when opening or constructing a [`World`].
#[derive(Debug, Clone)]
pub struct OpenOptions {
    /// Reject mutating operations when set.
    pub read_only: bool,
    /// Preferred world storage format. [`WorldFormatHint::Auto`] detects the
    /// backend from `db/CURRENT` and old `chunks.dat` files.
    pub format: WorldFormatHint,
}

impl Default for OpenOptions {
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

/// Lazy access to a Minecraft Bedrock world folder.
///
/// A world stores its folder path and storage backend. It does not scan or parse LevelDB until a
/// query method is called. Transactions opened for the same path share an in-process mutation lock.
/// This coordination does not extend to an external Minecraft process.
pub struct World<S = Arc<dyn WorldStorage>> {
    pub(super) path: PathBuf,
    pub(super) options: OpenOptions,
    pub(super) storage: S,
    pub(super) format: WorldFormat,
}

/// Storage backend accepted by generic [`World`] methods.
pub trait StorageBackend: Clone + Send + Sync + 'static {
    /// Returns the raw storage backend.
    fn storage(&self) -> &dyn WorldStorage;
}

impl<T> StorageBackend for T
where
    T: WorldStorage + Clone + Send + Sync + 'static,
{
    fn storage(&self) -> &dyn WorldStorage {
        self
    }
}

impl<T> StorageBackend for Arc<T>
where
    T: WorldStorage + 'static,
{
    fn storage(&self) -> &dyn WorldStorage {
        self.as_ref()
    }
}

impl StorageBackend for Arc<dyn WorldStorage> {
    fn storage(&self) -> &dyn WorldStorage {
        self.as_ref()
    }
}

#[cfg(feature = "async")]
mod r#async;
mod query;
mod scan;

impl World<Arc<dyn WorldStorage>> {
    /// Opens a world on the calling thread with automatic format detection.
    pub fn open(path: impl AsRef<Path>, options: OpenOptions) -> Result<Self> {
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
                Arc::new(PocketWorldStorage::open(&path)?)
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
    /// Opens a world on a blocking worker thread for asynchronous use.
    pub async fn open_async(path: impl AsRef<Path>, options: OpenOptions) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        tokio::task::spawn_blocking(move || Self::open(path, options))
            .await
            .map_err(|error| BedrockWorldError::Join(error.to_string()))?
    }

    #[must_use]
    /// Creates a world from an already-open storage backend.
    pub fn from_storage(
        path: impl Into<PathBuf>,
        storage: Arc<dyn WorldStorage>,
        options: OpenOptions,
    ) -> Self {
        Self {
            path: path.into(),
            options,
            storage,
            format: WorldFormat::LevelDb,
        }
    }

    #[must_use]
    /// Creates a world from an already-open storage backend and explicit format.
    pub fn from_storage_with_format(
        path: impl Into<PathBuf>,
        storage: Arc<dyn WorldStorage>,
        options: OpenOptions,
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

impl World<BedrockLevelDbStorage> {
    /// Opens a world with a concrete `BedrockLevelDbStorage` backend on the calling thread.
    pub fn open_typed(path: impl AsRef<Path>, options: OpenOptions) -> Result<Self> {
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

impl<S> World<S>
where
    S: StorageBackend,
{
    #[must_use]
    /// Creates a world from a concrete storage backend.
    pub fn from_typed_storage(path: impl Into<PathBuf>, storage: S, options: OpenOptions) -> Self {
        Self {
            path: path.into(),
            options,
            storage,
            format: WorldFormat::LevelDb,
        }
    }

    #[must_use]
    /// Creates a world from a concrete storage backend and explicit format.
    pub fn from_typed_storage_with_format(
        path: impl Into<PathBuf>,
        storage: S,
        options: OpenOptions,
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

    /// Returns the concrete storage backend used by this world.
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

    /// Reads the current `level.dat` document from this world folder.
    pub fn read_level_dat(&self) -> Result<LevelDatDocument> {
        read_level_dat_document(&self.path.join("level.dat"))
    }

    /// Replaces this world's `level.dat` document.
    ///
    /// This write is outside LevelDB and therefore outside [`WorldTransaction`] atomicity.
    pub fn write_level_dat(&self, document: &LevelDatDocument) -> Result<()> {
        self.ensure_writable()?;
        write_level_dat_document(&self.path.join("level.dat"), document)
    }

    /// Compacts the underlying world storage after writes.
    pub fn compact_storage(&self) -> Result<()> {
        self.ensure_writable()?;
        self.storage().compact()
    }

    /// Corrects actor storage tokens from each payload's authoritative NBT `UniqueID`.
    pub fn repair_actor_uids(&self) -> Result<ActorUidRepairReport> {
        self.ensure_writable()?;
        let (batch, report) = stage_actor_uid_repair(self.storage())?;
        if !batch.is_empty() {
            self.storage().write_batch(&batch)?;
        }
        Ok(report)
    }

    // Chunk and terrain queries are implemented in the query module.
    /// Writes one exact raw chunk record directly to the storage backend.
    pub fn put_raw(&self, key: &ChunkKey, value: &[u8]) -> Result<()> {
        self.ensure_writable()?;
        self.storage().put(&key.encode(), value)
    }

    /// Deletes one exact raw chunk record directly from the storage backend.
    pub fn delete_raw(&self, key: &ChunkKey) -> Result<()> {
        self.ensure_writable()?;
        self.storage().delete(&key.encode())
    }

    #[must_use]
    /// Starts a buffered LevelDB transaction sharing this world's authoritative mutation boundary.
    ///
    /// Staged player, map, chunk, actor and raw-key changes can be committed in one backend batch.
    /// `level.dat` is a separate file and is intentionally not part of this transaction.
    pub fn transaction(&self) -> WorldTransaction<'_, S> {
        WorldTransaction {
            storage: &self.storage,
            batch: StorageBatch::new(),
            read_only: self.options.read_only,
            actor_ownership: None,
            preconditions: Vec::new(),
            mutation_lock: world_mutation_lock(&self.path),
        }
    }

    pub(super) fn ensure_writable(&self) -> Result<()> {
        if self.options.read_only {
            return Err(BedrockWorldError::ReadOnly);
        }
        Ok(())
    }
}

/// Batched LevelDB mutations for a [`World`].
mod transaction;

pub use transaction::WorldTransaction;


fn world_mutation_lock(path: &Path) -> Arc<Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();
    let registry = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut registry = registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(lock) = registry.get(path).and_then(Weak::upgrade) {
        return lock;
    }
    registry.retain(|_, lock| lock.strong_count() > 0);
    let lock = Arc::new(Mutex::new(()));
    registry.insert(path.to_path_buf(), Arc::downgrade(&lock));
    lock
}

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


#[cfg(test)]
mod tests;

pub use crate::scan::{RetentionMode, ScanCategories, ScanOptions, ScanReport};
pub use create::{BedrockDifficulty, BedrockGameMode, CreateOptions, Spawn};
pub use migration::{
    DowngradeOptions, DowngradeReport, MigrationGap, SubChunkUpgradeOptions, UpgradeOptions,
    UpgradeReport,
};
