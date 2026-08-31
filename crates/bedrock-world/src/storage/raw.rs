//! Raw Minecraft Bedrock storage contracts and the supported LevelDB adapter.
//!
//! Historical container-specific readers live beside this module rather than being normalized into
//! later LevelDB representations. In particular, pre-LevelDB Pocket `chunks.dat` terrain is handled
//! by `database::pocket_chunks`, so this file has no path that can synthesize missing biome bytes.

use crate::error::{BedrockWorldError, Result};
use bytes::Bytes;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{
    Arc, RwLock,
    atomic::{AtomicBool, Ordering},
};

/// Owned raw key/value storage entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageEntry {
    /// Exact persisted key bytes.
    pub key: Bytes,
    /// Exact persisted value bytes.
    pub value: Bytes,
}

/// Borrowed raw key/value storage entry view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageEntryRef<'a> {
    /// Exact persisted key bytes.
    pub key: &'a [u8],
    /// Exact persisted value bytes.
    pub value: &'a [u8],
}

/// Raw storage mutation operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageOp {
    /// Writes or replaces one exact raw value.
    Put {
        /// Exact persisted key bytes.
        key: Bytes,
        /// Exact persisted value bytes.
        value: Bytes,
    },
    /// Deletes one exact raw key.
    Delete {
        /// Exact persisted key bytes.
        key: Bytes,
    },
}

/// Options controlling raw storage reads and scans.
#[derive(Debug, Clone)]
pub struct StorageReadOptions {
    /// Threading policy for this operation.
    pub threading: StorageThreadingOptions,
    /// Scan strategy requested from the backend.
    pub scan_mode: StorageScanMode,
    /// Backend cache strategy for table/data blocks.
    pub cache_policy: StorageCachePolicy,
    /// Bounded pipeline settings for this operation.
    pub pipeline: StoragePipelineOptions,
    /// Optional cooperative cancellation flag.
    pub cancel: Option<StorageCancelFlag>,
    /// Optional progress callback.
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

/// Cache policy for backend storage reads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StorageCachePolicy {
    /// Bypass shared backend caches.
    #[default]
    Bypass,
    /// Use shared backend caches when available.
    Use,
}

/// Bounded pipeline settings for storage scans.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StoragePipelineOptions {
    /// Maximum queued work items; zero selects an automatic default.
    pub queue_depth: usize,
    /// Table batch size; zero selects an automatic default.
    pub table_batch_size: usize,
    /// Progress callback interval; zero selects the backend default.
    pub progress_interval: usize,
}

/// Threading policy for storage operations.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StorageThreadingOptions {
    /// Automatically choose a bounded worker count.
    #[default]
    Auto,
    /// Use an explicit worker count.
    Fixed(usize),
    /// Force the operation onto one worker.
    Single,
}

/// Scan strategy requested from a storage backend.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StorageScanMode {
    /// Scan on one worker in storage order.
    #[default]
    Sequential,
    /// Scan independent table files in parallel when supported.
    ParallelTables,
}

/// Visitor control flow for storage scans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageVisitorControl {
    /// Continue visiting records.
    Continue,
    /// Stop without treating the visitor decision as an error.
    Stop,
}

/// Diagnostics collected by a storage scan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StorageScanOutcome {
    /// Number of visible entries visited.
    pub visited: usize,
    /// Sum of visited value bytes.
    pub bytes_read: usize,
    /// Whether a visitor requested early termination.
    pub stopped: bool,
    /// Number of backend tables scanned.
    pub tables_scanned: usize,
    /// Maximum worker count used by the operation.
    pub worker_threads: usize,
    /// Milliseconds spent waiting for bounded pipeline capacity.
    pub queue_wait_ms: u128,
    /// Number of cooperative cancellation checks.
    pub cancel_checks: usize,
    /// Number of exact point lookups performed by batch APIs.
    pub exact_gets: usize,
    /// Number of exact point-lookup batches.
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
    /// Returns an empty scan result.
    #[must_use]
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

    /// Records one visible value.
    pub fn record(&mut self, value_len: usize) {
        self.visited = self.visited.saturating_add(1);
        self.bytes_read = self.bytes_read.saturating_add(value_len);
    }

    /// Merges another scan result into this one.
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

/// Shareable cooperative cancellation flag.
#[derive(Debug, Clone, Default)]
pub struct StorageCancelFlag(pub(crate) Arc<AtomicBool>);

impl StorageCancelFlag {
    /// Creates a non-cancelled flag.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Requests cancellation.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Wraps a caller-owned shared atomic flag.
    #[must_use]
    pub fn from_shared(cancelled: Arc<AtomicBool>) -> Self {
        Self(cancelled)
    }

    /// Returns whether cancellation was requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Callback sink for storage progress updates.
#[derive(Clone)]
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
    /// Creates a progress sink.
    #[must_use]
    pub fn new(callback: impl Fn(StorageScanProgress) + Send + Sync + 'static) -> Self {
        Self {
            inner: Arc::new(callback),
        }
    }

    /// Emits one progress sample.
    pub fn emit(&self, progress: StorageScanProgress) {
        (self.inner)(progress);
    }
}

/// Progress sample emitted by a storage backend.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StorageScanProgress {
    /// Number of entries observed so far.
    pub entries_seen: usize,
    /// Number of value bytes read so far.
    pub bytes_read: usize,
}

/// Buffered atomic raw-storage mutation batch.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StorageBatch {
    ops: Vec<StorageOp>,
}

impl StorageBatch {
    /// Creates an empty batch.
    #[must_use]
    pub const fn new() -> Self {
        Self { ops: Vec::new() }
    }

    /// Adds a raw put operation.
    pub fn put(&mut self, key: impl Into<Bytes>, value: impl Into<Bytes>) {
        self.ops.push(StorageOp::Put {
            key: key.into(),
            value: value.into(),
        });
    }

    /// Adds a raw delete operation.
    pub fn delete(&mut self, key: impl Into<Bytes>) {
        self.ops.push(StorageOp::Delete { key: key.into() });
    }

    /// Returns whether the batch contains no operations.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// Returns the buffered operations.
    #[must_use]
    pub fn ops(&self) -> &[StorageOp] {
        &self.ops
    }
}

/// Raw key/value storage abstraction used by [`crate::world::BedrockWorld`].
pub trait WorldStorage: Send + Sync {
    /// Reads one exact raw value.
    fn get(&self, key: &[u8]) -> Result<Option<Bytes>>;

    /// Reads exact raw values while preserving input order.
    fn get_many(&self, keys: &[Bytes]) -> Result<Vec<Option<Bytes>>> {
        keys.iter().map(|key| self.get(key)).collect()
    }

    /// Reads exact raw values with cooperative control while preserving input order.
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

    /// Writes one exact raw key/value pair.
    fn put(&self, key: &[u8], value: &[u8]) -> Result<()>;

    /// Deletes one exact raw key.
    fn delete(&self, key: &[u8]) -> Result<()>;

    /// Visits visible keys without forcing value materialization when supported.
    fn for_each_key(
        &self,
        options: StorageReadOptions,
        visitor: &mut (dyn FnMut(&[u8]) -> Result<StorageVisitorControl> + Send),
    ) -> Result<StorageScanOutcome>;

    /// Visits visible key/value records beginning with `prefix`.
    fn for_each_prefix(
        &self,
        prefix: &[u8],
        options: StorageReadOptions,
        visitor: &mut (dyn FnMut(&[u8], &Bytes) -> Result<StorageVisitorControl> + Send),
    ) -> Result<StorageScanOutcome>;

    /// Visits prefix records as borrowed byte views when the backend supports it.
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

    /// Visits prefix keys without requiring value materialization when supported.
    fn for_each_prefix_key(
        &self,
        prefix: &[u8],
        options: StorageReadOptions,
        visitor: &mut (dyn FnMut(&[u8]) -> Result<StorageVisitorControl> + Send),
    ) -> Result<StorageScanOutcome> {
        self.for_each_prefix(prefix, options, &mut |key, _| visitor(key))
    }

    /// Visits every visible raw record.
    fn for_each_entry(
        &self,
        options: StorageReadOptions,
        visitor: &mut (dyn FnMut(&[u8], &Bytes) -> Result<StorageVisitorControl> + Send),
    ) -> Result<StorageScanOutcome> {
        self.for_each_prefix(b"", options, visitor)
    }

    /// Applies all raw operations atomically when the backend supports writes.
    fn write_batch(&self, batch: &StorageBatch) -> Result<()>;

    /// Flushes pending writes to durable storage.
    fn flush(&self) -> Result<()>;

    /// Compacts obsolete table state when supported.
    fn compact(&self) -> Result<()> {
        self.flush()
    }
}

/// Storage backend capable of table-parallel scans with worker-local reduction state.
pub trait PartitionedWorldStorage: WorldStorage {
    /// Scans keys into one independent reduction value per worker.
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

/// In-memory storage backend for tests and synthetic tools.
#[derive(Debug, Clone, Default)]
pub struct MemoryStorage {
    values: Arc<RwLock<BTreeMap<Vec<u8>, Bytes>>>,
}

impl MemoryStorage {
    /// Creates an empty in-memory backend.
    #[must_use]
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
        outcome.worker_threads = 1;
        for (key, value) in values.iter() {
            check_cancelled(&options)?;
            outcome.cancel_checks = outcome.cancel_checks.saturating_add(1);
            outcome.record(value.len());
            if visitor(key)? == StorageVisitorControl::Stop {
                outcome.stopped = true;
                break;
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
        outcome.worker_threads = 1;
        for (key, value) in values
            .range(prefix.to_vec()..)
            .take_while(|(key, _)| key.starts_with(prefix))
        {
            check_cancelled(&options)?;
            outcome.cancel_checks = outcome.cancel_checks.saturating_add(1);
            outcome.record(value.len());
            if visitor(key, value)? == StorageVisitorControl::Stop {
                outcome.stopped = true;
                break;
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
        outcome.worker_threads = 1;
        for (key, value) in values
            .range(prefix.to_vec()..)
            .take_while(|(key, _)| key.starts_with(prefix))
        {
            check_cancelled(&options)?;
            outcome.cancel_checks = outcome.cancel_checks.saturating_add(1);
            outcome.record(value.len());
            if visitor(key)? == StorageVisitorControl::Stop {
                outcome.stopped = true;
                break;
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

/// Concrete backend implementations.
pub mod backend {
    use super::*;

    /// Bedrock LevelDB storage adapter.
    #[cfg(feature = "bedrock-leveldb")]
    #[derive(Clone)]
    pub struct BedrockLevelDbStorage {
        db: Arc<bedrock_leveldb::Db>,
    }

    #[cfg(feature = "bedrock-leveldb")]
    impl BedrockLevelDbStorage {
        /// Opens an existing Bedrock LevelDB for writes.
        pub fn open(path: impl AsRef<Path>) -> Result<Self> {
            Self::open_inner(path, false, true)
        }

        /// Opens an existing Bedrock LevelDB read-only.
        pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self> {
            Self::open_inner(path, true, true)
        }

        /// Opens read-only while allowing invalid table-block checksums.
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
            let options = bedrock_leveldb::LevelDbOpenOptions {
                read_only,
                create_if_missing: false,
                error_if_exists: false,
                paranoid_checks,
                // Mojang Bedrock native table compression id 4: raw DEFLATE.
                compression_policy: bedrock_leveldb::CompressionPolicy::RawDeflate,
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
                // World writes intentionally remain in the WAL overlay until an explicit flush/
                // transaction boundary asks the backend to produce a native table.
                write_buffer_size: 0,
            };
            let db = bedrock_leveldb::Db::open(path, options).map_err(map_leveldb_error)?;
            Ok(Self { db: Arc::new(db) })
        }
    }

    #[cfg(feature = "bedrock-leveldb")]
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
            let mut visitor_error = None;
            let result =
                self.db
                    .for_each_key(to_leveldb_read_options(options), |key| match visitor(key) {
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
            finish_scan(result, visitor_error)
        }

        fn for_each_prefix(
            &self,
            prefix: &[u8],
            options: StorageReadOptions,
            visitor: &mut (dyn FnMut(&[u8], &Bytes) -> Result<StorageVisitorControl> + Send),
        ) -> Result<StorageScanOutcome> {
            let mut visitor_error = None;
            let result =
                self.db
                    .for_each_prefix(prefix, to_leveldb_read_options(options), |key, value| {
                        match visitor(key, value) {
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
                        }
                    });
            finish_scan(result, visitor_error)
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
            let result = self.db.for_each_prefix_ref(prefix, read_options, |entry| {
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
            finish_scan(result, visitor_error)
        }

        fn for_each_prefix_key(
            &self,
            prefix: &[u8],
            options: StorageReadOptions,
            visitor: &mut (dyn FnMut(&[u8]) -> Result<StorageVisitorControl> + Send),
        ) -> Result<StorageScanOutcome> {
            let mut visitor_error = None;
            let result =
                self.db
                    .for_each_prefix_key(prefix, to_leveldb_read_options(options), |key| {
                        match visitor(key) {
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
                        }
                    });
            finish_scan(result, visitor_error)
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
            self.db.flush().map_err(map_leveldb_error)
        }

        fn compact(&self) -> Result<()> {
            self.db.compact().map_err(map_leveldb_error)
        }
    }

    #[cfg(feature = "bedrock-leveldb")]
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
            let result = self.db.scan_keys_partitioned(
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
            let (outcome, partitions) = result.map_err(map_leveldb_error)?;
            Ok((to_storage_outcome(outcome), partitions))
        }
    }

    #[cfg(feature = "bedrock-leveldb")]
    fn finish_scan(
        result: bedrock_leveldb::Result<bedrock_leveldb::ScanOutcome>,
        visitor_error: Option<BedrockWorldError>,
    ) -> Result<StorageScanOutcome> {
        if let Some(error) = visitor_error {
            return Err(error);
        }
        result.map(to_storage_outcome).map_err(map_leveldb_error)
    }

    #[cfg(feature = "bedrock-leveldb")]
    const fn write_options() -> bedrock_leveldb::WriteOptions {
        bedrock_leveldb::WriteOptions { sync: true }
    }

    #[cfg(feature = "bedrock-leveldb")]
    fn map_leveldb_error(error: bedrock_leveldb::LevelDbError) -> BedrockWorldError {
        match error.kind() {
            bedrock_leveldb::ErrorKind::Cancelled => BedrockWorldError::Cancelled {
                operation: "LevelDB scan",
            },
            bedrock_leveldb::ErrorKind::ReadOnly => BedrockWorldError::ReadOnly,
            _ => BedrockWorldError::LevelDb(error.to_string()),
        }
    }

    #[cfg(feature = "bedrock-leveldb")]
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

    #[cfg(feature = "bedrock-leveldb")]
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

    /// Placeholder backend returned when `bedrock-leveldb` is disabled.
    #[cfg(not(feature = "bedrock-leveldb"))]
    #[derive(Debug, Clone, Copy)]
    pub struct BedrockLevelDbStorage;

    #[cfg(not(feature = "bedrock-leveldb"))]
    impl BedrockLevelDbStorage {
        /// Returns an error because the LevelDB backend feature is disabled.
        pub fn open(_path: impl AsRef<Path>) -> Result<Self> {
            Err(feature_disabled())
        }

        /// Returns an error because the LevelDB backend feature is disabled.
        pub fn open_read_only(_path: impl AsRef<Path>) -> Result<Self> {
            Err(feature_disabled())
        }

        /// Returns an error because the LevelDB backend feature is disabled.
        pub fn open_read_only_best_effort(_path: impl AsRef<Path>) -> Result<Self> {
            Err(feature_disabled())
        }
    }

    #[cfg(not(feature = "bedrock-leveldb"))]
    impl WorldStorage for BedrockLevelDbStorage {
        fn get(&self, _key: &[u8]) -> Result<Option<Bytes>> {
            Err(feature_disabled())
        }

        fn get_many(&self, _keys: &[Bytes]) -> Result<Vec<Option<Bytes>>> {
            Err(feature_disabled())
        }

        fn put(&self, _key: &[u8], _value: &[u8]) -> Result<()> {
            Err(feature_disabled())
        }

        fn delete(&self, _key: &[u8]) -> Result<()> {
            Err(feature_disabled())
        }

        fn for_each_key(
            &self,
            _options: StorageReadOptions,
            _visitor: &mut (dyn FnMut(&[u8]) -> Result<StorageVisitorControl> + Send),
        ) -> Result<StorageScanOutcome> {
            Err(feature_disabled())
        }

        fn for_each_prefix(
            &self,
            _prefix: &[u8],
            _options: StorageReadOptions,
            _visitor: &mut (dyn FnMut(&[u8], &Bytes) -> Result<StorageVisitorControl> + Send),
        ) -> Result<StorageScanOutcome> {
            Err(feature_disabled())
        }

        fn write_batch(&self, _batch: &StorageBatch) -> Result<()> {
            Err(feature_disabled())
        }

        fn flush(&self) -> Result<()> {
            Err(feature_disabled())
        }

        fn compact(&self) -> Result<()> {
            Err(feature_disabled())
        }
    }

    #[cfg(not(feature = "bedrock-leveldb"))]
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
            Err(feature_disabled())
        }
    }

    #[cfg(not(feature = "bedrock-leveldb"))]
    fn feature_disabled() -> BedrockWorldError {
        BedrockWorldError::LevelDb("bedrock-leveldb feature is disabled".to_string())
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
    let Some(progress) = &options.progress else {
        return;
    };
    let interval = options.pipeline.progress_interval;
    if interval == 0 || outcome.visited.is_multiple_of(interval) {
        progress.emit(StorageScanProgress {
            entries_seen: outcome.visited,
            bytes_read: outcome.bytes_read,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_storage_scans_prefix_without_copying_values() {
        let storage = MemoryStorage::new();
        storage.put(b"abc1", b"one").unwrap();
        storage.put(b"abc2", b"two").unwrap();
        storage.put(b"abd", b"three").unwrap();

        let mut entries = Vec::new();
        storage
            .for_each_prefix(b"abc", StorageReadOptions::default(), &mut |key, value| {
                entries.push(StorageEntry {
                    key: Bytes::copy_from_slice(key),
                    value: value.clone(),
                });
                Ok(StorageVisitorControl::Continue)
            })
            .unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].key, Bytes::from_static(b"abc1"));
        assert_eq!(entries[1].value, Bytes::from_static(b"two"));
    }

    #[cfg(feature = "bedrock-leveldb")]
    #[test]
    fn bedrock_leveldb_storage_roundtrips_raw_records() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let path = std::env::temp_dir().join(format!(
            "bedrock-world-storage-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        drop(
            bedrock_leveldb::Db::open(&path, bedrock_leveldb::LevelDbOpenOptions::default())
                .unwrap(),
        );

        let storage = backend::BedrockLevelDbStorage::open(&path).unwrap();
        storage.put(b"player_1", b"one").unwrap();
        storage.put(b"player_2", b"two").unwrap();
        storage.flush().unwrap();
        let reopened = backend::BedrockLevelDbStorage::open(&path).unwrap();
        assert_eq!(
            reopened.get(b"player_1").unwrap(),
            Some(Bytes::from_static(b"one"))
        );
        reopened.delete(b"player_2").unwrap();
        reopened.compact().unwrap();
        assert!(reopened.get(b"player_2").unwrap().is_none());

        drop(reopened);
        drop(storage);
        std::fs::remove_dir_all(path).unwrap();
    }
}
