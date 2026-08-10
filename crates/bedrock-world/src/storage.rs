//! Storage abstraction used by [`crate::BedrockWorld`].
//!
//! The trait in this module keeps world parsing independent from a concrete
//! LevelDB implementation. [`MemoryStorage`] is useful for tests and synthetic
//! tools, while [`BedrockLevelDbStorage`](crate::BedrockLevelDbStorage) adapts
//! the `bedrock-leveldb` crate.

use crate::chunk::{ChunkKey, ChunkPos, ChunkRecordTag, Dimension, LEGACY_TERRAIN_VALUE_LEN};
use crate::error::{BedrockWorldError, Result};
use crate::level_dat::read_level_dat_document;
use crate::nbt::NbtTag;
use bytes::Bytes;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::{
    Arc, RwLock,
    atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone, PartialEq, Eq)]
/// Owned raw key/value storage entry.
pub struct StorageEntry {
    /// Decoded storage key for this record.
    pub key: Bytes,
    /// Parsed or raw value associated with this record.
    pub value: Bytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Borrowed raw key/value storage entry view.
pub struct StorageEntryRef<'a> {
    /// Decoded storage key for this record.
    pub key: &'a [u8],
    /// Parsed or raw value associated with this record.
    pub value: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Raw storage mutation operation.
pub enum StorageOp {
    /// Writes or replaces a raw storage value.
    Put {
        /// Decoded storage key for this record.
        key: Bytes,
        /// Raw value bytes written for the key.
        value: Bytes,
    },
    /// Delete operation.
    Delete {
        /// Decoded storage key for this record.
        key: Bytes,
    },
}

#[derive(Debug, Clone)]
/// Options controlling raw storage reads and scans.
pub struct StorageReadOptions {
    /// Threading policy for this operation.
    pub threading: StorageThreadingOptions,
    /// Scan strategy requested from the backend.
    pub scan_mode: StorageScanMode,
    /// Backend cache strategy for table/data blocks.
    pub cache_policy: StorageCachePolicy,
    /// Bounded pipeline settings for this operation.
    pub pipeline: StoragePipelineOptions,
    /// Optional cancellation flag checked during long-running work.
    pub cancel: Option<StorageCancelFlag>,
    /// Optional progress sink invoked during long-running work.
    pub progress: Option<StorageProgressSink>,
}

impl Default for StorageReadOptions {
    fn default() -> Self {
        Self {
            threading: StorageThreadingOptions::Auto,
            scan_mode: StorageScanMode::ParallelTables,
            cache_policy: StorageCachePolicy::Bypass,
            pipeline: StoragePipelineOptions::default(),
            cancel: None,
            progress: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
/// Cache policy for backend storage reads.
pub enum StorageCachePolicy {
    #[default]
    /// Bypass shared backend block caches.
    Bypass,
    /// Use shared backend block caches when available.
    Use,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
/// Bounded pipeline settings for storage scans.
pub struct StoragePipelineOptions {
    /// Maximum queued work items; zero selects an automatic default.
    pub queue_depth: usize,
    /// Table batch size; zero selects an automatic default.
    pub table_batch_size: usize,
    /// Progress callback interval; zero selects an automatic default.
    pub progress_interval: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
/// Threading policy for storage operations.
pub enum StorageThreadingOptions {
    #[default]
    /// Automatically choose the appropriate mode.
    Auto,
    /// Use a fixed worker count.
    Fixed(usize),
    /// Use a single worker.
    Single,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
/// Scan strategy requested from a storage backend.
pub enum StorageScanMode {
    #[default]
    /// Scan sequentially.
    Sequential,
    /// Scan backend tables in parallel when supported.
    ParallelTables,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Visitor control flow for storage scans.
pub enum StorageVisitorControl {
    /// Continue visiting records.
    Continue,
    /// Stop visiting records.
    Stop,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
/// Diagnostics collected by a storage scan.
pub struct StorageScanOutcome {
    /// Number of entries visited by the scan.
    pub visited: usize,
    /// Number of value bytes read while scanning.
    pub bytes_read: usize,
    /// Whether a visitor requested early termination.
    pub stopped: bool,
    /// Number of backend tables scanned.
    pub tables_scanned: usize,
    /// Number of worker threads used by the operation.
    pub worker_threads: usize,
    /// Milliseconds spent waiting for bounded pipeline capacity.
    pub queue_wait_ms: u128,
    /// Number of cancellation checks performed.
    pub cancel_checks: usize,
    /// Number of exact point lookups performed by batch APIs.
    pub exact_gets: usize,
    /// Number of exact point lookup batches performed.
    pub exact_get_batches: usize,
    /// Number of table-index cache hits.
    pub table_index_hits: usize,
    /// Number of table-index cache misses.
    pub table_index_misses: usize,
    /// Number of data-block cache hits.
    pub data_block_hits: usize,
    /// Number of data-block cache misses.
    pub data_block_misses: usize,
}

impl StorageScanOutcome {
    #[must_use]
    /// Returns an empty scan outcome with all counters set to zero.
    pub const fn empty() -> Self {
        Self {
            visited: 0,
            bytes_read: 0,
            stopped: false,
            tables_scanned: 0,
            worker_threads: 0,
            queue_wait_ms: 0,
            cancel_checks: 0,
            exact_gets: 0,
            exact_get_batches: 0,
            table_index_hits: 0,
            table_index_misses: 0,
            data_block_hits: 0,
            data_block_misses: 0,
        }
    }

    /// Records one visited value and its byte length.
    pub fn record(&mut self, value_len: usize) {
        self.visited = self.visited.saturating_add(1);
        self.bytes_read = self.bytes_read.saturating_add(value_len);
    }

    /// Merges another scan outcome into this one.
    pub fn merge(&mut self, other: Self) {
        self.visited = self.visited.saturating_add(other.visited);
        self.bytes_read = self.bytes_read.saturating_add(other.bytes_read);
        self.stopped |= other.stopped;
        self.tables_scanned = self.tables_scanned.saturating_add(other.tables_scanned);
        self.worker_threads = self.worker_threads.max(other.worker_threads);
        self.queue_wait_ms = self.queue_wait_ms.saturating_add(other.queue_wait_ms);
        self.cancel_checks = self.cancel_checks.saturating_add(other.cancel_checks);
        self.exact_gets = self.exact_gets.saturating_add(other.exact_gets);
        self.exact_get_batches = self
            .exact_get_batches
            .saturating_add(other.exact_get_batches);
        self.table_index_hits = self.table_index_hits.saturating_add(other.table_index_hits);
        self.table_index_misses = self
            .table_index_misses
            .saturating_add(other.table_index_misses);
        self.data_block_hits = self.data_block_hits.saturating_add(other.data_block_hits);
        self.data_block_misses = self
            .data_block_misses
            .saturating_add(other.data_block_misses);
    }
}

#[derive(Debug, Clone, Default)]
/// Shareable cancellation flag for storage operations.
pub struct StorageCancelFlag(Arc<AtomicBool>);

impl StorageCancelFlag {
    #[must_use]
    /// Creates a new value.
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation for operations sharing this flag.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    #[must_use]
    /// Creates a cancellation flag from a shared atomic boolean.
    pub fn from_shared(cancelled: Arc<AtomicBool>) -> Self {
        Self(cancelled)
    }

    #[must_use]
    /// Returns whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

#[derive(Clone)]
/// Callback sink for storage progress updates.
pub struct StorageProgressSink {
    inner: Arc<dyn Fn(StorageScanProgress) + Send + Sync>,
}

impl std::fmt::Debug for StorageProgressSink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StorageProgressSink")
            .finish_non_exhaustive()
    }
}

impl StorageProgressSink {
    #[must_use]
    /// Creates a new value.
    pub fn new(callback: impl Fn(StorageScanProgress) + Send + Sync + 'static) -> Self {
        Self {
            inner: Arc::new(callback),
        }
    }

    /// Emits a progress update to the callback.
    pub fn emit(&self, progress: StorageScanProgress) {
        (self.inner)(progress);
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
/// Progress update emitted by a storage backend.
pub struct StorageScanProgress {
    /// Number of entries observed when progress was emitted.
    pub entries_seen: usize,
    /// Number of value bytes read while scanning.
    pub bytes_read: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
/// Buffered batch of raw storage operations.
pub struct StorageBatch {
    ops: Vec<StorageOp>,
}

impl StorageBatch {
    #[must_use]
    /// Creates a new value.
    pub const fn new() -> Self {
        Self { ops: Vec::new() }
    }

    /// Adds a raw put operation to this batch.
    pub fn put(&mut self, key: impl Into<Bytes>, value: impl Into<Bytes>) {
        self.ops.push(StorageOp::Put {
            key: key.into(),
            value: value.into(),
        });
    }

    /// Adds a raw delete operation to this batch.
    pub fn delete(&mut self, key: impl Into<Bytes>) {
        self.ops.push(StorageOp::Delete { key: key.into() });
    }

    #[must_use]
    /// Returns whether this batch contains no operations.
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    #[must_use]
    /// Returns the buffered operations.
    pub fn ops(&self) -> &[StorageOp] {
        &self.ops
    }
}

/// Raw key/value storage abstraction used by [`BedrockWorld`](crate::BedrockWorld).
pub trait WorldStorage: Send + Sync {
    /// Looks up a raw value by exact key.
    fn get(&self, key: &[u8]) -> Result<Option<Bytes>>;
    /// Looks up raw values by exact key, preserving input order.
    fn get_many(&self, keys: &[Bytes]) -> Result<Vec<Option<Bytes>>> {
        keys.iter().map(|key| self.get(key)).collect()
    }
    /// Looks up raw values by exact key with read options and cancellation, preserving input order.
    fn get_many_ordered_with_control(
        &self,
        keys: &[Bytes],
        options: StorageReadOptions,
    ) -> Result<Vec<Option<Bytes>>> {
        check_cancelled(&options)?;
        let mut values = Vec::with_capacity(keys.len());
        for key in keys {
            check_cancelled(&options)?;
            values.push(self.get(key)?);
        }
        Ok(values)
    }
    /// Writes a raw key/value pair.
    fn put(&self, key: &[u8], value: &[u8]) -> Result<()>;
    /// Deletes a raw key.
    fn delete(&self, key: &[u8]) -> Result<()>;
    /// Visits keys without forcing value materialization when the backend can
    /// support key-only scans.
    fn for_each_key(
        &self,
        options: StorageReadOptions,
        visitor: &mut (dyn FnMut(&[u8]) -> Result<StorageVisitorControl> + Send),
    ) -> Result<StorageScanOutcome>;
    /// Visits key/value records whose key starts with `prefix`.
    fn for_each_prefix(
        &self,
        prefix: &[u8],
        options: StorageReadOptions,
        visitor: &mut (dyn FnMut(&[u8], &Bytes) -> Result<StorageVisitorControl> + Send),
    ) -> Result<StorageScanOutcome>;
    /// Visits key/value records as borrowed byte views.
    fn for_each_prefix_ref(
        &self,
        prefix: &[u8],
        options: StorageReadOptions,
        visitor: &mut (dyn FnMut(StorageEntryRef<'_>) -> Result<StorageVisitorControl> + Send),
    ) -> Result<StorageScanOutcome> {
        self.for_each_prefix(prefix, options, &mut |key, value| {
            visitor(StorageEntryRef {
                key,
                value: value.as_ref(),
            })
        })
    }
    /// Visits keys whose key starts with `prefix` without requiring value
    /// materialization when the backend can support key-only scans.
    fn for_each_prefix_key(
        &self,
        prefix: &[u8],
        options: StorageReadOptions,
        visitor: &mut (dyn FnMut(&[u8]) -> Result<StorageVisitorControl> + Send),
    ) -> Result<StorageScanOutcome> {
        self.for_each_prefix(prefix, options, &mut |key, _value| visitor(key))
    }
    /// Visits all key/value records.
    fn for_each_entry(
        &self,
        options: StorageReadOptions,
        visitor: &mut (dyn FnMut(&[u8], &Bytes) -> Result<StorageVisitorControl> + Send),
    ) -> Result<StorageScanOutcome> {
        self.for_each_prefix(b"", options, visitor)
    }
    /// Applies a batch of raw operations atomically when supported by the backend.
    fn write_batch(&self, batch: &StorageBatch) -> Result<()>;
    /// Flushes pending writes to durable storage when supported by the backend.
    fn flush(&self) -> Result<()>;
    /// Rewrites visible storage data to remove obsolete delete markers and old tables.
    fn compact(&self) -> Result<()> {
        self.flush()
    }
}

/// Storage backend capable of table-parallel scans with worker-local reduction state.
///
/// Unlike [`WorldStorage::for_each_key`], this API never serializes successful
/// visitor calls through one shared mutable closure. Each backend worker owns one
/// `T`; callers merge the returned partitions after the scan.
pub trait PartitionedWorldStorage: WorldStorage {
    /// Scans visible keys with one independently initialized reduction value per worker.
    fn scan_keys_partitioned<T, I, F>(
        &self,
        options: StorageReadOptions,
        init: I,
        visitor: F,
    ) -> Result<(StorageScanOutcome, Vec<T>)>
    where
        T: Send,
        I: Fn() -> T + Send + Sync,
        F: Fn(&mut T, &[u8]) -> Result<StorageVisitorControl> + Send + Sync;
}

#[derive(Debug, Clone, Default)]
/// In-memory storage backend for tests and synthetic tools.
pub struct MemoryStorage {
    values: Arc<RwLock<BTreeMap<Vec<u8>, Bytes>>>,
}

impl MemoryStorage {
    #[must_use]
    /// Creates a new value.
    pub fn new() -> Self {
        Self::default()
    }
}

impl WorldStorage for MemoryStorage {
    fn get(&self, key: &[u8]) -> Result<Option<Bytes>> {
        let values = self.values.read().map_err(|_| {
            BedrockWorldError::ConcurrentWrite("memory storage poisoned".to_string())
        })?;
        Ok(values.get(key).cloned())
    }

    fn get_many(&self, keys: &[Bytes]) -> Result<Vec<Option<Bytes>>> {
        let values = self.values.read().map_err(|_| {
            BedrockWorldError::ConcurrentWrite("memory storage poisoned".to_string())
        })?;
        Ok(keys
            .iter()
            .map(|key| values.get(key.as_ref()).cloned())
            .collect())
    }

    fn put(&self, key: &[u8], value: &[u8]) -> Result<()> {
        let mut values = self.values.write().map_err(|_| {
            BedrockWorldError::ConcurrentWrite("memory storage poisoned".to_string())
        })?;
        values.insert(key.to_vec(), Bytes::copy_from_slice(value));
        Ok(())
    }

    fn delete(&self, key: &[u8]) -> Result<()> {
        let mut values = self.values.write().map_err(|_| {
            BedrockWorldError::ConcurrentWrite("memory storage poisoned".to_string())
        })?;
        values.remove(key);
        Ok(())
    }

    fn for_each_key(
        &self,
        options: StorageReadOptions,
        visitor: &mut (dyn FnMut(&[u8]) -> Result<StorageVisitorControl> + Send),
    ) -> Result<StorageScanOutcome> {
        let values = self.values.read().map_err(|_| {
            BedrockWorldError::ConcurrentWrite("memory storage poisoned".to_string())
        })?;
        let mut outcome = StorageScanOutcome::empty();
        for (key, value) in values.iter() {
            check_cancelled(&options)?;
            outcome.record(value.len());
            if visitor(key)? == StorageVisitorControl::Stop {
                outcome.stopped = true;
                return Ok(outcome);
            }
            emit_progress(&options, outcome);
        }
        Ok(outcome)
    }

    fn for_each_prefix(
        &self,
        prefix: &[u8],
        options: StorageReadOptions,
        visitor: &mut (dyn FnMut(&[u8], &Bytes) -> Result<StorageVisitorControl> + Send),
    ) -> Result<StorageScanOutcome> {
        let values = self.values.read().map_err(|_| {
            BedrockWorldError::ConcurrentWrite("memory storage poisoned".to_string())
        })?;
        let mut outcome = StorageScanOutcome::empty();
        for (key, value) in values
            .range(prefix.to_vec()..)
            .take_while(|(key, _)| key.starts_with(prefix))
        {
            check_cancelled(&options)?;
            outcome.record(value.len());
            if visitor(key, value)? == StorageVisitorControl::Stop {
                outcome.stopped = true;
                return Ok(outcome);
            }
            emit_progress(&options, outcome);
        }
        Ok(outcome)
    }

    fn for_each_prefix_key(
        &self,
        prefix: &[u8],
        options: StorageReadOptions,
        visitor: &mut (dyn FnMut(&[u8]) -> Result<StorageVisitorControl> + Send),
    ) -> Result<StorageScanOutcome> {
        let values = self.values.read().map_err(|_| {
            BedrockWorldError::ConcurrentWrite("memory storage poisoned".to_string())
        })?;
        let mut outcome = StorageScanOutcome::empty();
        for (key, value) in values
            .range(prefix.to_vec()..)
            .take_while(|(key, _)| key.starts_with(prefix))
        {
            check_cancelled(&options)?;
            outcome.record(value.len());
            if visitor(key)? == StorageVisitorControl::Stop {
                outcome.stopped = true;
                return Ok(outcome);
            }
            emit_progress(&options, outcome);
        }
        Ok(outcome)
    }

    fn write_batch(&self, batch: &StorageBatch) -> Result<()> {
        let mut values = self.values.write().map_err(|_| {
            BedrockWorldError::ConcurrentWrite("memory storage poisoned".to_string())
        })?;
        for op in batch.ops() {
            match op {
                StorageOp::Put { key, value } => {
                    values.insert(key.to_vec(), value.clone());
                }
                StorageOp::Delete { key } => {
                    values.remove(key.as_ref());
                }
            }
        }
        Ok(())
    }

    fn flush(&self) -> Result<()> {
        Ok(())
    }

    fn compact(&self) -> Result<()> {
        Ok(())
    }
}

impl PartitionedWorldStorage for MemoryStorage {
    fn scan_keys_partitioned<T, I, F>(
        &self,
        options: StorageReadOptions,
        init: I,
        visitor: F,
    ) -> Result<(StorageScanOutcome, Vec<T>)>
    where
        T: Send,
        I: Fn() -> T + Send + Sync,
        F: Fn(&mut T, &[u8]) -> Result<StorageVisitorControl> + Send + Sync,
    {
        let mut partition = init();
        let outcome = self.for_each_key(options, &mut |key| visitor(&mut partition, key))?;
        Ok((outcome, vec![partition]))
    }
}

/// Terrain payload length used by old Pocket Edition `chunks.dat` files before
/// the 1024-byte `[biome_id, red, green, blue]` tail was added to `LevelDB`
/// `LegacyTerrain`.
pub const POCKET_CHUNKS_DAT_TERRAIN_VALUE_LEN: usize = 82_176;
const POCKET_CHUNKS_DAT_LOCATION_TABLE_LEN: usize = 4 * 32 * 32;
const POCKET_CHUNKS_DAT_SECTOR_BYTES: usize = 4096;
const DEFAULT_LEGACY_BIOME_SAMPLE: [u8; 4] = [1, 0x7f, 0xb2, 0x38];

#[derive(Debug, Clone)]
/// Read-only backend for pre-LevelDB Pocket Edition chunks.dat worlds.
pub struct PocketChunksDatStorage {
    values: Arc<BTreeMap<Vec<u8>, Bytes>>,
    origin_chunk_x: i32,
    origin_chunk_z: i32,
}

impl PocketChunksDatStorage {
    /// Opens a read-only backend for a pre-`LevelDB` Pocket Edition world folder.
    pub fn open(world_path: impl AsRef<Path>) -> Result<Self> {
        let world_path = world_path.as_ref();
        let chunks_path = world_path.join("chunks.dat");
        let bytes = fs::read(&chunks_path)?;
        let (origin_chunk_x, origin_chunk_z) = read_limited_world_origin(world_path);
        let values = parse_pocket_chunks_dat(&bytes, origin_chunk_x, origin_chunk_z)?;
        if world_path.join("entities.dat").is_file() {
            match fs::read(world_path.join("entities.dat")) {
                Ok(bytes) => log::debug!(
                    "legacy entities.dat present (bytes={}, parser=best_effort_skip)",
                    bytes.len()
                ),
                Err(error) => log::warn!("failed to read legacy entities.dat: {error}"),
            }
        }
        log::debug!(
            "opened Pocket chunks.dat storage (chunks={}, origin=({}, {}), path={})",
            values.len(),
            origin_chunk_x,
            origin_chunk_z,
            chunks_path.display()
        );
        Ok(Self {
            values: Arc::new(values),
            origin_chunk_x,
            origin_chunk_z,
        })
    }

    #[must_use]
    /// Origin chunk x.
    pub const fn origin_chunk_x(&self) -> i32 {
        self.origin_chunk_x
    }

    #[must_use]
    /// Origin chunk z.
    pub const fn origin_chunk_z(&self) -> i32 {
        self.origin_chunk_z
    }
}

impl WorldStorage for PocketChunksDatStorage {
    fn get(&self, key: &[u8]) -> Result<Option<Bytes>> {
        Ok(self.values.get(key).cloned())
    }

    fn get_many(&self, keys: &[Bytes]) -> Result<Vec<Option<Bytes>>> {
        Ok(keys
            .iter()
            .map(|key| self.values.get(key.as_ref()).cloned())
            .collect())
    }

    fn put(&self, _key: &[u8], _value: &[u8]) -> Result<()> {
        Err(pocket_chunks_dat_read_only_error())
    }

    fn delete(&self, _key: &[u8]) -> Result<()> {
        Err(pocket_chunks_dat_read_only_error())
    }

    fn for_each_key(
        &self,
        options: StorageReadOptions,
        visitor: &mut (dyn FnMut(&[u8]) -> Result<StorageVisitorControl> + Send),
    ) -> Result<StorageScanOutcome> {
        let mut outcome = StorageScanOutcome::empty();
        for (key, value) in self.values.iter() {
            check_cancelled(&options)?;
            outcome.record(value.len());
            if visitor(key)? == StorageVisitorControl::Stop {
                outcome.stopped = true;
                return Ok(outcome);
            }
            emit_progress(&options, outcome);
        }
        Ok(outcome)
    }

    fn for_each_prefix(
        &self,
        prefix: &[u8],
        options: StorageReadOptions,
        visitor: &mut (dyn FnMut(&[u8], &Bytes) -> Result<StorageVisitorControl> + Send),
    ) -> Result<StorageScanOutcome> {
        let mut outcome = StorageScanOutcome::empty();
        for (key, value) in self
            .values
            .range(prefix.to_vec()..)
            .take_while(|(key, _)| key.starts_with(prefix))
        {
            check_cancelled(&options)?;
            outcome.record(value.len());
            if visitor(key, value)? == StorageVisitorControl::Stop {
                outcome.stopped = true;
                return Ok(outcome);
            }
            emit_progress(&options, outcome);
        }
        Ok(outcome)
    }

    fn for_each_prefix_key(
        &self,
        prefix: &[u8],
        options: StorageReadOptions,
        visitor: &mut (dyn FnMut(&[u8]) -> Result<StorageVisitorControl> + Send),
    ) -> Result<StorageScanOutcome> {
        let mut outcome = StorageScanOutcome::empty();
        for (key, value) in self
            .values
            .range(prefix.to_vec()..)
            .take_while(|(key, _)| key.starts_with(prefix))
        {
            check_cancelled(&options)?;
            outcome.record(value.len());
            if visitor(key)? == StorageVisitorControl::Stop {
                outcome.stopped = true;
                return Ok(outcome);
            }
            emit_progress(&options, outcome);
        }
        Ok(outcome)
    }

    fn write_batch(&self, _batch: &StorageBatch) -> Result<()> {
        Err(pocket_chunks_dat_read_only_error())
    }

    fn flush(&self) -> Result<()> {
        Ok(())
    }

    fn compact(&self) -> Result<()> {
        Ok(())
    }
}

fn parse_pocket_chunks_dat(
    bytes: &[u8],
    origin_chunk_x: i32,
    origin_chunk_z: i32,
) -> Result<BTreeMap<Vec<u8>, Bytes>> {
    if bytes.len() < POCKET_CHUNKS_DAT_LOCATION_TABLE_LEN {
        return Err(BedrockWorldError::CorruptWorld(format!(
            "chunks.dat is too small for its location table: {} bytes",
            bytes.len()
        )));
    }
    let mut values = BTreeMap::new();
    for index in 0..(32 * 32) {
        let entry_offset = index * 4;
        let entry = &bytes[entry_offset..entry_offset + 4];
        if entry == [0, 0, 0, 0] {
            continue;
        }
        let sector_count = usize::from(entry[0]);
        let sector_offset =
            usize::from(entry[1]) | (usize::from(entry[2]) << 8) | (usize::from(entry[3]) << 16);
        if sector_count == 0 || sector_offset == 0 {
            continue;
        }
        let Some(byte_offset) = sector_offset.checked_mul(POCKET_CHUNKS_DAT_SECTOR_BYTES) else {
            continue;
        };
        let Some(payload) = pocket_chunk_payload(bytes, byte_offset, sector_count) else {
            log::warn!(
                "skipping invalid chunks.dat entry (index={index}, sector_offset={sector_offset}, sector_count={sector_count})"
            );
            continue;
        };
        let local_x = i32::try_from(index % 32).unwrap_or(0);
        let local_z = i32::try_from(index / 32).unwrap_or(0);
        let pos = ChunkPos {
            x: origin_chunk_x.saturating_add(local_x),
            z: origin_chunk_z.saturating_add(local_z),
            dimension: Dimension::Overworld,
        };
        values.insert(
            ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain)
                .encode()
                .to_vec(),
            convert_pocket_terrain_to_legacy(payload),
        );
    }
    Ok(values)
}

fn pocket_chunk_payload(bytes: &[u8], byte_offset: usize, sector_count: usize) -> Option<&[u8]> {
    let sector_bytes = sector_count.checked_mul(POCKET_CHUNKS_DAT_SECTOR_BYTES)?;
    let max_end = byte_offset.checked_add(sector_bytes)?.min(bytes.len());
    if byte_offset >= bytes.len() || byte_offset >= max_end {
        return None;
    }
    let available = &bytes[byte_offset..max_end];
    if available.len() >= 4 {
        let declared_len = u32::from_le_bytes(available[0..4].try_into().ok()?) as usize;
        if declared_len == POCKET_CHUNKS_DAT_TERRAIN_VALUE_LEN
            && available.len() >= 4 + declared_len
        {
            return Some(&available[4..4 + declared_len]);
        }
        if declared_len == LEGACY_TERRAIN_VALUE_LEN && available.len() >= 4 + declared_len {
            return Some(&available[4..4 + POCKET_CHUNKS_DAT_TERRAIN_VALUE_LEN]);
        }
    }
    if available.len() >= POCKET_CHUNKS_DAT_TERRAIN_VALUE_LEN {
        return Some(&available[..POCKET_CHUNKS_DAT_TERRAIN_VALUE_LEN]);
    }
    None
}

fn convert_pocket_terrain_to_legacy(payload: &[u8]) -> Bytes {
    if payload.len() == LEGACY_TERRAIN_VALUE_LEN {
        return Bytes::copy_from_slice(payload);
    }
    let mut legacy = Vec::with_capacity(LEGACY_TERRAIN_VALUE_LEN);
    legacy.extend_from_slice(&payload[..POCKET_CHUNKS_DAT_TERRAIN_VALUE_LEN]);
    for _ in 0..256 {
        legacy.extend_from_slice(&DEFAULT_LEGACY_BIOME_SAMPLE);
    }
    Bytes::from(legacy)
}

fn read_limited_world_origin(world_path: &Path) -> (i32, i32) {
    let Ok(document) = read_level_dat_document(&world_path.join("level.dat")) else {
        return (0, 0);
    };
    let NbtTag::Compound(root) = document.root else {
        return (0, 0);
    };
    (
        nbt_i32(root.get("LimitedWorldOriginX")).unwrap_or(0),
        nbt_i32(root.get("LimitedWorldOriginZ")).unwrap_or(0),
    )
}

fn nbt_i32(tag: Option<&NbtTag>) -> Option<i32> {
    match tag {
        Some(NbtTag::Byte(value)) => Some(i32::from(*value)),
        Some(NbtTag::Short(value)) => Some(i32::from(*value)),
        Some(NbtTag::Int(value)) => Some(*value),
        Some(NbtTag::Long(value)) => i32::try_from(*value).ok(),
        _ => None,
    }
}

fn pocket_chunks_dat_read_only_error() -> BedrockWorldError {
    BedrockWorldError::UnsupportedChunkFormat("Pocket chunks.dat storage is read-only".to_string())
}

/// Backend module.
pub mod backend {
    use super::*;

    #[cfg(feature = "backend-bedrock-leveldb")]
    #[derive(Clone)]
    /// Bedrock level db storage data model.
    pub struct BedrockLevelDbStorage {
        db: Arc<bedrock_leveldb::Db>,
    }

    #[cfg(feature = "backend-bedrock-leveldb")]
    impl BedrockLevelDbStorage {
        /// Opens a `LevelDB` directory for read/write access.
        pub fn open(path: impl AsRef<Path>) -> Result<Self> {
            Self::open_inner(path, false, true)
        }

        /// Opens a `LevelDB` directory without allowing backend writes.
        pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self> {
            Self::open_inner(path, true, true)
        }

        /// Opens a read-only database while allowing table blocks with invalid checksums.
        ///
        /// This is intended for best-effort inspection of damaged worlds. The returned
        /// storage never writes to the database.
        pub fn open_read_only_best_effort(path: impl AsRef<Path>) -> Result<Self> {
            Self::open_inner(path, true, false)
        }

        fn open_inner(
            path: impl AsRef<Path>,
            read_only: bool,
            paranoid_checks: bool,
        ) -> Result<Self> {
            let path = path.as_ref().to_path_buf();
            if !path.exists() {
                return Err(BedrockWorldError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("LevelDB path not found: {}", path.display()),
                )));
            }
            let options = bedrock_leveldb::OpenOptions {
                read_only,
                create_if_missing: false,
                error_if_exists: false,
                paranoid_checks,
                compression_policy: bedrock_leveldb::CompressionPolicy::Zlib,
                cache: if read_only {
                    bedrock_leveldb::NativeCacheOptions {
                        data_capacity: 32 * 1024 * 1024,
                        index_capacity: 64 * 1024 * 1024,
                        file_capacity: 256,
                        shards: 16,
                    }
                } else {
                    bedrock_leveldb::NativeCacheOptions::default()
                },
                // Ordinary map edits stay WAL-backed. The current native flush path is an
                // explicit full-state rewrite, so an automatic 4 MiB flush would turn a
                // small edit into one table as large as the complete world database.
                write_buffer_size: 0,
            };
            let db = bedrock_leveldb::Db::open(path, options).map_err(map_leveldb_error)?;
            Ok(Self { db: Arc::new(db) })
        }
    }

    #[cfg(feature = "backend-bedrock-leveldb")]
    impl WorldStorage for BedrockLevelDbStorage {
        fn get(&self, key: &[u8]) -> Result<Option<Bytes>> {
            self.db.get(key).map_err(map_leveldb_error)
        }

        fn get_many(&self, keys: &[Bytes]) -> Result<Vec<Option<Bytes>>> {
            self.db
                .get_many_owned(
                    keys.iter().cloned(),
                    bedrock_leveldb::ReadOptions::default(),
                )
                .map_err(map_leveldb_error)
        }

        fn get_many_ordered_with_control(
            &self,
            keys: &[Bytes],
            options: StorageReadOptions,
        ) -> Result<Vec<Option<Bytes>>> {
            check_cancelled(&options)?;
            self.db
                .get_many_owned(keys.iter().cloned(), to_leveldb_read_options(options))
                .map_err(map_leveldb_error)
        }

        fn put(&self, key: &[u8], value: &[u8]) -> Result<()> {
            self.db
                .put(
                    Bytes::copy_from_slice(key),
                    Bytes::copy_from_slice(value),
                    write_options(),
                )
                .map_err(map_leveldb_error)
        }

        fn delete(&self, key: &[u8]) -> Result<()> {
            self.db
                .delete(Bytes::copy_from_slice(key), write_options())
                .map_err(map_leveldb_error)
        }

        fn for_each_key(
            &self,
            options: StorageReadOptions,
            visitor: &mut (dyn FnMut(&[u8]) -> Result<StorageVisitorControl> + Send),
        ) -> Result<StorageScanOutcome> {
            let read_options = to_leveldb_read_options(options);
            let mut visitor_error = None;
            let scan_result = self
                .db
                .for_each_key(read_options, |key| match visitor(key) {
                    Ok(StorageVisitorControl::Continue) => {
                        Ok(bedrock_leveldb::VisitorControl::Continue)
                    }
                    Ok(StorageVisitorControl::Stop) => Ok(bedrock_leveldb::VisitorControl::Stop),
                    Err(error) => {
                        visitor_error = Some(error);
                        Ok(bedrock_leveldb::VisitorControl::Stop)
                    }
                });
            match (scan_result, visitor_error) {
                (_, Some(error)) => Err(error),
                (Ok(outcome), None) => Ok(to_storage_outcome(outcome)),
                (Err(error), None) => Err(map_leveldb_error(error)),
            }
        }

        fn for_each_prefix(
            &self,
            prefix: &[u8],
            options: StorageReadOptions,
            visitor: &mut (dyn FnMut(&[u8], &Bytes) -> Result<StorageVisitorControl> + Send),
        ) -> Result<StorageScanOutcome> {
            let read_options = to_leveldb_read_options(options);
            let mut visitor_error = None;
            let scan_result = self.db.for_each_prefix(prefix, read_options, |key, value| {
                match visitor(key, value) {
                    Ok(StorageVisitorControl::Continue) => {
                        Ok(bedrock_leveldb::VisitorControl::Continue)
                    }
                    Ok(StorageVisitorControl::Stop) => Ok(bedrock_leveldb::VisitorControl::Stop),
                    Err(error) => {
                        visitor_error = Some(error);
                        Ok(bedrock_leveldb::VisitorControl::Stop)
                    }
                }
            });
            match (scan_result, visitor_error) {
                (_, Some(error)) => Err(error),
                (Ok(outcome), None) => Ok(to_storage_outcome(outcome)),
                (Err(error), None) => Err(map_leveldb_error(error)),
            }
        }

        fn for_each_prefix_ref(
            &self,
            prefix: &[u8],
            options: StorageReadOptions,
            visitor: &mut (dyn FnMut(StorageEntryRef<'_>) -> Result<StorageVisitorControl> + Send),
        ) -> Result<StorageScanOutcome> {
            let mut read_options = to_leveldb_read_options(options);
            read_options.read_strategy = bedrock_leveldb::ReadStrategy::Borrowed;
            let mut visitor_error = None;
            let scan_result = self.db.for_each_prefix_ref(prefix, read_options, |entry| {
                match visitor(StorageEntryRef {
                    key: entry.key.as_bytes(),
                    value: entry.value.as_bytes(),
                }) {
                    Ok(StorageVisitorControl::Continue) => {
                        Ok(bedrock_leveldb::VisitorControl::Continue)
                    }
                    Ok(StorageVisitorControl::Stop) => Ok(bedrock_leveldb::VisitorControl::Stop),
                    Err(error) => {
                        visitor_error = Some(error);
                        Ok(bedrock_leveldb::VisitorControl::Stop)
                    }
                }
            });
            match (scan_result, visitor_error) {
                (_, Some(error)) => Err(error),
                (Ok(outcome), None) => Ok(to_storage_outcome(outcome)),
                (Err(error), None) => Err(map_leveldb_error(error)),
            }
        }

        fn for_each_prefix_key(
            &self,
            prefix: &[u8],
            options: StorageReadOptions,
            visitor: &mut (dyn FnMut(&[u8]) -> Result<StorageVisitorControl> + Send),
        ) -> Result<StorageScanOutcome> {
            let read_options = to_leveldb_read_options(options);
            let mut visitor_error = None;
            let scan_result =
                self.db
                    .for_each_prefix_key(prefix, read_options, |key| match visitor(key) {
                        Ok(StorageVisitorControl::Continue) => {
                            Ok(bedrock_leveldb::VisitorControl::Continue)
                        }
                        Ok(StorageVisitorControl::Stop) => {
                            Ok(bedrock_leveldb::VisitorControl::Stop)
                        }
                        Err(error) => {
                            visitor_error = Some(error);
                            Ok(bedrock_leveldb::VisitorControl::Stop)
                        }
                    });
            match (scan_result, visitor_error) {
                (_, Some(error)) => Err(error),
                (Ok(outcome), None) => Ok(to_storage_outcome(outcome)),
                (Err(error), None) => Err(map_leveldb_error(error)),
            }
        }

        fn write_batch(&self, batch: &StorageBatch) -> Result<()> {
            let mut db_batch = bedrock_leveldb::WriteBatch::new();
            for op in batch.ops() {
                match op {
                    StorageOp::Put { key, value } => db_batch.put(key.clone(), value.clone()),
                    StorageOp::Delete { key } => db_batch.delete(key.clone()),
                }
            }
            self.db
                .write(db_batch, write_options())
                .map_err(map_leveldb_error)
        }

        fn flush(&self) -> Result<()> {
            self.db.flush_memtable().map_err(map_leveldb_error)
        }

        fn compact(&self) -> Result<()> {
            self.db
                .compact_range_native(None, None)
                .map_err(map_leveldb_error)
        }
    }

    #[cfg(feature = "backend-bedrock-leveldb")]
    impl PartitionedWorldStorage for BedrockLevelDbStorage {
        fn scan_keys_partitioned<T, I, F>(
            &self,
            options: StorageReadOptions,
            init: I,
            visitor: F,
        ) -> Result<(StorageScanOutcome, Vec<T>)>
        where
            T: Send,
            I: Fn() -> T + Send + Sync,
            F: Fn(&mut T, &[u8]) -> Result<StorageVisitorControl> + Send + Sync,
        {
            let visitor_error = Arc::new(std::sync::Mutex::new(None));
            let visitor_error_for_scan = Arc::clone(&visitor_error);
            let scan_result = self.db.scan_keys_partitioned(
                to_leveldb_read_options(options),
                init,
                move |partition, key| match visitor(partition, key) {
                    Ok(StorageVisitorControl::Continue) => {
                        Ok(bedrock_leveldb::VisitorControl::Continue)
                    }
                    Ok(StorageVisitorControl::Stop) => Ok(bedrock_leveldb::VisitorControl::Stop),
                    Err(error) => {
                        if let Ok(mut slot) = visitor_error_for_scan.lock()
                            && slot.is_none()
                        {
                            *slot = Some(error);
                        }
                        Ok(bedrock_leveldb::VisitorControl::Stop)
                    }
                },
            );
            if let Ok(mut slot) = visitor_error.lock()
                && let Some(error) = slot.take()
            {
                return Err(error);
            }
            let (outcome, partitions) = scan_result.map_err(map_leveldb_error)?;
            Ok((to_storage_outcome(outcome), partitions))
        }
    }

    #[cfg(feature = "backend-bedrock-leveldb")]
    const fn write_options() -> bedrock_leveldb::WriteOptions {
        bedrock_leveldb::WriteOptions { sync: true }
    }

    #[cfg(feature = "backend-bedrock-leveldb")]
    fn map_leveldb_error(error: bedrock_leveldb::LevelDbError) -> BedrockWorldError {
        match error.kind() {
            bedrock_leveldb::ErrorKind::Cancelled => BedrockWorldError::Cancelled {
                operation: "LevelDB scan",
            },
            bedrock_leveldb::ErrorKind::ReadOnly => BedrockWorldError::ReadOnly,
            _ => BedrockWorldError::LevelDb(error.to_string()),
        }
    }

    #[cfg(feature = "backend-bedrock-leveldb")]
    fn to_leveldb_read_options(options: StorageReadOptions) -> bedrock_leveldb::ReadOptions {
        bedrock_leveldb::ReadOptions {
            checksum: bedrock_leveldb::ChecksumMode::Inherit,
            cache_policy: match options.cache_policy {
                StorageCachePolicy::Bypass => bedrock_leveldb::CachePolicy::Bypass,
                StorageCachePolicy::Use => bedrock_leveldb::CachePolicy::Use,
            },
            read_strategy: bedrock_leveldb::ReadStrategy::Shared,
            threading: match options.threading {
                StorageThreadingOptions::Auto => bedrock_leveldb::ThreadingOptions::Auto,
                StorageThreadingOptions::Fixed(threads) => {
                    bedrock_leveldb::ThreadingOptions::Fixed(threads)
                }
                StorageThreadingOptions::Single => bedrock_leveldb::ThreadingOptions::Single,
            },
            scan_mode: match options.scan_mode {
                StorageScanMode::Sequential => bedrock_leveldb::ScanMode::Sequential,
                StorageScanMode::ParallelTables => bedrock_leveldb::ScanMode::ParallelTables,
            },
            pipeline: bedrock_leveldb::ScanPipelineOptions {
                queue_depth: options.pipeline.queue_depth,
                table_batch_size: options.pipeline.table_batch_size,
                progress_interval: options.pipeline.progress_interval,
            },
            cancel: options
                .cancel
                .map(|cancel| bedrock_leveldb::ScanCancelFlag::from_shared(cancel.0)),
            progress: options.progress.map(|progress| {
                bedrock_leveldb::ScanProgressSink::new(move |db_progress| {
                    progress.emit(StorageScanProgress {
                        entries_seen: db_progress.visited,
                        bytes_read: db_progress.bytes_read,
                    });
                })
            }),
        }
    }

    #[cfg(feature = "backend-bedrock-leveldb")]
    const fn to_storage_outcome(outcome: bedrock_leveldb::ScanOutcome) -> StorageScanOutcome {
        StorageScanOutcome {
            visited: outcome.visited,
            bytes_read: outcome.bytes_read,
            stopped: outcome.stopped,
            tables_scanned: outcome.tables_scanned,
            worker_threads: outcome.worker_threads,
            queue_wait_ms: outcome.queue_wait_ms,
            cancel_checks: outcome.cancel_checks,
            exact_gets: outcome.exact_gets,
            exact_get_batches: outcome.exact_get_batches,
            table_index_hits: outcome.table_index_hits,
            table_index_misses: outcome.table_index_misses,
            data_block_hits: outcome.data_block_hits,
            data_block_misses: outcome.data_block_misses,
        }
    }

    #[cfg(not(feature = "backend-bedrock-leveldb"))]
    #[derive(Debug, Clone, Copy)]
    /// Placeholder backend returned when `backend-bedrock-leveldb` is disabled.
    pub struct BedrockLevelDbStorage;

    #[cfg(not(feature = "backend-bedrock-leveldb"))]
    impl BedrockLevelDbStorage {
        /// Returns an error because the LevelDB backend feature is disabled.
        pub fn open(_path: impl AsRef<Path>) -> Result<Self> {
            Err(BedrockWorldError::LevelDb(
                "backend-bedrock-leveldb feature is disabled".to_string(),
            ))
        }

        /// Returns an error because the LevelDB backend feature is disabled.
        pub fn open_read_only(_path: impl AsRef<Path>) -> Result<Self> {
            Err(BedrockWorldError::LevelDb(
                "backend-bedrock-leveldb feature is disabled".to_string(),
            ))
        }

        /// Returns an error because the LevelDB backend feature is disabled.
        pub fn open_read_only_best_effort(_path: impl AsRef<Path>) -> Result<Self> {
            Err(BedrockWorldError::LevelDb(
                "backend-bedrock-leveldb feature is disabled".to_string(),
            ))
        }
    }

    #[cfg(not(feature = "backend-bedrock-leveldb"))]
    impl PartitionedWorldStorage for BedrockLevelDbStorage {
        fn scan_keys_partitioned<T, I, F>(
            &self,
            _options: StorageReadOptions,
            _init: I,
            _visitor: F,
        ) -> Result<(StorageScanOutcome, Vec<T>)>
        where
            T: Send,
            I: Fn() -> T + Send + Sync,
            F: Fn(&mut T, &[u8]) -> Result<StorageVisitorControl> + Send + Sync,
        {
            Err(BedrockWorldError::LevelDb(
                "backend-bedrock-leveldb feature is disabled".to_string(),
            ))
        }
    }

    #[cfg(not(feature = "backend-bedrock-leveldb"))]
    impl WorldStorage for BedrockLevelDbStorage {
        fn get(&self, _key: &[u8]) -> Result<Option<Bytes>> {
            Err(BedrockWorldError::LevelDb(
                "backend-bedrock-leveldb feature is disabled".to_string(),
            ))
        }

        fn get_many(&self, _keys: &[Bytes]) -> Result<Vec<Option<Bytes>>> {
            Err(BedrockWorldError::LevelDb(
                "backend-bedrock-leveldb feature is disabled".to_string(),
            ))
        }

        fn put(&self, _key: &[u8], _value: &[u8]) -> Result<()> {
            Err(BedrockWorldError::LevelDb(
                "backend-bedrock-leveldb feature is disabled".to_string(),
            ))
        }

        fn delete(&self, _key: &[u8]) -> Result<()> {
            Err(BedrockWorldError::LevelDb(
                "backend-bedrock-leveldb feature is disabled".to_string(),
            ))
        }

        fn for_each_key(
            &self,
            _options: StorageReadOptions,
            _visitor: &mut (dyn FnMut(&[u8]) -> Result<StorageVisitorControl> + Send),
        ) -> Result<StorageScanOutcome> {
            Err(BedrockWorldError::LevelDb(
                "backend-bedrock-leveldb feature is disabled".to_string(),
            ))
        }

        fn for_each_prefix(
            &self,
            _prefix: &[u8],
            _options: StorageReadOptions,
            _visitor: &mut (dyn FnMut(&[u8], &Bytes) -> Result<StorageVisitorControl> + Send),
        ) -> Result<StorageScanOutcome> {
            Err(BedrockWorldError::LevelDb(
                "backend-bedrock-leveldb feature is disabled".to_string(),
            ))
        }

        fn write_batch(&self, _batch: &StorageBatch) -> Result<()> {
            Err(BedrockWorldError::LevelDb(
                "backend-bedrock-leveldb feature is disabled".to_string(),
            ))
        }

        fn flush(&self) -> Result<()> {
            Err(BedrockWorldError::LevelDb(
                "backend-bedrock-leveldb feature is disabled".to_string(),
            ))
        }

        fn compact(&self) -> Result<()> {
            Err(BedrockWorldError::LevelDb(
                "backend-bedrock-leveldb feature is disabled".to_string(),
            ))
        }
    }
}

fn check_cancelled(options: &StorageReadOptions) -> Result<()> {
    if options
        .cancel
        .as_ref()
        .is_some_and(StorageCancelFlag::is_cancelled)
    {
        return Err(BedrockWorldError::Cancelled {
            operation: "storage scan",
        });
    }
    Ok(())
}

fn emit_progress(options: &StorageReadOptions, outcome: StorageScanOutcome) {
    if let Some(progress) = &options.progress {
        progress.emit(StorageScanProgress {
            entries_seen: outcome.visited,
            bytes_read: outcome.bytes_read,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "backend-bedrock-leveldb")]
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn memory_storage_scans_prefix_without_copying_values() {
        let storage = MemoryStorage::new();
        storage.put(b"abc1", b"one").expect("put");
        storage.put(b"abc2", b"two").expect("put");
        storage.put(b"abd", b"three").expect("put");

        let mut entries = Vec::new();
        storage
            .for_each_prefix(b"abc", StorageReadOptions::default(), &mut |key, value| {
                entries.push(StorageEntry {
                    key: Bytes::copy_from_slice(key),
                    value: value.clone(),
                });
                Ok(StorageVisitorControl::Continue)
            })
            .expect("scan");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].key, Bytes::from_static(b"abc1"));
        assert_eq!(entries[1].value, Bytes::from_static(b"two"));
    }

    #[cfg(feature = "backend-bedrock-leveldb")]
    #[test]
    fn bedrock_leveldb_storage_roundtrips_raw_records() {
        let path = std::env::temp_dir().join(format!(
            "bedrock-world-storage-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).expect("create");
        drop(
            bedrock_leveldb::Db::open(&path, bedrock_leveldb::OpenOptions::default())
                .expect("initialize"),
        );

        let storage = backend::BedrockLevelDbStorage::open(&path).expect("open");
        storage.put(b"player_1", b"one").expect("put");
        storage.put(b"player_2", b"two").expect("put");
        storage.flush().expect("flush");
        let table_count = std::fs::read_dir(&path)
            .expect("read dir")
            .filter_map(std::result::Result::ok)
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "ldb")
            })
            .count();
        assert_eq!(table_count, 1);

        let reopened = backend::BedrockLevelDbStorage::open(&path).expect("reopen");
        assert_eq!(
            reopened.get(b"player_1").expect("get"),
            Some(Bytes::from_static(b"one"))
        );
        let mut player_count = 0;
        reopened
            .for_each_prefix(
                b"player_",
                StorageReadOptions::default(),
                &mut |_key, _value| {
                    player_count += 1;
                    Ok(StorageVisitorControl::Continue)
                },
            )
            .expect("scan");
        assert_eq!(player_count, 2);

        reopened.delete(b"player_2").expect("delete");
        reopened.compact().expect("compact");
        assert_eq!(reopened.get(b"player_2").expect("get deleted"), None);

        drop(reopened);
        drop(storage);
        std::fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn pocket_chunks_dat_exposes_virtual_legacy_terrain_records() {
        let path = std::env::temp_dir().join(format!(
            "bedrock-world-pocket-chunks-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).expect("create world dir");
        let mut terrain = vec![0_u8; POCKET_CHUNKS_DAT_TERRAIN_VALUE_LEN];
        let block_index = (1_usize << 11) | (3_usize << 7) | 2_usize;
        let column_index = 3_usize * 16 + 1_usize;
        terrain[block_index] = 42;
        terrain[crate::LEGACY_TERRAIN_BLOCK_COUNT
            + (crate::LEGACY_TERRAIN_BLOCK_COUNT / 2) * 3
            + column_index] = 99;
        let mut chunks = vec![0_u8; POCKET_CHUNKS_DAT_SECTOR_BYTES];
        chunks[0] = 21;
        chunks[1] = 1;
        let mut payload = Vec::new();
        payload.extend_from_slice(&(POCKET_CHUNKS_DAT_TERRAIN_VALUE_LEN as u32).to_le_bytes());
        payload.extend_from_slice(&terrain);
        chunks.extend_from_slice(&payload);
        let padded_len = POCKET_CHUNKS_DAT_SECTOR_BYTES * 22;
        chunks.resize(padded_len, 0);
        std::fs::write(path.join("chunks.dat"), chunks).expect("write chunks.dat");

        let storage = PocketChunksDatStorage::open(&path).expect("open pocket chunks");
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let legacy_key = ChunkKey::new(pos, ChunkRecordTag::LegacyTerrain).encode();
        let missing_key = ChunkKey::new(
            ChunkPos {
                x: 1,
                z: 0,
                dimension: Dimension::Overworld,
            },
            ChunkRecordTag::LegacyTerrain,
        )
        .encode();

        let values = storage
            .get_many(&[missing_key.clone(), legacy_key.clone()])
            .expect("get many");
        assert!(values[0].is_none());
        let Some(value) = &values[1] else {
            panic!("legacy terrain should be present");
        };
        assert_eq!(value.len(), LEGACY_TERRAIN_VALUE_LEN);
        assert_eq!(
            &value[..POCKET_CHUNKS_DAT_TERRAIN_VALUE_LEN],
            terrain.as_slice()
        );
        let terrain = crate::LegacyTerrain::parse(value.clone()).expect("legacy terrain");
        assert_eq!(terrain.block_id_at(1, 2, 3), Some(42));
        assert_eq!(terrain.height_at(1, 3), Some(99));

        let mut keys = Vec::new();
        storage
            .for_each_key(StorageReadOptions::default(), &mut |key| {
                keys.push(Bytes::copy_from_slice(key));
                Ok(StorageVisitorControl::Continue)
            })
            .expect("scan keys");
        assert_eq!(keys, vec![legacy_key]);
        assert!(storage.put(b"x", b"y").is_err());

        std::fs::remove_dir_all(path).expect("cleanup");
    }
}
