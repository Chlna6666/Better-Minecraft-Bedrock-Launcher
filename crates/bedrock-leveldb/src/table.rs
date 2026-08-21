use crate::coding::{get_varint32, get_varint64, masked_crc32c};
use crate::compression::{COMPRESSION_NONE, with_decompressed};
use crate::error::{LevelDbError, Result};
use crate::options::{ScanOutcome, VisitorControl};
use crate::table_cursor::TableCursor;
use bytes::Bytes;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, hash_map::DefaultHasher};
use std::fs::File;
use std::hash::{Hash, Hasher};
#[cfg(not(any(unix, windows)))]
use std::io::{Read, Seek, SeekFrom};
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, MutexGuard, TryLockError,
    atomic::{AtomicU64, Ordering},
};

const CUSTOM_TABLE_MAGIC: &[u8; 9] = b"BWLDBTBL1";
const LEVELDB_TABLE_MAGIC: u64 = 0xdb47_7524_8b80_fb57;
const LEVELDB_FOOTER_LEN: usize = 48;
const LEVELDB_BLOCK_TRAILER_LEN: usize = 5;
const READ_SCRATCH_FLOOR: usize = 16 * 1024;

thread_local! {
    static READ_SCRATCH: RefCell<ReadScratch> = RefCell::new(ReadScratch::new());
}

struct ReadScratch {
    io: Vec<u8>,
}

impl ReadScratch {
    fn new() -> Self {
        Self {
            io: Vec::with_capacity(READ_SCRATCH_FLOOR),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
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
            self.weight = self
                .weight
                .saturating_sub(entry.weight)
                .saturating_add(weight);
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
            if self
                .entries
                .get(&key)
                .is_some_and(|entry| entry.referenced)
            {
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
        if self.clock.is_empty() || self.hand >= self.clock.len() {
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

#[derive(Debug, Clone, Copy)]
struct BlockHandle {
    offset: u64,
    size: u64,
}

#[derive(Debug, Clone)]
struct NativeIndexEntry {
    largest_user_key: Bytes,
    handle: BlockHandle,
}

type NativeIndexEntries = Arc<[NativeIndexEntry]>;

#[derive(Debug)]
pub(crate) struct NativeBlockCache {
    data: Box<[Mutex<ClockCacheShard<NativeBlockCacheKey, Bytes>>]>,
    indexes: Box<[Mutex<ClockCacheShard<NativeIndexCacheKey, NativeIndexEntries>>]>,
    files: Box<[Mutex<ClockCacheShard<TableId, CachedTableFile>>]>,
    counters: NativeCacheCounters,
}

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
            let weight = entries
                .iter()
                .map(|entry| entry.largest_user_key.len().saturating_add(size_of::<BlockHandle>()))
                .sum();
            shard.insert(key, entries, weight, &self.counters.index_evictions);
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

    pub(crate) fn invalidate_paths(&self, paths: &[PathBuf]) {
        if paths.is_empty() {
            return;
        }
        let mut ids = paths.iter().map(|path| table_id(path)).collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();
        for shard in &self.data {
            if let Some(mut shard) = lock_cache_shard(shard, &self.counters.lock_contention) {
                shard.remove_where(|key| ids.binary_search(&key.table_id).is_ok());
            }
        }
        for shard in &self.indexes {
            if let Some(mut shard) = lock_cache_shard(shard, &self.counters.lock_contention) {
                shard.remove_where(|key| ids.binary_search(&key.table_id).is_ok());
            }
        }
        for shard in &self.files {
            if let Some(mut shard) = lock_cache_shard(shard, &self.counters.lock_contention) {
                shard.remove_where(|key| ids.binary_search(key).is_ok());
            }
        }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TableLookup {
    Missing,
    Deleted,
    Value(Bytes),
}

pub(crate) fn get_table_lookup(
    path: &Path,
    key: &[u8],
    paranoid_checks: bool,
    cache: Option<&NativeBlockCache>,
) -> Result<TableLookup> {
    if is_custom_table(path, cache)? {
        return get_custom_table_lookups(path, &[Bytes::copy_from_slice(key)], paranoid_checks)
            .map(|mut values| values.pop().unwrap_or(TableLookup::Missing));
    }
    let file = open_table_file(path, cache)?;
    let index = read_native_index_entries(&file, path, paranoid_checks, cache)?;
    let Some(entry) = find_index_entry(&index, key) else {
        return Ok(TableLookup::Missing);
    };
    let cache_key = NativeBlockCacheKey {
        table_id: table_id(path),
        offset: entry.handle.offset,
        size: entry.handle.size,
        paranoid_checks,
    };
    if let Some(block) = cache.and_then(|cache| cache.get(&cache_key)) {
        return find_in_shared_block(&block, key);
    }
    if let Some(cache) = cache {
        let block = read_native_block_owned(&file, path, entry.handle, paranoid_checks)?;
        let result = find_in_shared_block(&block, key)?;
        cache.insert(cache_key, block);
        return Ok(result);
    }
    with_native_block(&file, path, entry.handle, paranoid_checks, |block| {
        find_in_borrowed_block(block, key)
    })
}

pub(crate) fn get_table_lookups(
    path: &Path,
    keys: &[Bytes],
    paranoid_checks: bool,
    cache: Option<&NativeBlockCache>,
) -> Result<Vec<TableLookup>> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    if is_custom_table(path, cache)? {
        return get_custom_table_lookups(path, keys, paranoid_checks);
    }
    get_native_table_lookups(path, keys, paranoid_checks, cache)
}

fn get_custom_table_lookups(
    path: &Path,
    keys: &[Bytes],
    paranoid_checks: bool,
) -> Result<Vec<TableLookup>> {
    let order = sorted_key_indices(keys);
    let mut results = vec![TableLookup::Missing; keys.len()];
    let mut requested = 0_usize;
    let mut cursor = TableCursor::open(path, paranoid_checks)?;
    while requested < order.len() {
        let Some(entry) = cursor.next()? else {
            break;
        };
        while requested < order.len() && keys[order[requested]].as_ref() < entry.key.as_slice() {
            requested = duplicate_key_group_end(keys, &order, requested);
        }
        if requested >= order.len() {
            break;
        }
        if keys[order[requested]].as_ref() == entry.key.as_slice() {
            let end = duplicate_key_group_end(keys, &order, requested);
            let lookup = entry
                .value
                .map_or(TableLookup::Deleted, TableLookup::Value);
            for input_index in &order[requested..end] {
                results[*input_index] = lookup.clone();
            }
            requested = end;
        }
    }
    Ok(results)
}

fn get_native_table_lookups(
    path: &Path,
    keys: &[Bytes],
    paranoid_checks: bool,
    cache: Option<&NativeBlockCache>,
) -> Result<Vec<TableLookup>> {
    let file = open_table_file(path, cache)?;
    let index = read_native_index_entries(&file, path, paranoid_checks, cache)?;
    let order = sorted_key_indices(keys);
    let mut planned = Vec::<(usize, usize)>::with_capacity(order.len());
    for input_index in order {
        let key = keys[input_index].as_ref();
        if let Some(block_index) = find_index_entry_position(&index, key) {
            planned.push((block_index, input_index));
        }
    }
    let mut results = vec![TableLookup::Missing; keys.len()];
    let mut start = 0_usize;
    while start < planned.len() {
        let block_index = planned[start].0;
        let mut end = start.saturating_add(1);
        while end < planned.len() && planned[end].0 == block_index {
            end = end.saturating_add(1);
        }
        let entry = &index[block_index];
        let cache_key = NativeBlockCacheKey {
            table_id: table_id(path),
            offset: entry.handle.offset,
            size: entry.handle.size,
            paranoid_checks,
        };
        if let Some(block) = cache.and_then(|cache| cache.get(&cache_key)) {
            match_exact_shared_block(&block, keys, &planned[start..end], &mut results)?;
        } else {
            with_native_block(&file, path, entry.handle, paranoid_checks, |block| {
                match_exact_borrowed_block(block, keys, &planned[start..end], &mut results)
            })?;
        }
        start = end;
    }
    Ok(results)
}

pub(crate) fn read_table_lookups(
    path: &Path,
    paranoid_checks: bool,
) -> Result<BTreeMap<Vec<u8>, TableLookup>> {
    let mut entries = BTreeMap::new();
    let mut cursor = TableCursor::open(path, paranoid_checks)?;
    while let Some(entry) = cursor.next()? {
        entries.insert(
            entry.key,
            entry
                .value
                .map_or(TableLookup::Deleted, TableLookup::Value),
        );
    }
    Ok(entries)
}

pub(crate) fn read_table_max_sequence(path: &Path, paranoid_checks: bool) -> Result<u64> {
    if is_custom_table(path, None)? {
        return Ok(0);
    }
    let file = open_table_file(path, None)?;
    let index = read_native_index_entries(&file, path, paranoid_checks, None)?;
    let mut max_sequence = 0_u64;
    for entry in index.iter() {
        with_native_block(&file, path, entry.handle, paranoid_checks, |block| {
            let mut decoder = BlockEntryDecoder::new(block)?;
            while let Some(decoded) = decoder.next()? {
                if let Some(sequence) = internal_key_sequence(decoded.internal_key) {
                    max_sequence = max_sequence.max(sequence);
                }
            }
            Ok(())
        })?;
    }
    Ok(max_sequence)
}

pub(crate) fn for_each_table_lookup<F>(
    path: &Path,
    paranoid_checks: bool,
    _cache: Option<&NativeBlockCache>,
    mut visitor: F,
) -> Result<ScanOutcome>
where
    F: FnMut(&[u8], Option<&Bytes>) -> Result<VisitorControl>,
{
    let mut cursor = TableCursor::open(path, paranoid_checks)?;
    let mut outcome = ScanOutcome::empty();
    while let Some(entry) = cursor.next()? {
        if let Some(value) = &entry.value {
            outcome.record(value.len());
        }
        if visitor(&entry.key, entry.value.as_ref())? == VisitorControl::Stop {
            outcome.stopped = true;
            break;
        }
    }
    outcome.tables_scanned = outcome.tables_scanned.saturating_add(1);
    Ok(outcome)
}

fn find_index_entry<'a>(index: &'a [NativeIndexEntry], key: &[u8]) -> Option<&'a NativeIndexEntry> {
    find_index_entry_position(index, key).and_then(|position| index.get(position))
}

fn find_index_entry_position(index: &[NativeIndexEntry], key: &[u8]) -> Option<usize> {
    let position = index.partition_point(|entry| entry.largest_user_key.as_ref() < key);
    (position < index.len()).then_some(position)
}

fn find_in_shared_block(block: &Bytes, key: &[u8]) -> Result<TableLookup> {
    let mut decoder = BlockEntryDecoder::new(block)?;
    while let Some(entry) = decoder.next()? {
        let Some((user_key, is_value)) = split_internal_key(entry.internal_key) else {
            continue;
        };
        match user_key.cmp(key) {
            std::cmp::Ordering::Less => {}
            std::cmp::Ordering::Equal => {
                return Ok(if is_value {
                    let range = entry.value_range;
                    TableLookup::Value(block.slice(range))
                } else {
                    TableLookup::Deleted
                });
            }
            std::cmp::Ordering::Greater => break,
        }
    }
    Ok(TableLookup::Missing)
}

fn find_in_borrowed_block(block: &[u8], key: &[u8]) -> Result<TableLookup> {
    let mut decoder = BlockEntryDecoder::new(block)?;
    while let Some(entry) = decoder.next()? {
        let Some((user_key, is_value)) = split_internal_key(entry.internal_key) else {
            continue;
        };
        match user_key.cmp(key) {
            std::cmp::Ordering::Less => {}
            std::cmp::Ordering::Equal => {
                return Ok(if is_value {
                    TableLookup::Value(Bytes::copy_from_slice(entry.value))
                } else {
                    TableLookup::Deleted
                });
            }
            std::cmp::Ordering::Greater => break,
        }
    }
    Ok(TableLookup::Missing)
}

fn match_exact_shared_block(
    block: &Bytes,
    keys: &[Bytes],
    planned: &[(usize, usize)],
    results: &mut [TableLookup],
) -> Result<()> {
    let mut decoder = BlockEntryDecoder::new(block)?;
    let mut requested = 0_usize;
    while requested < planned.len() {
        let Some(entry) = decoder.next()? else {
            break;
        };
        let Some((user_key, is_value)) = split_internal_key(entry.internal_key) else {
            continue;
        };
        while requested < planned.len() && keys[planned[requested].1].as_ref() < user_key {
            requested = planned_duplicate_end(keys, planned, requested);
        }
        if requested >= planned.len() {
            break;
        }
        if keys[planned[requested].1].as_ref() == user_key {
            let end = planned_duplicate_end(keys, planned, requested);
            let lookup = if is_value {
                TableLookup::Value(block.slice(entry.value_range.clone()))
            } else {
                TableLookup::Deleted
            };
            for (_, input_index) in &planned[requested..end] {
                results[*input_index] = lookup.clone();
            }
            requested = end;
        }
    }
    Ok(())
}

fn match_exact_borrowed_block(
    block: &[u8],
    keys: &[Bytes],
    planned: &[(usize, usize)],
    results: &mut [TableLookup],
) -> Result<()> {
    let mut decoder = BlockEntryDecoder::new(block)?;
    let mut requested = 0_usize;
    while requested < planned.len() {
        let Some(entry) = decoder.next()? else {
            break;
        };
        let Some((user_key, is_value)) = split_internal_key(entry.internal_key) else {
            continue;
        };
        while requested < planned.len() && keys[planned[requested].1].as_ref() < user_key {
            requested = planned_duplicate_end(keys, planned, requested);
        }
        if requested >= planned.len() {
            break;
        }
        if keys[planned[requested].1].as_ref() == user_key {
            let end = planned_duplicate_end(keys, planned, requested);
            let lookup = if is_value {
                TableLookup::Value(Bytes::copy_from_slice(entry.value))
            } else {
                TableLookup::Deleted
            };
            for (_, input_index) in &planned[requested..end] {
                results[*input_index] = lookup.clone();
            }
            requested = end;
        }
    }
    Ok(())
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

fn planned_duplicate_end(
    keys: &[Bytes],
    planned: &[(usize, usize)],
    start: usize,
) -> usize {
    let key = keys[planned[start].1].as_ref();
    let mut end = start.saturating_add(1);
    while end < planned.len() && keys[planned[end].1].as_ref() == key {
        end = end.saturating_add(1);
    }
    end
}

fn is_custom_table(path: &Path, cache: Option<&NativeBlockCache>) -> Result<bool> {
    let file = open_table_file(path, cache)?;
    let mut magic = [0_u8; CUSTOM_TABLE_MAGIC.len()];
    let read = read_at(&file, &mut magic, 0)
        .map_err(|error| LevelDbError::io_at("read table magic", path, error))?;
    Ok(read == CUSTOM_TABLE_MAGIC.len() && magic == *CUSTOM_TABLE_MAGIC)
}

fn read_native_index_entries(
    file: &File,
    path: &Path,
    paranoid_checks: bool,
    cache: Option<&NativeBlockCache>,
) -> Result<NativeIndexEntries> {
    let cache_key = NativeIndexCacheKey {
        table_id: table_id(path),
        paranoid_checks,
    };
    if let Some(entries) = cache.and_then(|cache| cache.get_index(&cache_key)) {
        return Ok(entries);
    }
    let footer = read_native_footer(file, path)?;
    let magic_offset = LEVELDB_FOOTER_LEN - 8;
    let magic = u64::from_le_bytes(
        footer[magic_offset..]
            .try_into()
            .map_err(|_| LevelDbError::corruption_at(path, "native footer magic is invalid"))?,
    );
    if magic != LEVELDB_TABLE_MAGIC {
        return Err(LevelDbError::corruption_at(path, "native table magic mismatch"));
    }
    let mut footer_input = &footer[..magic_offset];
    let _meta_index = read_block_handle(&mut footer_input)?;
    let index_handle = read_block_handle(&mut footer_input)?;
    let index_block = read_native_block_owned(file, path, index_handle, paranoid_checks)?;
    let entries = decode_native_index_entries(&index_block)?;
    if let Some(cache) = cache {
        cache.insert_index(cache_key, Arc::clone(&entries));
    }
    Ok(entries)
}

fn decode_native_index_entries(block: &[u8]) -> Result<NativeIndexEntries> {
    let mut decoder = BlockEntryDecoder::new(block)?;
    let mut entries = Vec::new();
    while let Some(entry) = decoder.next()? {
        let Some((largest_user_key, _)) = split_internal_key(entry.internal_key) else {
            continue;
        };
        let mut handle_input = entry.value;
        let handle = read_block_handle(&mut handle_input)?;
        entries.push(NativeIndexEntry {
            largest_user_key: Bytes::copy_from_slice(largest_user_key),
            handle,
        });
    }
    Ok(entries.into())
}

fn read_native_footer(file: &File, path: &Path) -> Result<[u8; LEVELDB_FOOTER_LEN]> {
    let file_len = file
        .metadata()
        .map_err(|error| LevelDbError::io_at("stat native table", path, error))?
        .len();
    if file_len < LEVELDB_FOOTER_LEN as u64 {
        return Err(LevelDbError::corruption_at(path, "native table is truncated"));
    }
    let mut footer = [0_u8; LEVELDB_FOOTER_LEN];
    read_exact_at(
        file,
        &mut footer,
        file_len.saturating_sub(LEVELDB_FOOTER_LEN as u64),
    )
    .map_err(|error| LevelDbError::io_at("read native table footer", path, error))?;
    Ok(footer)
}

fn read_native_block_owned(
    file: &File,
    path: &Path,
    handle: BlockHandle,
    paranoid_checks: bool,
) -> Result<Bytes> {
    with_native_block(file, path, handle, paranoid_checks, |block| {
        Ok(Bytes::copy_from_slice(block))
    })
}

fn with_native_block<T>(
    file: &File,
    path: &Path,
    handle: BlockHandle,
    paranoid_checks: bool,
    consume: impl FnOnce(&[u8]) -> Result<T>,
) -> Result<T> {
    READ_SCRATCH.with(|scratch| {
        let mut scratch = scratch.borrow_mut();
        let size = usize::try_from(handle.size)
            .map_err(|_| LevelDbError::corruption_at(path, "native block size overflow"))?;
        let total_size = size.checked_add(LEVELDB_BLOCK_TRAILER_LEN).ok_or_else(|| {
            LevelDbError::corruption_at(path, "native block trailer range overflow")
        })?;
        scratch.io.clear();
        scratch.io.resize(total_size, 0);
        read_exact_at(file, &mut scratch.io, handle.offset)
            .map_err(|error| LevelDbError::io_at("read native table block", path, error))?;
        let compression = scratch.io[size];
        if paranoid_checks {
            let expected_crc = u32::from_le_bytes(
                scratch.io[size + 1..total_size]
                    .try_into()
                    .map_err(|_| LevelDbError::corruption_at(path, "native block crc is invalid"))?,
            );
            let actual_crc = masked_crc32c(&[&scratch.io[..size], &[compression]]);
            if actual_crc != expected_crc {
                return Err(LevelDbError::corruption_at(
                    path,
                    format!("native block checksum mismatch at offset {}", handle.offset),
                ));
            }
        }
        if compression == COMPRESSION_NONE {
            return consume(&scratch.io[..size]);
        }
        with_decompressed(compression, &scratch.io[..size], consume)
    })
}

struct DecodedBlockEntry<'a> {
    internal_key: &'a [u8],
    value: &'a [u8],
    value_range: std::ops::Range<usize>,
}

struct BlockEntryDecoder<'a> {
    block: &'a [u8],
    entries_end: usize,
    offset: usize,
    key: Vec<u8>,
}

impl<'a> BlockEntryDecoder<'a> {
    fn new(block: &'a [u8]) -> Result<Self> {
        Ok(Self {
            block,
            entries_end: native_block_entries_end(block)?,
            offset: 0,
            key: Vec::with_capacity(48),
        })
    }

    fn next(&mut self) -> Result<Option<DecodedBlockEntry<'_>>> {
        if self.offset >= self.entries_end {
            return Ok(None);
        }
        let mut input = &self.block[self.offset..self.entries_end];
        let shared = usize::try_from(get_varint32(&mut input)?)
            .map_err(|_| LevelDbError::corruption("native shared key length overflow"))?;
        let non_shared = usize::try_from(get_varint32(&mut input)?)
            .map_err(|_| LevelDbError::corruption("native key delta length overflow"))?;
        let value_len = usize::try_from(get_varint32(&mut input)?)
            .map_err(|_| LevelDbError::corruption("native value length overflow"))?;
        if shared > self.key.len() {
            return Err(LevelDbError::corruption(
                "native shared prefix exceeds previous key".to_string(),
            ));
        }
        if input.len() < non_shared.saturating_add(value_len) {
            return Err(LevelDbError::corruption("native block entry is truncated"));
        }
        self.key.truncate(shared);
        self.key.extend_from_slice(&input[..non_shared]);
        input = &input[non_shared..];
        let value_start = self.entries_end.saturating_sub(input.len());
        let value_end = value_start.checked_add(value_len).ok_or_else(|| {
            LevelDbError::corruption("native value range overflow")
        })?;
        input = &input[value_len..];
        self.offset = self.entries_end.saturating_sub(input.len());
        Ok(Some(DecodedBlockEntry {
            internal_key: &self.key,
            value: &self.block[value_start..value_end],
            value_range: value_start..value_end,
        }))
    }
}

fn native_block_entries_end(block: &[u8]) -> Result<usize> {
    if block.len() < 4 {
        return Err(LevelDbError::corruption("native block is truncated"));
    }
    let count_offset = block.len() - 4;
    let restart_count = usize::try_from(u32::from_le_bytes(
        block[count_offset..]
            .try_into()
            .map_err(|_| LevelDbError::corruption("native restart count is invalid"))?,
    ))
    .map_err(|_| LevelDbError::corruption("native restart count overflow"))?;
    let restart_bytes = restart_count
        .checked_mul(4)
        .ok_or_else(|| LevelDbError::corruption("native restart array overflow"))?;
    if restart_bytes > count_offset {
        return Err(LevelDbError::corruption("native restart array is truncated"));
    }
    Ok(count_offset - restart_bytes)
}

fn split_internal_key(internal_key: &[u8]) -> Option<(&[u8], bool)> {
    let user_len = internal_key.len().checked_sub(8)?;
    let user_key = internal_key.get(..user_len)?;
    let trailer: [u8; 8] = internal_key.get(user_len..)?.try_into().ok()?;
    let tag = u64::from_le_bytes(trailer);
    match (tag & 0xff) as u8 {
        crate::coding::VALUE_TYPE_VALUE => Some((user_key, true)),
        crate::coding::VALUE_TYPE_DELETION => Some((user_key, false)),
        _ => None,
    }
}

fn internal_key_sequence(internal_key: &[u8]) -> Option<u64> {
    let trailer: [u8; 8] = internal_key
        .get(internal_key.len().checked_sub(8)?..)?
        .try_into()
        .ok()?;
    Some(u64::from_le_bytes(trailer) >> 8)
}

fn read_block_handle(input: &mut &[u8]) -> Result<BlockHandle> {
    Ok(BlockHandle {
        offset: get_varint64(input)?,
        size: get_varint64(input)?,
    })
}

fn open_table_file(path: &Path, cache: Option<&NativeBlockCache>) -> Result<Arc<File>> {
    if let Some(cache) = cache {
        cache.open_table_file(path)
    } else {
        open_table_file_uncached(path).map(Arc::new)
    }
}

fn open_table_file_uncached(path: &Path) -> Result<File> {
    File::open(path).map_err(|error| LevelDbError::io_at("open native table", path, error))
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

#[cfg(unix)]
fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, buffer, offset)
}

#[cfg(windows)]
fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> std::io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, buffer, offset)
}

#[cfg(not(any(unix, windows)))]
fn read_at(file: &File, buffer: &mut [u8], offset: u64) -> std::io::Result<usize> {
    let mut file = file.try_clone()?;
    file.seek(SeekFrom::Start(offset))?;
    file.read(buffer)
}

fn read_exact_at(file: &File, mut buffer: &mut [u8], mut offset: u64) -> std::io::Result<()> {
    while !buffer.is_empty() {
        match read_at(file, buffer, offset)? {
            0 => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "failed to fill positional read buffer",
                ));
            }
            read => {
                offset = offset.saturating_add(read as u64);
                buffer = &mut buffer[read..];
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_batch_plans_duplicate_keys_once_per_block() {
        let keys = vec![
            Bytes::from_static(b"a"),
            Bytes::from_static(b"a"),
            Bytes::from_static(b"z"),
        ];
        let order = sorted_key_indices(&keys);
        assert_eq!(order, vec![0, 1, 2]);
        assert_eq!(duplicate_key_group_end(&keys, &order, 0), 2);
    }
}
