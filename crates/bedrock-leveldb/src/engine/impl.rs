use crate::batch::{WriteBatch, WriteOp};
use crate::error::{ErrorKind, LevelDbError, Result};
use crate::manifest::Manifest;
use crate::options::{
    CachePolicy, ChecksumMode, CompressionPolicy, OpenOptions, ReadOptions, ReadStrategy, ScanMode,
    ScanOutcome, VisitorControl, WriteOptions,
};
use crate::table;
use crate::wal;
use bytes::Bytes;
use rayon::ThreadPoolBuilder;
use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::ops::Bound::{Included, Unbounded};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, OnceLock, RwLock,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Instant;

/// Fast or full database statistics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbStats {
    /// Visible entries counted by the selected stats path.
    pub entries: usize,
    /// Table files listed in the current manifest.
    pub tables: usize,
    /// Active log file number.
    pub log_number: u64,
    /// Approximate visible bytes or overlay bytes, depending on the stats path.
    pub approximate_bytes: usize,
}

/// Snapshot of the sharded native table caches.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DbCacheStats {
    /// Decoded data-block cache hits.
    pub data_hits: u64,
    /// Decoded data-block cache misses.
    pub data_misses: u64,
    /// Decoded data-block evictions.
    pub data_evictions: u64,
    /// Table-index cache hits.
    pub index_hits: u64,
    /// Table-index cache misses.
    pub index_misses: u64,
    /// Table-index evictions.
    pub index_evictions: u64,
    /// Open-file cache hits.
    pub file_hits: u64,
    /// Open-file cache misses.
    pub file_misses: u64,
    /// Open-file cache evictions.
    pub file_evictions: u64,
    /// Number of shard lock acquisitions that observed contention.
    pub lock_contention: u64,
    /// Current decoded data-block entry count.
    pub data_entries: usize,
    /// Current table-index entry count.
    pub index_entries: usize,
    /// Current cached open SSTable handle count.
    pub open_handles: usize,
}

/// Summary returned by [`Db::repair`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepairReport {
    /// Table files successfully read during repair.
    pub recovered_tables: usize,
    /// WAL records successfully replayed during repair.
    pub recovered_log_records: usize,
    /// Files ignored because they could not be read.
    pub dropped_files: usize,
}

/// Materialized, immutable view of the database at one sequence.
#[derive(Debug, Clone)]
pub struct Snapshot {
    sequence: u64,
    values: Arc<BTreeMap<Vec<u8>, Bytes>>,
}

impl Snapshot {
    /// Returns the last sequence number included in the snapshot.
    #[must_use]
    pub const fn sequence(&self) -> u64 {
        self.sequence
    }

    /// Returns a copy-on-write handle to the value for `key`, if visible.
    #[must_use]
    pub fn get(&self, key: &[u8]) -> Option<Bytes> {
        self.values.get(key).cloned()
    }

    /// Iterates over all materialized snapshot entries in key order.
    #[must_use]
    pub fn iter(&self) -> RawIterator {
        RawIterator::new(self.values.as_ref(), &[])
    }

    /// Iterates over all materialized snapshot entries whose key starts with `prefix`.
    #[must_use]
    pub fn scan_prefix(&self, prefix: &[u8]) -> PrefixIterator {
        PrefixIterator {
            inner: RawIterator::new(self.values.as_ref(), prefix),
        }
    }
}

impl IntoIterator for &Snapshot {
    type Item = (Bytes, Bytes);
    type IntoIter = RawIterator;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Borrowed or shared key view returned by zero-copy-oriented APIs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyRef<'a> {
    bytes: &'a [u8],
}

impl<'a> KeyRef<'a> {
    /// Creates a key view from raw bytes.
    #[must_use]
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    /// Returns the raw key bytes.
    #[must_use]
    pub const fn as_bytes(self) -> &'a [u8] {
        self.bytes
    }
}

impl AsRef<[u8]> for KeyRef<'_> {
    fn as_ref(&self) -> &[u8] {
        self.bytes
    }
}

/// Value view used by borrowed-first read APIs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueRef<'a> {
    /// Borrowed directly from a caller-owned or mapped buffer.
    Borrowed(&'a [u8]),
    /// Shared immutable bytes. This is used for overlay values and decoded
    /// compressed blocks.
    Shared(Bytes),
    /// Explicitly materialized owned bytes.
    Owned(Bytes),
}

impl ValueRef<'_> {
    /// Returns the value bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Borrowed(bytes) => bytes,
            Self::Shared(bytes) | Self::Owned(bytes) => bytes.as_ref(),
        }
    }

    /// Returns the value length in bytes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.as_bytes().len()
    }

    /// Returns whether this value is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.as_bytes().is_empty()
    }

    /// Materializes this value as [`Bytes`].
    #[must_use]
    pub fn into_bytes(self) -> Bytes {
        match self {
            Self::Borrowed(bytes) => Bytes::copy_from_slice(bytes),
            Self::Shared(bytes) | Self::Owned(bytes) => bytes,
        }
    }

    fn from_shared(bytes: Bytes, strategy: ReadStrategy) -> Self {
        match strategy {
            ReadStrategy::Owned => Self::Owned(Bytes::copy_from_slice(&bytes)),
            ReadStrategy::Borrowed | ReadStrategy::Shared => Self::Shared(bytes),
        }
    }
}

impl AsRef<[u8]> for ValueRef<'_> {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

/// Raw key/value entry view used by visitor-based scans.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryRef<'a> {
    /// Raw key bytes.
    pub key: KeyRef<'a>,
    /// Raw value bytes.
    pub value: ValueRef<'a>,
}

/// Open database handle.
pub struct Db {
    root: PathBuf,
    options: OpenOptions,
    inner: RwLock<DbInner>,
    block_cache: table::NativeBlockCache,
}

#[derive(Debug)]
struct DbInner {
    overlay: BTreeMap<Vec<u8>, Option<Bytes>>,
    manifest: Manifest,
    last_sequence: u64,
    approximate_bytes: usize,
}

type Overlay = BTreeMap<Vec<u8>, Option<Bytes>>;
type LoadedState = (Manifest, Overlay, u64, usize);

// ReadOptions and OpenOptions are intentionally passed by value at the public
// boundary so callers can use struct-update syntax without storing temporaries.
#[allow(clippy::needless_pass_by_value)]
impl Db {
    /// Opens a Bedrock/native `LevelDB` directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the directory is missing and creation is disabled,
    /// when `read_only` would require initialization, when existing metadata is
    /// corrupt, or when filesystem I/O fails.
    pub fn open(path: impl AsRef<Path>, options: OpenOptions) -> Result<Self> {
        let cache_options = options.cache.normalized();
        let root = path.as_ref().to_path_buf();
        log::debug!(
            "opening database at {} (read_only={}, create_if_missing={})",
            root.display(),
            options.read_only,
            options.create_if_missing
        );
        if root.exists() {
            if options.error_if_exists {
                let mut entries = fs::read_dir(&root).map_err(|error| {
                    LevelDbError::io_at("read database directory", &root, error)
                })?;
                if entries
                    .next()
                    .transpose()
                    .map_err(|error| LevelDbError::io_at("read database directory", &root, error))?
                    .is_some()
                {
                    return Err(LevelDbError::already_exists(root.clone()));
                }
            }
        } else if options.read_only {
            return Err(LevelDbError::not_found(root.clone()));
        } else if options.create_if_missing {
            fs::create_dir_all(&root)
                .map_err(|error| LevelDbError::io_at("create database directory", &root, error))?;
        } else {
            return Err(LevelDbError::not_found(root.clone()));
        }

        let (manifest, overlay, last_sequence, approximate_bytes) =
            load_existing_or_initialize(&root, &options)?;
        log::debug!(
            "opened database at {} (tables={}, overlay_entries={}, last_sequence={})",
            root.display(),
            manifest.table_numbers.len(),
            overlay.len(),
            last_sequence
        );
        Ok(Self {
            root,
            options,
            inner: RwLock::new(DbInner {
                overlay,
                manifest,
                last_sequence,
                approximate_bytes,
            }),
            block_cache: table::NativeBlockCache::new(
                cache_options.data_capacity,
                cache_options.index_capacity,
                cache_options.file_capacity,
                cache_options.shards,
            ),
        })
    }

    /// Returns a point-in-time snapshot of cache activity and occupancy.
    #[must_use]
    pub fn cache_stats(&self) -> DbCacheStats {
        let stats = self.block_cache.stats();
        DbCacheStats {
            data_hits: stats.data_hits,
            data_misses: stats.data_misses,
            data_evictions: stats.data_evictions,
            index_hits: stats.index_hits,
            index_misses: stats.index_misses,
            index_evictions: stats.index_evictions,
            file_hits: stats.file_hits,
            file_misses: stats.file_misses,
            file_evictions: stats.file_evictions,
            lock_contention: stats.lock_contention,
            data_entries: stats.data_entries,
            index_entries: stats.index_entries,
            open_handles: stats.open_handles,
        }
    }

    #[cfg(feature = "async")]
    /// Opens a database on a blocking Tokio task.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Db::open`] plus a join error if the blocking
    /// task fails to complete.
    pub async fn open_async(path: impl AsRef<Path>, options: OpenOptions) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        tokio::task::spawn_blocking(move || Self::open(path, options))
            .await
            .map_err(|error| LevelDbError::join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Reads a key on a blocking Tokio task using an owned [`Arc<Db>`].
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Db::get`] plus a join error if the blocking
    /// task fails to complete.
    pub async fn get_async(self: Arc<Self>, key: Bytes) -> Result<Option<Bytes>> {
        tokio::task::spawn_blocking(move || self.get(&key))
            .await
            .map_err(|error| LevelDbError::join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Reads a key on a blocking Tokio task using explicit [`ReadOptions`].
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Db::get_with`] plus a join error if the
    /// blocking task fails to complete.
    pub async fn get_with_async(
        self: Arc<Self>,
        key: Bytes,
        options: ReadOptions,
    ) -> Result<Option<Bytes>> {
        tokio::task::spawn_blocking(move || self.get_with(&key, options))
            .await
            .map_err(|error| LevelDbError::join(error.to_string()))?
    }

    /// Reads a key using default [`ReadOptions`], materializing it as shared
    /// [`Bytes`] for compatibility.
    ///
    /// # Errors
    ///
    /// Returns an error if metadata, table blocks, compression, or filesystem
    /// reads fail.
    pub fn get(&self, key: &[u8]) -> Result<Option<Bytes>> {
        self.get_owned(key)
    }

    /// Reads a key using default [`ReadOptions`] and returns a borrowed-first
    /// value view.
    ///
    /// # Errors
    ///
    /// Returns an error if metadata, table blocks, compression, or filesystem
    /// reads fail.
    pub fn get_ref(&self, key: &[u8]) -> Result<Option<ValueRef<'static>>> {
        self.get_with_ref(key, ReadOptions::default())
    }

    /// Reads a key and explicitly materializes it as [`Bytes`].
    ///
    /// # Errors
    ///
    /// Returns an error if metadata, table blocks, compression, or filesystem
    /// reads fail.
    pub fn get_owned(&self, key: &[u8]) -> Result<Option<Bytes>> {
        Ok(self
            .get_with_ref(key, ReadOptions::default())?
            .map(ValueRef::into_bytes))
    }

    /// Reads a key using explicit [`ReadOptions`].
    ///
    /// # Errors
    ///
    /// Returns an error if metadata, table blocks, compression, checksum
    /// verification, or filesystem reads fail.
    pub fn get_with(&self, key: &[u8], options: ReadOptions) -> Result<Option<Bytes>> {
        Ok(self.get_with_ref(key, options)?.map(ValueRef::into_bytes))
    }

    /// Reads a key using explicit [`ReadOptions`] and returns a borrowed-first
    /// value view. The current safe default uses shared buffers for decoded
    /// table blocks; future `mmap` paths can return [`ValueRef::Borrowed`].
    ///
    /// # Errors
    ///
    /// Returns an error if metadata, table blocks, compression, checksum
    /// verification, or filesystem reads fail.
    pub fn get_with_ref(
        &self,
        key: &[u8],
        options: ReadOptions,
    ) -> Result<Option<ValueRef<'static>>> {
        let inner = self.read_inner()?;
        if let Some(value) = inner.overlay.get(key) {
            return Ok(value
                .clone()
                .map(|value| ValueRef::from_shared(value, options.read_strategy)));
        }
        for table in manifest_tables(&inner.manifest).iter().rev() {
            if !table.may_contain_user_key(key) {
                continue;
            }
            let table_path = self.root.join(Manifest::table_name(table.number));
            if !table_path.exists() {
                continue;
            }
            if let Some(value) = table::get_table_entry(
                &table_path,
                key,
                read_checksums(&self.options, &options),
                read_cache(&options, &self.block_cache),
            )? {
                return Ok(Some(ValueRef::from_shared(value, options.read_strategy)));
            }
        }
        Ok(None)
    }

    /// Reads many keys using explicit [`ReadOptions`], preserving input order.
    ///
    /// This batches keys by table file so native table indexes and data blocks
    /// are reused across a render chunk read instead of reopened for each key.
    ///
    /// # Errors
    ///
    /// Returns an error if metadata, table blocks, compression, checksum
    /// verification, or filesystem reads fail.
    pub fn get_many_owned(
        &self,
        keys: impl IntoIterator<Item = Bytes>,
        options: ReadOptions,
    ) -> Result<Vec<Option<Bytes>>> {
        let started = Instant::now();
        let keys = keys.into_iter().collect::<Vec<_>>();
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let inner = self.read_inner()?;
        let mut results = vec![None; keys.len()];
        let mut unresolved = Vec::with_capacity(keys.len());
        for (index, key) in keys.iter().enumerate() {
            if let Some(value) = inner.overlay.get(key.as_ref()) {
                results[index].clone_from(value);
            } else {
                unresolved.push(index);
            }
        }
        unresolved.sort_unstable_by(|left, right| {
            keys[*left]
                .as_ref()
                .cmp(keys[*right].as_ref())
                .then_with(|| left.cmp(right))
        });

        let mut table_probes = 0usize;
        let mut table_hits = 0usize;
        for table in manifest_tables(&inner.manifest).iter().rev() {
            if unresolved.is_empty() {
                break;
            }
            let table_indices = unresolved
                .iter()
                .copied()
                .filter(|index| table.may_contain_user_key(keys[*index].as_ref()))
                .collect::<Vec<_>>();
            if table_indices.is_empty() {
                continue;
            }
            let table_path = self.root.join(Manifest::table_name(table.number));
            if !table_path.exists() {
                continue;
            }
            let table_keys = table_indices
                .iter()
                .map(|index| keys[*index].clone())
                .collect::<Vec<_>>();
            table_probes = table_probes.saturating_add(1);
            let table_results = table::get_table_entries(
                &table_path,
                &table_keys,
                read_checksums(&self.options, &options),
                read_cache(&options, &self.block_cache),
            )?;
            for (input_index, value) in table_indices.into_iter().zip(table_results) {
                if let Some(value) = value {
                    results[input_index] = Some(value);
                    table_hits = table_hits.saturating_add(1);
                }
            }
            unresolved.retain(|index| results[*index].is_none());
        }
        log::debug!(
            "batch exact get complete (keys={}, hits={}, table_probes={}, elapsed_ms={})",
            keys.len(),
            results.iter().filter(|value| value.is_some()).count(),
            table_probes,
            started.elapsed().as_millis()
        );
        log::trace!(
            "batch exact get detail (keys={}, table_hits={}, unresolved={})",
            keys.len(),
            table_hits,
            unresolved.len()
        );
        Ok(results)
    }

    #[cfg(feature = "async")]
    /// Reads many keys on a blocking Tokio task, preserving input order.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Db::get_many_owned`] plus a join error.
    pub async fn get_many_owned_async(
        self: Arc<Self>,
        keys: Vec<Bytes>,
        options: ReadOptions,
    ) -> Result<Vec<Option<Bytes>>> {
        tokio::task::spawn_blocking(move || self.get_many_owned(keys, options))
            .await
            .map_err(|error| LevelDbError::join(error.to_string()))?
    }

    /// Appends a single put operation to the WAL overlay.
    ///
    /// # Errors
    ///
    /// Returns an error when the database is read-only, the key is empty, the
    /// batch cannot be encoded, the log cannot be written, or a flush fails.
    pub fn put(
        &self,
        key: impl Into<Bytes>,
        value: impl Into<Bytes>,
        options: WriteOptions,
    ) -> Result<()> {
        let mut batch = WriteBatch::new();
        batch.put(key.into(), value.into());
        self.write(batch, options)
    }

    /// Appends a single delete operation to the WAL overlay.
    ///
    /// # Errors
    ///
    /// Returns an error when the database is read-only, the key is empty, the
    /// batch cannot be encoded, the log cannot be written, or a flush fails.
    pub fn delete(&self, key: impl Into<Bytes>, options: WriteOptions) -> Result<()> {
        let mut batch = WriteBatch::new();
        batch.delete(key.into());
        self.write(batch, options)
    }

    /// Appends a batch to the native `LevelDB` WAL overlay.
    ///
    /// The method flushes to a native table when the write buffer reaches
    /// [`OpenOptions::write_buffer_size`]. Set that option to `0` to disable
    /// automatic flushes and keep writes WAL-backed until an explicit flush.
    ///
    /// # Errors
    ///
    /// Returns an error when the database is read-only, a key is empty, sequence
    /// numbers overflow, the batch cannot be encoded, the log cannot be written,
    /// or a flush fails.
    pub fn write(&self, mut batch: WriteBatch, options: WriteOptions) -> Result<()> {
        if self.options.read_only {
            return Err(LevelDbError::ReadOnly);
        }
        if batch.is_empty() {
            return Ok(());
        }
        validate_batch(&batch)?;

        let mut inner = self.write_inner()?;
        let first_sequence = inner.last_sequence.checked_add(1).ok_or_else(|| {
            LevelDbError::invalid_argument("write sequence number overflowed".to_string())
        })?;
        let batch_len = u64::try_from(batch.len()).map_err(|_| {
            LevelDbError::invalid_argument("write batch length overflowed".to_string())
        })?;
        let last_sequence = inner.last_sequence.checked_add(batch_len).ok_or_else(|| {
            LevelDbError::invalid_argument("write sequence number overflowed".to_string())
        })?;
        batch.set_sequence(first_sequence);
        append_batch_to_log(&self.root, inner.manifest.log_number, &batch, options)?;
        let approximate_bytes = inner.approximate_bytes;
        inner.approximate_bytes = apply_batch(&mut inner.overlay, &batch, approximate_bytes);
        inner.last_sequence = last_sequence;

        if self.options.write_buffer_size != 0
            && inner.approximate_bytes >= self.options.write_buffer_size
        {
            self.flush_locked(&mut inner)?;
        }
        Ok(())
    }

    /// Appends a batch through the native `LevelDB` write path.
    ///
    /// This is the explicit v0.2 name for [`Db::write`]. It writes a standard
    /// `LevelDB` WAL batch and any automatic flush writes native `.ldb` tables
    /// plus a native manifest version edit. Automatic flushes are skipped when
    /// [`OpenOptions::write_buffer_size`] is `0`.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Db::write`].
    pub fn write_batch_native(&self, batch: WriteBatch, options: WriteOptions) -> Result<()> {
        self.write(batch, options)
    }

    /// Visits visible keys without cloning values.
    ///
    /// # Errors
    ///
    /// Returns an error when scan cancellation is requested, an underlying table
    /// cannot be read, checksum or compression validation fails, thread options
    /// are invalid, or the visitor returns an error.
    pub fn for_each_key<F>(&self, options: ReadOptions, mut visitor: F) -> Result<ScanOutcome>
    where
        F: FnMut(&[u8]) -> Result<VisitorControl> + Send,
    {
        let started = Instant::now();
        let inner = self.read_inner()?;
        log::debug!(
            "starting key scan (tables={}, scan_mode={:?}, threading={:?})",
            inner.manifest.table_numbers.len(),
            options.scan_mode,
            options.threading
        );
        let result = self.for_each_key_locked(&inner, &options, &mut visitor);
        log_scan_result("key scan", started, &result);
        result
    }

    /// Visits visible key/value entries.
    ///
    /// # Errors
    ///
    /// Returns an error when scan cancellation is requested, an underlying table
    /// cannot be read, checksum or compression validation fails, thread options
    /// are invalid, or the visitor returns an error.
    pub fn for_each_entry<F>(&self, options: ReadOptions, mut visitor: F) -> Result<ScanOutcome>
    where
        F: FnMut(&[u8], &Bytes) -> Result<VisitorControl> + Send,
    {
        let started = Instant::now();
        let inner = self.read_inner()?;
        log::debug!(
            "starting entry scan (tables={}, scan_mode={:?}, threading={:?})",
            inner.manifest.table_numbers.len(),
            options.scan_mode,
            options.threading
        );
        let result = self.for_each_entry_locked(&inner, &options, &mut visitor);
        log_scan_result("entry scan", started, &result);
        result
    }

    /// Visits visible entries as borrowed-first [`EntryRef`] values.
    ///
    /// # Errors
    ///
    /// Returns an error when scan cancellation is requested, an underlying table
    /// cannot be read, checksum or compression validation fails, thread options
    /// are invalid, or the visitor returns an error.
    pub fn for_each_entry_ref<F>(&self, options: ReadOptions, mut visitor: F) -> Result<ScanOutcome>
    where
        F: FnMut(EntryRef<'_>) -> Result<VisitorControl> + Send,
    {
        if options.read_strategy == ReadStrategy::Borrowed
            && options.scan_mode == ScanMode::Sequential
        {
            let started = Instant::now();
            let inner = self.read_inner()?;
            let result = self.for_each_entry_ref_locked(&inner, &options, &mut visitor);
            log_scan_result("entry ref scan", started, &result);
            return result;
        }
        let strategy = options.read_strategy;
        self.for_each_entry(options, |key, value| {
            visitor(EntryRef {
                key: KeyRef::new(key),
                value: ValueRef::from_shared(value.clone(), strategy),
            })
        })
    }

    /// Visits visible key/value entries whose key starts with `prefix`.
    ///
    /// # Errors
    ///
    /// Returns an error when scan cancellation is requested, an underlying table
    /// cannot be read, checksum or compression validation fails, thread options
    /// are invalid, or the visitor returns an error.
    pub fn for_each_prefix<F>(
        &self,
        prefix: &[u8],
        options: ReadOptions,
        mut visitor: F,
    ) -> Result<ScanOutcome>
    where
        F: FnMut(&[u8], &Bytes) -> Result<VisitorControl> + Send,
    {
        let started = Instant::now();
        let inner = self.read_inner()?;
        log::debug!(
            "starting prefix entry scan (prefix_len={}, tables={}, scan_mode={:?}, threading={:?})",
            prefix.len(),
            inner.manifest.table_numbers.len(),
            options.scan_mode,
            options.threading
        );
        let result = self.for_each_prefix_locked(&inner, prefix, &options, &mut visitor);
        log_scan_result("prefix entry scan", started, &result);
        result
    }

    /// Visits visible prefix entries as borrowed-first [`EntryRef`] values.
    ///
    /// # Errors
    ///
    /// Returns an error when scan cancellation is requested, an underlying table
    /// cannot be read, checksum or compression validation fails, thread options
    /// are invalid, or the visitor returns an error.
    pub fn for_each_prefix_ref<F>(
        &self,
        prefix: &[u8],
        options: ReadOptions,
        mut visitor: F,
    ) -> Result<ScanOutcome>
    where
        F: FnMut(EntryRef<'_>) -> Result<VisitorControl> + Send,
    {
        if options.read_strategy == ReadStrategy::Borrowed
            && options.scan_mode == ScanMode::Sequential
        {
            let started = Instant::now();
            let inner = self.read_inner()?;
            let result = self.for_each_prefix_ref_locked(&inner, prefix, &options, &mut visitor);
            log_scan_result("prefix entry ref scan", started, &result);
            return result;
        }
        let strategy = options.read_strategy;
        self.for_each_prefix(prefix, options, |key, value| {
            visitor(EntryRef {
                key: KeyRef::new(key),
                value: ValueRef::from_shared(value.clone(), strategy),
            })
        })
    }

    /// Visits visible keys whose key starts with `prefix` without materializing
    /// table values for the visitor.
    ///
    /// # Errors
    ///
    /// Returns an error when scan cancellation is requested, an underlying table
    /// cannot be read, checksum or compression validation fails, thread options
    /// are invalid, or the visitor returns an error.
    pub fn for_each_prefix_key<F>(
        &self,
        prefix: &[u8],
        options: ReadOptions,
        mut visitor: F,
    ) -> Result<ScanOutcome>
    where
        F: FnMut(&[u8]) -> Result<VisitorControl> + Send,
    {
        let started = Instant::now();
        let inner = self.read_inner()?;
        let result = self.for_each_prefix_key_locked(&inner, prefix, &options, &mut visitor);
        log_scan_result("prefix key scan", started, &result);
        result
    }

    /// Collects visible keys without materializing table values.
    ///
    /// # Errors
    ///
    /// Returns an error if the key scan fails or is cancelled.
    pub fn collect_keys_owned(&self, options: ReadOptions) -> Result<Vec<Bytes>> {
        let mut keys = Vec::new();
        self.for_each_key(options, |key| {
            keys.push(Bytes::copy_from_slice(key));
            Ok(VisitorControl::Continue)
        })?;
        Ok(keys)
    }

    /// Collects visible keys whose key starts with `prefix` without
    /// materializing table values.
    ///
    /// # Errors
    ///
    /// Returns an error if the key scan fails or is cancelled.
    pub fn collect_prefix_keys_owned(
        &self,
        prefix: &[u8],
        options: ReadOptions,
    ) -> Result<Vec<Bytes>> {
        let mut keys = Vec::new();
        self.for_each_prefix_key(prefix, options, |key| {
            keys.push(Bytes::copy_from_slice(key));
            Ok(VisitorControl::Continue)
        })?;
        Ok(keys)
    }

    /// Collects visible key/value entries whose key starts with `prefix`.
    ///
    /// # Errors
    ///
    /// Returns an error if the scan fails or is cancelled.
    pub fn collect_prefix_owned(
        &self,
        prefix: &[u8],
        options: ReadOptions,
    ) -> Result<Vec<(Bytes, Bytes)>> {
        let mut entries = Vec::new();
        self.for_each_prefix(prefix, options, |key, value| {
            entries.push((Bytes::copy_from_slice(key), value.clone()));
            Ok(VisitorControl::Continue)
        })?;
        Ok(entries)
    }

    #[cfg(feature = "async")]
    /// Collects visible keys on a blocking Tokio task.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Db::collect_keys_owned`] plus a join error.
    pub async fn collect_keys_owned_async(
        self: Arc<Self>,
        options: ReadOptions,
    ) -> Result<Vec<Bytes>> {
        tokio::task::spawn_blocking(move || self.collect_keys_owned(options))
            .await
            .map_err(|error| LevelDbError::join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Collects visible prefix keys on a blocking Tokio task.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Db::collect_prefix_keys_owned`] plus a join
    /// error.
    pub async fn collect_prefix_keys_owned_async(
        self: Arc<Self>,
        prefix: Bytes,
        options: ReadOptions,
    ) -> Result<Vec<Bytes>> {
        tokio::task::spawn_blocking(move || self.collect_prefix_keys_owned(&prefix, options))
            .await
            .map_err(|error| LevelDbError::join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Collects visible prefix entries on a blocking Tokio task.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Db::collect_prefix_owned`] plus a join
    /// error.
    pub async fn collect_prefix_owned_async(
        self: Arc<Self>,
        prefix: Bytes,
        options: ReadOptions,
    ) -> Result<Vec<(Bytes, Bytes)>> {
        tokio::task::spawn_blocking(move || self.collect_prefix_owned(&prefix, options))
            .await
            .map_err(|error| LevelDbError::join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Collects visible keys with `prefix` on a blocking Tokio task.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Db::for_each_prefix_key`] plus a join error
    /// if the blocking task fails to complete.
    pub async fn prefix_keys_async(
        self: Arc<Self>,
        prefix: Bytes,
        options: ReadOptions,
    ) -> Result<Vec<Bytes>> {
        self.collect_prefix_keys_owned_async(prefix, options).await
    }

    /// Visits keys with per-worker local state for table-parallel reductions.
    ///
    /// # Errors
    ///
    /// Returns an error when scan cancellation is requested, an underlying table
    /// cannot be read, checksum or compression validation fails, thread options
    /// are invalid, or the visitor returns an error.
    pub fn scan_keys_partitioned<T, I, F>(
        &self,
        options: ReadOptions,
        init: I,
        visitor: F,
    ) -> Result<(ScanOutcome, Vec<T>)>
    where
        T: Send,
        I: Fn() -> T + Send + Sync,
        F: Fn(&mut T, &[u8]) -> Result<VisitorControl> + Send + Sync,
    {
        let inner = self.read_inner()?;
        self.scan_keys_partitioned_locked(&inner, &options, &init, &visitor)
    }

    /// Visits entries with per-worker local state for table-parallel reductions.
    ///
    /// # Errors
    ///
    /// Returns an error when scan cancellation is requested, an underlying table
    /// cannot be read, checksum or compression validation fails, thread options
    /// are invalid, or the visitor returns an error.
    pub fn scan_entries_partitioned<T, I, F>(
        &self,
        options: ReadOptions,
        init: I,
        visitor: F,
    ) -> Result<(ScanOutcome, Vec<T>)>
    where
        T: Send,
        I: Fn() -> T + Send + Sync,
        F: Fn(&mut T, &[u8], &Bytes) -> Result<VisitorControl> + Send + Sync,
    {
        let inner = self.read_inner()?;
        self.scan_entries_partitioned_locked(&inner, &options, &init, &visitor)
    }

    /// Materializes all visible entries into an iterator.
    ///
    /// Prefer visitor scans for large worlds.
    ///
    /// # Errors
    ///
    /// Returns an error when any underlying table cannot be read or decoded.
    pub fn iterator(&self, options: ReadOptions) -> Result<RawIterator> {
        let entries = self.collect_visible_entries(&options)?;
        Ok(RawIterator::new(&entries, &[]))
    }

    /// Materializes visible entries with `prefix` into an iterator.
    ///
    /// Prefer [`Db::for_each_prefix`] for large worlds.
    ///
    /// # Errors
    ///
    /// Returns an error when any underlying table cannot be read or decoded.
    pub fn prefix_iterator(&self, prefix: &[u8], options: ReadOptions) -> Result<PrefixIterator> {
        let entries = self.collect_visible_prefix(prefix, &options)?;
        Ok(PrefixIterator {
            inner: RawIterator::new(&entries, prefix),
        })
    }

    /// Materializes the current visible view.
    ///
    /// Prefer visitor scans for large worlds.
    ///
    /// # Errors
    ///
    /// Returns an error when any underlying table cannot be read or decoded.
    pub fn snapshot(&self) -> Result<Snapshot> {
        let inner = self.read_inner()?;
        Ok(Snapshot {
            sequence: inner.last_sequence,
            values: Arc::new(self.collect_visible_entries_locked(&inner, &ReadOptions::default())?),
        })
    }

    /// Flushes the WAL overlay into a native `LevelDB` table.
    ///
    /// # Errors
    ///
    /// Returns [`LevelDbError::ReadOnly`] for read-only handles or an I/O,
    /// compression, or decoding error when the flush fails.
    pub fn flush(&self) -> Result<()> {
        if self.options.read_only {
            return Err(LevelDbError::ReadOnly);
        }
        let mut inner = self.write_inner()?;
        self.flush_locked(&mut inner)
    }

    /// Flushes the current memtable/overlay into a native `LevelDB` table.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Db::flush`].
    pub fn flush_memtable(&self) -> Result<()> {
        self.flush()
    }

    /// Flushes a range into a new native `LevelDB` table.
    ///
    /// # Errors
    ///
    /// Returns [`LevelDbError::ReadOnly`] for read-only handles or an I/O,
    /// compression, or decoding error when the range cannot be materialized.
    pub fn compact_range(&self, start: Option<&[u8]>, end: Option<&[u8]>) -> Result<()> {
        if self.options.read_only {
            return Err(LevelDbError::ReadOnly);
        }
        let mut inner = self.write_inner()?;
        // The current storage layout has no levels. A range-only rewrite would
        // leave overlapping older tables alive and would not reclaim tombstones.
        // Rewriting the complete visible state is therefore the only correct
        // compaction boundary. Keep the range arguments for API compatibility.
        let _requested_range = match (start, end) {
            (Some(start), Some(end)) => (Included(start.to_vec()), Included(end.to_vec())),
            (Some(start), None) => (Included(start.to_vec()), Unbounded),
            (None, Some(end)) => (Unbounded, Included(end.to_vec())),
            (None, None) => (Unbounded, Unbounded),
        };
        self.rewrite_visible_state_locked(&mut inner, true)
    }

    /// Flushes a range into a new native `LevelDB` table.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Db::compact_range`].
    pub fn compact_range_native(&self, start: Option<&[u8]>, end: Option<&[u8]>) -> Result<()> {
        self.compact_range(start, end)
    }

    /// Rebuilds a native manifest/table from readable tables and logs.
    ///
    /// # Errors
    ///
    /// Returns [`LevelDbError::ReadOnly`] when `options.read_only` is set,
    /// [`LevelDbError::NotFound`] when the directory is missing and creation is
    /// disabled, or an I/O/compression error while writing repaired files.
    pub fn repair(path: impl AsRef<Path>, options: OpenOptions) -> Result<RepairReport> {
        if options.read_only {
            return Err(LevelDbError::ReadOnly);
        }
        let root = path.as_ref();
        log::debug!("repairing database at {}", root.display());
        if !root.exists() {
            if options.create_if_missing {
                fs::create_dir_all(root)
                    .map_err(|error| LevelDbError::io_at("create repair directory", root, error))?;
            } else {
                return Err(LevelDbError::not_found(root.to_path_buf()));
            }
        }

        let mut report = RepairReport::default();
        let mut values = BTreeMap::new();
        let mut table_numbers = Vec::new();

        for entry in fs::read_dir(root)
            .map_err(|error| LevelDbError::io_at("read repair directory", root, error))?
        {
            let entry = entry
                .map_err(|error| LevelDbError::io_at("read repair directory entry", root, error))?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("ldb") {
                match table::read_table(&path, false) {
                    Ok(table_values) => {
                        values.extend(table_values);
                        if let Some(number) = parse_file_number(&path) {
                            table_numbers.push(number);
                        }
                        report.recovered_tables += 1;
                    }
                    Err(error) => {
                        log::warn!(
                            "dropping unreadable table during repair: {} ({})",
                            path.display(),
                            error
                        );
                        report.dropped_files += 1;
                    }
                }
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("log") {
                match File::open(&path) {
                    Ok(mut file) => {
                        let mut approximate_bytes = approximate_entries_size(&values);
                        for record in wal::read_records(&mut file, false)? {
                            if let Ok(batch) = WriteBatch::decode(&record) {
                                approximate_bytes =
                                    apply_batch_to_values(&mut values, &batch, approximate_bytes);
                                report.recovered_log_records += 1;
                            }
                        }
                    }
                    Err(error) => {
                        log::warn!(
                            "dropping unreadable WAL during repair: {} ({})",
                            path.display(),
                            error
                        );
                        report.dropped_files += 1;
                    }
                }
            }
        }

        write_recovered_native_state(root, &values, table_numbers, options.compression_policy)?;
        log::debug!(
            "repaired database at {} (tables={}, log_records={}, dropped_files={})",
            root.display(),
            report.recovered_tables,
            report.recovered_log_records,
            report.dropped_files
        );
        Ok(report)
    }

    /// Rebuilds a native manifest/table from readable tables and logs.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Db::repair`].
    pub fn recover_native(path: impl AsRef<Path>, options: OpenOptions) -> Result<RepairReport> {
        Self::repair(path, options)
    }

    /// Returns metadata and overlay-only stats without table scans.
    ///
    /// # Errors
    ///
    /// Returns [`LevelDbError::LockPoisoned`] if the database lock is poisoned.
    pub fn stats_fast(&self) -> Result<DbStats> {
        let inner = self.read_inner()?;
        Ok(DbStats {
            entries: inner
                .overlay
                .values()
                .filter(|value| value.is_some())
                .count(),
            tables: inner.manifest.table_numbers.len(),
            log_number: inner.manifest.log_number,
            approximate_bytes: inner.approximate_bytes,
        })
    }

    /// Materializes visible entries to compute full stats.
    ///
    /// # Errors
    ///
    /// Returns an error when any underlying table cannot be read or decoded.
    pub fn stats_full(&self) -> Result<DbStats> {
        let inner = self.read_inner()?;
        let entries = self.collect_visible_entries_locked(&inner, &ReadOptions::default())?;
        Ok(DbStats {
            entries: entries.len(),
            tables: inner.manifest.table_numbers.len(),
            log_number: inner.manifest.log_number,
            approximate_bytes: approximate_entries_size(&entries),
        })
    }

    /// Alias for [`Db::stats_full`].
    ///
    /// # Errors
    ///
    /// Returns an error when any underlying table cannot be read or decoded.
    pub fn stats(&self) -> Result<DbStats> {
        self.stats_full()
    }

    fn flush_locked(&self, inner: &mut DbInner) -> Result<()> {
        if inner.overlay.is_empty() {
            return Ok(());
        }
        self.rewrite_visible_state_locked(inner, false)
    }

    fn rewrite_visible_state_locked(
        &self,
        inner: &mut DbInner,
        force_compaction: bool,
    ) -> Result<()> {
        if !force_compaction && inner.overlay.is_empty() {
            return Ok(());
        }

        let values = self.collect_visible_entries_locked(inner, &ReadOptions::default())?;
        let old_table_numbers = inner.manifest.table_numbers.clone();
        let old_log_number = inner.manifest.log_number;
        let mut next_manifest = inner.manifest.clone();

        let table_number = allocate_file_number(&mut next_manifest);
        let table_path = self.root.join(Manifest::table_name(table_number));
        log::debug!(
            "rewriting visible state into native table {} (entries={}, force_compaction={})",
            table_path.display(),
            values.len(),
            force_compaction
        );

        let next_table = if values.is_empty() {
            None
        } else {
            let written = table::write_native_table(
                &table_path,
                &values,
                inner.last_sequence,
                self.options.compression_policy,
            )?;
            Some(crate::manifest::TableFileMeta::native(
                table_number,
                written.file_size,
                written.smallest_internal_key,
                written.largest_internal_key,
            ))
        };

        let new_log_number = allocate_file_number(&mut next_manifest);
        let new_log = self.root.join(Manifest::log_name(new_log_number));
        let new_log_file = File::create(&new_log)
            .map_err(|error| LevelDbError::io_at("create WAL", &new_log, error))?;
        new_log_file
            .sync_all()
            .map_err(|error| LevelDbError::io_at("sync WAL", &new_log, error))?;

        next_manifest.log_number = new_log_number;
        next_manifest.table_numbers = next_table
            .as_ref()
            .map_or_else(Vec::new, |table| vec![table.number]);
        next_manifest.table_files = next_table.into_iter().collect();

        // Commit metadata before deleting any file referenced by the previous
        // manifest. A crash before this point leaves the old state readable; a
        // crash after this point leaves only harmless obsolete files.
        next_manifest.store(&self.root)?;
        inner.manifest = next_manifest;
        inner.overlay.clear();
        inner.approximate_bytes = 0;

        let obsolete_paths = old_table_numbers
            .iter()
            .map(|number| self.root.join(Manifest::table_name(*number)))
            .collect::<Vec<_>>();
        self.block_cache.invalidate_paths(&obsolete_paths);
        for old_path in obsolete_paths {
            if old_path != table_path && old_path.exists() {
                if let Err(error) = fs::remove_file(&old_path) {
                    log::warn!(
                        "failed to remove obsolete table {} after manifest commit: {}",
                        old_path.display(),
                        error
                    );
                }
            }
        }

        let old_log = self.root.join(Manifest::log_name(old_log_number));
        if old_log != new_log && old_log.exists() {
            if let Err(error) = fs::remove_file(&old_log) {
                log::warn!(
                    "failed to remove obsolete WAL {} after manifest commit: {}",
                    old_log.display(),
                    error
                );
            }
        }
        log::debug!(
            "visible state rewrite committed (entries={}, tables={}, wal={})",
            values.len(),
            inner.manifest.table_numbers.len(),
            new_log.display()
        );
        Ok(())
    }

    fn for_each_entry_locked<F>(
        &self,
        inner: &DbInner,
        options: &ReadOptions,
        visitor: &mut F,
    ) -> Result<ScanOutcome>
    where
        F: FnMut(&[u8], &Bytes) -> Result<VisitorControl> + Send,
    {
        let hidden_keys = &inner.overlay;
        let verify_checksums = read_checksums(&self.options, options);
        let mut outcome = ScanOutcome::empty();
        outcome.worker_threads = 1;
        match options.scan_mode {
            ScanMode::Sequential => {
                let table_count = inner.manifest.table_numbers.len();
                for (table_index, table_number) in inner.manifest.table_numbers.iter().enumerate() {
                    check_scan_cancelled(options)?;
                    let table_path = self.root.join(Manifest::table_name(*table_number));
                    if !table_path.exists() {
                        continue;
                    }
                    let table_outcome = table::for_each_table_entry(
                        &table_path,
                        verify_checksums,
                        read_cache(options, &self.block_cache),
                        |key, value| {
                            if !hidden_keys.contains_key(key) {
                                return visitor(key, value);
                            }
                            Ok(VisitorControl::Continue)
                        },
                    )?;
                    outcome.merge(table_outcome);
                    emit_scan_progress(options, outcome);
                    log::trace!(
                        "entry scan progress (table_index={}, tables={}, visited={}, tables_scanned={}, bytes_read={}, stopped={})",
                        table_index.saturating_add(1),
                        table_count,
                        outcome.visited,
                        outcome.tables_scanned,
                        outcome.bytes_read,
                        outcome.stopped
                    );
                    if outcome.stopped {
                        return Ok(outcome);
                    }
                }
            }
            ScanMode::ParallelTables => {
                let table_paths = table_paths(&self.root, &inner.manifest);
                let table_outcome = for_each_table_paths_parallel(
                    table_paths,
                    None,
                    verify_checksums,
                    read_cache(options, &self.block_cache),
                    options
                        .threading
                        .resolve_checked(inner.manifest.table_numbers.len())?,
                    hidden_keys,
                    visitor,
                    options,
                )?;
                outcome.merge(table_outcome);
                if outcome.stopped {
                    return Ok(outcome);
                }
            }
        }
        for (key, value) in &inner.overlay {
            check_scan_cancelled(options)?;
            if let Some(value) = value {
                outcome.record(value.len());
                if visitor(key, value)? == VisitorControl::Stop {
                    outcome.stopped = true;
                    return Ok(outcome);
                }
            }
        }
        Ok(outcome)
    }

    fn for_each_prefix_locked<F>(
        &self,
        inner: &DbInner,
        prefix: &[u8],
        options: &ReadOptions,
        visitor: &mut F,
    ) -> Result<ScanOutcome>
    where
        F: FnMut(&[u8], &Bytes) -> Result<VisitorControl> + Send,
    {
        let hidden_keys = &inner.overlay;
        let verify_checksums = read_checksums(&self.options, options);
        let mut outcome = ScanOutcome::empty();
        outcome.worker_threads = 1;
        match options.scan_mode {
            ScanMode::Sequential => {
                for table_number in &inner.manifest.table_numbers {
                    check_scan_cancelled(options)?;
                    let table_path = self.root.join(Manifest::table_name(*table_number));
                    if !table_path.exists() {
                        continue;
                    }
                    let table_outcome = table::for_each_table_prefix(
                        &table_path,
                        prefix,
                        verify_checksums,
                        read_cache(options, &self.block_cache),
                        |key, value| {
                            if !hidden_keys.contains_key(key) {
                                return visitor(key, value);
                            }
                            Ok(VisitorControl::Continue)
                        },
                    )?;
                    outcome.merge(table_outcome);
                    emit_scan_progress(options, outcome);
                    if outcome.stopped {
                        return Ok(outcome);
                    }
                }
            }
            ScanMode::ParallelTables => {
                let table_paths = table_paths(&self.root, &inner.manifest);
                let table_outcome = for_each_table_paths_parallel(
                    table_paths,
                    Some(prefix.to_vec()),
                    verify_checksums,
                    read_cache(options, &self.block_cache),
                    options
                        .threading
                        .resolve_checked(inner.manifest.table_numbers.len())?,
                    hidden_keys,
                    visitor,
                    options,
                )?;
                outcome.merge(table_outcome);
                if outcome.stopped {
                    return Ok(outcome);
                }
            }
        }
        for (key, value) in inner
            .overlay
            .range(prefix.to_vec()..)
            .take_while(|(key, _)| key.starts_with(prefix))
        {
            check_scan_cancelled(options)?;
            if let Some(value) = value {
                outcome.record(value.len());
                if visitor(key, value)? == VisitorControl::Stop {
                    outcome.stopped = true;
                    return Ok(outcome);
                }
            }
        }
        Ok(outcome)
    }

    fn for_each_entry_ref_locked<F>(
        &self,
        inner: &DbInner,
        options: &ReadOptions,
        visitor: &mut F,
    ) -> Result<ScanOutcome>
    where
        F: FnMut(EntryRef<'_>) -> Result<VisitorControl> + Send,
    {
        let hidden_keys = &inner.overlay;
        let verify_checksums = read_checksums(&self.options, options);
        let mut outcome = ScanOutcome::empty();
        outcome.worker_threads = 1;
        for table_number in &inner.manifest.table_numbers {
            check_scan_cancelled(options)?;
            let table_path = self.root.join(Manifest::table_name(*table_number));
            if !table_path.exists() {
                continue;
            }
            let table_outcome =
                table::for_each_table_entry_ref(&table_path, verify_checksums, |key, value| {
                    if !hidden_keys.contains_key(key) {
                        return visitor(EntryRef {
                            key: KeyRef::new(key),
                            value,
                        });
                    }
                    Ok(VisitorControl::Continue)
                })?;
            outcome.merge(table_outcome);
            emit_scan_progress(options, outcome);
            if outcome.stopped {
                return Ok(outcome);
            }
        }
        for (key, value) in &inner.overlay {
            check_scan_cancelled(options)?;
            if let Some(value) = value {
                outcome.record(value.len());
                if visitor(EntryRef {
                    key: KeyRef::new(key),
                    value: ValueRef::Shared(value.clone()),
                })? == VisitorControl::Stop
                {
                    outcome.stopped = true;
                    return Ok(outcome);
                }
            }
        }
        Ok(outcome)
    }

    fn for_each_prefix_ref_locked<F>(
        &self,
        inner: &DbInner,
        prefix: &[u8],
        options: &ReadOptions,
        visitor: &mut F,
    ) -> Result<ScanOutcome>
    where
        F: FnMut(EntryRef<'_>) -> Result<VisitorControl> + Send,
    {
        let hidden_keys = &inner.overlay;
        let verify_checksums = read_checksums(&self.options, options);
        let mut outcome = ScanOutcome::empty();
        outcome.worker_threads = 1;
        for table_number in &inner.manifest.table_numbers {
            check_scan_cancelled(options)?;
            let table_path = self.root.join(Manifest::table_name(*table_number));
            if !table_path.exists() {
                continue;
            }
            let table_outcome = table::for_each_table_prefix_ref(
                &table_path,
                prefix,
                verify_checksums,
                |key, value| {
                    if !hidden_keys.contains_key(key) {
                        return visitor(EntryRef {
                            key: KeyRef::new(key),
                            value,
                        });
                    }
                    Ok(VisitorControl::Continue)
                },
            )?;
            outcome.merge(table_outcome);
            emit_scan_progress(options, outcome);
            if outcome.stopped {
                return Ok(outcome);
            }
        }
        for (key, value) in inner
            .overlay
            .range(prefix.to_vec()..)
            .take_while(|(key, _)| key.starts_with(prefix))
        {
            check_scan_cancelled(options)?;
            if let Some(value) = value {
                outcome.record(value.len());
                if visitor(EntryRef {
                    key: KeyRef::new(key),
                    value: ValueRef::Shared(value.clone()),
                })? == VisitorControl::Stop
                {
                    outcome.stopped = true;
                    return Ok(outcome);
                }
            }
        }
        Ok(outcome)
    }

    fn for_each_prefix_key_locked<F>(
        &self,
        inner: &DbInner,
        prefix: &[u8],
        options: &ReadOptions,
        visitor: &mut F,
    ) -> Result<ScanOutcome>
    where
        F: FnMut(&[u8]) -> Result<VisitorControl> + Send,
    {
        let hidden_keys = &inner.overlay;
        let verify_checksums = read_checksums(&self.options, options);
        let mut outcome = ScanOutcome::empty();
        outcome.worker_threads = 1;
        log::debug!(
            "starting prefix key scan (prefix_len={}, tables={}, scan_mode={:?})",
            prefix.len(),
            inner.manifest.table_numbers.len(),
            options.scan_mode
        );
        match options.scan_mode {
            ScanMode::Sequential => {
                let table_count = inner.manifest.table_numbers.len();
                for (table_index, table_number) in inner.manifest.table_numbers.iter().enumerate() {
                    check_scan_cancelled(options)?;
                    let table_path = self.root.join(Manifest::table_name(*table_number));
                    if !table_path.exists() {
                        continue;
                    }
                    let table_outcome = table::for_each_table_prefix_key(
                        &table_path,
                        prefix,
                        verify_checksums,
                        read_cache(options, &self.block_cache),
                        |key| {
                            if !hidden_keys.contains_key(key) {
                                return visitor(key);
                            }
                            Ok(VisitorControl::Continue)
                        },
                    )?;
                    outcome.merge(table_outcome);
                    emit_scan_progress(options, outcome);
                    log::trace!(
                        "prefix key scan progress (prefix_len={}, table_index={}, tables={}, visited={}, tables_scanned={}, bytes_read={}, stopped={})",
                        prefix.len(),
                        table_index.saturating_add(1),
                        table_count,
                        outcome.visited,
                        outcome.tables_scanned,
                        outcome.bytes_read,
                        outcome.stopped
                    );
                    if outcome.stopped {
                        return Ok(outcome);
                    }
                }
            }
            ScanMode::ParallelTables => {
                let table_paths = table_paths(&self.root, &inner.manifest);
                let table_outcome = for_each_table_prefix_key_paths_parallel(
                    table_paths,
                    prefix.to_vec(),
                    verify_checksums,
                    read_cache(options, &self.block_cache),
                    options
                        .threading
                        .resolve_checked(inner.manifest.table_numbers.len())?,
                    hidden_keys,
                    visitor,
                    options,
                )?;
                outcome.merge(table_outcome);
                if outcome.stopped {
                    return Ok(outcome);
                }
            }
        }
        for (key, value) in inner
            .overlay
            .range(prefix.to_vec()..)
            .take_while(|(key, _)| key.starts_with(prefix))
        {
            check_scan_cancelled(options)?;
            if let Some(value) = value {
                outcome.record(value.len());
                if visitor(key)? == VisitorControl::Stop {
                    outcome.stopped = true;
                    return Ok(outcome);
                }
            }
        }
        Ok(outcome)
    }

    fn for_each_key_locked<F>(
        &self,
        inner: &DbInner,
        options: &ReadOptions,
        visitor: &mut F,
    ) -> Result<ScanOutcome>
    where
        F: FnMut(&[u8]) -> Result<VisitorControl> + Send,
    {
        let hidden_keys = &inner.overlay;
        let verify_checksums = read_checksums(&self.options, options);
        let mut outcome = ScanOutcome::empty();
        outcome.worker_threads = 1;
        match options.scan_mode {
            ScanMode::Sequential => {
                for table_number in &inner.manifest.table_numbers {
                    check_scan_cancelled(options)?;
                    let table_path = self.root.join(Manifest::table_name(*table_number));
                    if !table_path.exists() {
                        continue;
                    }
                    let table_outcome = table::for_each_table_key(
                        &table_path,
                        verify_checksums,
                        read_cache(options, &self.block_cache),
                        |key| {
                            if !hidden_keys.contains_key(key) {
                                return visitor(key);
                            }
                            Ok(VisitorControl::Continue)
                        },
                    )?;
                    outcome.merge(table_outcome);
                    emit_scan_progress(options, outcome);
                    if outcome.stopped {
                        return Ok(outcome);
                    }
                }
            }
            ScanMode::ParallelTables => {
                let table_paths = table_paths(&self.root, &inner.manifest);
                let table_outcome = for_each_table_key_paths_parallel(
                    table_paths,
                    verify_checksums,
                    read_cache(options, &self.block_cache),
                    options
                        .threading
                        .resolve_checked(inner.manifest.table_numbers.len())?,
                    hidden_keys,
                    visitor,
                    options,
                )?;
                outcome.merge(table_outcome);
                if outcome.stopped {
                    return Ok(outcome);
                }
            }
        }
        for (key, value) in &inner.overlay {
            check_scan_cancelled(options)?;
            if let Some(value) = value {
                outcome.record(value.len());
                if visitor(key)? == VisitorControl::Stop {
                    outcome.stopped = true;
                    return Ok(outcome);
                }
            }
        }
        Ok(outcome)
    }

    fn scan_keys_partitioned_locked<T, I, F>(
        &self,
        inner: &DbInner,
        options: &ReadOptions,
        init: &I,
        visitor: &F,
    ) -> Result<(ScanOutcome, Vec<T>)>
    where
        T: Send,
        I: Fn() -> T + Send + Sync,
        F: Fn(&mut T, &[u8]) -> Result<VisitorControl> + Send + Sync,
    {
        let hidden_keys = &inner.overlay;
        let verify_checksums = read_checksums(&self.options, options);
        let mut partitions = Vec::new();
        let mut outcome = ScanOutcome::empty();
        outcome.worker_threads = 1;
        match options.scan_mode {
            ScanMode::Sequential => {
                let mut partition = init();
                for table_number in &inner.manifest.table_numbers {
                    check_scan_cancelled(options)?;
                    let table_path = self.root.join(Manifest::table_name(*table_number));
                    if !table_path.exists() {
                        continue;
                    }
                    let table_outcome = table::for_each_table_key(
                        &table_path,
                        verify_checksums,
                        read_cache(options, &self.block_cache),
                        |key| {
                            if hidden_keys.contains_key(key) {
                                return Ok(VisitorControl::Continue);
                            }
                            visitor(&mut partition, key)
                        },
                    )?;
                    outcome.merge(table_outcome);
                    if outcome.stopped {
                        partitions.push(partition);
                        return Ok((outcome, partitions));
                    }
                }
                partitions.push(partition);
            }
            ScanMode::ParallelTables => {
                let table_paths = table_paths(&self.root, &inner.manifest);
                let (table_outcome, mut table_partitions) = for_each_table_key_paths_partitioned(
                    table_paths,
                    verify_checksums,
                    read_cache(options, &self.block_cache),
                    options
                        .threading
                        .resolve_checked(inner.manifest.table_numbers.len())?,
                    hidden_keys,
                    init,
                    visitor,
                    options,
                )?;
                outcome.merge(table_outcome);
                partitions.append(&mut table_partitions);
                if outcome.stopped {
                    return Ok((outcome, partitions));
                }
            }
        }
        let mut overlay_partition = init();
        for (key, value) in &inner.overlay {
            check_scan_cancelled(options)?;
            if let Some(value) = value {
                outcome.record(value.len());
                if visitor(&mut overlay_partition, key)? == VisitorControl::Stop {
                    outcome.stopped = true;
                    break;
                }
            }
        }
        partitions.push(overlay_partition);
        Ok((outcome, partitions))
    }

    fn scan_entries_partitioned_locked<T, I, F>(
        &self,
        inner: &DbInner,
        options: &ReadOptions,
        init: &I,
        visitor: &F,
    ) -> Result<(ScanOutcome, Vec<T>)>
    where
        T: Send,
        I: Fn() -> T + Send + Sync,
        F: Fn(&mut T, &[u8], &Bytes) -> Result<VisitorControl> + Send + Sync,
    {
        let hidden_keys = &inner.overlay;
        let verify_checksums = read_checksums(&self.options, options);
        let mut partitions = Vec::new();
        let mut outcome = ScanOutcome::empty();
        outcome.worker_threads = 1;
        match options.scan_mode {
            ScanMode::Sequential => {
                let mut partition = init();
                for table_number in &inner.manifest.table_numbers {
                    check_scan_cancelled(options)?;
                    let table_path = self.root.join(Manifest::table_name(*table_number));
                    if !table_path.exists() {
                        continue;
                    }
                    let table_outcome = table::for_each_table_entry(
                        &table_path,
                        verify_checksums,
                        read_cache(options, &self.block_cache),
                        |key, value| {
                            if hidden_keys.contains_key(key) {
                                return Ok(VisitorControl::Continue);
                            }
                            visitor(&mut partition, key, value)
                        },
                    )?;
                    outcome.merge(table_outcome);
                    if outcome.stopped {
                        partitions.push(partition);
                        return Ok((outcome, partitions));
                    }
                }
                partitions.push(partition);
            }
            ScanMode::ParallelTables => {
                let table_paths = table_paths(&self.root, &inner.manifest);
                let (table_outcome, mut table_partitions) = for_each_table_paths_partitioned(
                    table_paths,
                    verify_checksums,
                    read_cache(options, &self.block_cache),
                    options
                        .threading
                        .resolve_checked(inner.manifest.table_numbers.len())?,
                    hidden_keys,
                    init,
                    visitor,
                    options,
                )?;
                outcome.merge(table_outcome);
                partitions.append(&mut table_partitions);
                if outcome.stopped {
                    return Ok((outcome, partitions));
                }
            }
        }
        let mut overlay_partition = init();
        for (key, value) in &inner.overlay {
            check_scan_cancelled(options)?;
            if let Some(value) = value {
                outcome.record(value.len());
                if visitor(&mut overlay_partition, key, value)? == VisitorControl::Stop {
                    outcome.stopped = true;
                    break;
                }
            }
        }
        partitions.push(overlay_partition);
        Ok((outcome, partitions))
    }

    fn collect_visible_entries(&self, options: &ReadOptions) -> Result<BTreeMap<Vec<u8>, Bytes>> {
        let inner = self.read_inner()?;
        self.collect_visible_entries_locked(&inner, options)
    }

    fn collect_visible_prefix(
        &self,
        prefix: &[u8],
        options: &ReadOptions,
    ) -> Result<BTreeMap<Vec<u8>, Bytes>> {
        let inner = self.read_inner()?;
        let mut entries = BTreeMap::new();
        self.for_each_prefix_locked(&inner, prefix, options, &mut |key, value| {
            entries.insert(key.to_vec(), value.clone());
            Ok(VisitorControl::Continue)
        })?;
        Ok(entries)
    }

    fn collect_visible_entries_locked(
        &self,
        inner: &DbInner,
        options: &ReadOptions,
    ) -> Result<BTreeMap<Vec<u8>, Bytes>> {
        let mut entries = BTreeMap::new();
        self.for_each_entry_locked(inner, options, &mut |key, value| {
            entries.insert(key.to_vec(), value.clone());
            Ok(VisitorControl::Continue)
        })?;
        Ok(entries)
    }

    fn read_inner(&self) -> Result<std::sync::RwLockReadGuard<'_, DbInner>> {
        self.inner
            .read()
            .map_err(|_| LevelDbError::lock_poisoned("acquiring database read lock"))
    }

    fn write_inner(&self) -> Result<std::sync::RwLockWriteGuard<'_, DbInner>> {
        self.inner
            .write()
            .map_err(|_| LevelDbError::lock_poisoned("acquiring database write lock"))
    }
}

/// Materialized iterator over raw key/value pairs.
pub struct RawIterator {
    entries: Vec<(Bytes, Bytes)>,
    index: usize,
}

impl RawIterator {
    fn new(values: &BTreeMap<Vec<u8>, Bytes>, prefix: &[u8]) -> Self {
        let entries = values
            .range(prefix.to_vec()..)
            .take_while(|(key, _)| prefix.is_empty() || key.starts_with(prefix))
            .map(|(key, value)| (Bytes::copy_from_slice(key), value.clone()))
            .collect();
        Self { entries, index: 0 }
    }
}

impl Iterator for RawIterator {
    type Item = (Bytes, Bytes);

    fn next(&mut self) -> Option<Self::Item> {
        let item = self.entries.get(self.index).cloned();
        self.index = self.index.saturating_add(usize::from(item.is_some()));
        item
    }
}

/// Materialized iterator over raw key/value pairs with one prefix.
pub struct PrefixIterator {
    inner: RawIterator,
}

impl Iterator for PrefixIterator {
    type Item = (Bytes, Bytes);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

fn load_existing_or_initialize(root: &Path, options: &OpenOptions) -> Result<LoadedState> {
    match Manifest::load(root) {
        Ok(manifest) => {
            let mut overlay = BTreeMap::new();
            let mut last_sequence = 0_u64;
            let mut approximate_bytes = 0usize;
            let log_path = root.join(Manifest::log_name(manifest.log_number));
            log::trace!(
                "loaded manifest from {} (tables={}, log_number={})",
                root.display(),
                manifest.table_numbers.len(),
                manifest.log_number
            );
            if log_path.exists() {
                log::trace!("replaying WAL {}", log_path.display());
                let mut file = File::open(&log_path)
                    .map_err(|error| LevelDbError::io_at("open WAL", &log_path, error))?;
                for record in wal::read_records(&mut file, options.paranoid_checks)? {
                    let batch = WriteBatch::decode(&record).map_err(|error| {
                        LevelDbError::corruption_at(
                            &log_path,
                            format!(
                                "failed to decode write batch from {}: {error}",
                                log_path.display()
                            ),
                        )
                    })?;
                    let batch_len = u64::try_from(batch.len()).map_err(|_| {
                        LevelDbError::corruption_at(
                            &log_path,
                            format!("write batch length overflow in {}", log_path.display()),
                        )
                    })?;
                    let batch_last_sequence =
                        batch.sequence().checked_add(batch_len).ok_or_else(|| {
                            LevelDbError::corruption_at(
                                &log_path,
                                format!("write batch sequence overflow in {}", log_path.display()),
                            )
                        })?;
                    last_sequence = last_sequence.max(batch_last_sequence);
                    approximate_bytes = apply_batch(&mut overlay, &batch, approximate_bytes);
                }
            }
            Ok((manifest, overlay, last_sequence, approximate_bytes))
        }
        Err(error)
            if error.kind() == ErrorKind::NotFound
                && options.create_if_missing
                && !options.read_only =>
        {
            log::debug!(
                "initializing new native LevelDB database at {}",
                root.display()
            );
            let manifest = Manifest::default();
            manifest.store(root)?;
            let log_path = root.join(Manifest::log_name(manifest.log_number));
            File::create(&log_path)
                .map_err(|error| LevelDbError::io_at("create WAL", &log_path, error))?;
            Ok((manifest, BTreeMap::new(), 0, 0))
        }
        Err(error) => Err(error),
    }
}

fn append_batch_to_log(
    root: &Path,
    log_number: u64,
    batch: &WriteBatch,
    options: WriteOptions,
) -> Result<()> {
    let log_path = root.join(Manifest::log_name(log_number));
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|error| LevelDbError::io_at("open WAL for append", &log_path, error))?;
    log::trace!(
        "appending write batch with {} operations to WAL {}",
        batch.len(),
        log_path.display()
    );
    wal::append_record(&mut file, &batch.encode()?)?;
    if options.sync {
        file.sync_data()
            .map_err(|error| LevelDbError::io_at("sync WAL", &log_path, error))?;
    }
    Ok(())
}

fn write_recovered_native_state(
    root: &Path,
    values: &BTreeMap<Vec<u8>, Bytes>,
    mut table_numbers: Vec<u64>,
    compression: CompressionPolicy,
) -> Result<()> {
    table_numbers.sort_unstable();
    table_numbers.dedup();
    let next_file_number = table_numbers
        .iter()
        .copied()
        .max()
        .unwrap_or(1)
        .saturating_add(2);
    let repaired_table = next_file_number.saturating_sub(1);
    let table_files = if values.is_empty() {
        Vec::new()
    } else {
        let written = table::write_native_table(
            &root.join(Manifest::table_name(repaired_table)),
            values,
            next_file_number,
            compression,
        )?;
        vec![crate::manifest::TableFileMeta::native(
            repaired_table,
            written.file_size,
            written.smallest_internal_key,
            written.largest_internal_key,
        )]
    };
    let table_numbers = if table_files.is_empty() {
        Vec::new()
    } else {
        vec![repaired_table]
    };
    let manifest = Manifest {
        next_file_number,
        log_number: next_file_number,
        table_numbers,
        table_files,
    };
    manifest.store(root)?;
    let repaired_log = root.join(Manifest::log_name(manifest.log_number));
    File::create(&repaired_log)
        .map_err(|error| LevelDbError::io_at("create repaired WAL", &repaired_log, error))?;
    Ok(())
}

fn apply_batch(
    overlay: &mut BTreeMap<Vec<u8>, Option<Bytes>>,
    batch: &WriteBatch,
    mut approximate_bytes: usize,
) -> usize {
    for op in batch.ops() {
        match op {
            WriteOp::Put { key, value } => {
                let key_size = key.len();
                let value_size = value.len();
                if let Some(old_value) = overlay.insert(key.to_vec(), Some(value.clone())) {
                    let old_value_size = old_value.as_ref().map_or(0, Bytes::len);
                    approximate_bytes =
                        approximate_bytes.saturating_sub(key_size.saturating_add(old_value_size));
                }
                approximate_bytes =
                    approximate_bytes.saturating_add(key_size.saturating_add(value_size));
            }
            WriteOp::Delete { key } => {
                if let Some(old_value) = overlay.insert(key.to_vec(), None) {
                    let old_value_size = old_value.as_ref().map_or(0, Bytes::len);
                    approximate_bytes =
                        approximate_bytes.saturating_sub(key.len().saturating_add(old_value_size));
                }
                approximate_bytes = approximate_bytes.saturating_add(key.len());
            }
        }
    }
    approximate_bytes
}

fn apply_batch_to_values(
    values: &mut BTreeMap<Vec<u8>, Bytes>,
    batch: &WriteBatch,
    mut approximate_bytes: usize,
) -> usize {
    for op in batch.ops() {
        match op {
            WriteOp::Put { key, value } => {
                let key_size = key.len();
                let value_size = value.len();
                if let Some(old_value) = values.insert(key.to_vec(), value.clone()) {
                    approximate_bytes =
                        approximate_bytes.saturating_sub(key_size.saturating_add(old_value.len()));
                }
                approximate_bytes =
                    approximate_bytes.saturating_add(key_size.saturating_add(value_size));
            }
            WriteOp::Delete { key } => {
                if let Some(old_value) = values.remove(key.as_ref()) {
                    approximate_bytes =
                        approximate_bytes.saturating_sub(key.len().saturating_add(old_value.len()));
                }
            }
        }
    }
    approximate_bytes
}

fn validate_batch(batch: &WriteBatch) -> Result<()> {
    for op in batch.ops() {
        match op {
            WriteOp::Put { key, .. } | WriteOp::Delete { key } if key.is_empty() => {
                return Err(LevelDbError::invalid_argument(
                    "empty keys are not supported".to_string(),
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

fn approximate_entries_size(values: &BTreeMap<Vec<u8>, Bytes>) -> usize {
    values
        .iter()
        .map(|(key, value)| key.len().saturating_add(value.len()))
        .sum()
}

fn read_checksums(open: &OpenOptions, read: &ReadOptions) -> bool {
    match read.checksum {
        ChecksumMode::Inherit => open.paranoid_checks,
        ChecksumMode::Verify => true,
        ChecksumMode::Skip => false,
    }
}

fn read_cache<'a>(
    read: &ReadOptions,
    cache: &'a table::NativeBlockCache,
) -> Option<&'a table::NativeBlockCache> {
    match read.cache_policy {
        CachePolicy::Use => Some(cache),
        CachePolicy::Bypass => None,
    }
}

fn check_scan_cancelled(options: &ReadOptions) -> Result<()> {
    if options
        .cancel
        .as_ref()
        .is_some_and(crate::options::ScanCancelFlag::is_cancelled)
    {
        return Err(LevelDbError::Cancelled);
    }
    Ok(())
}

fn emit_scan_progress(options: &ReadOptions, outcome: ScanOutcome) {
    if let Some(progress) = &options.progress {
        progress.emit(crate::options::ScanProgress {
            visited: outcome.visited,
            bytes_read: outcome.bytes_read,
        });
    }
}

fn log_scan_result(operation: &str, started: Instant, result: &Result<ScanOutcome>) {
    match result {
        Ok(outcome) => log::debug!(
            "{operation} complete (visited={}, bytes_read={}, tables_scanned={}, worker_threads={}, queue_wait_ms={}, cancel_checks={}, stopped={}, elapsed_ms={})",
            outcome.visited,
            outcome.bytes_read,
            outcome.tables_scanned,
            outcome.worker_threads,
            outcome.queue_wait_ms,
            outcome.cancel_checks,
            outcome.stopped,
            started.elapsed().as_millis()
        ),
        Err(error) => log::warn!(
            "{operation} failed (elapsed_ms={}, error={})",
            started.elapsed().as_millis(),
            error
        ),
    }
}

fn should_emit_scan_progress(options: &ReadOptions, visited: usize) -> bool {
    visited.is_multiple_of(options.pipeline.resolve_progress_interval())
}

fn scan_pool(worker_count: usize) -> Result<Arc<rayon::ThreadPool>> {
    static SCAN_POOLS: OnceLock<Mutex<HashMap<usize, Arc<rayon::ThreadPool>>>> = OnceLock::new();

    let worker_count = worker_count.max(1);
    let pools = SCAN_POOLS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(pool) = pools
        .lock()
        .ok()
        .and_then(|pools| pools.get(&worker_count).cloned())
    {
        return Ok(pool);
    }

    let pool = Arc::new(
        ThreadPoolBuilder::new()
            // `ThreadPool::scope` runs the receiver/coordinator on one pool
            // thread, so reserve one additional thread for worker progress.
            .num_threads(worker_count.saturating_add(1))
            .thread_name(|index| format!("bedrock-leveldb-scan-{index}"))
            .build()
            .map_err(|error| {
                LevelDbError::invalid_argument(format!("failed to build scan worker pool: {error}"))
            })?,
    );
    let mut pools = pools
        .lock()
        .map_err(|_| LevelDbError::lock_poisoned("caching scan worker pool"))?;
    if pools.len() >= 4 {
        return Ok(pool);
    }
    Ok(Arc::clone(
        pools
            .entry(worker_count)
            .or_insert_with(|| Arc::clone(&pool)),
    ))
}

fn send_with_wait<T>(
    sender: &mpsc::SyncSender<T>,
    message: T,
    queue_wait_ms: &std::sync::atomic::AtomicU64,
) -> bool {
    let started = Instant::now();
    let result = sender.send(message);
    let waited = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    queue_wait_ms.fetch_add(waited, Ordering::Relaxed);
    result.is_ok()
}

fn parallel_queue_depth(options: &ReadOptions, workers: usize, tables: usize) -> usize {
    options.pipeline.resolve_queue_depth(workers, tables)
}

fn merge_parallel_worker_metadata(outcome: &mut ScanOutcome, worker: ScanOutcome) {
    outcome.bytes_read = outcome.bytes_read.saturating_add(worker.bytes_read);
    merge_parallel_worker_control(outcome, worker);
}

fn merge_parallel_worker_control(outcome: &mut ScanOutcome, worker: ScanOutcome) {
    outcome.tables_scanned = outcome.tables_scanned.saturating_add(worker.tables_scanned);
    outcome.worker_threads = outcome.worker_threads.max(worker.worker_threads);
    outcome.queue_wait_ms = outcome.queue_wait_ms.saturating_add(worker.queue_wait_ms);
    outcome.cancel_checks = outcome.cancel_checks.saturating_add(worker.cancel_checks);
    outcome.stopped |= worker.stopped;
}

fn allocate_file_number(manifest: &mut Manifest) -> u64 {
    let number = manifest.next_file_number;
    manifest.next_file_number = manifest.next_file_number.saturating_add(1);
    number
}

fn parse_file_number(path: &Path) -> Option<u64> {
    path.file_stem()?.to_str()?.parse().ok()
}

fn table_paths(root: &Path, manifest: &Manifest) -> Vec<PathBuf> {
    manifest
        .table_numbers
        .iter()
        .map(|number| root.join(Manifest::table_name(*number)))
        .filter(|path| path.exists())
        .collect()
}

fn manifest_tables(manifest: &Manifest) -> Vec<crate::manifest::TableFileMeta> {
    if manifest.table_files.is_empty() {
        return manifest
            .table_numbers
            .iter()
            .copied()
            .map(crate::manifest::TableFileMeta::without_range)
            .collect();
    }
    manifest.table_files.clone()
}

fn partition_paths_by_size(table_paths: Vec<PathBuf>, worker_count: usize) -> Vec<Vec<PathBuf>> {
    let worker_count = worker_count.max(1);
    let mut paths = table_paths
        .into_iter()
        .map(|path| {
            let size = std::fs::metadata(&path).map_or(0, |metadata| metadata.len());
            (path, size)
        })
        .collect::<Vec<_>>();
    paths.sort_by_key(|(_, size)| Reverse(*size));
    let mut worker_loads = vec![0_u64; worker_count];
    let mut worker_paths = vec![Vec::new(); worker_count];
    for (path, size) in paths {
        let Some((worker_index, load)) = worker_loads
            .iter_mut()
            .enumerate()
            .min_by_key(|(_, load)| **load)
        else {
            continue;
        };
        *load = load.saturating_add(size);
        worker_paths[worker_index].push(path);
    }
    worker_paths
}

// The parallel helpers keep their call sites explicit because each parameter is
// a separate scan policy; grouping them did not make the ownership easier.
#[allow(clippy::too_many_arguments, clippy::needless_pass_by_value)]
#[allow(clippy::too_many_lines)]
fn for_each_table_paths_parallel<F>(
    table_paths: Vec<PathBuf>,
    prefix: Option<Vec<u8>>,
    verify_checksums: bool,
    cache: Option<&table::NativeBlockCache>,
    threads: usize,
    hidden_keys: &BTreeMap<Vec<u8>, Option<Bytes>>,
    visitor: &mut F,
    options: &ReadOptions,
) -> Result<ScanOutcome>
where
    F: FnMut(&[u8], &Bytes) -> Result<VisitorControl> + Send,
{
    enum TableMessage {
        Entries(Vec<(Vec<u8>, Bytes)>),
        Error(LevelDbError),
        Outcome(ScanOutcome),
    }

    if table_paths.is_empty() {
        return Ok(ScanOutcome::empty());
    }
    let worker_count = threads.max(1).min(table_paths.len());
    let queue_depth = parallel_queue_depth(options, worker_count, table_paths.len());
    let worker_paths = partition_paths_by_size(table_paths, worker_count);

    let cancelled = Arc::new(AtomicBool::new(false));
    let queue_wait_ms = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (sender, receiver) = mpsc::sync_channel::<TableMessage>(queue_depth);
    let pool = scan_pool(worker_count)?;

    pool.scope(|scope| {
        for paths in worker_paths {
            let sender = sender.clone();
            let prefix = prefix.clone();
            let cancelled = cancelled.clone();
            let queue_wait_ms = Arc::clone(&queue_wait_ms);
            scope.spawn(move |_| {
                let mut batch = Vec::with_capacity(256);
                for path in paths {
                    if cancelled.load(Ordering::Relaxed) {
                        return;
                    }
                    let mut queue_entry = |key: &[u8], value: &Bytes| {
                        if hidden_keys.contains_key(key) {
                            return Ok(VisitorControl::Continue);
                        }
                        batch.push((key.to_vec(), value.clone()));
                        if batch.len() == 256 {
                            let entries = std::mem::replace(&mut batch, Vec::with_capacity(256));
                            if !send_with_wait(
                                &sender,
                                TableMessage::Entries(entries),
                                &queue_wait_ms,
                            ) {
                                cancelled.store(true, Ordering::Relaxed);
                                return Ok(VisitorControl::Stop);
                            }
                        }
                        Ok(VisitorControl::Continue)
                    };
                    let scan_result = if let Some(prefix) = &prefix {
                        table::for_each_table_prefix(
                            &path,
                            prefix,
                            verify_checksums,
                            cache,
                            &mut queue_entry,
                        )
                    } else {
                        table::for_each_table_entry(
                            &path,
                            verify_checksums,
                            cache,
                            &mut queue_entry,
                        )
                    };
                    if !batch.is_empty() {
                        let entries = std::mem::replace(&mut batch, Vec::with_capacity(256));
                        if !send_with_wait(&sender, TableMessage::Entries(entries), &queue_wait_ms)
                        {
                            cancelled.store(true, Ordering::Relaxed);
                            return;
                        }
                    }
                    match scan_result {
                        Ok(outcome) => {
                            if !send_with_wait(
                                &sender,
                                TableMessage::Outcome(outcome),
                                &queue_wait_ms,
                            ) {
                                cancelled.store(true, Ordering::Relaxed);
                                return;
                            }
                        }
                        Err(error) => {
                            if !send_with_wait(&sender, TableMessage::Error(error), &queue_wait_ms)
                            {
                                cancelled.store(true, Ordering::Relaxed);
                                return;
                            }
                            cancelled.store(true, Ordering::Relaxed);
                            return;
                        }
                    }
                }
            });
        }
        drop(sender);

        let mut outcome = ScanOutcome::empty();
        outcome.worker_threads = worker_count;
        for message in receiver {
            outcome.cancel_checks = outcome.cancel_checks.saturating_add(1);
            match message {
                TableMessage::Entries(entries) => {
                    for (key, value) in entries {
                        check_scan_cancelled(options)?;
                        outcome.record(value.len());
                        match visitor(&key, &value)? {
                            VisitorControl::Continue => {}
                            VisitorControl::Stop => {
                                outcome.stopped = true;
                                cancelled.store(true, Ordering::Relaxed);
                                outcome.queue_wait_ms = outcome.queue_wait_ms.saturating_add(
                                    u128::from(queue_wait_ms.load(Ordering::Relaxed)),
                                );
                                return Ok(outcome);
                            }
                        }
                        if should_emit_scan_progress(options, outcome.visited) {
                            emit_scan_progress(options, outcome);
                        }
                    }
                }
                TableMessage::Outcome(worker_outcome) => {
                    // Entry bytes are counted when the consumer invokes the
                    // visitor, so only merge worker control-plane metadata.
                    merge_parallel_worker_control(&mut outcome, worker_outcome);
                }
                TableMessage::Error(error) => {
                    cancelled.store(true, Ordering::Relaxed);
                    return Err(error);
                }
            }
        }
        outcome.queue_wait_ms = outcome
            .queue_wait_ms
            .saturating_add(u128::from(queue_wait_ms.load(Ordering::Relaxed)));
        Ok(outcome)
    })
}

#[allow(clippy::too_many_lines)]
fn for_each_table_key_paths_parallel<F>(
    table_paths: Vec<PathBuf>,
    verify_checksums: bool,
    cache: Option<&table::NativeBlockCache>,
    threads: usize,
    hidden_keys: &BTreeMap<Vec<u8>, Option<Bytes>>,
    visitor: &mut F,
    options: &ReadOptions,
) -> Result<ScanOutcome>
where
    F: FnMut(&[u8]) -> Result<VisitorControl> + Send,
{
    enum TableMessage {
        Keys(Vec<Vec<u8>>),
        Error(LevelDbError),
        Outcome(ScanOutcome),
    }

    if table_paths.is_empty() {
        return Ok(ScanOutcome::empty());
    }
    let worker_count = threads.max(1).min(table_paths.len());
    let queue_depth = parallel_queue_depth(options, worker_count, table_paths.len());
    let worker_paths = partition_paths_by_size(table_paths, worker_count);

    let cancelled = Arc::new(AtomicBool::new(false));
    let queue_wait_ms = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (sender, receiver) = mpsc::sync_channel::<TableMessage>(queue_depth);
    let pool = scan_pool(worker_count)?;

    pool.scope(|scope| {
        for paths in worker_paths {
            let sender = sender.clone();
            let cancelled = cancelled.clone();
            let queue_wait_ms = Arc::clone(&queue_wait_ms);
            scope.spawn(move |_| {
                let mut batch = Vec::with_capacity(256);
                for path in paths {
                    if cancelled.load(Ordering::Relaxed) {
                        return;
                    }
                    let scan_result =
                        table::for_each_table_key(&path, verify_checksums, cache, |key| {
                            if hidden_keys.contains_key(key) {
                                return Ok(VisitorControl::Continue);
                            }
                            batch.push(key.to_vec());
                            if batch.len() == 256
                                && !send_with_wait(
                                    &sender,
                                    TableMessage::Keys(std::mem::replace(
                                        &mut batch,
                                        Vec::with_capacity(256),
                                    )),
                                    &queue_wait_ms,
                                )
                            {
                                cancelled.store(true, Ordering::Relaxed);
                                return Ok(VisitorControl::Stop);
                            }
                            Ok(VisitorControl::Continue)
                        });
                    if !batch.is_empty()
                        && !send_with_wait(
                            &sender,
                            TableMessage::Keys(std::mem::replace(
                                &mut batch,
                                Vec::with_capacity(256),
                            )),
                            &queue_wait_ms,
                        )
                    {
                        cancelled.store(true, Ordering::Relaxed);
                        return;
                    }
                    match scan_result {
                        Ok(outcome) => {
                            if !send_with_wait(
                                &sender,
                                TableMessage::Outcome(outcome),
                                &queue_wait_ms,
                            ) {
                                cancelled.store(true, Ordering::Relaxed);
                                return;
                            }
                        }
                        Err(error) => {
                            let _ =
                                send_with_wait(&sender, TableMessage::Error(error), &queue_wait_ms);
                            cancelled.store(true, Ordering::Relaxed);
                            return;
                        }
                    }
                }
            });
        }
        drop(sender);

        let mut outcome = ScanOutcome::empty();
        outcome.worker_threads = worker_count;
        for message in receiver {
            outcome.cancel_checks = outcome.cancel_checks.saturating_add(1);
            match message {
                TableMessage::Keys(keys) => {
                    for key in keys {
                        check_scan_cancelled(options)?;
                        outcome.record(0);
                        if visitor(&key)? == VisitorControl::Stop {
                            outcome.stopped = true;
                            cancelled.store(true, Ordering::Relaxed);
                            outcome.queue_wait_ms = outcome
                                .queue_wait_ms
                                .saturating_add(u128::from(queue_wait_ms.load(Ordering::Relaxed)));
                            return Ok(outcome);
                        }
                        if should_emit_scan_progress(options, outcome.visited) {
                            emit_scan_progress(options, outcome);
                        }
                    }
                }
                TableMessage::Outcome(worker_outcome) => {
                    merge_parallel_worker_metadata(&mut outcome, worker_outcome);
                }
                TableMessage::Error(error) => {
                    cancelled.store(true, Ordering::Relaxed);
                    return Err(error);
                }
            }
        }
        outcome.queue_wait_ms = outcome
            .queue_wait_ms
            .saturating_add(u128::from(queue_wait_ms.load(Ordering::Relaxed)));
        Ok(outcome)
    })
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn for_each_table_prefix_key_paths_parallel<F>(
    table_paths: Vec<PathBuf>,
    prefix: Vec<u8>,
    verify_checksums: bool,
    cache: Option<&table::NativeBlockCache>,
    threads: usize,
    hidden_keys: &BTreeMap<Vec<u8>, Option<Bytes>>,
    visitor: &mut F,
    options: &ReadOptions,
) -> Result<ScanOutcome>
where
    F: FnMut(&[u8]) -> Result<VisitorControl> + Send,
{
    enum TableMessage {
        Keys(Vec<Vec<u8>>),
        Error(LevelDbError),
        Outcome(ScanOutcome),
    }

    if table_paths.is_empty() {
        return Ok(ScanOutcome::empty());
    }
    let worker_count = threads.max(1).min(table_paths.len());
    let queue_depth = parallel_queue_depth(options, worker_count, table_paths.len());
    let worker_paths = partition_paths_by_size(table_paths, worker_count);

    log::debug!(
        "starting parallel prefix key scan (workers={}, prefix_len={})",
        worker_count,
        prefix.len()
    );

    let prefix = Arc::new(prefix);
    let cancelled = Arc::new(AtomicBool::new(false));
    let queue_wait_ms = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (sender, receiver) = mpsc::sync_channel::<TableMessage>(queue_depth);
    let pool = scan_pool(worker_count)?;

    pool.scope(|scope| {
        for (worker_index, paths) in worker_paths.into_iter().enumerate() {
            let sender = sender.clone();
            let prefix = prefix.clone();
            let cancelled = cancelled.clone();
            let queue_wait_ms = Arc::clone(&queue_wait_ms);
            scope.spawn(move |_| {
                let mut batch = Vec::with_capacity(256);
                log::trace!(
                    "prefix key scan worker {} started with {} table(s)",
                    worker_index,
                    paths.len()
                );
                for path in paths {
                    if cancelled.load(Ordering::Relaxed) {
                        log::trace!("prefix key scan worker {worker_index} cancelled");
                        return;
                    }
                    let scan_result = table::for_each_table_prefix_key(
                        &path,
                        &prefix,
                        verify_checksums,
                        cache,
                        |key| {
                            if hidden_keys.contains_key(key) {
                                return Ok(VisitorControl::Continue);
                            }
                            batch.push(key.to_vec());
                            if batch.len() == 256
                                && !send_with_wait(
                                    &sender,
                                    TableMessage::Keys(std::mem::replace(
                                        &mut batch,
                                        Vec::with_capacity(256),
                                    )),
                                    &queue_wait_ms,
                                )
                            {
                                cancelled.store(true, Ordering::Relaxed);
                                return Ok(VisitorControl::Stop);
                            }
                            Ok(VisitorControl::Continue)
                        },
                    );
                    if !batch.is_empty()
                        && !send_with_wait(
                            &sender,
                            TableMessage::Keys(std::mem::replace(
                                &mut batch,
                                Vec::with_capacity(256),
                            )),
                            &queue_wait_ms,
                        )
                    {
                        cancelled.store(true, Ordering::Relaxed);
                        return;
                    }
                    match scan_result {
                        Ok(outcome) => {
                            if !send_with_wait(
                                &sender,
                                TableMessage::Outcome(outcome),
                                &queue_wait_ms,
                            ) {
                                cancelled.store(true, Ordering::Relaxed);
                                return;
                            }
                        }
                        Err(error) => {
                            let _ =
                                send_with_wait(&sender, TableMessage::Error(error), &queue_wait_ms);
                            cancelled.store(true, Ordering::Relaxed);
                            return;
                        }
                    }
                }
                log::trace!("prefix key scan worker {worker_index} finished");
            });
        }
        drop(sender);

        let mut outcome = ScanOutcome::empty();
        outcome.worker_threads = worker_count;
        for message in receiver {
            outcome.cancel_checks = outcome.cancel_checks.saturating_add(1);
            match message {
                TableMessage::Keys(keys) => {
                    for key in keys {
                        check_scan_cancelled(options)?;
                        outcome.record(0);
                        if visitor(&key)? == VisitorControl::Stop {
                            outcome.stopped = true;
                            cancelled.store(true, Ordering::Relaxed);
                            outcome.queue_wait_ms = outcome
                                .queue_wait_ms
                                .saturating_add(u128::from(queue_wait_ms.load(Ordering::Relaxed)));
                            return Ok(outcome);
                        }
                        if should_emit_scan_progress(options, outcome.visited) {
                            emit_scan_progress(options, outcome);
                        }
                    }
                }
                TableMessage::Outcome(worker_outcome) => {
                    merge_parallel_worker_metadata(&mut outcome, worker_outcome);
                }
                TableMessage::Error(error) => {
                    cancelled.store(true, Ordering::Relaxed);
                    return Err(error);
                }
            }
        }
        outcome.queue_wait_ms = outcome
            .queue_wait_ms
            .saturating_add(u128::from(queue_wait_ms.load(Ordering::Relaxed)));
        Ok(outcome)
    })
}

#[allow(clippy::too_many_arguments)]
fn for_each_table_key_paths_partitioned<T, I, F>(
    table_paths: Vec<PathBuf>,
    verify_checksums: bool,
    cache: Option<&table::NativeBlockCache>,
    threads: usize,
    hidden_keys: &BTreeMap<Vec<u8>, Option<Bytes>>,
    init: &I,
    visitor: &F,
    options: &ReadOptions,
) -> Result<(ScanOutcome, Vec<T>)>
where
    T: Send,
    I: Fn() -> T + Send + Sync,
    F: Fn(&mut T, &[u8]) -> Result<VisitorControl> + Send + Sync,
{
    enum TableMessage<T> {
        Partition(T, ScanOutcome),
        Error(LevelDbError),
    }

    if table_paths.is_empty() {
        return Ok((ScanOutcome::empty(), Vec::new()));
    }
    let worker_count = threads.max(1).min(table_paths.len());
    let queue_depth = parallel_queue_depth(options, worker_count, table_paths.len());
    let worker_paths = partition_paths_by_size(table_paths, worker_count);

    let cancelled = Arc::new(AtomicBool::new(false));
    let queue_wait_ms = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (sender, receiver) = mpsc::sync_channel::<TableMessage<T>>(queue_depth);
    let pool = scan_pool(worker_count)?;
    pool.scope(|scope| {
        for paths in worker_paths {
            let sender = sender.clone();
            let cancelled = cancelled.clone();
            let queue_wait_ms = Arc::clone(&queue_wait_ms);
            scope.spawn(move |_| {
                let mut partition = init();
                let mut outcome = ScanOutcome::empty();
                for path in paths {
                    if cancelled.load(Ordering::Relaxed) {
                        return;
                    }
                    let scan_result =
                        table::for_each_table_key(&path, verify_checksums, cache, |key| {
                            if hidden_keys.contains_key(key) {
                                return Ok(VisitorControl::Continue);
                            }
                            visitor(&mut partition, key)
                        });
                    match scan_result {
                        Ok(table_outcome) => {
                            outcome.merge(table_outcome);
                            if outcome.stopped {
                                cancelled.store(true, Ordering::Relaxed);
                                break;
                            }
                        }
                        Err(error) => {
                            let _ =
                                send_with_wait(&sender, TableMessage::Error(error), &queue_wait_ms);
                            cancelled.store(true, Ordering::Relaxed);
                            return;
                        }
                    }
                }
                let _ = send_with_wait(
                    &sender,
                    TableMessage::Partition(partition, outcome),
                    &queue_wait_ms,
                );
            });
        }
        drop(sender);
        let mut outcome = ScanOutcome::empty();
        outcome.worker_threads = worker_count;
        let mut partitions = Vec::new();
        for message in receiver {
            outcome.cancel_checks = outcome.cancel_checks.saturating_add(1);
            check_scan_cancelled(options)?;
            match message {
                TableMessage::Partition(partition, partition_outcome) => {
                    outcome.merge(partition_outcome);
                    partitions.push(partition);
                    if outcome.stopped {
                        cancelled.store(true, Ordering::Relaxed);
                    }
                }
                TableMessage::Error(error) => {
                    cancelled.store(true, Ordering::Relaxed);
                    return Err(error);
                }
            }
        }
        outcome.queue_wait_ms = outcome
            .queue_wait_ms
            .saturating_add(u128::from(queue_wait_ms.load(Ordering::Relaxed)));
        Ok((outcome, partitions))
    })
}

#[allow(clippy::too_many_arguments)]
fn for_each_table_paths_partitioned<T, I, F>(
    table_paths: Vec<PathBuf>,
    verify_checksums: bool,
    cache: Option<&table::NativeBlockCache>,
    threads: usize,
    hidden_keys: &BTreeMap<Vec<u8>, Option<Bytes>>,
    init: &I,
    visitor: &F,
    options: &ReadOptions,
) -> Result<(ScanOutcome, Vec<T>)>
where
    T: Send,
    I: Fn() -> T + Send + Sync,
    F: Fn(&mut T, &[u8], &Bytes) -> Result<VisitorControl> + Send + Sync,
{
    enum TableMessage<T> {
        Partition(T, ScanOutcome),
        Error(LevelDbError),
    }

    if table_paths.is_empty() {
        return Ok((ScanOutcome::empty(), Vec::new()));
    }
    let worker_count = threads.max(1).min(table_paths.len());
    let queue_depth = parallel_queue_depth(options, worker_count, table_paths.len());
    let worker_paths = partition_paths_by_size(table_paths, worker_count);

    let cancelled = Arc::new(AtomicBool::new(false));
    let queue_wait_ms = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (sender, receiver) = mpsc::sync_channel::<TableMessage<T>>(queue_depth);
    let pool = scan_pool(worker_count)?;
    pool.scope(|scope| {
        for paths in worker_paths {
            let sender = sender.clone();
            let cancelled = cancelled.clone();
            let queue_wait_ms = Arc::clone(&queue_wait_ms);
            scope.spawn(move |_| {
                let mut partition = init();
                let mut outcome = ScanOutcome::empty();
                for path in paths {
                    if cancelled.load(Ordering::Relaxed) {
                        return;
                    }
                    let scan_result = table::for_each_table_entry(
                        &path,
                        verify_checksums,
                        cache,
                        |key, value| {
                            if hidden_keys.contains_key(key) {
                                return Ok(VisitorControl::Continue);
                            }
                            visitor(&mut partition, key, value)
                        },
                    );
                    match scan_result {
                        Ok(table_outcome) => {
                            outcome.merge(table_outcome);
                            if outcome.stopped {
                                cancelled.store(true, Ordering::Relaxed);
                                break;
                            }
                        }
                        Err(error) => {
                            let _ =
                                send_with_wait(&sender, TableMessage::Error(error), &queue_wait_ms);
                            cancelled.store(true, Ordering::Relaxed);
                            return;
                        }
                    }
                }
                let _ = send_with_wait(
                    &sender,
                    TableMessage::Partition(partition, outcome),
                    &queue_wait_ms,
                );
            });
        }
        drop(sender);
        let mut outcome = ScanOutcome::empty();
        outcome.worker_threads = worker_count;
        let mut partitions = Vec::new();
        for message in receiver {
            outcome.cancel_checks = outcome.cancel_checks.saturating_add(1);
            check_scan_cancelled(options)?;
            match message {
                TableMessage::Partition(partition, partition_outcome) => {
                    outcome.merge(partition_outcome);
                    partitions.push(partition);
                    if outcome.stopped {
                        cancelled.store(true, Ordering::Relaxed);
                    }
                }
                TableMessage::Error(error) => {
                    cancelled.store(true, Ordering::Relaxed);
                    return Err(error);
                }
            }
        }
        outcome.queue_wait_ms = outcome
            .queue_wait_ms
            .saturating_add(u128::from(queue_wait_ms.load(Ordering::Relaxed)));
        Ok((outcome, partitions))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::{CompressionPolicy, ScanCancelFlag};
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "bedrock-leveldb-{name}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ))
    }

    #[test]
    fn db_recovers_wal_after_reopen() {
        let path = temp_dir("wal");
        let options = OpenOptions {
            compression_policy: CompressionPolicy::None,
            ..OpenOptions::default()
        };
        {
            let db = Db::open(&path, options.clone()).expect("open");
            db.put(
                b"player_1".as_slice(),
                b"one".as_slice(),
                WriteOptions::default(),
            )
            .expect("put");
        }
        let db = Db::open(&path, options).expect("reopen");
        assert_eq!(
            db.get(b"player_1").expect("get"),
            Some(Bytes::from_static(b"one"))
        );
        std::fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn zero_write_buffer_keeps_writes_in_wal_until_explicit_flush() {
        let path = temp_dir("wal-no-auto-flush");
        let options = OpenOptions {
            compression_policy: CompressionPolicy::None,
            write_buffer_size: 0,
            ..OpenOptions::default()
        };
        {
            let db = Db::open(&path, options.clone()).expect("open");
            let value = Bytes::from(vec![1_u8; 4096]);
            for index in 0..8 {
                db.put(
                    Bytes::from(format!("chunk:{index}")),
                    value.clone(),
                    WriteOptions::default(),
                )
                .expect("put");
            }
            assert_eq!(db.stats_fast().expect("stats").tables, 0);
        }

        let db = Db::open(&path, options).expect("reopen");
        assert_eq!(db.stats_fast().expect("stats").tables, 0);
        assert_eq!(
            db.get(b"chunk:7").expect("get"),
            Some(Bytes::from(vec![1_u8; 4096]))
        );
        std::fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn compaction_reclaims_old_tables_and_tombstones() {
        let path = temp_dir("compaction-reclaims");
        let options = OpenOptions {
            compression_policy: CompressionPolicy::None,
            write_buffer_size: 0,
            ..OpenOptions::default()
        };
        let db = Db::open(&path, options.clone()).expect("open");
        db.put(b"a".as_slice(), b"one".as_slice(), WriteOptions::default())
            .expect("put a");
        db.put(b"b".as_slice(), b"two".as_slice(), WriteOptions::default())
            .expect("put b");
        db.flush().expect("first flush");
        db.put(
            b"a".as_slice(),
            b"updated".as_slice(),
            WriteOptions::default(),
        )
        .expect("update a");
        db.delete(b"b".as_slice(), WriteOptions::default())
            .expect("delete b");
        db.put(
            b"c".as_slice(),
            b"three".as_slice(),
            WriteOptions::default(),
        )
        .expect("put c");

        db.compact_range_native(None, None).expect("compact");

        assert_eq!(
            db.get(b"a").expect("get a"),
            Some(Bytes::from_static(b"updated"))
        );
        assert_eq!(db.get(b"b").expect("get b"), None);
        assert_eq!(
            db.get(b"c").expect("get c"),
            Some(Bytes::from_static(b"three"))
        );
        assert_eq!(db.stats_fast().expect("stats").tables, 1);
        drop(db);

        let reopened = Db::open(&path, options).expect("reopen");
        assert_eq!(reopened.get(b"b").expect("get deleted after reopen"), None);
        assert_eq!(
            reopened.get(b"c").expect("get c after reopen"),
            Some(Bytes::from_static(b"three"))
        );
        drop(reopened);
        std::fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn db_flushes_and_scans_prefix() {
        let path = temp_dir("scan");
        let options = OpenOptions {
            compression_policy: CompressionPolicy::None,
            ..OpenOptions::default()
        };
        let db = Db::open(&path, options).expect("open");
        db.put(
            b"abc1".as_slice(),
            b"one".as_slice(),
            WriteOptions::default(),
        )
        .expect("put");
        db.put(
            b"abc2".as_slice(),
            b"two".as_slice(),
            WriteOptions::default(),
        )
        .expect("put");
        db.put(
            b"abd".as_slice(),
            b"three".as_slice(),
            WriteOptions::default(),
        )
        .expect("put");
        db.flush().expect("flush");

        let mut values = Vec::new();
        db.for_each_prefix(b"abc", ReadOptions::default(), |key, value| {
            values.push((Bytes::copy_from_slice(key), value.clone()));
            Ok(VisitorControl::Continue)
        })
        .expect("scan");
        assert_eq!(values.len(), 2);
        assert_eq!(values[1].1, Bytes::from_static(b"two"));
        let stats = db.stats().expect("stats");
        assert!(stats.approximate_bytes >= 18);
        std::fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn db_scans_prefix_keys_without_values() {
        let path = temp_dir("scan-prefix-keys");
        let options = OpenOptions {
            compression_policy: CompressionPolicy::None,
            ..OpenOptions::default()
        };
        let db = Db::open(&path, options).expect("open");
        db.put(
            b"chunk_a".as_slice(),
            b"one".as_slice(),
            WriteOptions::default(),
        )
        .expect("put a");
        db.put(
            b"chunk_b".as_slice(),
            b"two".as_slice(),
            WriteOptions::default(),
        )
        .expect("put b");
        db.put(
            b"other".as_slice(),
            b"three".as_slice(),
            WriteOptions::default(),
        )
        .expect("put other");
        db.flush().expect("flush");

        let mut keys = Vec::new();
        let outcome = db
            .for_each_prefix_key(b"chunk_", ReadOptions::default(), |key| {
                keys.push(Bytes::copy_from_slice(key));
                Ok(VisitorControl::Continue)
            })
            .expect("prefix key scan");

        assert_eq!(
            keys,
            vec![
                Bytes::from_static(b"chunk_a"),
                Bytes::from_static(b"chunk_b")
            ]
        );
        assert_eq!(outcome.visited, 2);
        std::fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn owned_collectors_return_keys_and_values() {
        let path = temp_dir("owned-collectors");
        let db = Db::open(&path, OpenOptions::default()).expect("open");
        db.put(
            b"chunk_a".as_slice(),
            b"one".as_slice(),
            WriteOptions::default(),
        )
        .expect("put a");
        db.put(
            b"chunk_b".as_slice(),
            b"two".as_slice(),
            WriteOptions::default(),
        )
        .expect("put b");
        db.put(
            b"other".as_slice(),
            b"three".as_slice(),
            WriteOptions::default(),
        )
        .expect("put other");

        let keys = db
            .collect_prefix_keys_owned(b"chunk_", ReadOptions::default())
            .expect("collect keys");
        let entries = db
            .collect_prefix_owned(b"chunk_", ReadOptions::default())
            .expect("collect entries");

        assert_eq!(
            keys,
            vec![
                Bytes::from_static(b"chunk_a"),
                Bytes::from_static(b"chunk_b")
            ]
        );
        assert_eq!(
            entries,
            vec![
                (Bytes::from_static(b"chunk_a"), Bytes::from_static(b"one")),
                (Bytes::from_static(b"chunk_b"), Bytes::from_static(b"two")),
            ]
        );
        std::fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn borrowed_first_read_api_exposes_entry_refs_without_owned_collection() {
        let path = temp_dir("entry-ref-api");
        let db = Db::open(
            &path,
            OpenOptions {
                compression_policy: CompressionPolicy::None,
                ..OpenOptions::default()
            },
        )
        .expect("open");
        db.put(
            b"chunk_a".as_slice(),
            b"one".as_slice(),
            WriteOptions::default(),
        )
        .expect("put a");
        db.put(
            b"chunk_b".as_slice(),
            b"two".as_slice(),
            WriteOptions::default(),
        )
        .expect("put b");
        db.flush().expect("flush");

        let value = db
            .get_with_ref(
                b"chunk_a",
                ReadOptions {
                    read_strategy: crate::options::ReadStrategy::Shared,
                    ..ReadOptions::default()
                },
            )
            .expect("get ref")
            .expect("value");
        assert_eq!(value.as_bytes(), b"one");

        let owned = db
            .get_with_ref(
                b"chunk_a",
                ReadOptions {
                    read_strategy: crate::options::ReadStrategy::Owned,
                    ..ReadOptions::default()
                },
            )
            .expect("get owned ref")
            .expect("value");
        assert!(matches!(owned, ValueRef::Owned(_)));

        let mut entries = Vec::new();
        db.for_each_prefix_ref(b"chunk_", ReadOptions::default(), |entry| {
            entries.push((
                Bytes::copy_from_slice(entry.key.as_bytes()),
                Bytes::copy_from_slice(entry.value.as_bytes()),
            ));
            Ok(VisitorControl::Continue)
        })
        .expect("prefix refs");
        assert_eq!(
            entries,
            vec![
                (Bytes::from_static(b"chunk_a"), Bytes::from_static(b"one")),
                (Bytes::from_static(b"chunk_b"), Bytes::from_static(b"two")),
            ]
        );
        std::fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn borrowed_strategy_scans_uncompressed_custom_table_as_borrowed() {
        let path = temp_dir("borrowed-uncompressed");
        let db = Db::open(
            &path,
            OpenOptions {
                compression_policy: CompressionPolicy::None,
                ..OpenOptions::default()
            },
        )
        .expect("open");
        db.put(
            b"chunk_a".as_slice(),
            b"one".as_slice(),
            WriteOptions::default(),
        )
        .expect("put a");
        db.put(
            b"chunk_b".as_slice(),
            b"two".as_slice(),
            WriteOptions::default(),
        )
        .expect("put b");
        db.flush().expect("flush");

        let mut borrowed = 0usize;
        let mut values = Vec::new();
        db.for_each_prefix_ref(
            b"chunk_",
            ReadOptions {
                read_strategy: ReadStrategy::Borrowed,
                scan_mode: ScanMode::Sequential,
                ..ReadOptions::default()
            },
            |entry| {
                if matches!(entry.value, ValueRef::Borrowed(_)) {
                    borrowed = borrowed.saturating_add(1);
                }
                values.push(Bytes::copy_from_slice(entry.value.as_bytes()));
                Ok(VisitorControl::Continue)
            },
        )
        .expect("borrowed prefix scan");

        assert_eq!(
            values,
            vec![Bytes::from_static(b"one"), Bytes::from_static(b"two")]
        );
        assert_eq!(borrowed, 2);
        std::fs::remove_dir_all(path).expect("cleanup");
    }

    #[cfg(feature = "zlib")]
    #[test]
    fn borrowed_strategy_keeps_compressed_custom_table_values_shared() {
        let path = temp_dir("borrowed-compressed");
        let db = Db::open(
            &path,
            OpenOptions {
                compression_policy: CompressionPolicy::Zlib,
                ..OpenOptions::default()
            },
        )
        .expect("open");
        db.put(
            b"chunk_a".as_slice(),
            b"one".as_slice(),
            WriteOptions::default(),
        )
        .expect("put a");
        db.put(
            b"chunk_b".as_slice(),
            b"two".as_slice(),
            WriteOptions::default(),
        )
        .expect("put b");
        db.flush().expect("flush");

        let mut shared = 0usize;
        db.for_each_prefix_ref(
            b"chunk_",
            ReadOptions {
                read_strategy: ReadStrategy::Borrowed,
                scan_mode: ScanMode::Sequential,
                ..ReadOptions::default()
            },
            |entry| {
                if matches!(entry.value, ValueRef::Shared(_)) {
                    shared = shared.saturating_add(1);
                }
                Ok(VisitorControl::Continue)
            },
        )
        .expect("compressed prefix scan");

        assert_eq!(shared, 2);
        std::fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn read_state_snapshot_allows_concurrent_lock_free_table_reads() {
        let path = temp_dir("concurrent-reads");
        let db = Arc::new(
            Db::open(
                &path,
                OpenOptions {
                    compression_policy: CompressionPolicy::None,
                    ..OpenOptions::default()
                },
            )
            .expect("open"),
        );
        let mut batch = WriteBatch::new();
        for index in 0..64 {
            batch.put(
                Bytes::from(format!("key:{index:03}")),
                Bytes::from(format!("value:{index:03}")),
            );
        }
        db.write(batch, WriteOptions::default()).expect("write");
        db.flush().expect("flush");

        let handles = (0..8)
            .map(|_| {
                let db = Arc::clone(&db);
                std::thread::spawn(move || {
                    for index in 0..64 {
                        let key = format!("key:{index:03}");
                        let value = db
                            .get_ref(key.as_bytes())
                            .expect("get")
                            .expect("value")
                            .into_bytes();
                        assert_eq!(value, Bytes::from(format!("value:{index:03}")));
                    }
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            handle.join().expect("reader thread");
        }
        std::fs::remove_dir_all(path).expect("cleanup");
    }

    #[cfg(feature = "async")]
    #[test]
    fn async_owned_reads_collect_prefix_keys() {
        let path = temp_dir("async-prefix-keys");
        let options = OpenOptions {
            compression_policy: CompressionPolicy::None,
            ..OpenOptions::default()
        };
        let db = Arc::new(Db::open(&path, options).expect("open"));
        db.put(
            b"chunk_a".as_slice(),
            b"one".as_slice(),
            WriteOptions::default(),
        )
        .expect("put a");
        db.put(
            b"chunk_b".as_slice(),
            b"two".as_slice(),
            WriteOptions::default(),
        )
        .expect("put b");

        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        let keys = runtime
            .block_on(db.clone().collect_prefix_keys_owned_async(
                Bytes::from_static(b"chunk_"),
                ReadOptions::default(),
            ))
            .expect("async prefix keys");
        let value = runtime
            .block_on(db.get_async(Bytes::from_static(b"chunk_a")))
            .expect("async get")
            .expect("value");

        assert_eq!(
            keys,
            vec![
                Bytes::from_static(b"chunk_a"),
                Bytes::from_static(b"chunk_b")
            ]
        );
        assert_eq!(value, Bytes::from_static(b"one"));
        std::fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn db_key_scan_can_stop_without_error() {
        let path = temp_dir("key-scan-stop");
        let db = Db::open(&path, OpenOptions::default()).expect("open");
        db.put(b"a".as_slice(), b"one".as_slice(), WriteOptions::default())
            .expect("put a");
        db.put(b"b".as_slice(), b"two".as_slice(), WriteOptions::default())
            .expect("put b");

        let mut keys = Vec::new();
        let outcome = db
            .for_each_key(ReadOptions::default(), |key| {
                keys.push(Bytes::copy_from_slice(key));
                Ok(VisitorControl::Stop)
            })
            .expect("scan keys");
        assert!(outcome.stopped);
        assert_eq!(keys.len(), 1);
        std::fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn db_scan_cancel_returns_typed_error() {
        let path = temp_dir("scan-cancel");
        let db = Db::open(&path, OpenOptions::default()).expect("open");
        db.put(b"a".as_slice(), b"one".as_slice(), WriteOptions::default())
            .expect("put");
        let cancel = ScanCancelFlag::new();
        cancel.cancel();
        let result = db.for_each_key(
            ReadOptions {
                cancel: Some(cancel),
                ..ReadOptions::default()
            },
            |_key| Ok(VisitorControl::Continue),
        );
        assert_eq!(
            result.expect_err("cancelled scan").kind(),
            ErrorKind::Cancelled
        );
        std::fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn parallel_scan_matches_sequential_scan() {
        let path = temp_dir("parallel-scan");
        let options = OpenOptions {
            compression_policy: CompressionPolicy::None,
            ..OpenOptions::default()
        };
        let db = Db::open(&path, options).expect("open");
        let mut batch = WriteBatch::new();
        for index in 0..128 {
            batch.put(
                Bytes::from(format!("key:{index:03}")),
                Bytes::from(format!("value:{index:03}")),
            );
        }
        db.write(batch, WriteOptions::default()).expect("write");
        db.flush().expect("flush");

        let mut sequential = Vec::new();
        db.for_each_key(ReadOptions::default(), |key| {
            sequential.push(Bytes::copy_from_slice(key));
            Ok(VisitorControl::Continue)
        })
        .expect("sequential");
        let mut parallel = Vec::new();
        db.for_each_key(
            ReadOptions {
                scan_mode: ScanMode::ParallelTables,
                ..ReadOptions::default()
            },
            |key| {
                parallel.push(Bytes::copy_from_slice(key));
                Ok(VisitorControl::Continue)
            },
        )
        .expect("parallel");
        sequential.sort();
        parallel.sort();
        assert_eq!(parallel, sequential);
        std::fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn partitioned_key_scan_reduces_locally() {
        let path = temp_dir("partitioned-key-scan");
        let options = OpenOptions {
            compression_policy: CompressionPolicy::None,
            ..OpenOptions::default()
        };
        let db = Db::open(&path, options).expect("open");
        let mut batch = WriteBatch::new();
        for index in 0..64 {
            batch.put(
                Bytes::from(format!("key:{index:03}")),
                Bytes::from(format!("value:{index:03}")),
            );
        }
        db.write(batch, WriteOptions::default()).expect("write");
        db.flush().expect("flush");

        let (outcome, partitions) = db
            .scan_keys_partitioned(
                ReadOptions {
                    scan_mode: ScanMode::ParallelTables,
                    ..ReadOptions::default()
                },
                Vec::<Bytes>::new,
                |partition, key| {
                    partition.push(Bytes::copy_from_slice(key));
                    Ok(VisitorControl::Continue)
                },
            )
            .expect("partitioned scan");
        let total_keys = partitions.iter().map(Vec::len).sum::<usize>();
        assert_eq!(outcome.visited, 64);
        assert_eq!(total_keys, 64);
        std::fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn partitioned_entry_scan_reduces_values_locally() {
        let path = temp_dir("partitioned-entry-scan");
        let options = OpenOptions {
            compression_policy: CompressionPolicy::None,
            ..OpenOptions::default()
        };
        let db = Db::open(&path, options).expect("open");
        let mut batch = WriteBatch::new();
        for index in 0..32 {
            batch.put(
                Bytes::from(format!("key:{index:03}")),
                Bytes::from_static(b"value"),
            );
        }
        db.write(batch, WriteOptions::default()).expect("write");
        db.flush().expect("flush");

        let (outcome, partitions) = db
            .scan_entries_partitioned(
                ReadOptions {
                    scan_mode: ScanMode::ParallelTables,
                    ..ReadOptions::default()
                },
                || 0usize,
                |partition, _key, value| {
                    *partition = partition.saturating_add(value.len());
                    Ok(VisitorControl::Continue)
                },
            )
            .expect("partitioned entry scan");
        assert_eq!(outcome.visited, 32);
        assert_eq!(partitions.into_iter().sum::<usize>(), 32 * 5);
        std::fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn snapshot_preserves_old_view() {
        let path = temp_dir("snapshot");
        let db = Db::open(&path, OpenOptions::default()).expect("open");
        db.put(b"k".as_slice(), b"old".as_slice(), WriteOptions::default())
            .expect("put old");
        let snapshot = db.snapshot().expect("snapshot");
        db.put(b"k".as_slice(), b"new".as_slice(), WriteOptions::default())
            .expect("put new");
        assert_eq!(snapshot.get(b"k"), Some(Bytes::from_static(b"old")));
        assert_eq!(db.get(b"k").expect("get"), Some(Bytes::from_static(b"new")));
        std::fs::remove_dir_all(path).expect("cleanup");
    }
}
