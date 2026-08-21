use crate::batch::{WriteBatch, WriteOp};
use crate::compaction;
use crate::db_lock::DatabaseLock;
use crate::error::{ErrorKind, LevelDbError, Result};
use crate::manifest::{Manifest, TableFileMeta};
use crate::native_table_writer::{NativeTableWriter, WrittenNativeTable};
use crate::obsolete;
use crate::options::{
    CachePolicy, ChecksumMode, CompressionPolicy, LevelDbOpenOptions, ReadOptions, ReadStrategy,
    ScanMode, ScanOutcome, VisitorControl, WriteOptions,
};
use crate::table;
use crate::version::{ImmutableMemTable, MemTableEntries, ReadVersion};
use crate::wal;
use bytes::Bytes;
use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Condvar, Mutex, RwLock, Weak,
    atomic::{AtomicU64, Ordering},
    mpsc::{self, Receiver, SyncSender},
};
use std::thread::{self, JoinHandle};
use std::time::Instant;

const BACKGROUND_QUEUE_DEPTH: usize = 4;
const BACKGROUND_STACK_BYTES: usize = 2 * 1024 * 1024;
const AUTO_FLUSH_HARD_MULTIPLIER: usize = 2;

thread_local! {
    static WAL_ENCODE_SCRATCH: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(16 * 1024));
}

/// Fast or full database statistics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbStats {
    /// Visible entries counted by the selected stats path.
    pub entries: usize,
    /// Table files listed in the current manifest.
    pub tables: usize,
    /// Active log file number.
    pub log_number: u64,
    /// Approximate visible bytes or memtable bytes, depending on the stats path.
    pub approximate_bytes: usize,
}

/// Snapshot of the sharded native table caches.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DbCacheStats {
    /// Decoded data-block cache hits.
    pub data_hits: u64,
    /// Decoded data-block cache misses.
    pub data_misses: u64,
    /// Decoded data-block cache evictions.
    pub data_evictions: u64,
    /// Table-index cache hits.
    pub index_hits: u64,
    /// Table-index cache misses.
    pub index_misses: u64,
    /// Table-index cache evictions.
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

    /// Returns a shared value handle for `key`, if visible.
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

/// Borrowed raw key view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyRef<'a> {
    bytes: &'a [u8],
}

impl<'a> KeyRef<'a> {
    /// Creates a key view.
    #[must_use]
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    /// Returns the raw bytes.
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
    /// Shared immutable bytes.
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

    /// Returns the value length.
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

/// Raw key/value entry view used by visitor-compatible scan APIs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryRef<'a> {
    /// Raw key bytes.
    pub key: KeyRef<'a>,
    /// Raw value bytes.
    pub value: ValueRef<'a>,
}

struct SharedDb {
    root: PathBuf,
    options: LevelDbOpenOptions,
    inner: RwLock<DbInner>,
    block_cache: Arc<table::NativeBlockCache>,
    next_file_number: AtomicU64,
    manifest_io: Mutex<()>,
}

struct DbInner {
    active: MemTableEntries,
    active_bytes: usize,
    immutable: Option<Arc<ImmutableMemTable>>,
    manifest: Manifest,
    version: Arc<ReadVersion>,
    last_sequence: u64,
}

struct ReadState {
    active: Arc<MemTableEntries>,
    immutable: Option<Arc<ImmutableMemTable>>,
    version: Arc<ReadVersion>,
    sequence: u64,
}

/// Open database handle.
pub struct Db {
    shared: Arc<SharedDb>,
    background: Option<Background>,
    write_serial: Mutex<()>,
    _database_lock: Option<DatabaseLock>,
}

struct Background {
    sender: SyncSender<BackgroundCommand>,
    status: Arc<(Mutex<BackgroundStatus>, Condvar)>,
    worker: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct BackgroundStatus {
    pending_flushes: usize,
    fatal_error: Option<String>,
}

enum BackgroundCommand {
    Flush(Arc<ImmutableMemTable>),
    Compact {
        force: bool,
        done: mpsc::Sender<Result<()>>,
    },
    Shutdown,
}

struct PendingCompactionOutputs {
    paths: Vec<PathBuf>,
    tables: Vec<TableFileMeta>,
    committed: bool,
}

impl PendingCompactionOutputs {
    fn new() -> Self {
        Self {
            paths: Vec::new(),
            tables: Vec::new(),
            committed: false,
        }
    }

    fn push(&mut self, path: PathBuf, table: TableFileMeta) {
        self.paths.push(path);
        self.tables.push(table);
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for PendingCompactionOutputs {
    fn drop(&mut self) {
        if !self.committed {
            obsolete::remove_with_retry(&self.paths);
        }
    }
}

type LoadedState = (Manifest, MemTableEntries, u64, usize);

#[allow(clippy::needless_pass_by_value)]
impl Db {
    /// Opens a Bedrock/native LevelDB directory.
    pub fn open(path: impl AsRef<Path>, options: LevelDbOpenOptions) -> Result<Self> {
        let cache_options = options.cache.normalized();
        let root = path.as_ref().to_path_buf();
        prepare_database_directory(&root, &options)?;
        let database_lock = (!options.read_only)
            .then(|| DatabaseLock::acquire(&root))
            .transpose()?;

        let (mut manifest, mut active, mut last_sequence, mut active_bytes) =
            load_existing_or_initialize(&root, &options)?;
        normalize_next_file_number(&root, &mut manifest)?;
        if !options.read_only && manifest.prev_log_number != 0 {
            recover_pending_logs(
                &root,
                &options,
                &mut manifest,
                &mut active,
                &mut last_sequence,
                &mut active_bytes,
            )?;
        }

        if !options.read_only {
            let paths = obsolete::files(&root, &manifest)?;
            obsolete::remove_with_retry(&paths);
        }

        let version = Arc::new(ReadVersion::from_manifest(&manifest));
        let next_file_number = manifest.next_file_number;
        let block_cache = Arc::new(table::NativeBlockCache::new(
            cache_options.data_capacity,
            cache_options.index_capacity,
            cache_options.file_capacity,
            cache_options.shards,
        ));
        let shared = Arc::new(SharedDb {
            root,
            options,
            inner: RwLock::new(DbInner {
                active,
                active_bytes,
                immutable: None,
                manifest,
                version,
                last_sequence,
            }),
            block_cache,
            next_file_number: AtomicU64::new(next_file_number),
            manifest_io: Mutex::new(()),
        });
        let background = if shared.options.read_only {
            None
        } else {
            Some(Background::spawn(&shared)?)
        };
        Ok(Self {
            shared,
            background,
            write_serial: Mutex::new(()),
            _database_lock: database_lock,
        })
    }

    /// Returns a point-in-time cache statistics snapshot.
    #[must_use]
    pub fn cache_stats(&self) -> DbCacheStats {
        let stats = self.shared.block_cache.stats();
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
    pub async fn open_async(path: impl AsRef<Path>, options: LevelDbOpenOptions) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        tokio::task::spawn_blocking(move || Self::open(path, options))
            .await
            .map_err(|error| LevelDbError::join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Reads a key on a blocking Tokio task.
    pub async fn get_async(self: Arc<Self>, key: Bytes) -> Result<Option<Bytes>> {
        tokio::task::spawn_blocking(move || self.get(&key))
            .await
            .map_err(|error| LevelDbError::join(error.to_string()))?
    }

    #[cfg(feature = "async")]
    /// Reads a key with explicit options on a blocking Tokio task.
    pub async fn get_with_async(
        self: Arc<Self>,
        key: Bytes,
        options: ReadOptions,
    ) -> Result<Option<Bytes>> {
        tokio::task::spawn_blocking(move || self.get_with(&key, options))
            .await
            .map_err(|error| LevelDbError::join(error.to_string()))?
    }

    /// Reads one key using default options.
    pub fn get(&self, key: &[u8]) -> Result<Option<Bytes>> {
        self.get_owned(key)
    }

    /// Reads one key as a borrowed-first value view.
    pub fn get_ref(&self, key: &[u8]) -> Result<Option<ValueRef<'static>>> {
        self.get_with_ref(key, ReadOptions::default())
    }

    /// Reads one key as shared owned bytes.
    pub fn get_owned(&self, key: &[u8]) -> Result<Option<Bytes>> {
        Ok(self
            .get_with_ref(key, ReadOptions::default())?
            .map(ValueRef::into_bytes))
    }

    /// Reads one key with explicit options.
    pub fn get_with(&self, key: &[u8], options: ReadOptions) -> Result<Option<Bytes>> {
        Ok(self.get_with_ref(key, options)?.map(ValueRef::into_bytes))
    }

    /// Reads one key while holding the metadata lock only long enough to
    /// inspect memtables and pin the immutable read version.
    pub fn get_with_ref(
        &self,
        key: &[u8],
        options: ReadOptions,
    ) -> Result<Option<ValueRef<'static>>> {
        let version = {
            let inner = read_lock(&self.shared.inner, "acquiring database read lock")?;
            if let Some(value) = inner.active.get(key) {
                return Ok(value
                    .clone()
                    .map(|value| ValueRef::from_shared(value, options.read_strategy)));
            }
            if let Some(immutable) = &inner.immutable
                && let Some(value) = immutable.get(key)
            {
                return Ok(value
                    .map(|value| ValueRef::from_shared(value, options.read_strategy)));
            }
            Arc::clone(&inner.version)
        };

        for table_meta in version.tables().iter().rev() {
            if !table_meta.may_contain_user_key(key) {
                continue;
            }
            let path = self
                .shared
                .root
                .join(Manifest::table_name(table_meta.number));
            if !path.exists() {
                continue;
            }
            match table::get_table_lookup(
                &path,
                key,
                read_checksums(&self.shared.options, &options),
                read_cache(&options, &self.shared.block_cache),
            )? {
                table::TableLookup::Value(value) => {
                    return Ok(Some(ValueRef::from_shared(value, options.read_strategy)));
                }
                table::TableLookup::Deleted => return Ok(None),
                table::TableLookup::Missing => {}
            }
        }
        Ok(None)
    }

    /// Reads many exact keys while preserving input order.
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

        let (version, mut results, mut resolved, mut unresolved) = {
            let inner = read_lock(&self.shared.inner, "acquiring database batch-read lock")?;
            let mut results = vec![None; keys.len()];
            let mut resolved = vec![false; keys.len()];
            let mut unresolved = Vec::with_capacity(keys.len());
            for (index, key) in keys.iter().enumerate() {
                if let Some(value) = inner.active.get(key.as_ref()) {
                    results[index].clone_from(value);
                    resolved[index] = true;
                } else if let Some(immutable) = &inner.immutable
                    && let Some(value) = immutable.get(key.as_ref())
                {
                    results[index] = value;
                    resolved[index] = true;
                } else {
                    unresolved.push(index);
                }
            }
            (
                Arc::clone(&inner.version),
                results,
                resolved,
                unresolved,
            )
        };

        unresolved.sort_unstable_by(|left, right| {
            keys[*left]
                .as_ref()
                .cmp(keys[*right].as_ref())
                .then_with(|| left.cmp(right))
        });

        let mut table_probes = 0_usize;
        for table_meta in version.tables().iter().rev() {
            if unresolved.is_empty() {
                break;
            }
            let table_indices = unresolved
                .iter()
                .copied()
                .filter(|index| table_meta.may_contain_user_key(keys[*index].as_ref()))
                .collect::<Vec<_>>();
            if table_indices.is_empty() {
                continue;
            }
            let path = self
                .shared
                .root
                .join(Manifest::table_name(table_meta.number));
            if !path.exists() {
                continue;
            }
            let table_keys = table_indices
                .iter()
                .map(|index| keys[*index].clone())
                .collect::<Vec<_>>();
            table_probes = table_probes.saturating_add(1);
            let lookups = table::get_table_lookups(
                &path,
                &table_keys,
                read_checksums(&self.shared.options, &options),
                read_cache(&options, &self.shared.block_cache),
            )?;
            for (input_index, lookup) in table_indices.into_iter().zip(lookups) {
                match lookup {
                    table::TableLookup::Value(value) => {
                        results[input_index] = Some(value);
                        resolved[input_index] = true;
                    }
                    table::TableLookup::Deleted => resolved[input_index] = true,
                    table::TableLookup::Missing => {}
                }
            }
            unresolved.retain(|index| !resolved[*index]);
        }
        log::debug!(
            "batch exact get complete (keys={}, hits={}, table_probes={}, elapsed_ms={})",
            keys.len(),
            results.iter().filter(|value| value.is_some()).count(),
            table_probes,
            started.elapsed().as_millis()
        );
        Ok(results)
    }

    #[cfg(feature = "async")]
    /// Reads many exact keys on a blocking Tokio task.
    pub async fn get_many_owned_async(
        self: Arc<Self>,
        keys: Vec<Bytes>,
        options: ReadOptions,
    ) -> Result<Vec<Option<Bytes>>> {
        tokio::task::spawn_blocking(move || self.get_many_owned(keys, options))
            .await
            .map_err(|error| LevelDbError::join(error.to_string()))?
    }

    /// Appends one put operation to the WAL-backed active memtable.
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

    /// Appends one delete operation to the WAL-backed active memtable.
    pub fn delete(&self, key: impl Into<Bytes>, options: WriteOptions) -> Result<()> {
        let mut batch = WriteBatch::new();
        batch.delete(key.into());
        self.write(batch, options)
    }

    /// Appends a batch without holding the metadata lock during WAL I/O.
    pub fn write(&self, mut batch: WriteBatch, options: WriteOptions) -> Result<()> {
        if self.shared.options.read_only {
            return Err(LevelDbError::ReadOnly);
        }
        if batch.is_empty() {
            return Ok(());
        }
        validate_batch(&batch)?;
        let _writer = mutex_lock(&self.write_serial, "serializing database writers")?;
        self.background()?.ensure_healthy()?;

        let (first_sequence, last_sequence, log_number) = {
            let inner = read_lock(&self.shared.inner, "reading write sequence")?;
            let first_sequence = inner.last_sequence.checked_add(1).ok_or_else(|| {
                LevelDbError::invalid_argument("write sequence number overflowed".to_string())
            })?;
            let batch_len = u64::try_from(batch.len()).map_err(|_| {
                LevelDbError::invalid_argument("write batch length overflowed".to_string())
            })?;
            let last_sequence = inner.last_sequence.checked_add(batch_len).ok_or_else(|| {
                LevelDbError::invalid_argument("write sequence number overflowed".to_string())
            })?;
            (first_sequence, last_sequence, inner.manifest.log_number)
        };
        batch.set_sequence(first_sequence);
        append_batch_to_log(&self.shared.root, log_number, &batch, options)?;

        let (should_rotate, hard_backpressure) = {
            let mut inner = write_lock(&self.shared.inner, "publishing database write")?;
            let active_bytes = inner.active_bytes;
            inner.active_bytes = apply_batch(&mut inner.active, &batch, active_bytes);
            inner.last_sequence = last_sequence;
            let limit = self.shared.options.write_buffer_size;
            let should_rotate = limit != 0 && inner.active_bytes >= limit;
            let hard_limit = limit.saturating_mul(AUTO_FLUSH_HARD_MULTIPLIER);
            let hard_backpressure = should_rotate
                && inner.immutable.is_some()
                && inner.active_bytes >= hard_limit.max(limit);
            (should_rotate, hard_backpressure)
        };

        if should_rotate {
            if hard_backpressure {
                self.background()?.wait_for_flushes()?;
            }
            self.rotate_active_memtable(false)?;
        }
        Ok(())
    }

    /// Visits visible keys. This compatibility surface no longer keeps the DB
    /// metadata lock during table I/O; the table scan implementation will be
    /// replaced by the batch cursor in the next storage-scan stage.
    pub fn for_each_key<F>(&self, options: ReadOptions, mut visitor: F) -> Result<ScanOutcome>
    where
        F: FnMut(&[u8]) -> Result<VisitorControl> + Send,
    {
        self.for_each_entry(options, |key, _value| visitor(key))
    }

    /// Visits visible key/value entries using a pinned read state.
    pub fn for_each_entry<F>(&self, options: ReadOptions, mut visitor: F) -> Result<ScanOutcome>
    where
        F: FnMut(&[u8], &Bytes) -> Result<VisitorControl> + Send,
    {
        self.scan_visible(None, &options, &mut visitor)
    }

    /// Visits visible entries as borrowed-first entry views.
    pub fn for_each_entry_ref<F>(&self, options: ReadOptions, mut visitor: F) -> Result<ScanOutcome>
    where
        F: FnMut(EntryRef<'_>) -> Result<VisitorControl> + Send,
    {
        let strategy = options.read_strategy;
        self.for_each_entry(options, |key, value| {
            visitor(EntryRef {
                key: KeyRef::new(key),
                value: ValueRef::from_shared(value.clone(), strategy),
            })
        })
    }

    /// Visits visible key/value entries beginning with `prefix`.
    pub fn for_each_prefix<F>(
        &self,
        prefix: &[u8],
        options: ReadOptions,
        mut visitor: F,
    ) -> Result<ScanOutcome>
    where
        F: FnMut(&[u8], &Bytes) -> Result<VisitorControl> + Send,
    {
        self.scan_visible(Some(prefix), &options, &mut visitor)
    }

    /// Visits visible prefix entries as borrowed-first entry views.
    pub fn for_each_prefix_ref<F>(
        &self,
        prefix: &[u8],
        options: ReadOptions,
        mut visitor: F,
    ) -> Result<ScanOutcome>
    where
        F: FnMut(EntryRef<'_>) -> Result<VisitorControl> + Send,
    {
        let strategy = options.read_strategy;
        self.for_each_prefix(prefix, options, |key, value| {
            visitor(EntryRef {
                key: KeyRef::new(key),
                value: ValueRef::from_shared(value.clone(), strategy),
            })
        })
    }

    /// Visits visible keys beginning with `prefix`.
    pub fn for_each_prefix_key<F>(
        &self,
        prefix: &[u8],
        options: ReadOptions,
        mut visitor: F,
    ) -> Result<ScanOutcome>
    where
        F: FnMut(&[u8]) -> Result<VisitorControl> + Send,
    {
        self.for_each_prefix(prefix, options, |key, _value| visitor(key))
    }

    /// Collects visible keys.
    pub fn collect_keys_owned(&self, options: ReadOptions) -> Result<Vec<Bytes>> {
        let mut keys = Vec::new();
        self.for_each_key(options, |key| {
            keys.push(Bytes::copy_from_slice(key));
            Ok(VisitorControl::Continue)
        })?;
        Ok(keys)
    }

    /// Collects visible prefix keys.
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

    /// Collects visible prefix entries.
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
    /// Compatibility alias for prefix-key collection.
    pub async fn prefix_keys_async(
        self: Arc<Self>,
        prefix: Bytes,
        options: ReadOptions,
    ) -> Result<Vec<Bytes>> {
        self.collect_prefix_keys_owned_async(prefix, options).await
    }

    /// Runs a key reduction. The current implementation preserves exact
    /// visibility semantics with one reduction partition; table-parallel cursor
    /// reduction is intentionally kept out of the metadata-lock path.
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
        let mut partition = init();
        let outcome = self.for_each_key(options, |key| visitor(&mut partition, key))?;
        Ok((outcome, vec![partition]))
    }

    /// Runs an entry reduction with one visibility-correct partition.
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
        let mut partition = init();
        let outcome = self.for_each_entry(options, |key, value| {
            visitor(&mut partition, key, value)
        })?;
        Ok((outcome, vec![partition]))
    }

    /// Materializes all visible entries into an iterator.
    pub fn iterator(&self, options: ReadOptions) -> Result<RawIterator> {
        let entries = self.collect_visible_entries(&options)?;
        Ok(RawIterator::new(&entries, &[]))
    }

    /// Materializes visible prefix entries into an iterator.
    pub fn prefix_iterator(&self, prefix: &[u8], options: ReadOptions) -> Result<PrefixIterator> {
        let entries = self.collect_visible_prefix(prefix, &options)?;
        Ok(PrefixIterator {
            inner: RawIterator::new(&entries, prefix),
        })
    }

    /// Materializes a point-in-time visible snapshot.
    pub fn snapshot(&self) -> Result<Snapshot> {
        let state = self.read_state_snapshot()?;
        let sequence = state.sequence;
        let values = Arc::new(collect_visible_from_state(
            &self.shared,
            &state,
            &ReadOptions::default(),
            None,
        )?);
        Ok(Snapshot { sequence, values })
    }

    /// Flushes the active memtable through the persistent background worker and
    /// waits until all immutable memtables queued before this call are durable.
    pub fn flush(&self) -> Result<()> {
        if self.shared.options.read_only {
            return Err(LevelDbError::ReadOnly);
        }
        let _writer = mutex_lock(&self.write_serial, "serializing explicit flush")?;
        self.background()?.ensure_healthy()?;
        self.rotate_active_memtable(true)?;
        self.background()?.wait_for_flushes()
    }

    /// Flushes outstanding writes and forces leveled compaction in the
    /// persistent background worker. Foreground reads remain lock-free with
    /// respect to SST I/O while explicit compaction runs.
    pub fn compact(&self) -> Result<()> {
        if self.shared.options.read_only {
            return Err(LevelDbError::ReadOnly);
        }
        let _writer = mutex_lock(&self.write_serial, "serializing explicit compaction")?;
        self.background()?.ensure_healthy()?;
        self.rotate_active_memtable(true)?;
        self.background()?.wait_for_flushes()?;
        self.background()?.compact(true)
    }

    /// Rebuilds a native manifest/table from readable tables and logs.
    pub fn repair(path: impl AsRef<Path>, options: LevelDbOpenOptions) -> Result<RepairReport> {
        if options.read_only {
            return Err(LevelDbError::ReadOnly);
        }
        let root = path.as_ref();
        if !root.exists() {
            if options.create_if_missing {
                fs::create_dir_all(root)
                    .map_err(|error| LevelDbError::io_at("create repair directory", root, error))?;
            } else {
                return Err(LevelDbError::not_found(root.to_path_buf()));
            }
        }
        let _database_lock = DatabaseLock::acquire(root)?;
        repair_database(root, &options)
    }

    /// Returns metadata/memtable statistics without table scans.
    pub fn stats_fast(&self) -> Result<DbStats> {
        let inner = read_lock(&self.shared.inner, "reading fast database stats")?;
        let immutable_entries = inner.immutable.as_ref().map_or(0, |immutable| {
            immutable
                .entries()
                .values()
                .filter(|value| value.is_some())
                .count()
        });
        let immutable_bytes = inner
            .immutable
            .as_ref()
            .map_or(0, |immutable| immutable.approximate_bytes());
        Ok(DbStats {
            entries: inner
                .active
                .values()
                .filter(|value| value.is_some())
                .count()
                .saturating_add(immutable_entries),
            tables: inner.version.tables().len(),
            log_number: inner.manifest.log_number,
            approximate_bytes: inner.active_bytes.saturating_add(immutable_bytes),
        })
    }

    /// Materializes visible entries to compute full statistics.
    pub fn stats_full(&self) -> Result<DbStats> {
        let entries = self.collect_visible_entries(&ReadOptions::default())?;
        let inner = read_lock(&self.shared.inner, "reading full database stats metadata")?;
        Ok(DbStats {
            entries: entries.len(),
            tables: inner.version.tables().len(),
            log_number: inner.manifest.log_number,
            approximate_bytes: approximate_entries_size(&entries),
        })
    }

    /// Alias for [`Db::stats_full`].
    pub fn stats(&self) -> Result<DbStats> {
        self.stats_full()
    }

    fn background(&self) -> Result<&Background> {
        self.background.as_ref().ok_or(LevelDbError::ReadOnly)
    }

    fn rotate_active_memtable(&self, force: bool) -> Result<()> {
        let background = self.background()?;
        if !force && self.shared.options.write_buffer_size == 0 {
            return Ok(());
        }
        {
            let inner = read_lock(&self.shared.inner, "checking memtable rotation")?;
            if inner.active.is_empty() || inner.immutable.is_some() {
                return Ok(());
            }
            if !force
                && inner.active_bytes < self.shared.options.write_buffer_size
            {
                return Ok(());
            }
        }

        let new_log_number = allocate_file_number(&self.shared)?;
        let new_log_path = self
            .shared
            .root
            .join(Manifest::log_name(new_log_number));
        create_empty_wal(&new_log_path)?;

        let _manifest_guard = mutex_lock(&self.shared.manifest_io, "serializing manifest update")?;
        let next_manifest = {
            let inner = read_lock(&self.shared.inner, "staging memtable rotation")?;
            if inner.active.is_empty() || inner.immutable.is_some() {
                let _ = fs::remove_file(&new_log_path);
                return Ok(());
            }
            let mut manifest = inner.manifest.clone();
            manifest.prev_log_number = manifest.log_number;
            manifest.log_number = new_log_number;
            manifest.last_sequence = inner.last_sequence;
            manifest.next_file_number = self.shared.next_file_number.load(Ordering::Relaxed);
            manifest
        };
        next_manifest.store(&self.shared.root)?;

        let immutable = {
            let mut inner = write_lock(&self.shared.inner, "publishing immutable memtable")?;
            let old_log_number = inner.manifest.log_number;
            let entries = std::mem::take(&mut inner.active);
            let bytes = std::mem::replace(&mut inner.active_bytes, 0);
            let immutable = Arc::new(ImmutableMemTable::new(
                entries,
                inner.last_sequence,
                old_log_number,
                bytes,
            ));
            inner.immutable = Some(Arc::clone(&immutable));
            inner.manifest = next_manifest;
            immutable
        };
        background.enqueue_flush(immutable)
    }

    fn read_state_snapshot(&self) -> Result<ReadState> {
        let inner = read_lock(&self.shared.inner, "snapshotting database read state")?;
        Ok(ReadState {
            active: Arc::new(inner.active.clone()),
            immutable: inner.immutable.as_ref().map(Arc::clone),
            version: Arc::clone(&inner.version),
            sequence: inner.last_sequence,
        })
    }

    fn scan_visible<F>(
        &self,
        prefix: Option<&[u8]>,
        options: &ReadOptions,
        visitor: &mut F,
    ) -> Result<ScanOutcome>
    where
        F: FnMut(&[u8], &Bytes) -> Result<VisitorControl> + Send,
    {
        let started = Instant::now();
        let state = self.read_state_snapshot()?;
        let mut seen = HashSet::<Vec<u8>>::with_capacity(
            state
                .active
                .len()
                .saturating_add(state.immutable.as_ref().map_or(0, |table| table.entries().len())),
        );
        seen.extend(state.active.keys().cloned());
        if let Some(immutable) = &state.immutable {
            seen.extend(immutable.entries().keys().cloned());
        }

        let verify_checksums = read_checksums(&self.shared.options, options);
        let mut outcome = ScanOutcome::empty();
        outcome.worker_threads = 1;
        for table_meta in state.version.tables().iter().rev() {
            check_scan_cancelled(options)?;
            let path = self
                .shared
                .root
                .join(Manifest::table_name(table_meta.number));
            if !path.exists() {
                continue;
            }
            let table_outcome = table::for_each_table_lookup(
                &path,
                verify_checksums,
                read_cache(options, &self.shared.block_cache),
                |key, value| {
                    if prefix.is_some_and(|prefix| !key.starts_with(prefix))
                        || !seen.insert(key.to_vec())
                    {
                        return Ok(VisitorControl::Continue);
                    }
                    if let Some(value) = value {
                        if visitor(key, value)? == VisitorControl::Stop {
                            return Ok(VisitorControl::Stop);
                        }
                    }
                    Ok(VisitorControl::Continue)
                },
            )?;
            outcome.merge(table_outcome);
            if outcome.stopped {
                return Ok(outcome);
            }
        }

        if let Some(immutable) = &state.immutable {
            for (key, value) in immutable.entries() {
                check_scan_cancelled(options)?;
                if state.active.contains_key(key)
                    || prefix.is_some_and(|prefix| !key.starts_with(prefix))
                {
                    continue;
                }
                if let Some(value) = value {
                    outcome.record(value.len());
                    if visitor(key, value)? == VisitorControl::Stop {
                        outcome.stopped = true;
                        return Ok(outcome);
                    }
                }
            }
        }
        for (key, value) in state.active.iter() {
            check_scan_cancelled(options)?;
            if prefix.is_some_and(|prefix| !key.starts_with(prefix)) {
                continue;
            }
            if let Some(value) = value {
                outcome.record(value.len());
                if visitor(key, value)? == VisitorControl::Stop {
                    outcome.stopped = true;
                    return Ok(outcome);
                }
            }
        }
        log::debug!(
            "pinned scan complete (visited={}, tables={}, elapsed_ms={})",
            outcome.visited,
            outcome.tables_scanned,
            started.elapsed().as_millis()
        );
        Ok(outcome)
    }

    fn collect_visible_entries(&self, options: &ReadOptions) -> Result<BTreeMap<Vec<u8>, Bytes>> {
        let mut entries = BTreeMap::new();
        self.for_each_entry(options.clone(), |key, value| {
            entries.insert(key.to_vec(), value.clone());
            Ok(VisitorControl::Continue)
        })?;
        Ok(entries)
    }

    fn collect_visible_prefix(
        &self,
        prefix: &[u8],
        options: &ReadOptions,
    ) -> Result<BTreeMap<Vec<u8>, Bytes>> {
        let mut entries = BTreeMap::new();
        self.for_each_prefix(prefix, options.clone(), |key, value| {
            entries.insert(key.to_vec(), value.clone());
            Ok(VisitorControl::Continue)
        })?;
        Ok(entries)
    }
}

impl Drop for Db {
    fn drop(&mut self) {
        if let Some(background) = self.background.as_mut() {
            background.shutdown();
        }
    }
}

impl Background {
    fn spawn(shared: &Arc<SharedDb>) -> Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(BACKGROUND_QUEUE_DEPTH);
        let status = Arc::new((Mutex::new(BackgroundStatus::default()), Condvar::new()));
        let worker_status = Arc::clone(&status);
        let weak = Arc::downgrade(shared);
        let worker = thread::Builder::new()
            .name("bedrock-leveldb-background".to_string())
            .stack_size(BACKGROUND_STACK_BYTES)
            .spawn(move || background_worker(weak, receiver, worker_status))
            .map_err(|error| {
                LevelDbError::io("spawn background maintenance worker", None, error)
            })?;
        Ok(Self {
            sender,
            status,
            worker: Some(worker),
        })
    }

    fn ensure_healthy(&self) -> Result<()> {
        let (lock, _) = self.status.as_ref();
        let status = mutex_lock(lock, "reading background maintenance status")?;
        if let Some(error) = &status.fatal_error {
            return Err(LevelDbError::corruption(format!(
                "background flush failed: {error}"
            )));
        }
        Ok(())
    }

    fn enqueue_flush(&self, immutable: Arc<ImmutableMemTable>) -> Result<()> {
        self.ensure_healthy()?;
        let (lock, cv) = self.status.as_ref();
        {
            let mut status = mutex_lock(lock, "queuing background flush")?;
            status.pending_flushes = status.pending_flushes.saturating_add(1);
        }
        if self.sender.send(BackgroundCommand::Flush(immutable)).is_err() {
            let mut status = mutex_lock(lock, "rolling back background flush queue")?;
            status.pending_flushes = status.pending_flushes.saturating_sub(1);
            cv.notify_all();
            return Err(LevelDbError::corruption(
                "background maintenance worker is unavailable".to_string(),
            ));
        }
        Ok(())
    }

    fn wait_for_flushes(&self) -> Result<()> {
        let (lock, cv) = self.status.as_ref();
        let mut status = mutex_lock(lock, "waiting for background flush")?;
        while status.pending_flushes != 0 {
            status = cv
                .wait(status)
                .map_err(|_| LevelDbError::lock_poisoned("waiting for background flush"))?;
        }
        if let Some(error) = &status.fatal_error {
            return Err(LevelDbError::corruption(format!(
                "background flush failed: {error}"
            )));
        }
        Ok(())
    }

    fn compact(&self, force: bool) -> Result<()> {
        self.ensure_healthy()?;
        let (done_sender, done_receiver) = mpsc::channel();
        self.sender
            .send(BackgroundCommand::Compact {
                force,
                done: done_sender,
            })
            .map_err(|_| {
                LevelDbError::corruption("background maintenance worker is unavailable".to_string())
            })?;
        done_receiver.recv().map_err(|_| {
            LevelDbError::corruption("background compaction result channel closed".to_string())
        })?
    }

    fn shutdown(&mut self) {
        let _ = self.sender.send(BackgroundCommand::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn background_worker(
    shared: Weak<SharedDb>,
    receiver: Receiver<BackgroundCommand>,
    status: Arc<(Mutex<BackgroundStatus>, Condvar)>,
) {
    while let Ok(command) = receiver.recv() {
        match command {
            BackgroundCommand::Flush(immutable) => {
                let result = shared
                    .upgrade()
                    .ok_or_else(|| {
                        LevelDbError::corruption("database closed during background flush")
                    })
                    .and_then(|shared| flush_immutable_memtable(&shared, &immutable));
                let (lock, cv) = status.as_ref();
                match lock.lock() {
                    Ok(mut state) => {
                        state.pending_flushes = state.pending_flushes.saturating_sub(1);
                        if let Err(error) = &result {
                            state.fatal_error = Some(error.to_string());
                        }
                        cv.notify_all();
                    }
                    Err(poisoned) => {
                        let mut state = poisoned.into_inner();
                        state.pending_flushes = state.pending_flushes.saturating_sub(1);
                        if let Err(error) = &result {
                            state.fatal_error = Some(error.to_string());
                        }
                        cv.notify_all();
                    }
                }
                if result.is_ok()
                    && let Some(shared) = shared.upgrade()
                    && let Err(error) = compact_levels(&shared, false)
                {
                    log::warn!("automatic LevelDB compaction failed: {error}");
                }
            }
            BackgroundCommand::Compact { force, done } => {
                let result = shared
                    .upgrade()
                    .ok_or_else(|| {
                        LevelDbError::corruption("database closed during background compaction")
                    })
                    .and_then(|shared| compact_levels(&shared, force));
                let _ = done.send(result);
            }
            BackgroundCommand::Shutdown => break,
        }
    }
}

fn flush_immutable_memtable(
    shared: &Arc<SharedDb>,
    immutable: &Arc<ImmutableMemTable>,
) -> Result<()> {
    if immutable.is_empty() {
        return install_empty_immutable(shared, immutable);
    }
    let table_number = allocate_file_number(shared)?;
    let table_path = shared.root.join(Manifest::table_name(table_number));
    let mut writer = NativeTableWriter::create(
        &table_path,
        immutable.last_sequence(),
        shared.options.compression_policy,
    )?;
    for (key, value) in immutable.entries() {
        writer.push(key, value.as_deref())?;
    }
    let written = writer.finish()?;
    let table_meta = written_table_meta(table_number, 0, written);

    let _manifest_guard = mutex_lock(&shared.manifest_io, "installing flushed memtable")?;
    let next_manifest = {
        let inner = read_lock(&shared.inner, "staging flushed memtable")?;
        let Some(current) = &inner.immutable else {
            obsolete::remove_with_retry(std::slice::from_ref(&table_path));
            return Err(LevelDbError::corruption(
                "immutable memtable disappeared before flush installation".to_string(),
            ));
        };
        if !Arc::ptr_eq(current, immutable) {
            obsolete::remove_with_retry(std::slice::from_ref(&table_path));
            return Err(LevelDbError::corruption(
                "immutable memtable generation changed during flush".to_string(),
            ));
        }
        let mut manifest = inner.manifest.clone();
        manifest.prev_log_number = 0;
        manifest.last_sequence = inner.last_sequence;
        manifest.next_file_number = shared.next_file_number.load(Ordering::Relaxed);
        manifest.table_numbers.push(table_number);
        manifest.table_numbers.sort_unstable();
        manifest.table_numbers.dedup();
        manifest.table_files.push(table_meta.clone());
        manifest.table_files.sort_by_key(|table| table.number);
        manifest.table_files.dedup_by_key(|table| table.number);
        manifest
    };
    if let Err(error) = next_manifest.store(&shared.root) {
        obsolete::remove_with_retry(std::slice::from_ref(&table_path));
        return Err(error);
    }
    let old_log_number = immutable.log_number();
    {
        let mut inner = write_lock(&shared.inner, "publishing flushed memtable")?;
        inner.manifest = next_manifest;
        inner.version = Arc::new(ReadVersion::from_manifest(&inner.manifest));
        inner.immutable = None;
    }
    let old_log = shared.root.join(Manifest::log_name(old_log_number));
    obsolete::remove_with_retry(std::slice::from_ref(&old_log));
    Ok(())
}

fn install_empty_immutable(
    shared: &Arc<SharedDb>,
    immutable: &Arc<ImmutableMemTable>,
) -> Result<()> {
    let _manifest_guard = mutex_lock(&shared.manifest_io, "installing empty immutable memtable")?;
    let next_manifest = {
        let inner = read_lock(&shared.inner, "staging empty immutable memtable")?;
        let mut manifest = inner.manifest.clone();
        manifest.prev_log_number = 0;
        manifest.last_sequence = inner.last_sequence;
        manifest.next_file_number = shared.next_file_number.load(Ordering::Relaxed);
        manifest
    };
    next_manifest.store(&shared.root)?;
    {
        let mut inner = write_lock(&shared.inner, "publishing empty immutable memtable")?;
        if inner
            .immutable
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, immutable))
        {
            inner.manifest = next_manifest;
            inner.immutable = None;
        }
    }
    let old_log = shared.root.join(Manifest::log_name(immutable.log_number()));
    obsolete::remove_with_retry(std::slice::from_ref(&old_log));
    Ok(())
}

fn compact_levels(shared: &Arc<SharedDb>, force: bool) -> Result<()> {
    loop {
        let (plan, version_pin) = {
            let inner = read_lock(&shared.inner, "planning background compaction")?;
            let Some(plan) = compaction::plan(&inner.manifest, force) else {
                return Ok(());
            };
            (plan, Arc::clone(&inner.version))
        };
        compact_level(shared, &plan, &version_pin)?;
    }
}

fn compact_level(
    shared: &Arc<SharedDb>,
    plan: &compaction::CompactionPlan,
    _version_pin: &Arc<ReadVersion>,
) -> Result<()> {
    let mut outputs = write_compaction_outputs(shared, plan)?;
    let input_numbers = plan.input_numbers();
    let input_paths = plan
        .inputs
        .iter()
        .map(|table| shared.root.join(Manifest::table_name(table.number)))
        .collect::<Vec<_>>();

    let _manifest_guard = mutex_lock(&shared.manifest_io, "installing background compaction")?;
    let next_manifest = {
        let inner = read_lock(&shared.inner, "staging background compaction")?;
        if !input_numbers
            .iter()
            .all(|number| inner.manifest.table_numbers.binary_search(number).is_ok())
        {
            return Err(LevelDbError::corruption(
                "compaction inputs changed before installation".to_string(),
            ));
        }
        let mut manifest = inner.manifest.clone();
        manifest
            .table_numbers
            .retain(|number| !input_numbers.contains(number));
        manifest
            .table_files
            .retain(|table| !input_numbers.contains(&table.number));
        manifest
            .table_numbers
            .extend(outputs.tables.iter().map(|table| table.number));
        manifest.table_files.extend(outputs.tables.iter().cloned());
        manifest.table_numbers.sort_unstable();
        manifest.table_numbers.dedup();
        manifest.table_files.sort_by_key(|table| table.number);
        manifest.table_files.dedup_by_key(|table| table.number);
        manifest.next_file_number = shared.next_file_number.load(Ordering::Relaxed);
        manifest.last_sequence = inner.last_sequence;
        manifest
    };
    next_manifest.store(&shared.root)?;
    outputs.commit();

    shared.block_cache.invalidate_paths(&input_paths);
    {
        let mut inner = write_lock(&shared.inner, "publishing background compaction")?;
        let old_version = Arc::clone(&inner.version);
        old_version.retire_paths(&input_paths);
        inner.manifest = next_manifest;
        inner.version = Arc::new(ReadVersion::from_manifest(&inner.manifest));
    }
    Ok(())
}

fn write_compaction_outputs(
    shared: &Arc<SharedDb>,
    plan: &compaction::CompactionPlan,
) -> Result<PendingCompactionOutputs> {
    let sequence = {
        let inner = read_lock(&shared.inner, "reading compaction sequence")?;
        inner.last_sequence
    };
    let mut outputs = PendingCompactionOutputs::new();
    let mut current: Option<(u64, PathBuf, NativeTableWriter)> = None;

    let merge_result = compaction::merge_into(
        &shared.root,
        plan,
        shared.options.paranoid_checks,
        |key, value| {
            let should_finish = current.as_ref().is_some_and(|(_, _, writer)| {
                !writer.is_empty()
                    && writer.estimated_size() >= compaction::TARGET_OUTPUT_FILE_BYTES
            });
            if should_finish {
                finish_compaction_writer(&mut current, plan.output_level, &mut outputs)?;
            }
            if current.is_none() {
                let number = allocate_file_number(shared)?;
                let path = shared.root.join(Manifest::table_name(number));
                let writer = NativeTableWriter::create(
                    &path,
                    sequence,
                    shared.options.compression_policy,
                )?;
                current = Some((number, path, writer));
            }
            let (_, _, writer) = current
                .as_mut()
                .expect("compaction writer is initialized before append");
            writer.push(key, value.map(Bytes::as_ref))
        },
    );
    if let Err(error) = merge_result {
        return Err(error);
    }
    finish_compaction_writer(&mut current, plan.output_level, &mut outputs)?;
    Ok(outputs)
}

fn finish_compaction_writer(
    current: &mut Option<(u64, PathBuf, NativeTableWriter)>,
    level: u32,
    outputs: &mut PendingCompactionOutputs,
) -> Result<()> {
    let Some((number, path, writer)) = current.take() else {
        return Ok(());
    };
    let written = writer.finish()?;
    outputs.push(path, written_table_meta(number, level, written));
    Ok(())
}

fn written_table_meta(number: u64, level: u32, written: WrittenNativeTable) -> TableFileMeta {
    TableFileMeta::native(
        number,
        level,
        written.file_size,
        written.smallest_internal_key,
        written.largest_internal_key,
    )
}

fn collect_visible_from_state(
    shared: &SharedDb,
    state: &ReadState,
    options: &ReadOptions,
    prefix: Option<&[u8]>,
) -> Result<BTreeMap<Vec<u8>, Bytes>> {
    let mut values = BTreeMap::new();
    let mut seen = HashSet::new();
    seen.extend(state.active.keys().cloned());
    if let Some(immutable) = &state.immutable {
        seen.extend(immutable.entries().keys().cloned());
    }
    for table_meta in state.version.tables().iter().rev() {
        let path = shared.root.join(Manifest::table_name(table_meta.number));
        if !path.exists() {
            continue;
        }
        table::for_each_table_lookup(
            &path,
            read_checksums(&shared.options, options),
            read_cache(options, &shared.block_cache),
            |key, value| {
                if prefix.is_some_and(|prefix| !key.starts_with(prefix))
                    || !seen.insert(key.to_vec())
                {
                    return Ok(VisitorControl::Continue);
                }
                if let Some(value) = value {
                    values.insert(key.to_vec(), value.clone());
                }
                Ok(VisitorControl::Continue)
            },
        )?;
    }
    if let Some(immutable) = &state.immutable {
        for (key, value) in immutable.entries() {
            if state.active.contains_key(key)
                || prefix.is_some_and(|prefix| !key.starts_with(prefix))
            {
                continue;
            }
            if let Some(value) = value {
                values.insert(key.clone(), value.clone());
            }
        }
    }
    for (key, value) in state.active.iter() {
        if prefix.is_some_and(|prefix| !key.starts_with(prefix)) {
            continue;
        }
        match value {
            Some(value) => {
                values.insert(key.clone(), value.clone());
            }
            None => {
                values.remove(key);
            }
        }
    }
    Ok(values)
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

fn prepare_database_directory(root: &Path, options: &LevelDbOpenOptions) -> Result<()> {
    if root.exists() {
        if options.error_if_exists {
            let mut entries = fs::read_dir(root)
                .map_err(|error| LevelDbError::io_at("read database directory", root, error))?;
            if entries
                .next()
                .transpose()
                .map_err(|error| LevelDbError::io_at("read database directory", root, error))?
                .is_some()
            {
                return Err(LevelDbError::already_exists(root.to_path_buf()));
            }
        }
    } else if options.read_only {
        return Err(LevelDbError::not_found(root.to_path_buf()));
    } else if options.create_if_missing {
        fs::create_dir_all(root)
            .map_err(|error| LevelDbError::io_at("create database directory", root, error))?;
    } else {
        return Err(LevelDbError::not_found(root.to_path_buf()));
    }
    Ok(())
}

fn load_existing_or_initialize(root: &Path, options: &LevelDbOpenOptions) -> Result<LoadedState> {
    match Manifest::load(root) {
        Ok(manifest) => {
            let mut active = BTreeMap::new();
            let mut last_sequence = manifest.last_sequence;
            let mut active_bytes = 0_usize;
            let mut log_numbers = [manifest.prev_log_number, manifest.log_number]
                .into_iter()
                .filter(|number| *number != 0)
                .collect::<Vec<_>>();
            log_numbers.sort_unstable();
            log_numbers.dedup();
            for log_number in log_numbers {
                let path = root.join(Manifest::log_name(log_number));
                if !path.exists() {
                    continue;
                }
                let mut file = File::open(&path)
                    .map_err(|error| LevelDbError::io_at("open WAL", &path, error))?;
                wal::for_each_record(&mut file, options.paranoid_checks, |record| {
                    let batch = WriteBatch::decode(record).map_err(|error| {
                        LevelDbError::corruption_at(
                            &path,
                            format!("failed to decode write batch: {error}"),
                        )
                    })?;
                    let batch_len = u64::try_from(batch.len()).map_err(|_| {
                        LevelDbError::corruption_at(&path, "write batch length overflow")
                    })?;
                    let batch_last_sequence = if batch_len == 0 {
                        batch.sequence()
                    } else {
                        batch.sequence().checked_add(batch_len - 1).ok_or_else(|| {
                            LevelDbError::corruption_at(&path, "write batch sequence overflow")
                        })?
                    };
                    last_sequence = last_sequence.max(batch_last_sequence);
                    active_bytes = apply_batch(&mut active, &batch, active_bytes);
                    Ok(())
                })?;
            }
            Ok((manifest, active, last_sequence, active_bytes))
        }
        Err(error)
            if error.kind() == ErrorKind::NotFound
                && options.create_if_missing
                && !options.read_only =>
        {
            let manifest = Manifest::default();
            manifest.store(root)?;
            create_empty_wal(&root.join(Manifest::log_name(manifest.log_number)))?;
            Ok((manifest, BTreeMap::new(), 0, 0))
        }
        Err(error) => Err(error),
    }
}

fn normalize_next_file_number(root: &Path, manifest: &mut Manifest) -> Result<()> {
    let highest = fs::read_dir(root)
        .map_err(|error| LevelDbError::io_at("scan database file numbers", root, error))?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| parse_file_number(&entry.path()))
        .max()
        .unwrap_or(0);
    manifest.next_file_number = manifest
        .next_file_number
        .max(highest.saturating_add(1))
        .max(2);
    Ok(())
}

fn recover_pending_logs(
    root: &Path,
    options: &LevelDbOpenOptions,
    manifest: &mut Manifest,
    active: &mut MemTableEntries,
    last_sequence: &mut u64,
    active_bytes: &mut usize,
) -> Result<()> {
    let old_logs = [manifest.prev_log_number, manifest.log_number]
        .into_iter()
        .filter(|number| *number != 0)
        .map(|number| root.join(Manifest::log_name(number)))
        .collect::<Vec<_>>();
    let table_number = if active.is_empty() {
        None
    } else {
        let number = take_manifest_file_number(manifest)?;
        let path = root.join(Manifest::table_name(number));
        let mut writer = NativeTableWriter::create(&path, *last_sequence, options.compression_policy)?;
        for (key, value) in active.iter() {
            writer.push(key, value.as_deref())?;
        }
        let written = writer.finish()?;
        let meta = written_table_meta(number, 0, written);
        manifest.table_numbers.push(number);
        manifest.table_files.push(meta);
        Some(number)
    };
    let new_log_number = take_manifest_file_number(manifest)?;
    create_empty_wal(&root.join(Manifest::log_name(new_log_number)))?;
    manifest.log_number = new_log_number;
    manifest.prev_log_number = 0;
    manifest.last_sequence = *last_sequence;
    manifest.table_numbers.sort_unstable();
    manifest.table_numbers.dedup();
    manifest.table_files.sort_by_key(|table| table.number);
    manifest.table_files.dedup_by_key(|table| table.number);
    if let Err(error) = manifest.store(root) {
        if let Some(number) = table_number {
            obsolete::remove_with_retry(std::slice::from_ref(
                &root.join(Manifest::table_name(number)),
            ));
        }
        return Err(error);
    }
    obsolete::remove_with_retry(&old_logs);
    active.clear();
    *active_bytes = 0;
    Ok(())
}

fn take_manifest_file_number(manifest: &mut Manifest) -> Result<u64> {
    let number = manifest.next_file_number;
    manifest.next_file_number = manifest.next_file_number.checked_add(1).ok_or_else(|| {
        LevelDbError::invalid_argument("database file number overflowed".to_string())
    })?;
    Ok(number)
}

fn allocate_file_number(shared: &SharedDb) -> Result<u64> {
    shared
        .next_file_number
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |number| {
            number.checked_add(1)
        })
        .map_err(|_| LevelDbError::invalid_argument("database file number overflowed".to_string()))
}

fn append_batch_to_log(
    root: &Path,
    log_number: u64,
    batch: &WriteBatch,
    options: WriteOptions,
) -> Result<()> {
    let path = root.join(Manifest::log_name(log_number));
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| LevelDbError::io_at("open WAL for append", &path, error))?;
    WAL_ENCODE_SCRATCH.with(|scratch| -> Result<()> {
        let mut scratch = scratch.borrow_mut();
        batch.encode_into(&mut scratch)?;
        wal::append_record(&mut file, &scratch)
    })?;
    if options.sync {
        file.sync_data()
            .map_err(|error| LevelDbError::io_at("sync WAL", &path, error))?;
    }
    Ok(())
}

fn create_empty_wal(path: &Path) -> Result<()> {
    let file = File::create(path).map_err(|error| LevelDbError::io_at("create WAL", path, error))?;
    file.sync_all()
        .map_err(|error| LevelDbError::io_at("sync WAL", path, error))
}

fn apply_batch(
    active: &mut MemTableEntries,
    batch: &WriteBatch,
    mut approximate_bytes: usize,
) -> usize {
    for op in batch.ops() {
        match op {
            WriteOp::Put { key, value } => {
                let key_size = key.len();
                let value_size = value.len();
                if let Some(old_value) = active.insert(key.to_vec(), Some(value.clone())) {
                    approximate_bytes = approximate_bytes.saturating_sub(
                        key_size.saturating_add(old_value.as_ref().map_or(0, Bytes::len)),
                    );
                }
                approximate_bytes =
                    approximate_bytes.saturating_add(key_size.saturating_add(value_size));
            }
            WriteOp::Delete { key } => {
                if let Some(old_value) = active.insert(key.to_vec(), None) {
                    approximate_bytes = approximate_bytes.saturating_sub(
                        key.len()
                            .saturating_add(old_value.as_ref().map_or(0, Bytes::len)),
                    );
                }
                approximate_bytes = approximate_bytes.saturating_add(key.len());
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

fn read_checksums(open: &LevelDbOpenOptions, read: &ReadOptions) -> bool {
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

fn approximate_entries_size(values: &BTreeMap<Vec<u8>, Bytes>) -> usize {
    values
        .iter()
        .map(|(key, value)| key.len().saturating_add(value.len()))
        .sum()
}

fn parse_file_number(path: &Path) -> Option<u64> {
    path.file_stem()?.to_str()?.parse().ok()
}

fn repair_database(root: &Path, options: &LevelDbOpenOptions) -> Result<RepairReport> {
    let mut report = RepairReport::default();
    let source_manifest = match Manifest::load(root) {
        Ok(manifest) => manifest,
        Err(error) if error.kind() == ErrorKind::NotFound && options.create_if_missing => {
            Manifest::default()
        }
        Err(error) => return Err(error),
    };
    let mut values = BTreeMap::new();
    let mut last_sequence = source_manifest.last_sequence;
    let paths = sorted_database_paths(root)?;

    for path in paths
        .iter()
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("ldb"))
    {
        match table::read_table_lookups(path, false).and_then(|entries| {
            table::read_table_max_sequence(path, false).map(|sequence| (entries, sequence))
        }) {
            Ok((entries, sequence)) => {
                for (key, lookup) in entries {
                    match lookup {
                        table::TableLookup::Value(value) => {
                            values.insert(key, value);
                        }
                        table::TableLookup::Deleted => {
                            values.remove(&key);
                        }
                        table::TableLookup::Missing => {}
                    }
                }
                last_sequence = last_sequence.max(sequence);
                report.recovered_tables = report.recovered_tables.saturating_add(1);
            }
            Err(error) => {
                log::warn!("dropping unreadable table during repair: {} ({error})", path.display());
                report.dropped_files = report.dropped_files.saturating_add(1);
            }
        }
    }

    for path in paths
        .iter()
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("log"))
    {
        match File::open(path) {
            Ok(mut file) => {
                let result = wal::for_each_record(&mut file, false, |record| {
                    let batch = WriteBatch::decode(record)?;
                    let batch_len = u64::try_from(batch.len()).unwrap_or(u64::MAX);
                    last_sequence = last_sequence.max(
                        batch
                            .sequence()
                            .saturating_add(batch_len.saturating_sub(1)),
                    );
                    apply_batch_to_values(&mut values, &batch);
                    report.recovered_log_records = report.recovered_log_records.saturating_add(1);
                    Ok(())
                });
                if let Err(error) = result {
                    log::warn!("dropping unreadable WAL during repair: {} ({error})", path.display());
                    report.dropped_files = report.dropped_files.saturating_add(1);
                }
            }
            Err(error) => {
                log::warn!("dropping unreadable WAL during repair: {} ({error})", path.display());
                report.dropped_files = report.dropped_files.saturating_add(1);
            }
        }
    }

    write_recovered_state(root, &values, last_sequence, options.compression_policy)?;
    Ok(report)
}

fn write_recovered_state(
    root: &Path,
    values: &BTreeMap<Vec<u8>, Bytes>,
    last_sequence: u64,
    compression: CompressionPolicy,
) -> Result<()> {
    let highest = sorted_database_paths(root)?
        .iter()
        .filter_map(|path| parse_file_number(path))
        .max()
        .unwrap_or(1);
    let table_number = highest.saturating_add(1);
    let log_number = highest.saturating_add(2);
    let mut table_files = Vec::new();
    if !values.is_empty() {
        let path = root.join(Manifest::table_name(table_number));
        let mut writer = NativeTableWriter::create(&path, last_sequence, compression)?;
        for (key, value) in values {
            writer.push(key, Some(value))?;
        }
        let written = writer.finish()?;
        table_files.push(written_table_meta(table_number, 0, written));
    }
    create_empty_wal(&root.join(Manifest::log_name(log_number)))?;
    let table_numbers = table_files.iter().map(|table| table.number).collect();
    let manifest = Manifest {
        next_file_number: log_number.saturating_add(1),
        log_number,
        prev_log_number: 0,
        last_sequence,
        table_numbers,
        table_files,
    };
    manifest.store(root)
}

fn apply_batch_to_values(values: &mut BTreeMap<Vec<u8>, Bytes>, batch: &WriteBatch) {
    for op in batch.ops() {
        match op {
            WriteOp::Put { key, value } => {
                values.insert(key.to_vec(), value.clone());
            }
            WriteOp::Delete { key } => {
                values.remove(key.as_ref());
            }
        }
    }
}

fn sorted_database_paths(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = fs::read_dir(root)
        .map_err(|error| LevelDbError::io_at("read database directory", root, error))?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| LevelDbError::io_at("read database entry", root, error))
        })
        .collect::<Result<Vec<_>>>()?;
    paths.sort_by_key(|path| parse_file_number(path));
    Ok(paths)
}

fn read_lock<'a, T>(
    lock: &'a RwLock<T>,
    operation: &'static str,
) -> Result<std::sync::RwLockReadGuard<'a, T>> {
    lock.read()
        .map_err(|_| LevelDbError::lock_poisoned(operation))
}

fn write_lock<'a, T>(
    lock: &'a RwLock<T>,
    operation: &'static str,
) -> Result<std::sync::RwLockWriteGuard<'a, T>> {
    lock.write()
        .map_err(|_| LevelDbError::lock_poisoned(operation))
}

fn mutex_lock<'a, T>(
    lock: &'a Mutex<T>,
    operation: &'static str,
) -> Result<std::sync::MutexGuard<'a, T>> {
    lock.lock()
        .map_err(|_| LevelDbError::lock_poisoned(operation))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::ScanCancelFlag;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "bedrock-leveldb-v2-{name}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ))
    }

    #[test]
    fn active_and_immutable_are_visible_during_background_flush() {
        let path = temp_dir("immutable-visible");
        let db = Db::open(
            &path,
            LevelDbOpenOptions {
                compression_policy: CompressionPolicy::None,
                write_buffer_size: 32,
                ..LevelDbOpenOptions::default()
            },
        )
        .expect("open");
        db.put(
            Bytes::from_static(b"old"),
            Bytes::from_static(b"value-old-value-old"),
            WriteOptions::default(),
        )
        .expect("put old");
        db.put(
            Bytes::from_static(b"new"),
            Bytes::from_static(b"value-new"),
            WriteOptions::default(),
        )
        .expect("put new");
        assert_eq!(db.get(b"old").expect("get old"), Some(Bytes::from_static(b"value-old-value-old")));
        assert_eq!(db.get(b"new").expect("get new"), Some(Bytes::from_static(b"value-new")));
        db.flush().expect("flush");
        drop(db);
        fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn read_version_survives_compaction_install() {
        let path = temp_dir("version-pin");
        let db = Db::open(
            &path,
            LevelDbOpenOptions {
                compression_policy: CompressionPolicy::None,
                write_buffer_size: 0,
                ..LevelDbOpenOptions::default()
            },
        )
        .expect("open");
        for round in 0..6_u8 {
            db.put(
                Bytes::from_static(b"key"),
                Bytes::from(vec![round; 128]),
                WriteOptions::default(),
            )
            .expect("put");
            db.flush().expect("flush");
        }
        db.compact().expect("compact");
        assert_eq!(db.get(b"key").expect("get").expect("value")[0], 5);
        drop(db);
        fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn wal_reopens_without_flushing_active_memtable() {
        let path = temp_dir("wal-reopen");
        let options = LevelDbOpenOptions {
            compression_policy: CompressionPolicy::None,
            write_buffer_size: 0,
            ..LevelDbOpenOptions::default()
        };
        {
            let db = Db::open(&path, options.clone()).expect("open");
            db.put(b"key".as_slice(), b"value".as_slice(), WriteOptions::default())
                .expect("put");
        }
        let db = Db::open(&path, options).expect("reopen");
        assert_eq!(db.get(b"key").expect("get"), Some(Bytes::from_static(b"value")));
        drop(db);
        fs::remove_dir_all(path).expect("cleanup");
    }

    #[test]
    fn scan_cancel_is_typed() {
        let path = temp_dir("cancel");
        let db = Db::open(&path, LevelDbOpenOptions::default()).expect("open");
        db.put(b"a".as_slice(), b"one".as_slice(), WriteOptions::default())
            .expect("put");
        let cancel = ScanCancelFlag::new();
        cancel.cancel();
        let error = db
            .for_each_key(
                ReadOptions {
                    cancel: Some(cancel),
                    ..ReadOptions::default()
                },
                |_key| Ok(VisitorControl::Continue),
            )
            .expect_err("cancelled");
        assert_eq!(error.kind(), ErrorKind::Cancelled);
        drop(db);
        fs::remove_dir_all(path).expect("cleanup");
    }
}
