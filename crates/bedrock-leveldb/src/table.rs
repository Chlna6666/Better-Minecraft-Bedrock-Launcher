use crate::coding::{
    get_length_prefixed_slice, get_varint32, put_length_prefixed_slice, put_varint32, put_varint64,
};
use crate::db::ValueRef;
use crate::error::{LevelDbError, Result};
use crate::options::{CompressionPolicy, ScanOutcome, VisitorControl};
use bytes::Bytes;
#[cfg(feature = "mmap")]
use memmap2::Mmap;
use std::collections::{BTreeMap, HashMap, hash_map::DefaultHasher};
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{BufWriter, Write};
#[cfg(not(any(unix, windows)))]
use std::io::{Seek, SeekFrom};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, MutexGuard, TryLockError,
    atomic::{AtomicU64, Ordering},
};

const TABLE_MAGIC: &[u8; 9] = b"BWLDBTBL1";
const TABLE_VERSION: u32 = 1;
const CUSTOM_TABLE_HEADER_LEN: usize = TABLE_MAGIC.len() + 9;

const COMPRESSION_NONE: u8 = 0;
const COMPRESSION_SNAPPY: u8 = 1;
const COMPRESSION_ZLIB: u8 = 2;
const COMPRESSION_BEDROCK_ZLIB: u8 = 4;
const LEVELDB_TABLE_MAGIC: u64 = 0xdb47_7524_8b80_fb57;
const LEVELDB_FOOTER_LEN: usize = 48;
const LEVELDB_BLOCK_TRAILER_LEN: usize = 5;
const NATIVE_DATA_BLOCK_TARGET: usize = 4 * 1024;
const NATIVE_RESTART_INTERVAL: usize = 16;

enum TableBuffer {
    Heap(Bytes),
    #[cfg(feature = "mmap")]
    Mapped(Arc<Mmap>),
}

impl TableBuffer {
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Heap(bytes) => bytes,
            #[cfg(feature = "mmap")]
            Self::Mapped(map) => map.as_ref(),
        }
    }
}

enum BlockValue<'a> {
    Borrowed(&'a [u8]),
    Shared(Bytes),
}

impl BlockValue<'_> {
    fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Borrowed(bytes) => bytes,
            Self::Shared(bytes) => bytes.as_ref(),
        }
    }

    fn value_ref<'b>(&'b self, slice: &'b [u8]) -> Result<ValueRef<'b>> {
        match self {
            Self::Borrowed(_) => Ok(ValueRef::Borrowed(slice)),
            Self::Shared(bytes) => Ok(ValueRef::Shared(bytes_slice_from_payload(bytes, slice)?)),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct CustomTablePayload<'a> {
    compression_tag: u8,
    encoded: &'a [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
            let referenced = self.entries.get(&key).is_some_and(|entry| entry.referenced);
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

#[allow(
    dead_code,
    reason = "legacy BWLDB writer retained only for compatibility tests and migrations"
)]
pub(crate) fn write_table(
    path: &Path,
    entries: &BTreeMap<Vec<u8>, Bytes>,
    compression: CompressionPolicy,
) -> Result<()> {
    log::trace!(
        "writing custom table {} with {} entries",
        path.display(),
        entries.len()
    );
    let mut payload = Vec::new();
    let len = u32::try_from(entries.len())
        .map_err(|_| LevelDbError::invalid_argument("table has too many entries".to_string()))?;
    put_varint32(len, &mut payload);
    for (key, value) in entries {
        put_length_prefixed_slice(key, &mut payload)?;
        put_length_prefixed_slice(value, &mut payload)?;
    }

    let compression_tag = compression_tag(compression);
    let encoded = compress_payload(compression, &payload)?;
    let mut file_bytes = Vec::new();
    file_bytes.extend_from_slice(TABLE_MAGIC);
    file_bytes.extend_from_slice(&TABLE_VERSION.to_le_bytes());
    file_bytes.push(compression_tag);
    file_bytes.extend_from_slice(&crate::coding::crc32c(&encoded).to_le_bytes());
    file_bytes.extend_from_slice(&encoded);

    let tmp_path = path.with_extension("ldbtmp");
    fs::write(&tmp_path, file_bytes)
        .map_err(|error| LevelDbError::io_at("write table temp file", &tmp_path, error))?;
    replace_file(&tmp_path, path)?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WrittenNativeTable {
    pub(crate) file_size: u64,
    pub(crate) smallest_internal_key: Vec<u8>,
    pub(crate) largest_internal_key: Vec<u8>,
}

pub(crate) fn write_native_table(
    path: &Path,
    entries: &BTreeMap<Vec<u8>, Bytes>,
    sequence: u64,
    compression: CompressionPolicy,
) -> Result<WrittenNativeTable> {
    log::trace!(
        "writing native LevelDB table {} with {} visible entries",
        path.display(),
        entries.len()
    );
    if entries.is_empty() {
        return Err(LevelDbError::invalid_argument(
            "native table writer requires at least one entry".to_string(),
        ));
    }

    let smallest_internal_key = internal_key(
        entries.first_key_value().expect("entries is not empty").0,
        sequence,
        crate::coding::VALUE_TYPE_VALUE,
    );
    let largest_internal_key = internal_key(
        entries.last_key_value().expect("entries is not empty").0,
        sequence,
        crate::coding::VALUE_TYPE_VALUE,
    );

    let tmp_path = path.with_extension("ldbtmp");
    let file = File::create(&tmp_path)
        .map_err(|error| LevelDbError::io_at("create native table temp file", &tmp_path, error))?;
    let mut writer = BufWriter::new(file);
    let data_compression = compression_tag(compression);
    let mut file_offset = 0_u64;
    let mut index_entries = Vec::new();
    let mut block = NativeBlockBuilder::new();
    let mut block_largest_key = Vec::new();

    for (key, value) in entries {
        let internal_key = internal_key(key, sequence, crate::coding::VALUE_TYPE_VALUE);
        if !block.is_empty()
            && block.estimated_size_after(&internal_key, value) > NATIVE_DATA_BLOCK_TARGET
        {
            let encoded = block.finish()?;
            let handle = write_native_block(
                &mut writer,
                &mut file_offset,
                &encoded,
                compression,
                data_compression,
            )?;
            let mut handle_bytes = Vec::new();
            write_block_handle(handle, &mut handle_bytes);
            index_entries.push((
                std::mem::take(&mut block_largest_key),
                Bytes::from(handle_bytes),
            ));
            block = NativeBlockBuilder::new();
        }
        block.add(&internal_key, value)?;
        block_largest_key = internal_key;
    }

    if !block.is_empty() {
        let encoded = block.finish()?;
        let handle = write_native_block(
            &mut writer,
            &mut file_offset,
            &encoded,
            compression,
            data_compression,
        )?;
        let mut handle_bytes = Vec::new();
        write_block_handle(handle, &mut handle_bytes);
        index_entries.push((block_largest_key, Bytes::from(handle_bytes)));
    }

    let index_offset = file_offset;
    let index_block = encode_native_block(&index_entries)?;
    let index_handle = write_native_block(
        &mut writer,
        &mut file_offset,
        &index_block,
        CompressionPolicy::None,
        COMPRESSION_NONE,
    )?;
    debug_assert_eq!(index_handle.offset, index_offset);

    let footer = native_footer(BlockHandle { offset: 0, size: 0 }, index_handle);
    writer
        .write_all(&footer)
        .map_err(|error| LevelDbError::io_at("write native table footer", &tmp_path, error))?;
    file_offset = file_offset
        .saturating_add(u64::try_from(footer.len()).expect("fixed footer length fits in u64"));
    writer
        .flush()
        .map_err(|error| LevelDbError::io_at("flush native table", &tmp_path, error))?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|error| LevelDbError::io_at("sync native table", &tmp_path, error))?;
    drop(writer);
    replace_file(&tmp_path, path)?;

    Ok(WrittenNativeTable {
        file_size: file_offset,
        smallest_internal_key,
        largest_internal_key,
    })
}

fn write_native_block(
    writer: &mut impl Write,
    file_offset: &mut u64,
    raw: &[u8],
    compression: CompressionPolicy,
    compression_tag: u8,
) -> Result<BlockHandle> {
    let encoded = compress_payload(compression, raw)?;
    let handle = BlockHandle {
        offset: *file_offset,
        size: u64::try_from(encoded.len())
            .map_err(|_| LevelDbError::invalid_argument("native block is too large".to_string()))?,
    };
    writer.write_all(&encoded)?;
    let mut trailer = [0_u8; LEVELDB_BLOCK_TRAILER_LEN];
    trailer[0] = compression_tag;
    trailer[1..].copy_from_slice(
        &crate::coding::masked_crc32c(&[&encoded, &[compression_tag]]).to_le_bytes(),
    );
    writer.write_all(&trailer)?;
    *file_offset = file_offset
        .saturating_add(handle.size)
        .saturating_add(LEVELDB_BLOCK_TRAILER_LEN as u64);
    Ok(handle)
}

struct NativeBlockBuilder {
    data: Vec<u8>,
    restarts: Vec<u32>,
    previous_key: Vec<u8>,
    entries_since_restart: usize,
}

impl NativeBlockBuilder {
    fn new() -> Self {
        Self {
            data: Vec::with_capacity(NATIVE_DATA_BLOCK_TARGET),
            restarts: Vec::new(),
            previous_key: Vec::new(),
            entries_since_restart: NATIVE_RESTART_INTERVAL,
        }
    }

    fn is_empty(&self) -> bool {
        self.previous_key.is_empty() && self.data.is_empty()
    }

    fn estimated_size_after(&self, key: &[u8], value: &[u8]) -> usize {
        self.data
            .len()
            .saturating_add(key.len())
            .saturating_add(value.len())
            .saturating_add(15)
            .saturating_add((self.restarts.len() + 1).saturating_mul(4))
            .saturating_add(4)
    }

    fn add(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        let restart = self.entries_since_restart >= NATIVE_RESTART_INTERVAL;
        let shared = if restart {
            self.restarts
                .push(u32::try_from(self.data.len()).map_err(|_| {
                    LevelDbError::invalid_argument("native block offset exceeds u32".to_string())
                })?);
            self.entries_since_restart = 0;
            0
        } else {
            common_prefix_len(&self.previous_key, key)
        };
        let non_shared = &key[shared..];
        put_varint32(
            u32::try_from(shared).map_err(|_| {
                LevelDbError::invalid_argument("native shared key length exceeds u32".to_string())
            })?,
            &mut self.data,
        );
        put_varint32(
            u32::try_from(non_shared.len()).map_err(|_| {
                LevelDbError::invalid_argument("native key length exceeds u32".to_string())
            })?,
            &mut self.data,
        );
        put_varint32(
            u32::try_from(value.len()).map_err(|_| {
                LevelDbError::invalid_argument("native value length exceeds u32".to_string())
            })?,
            &mut self.data,
        );
        self.data.extend_from_slice(non_shared);
        self.data.extend_from_slice(value);
        self.previous_key.clear();
        self.previous_key.extend_from_slice(key);
        self.entries_since_restart = self.entries_since_restart.saturating_add(1);
        Ok(())
    }

    fn finish(mut self) -> Result<Vec<u8>> {
        if self.restarts.is_empty() {
            self.restarts.push(0);
        }
        for restart in &self.restarts {
            self.data.extend_from_slice(&restart.to_le_bytes());
        }
        self.data.extend_from_slice(
            &u32::try_from(self.restarts.len())
                .map_err(|_| {
                    LevelDbError::invalid_argument("native restart count is too large".to_string())
                })?
                .to_le_bytes(),
        );
        Ok(self.data)
    }
}

fn common_prefix_len(left: &[u8], right: &[u8]) -> usize {
    left.iter()
        .zip(right)
        .take_while(|(left, right)| left == right)
        .count()
}

pub(crate) fn read_table(path: &Path, paranoid_checks: bool) -> Result<BTreeMap<Vec<u8>, Bytes>> {
    log::trace!("reading table {}", path.display());
    read_table_impl(path, paranoid_checks).map_err(|error| with_table_path(error, path))
}

fn read_table_impl(path: &Path, paranoid_checks: bool) -> Result<BTreeMap<Vec<u8>, Bytes>> {
    let bytes = fs::read(path).map_err(|error| LevelDbError::io_at("read table", path, error))?;
    if bytes.len() < CUSTOM_TABLE_HEADER_LEN {
        return read_native_table(path, &bytes, paranoid_checks);
    }
    if &bytes[..TABLE_MAGIC.len()] != TABLE_MAGIC {
        return read_native_table(path, &bytes, paranoid_checks);
    }
    let custom_payload = custom_table_payload(path, &bytes, paranoid_checks)?;
    let payload = decompress_payload(custom_payload.compression_tag, custom_payload.encoded)?;
    decode_entries(&payload)
}

fn with_table_path(error: LevelDbError, path: &Path) -> LevelDbError {
    match error {
        LevelDbError::Corruption {
            path: None,
            message,
        } => LevelDbError::corruption_at(path, message),
        other => other,
    }
}

fn custom_table_payload<'a>(
    path: &Path,
    bytes: &'a [u8],
    paranoid_checks: bool,
) -> Result<CustomTablePayload<'a>> {
    if !bytes.starts_with(TABLE_MAGIC) {
        return Err(LevelDbError::corruption_at(
            path,
            "custom table magic mismatch".to_string(),
        ));
    }

    let version_offset = TABLE_MAGIC.len();
    let version = u32::from_le_bytes(
        bytes
            .get(version_offset..version_offset + 4)
            .ok_or_else(|| {
                LevelDbError::corruption_at(path, "table version is truncated".to_string())
            })?
            .try_into()
            .map_err(|_| LevelDbError::corruption_at(path, "table version is truncated"))?,
    );
    if version != TABLE_VERSION {
        return Err(LevelDbError::corruption_at(
            path,
            format!("unsupported table version {version}"),
        ));
    }

    let compression_tag = *bytes.get(version_offset + 4).ok_or_else(|| {
        LevelDbError::corruption_at(path, "table compression tag is truncated".to_string())
    })?;
    let crc_offset = version_offset + 5;
    let expected_crc = u32::from_le_bytes(
        bytes
            .get(crc_offset..crc_offset + 4)
            .ok_or_else(|| LevelDbError::corruption_at(path, "table crc is truncated".to_string()))?
            .try_into()
            .map_err(|_| LevelDbError::corruption_at(path, "table crc is truncated"))?,
    );
    let encoded = bytes
        .get(crc_offset + 4..)
        .ok_or_else(|| LevelDbError::corruption_at(path, "table payload is truncated"))?;
    if paranoid_checks && crate::coding::crc32c(encoded) != expected_crc {
        return Err(LevelDbError::corruption_at(
            path,
            format!("table {} checksum mismatch", path.display()),
        ));
    }

    Ok(CustomTablePayload {
        compression_tag,
        encoded,
    })
}

pub(crate) fn for_each_table_entry<F>(
    path: &Path,
    paranoid_checks: bool,
    cache: Option<&NativeBlockCache>,
    mut visitor: F,
) -> Result<ScanOutcome>
where
    F: FnMut(&[u8], &Bytes) -> Result<VisitorControl>,
{
    let Some(bytes) = read_custom_table_bytes(path, cache)? else {
        log::trace!("scanning native table entries {}", path.display());
        return for_each_native_table_entry_seeked(path, paranoid_checks, cache, visitor);
    };
    log::trace!("scanning custom table entries {}", path.display());
    let custom_payload = custom_table_payload(path, &bytes, paranoid_checks)?;
    let payload = Bytes::from(decompress_payload(
        custom_payload.compression_tag,
        custom_payload.encoded,
    )?);
    let mut input = payload.as_ref();
    let count = usize::try_from(get_varint32(&mut input)?)
        .map_err(|_| LevelDbError::corruption("entry count overflow".to_string()))?;
    let mut outcome = ScanOutcome::empty();
    for _ in 0..count {
        let key = get_length_prefixed_slice(&mut input)?;
        let value_slice = get_length_prefixed_slice(&mut input)?;
        let value = bytes_slice_from_payload(&payload, value_slice)?;
        outcome.record(value.len());
        if visitor(key, &value)? == VisitorControl::Stop {
            outcome.stopped = true;
            return Ok(mark_table_scanned(outcome));
        }
    }
    if !input.is_empty() {
        return Err(LevelDbError::corruption(
            "table contains trailing bytes".to_string(),
        ));
    }
    Ok(mark_table_scanned(outcome))
}

pub(crate) fn for_each_table_entry_ref<F>(
    path: &Path,
    paranoid_checks: bool,
    mut visitor: F,
) -> Result<ScanOutcome>
where
    F: FnMut(&[u8], ValueRef<'_>) -> Result<VisitorControl>,
{
    let buffer = read_table_buffer(path)?;
    let bytes = buffer.as_slice();
    if is_custom_table_bytes(bytes) {
        log::trace!("scanning custom table entries by ref {}", path.display());
        return for_each_custom_table_entry_ref_bytes(path, bytes, paranoid_checks, visitor);
    }
    log::trace!("scanning native table entries by ref {}", path.display());
    for_each_native_table_entry_ref_bytes(path, bytes, paranoid_checks, &mut visitor)
}

pub(crate) fn for_each_table_key<F>(
    path: &Path,
    paranoid_checks: bool,
    cache: Option<&NativeBlockCache>,
    mut visitor: F,
) -> Result<ScanOutcome>
where
    F: FnMut(&[u8]) -> Result<VisitorControl>,
{
    let Some(bytes) = read_custom_table_bytes(path, cache)? else {
        log::trace!("scanning native table keys {}", path.display());
        return for_each_native_table_key_seeked(path, paranoid_checks, cache, visitor);
    };
    log::trace!("scanning custom table keys {}", path.display());
    let custom_payload = custom_table_payload(path, &bytes, paranoid_checks)?;
    let payload = Bytes::from(decompress_payload(
        custom_payload.compression_tag,
        custom_payload.encoded,
    )?);
    let mut input = payload.as_ref();
    let count = usize::try_from(get_varint32(&mut input)?)
        .map_err(|_| LevelDbError::corruption("entry count overflow".to_string()))?;
    let mut outcome = ScanOutcome::empty();
    for _ in 0..count {
        let key = get_length_prefixed_slice(&mut input)?;
        let value_len = get_length_prefixed_slice(&mut input)?.len();
        outcome.record(value_len);
        if visitor(key)? == VisitorControl::Stop {
            outcome.stopped = true;
            return Ok(mark_table_scanned(outcome));
        }
    }
    if !input.is_empty() {
        return Err(LevelDbError::corruption(
            "table contains trailing bytes".to_string(),
        ));
    }
    Ok(mark_table_scanned(outcome))
}

fn for_each_custom_table_entry_bytes<F>(
    path: &Path,
    bytes: &[u8],
    paranoid_checks: bool,
    mut visitor: F,
) -> Result<ScanOutcome>
where
    F: FnMut(&[u8], &Bytes) -> Result<VisitorControl>,
{
    let custom_payload = custom_table_payload(path, bytes, paranoid_checks)?;
    let payload = Bytes::from(decompress_payload(
        custom_payload.compression_tag,
        custom_payload.encoded,
    )?);
    let mut input = payload.as_ref();
    let count = usize::try_from(get_varint32(&mut input)?)
        .map_err(|_| LevelDbError::corruption("entry count overflow".to_string()))?;
    let mut outcome = ScanOutcome::empty();
    for _ in 0..count {
        let key = get_length_prefixed_slice(&mut input)?;
        let value_slice = get_length_prefixed_slice(&mut input)?;
        let value = bytes_slice_from_payload(&payload, value_slice)?;
        outcome.record(value.len());
        if visitor(key, &value)? == VisitorControl::Stop {
            outcome.stopped = true;
            return Ok(mark_table_scanned(outcome));
        }
    }
    if !input.is_empty() {
        return Err(LevelDbError::corruption(
            "table contains trailing bytes".to_string(),
        ));
    }
    Ok(mark_table_scanned(outcome))
}

fn for_each_custom_table_entry_ref_bytes<F>(
    path: &Path,
    bytes: &[u8],
    paranoid_checks: bool,
    mut visitor: F,
) -> Result<ScanOutcome>
where
    F: FnMut(&[u8], ValueRef<'_>) -> Result<VisitorControl>,
{
    let custom_payload = custom_table_payload(path, bytes, paranoid_checks)?;
    let payload = if custom_payload.compression_tag == COMPRESSION_NONE {
        BlockValue::Borrowed(custom_payload.encoded)
    } else {
        BlockValue::Shared(Bytes::from(decompress_payload(
            custom_payload.compression_tag,
            custom_payload.encoded,
        )?))
    };
    let mut input = payload.as_bytes();
    let count = usize::try_from(get_varint32(&mut input)?)
        .map_err(|_| LevelDbError::corruption("entry count overflow".to_string()))?;
    let mut outcome = ScanOutcome::empty();
    for _ in 0..count {
        let key = get_length_prefixed_slice(&mut input)?;
        let value_slice = get_length_prefixed_slice(&mut input)?;
        let value = payload.value_ref(value_slice)?;
        outcome.record(value.len());
        if visitor(key, value)? == VisitorControl::Stop {
            outcome.stopped = true;
            return Ok(mark_table_scanned(outcome));
        }
    }
    if !input.is_empty() {
        return Err(LevelDbError::corruption(
            "table contains trailing bytes".to_string(),
        ));
    }
    Ok(mark_table_scanned(outcome))
}

pub(crate) fn for_each_table_prefix<F>(
    path: &Path,
    prefix: &[u8],
    paranoid_checks: bool,
    cache: Option<&NativeBlockCache>,
    mut visitor: F,
) -> Result<ScanOutcome>
where
    F: FnMut(&[u8], &Bytes) -> Result<VisitorControl>,
{
    if prefix.is_empty() {
        return for_each_table_entry(path, paranoid_checks, cache, visitor);
    }
    let Some(bytes) = read_custom_table_bytes(path, cache)? else {
        log::trace!(
            "scanning native table prefix of {} bytes in {}",
            prefix.len(),
            path.display()
        );
        return for_each_native_table_prefix_seeked(path, prefix, paranoid_checks, cache, visitor);
    };
    log::trace!("scanning custom table prefix in {}", path.display());
    for_each_custom_table_entry_bytes(path, &bytes, paranoid_checks, |key, value| {
        if key.starts_with(prefix) {
            return visitor(key, value);
        }
        Ok(VisitorControl::Continue)
    })
}

pub(crate) fn for_each_table_prefix_ref<F>(
    path: &Path,
    prefix: &[u8],
    paranoid_checks: bool,
    mut visitor: F,
) -> Result<ScanOutcome>
where
    F: FnMut(&[u8], ValueRef<'_>) -> Result<VisitorControl>,
{
    if prefix.is_empty() {
        return for_each_table_entry_ref(path, paranoid_checks, visitor);
    }
    let buffer = read_table_buffer(path)?;
    let bytes = buffer.as_slice();
    if is_custom_table_bytes(bytes) {
        log::trace!("scanning custom table prefix by ref in {}", path.display());
        return for_each_custom_table_entry_ref_bytes(
            path,
            bytes,
            paranoid_checks,
            |key, value| {
                if key.starts_with(prefix) {
                    return visitor(key, value);
                }
                Ok(VisitorControl::Continue)
            },
        );
    }
    log::trace!(
        "scanning native table prefix by ref of {} bytes in {}",
        prefix.len(),
        path.display()
    );
    for_each_native_table_prefix_ref_bytes(path, bytes, prefix, paranoid_checks, &mut visitor)
}

pub(crate) fn for_each_table_prefix_key<F>(
    path: &Path,
    prefix: &[u8],
    paranoid_checks: bool,
    cache: Option<&NativeBlockCache>,
    visitor: F,
) -> Result<ScanOutcome>
where
    F: FnMut(&[u8]) -> Result<VisitorControl>,
{
    if prefix.is_empty() {
        return for_each_table_key(path, paranoid_checks, cache, visitor);
    }
    let Some(bytes) = read_custom_table_bytes(path, cache)? else {
        log::trace!(
            "scanning native table prefix keys of {} bytes in {}",
            prefix.len(),
            path.display()
        );
        return for_each_native_table_prefix_key_seeked(
            path,
            prefix,
            paranoid_checks,
            cache,
            visitor,
        );
    };
    log::trace!("scanning custom table prefix keys in {}", path.display());
    for_each_custom_table_prefix_key_bytes(path, &bytes, prefix, paranoid_checks, visitor)
}

pub(crate) fn get_table_entry(
    path: &Path,
    key: &[u8],
    paranoid_checks: bool,
    cache: Option<&NativeBlockCache>,
) -> Result<Option<Bytes>> {
    get_table_entry_impl(path, key, paranoid_checks, cache)
        .map_err(|error| with_table_path(error, path))
}

fn get_table_entry_impl(
    path: &Path,
    key: &[u8],
    paranoid_checks: bool,
    cache: Option<&NativeBlockCache>,
) -> Result<Option<Bytes>> {
    let Some(bytes) = read_custom_table_bytes(path, cache)? else {
        return get_native_table_entry_seeked(path, key, paranoid_checks, cache);
    };

    let mut found = None;
    for_each_custom_table_entry_bytes(path, &bytes, paranoid_checks, |entry_key, value| {
        if entry_key == key {
            found = Some(value.clone());
            return Ok(VisitorControl::Stop);
        }
        Ok(VisitorControl::Continue)
    })?;
    Ok(found)
}

pub(crate) fn get_table_entries(
    path: &Path,
    keys: &[Bytes],
    paranoid_checks: bool,
    cache: Option<&NativeBlockCache>,
) -> Result<Vec<Option<Bytes>>> {
    get_table_entries_impl(path, keys, paranoid_checks, cache)
        .map_err(|error| with_table_path(error, path))
}

fn get_table_entries_impl(
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

fn advance_key_cursor_before(keys: &[Bytes], order: &[usize], cursor: &mut usize, user_key: &[u8]) {
    while *cursor < order.len() && keys[order[*cursor]].as_ref() < user_key {
        *cursor = duplicate_key_group_end(keys, order, *cursor);
    }
}

fn decode_entries(payload: &[u8]) -> Result<BTreeMap<Vec<u8>, Bytes>> {
    let mut input = payload;
    let count = usize::try_from(get_varint32(&mut input)?)
        .map_err(|_| LevelDbError::corruption("entry count overflow".to_string()))?;
    let mut entries = BTreeMap::new();
    for _ in 0..count {
        let key = get_length_prefixed_slice(&mut input)?.to_vec();
        let value = Bytes::copy_from_slice(get_length_prefixed_slice(&mut input)?);
        entries.insert(key, value);
    }
    if !input.is_empty() {
        return Err(LevelDbError::corruption(
            "table contains trailing bytes".to_string(),
        ));
    }
    Ok(entries)
}

fn bytes_slice_from_payload(payload: &Bytes, slice: &[u8]) -> Result<Bytes> {
    let base = payload.as_ptr() as usize;
    let start = (slice.as_ptr() as usize).checked_sub(base).ok_or_else(|| {
        LevelDbError::corruption("table value slice is outside payload".to_string())
    })?;
    let end = start
        .checked_add(slice.len())
        .ok_or_else(|| LevelDbError::corruption("table value slice range overflow".to_string()))?;
    if end > payload.len() {
        return Err(LevelDbError::corruption(
            "table value slice exceeds payload".to_string(),
        ));
    }
    Ok(payload.slice(start..end))
}

fn for_each_custom_table_prefix_key_bytes<F>(
    path: &Path,
    bytes: &[u8],
    prefix: &[u8],
    paranoid_checks: bool,
    mut visitor: F,
) -> Result<ScanOutcome>
where
    F: FnMut(&[u8]) -> Result<VisitorControl>,
{
    let custom_payload = custom_table_payload(path, bytes, paranoid_checks)?;
    let payload = Bytes::from(decompress_payload(
        custom_payload.compression_tag,
        custom_payload.encoded,
    )?);
    let mut input = payload.as_ref();
    let count = usize::try_from(get_varint32(&mut input)?)
        .map_err(|_| LevelDbError::corruption("entry count overflow".to_string()))?;
    let mut outcome = ScanOutcome::empty();
    for _ in 0..count {
        let key = get_length_prefixed_slice(&mut input)?;
        let value_len = get_length_prefixed_slice(&mut input)?.len();
        if key.starts_with(prefix) {
            outcome.record(value_len);
            if visitor(key)? == VisitorControl::Stop {
                outcome.stopped = true;
                return Ok(mark_table_scanned(outcome));
            }
        }
    }
    if !input.is_empty() {
        return Err(LevelDbError::corruption(
            "table contains trailing bytes".to_string(),
        ));
    }
    Ok(mark_table_scanned(outcome))
}

#[derive(Debug, Clone, Copy)]
struct BlockHandle {
    offset: u64,
    size: u64,
}

fn read_custom_table_bytes(
    path: &Path,
    cache: Option<&NativeBlockCache>,
) -> Result<Option<Vec<u8>>> {
    let file = open_table_file(path, cache)?;
    let mut header = [0_u8; TABLE_MAGIC.len()];
    let bytes_read = read_at(&file, &mut header, 0)
        .map_err(|error| LevelDbError::io_at("read table header", path, error))?;
    if bytes_read != TABLE_MAGIC.len() || header != *TABLE_MAGIC {
        return Ok(None);
    }
    let file_len = usize::try_from(
        file.metadata()
            .map_err(|error| LevelDbError::io_at("stat table", path, error))?
            .len(),
    )
    .map_err(|_| LevelDbError::corruption("custom table length overflows usize".to_string()))?;
    let mut bytes = vec![0_u8; file_len];
    read_exact_at(&file, &mut bytes, 0)
        .map_err(|error| LevelDbError::io_at("read table body", path, error))?;
    Ok(Some(bytes))
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

fn native_index_entries_size(entries: &[NativeIndexEntry]) -> usize {
    entries
        .iter()
        .map(|entry| entry.key.len().saturating_add(entry.value.len()))
        .sum()
}

fn read_table_buffer(path: &Path) -> Result<TableBuffer> {
    #[cfg(feature = "mmap")]
    {
        read_table_buffer_mmap(path)
    }
    #[cfg(not(feature = "mmap"))]
    {
        let bytes =
            fs::read(path).map_err(|error| LevelDbError::io_at("read table", path, error))?;
        Ok(TableBuffer::Heap(Bytes::from(bytes)))
    }
}

#[cfg(feature = "mmap")]
#[allow(unsafe_code)]
fn read_table_buffer_mmap(path: &Path) -> Result<TableBuffer> {
    let file = File::open(path).map_err(|error| LevelDbError::io_at("open table", path, error))?;
    if file
        .metadata()
        .map_err(|error| LevelDbError::io_at("stat table", path, error))?
        .len()
        == 0
    {
        return Ok(TableBuffer::Heap(Bytes::new()));
    }
    // SAFETY: the mapping is read-only and owns the OS mapping after creation.
    // This crate only exposes slices from the mapping inside visitor callbacks,
    // so callers cannot observe borrowed data after `TableBuffer` is dropped.
    let map = unsafe { Mmap::map(&file) }
        .map_err(|error| LevelDbError::io_at("mmap table", path, error))?;
    Ok(TableBuffer::Mapped(Arc::new(map)))
}

fn is_custom_table_bytes(bytes: &[u8]) -> bool {
    bytes.len() >= CUSTOM_TABLE_HEADER_LEN && &bytes[..TABLE_MAGIC.len()] == TABLE_MAGIC
}

fn for_each_native_table_entry_seeked<F>(
    path: &Path,
    paranoid_checks: bool,
    cache: Option<&NativeBlockCache>,
    mut visitor: F,
) -> Result<ScanOutcome>
where
    F: FnMut(&[u8], &Bytes) -> Result<VisitorControl>,
{
    let file = open_table_file(path, cache)?;
    let index_entries = read_native_index_entries(&file, path, paranoid_checks, cache)?;
    let mut outcome = ScanOutcome::empty();
    let mut previous_user_key = Vec::new();
    for entry in index_entries.iter() {
        let mut handle_input = entry.value.as_ref();
        let data_handle = read_block_handle(&mut handle_input)?;
        let data_block =
            read_native_block_from_file(&file, path, data_handle, paranoid_checks, cache)?;
        for (internal_key, value) in decode_native_block_entries_bytes(&data_block)? {
            let Some((user_key, is_value)) = split_internal_key(&internal_key) else {
                continue;
            };
            if !is_next_user_key(&mut previous_user_key, user_key) {
                continue;
            }
            if is_value {
                outcome.record(value.len());
                if visitor(user_key, &value)? == VisitorControl::Stop {
                    outcome.stopped = true;
                    return Ok(mark_table_scanned(outcome));
                }
            }
        }
    }
    Ok(mark_table_scanned(outcome))
}

fn for_each_native_table_key_seeked<F>(
    path: &Path,
    paranoid_checks: bool,
    cache: Option<&NativeBlockCache>,
    mut visitor: F,
) -> Result<ScanOutcome>
where
    F: FnMut(&[u8]) -> Result<VisitorControl>,
{
    let file = open_table_file(path, cache)?;
    let index_entries = read_native_index_entries(&file, path, paranoid_checks, cache)?;
    let mut outcome = ScanOutcome::empty();
    let mut previous_user_key = Vec::new();
    for entry in index_entries.iter() {
        let mut handle_input = entry.value.as_ref();
        let data_handle = read_block_handle(&mut handle_input)?;
        let data_block =
            read_native_block_from_file(&file, path, data_handle, paranoid_checks, cache)?;
        let stopped =
            decode_native_block_entry_ranges(&data_block, |internal_key, value_range| {
                let Some((user_key, is_value)) = split_internal_key(internal_key) else {
                    return Ok(VisitorControl::Continue);
                };
                if !is_next_user_key(&mut previous_user_key, user_key) {
                    return Ok(VisitorControl::Continue);
                }
                if is_value {
                    outcome.record(value_range.len());
                    if visitor(user_key)? == VisitorControl::Stop {
                        return Ok(VisitorControl::Stop);
                    }
                }
                Ok(VisitorControl::Continue)
            })?;
        if stopped == VisitorControl::Stop {
            outcome.stopped = true;
            return Ok(mark_table_scanned(outcome));
        }
    }
    Ok(mark_table_scanned(outcome))
}

fn for_each_native_table_prefix_seeked<F>(
    path: &Path,
    prefix: &[u8],
    paranoid_checks: bool,
    cache: Option<&NativeBlockCache>,
    mut visitor: F,
) -> Result<ScanOutcome>
where
    F: FnMut(&[u8], &Bytes) -> Result<VisitorControl>,
{
    let file = open_table_file(path, cache)?;
    let index_entries = read_native_index_entries(&file, path, paranoid_checks, cache)?;
    let mut outcome = ScanOutcome::empty();
    let mut previous_user_key = Vec::new();
    for entry in index_entries.iter() {
        let Some((largest_key, _)) = split_internal_key(entry.key.as_ref()) else {
            continue;
        };
        if largest_key < prefix {
            continue;
        }
        let mut handle_input = entry.value.as_ref();
        let data_handle = read_block_handle(&mut handle_input)?;
        let data_block =
            read_native_block_from_file(&file, path, data_handle, paranoid_checks, cache)?;
        for (internal_key, value) in decode_native_block_entries_bytes(&data_block)? {
            let Some((user_key, is_value)) = split_internal_key(&internal_key) else {
                continue;
            };
            if !is_next_user_key(&mut previous_user_key, user_key) {
                continue;
            }
            if user_key.starts_with(prefix) {
                if is_value {
                    outcome.record(value.len());
                    if visitor(user_key, &value)? == VisitorControl::Stop {
                        outcome.stopped = true;
                        return Ok(mark_table_scanned(outcome));
                    }
                }
            } else if user_key > prefix {
                return Ok(mark_table_scanned(outcome));
            }
        }
    }
    Ok(mark_table_scanned(outcome))
}

fn for_each_native_table_prefix_key_seeked<F>(
    path: &Path,
    prefix: &[u8],
    paranoid_checks: bool,
    cache: Option<&NativeBlockCache>,
    mut visitor: F,
) -> Result<ScanOutcome>
where
    F: FnMut(&[u8]) -> Result<VisitorControl>,
{
    let file = open_table_file(path, cache)?;
    let index_entries = read_native_index_entries(&file, path, paranoid_checks, cache)?;
    let mut outcome = ScanOutcome::empty();
    let mut previous_user_key = Vec::new();
    for entry in index_entries.iter() {
        let Some((largest_key, _)) = split_internal_key(entry.key.as_ref()) else {
            continue;
        };
        if largest_key < prefix {
            continue;
        }
        let mut handle_input = entry.value.as_ref();
        let data_handle = read_block_handle(&mut handle_input)?;
        let data_block =
            read_native_block_from_file(&file, path, data_handle, paranoid_checks, cache)?;
        let mut reached_prefix_end = false;
        let stopped =
            decode_native_block_entry_ranges(&data_block, |internal_key, value_range| {
                let Some((user_key, is_value)) = split_internal_key(internal_key) else {
                    return Ok(VisitorControl::Continue);
                };
                if !is_next_user_key(&mut previous_user_key, user_key) {
                    return Ok(VisitorControl::Continue);
                }
                if user_key.starts_with(prefix) {
                    if is_value {
                        outcome.record(value_range.len());
                        if visitor(user_key)? == VisitorControl::Stop {
                            return Ok(VisitorControl::Stop);
                        }
                    }
                } else if user_key > prefix {
                    reached_prefix_end = true;
                    return Ok(VisitorControl::Stop);
                }
                Ok(VisitorControl::Continue)
            })?;
        if stopped == VisitorControl::Stop {
            if reached_prefix_end {
                return Ok(mark_table_scanned(outcome));
            }
            outcome.stopped = true;
            return Ok(mark_table_scanned(outcome));
        }
    }
    Ok(mark_table_scanned(outcome))
}

fn for_each_native_table_entry_ref_bytes<F>(
    path: &Path,
    table_bytes: &[u8],
    paranoid_checks: bool,
    visitor: &mut F,
) -> Result<ScanOutcome>
where
    F: FnMut(&[u8], ValueRef<'_>) -> Result<VisitorControl>,
{
    let index_entries = read_native_index_entries_bytes(path, table_bytes, paranoid_checks)?;
    let mut outcome = ScanOutcome::empty();
    let mut previous_user_key = Vec::new();
    for (_, handle_bytes) in index_entries {
        let mut handle_input = handle_bytes.as_ref();
        let data_handle = read_block_handle(&mut handle_input)?;
        let data_block = read_native_block_value(path, table_bytes, data_handle, paranoid_checks)?;
        let stopped = decode_native_block_entries_ref(&data_block, |internal_key, value| {
            let Some((user_key, is_value)) = split_internal_key(internal_key) else {
                return Ok(VisitorControl::Continue);
            };
            if !is_next_user_key(&mut previous_user_key, user_key) {
                return Ok(VisitorControl::Continue);
            }
            if is_value {
                outcome.record(value.len());
                if visitor(user_key, value)? == VisitorControl::Stop {
                    return Ok(VisitorControl::Stop);
                }
            }
            Ok(VisitorControl::Continue)
        })?;
        if stopped == VisitorControl::Stop {
            outcome.stopped = true;
            return Ok(mark_table_scanned(outcome));
        }
    }
    Ok(mark_table_scanned(outcome))
}

fn for_each_native_table_prefix_ref_bytes<F>(
    path: &Path,
    table_bytes: &[u8],
    prefix: &[u8],
    paranoid_checks: bool,
    visitor: &mut F,
) -> Result<ScanOutcome>
where
    F: FnMut(&[u8], ValueRef<'_>) -> Result<VisitorControl>,
{
    let index_entries = read_native_index_entries_bytes(path, table_bytes, paranoid_checks)?;
    let mut outcome = ScanOutcome::empty();
    let mut previous_user_key = Vec::new();
    for (index_key, handle_bytes) in index_entries {
        let Some((largest_key, _)) = split_internal_key(&index_key) else {
            continue;
        };
        if largest_key < prefix {
            continue;
        }
        let mut handle_input = handle_bytes.as_ref();
        let data_handle = read_block_handle(&mut handle_input)?;
        let data_block = read_native_block_value(path, table_bytes, data_handle, paranoid_checks)?;
        let stopped = decode_native_block_entries_ref(&data_block, |internal_key, value| {
            let Some((user_key, is_value)) = split_internal_key(internal_key) else {
                return Ok(VisitorControl::Continue);
            };
            if !is_next_user_key(&mut previous_user_key, user_key) {
                return Ok(VisitorControl::Continue);
            }
            if user_key.starts_with(prefix) && is_value {
                outcome.record(value.len());
                if visitor(user_key, value)? == VisitorControl::Stop {
                    return Ok(VisitorControl::Stop);
                }
            }
            Ok(VisitorControl::Continue)
        })?;
        if stopped == VisitorControl::Stop {
            outcome.stopped = true;
            return Ok(mark_table_scanned(outcome));
        }
    }
    Ok(mark_table_scanned(outcome))
}

fn get_native_table_entry_seeked(
    path: &Path,
    key: &[u8],
    paranoid_checks: bool,
    cache: Option<&NativeBlockCache>,
) -> Result<Option<Bytes>> {
    let file = open_table_file(path, cache)?;
    let index_entries = read_native_index_entries(&file, path, paranoid_checks, cache)?;
    for entry in index_entries.iter() {
        let Some((largest_key, _)) = split_internal_key(entry.key.as_ref()) else {
            continue;
        };
        if largest_key < key {
            continue;
        }
        let mut handle_input = entry.value.as_ref();
        let data_handle = read_block_handle(&mut handle_input)?;
        let data_block =
            read_native_block_from_file(&file, path, data_handle, paranoid_checks, cache)?;
        let mut found = None;
        decode_native_block_entry_ranges(data_block.as_ref(), |internal_key, value_range| {
            let Some((user_key, is_value)) = split_internal_key(internal_key) else {
                return Ok(VisitorControl::Continue);
            };
            match user_key.cmp(key) {
                std::cmp::Ordering::Less => Ok(VisitorControl::Continue),
                std::cmp::Ordering::Equal => {
                    if is_value {
                        found = Some(data_block.slice(value_range));
                    }
                    Ok(VisitorControl::Stop)
                }
                std::cmp::Ordering::Greater => Ok(VisitorControl::Stop),
            }
        })?;
        return Ok(found);
    }
    Ok(None)
}

fn get_native_table_entries_seeked(
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
    let magic = u64::from_le_bytes(footer[magic_offset..].try_into().map_err(|_| {
        LevelDbError::corruption(format!("native table {} footer is invalid", path.display()))
    })?);
    if magic != LEVELDB_TABLE_MAGIC {
        return Err(LevelDbError::corruption(format!(
            "table {} has unsupported magic",
            path.display()
        )));
    }

    let mut footer_input = &footer[..magic_offset];
    let _meta_index_handle = read_block_handle(&mut footer_input)?;
    let index_handle = read_block_handle(&mut footer_input)?;
    let index_block =
        read_native_block_from_file(file, path, index_handle, paranoid_checks, cache)?;
    let entries = decode_native_index_entries(&index_block)?;
    if let Some(cache) = cache {
        cache.insert_index(cache_key, Arc::clone(&entries));
    }
    Ok(entries)
}

fn read_native_index_entries_bytes(
    path: &Path,
    table_bytes: &[u8],
    paranoid_checks: bool,
) -> Result<Vec<(Vec<u8>, Bytes)>> {
    if table_bytes.len() < LEVELDB_FOOTER_LEN {
        return Err(LevelDbError::corruption(format!(
            "native table {} is truncated",
            path.display()
        )));
    }
    let footer = &table_bytes[table_bytes.len() - LEVELDB_FOOTER_LEN..];
    let magic_offset = LEVELDB_FOOTER_LEN - 8;
    let magic = u64::from_le_bytes(footer[magic_offset..].try_into().map_err(|_| {
        LevelDbError::corruption(format!("native table {} footer is invalid", path.display()))
    })?);
    if magic != LEVELDB_TABLE_MAGIC {
        return Err(LevelDbError::corruption(format!(
            "table {} has unsupported magic",
            path.display()
        )));
    }

    let mut footer_input = &footer[..magic_offset];
    let _meta_index_handle = read_block_handle(&mut footer_input)?;
    let index_handle = read_block_handle(&mut footer_input)?;
    let index_block = read_native_block_value(path, table_bytes, index_handle, paranoid_checks)?;
    collect_native_block_entries(&index_block)
}

fn read_native_footer(file: &File, path: &Path) -> Result<[u8; LEVELDB_FOOTER_LEN]> {
    let file_len = file.metadata()?.len();
    if file_len < LEVELDB_FOOTER_LEN as u64 {
        return Err(LevelDbError::corruption(format!(
            "native table {} is truncated",
            path.display()
        )));
    }
    let mut footer = [0_u8; LEVELDB_FOOTER_LEN];
    read_exact_at(
        file,
        &mut footer,
        file_len.saturating_sub(LEVELDB_FOOTER_LEN as u64),
    )?;
    Ok(footer)
}

fn read_native_table(
    path: &Path,
    bytes: &[u8],
    paranoid_checks: bool,
) -> Result<BTreeMap<Vec<u8>, Bytes>> {
    if bytes.len() < LEVELDB_FOOTER_LEN {
        return Err(LevelDbError::corruption(format!(
            "native table {} is truncated",
            path.display()
        )));
    }
    let footer = &bytes[bytes.len() - LEVELDB_FOOTER_LEN..];
    let magic_offset = LEVELDB_FOOTER_LEN - 8;
    let magic = u64::from_le_bytes(footer[magic_offset..].try_into().map_err(|_| {
        LevelDbError::corruption(format!("native table {} footer is invalid", path.display()))
    })?);
    if magic != LEVELDB_TABLE_MAGIC {
        return Err(LevelDbError::corruption(format!(
            "table {} has unsupported magic",
            path.display()
        )));
    }

    let mut footer_input = &footer[..magic_offset];
    let _meta_index_handle = read_block_handle(&mut footer_input)?;
    let index_handle = read_block_handle(&mut footer_input)?;
    let index_block = Bytes::from(read_native_block(
        path,
        bytes,
        index_handle,
        paranoid_checks,
    )?);
    let index_entries = decode_native_block_entries_bytes(&index_block)?;
    let mut entries = BTreeMap::new();
    let mut previous_user_key = Vec::new();

    for (_, handle_bytes) in index_entries {
        let mut handle_input = handle_bytes.as_ref();
        let data_handle = read_block_handle(&mut handle_input)?;
        let data_block = Bytes::from(read_native_block(
            path,
            bytes,
            data_handle,
            paranoid_checks,
        )?);
        for (internal_key, value) in decode_native_block_entries_bytes(&data_block)? {
            let Some((user_key, is_value)) = split_internal_key(&internal_key) else {
                continue;
            };
            if !is_next_user_key(&mut previous_user_key, user_key) {
                continue;
            }
            if is_value {
                entries.insert(user_key.to_vec(), value);
            }
        }
    }

    Ok(entries)
}

fn read_block_handle(input: &mut &[u8]) -> Result<BlockHandle> {
    Ok(BlockHandle {
        offset: crate::coding::get_varint64(input)?,
        size: crate::coding::get_varint64(input)?,
    })
}

fn write_block_handle(handle: BlockHandle, out: &mut Vec<u8>) {
    put_varint64(handle.offset, out);
    put_varint64(handle.size, out);
}

fn read_native_block(
    path: &Path,
    table_bytes: &[u8],
    handle: BlockHandle,
    paranoid_checks: bool,
) -> Result<Vec<u8>> {
    let offset = usize::try_from(handle.offset).map_err(|_| {
        LevelDbError::corruption(format!(
            "table {} block offset overflows usize",
            path.display()
        ))
    })?;
    let size = usize::try_from(handle.size).map_err(|_| {
        LevelDbError::corruption(format!(
            "table {} block size overflows usize",
            path.display()
        ))
    })?;
    let trailer_offset = offset.checked_add(size).ok_or_else(|| {
        LevelDbError::corruption(format!("table {} block range overflows", path.display()))
    })?;
    let end = trailer_offset
        .checked_add(LEVELDB_BLOCK_TRAILER_LEN)
        .ok_or_else(|| {
            LevelDbError::corruption(format!("table {} block trailer overflows", path.display()))
        })?;
    if end > table_bytes.len() {
        return Err(LevelDbError::corruption(format!(
            "table {} block is truncated at offset {offset}",
            path.display()
        )));
    }
    let payload = &table_bytes[offset..trailer_offset];
    let compression = table_bytes[trailer_offset];
    if paranoid_checks {
        let expected_crc = u32::from_le_bytes(
            table_bytes[trailer_offset + 1..end]
                .try_into()
                .map_err(|_| {
                    LevelDbError::corruption(format!(
                        "table {} block crc is truncated",
                        path.display()
                    ))
                })?,
        );
        let actual_crc = crate::coding::masked_crc32c(&[payload, &[compression]]);
        if actual_crc != expected_crc {
            return Err(LevelDbError::corruption(format!(
                "table {} block checksum mismatch at offset {offset}",
                path.display()
            )));
        }
    }
    decompress_payload(compression, payload)
}

fn read_native_block_value<'a>(
    path: &Path,
    table_bytes: &'a [u8],
    handle: BlockHandle,
    paranoid_checks: bool,
) -> Result<BlockValue<'a>> {
    let offset = usize::try_from(handle.offset).map_err(|_| {
        LevelDbError::corruption(format!(
            "table {} block offset overflows usize",
            path.display()
        ))
    })?;
    let size = usize::try_from(handle.size).map_err(|_| {
        LevelDbError::corruption(format!(
            "table {} block size overflows usize",
            path.display()
        ))
    })?;
    let trailer_offset = offset.checked_add(size).ok_or_else(|| {
        LevelDbError::corruption(format!("table {} block range overflows", path.display()))
    })?;
    let end = trailer_offset
        .checked_add(LEVELDB_BLOCK_TRAILER_LEN)
        .ok_or_else(|| {
            LevelDbError::corruption(format!("table {} block trailer overflows", path.display()))
        })?;
    if end > table_bytes.len() {
        return Err(LevelDbError::corruption(format!(
            "table {} block is truncated at offset {offset}",
            path.display()
        )));
    }
    let payload = &table_bytes[offset..trailer_offset];
    let compression = table_bytes[trailer_offset];
    if paranoid_checks {
        let expected_crc = u32::from_le_bytes(
            table_bytes[trailer_offset + 1..end]
                .try_into()
                .map_err(|_| {
                    LevelDbError::corruption(format!(
                        "table {} block crc is truncated",
                        path.display()
                    ))
                })?,
        );
        let actual_crc = crate::coding::masked_crc32c(&[payload, &[compression]]);
        if actual_crc != expected_crc {
            return Err(LevelDbError::corruption(format!(
                "table {} block checksum mismatch at offset {offset}",
                path.display()
            )));
        }
    }
    if compression == COMPRESSION_NONE {
        Ok(BlockValue::Borrowed(payload))
    } else {
        Ok(BlockValue::Shared(Bytes::from(decompress_payload(
            compression,
            payload,
        )?)))
    }
}

fn read_native_block_from_file(
    file: &File,
    path: &Path,
    handle: BlockHandle,
    paranoid_checks: bool,
    cache: Option<&NativeBlockCache>,
) -> Result<Bytes> {
    let cache_key = NativeBlockCacheKey {
        table_id: table_id(path),
        offset: handle.offset,
        size: handle.size,
        paranoid_checks,
    };
    if let Some(block) = cache.and_then(|cache| cache.get(&cache_key)) {
        return Ok(block);
    }

    let size = usize::try_from(handle.size).map_err(|_| {
        LevelDbError::corruption(format!(
            "table {} block size overflows usize",
            path.display()
        ))
    })?;
    let total_size = size.checked_add(LEVELDB_BLOCK_TRAILER_LEN).ok_or_else(|| {
        LevelDbError::corruption(format!("table {} block trailer overflows", path.display()))
    })?;
    let mut block = vec![0_u8; total_size];
    read_exact_at(file, &mut block, handle.offset)?;

    let payload = &block[..size];
    let compression = block[size];
    if paranoid_checks {
        let expected_crc = u32::from_le_bytes(
            block[size + 1..size + LEVELDB_BLOCK_TRAILER_LEN]
                .try_into()
                .map_err(|_| {
                    LevelDbError::corruption(format!(
                        "table {} block crc is truncated",
                        path.display()
                    ))
                })?,
        );
        let actual_crc = crate::coding::masked_crc32c(&[payload, &[compression]]);
        if actual_crc != expected_crc {
            return Err(LevelDbError::corruption(format!(
                "table {} block checksum mismatch at offset {}",
                path.display(),
                handle.offset
            )));
        }
    }
    let block = if compression == COMPRESSION_NONE {
        block.truncate(size);
        Bytes::from(block)
    } else {
        Bytes::from(decompress_payload(compression, payload)?)
    };
    if let Some(cache) = cache {
        cache.insert(cache_key, block.clone());
    }
    Ok(block)
}

fn native_block_entries_end(block: &[u8]) -> Result<usize> {
    if block.len() < 4 {
        return Err(LevelDbError::corruption(
            "native block is missing restart count".to_string(),
        ));
    }
    let restart_count_offset = block.len() - 4;
    let restart_count = usize::try_from(u32::from_le_bytes(
        block[restart_count_offset..].try_into().map_err(|_| {
            LevelDbError::corruption("native block restart count is invalid".to_string())
        })?,
    ))
    .map_err(|_| LevelDbError::corruption("native block restart count overflow".to_string()))?;
    let restart_bytes = restart_count.checked_mul(4).ok_or_else(|| {
        LevelDbError::corruption("native block restart array overflow".to_string())
    })?;
    if restart_bytes > restart_count_offset {
        return Err(LevelDbError::corruption(
            "native block restart array is truncated".to_string(),
        ));
    }
    Ok(restart_count_offset - restart_bytes)
}

fn decode_native_block_entries_bytes(block: &Bytes) -> Result<Vec<(Vec<u8>, Bytes)>> {
    let mut entries = Vec::new();
    decode_native_block_entry_ranges(block.as_ref(), |key, value_range| {
        entries.push((key.to_vec(), block.slice(value_range)));
        Ok(VisitorControl::Continue)
    })?;
    Ok(entries)
}

fn decode_native_index_entries(block: &Bytes) -> Result<NativeIndexEntries> {
    let mut entries = Vec::new();
    decode_native_block_entry_ranges(block.as_ref(), |key, value_range| {
        entries.push(NativeIndexEntry {
            key: Bytes::copy_from_slice(key),
            value: block.slice(value_range),
        });
        Ok(VisitorControl::Continue)
    })?;
    Ok(entries.into())
}

fn decode_native_block_entry_ranges<F>(block: &[u8], mut visitor: F) -> Result<VisitorControl>
where
    F: FnMut(&[u8], Range<usize>) -> Result<VisitorControl>,
{
    let entries_end = native_block_entries_end(block)?;
    let mut input = &block[..entries_end];
    let mut key = Vec::new();
    while !input.is_empty() {
        let shared = usize::try_from(get_varint32(&mut input)?).map_err(|_| {
            LevelDbError::corruption("native block shared key length overflow".to_string())
        })?;
        let non_shared = usize::try_from(get_varint32(&mut input)?).map_err(|_| {
            LevelDbError::corruption("native block key delta length overflow".to_string())
        })?;
        let value_len = usize::try_from(get_varint32(&mut input)?).map_err(|_| {
            LevelDbError::corruption("native block value length overflow".to_string())
        })?;
        if shared > key.len() {
            return Err(LevelDbError::corruption(
                "native block shared prefix exceeds previous key".to_string(),
            ));
        }
        if input.len() < non_shared.saturating_add(value_len) {
            return Err(LevelDbError::corruption(
                "native block entry is truncated".to_string(),
            ));
        }
        key.truncate(shared);
        key.extend_from_slice(&input[..non_shared]);
        input = &input[non_shared..];
        let value_start = entries_end.saturating_sub(input.len());
        let value_end = value_start.checked_add(value_len).ok_or_else(|| {
            LevelDbError::corruption("native block value range overflow".to_string())
        })?;
        input = &input[value_len..];
        if visitor(&key, value_start..value_end)? == VisitorControl::Stop {
            return Ok(VisitorControl::Stop);
        }
    }

    Ok(VisitorControl::Continue)
}

fn is_next_user_key(previous: &mut Vec<u8>, user_key: &[u8]) -> bool {
    if previous.as_slice() == user_key {
        return false;
    }
    previous.clear();
    previous.extend_from_slice(user_key);
    true
}

fn collect_native_block_entries(block: &BlockValue<'_>) -> Result<Vec<(Vec<u8>, Bytes)>> {
    let mut entries = Vec::new();
    decode_native_block_entries_ref(block, |key, value| {
        entries.push((key.to_vec(), Bytes::copy_from_slice(value.as_bytes())));
        Ok(VisitorControl::Continue)
    })?;
    Ok(entries)
}

fn decode_native_block_entries_ref<F>(
    block: &BlockValue<'_>,
    mut visitor: F,
) -> Result<VisitorControl>
where
    F: FnMut(&[u8], ValueRef<'_>) -> Result<VisitorControl>,
{
    let block_bytes = block.as_bytes();
    decode_native_block_entry_ranges(block_bytes, |key, value_range| {
        let value = block.value_ref(&block_bytes[value_range])?;
        visitor(key, value)
    })
}

const fn mark_table_scanned(mut outcome: ScanOutcome) -> ScanOutcome {
    outcome.tables_scanned = outcome.tables_scanned.saturating_add(1);
    outcome
}

fn split_internal_key(internal_key: &[u8]) -> Option<(&[u8], bool)> {
    let (user_key, trailer) = internal_key.split_at_checked(internal_key.len().checked_sub(8)?)?;
    if trailer.len() != 8 {
        return None;
    }
    let tag = u64::from_le_bytes([
        trailer[0], trailer[1], trailer[2], trailer[3], trailer[4], trailer[5], trailer[6],
        trailer[7],
    ]);
    match (tag & 0xff) as u8 {
        crate::coding::VALUE_TYPE_VALUE => Some((user_key, true)),
        crate::coding::VALUE_TYPE_DELETION => Some((user_key, false)),
        _ => None,
    }
}

fn internal_key(user_key: &[u8], sequence: u64, value_type: u8) -> Vec<u8> {
    let mut key = Vec::with_capacity(user_key.len().saturating_add(8));
    key.extend_from_slice(user_key);
    key.extend_from_slice(&((sequence << 8) | u64::from(value_type)).to_le_bytes());
    key
}

fn encode_native_block(entries: &[(Vec<u8>, Bytes)]) -> Result<Vec<u8>> {
    let mut builder = NativeBlockBuilder::new();
    for (key, value) in entries {
        builder.add(key, value)?;
    }
    builder.finish()
}

fn native_footer(meta_index: BlockHandle, index: BlockHandle) -> Vec<u8> {
    let mut handles = Vec::new();
    write_block_handle(meta_index, &mut handles);
    write_block_handle(index, &mut handles);
    handles.resize(LEVELDB_FOOTER_LEN - 8, 0);
    handles.extend_from_slice(&LEVELDB_TABLE_MAGIC.to_le_bytes());
    handles
}

fn compression_tag(policy: CompressionPolicy) -> u8 {
    match policy {
        CompressionPolicy::None => COMPRESSION_NONE,
        CompressionPolicy::Snappy => COMPRESSION_SNAPPY,
        CompressionPolicy::Zlib => COMPRESSION_ZLIB,
        CompressionPolicy::RawDeflate => COMPRESSION_BEDROCK_ZLIB,
    }
}

fn compress_payload(policy: CompressionPolicy, payload: &[u8]) -> Result<Vec<u8>> {
    match policy {
        CompressionPolicy::None => Ok(payload.to_vec()),
        CompressionPolicy::Snappy => compress_snappy(payload),
        CompressionPolicy::Zlib => compress_zlib(payload),
        CompressionPolicy::RawDeflate => compress_deflate(payload),
    }
}

fn decompress_payload(tag: u8, payload: &[u8]) -> Result<Vec<u8>> {
    match tag {
        COMPRESSION_NONE => Ok(payload.to_vec()),
        COMPRESSION_SNAPPY => decompress_snappy(payload),
        COMPRESSION_ZLIB => decompress_zlib(payload),
        COMPRESSION_BEDROCK_ZLIB => decompress_deflate(payload),
        other => Err(LevelDbError::compression(
            "table",
            format!("unknown table compression tag {other}"),
        )),
    }
}

#[cfg(feature = "snappy")]
fn compress_snappy(payload: &[u8]) -> Result<Vec<u8>> {
    snap::raw::Encoder::new()
        .compress_vec(payload)
        .map_err(|error| LevelDbError::compression("table", error.to_string()))
}

#[cfg(not(feature = "snappy"))]
fn compress_snappy(_payload: &[u8]) -> Result<Vec<u8>> {
    Err(LevelDbError::unsupported(
        "snappy",
        "snappy feature is disabled",
    ))
}

#[cfg(feature = "snappy")]
fn decompress_snappy(payload: &[u8]) -> Result<Vec<u8>> {
    snap::raw::Decoder::new()
        .decompress_vec(payload)
        .map_err(|error| LevelDbError::compression("table", error.to_string()))
}

#[cfg(not(feature = "snappy"))]
fn decompress_snappy(_payload: &[u8]) -> Result<Vec<u8>> {
    Err(LevelDbError::unsupported(
        "snappy",
        "snappy feature is disabled",
    ))
}

#[cfg(feature = "zlib")]
fn compress_zlib(payload: &[u8]) -> Result<Vec<u8>> {
    use flate2::{Compression, write::ZlibEncoder};
    use std::io::Write;

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(payload)?;
    encoder
        .finish()
        .map_err(|error| LevelDbError::compression("table", error.to_string()))
}

#[cfg(not(feature = "zlib"))]
fn compress_zlib(_payload: &[u8]) -> Result<Vec<u8>> {
    Err(LevelDbError::unsupported(
        "zlib",
        "zlib feature is disabled",
    ))
}

#[cfg(feature = "zlib")]
fn compress_deflate(payload: &[u8]) -> Result<Vec<u8>> {
    use flate2::{Compression, write::DeflateEncoder};
    use std::io::Write;

    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(payload)?;
    encoder
        .finish()
        .map_err(|error| LevelDbError::compression("table", error.to_string()))
}

#[cfg(not(feature = "zlib"))]
fn compress_deflate(_payload: &[u8]) -> Result<Vec<u8>> {
    Err(LevelDbError::unsupported(
        "zlib",
        "zlib feature is disabled",
    ))
}

#[cfg(feature = "zlib")]
fn decompress_zlib(payload: &[u8]) -> Result<Vec<u8>> {
    use flate2::read::ZlibDecoder;
    use std::io::Read;

    let mut decoder = ZlibDecoder::new(payload);
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(|error| LevelDbError::compression("table", error.to_string()))?;
    Ok(out)
}

#[cfg(not(feature = "zlib"))]
fn decompress_zlib(_payload: &[u8]) -> Result<Vec<u8>> {
    Err(LevelDbError::unsupported(
        "zlib",
        "zlib feature is disabled",
    ))
}

#[cfg(feature = "zlib")]
fn decompress_deflate(payload: &[u8]) -> Result<Vec<u8>> {
    use flate2::read::DeflateDecoder;
    use std::io::Read;

    let mut decoder = DeflateDecoder::new(payload);
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(|error| LevelDbError::compression("table", error.to_string()))?;
    Ok(out)
}

#[cfg(not(feature = "zlib"))]
fn decompress_deflate(_payload: &[u8]) -> Result<Vec<u8>> {
    Err(LevelDbError::unsupported(
        "zlib",
        "zlib feature is disabled",
    ))
}

fn replace_file(tmp_path: &Path, path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_file(path).map_err(|error| LevelDbError::io_at("replace table", path, error))?;
    }
    fs::rename(tmp_path, path)
        .map_err(|error| LevelDbError::io_at("rename table temp file", path, error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_table_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "bedrock-leveldb-{name}-{}.ldb",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ))
    }

    #[test]
    fn table_roundtrips_without_compression() {
        let path = temp_table_path("table");
        let mut entries = BTreeMap::new();
        entries.insert(b"a".to_vec(), Bytes::from_static(b"one"));
        entries.insert(b"b".to_vec(), Bytes::from_static(b"two"));

        write_table(&path, &entries, CompressionPolicy::None).expect("write");
        let decoded = read_table(&path, true).expect("read");
        assert_eq!(decoded, entries);
        std::fs::remove_file(path).expect("cleanup");
    }

    #[cfg(feature = "zlib")]
    #[test]
    fn raw_deflate_policy_uses_bedrock_tag_and_roundtrips() {
        let payload = b"bedrock raw deflate compression payload";
        assert_eq!(
            compression_tag(CompressionPolicy::RawDeflate),
            COMPRESSION_BEDROCK_ZLIB
        );
        let encoded = compress_payload(CompressionPolicy::RawDeflate, payload).expect("compress");
        assert_eq!(
            decompress_payload(COMPRESSION_BEDROCK_ZLIB, &encoded).expect("decompress"),
            payload
        );
    }

    #[test]
    fn custom_table_key_scan_rejects_truncated_header_without_panic() {
        let path = temp_table_path("truncated-custom-table");
        std::fs::write(&path, TABLE_MAGIC).expect("write truncated custom table");

        let result = for_each_table_key(&path, true, None, |_| Ok(VisitorControl::Continue));

        assert!(
            matches!(result, Err(LevelDbError::Corruption { .. })),
            "expected corruption error, got {result:?}"
        );
        std::fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn cached_table_opens_reuse_positional_file_handle() {
        let path = temp_table_path("cached-positional-file");
        std::fs::write(&path, b"0123456789").expect("write table bytes");
        let cache = NativeBlockCache::new(1024, 1024, 8, 4);
        let first = cache.open_table_file(&path).expect("open first handle");
        let second = cache.open_table_file(&path).expect("open second handle");
        let mut first_byte = [0_u8; 1];
        let mut second_byte = [0_u8; 1];
        read_exact_at(&first, &mut first_byte, 1).expect("read first position");
        read_exact_at(&second, &mut second_byte, 4).expect("read second position");

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(first_byte, [b'1']);
        assert_eq!(second_byte, [b'4']);
        std::fs::remove_file(path).expect("cleanup");
    }

    #[test]
    fn native_writer_splits_large_tables_into_indexed_data_blocks() {
        let path = temp_table_path("multi-block-native");
        let mut entries = BTreeMap::new();
        for index in 0..256 {
            entries.insert(
                format!("key:{index:04}").into_bytes(),
                Bytes::from(vec![u8::try_from(index).expect("small index"); 512]),
            );
        }

        write_native_table(&path, &entries, 7, CompressionPolicy::None).expect("write native");
        let bytes = std::fs::read(&path).expect("read native table");
        let index_entries = read_native_index_entries_bytes(&path, &bytes, true)
            .expect("read native index entries");

        assert!(index_entries.len() > 1);
        assert_eq!(
            get_table_entry(&path, b"key:0192", true, None).expect("point get"),
            Some(Bytes::from(vec![192_u8; 512]))
        );
        std::fs::remove_file(path).expect("cleanup");
    }
}
