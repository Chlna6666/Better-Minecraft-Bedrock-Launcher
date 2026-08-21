mod core {
    include!("db_core.rs");
    include!("db_scan.rs");
}

pub use core::{
    DbCacheStats, DbStats, EntryRef, KeyRef, PrefixIterator, RawIterator, RepairReport, Snapshot,
    ValueRef,
};

use crate::batch::WriteBatch;
use crate::error::{LevelDbError, Result};
use crate::options::{
    LevelDbOpenOptions, ReadOptions, ReadStrategy, ScanOutcome, VisitorControl, WriteOptions,
};
use bytes::Bytes;
use std::path::Path;
use std::sync::Arc;

/// Public database handle.
///
/// Point reads, WAL writes, snapshots and maintenance delegate to the stable core.
/// Visibility scans use borrowed block buffers, ordered newest-wins merging and
/// bounded SST block-range workers.
pub struct Db(core::Db);

impl Db {
    /// Opens a Bedrock/native LevelDB directory.
    pub fn open(path: impl AsRef<Path>, options: LevelDbOpenOptions) -> Result<Self> {
        core::Db::open(path, options).map(Self)
    }

    /// Rebuilds a native manifest/table from readable tables and logs.
    pub fn repair(path: impl AsRef<Path>, options: LevelDbOpenOptions) -> Result<RepairReport> {
        core::Db::repair(path, options)
    }

    /// Returns a point-in-time cache statistics snapshot.
    #[must_use]
    pub fn cache_stats(&self) -> DbCacheStats {
        self.0.cache_stats()
    }

    /// Reads one key using default options.
    pub fn get(&self, key: &[u8]) -> Result<Option<Bytes>> {
        self.0.get(key)
    }

    /// Reads one key as a borrowed-first value view.
    pub fn get_ref(&self, key: &[u8]) -> Result<Option<ValueRef<'static>>> {
        self.0.get_ref(key)
    }

    /// Reads one key as shared owned bytes.
    pub fn get_owned(&self, key: &[u8]) -> Result<Option<Bytes>> {
        self.0.get_owned(key)
    }

    /// Reads one key with explicit options.
    pub fn get_with(&self, key: &[u8], options: ReadOptions) -> Result<Option<Bytes>> {
        self.0.get_with(key, options)
    }

    /// Reads one key with explicit borrowed/shared/owned strategy.
    pub fn get_with_ref(
        &self,
        key: &[u8],
        options: ReadOptions,
    ) -> Result<Option<ValueRef<'static>>> {
        self.0.get_with_ref(key, options)
    }

    /// Reads many exact keys while preserving input order.
    pub fn get_many_owned(
        &self,
        keys: impl IntoIterator<Item = Bytes>,
        options: ReadOptions,
    ) -> Result<Vec<Option<Bytes>>> {
        self.0.get_many_owned(keys, options)
    }

    /// Appends one put operation to the WAL-backed active memtable.
    pub fn put(
        &self,
        key: impl Into<Bytes>,
        value: impl Into<Bytes>,
        options: WriteOptions,
    ) -> Result<()> {
        self.0.put(key, value, options)
    }

    /// Appends one delete operation to the WAL-backed active memtable.
    pub fn delete(&self, key: impl Into<Bytes>, options: WriteOptions) -> Result<()> {
        self.0.delete(key, options)
    }

    /// Appends an atomic write batch.
    pub fn write(&self, batch: WriteBatch, options: WriteOptions) -> Result<()> {
        self.0.write(batch, options)
    }

    /// Visits visible keys through the visibility-correct borrowed scan.
    pub fn for_each_key<F>(&self, options: ReadOptions, visitor: F) -> Result<ScanOutcome>
    where
        F: FnMut(&[u8]) -> Result<VisitorControl> + Send,
    {
        self.0.for_each_key_scan(options, visitor)
    }

    /// Visits visible key/value entries as borrowed byte slices.
    ///
    /// The slices are valid only for the duration of the visitor call. Sequential
    /// scans point directly into reusable decoded SST block storage; parallel scans
    /// borrow directly from bounded decoded block-run buffers.
    pub fn for_each_entry_borrowed<F>(
        &self,
        options: ReadOptions,
        visitor: F,
    ) -> Result<ScanOutcome>
    where
        F: FnMut(&[u8], &[u8]) -> Result<VisitorControl> + Send,
    {
        self.0.for_each_entry_borrowed_scan(options, visitor)
    }

    /// Compatibility visitor that materializes one `Bytes` value per record.
    /// Prefer [`Db::for_each_entry_ref`] or [`Db::for_each_entry_borrowed`] in hot paths.
    pub fn for_each_entry<F>(&self, options: ReadOptions, mut visitor: F) -> Result<ScanOutcome>
    where
        F: FnMut(&[u8], &Bytes) -> Result<VisitorControl> + Send,
    {
        self.for_each_entry_borrowed(options, |key, value| {
            let value = Bytes::copy_from_slice(value);
            visitor(key, &value)
        })
    }

    /// Visits visible entries as borrowed-first entry views.
    pub fn for_each_entry_ref<F>(
        &self,
        options: ReadOptions,
        mut visitor: F,
    ) -> Result<ScanOutcome>
    where
        F: FnMut(EntryRef<'_>) -> Result<VisitorControl> + Send,
    {
        let strategy = options.read_strategy;
        self.for_each_entry_borrowed(options, |key, value| {
            visit_entry_ref(strategy, key, value, &mut visitor)
        })
    }

    /// Visits visible prefix entries as borrowed byte slices.
    pub fn for_each_prefix_borrowed<F>(
        &self,
        prefix: &[u8],
        options: ReadOptions,
        visitor: F,
    ) -> Result<ScanOutcome>
    where
        F: FnMut(&[u8], &[u8]) -> Result<VisitorControl> + Send,
    {
        self.0.for_each_prefix_borrowed_scan(prefix, options, visitor)
    }

    /// Compatibility prefix visitor that materializes one `Bytes` value per record.
    pub fn for_each_prefix<F>(
        &self,
        prefix: &[u8],
        options: ReadOptions,
        mut visitor: F,
    ) -> Result<ScanOutcome>
    where
        F: FnMut(&[u8], &Bytes) -> Result<VisitorControl> + Send,
    {
        self.for_each_prefix_borrowed(prefix, options, |key, value| {
            let value = Bytes::copy_from_slice(value);
            visitor(key, &value)
        })
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
        self.for_each_prefix_borrowed(prefix, options, |key, value| {
            visit_entry_ref(strategy, key, value, &mut visitor)
        })
    }

    /// Visits visible keys beginning with `prefix` without value materialization.
    pub fn for_each_prefix_key<F>(
        &self,
        prefix: &[u8],
        options: ReadOptions,
        visitor: F,
    ) -> Result<ScanOutcome>
    where
        F: FnMut(&[u8]) -> Result<VisitorControl> + Send,
    {
        self.0.for_each_prefix_key_scan(prefix, options, visitor)
    }

    /// Runs a visibility-correct key reduction using independent caller-owned states.
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
        self.0.scan_keys_partitioned_scan(options, init, visitor)
    }

    /// Runs an entry reduction through the compatibility `Bytes` visitor.
    ///
    /// This API remains for source compatibility; value-heavy hot paths should use
    /// borrowed entry scans and perform worker-local reduction at their own layer.
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

    /// Collects visible keys using the borrowed key scan.
    pub fn collect_keys_owned(&self, options: ReadOptions) -> Result<Vec<Bytes>> {
        let mut keys = Vec::new();
        self.for_each_key(options, |key| {
            keys.push(Bytes::copy_from_slice(key));
            Ok(VisitorControl::Continue)
        })?;
        Ok(keys)
    }

    /// Collects visible prefix keys using index/range seek.
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

    /// Collects visible prefix entries, materializing only requested output values.
    pub fn collect_prefix_owned(
        &self,
        prefix: &[u8],
        options: ReadOptions,
    ) -> Result<Vec<(Bytes, Bytes)>> {
        let mut entries = Vec::new();
        self.for_each_prefix_borrowed(prefix, options, |key, value| {
            entries.push((
                Bytes::copy_from_slice(key),
                Bytes::copy_from_slice(value),
            ));
            Ok(VisitorControl::Continue)
        })?;
        Ok(entries)
    }

    /// Materializes all visible entries into an iterator.
    ///
    /// Iterator materialization is not a streaming hot path and remains delegated to
    /// the stable pinned-snapshot implementation.
    pub fn iterator(&self, options: ReadOptions) -> Result<RawIterator> {
        self.0.iterator(options)
    }

    /// Materializes visible prefix entries into an iterator.
    pub fn prefix_iterator(&self, prefix: &[u8], options: ReadOptions) -> Result<PrefixIterator> {
        self.0.prefix_iterator(prefix, options)
    }

    /// Materializes a point-in-time visible snapshot.
    pub fn snapshot(&self) -> Result<Snapshot> {
        self.0.snapshot()
    }

    /// Flushes the active memtable and waits for durability.
    pub fn flush(&self) -> Result<()> {
        self.0.flush()
    }

    /// Flushes outstanding writes and forces leveled compaction.
    pub fn compact(&self) -> Result<()> {
        self.0.compact()
    }

    /// Returns metadata/memtable statistics without table scans.
    pub fn stats_fast(&self) -> Result<DbStats> {
        self.0.stats_fast()
    }

    /// Computes full statistics using the borrowed visibility scan.
    pub fn stats_full(&self) -> Result<DbStats> {
        let mut entries = 0usize;
        let mut approximate_bytes = 0usize;
        self.for_each_entry_borrowed(ReadOptions::default(), |key, value| {
            entries = entries.saturating_add(1);
            approximate_bytes = approximate_bytes
                .saturating_add(key.len())
                .saturating_add(value.len());
            Ok(VisitorControl::Continue)
        })?;
        let fast = self.0.stats_fast()?;
        Ok(DbStats {
            entries,
            tables: fast.tables,
            log_number: fast.log_number,
            approximate_bytes,
        })
    }

    /// Alias for [`Db::stats_full`].
    pub fn stats(&self) -> Result<DbStats> {
        self.stats_full()
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
    /// Reads one key with explicit options on a blocking Tokio task.
    pub async fn get_with_async(
        self: Arc<Self>,
        key: Bytes,
        options: ReadOptions,
    ) -> Result<Option<Bytes>> {
        tokio::task::spawn_blocking(move || self.get_with(&key, options))
            .await
            .map_err(|error| LevelDbError::join(error.to_string()))?
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
    /// Collects prefix keys on a blocking Tokio task.
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
    /// Collects prefix entries on a blocking Tokio task.
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
}

fn visit_entry_ref<F>(
    strategy: ReadStrategy,
    key: &[u8],
    value: &[u8],
    visitor: &mut F,
) -> Result<VisitorControl>
where
    F: FnMut(EntryRef<'_>) -> Result<VisitorControl>,
{
    match strategy {
        ReadStrategy::Borrowed => visitor(EntryRef {
            key: KeyRef::new(key),
            value: ValueRef::Borrowed(value),
        }),
        ReadStrategy::Shared => {
            let value = Bytes::copy_from_slice(value);
            visitor(EntryRef {
                key: KeyRef::new(key),
                value: ValueRef::Shared(value),
            })
        }
        ReadStrategy::Owned => {
            let value = Bytes::copy_from_slice(value);
            visitor(EntryRef {
                key: KeyRef::new(key),
                value: ValueRef::Owned(value),
            })
        }
    }
}
