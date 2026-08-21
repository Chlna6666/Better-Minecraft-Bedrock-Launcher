mod core {
    include!("db_core.rs");
    include!("db_scan_v2.rs");
}

pub use core::{
    DbCacheStats, DbStats, EntryRef, KeyRef, PrefixIterator, RawIterator, RepairReport, Snapshot,
    ValueRef,
};

use crate::error::{LevelDbError, Result};
use crate::options::{LevelDbOpenOptions, ReadOptions, ReadStrategy, ScanOutcome, VisitorControl};
use bytes::Bytes;
use std::ops::{Deref, DerefMut};
use std::path::Path;
use std::sync::Arc;

/// Public database handle.
///
/// Point reads, writes, WAL, snapshots and maintenance remain implemented by the
/// stable core handle. Scan APIs are overridden here so visibility scans use the
/// borrowed k-way/range implementation instead of the historical SST-wide seen set.
pub struct Db(core::Db);

impl Deref for Db {
    type Target = core::Db;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Db {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Db {
    /// Opens a Bedrock/native LevelDB directory.
    pub fn open(path: impl AsRef<Path>, options: LevelDbOpenOptions) -> Result<Self> {
        core::Db::open(path, options).map(Self)
    }

    /// Rebuilds a native manifest/table from readable tables and logs.
    pub fn repair(path: impl AsRef<Path>, options: LevelDbOpenOptions) -> Result<RepairReport> {
        core::Db::repair(path, options)
    }

    /// Visits visible keys through the visibility-correct borrowed k-way scan.
    pub fn for_each_key<F>(&self, options: ReadOptions, visitor: F) -> Result<ScanOutcome>
    where
        F: FnMut(&[u8]) -> Result<VisitorControl> + Send,
    {
        self.0.for_each_key_v2(options, visitor)
    }

    /// Visits visible key/value entries as borrowed byte slices.
    ///
    /// The slices are valid only for the duration of the visitor call. Sequential
    /// scans point directly into reusable decoded SST block storage; parallel scans
    /// point into recycled flat range batches.
    pub fn for_each_entry_borrowed<F>(
        &self,
        options: ReadOptions,
        visitor: F,
    ) -> Result<ScanOutcome>
    where
        F: FnMut(&[u8], &[u8]) -> Result<VisitorControl> + Send,
    {
        self.0.for_each_entry_borrowed_v2(options, visitor)
    }

    /// Compatibility visitor that materializes a `Bytes` value for each record.
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
        self.0.for_each_prefix_borrowed_v2(prefix, options, visitor)
    }

    /// Compatibility prefix visitor that materializes one `Bytes` per value.
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
        self.0.for_each_prefix_key_v2(prefix, options, visitor)
    }

    /// Runs a visibility-correct key reduction with one independent state per
    /// disjoint key-range worker.
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
        self.0.scan_keys_partitioned_v2(options, init, visitor)
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

    /// Collects visible prefix entries, materializing only the requested output.
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
