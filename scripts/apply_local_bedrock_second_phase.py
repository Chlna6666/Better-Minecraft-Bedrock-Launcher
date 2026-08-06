from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def write(path: str, text: str) -> None:
    target = ROOT / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(text, encoding="utf-8")


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


def replace_regex(text: str, pattern: str, replacement: str, label: str) -> str:
    updated, count = re.subn(pattern, replacement, text, count=1, flags=re.S)
    if count != 1:
        raise RuntimeError(f"{label}: expected exactly one regex match, found {count}")
    return updated


def patch_options() -> None:
    path = "crates/bedrock-leveldb/src/options.rs"
    text = read(path)
    marker = "/// Options used when opening a database directory.\n"
    if "pub struct NativeCacheOptions" not in text:
        cache_options = r'''/// Independent native table cache capacities.
///
/// The data and index caches are byte-bounded and sharded. The file cache is
/// entry-bounded so a read-heavy map viewer cannot retain an unbounded number
/// of open SSTable handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeCacheOptions {
    /// Maximum decoded data-block bytes retained across all shards.
    pub data_capacity: usize,
    /// Maximum decoded index-block bytes retained across all shards.
    pub index_capacity: usize,
    /// Maximum number of open SSTable file handles retained.
    pub file_capacity: usize,
    /// Number of cache shards. Values are normalized to `1..=64`.
    pub shards: usize,
}

impl NativeCacheOptions {
    /// Derives balanced capacities from the legacy aggregate cache size.
    #[must_use]
    pub const fn from_total(total: usize) -> Self {
        Self {
            data_capacity: total,
            index_capacity: total / 2,
            file_capacity: 256,
            shards: 16,
        }
    }

    /// Returns a normalized configuration suitable for cache construction.
    #[must_use]
    pub const fn normalized(self) -> Self {
        Self {
            data_capacity: self.data_capacity,
            index_capacity: self.index_capacity,
            file_capacity: self.file_capacity,
            shards: if self.shards == 0 {
                1
            } else if self.shards > 64 {
                64
            } else {
                self.shards
            },
        }
    }
}

impl Default for NativeCacheOptions {
    fn default() -> Self {
        Self {
            data_capacity: 64 * 1024 * 1024,
            index_capacity: 32 * 1024 * 1024,
            file_capacity: 256,
            shards: 16,
        }
    }
}

'''
        text = replace_once(text, marker, cache_options + marker, "insert NativeCacheOptions")
    write(path, text)


def patch_table_cache() -> None:
    path = "crates/bedrock-leveldb/src/table.rs"
    text = read(path)
    text = text.replace(
        "use std::collections::{BTreeMap, HashMap, VecDeque};",
        "use std::collections::{BTreeMap, HashMap, hash_map::DefaultHasher};",
    )
    text = text.replace(
        "use std::sync::{Arc, Mutex};",
        "use std::hash::{Hash, Hasher};\nuse std::sync::{\n    Arc, Mutex, MutexGuard, TryLockError,\n    atomic::{AtomicU64, Ordering},\n};",
    )

    cache_section = r'''#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct TableId(u64);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct NativeCacheStats {
    pub(crate) data_hits: u64,
    pub(crate) data_misses: u64,
    pub(crate) data_evictions: u64,
    pub(crate) index_hits: u64,
    pub(crate) index_misses: u64,
    pub(crate) index_evictions: u64,
    pub(crate) file_hits: u64,
    pub(crate) file_misses: u64,
    pub(crate) file_evictions: u64,
    pub(crate) lock_contention: u64,
    pub(crate) data_entries: usize,
    pub(crate) index_entries: usize,
    pub(crate) open_handles: usize,
}

#[derive(Debug, Default)]
struct NativeCacheCounters {
    data_hits: AtomicU64,
    data_misses: AtomicU64,
    data_evictions: AtomicU64,
    index_hits: AtomicU64,
    index_misses: AtomicU64,
    index_evictions: AtomicU64,
    file_hits: AtomicU64,
    file_misses: AtomicU64,
    file_evictions: AtomicU64,
    lock_contention: AtomicU64,
}

#[derive(Debug)]
struct ClockCacheEntry<V> {
    value: V,
    weight: usize,
    referenced: bool,
}

#[derive(Debug)]
struct ClockCacheShard<K, V> {
    capacity: usize,
    weight: usize,
    hand: usize,
    entries: HashMap<K, ClockCacheEntry<V>>,
    clock: Vec<K>,
}

impl<K, V> ClockCacheShard<K, V>
where
    K: Clone + Eq + Hash,
{
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            weight: 0,
            hand: 0,
            entries: HashMap::new(),
            clock: Vec::new(),
        }
    }

    fn get_cloned(&mut self, key: &K) -> Option<V>
    where
        V: Clone,
    {
        let entry = self.entries.get_mut(key)?;
        entry.referenced = true;
        Some(entry.value.clone())
    }

    fn insert(&mut self, key: K, value: V, weight: usize, evictions: &AtomicU64) {
        if self.capacity == 0 || weight > self.capacity {
            return;
        }
        if let Some(entry) = self.entries.get_mut(&key) {
            self.weight = self.weight.saturating_sub(entry.weight).saturating_add(weight);
            entry.value = value;
            entry.weight = weight;
            entry.referenced = true;
            self.evict_to_capacity(evictions);
            return;
        }
        self.weight = self.weight.saturating_add(weight);
        self.clock.push(key.clone());
        self.entries.insert(
            key,
            ClockCacheEntry {
                value,
                weight,
                referenced: true,
            },
        );
        self.evict_to_capacity(evictions);
    }

    fn evict_to_capacity(&mut self, evictions: &AtomicU64) {
        while self.weight > self.capacity && !self.clock.is_empty() {
            if self.hand >= self.clock.len() {
                self.hand = 0;
            }
            let key = self.clock[self.hand].clone();
            let referenced = self
                .entries
                .get(&key)
                .is_some_and(|entry| entry.referenced);
            if referenced {
                if let Some(entry) = self.entries.get_mut(&key) {
                    entry.referenced = false;
                }
                self.hand = self.hand.saturating_add(1);
                continue;
            }
            if let Some(entry) = self.entries.remove(&key) {
                self.weight = self.weight.saturating_sub(entry.weight);
                evictions.fetch_add(1, Ordering::Relaxed);
            }
            self.clock.swap_remove(self.hand);
        }
        if self.clock.is_empty() {
            self.hand = 0;
        } else if self.hand >= self.clock.len() {
            self.hand = 0;
        }
    }

    fn remove_where(&mut self, mut predicate: impl FnMut(&K) -> bool) {
        let removed_weight = self
            .entries
            .iter()
            .filter(|(key, _)| predicate(key))
            .map(|(_, entry)| entry.weight)
            .sum::<usize>();
        self.entries.retain(|key, _| !predicate(key));
        self.clock.retain(|key| !predicate(key));
        self.weight = self.weight.saturating_sub(removed_weight);
        if self.clock.is_empty() || self.hand >= self.clock.len() {
            self.hand = 0;
        }
    }

    fn len(&self) -> usize {
        self.entries.len()
    }
}

#[derive(Debug, Clone)]
struct CachedTableFile {
    path: PathBuf,
    file: Arc<File>,
}

#[derive(Debug)]
pub(crate) struct NativeBlockCache {
    data: Box<[Mutex<ClockCacheShard<NativeBlockCacheKey, Bytes>>]>,
    indexes: Box<[Mutex<ClockCacheShard<NativeIndexCacheKey, NativeIndexEntries>>]>,
    files: Box<[Mutex<ClockCacheShard<TableId, CachedTableFile>>]>,
    counters: NativeCacheCounters,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct NativeBlockCacheKey {
    table_id: TableId,
    offset: u64,
    size: u64,
    paranoid_checks: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct NativeIndexCacheKey {
    table_id: TableId,
    paranoid_checks: bool,
}

#[derive(Debug, Clone)]
struct NativeIndexEntry {
    key: Bytes,
    value: Bytes,
}

type NativeIndexEntries = Arc<[NativeIndexEntry]>;

impl NativeBlockCache {
    pub(crate) fn new(
        data_capacity: usize,
        index_capacity: usize,
        file_capacity: usize,
        shard_count: usize,
    ) -> Self {
        let shard_count = shard_count.clamp(1, 64);
        Self {
            data: cache_shards(data_capacity, shard_count),
            indexes: cache_shards(index_capacity, shard_count),
            files: cache_shards(file_capacity, shard_count),
            counters: NativeCacheCounters::default(),
        }
    }

    fn get(&self, key: &NativeBlockCacheKey) -> Option<Bytes> {
        let shard = &self.data[cache_shard_index(key, self.data.len())];
        let mut shard = lock_cache_shard(shard, &self.counters.lock_contention)?;
        let value = shard.get_cloned(key);
        if value.is_some() {
            self.counters.data_hits.fetch_add(1, Ordering::Relaxed);
        } else {
            self.counters.data_misses.fetch_add(1, Ordering::Relaxed);
        }
        value
    }

    fn insert(&self, key: NativeBlockCacheKey, block: Bytes) {
        let shard = &self.data[cache_shard_index(&key, self.data.len())];
        if let Some(mut shard) = lock_cache_shard(shard, &self.counters.lock_contention) {
            let weight = block.len();
            shard.insert(key, block, weight, &self.counters.data_evictions);
        }
    }

    fn get_index(&self, key: &NativeIndexCacheKey) -> Option<NativeIndexEntries> {
        let shard = &self.indexes[cache_shard_index(key, self.indexes.len())];
        let mut shard = lock_cache_shard(shard, &self.counters.lock_contention)?;
        let value = shard.get_cloned(key);
        if value.is_some() {
            self.counters.index_hits.fetch_add(1, Ordering::Relaxed);
        } else {
            self.counters.index_misses.fetch_add(1, Ordering::Relaxed);
        }
        value
    }

    fn insert_index(&self, key: NativeIndexCacheKey, entries: NativeIndexEntries) {
        let shard = &self.indexes[cache_shard_index(&key, self.indexes.len())];
        if let Some(mut shard) = lock_cache_shard(shard, &self.counters.lock_contention) {
            let weight = native_index_entries_size(&entries);
            shard.insert(key, entries, weight, &self.counters.index_evictions);
        }
    }

    pub(crate) fn invalidate_paths(&self, paths: &[PathBuf]) {
        if paths.is_empty() {
            return;
        }
        let table_ids = paths.iter().map(|path| table_id(path)).collect::<Vec<_>>();
        for shard in &self.data {
            if let Some(mut shard) = lock_cache_shard(shard, &self.counters.lock_contention) {
                shard.remove_where(|key| table_ids.contains(&key.table_id));
            }
        }
        for shard in &self.indexes {
            if let Some(mut shard) = lock_cache_shard(shard, &self.counters.lock_contention) {
                shard.remove_where(|key| table_ids.contains(&key.table_id));
            }
        }
        for shard in &self.files {
            if let Some(mut shard) = lock_cache_shard(shard, &self.counters.lock_contention) {
                shard.remove_where(|key| table_ids.contains(key));
            }
        }
    }

    fn open_table_file(&self, path: &Path) -> Result<Arc<File>> {
        let id = table_id(path);
        let shard = &self.files[cache_shard_index(&id, self.files.len())];
        if let Some(mut shard) = lock_cache_shard(shard, &self.counters.lock_contention)
            && let Some(cached) = shard.get_cloned(&id)
            && cached.path == path
        {
            self.counters.file_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(cached.file);
        }
        self.counters.file_misses.fetch_add(1, Ordering::Relaxed);
        let file = Arc::new(open_table_file_uncached(path)?);
        if let Some(mut shard) = lock_cache_shard(shard, &self.counters.lock_contention) {
            shard.insert(
                id,
                CachedTableFile {
                    path: path.to_path_buf(),
                    file: Arc::clone(&file),
                },
                1,
                &self.counters.file_evictions,
            );
        }
        Ok(file)
    }

    pub(crate) fn stats(&self) -> NativeCacheStats {
        NativeCacheStats {
            data_hits: self.counters.data_hits.load(Ordering::Relaxed),
            data_misses: self.counters.data_misses.load(Ordering::Relaxed),
            data_evictions: self.counters.data_evictions.load(Ordering::Relaxed),
            index_hits: self.counters.index_hits.load(Ordering::Relaxed),
            index_misses: self.counters.index_misses.load(Ordering::Relaxed),
            index_evictions: self.counters.index_evictions.load(Ordering::Relaxed),
            file_hits: self.counters.file_hits.load(Ordering::Relaxed),
            file_misses: self.counters.file_misses.load(Ordering::Relaxed),
            file_evictions: self.counters.file_evictions.load(Ordering::Relaxed),
            lock_contention: self.counters.lock_contention.load(Ordering::Relaxed),
            data_entries: cache_entry_count(&self.data),
            index_entries: cache_entry_count(&self.indexes),
            open_handles: cache_entry_count(&self.files),
        }
    }
}

fn table_id(path: &Path) -> TableId {
    if let Some(number) = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| stem.parse::<u64>().ok())
    {
        return TableId(number);
    }
    let mut hasher = DefaultHasher::new();
    path.to_string_lossy().hash(&mut hasher);
    TableId(hasher.finish())
}

fn cache_shards<K, V>(capacity: usize, shard_count: usize) -> Box<[Mutex<ClockCacheShard<K, V>>]>
where
    K: Clone + Eq + Hash,
{
    (0..shard_count)
        .map(|index| {
            let base = capacity / shard_count;
            let remainder = capacity % shard_count;
            Mutex::new(ClockCacheShard::new(
                base.saturating_add(usize::from(index < remainder)),
            ))
        })
        .collect::<Vec<_>>()
        .into_boxed_slice()
}

fn cache_shard_index(key: &impl Hash, shard_count: usize) -> usize {
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    usize::try_from(hasher.finish()).unwrap_or(0) % shard_count.max(1)
}

fn lock_cache_shard<'a, T>(
    shard: &'a Mutex<T>,
    contention: &AtomicU64,
) -> Option<MutexGuard<'a, T>> {
    match shard.try_lock() {
        Ok(guard) => Some(guard),
        Err(TryLockError::WouldBlock) => {
            contention.fetch_add(1, Ordering::Relaxed);
            shard.lock().ok()
        }
        Err(TryLockError::Poisoned(_)) => None,
    }
}

fn cache_entry_count<K, V>(shards: &[Mutex<ClockCacheShard<K, V>>]) -> usize
where
    K: Clone + Eq + Hash,
{
    shards
        .iter()
        .filter_map(|shard| shard.lock().ok().map(|shard| shard.len()))
        .sum()
}

'''
    text = replace_regex(
        text,
        r"#\[derive\(Debug\)\]\npub\(crate\) struct NativeBlockCache \{.*?\n\}\n\n#\[allow\(",
        cache_section + "#[allow(",
        "replace NativeBlockCache",
    )
    text = replace_once(
        text,
        "let cache_key = NativeIndexCacheKey {\n        path: path.to_path_buf(),\n        paranoid_checks,\n    };",
        "let cache_key = NativeIndexCacheKey {\n        table_id: table_id(path),\n        paranoid_checks,\n    };",
        "index cache key",
    )
    text = replace_once(
        text,
        "let cache_key = NativeBlockCacheKey {\n        path: path.to_path_buf(),\n        offset: handle.offset,\n        size: handle.size,\n        paranoid_checks,\n    };",
        "let cache_key = NativeBlockCacheKey {\n        table_id: table_id(path),\n        offset: handle.offset,\n        size: handle.size,\n        paranoid_checks,\n    };",
        "data cache key",
    )

    custom_get_many = r'''fn get_table_entries_impl(
    path: &Path,
    keys: &[Bytes],
    paranoid_checks: bool,
    cache: Option<&NativeBlockCache>,
) -> Result<Vec<Option<Bytes>>> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let Some(bytes) = read_custom_table_bytes(path, cache)? else {
        return get_native_table_entries_seeked(path, keys, paranoid_checks, cache);
    };

    let order = sorted_key_indices(keys);
    let mut cursor = 0usize;
    let mut results = vec![None; keys.len()];
    for_each_custom_table_entry_bytes(path, &bytes, paranoid_checks, |entry_key, value| {
        advance_key_cursor_before(keys, &order, &mut cursor, entry_key);
        if cursor >= order.len() {
            return Ok(VisitorControl::Stop);
        }
        if keys[order[cursor]].as_ref() == entry_key {
            let end = duplicate_key_group_end(keys, &order, cursor);
            for input_index in &order[cursor..end] {
                results[*input_index] = Some(value.clone());
            }
            cursor = end;
            if cursor >= order.len() {
                return Ok(VisitorControl::Stop);
            }
        }
        Ok(VisitorControl::Continue)
    })?;
    Ok(results)
}

fn sorted_key_indices(keys: &[Bytes]) -> Vec<usize> {
    let mut order = (0..keys.len()).collect::<Vec<_>>();
    order.sort_unstable_by(|left, right| {
        keys[*left]
            .as_ref()
            .cmp(keys[*right].as_ref())
            .then_with(|| left.cmp(right))
    });
    order
}

fn duplicate_key_group_end(keys: &[Bytes], order: &[usize], start: usize) -> usize {
    let key = keys[order[start]].as_ref();
    let mut end = start.saturating_add(1);
    while end < order.len() && keys[order[end]].as_ref() == key {
        end = end.saturating_add(1);
    }
    end
}

fn advance_key_cursor_before(
    keys: &[Bytes],
    order: &[usize],
    cursor: &mut usize,
    user_key: &[u8],
) {
    while *cursor < order.len() && keys[order[*cursor]].as_ref() < user_key {
        *cursor = duplicate_key_group_end(keys, order, *cursor);
    }
}

fn decode_entries'''
    text = replace_regex(
        text,
        r"fn get_table_entries_impl\(.*?\n\}\n\nfn decode_entries",
        custom_get_many,
        "replace custom table get_many",
    )

    native_get_many = r'''fn get_native_table_entries_seeked(
    path: &Path,
    keys: &[Bytes],
    paranoid_checks: bool,
    cache: Option<&NativeBlockCache>,
) -> Result<Vec<Option<Bytes>>> {
    let file = open_table_file(path, cache)?;
    let index_entries = read_native_index_entries(&file, path, paranoid_checks, cache)?;
    let order = sorted_key_indices(keys);
    let mut cursor = 0usize;
    let mut results = vec![None; keys.len()];

    for entry in index_entries.iter() {
        if cursor >= order.len() {
            break;
        }
        let Some((largest_key, _)) = split_internal_key(entry.key.as_ref()) else {
            continue;
        };
        if largest_key < keys[order[cursor]].as_ref() {
            continue;
        }
        let mut handle_input = entry.value.as_ref();
        let data_handle = read_block_handle(&mut handle_input)?;
        let data_block =
            read_native_block_from_file(&file, path, data_handle, paranoid_checks, cache)?;
        decode_native_block_entry_ranges(data_block.as_ref(), |internal_key, value_range| {
            let Some((user_key, is_value)) = split_internal_key(internal_key) else {
                return Ok(VisitorControl::Continue);
            };
            advance_key_cursor_before(keys, &order, &mut cursor, user_key);
            if cursor >= order.len() {
                return Ok(VisitorControl::Stop);
            }
            if keys[order[cursor]].as_ref() == user_key {
                let end = duplicate_key_group_end(keys, &order, cursor);
                if is_value {
                    let value = data_block.slice(value_range);
                    for input_index in &order[cursor..end] {
                        results[*input_index] = Some(value.clone());
                    }
                }
                cursor = end;
                if cursor >= order.len() {
                    return Ok(VisitorControl::Stop);
                }
            }
            Ok(VisitorControl::Continue)
        })?;
    }
    Ok(results)
}

fn read_native_index_entries'''
    text = replace_regex(
        text,
        r"fn get_native_table_entries_seeked\(.*?\n\}\n\nfn discard_missing_requests_before\(.*?\n\}\n\nfn read_native_index_entries",
        native_get_many,
        "replace native table get_many",
    )
    write(path, text)


def patch_db() -> None:
    path = "crates/bedrock-leveldb/src/db.rs"
    text = read(path)
    text = text.replace(
        "CachePolicy, ChecksumMode, CompressionPolicy, OpenOptions, ReadOptions, ReadStrategy, ScanMode,",
        "CachePolicy, ChecksumMode, CompressionPolicy, NativeCacheOptions, OpenOptions, ReadOptions, ReadStrategy, ScanMode,",
    )
    text = text.replace(
        "type LoadedState = (Manifest, Overlay, u64);",
        "type LoadedState = (Manifest, Overlay, u64, usize);",
    )
    if "pub struct DbCacheStats" not in text:
        marker = "/// Summary returned by [`Db::repair`].\n"
        stats = r'''/// Snapshot of the sharded native table caches.
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

'''
        text = replace_once(text, marker, stats + marker, "insert DbCacheStats")

    open_start = r'''    pub fn open(path: impl AsRef<Path>, options: OpenOptions) -> Result<Self> {
        let root = path.as_ref().to_path_buf();'''
    open_replacement = r'''    pub fn open(path: impl AsRef<Path>, options: OpenOptions) -> Result<Self> {
        let cache_options = NativeCacheOptions::from_total(options.cache_size);
        Self::open_with_cache_options(path, options, cache_options)
    }

    /// Opens a database with independent sharded cache capacities.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::open`].
    pub fn open_with_cache_options(
        path: impl AsRef<Path>,
        options: OpenOptions,
        cache_options: NativeCacheOptions,
    ) -> Result<Self> {
        let root = path.as_ref().to_path_buf();'''
    text = replace_once(text, open_start, open_replacement, "split Db::open")
    text = replace_once(
        text,
        "let (manifest, overlay, last_sequence) = load_existing_or_initialize(&root, &options)?;\n        let approximate_bytes = approximate_overlay_size(&overlay);\n        let cache_size = options.cache_size;",
        "let (manifest, overlay, last_sequence, approximate_bytes) =\n            load_existing_or_initialize(&root, &options)?;\n        let cache_options = cache_options.normalized();",
        "load state approximate bytes",
    )
    text = replace_once(
        text,
        "block_cache: table::NativeBlockCache::new(cache_size),",
        "block_cache: table::NativeBlockCache::new(\n                cache_options.data_capacity,\n                cache_options.index_capacity,\n                cache_options.file_capacity,\n                cache_options.shards,\n            ),",
        "construct sharded cache",
    )
    cache_stats_method = r'''    /// Returns a point-in-time snapshot of cache activity and occupancy.
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

'''
    if "pub fn cache_stats(&self)" not in text:
        text = replace_once(
            text,
            "    #[cfg(feature = \"async\")]\n    /// Opens a database on a blocking Tokio task.",
            cache_stats_method + "    #[cfg(feature = \"async\")]\n    /// Opens a database on a blocking Tokio task.",
            "insert cache stats method",
        )

    get_many_body = r'''    pub fn get_many_owned(
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

    #[cfg(feature = "async")]'''
    text = replace_regex(
        text,
        r"    pub fn get_many_owned\(.*?\n    \}\n\n    #\[cfg\(feature = \"async\"\)\]",
        get_many_body,
        "replace Db::get_many_owned",
    )

    text = replace_once(
        text,
        "            let mut overlay = BTreeMap::new();\n            let mut last_sequence = 0_u64;",
        "            let mut overlay = BTreeMap::new();\n            let mut last_sequence = 0_u64;\n            let mut approximate_bytes = 0usize;",
        "initialize WAL approximate bytes",
    )
    text = replace_once(
        text,
        "                    let approximate_bytes = approximate_overlay_size(&overlay);\n                    let _ = apply_batch(&mut overlay, &batch, approximate_bytes);",
        "                    approximate_bytes = apply_batch(&mut overlay, &batch, approximate_bytes);",
        "incremental WAL replay",
    )
    text = replace_once(
        text,
        "            Ok((manifest, overlay, last_sequence))",
        "            Ok((manifest, overlay, last_sequence, approximate_bytes))",
        "return WAL approximate bytes",
    )
    text = replace_once(
        text,
        "            Ok((manifest, BTreeMap::new(), 0))",
        "            Ok((manifest, BTreeMap::new(), 0, 0))",
        "return empty approximate bytes",
    )

    repair_old = r'''                    Ok(mut file) => {
                        for record in wal::read_records(&mut file, false)? {
                            if let Ok(batch) = WriteBatch::decode(&record) {
                                let approximate_bytes = approximate_entries_size(&values);
                                apply_batch_to_values(&mut values, &batch, approximate_bytes);
                                report.recovered_log_records += 1;
                            }
                        }
                    }'''
    repair_new = r'''                    Ok(mut file) => {
                        let mut approximate_bytes = approximate_entries_size(&values);
                        for record in wal::read_records(&mut file, false)? {
                            if let Ok(batch) = WriteBatch::decode(&record) {
                                approximate_bytes =
                                    apply_batch_to_values(&mut values, &batch, approximate_bytes);
                                report.recovered_log_records += 1;
                            }
                        }
                    }'''
    text = replace_once(text, repair_old, repair_new, "incremental repair WAL replay")
    write(path, text)


def patch_exports() -> None:
    path = "crates/bedrock-leveldb/src/bedrock_leveldb.rs"
    text = read(path)
    text = text.replace(
        "Db, DbStats, EntryRef, KeyRef, PrefixIterator, RawIterator, RepairReport, Snapshot, ValueRef,",
        "Db, DbCacheStats, DbStats, EntryRef, KeyRef, PrefixIterator, RawIterator, RepairReport, Snapshot, ValueRef,",
    )
    text = text.replace(
        "CachePolicy, ChecksumMode, CompressionPolicy, OpenOptions, ReadOptions, ReadStrategy,",
        "CachePolicy, ChecksumMode, CompressionPolicy, NativeCacheOptions, OpenOptions, ReadOptions, ReadStrategy,",
    )
    write(path, text)


def add_leveldb_tests() -> None:
    write(
        "crates/bedrock-leveldb/tests/second_phase.rs",
        r'''use bedrock_leveldb::{
    Db, NativeCacheOptions, OpenOptions, ReadOptions, Result, WriteOptions,
};
use bytes::Bytes;

#[test]
fn duplicate_batch_keys_share_payload_without_losing_input_order() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let db = Db::open_with_cache_options(
        dir.path(),
        OpenOptions::default(),
        NativeCacheOptions {
            data_capacity: 1024 * 1024,
            index_capacity: 1024 * 1024,
            file_capacity: 8,
            shards: 4,
        },
    )?;
    db.put(Bytes::from_static(b"alpha"), Bytes::from_static(b"one"), WriteOptions::default())?;
    db.put(Bytes::from_static(b"beta"), Bytes::from_static(b"two"), WriteOptions::default())?;
    db.flush()?;

    let keys = vec![
        Bytes::from_static(b"beta"),
        Bytes::from_static(b"alpha"),
        Bytes::from_static(b"alpha"),
        Bytes::from_static(b"missing"),
        Bytes::from_static(b"beta"),
    ];
    let first = db.get_many_owned(keys.clone(), ReadOptions::default())?;
    assert_eq!(first[0].as_deref(), Some(b"two".as_slice()));
    assert_eq!(first[1].as_deref(), Some(b"one".as_slice()));
    assert_eq!(first[2].as_deref(), Some(b"one".as_slice()));
    assert!(first[3].is_none());
    assert_eq!(first[4].as_deref(), Some(b"two".as_slice()));

    let before = db.cache_stats();
    let second = db.get_many_owned(keys, ReadOptions::default())?;
    assert_eq!(second, first);
    let after = db.cache_stats();
    assert!(after.index_hits >= before.index_hits);
    assert!(after.data_hits >= before.data_hits);
    assert!(after.open_handles <= 8);
    Ok(())
}
''',
    )


def main() -> None:
    patch_options()
    patch_table_cache()
    patch_db()
    patch_exports()
    add_leveldb_tests()


if __name__ == "__main__":
    main()
