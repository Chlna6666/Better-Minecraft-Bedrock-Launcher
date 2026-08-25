use crate::coding::{get_varint32, get_varint64, masked_crc32c};
use crate::compression::{COMPRESSION_NONE, decompress_append, decompress_into};
use crate::error::{LevelDbError, Result};
use crate::manifest::{Manifest, TableFileMeta};
use crate::options::{ReadOptions, ScanMode, ScanOutcome, VisitorControl};
use crate::table_cursor::BorrowedTableCursor;
use rayon::ScopeFifo;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, Sender, channel},
};

const CUSTOM_TABLE_MAGIC: &[u8; 9] = b"BWLDBTBL1";
const LEVELDB_TABLE_MAGIC: u64 = 0xdb47_7524_8b80_fb57;
const LEVELDB_FOOTER_LEN: usize = 48;
const LEVELDB_BLOCK_TRAILER_LEN: usize = 5;

const RUN_TARGET_ENCODED_BYTES: u64 =
    if cfg!(any(windows, target_os = "linux", target_os = "android")) {
        1024 * 1024
    } else {
        512 * 1024
    };
const RUN_MAX_BLOCKS: usize = if cfg!(any(windows, target_os = "linux", target_os = "android")) {
    256
} else {
    128
};
const MAX_PREFETCH_RUNS_PER_TABLE: usize = 32;
const MIN_RUN_BUFFER_POOL: usize = 2;

#[cfg(windows)]
const WINDOWS_WORKER_FILE_CACHE_CAPACITY: usize = 8;

thread_local! {
    static RUN_READ_SCRATCH: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(
        RUN_TARGET_ENCODED_BYTES as usize + LEVELDB_BLOCK_TRAILER_LEN,
    ));
}

#[cfg(windows)]
thread_local! {
    static WINDOWS_WORKER_FILES: RefCell<WindowsWorkerFileCache> =
        RefCell::new(WindowsWorkerFileCache::default());
}

#[derive(Debug, Clone, Copy)]
struct BlockHandle {
    offset: u64,
    size: u64,
}

#[derive(Debug, Clone, Copy)]
struct BlockRun {
    first_block: usize,
    block_count: usize,
    offset: u64,
    encoded_len: u64,
}

struct NativeTablePlan {
    rank: usize,
    path: PathBuf,
    file: Arc<File>,
    blocks: Vec<BlockHandle>,
    runs: Vec<BlockRun>,
    lower: Option<Vec<u8>>,
    upper: Option<Vec<u8>>,
}

impl NativeTablePlan {
    #[allow(clippy::too_many_arguments)]
    fn open(
        root: &Path,
        table: &TableFileMeta,
        rank: usize,
        lower: Option<&[u8]>,
        upper: Option<&[u8]>,
        paranoid_checks: bool,
        planning: &mut PlanningScratch,
    ) -> Result<PlanOpen> {
        if !table_overlaps(table, lower, upper) {
            return Ok(PlanOpen::Skip);
        }
        let path = root.join(Manifest::table_name(table.number));
        if !path.exists() {
            return Ok(PlanOpen::Skip);
        }
        let file = File::open(&path)
            .map_err(|error| LevelDbError::io_at("open planned table", &path, error))?;
        let mut magic = [0_u8; CUSTOM_TABLE_MAGIC.len()];
        let read = read_at(&file, &mut magic, 0)
            .map_err(|error| LevelDbError::io_at("read planned table header", &path, error))?;
        if read == CUSTOM_TABLE_MAGIC.len() && magic == *CUSTOM_TABLE_MAGIC {
            return Ok(PlanOpen::LegacyTable);
        }

        let footer = read_footer(&file, &path)?;
        validate_footer_magic(&footer, &path)?;
        let magic_offset = LEVELDB_FOOTER_LEN - 8;
        let mut footer_input = &footer[..magic_offset];
        let _meta_index = read_block_handle(&mut footer_input)?;
        let index_handle = read_block_handle(&mut footer_input)?;

        planning.reset();
        read_one_block(
            &file,
            &path,
            index_handle,
            paranoid_checks,
            &mut planning.encoded,
            &mut planning.decoded,
        )?;
        let blocks =
            decode_index_handles(&planning.decoded, lower, upper, &mut planning.decoder_key)?;
        if blocks.is_empty() {
            return Ok(PlanOpen::Skip);
        }
        let runs = plan_block_runs(&blocks)?;
        if runs.is_empty() {
            return Ok(PlanOpen::Skip);
        }

        Ok(PlanOpen::Native(Self {
            rank,
            path,
            file: Arc::new(file),
            blocks,
            runs,
            lower: lower.map(<[u8]>::to_vec),
            upper: upper.map(<[u8]>::to_vec),
        }))
    }
}

enum PlanOpen {
    Native(NativeTablePlan),
    LegacyTable,
    Skip,
}

enum PreparedScan {
    Native {
        plans: Vec<Arc<NativeTablePlan>>,
        workers: usize,
    },
    Sequential,
    Empty,
}

#[derive(Default)]
struct PlanningScratch {
    encoded: Vec<u8>,
    decoded: Vec<u8>,
    decoder_key: Vec<u8>,
}

impl PlanningScratch {
    fn reset(&mut self) {
        self.encoded.clear();
        self.decoded.clear();
        self.decoder_key.clear();
    }
}

#[derive(Debug, Clone, Copy)]
struct BlockEntry {
    key_start: u32,
    key_len: u32,
    value_start: u32,
    value_len: u32,
    is_value: bool,
}

#[derive(Debug, Clone, Copy)]
struct BlockMeta {
    entry_start: u32,
    entry_len: u32,
}

#[derive(Default)]
struct RunBuffers {
    decoded: Vec<u8>,
    keys: Vec<u8>,
    entries: Vec<BlockEntry>,
    block_meta: Vec<BlockMeta>,
    decoder_key: Vec<u8>,
    previous_user_key: Vec<u8>,
}

impl RunBuffers {
    fn prepare(&mut self, block_count: usize) {
        self.decoded.clear();
        self.keys.clear();
        self.entries.clear();
        self.block_meta.clear();
        self.decoder_key.clear();
        self.previous_user_key.clear();
        if self.block_meta.capacity() < block_count {
            self.block_meta.reserve(block_count);
        }
    }

    fn block_entry_count(&self, block_index: usize) -> usize {
        self.block_meta[block_index].entry_len as usize
    }

    fn entry(&self, block_index: usize, entry_index: usize) -> BlockEntry {
        let meta = self.block_meta[block_index];
        self.entries[meta.entry_start as usize + entry_index]
    }

    fn key(&self, block_index: usize, entry_index: usize) -> &[u8] {
        let entry = self.entry(block_index, entry_index);
        let start = entry.key_start as usize;
        let end = start + entry.key_len as usize;
        &self.keys[start..end]
    }

    fn value(&self, block_index: usize, entry_index: usize) -> &[u8] {
        let entry = self.entry(block_index, entry_index);
        let start = entry.value_start as usize;
        let end = start + entry.value_len as usize;
        &self.decoded[start..end]
    }

    fn is_value(&self, block_index: usize, entry_index: usize) -> bool {
        self.entry(block_index, entry_index).is_value
    }
}

struct DecodedRun {
    table_index: usize,
    run_index: usize,
    buffers: RunBuffers,
}

impl DecodedRun {
    fn block_count(&self) -> usize {
        self.buffers.block_meta.len()
    }

    fn block_entry_count(&self, block_index: usize) -> usize {
        self.buffers.block_entry_count(block_index)
    }

    fn key(&self, block_index: usize, entry_index: usize) -> &[u8] {
        self.buffers.key(block_index, entry_index)
    }

    fn value(&self, block_index: usize, entry_index: usize) -> &[u8] {
        self.buffers.value(block_index, entry_index)
    }

    fn is_value(&self, block_index: usize, entry_index: usize) -> bool {
        self.buffers.is_value(block_index, entry_index)
    }
}

#[derive(Default)]
struct RunBufferPool {
    buffers: Mutex<Vec<RunBuffers>>,
}

impl RunBufferPool {
    fn with_count(count: usize) -> Self {
        let mut buffers = Vec::with_capacity(count);
        buffers.resize_with(count, RunBuffers::default);
        Self {
            buffers: Mutex::new(buffers),
        }
    }

    fn take(&self) -> RunBuffers {
        self.buffers
            .lock()
            .ok()
            .and_then(|mut buffers| buffers.pop())
            .unwrap_or_default()
    }

    fn recycle(&self, mut buffers: RunBuffers) {
        buffers.decoded.clear();
        buffers.keys.clear();
        buffers.entries.clear();
        buffers.block_meta.clear();
        buffers.decoder_key.clear();
        buffers.previous_user_key.clear();
        if let Ok(mut pool) = self.buffers.lock() {
            pool.push(buffers);
        }
    }
}

enum DecodeMessage {
    Run(DecodedRun),
    Error(LevelDbError),
}

struct ResultRouter {
    receiver: Receiver<DecodeMessage>,
    pending: HashMap<(usize, usize), DecodedRun>,
}

impl ResultRouter {
    fn new(receiver: Receiver<DecodeMessage>, max_inflight: usize) -> Self {
        Self {
            receiver,
            pending: HashMap::with_capacity(max_inflight.max(1)),
        }
    }

    fn wait_for(&mut self, table_index: usize, run_index: usize) -> Result<DecodedRun> {
        let requested = (table_index, run_index);
        if let Some(run) = self.pending.remove(&requested) {
            return Ok(run);
        }
        loop {
            match self.receiver.recv().map_err(|_| {
                LevelDbError::corruption("parallel run worker stopped before producing a result")
            })? {
                DecodeMessage::Run(run) => {
                    let id = (run.table_index, run.run_index);
                    if id == requested {
                        return Ok(run);
                    }
                    if self.pending.insert(id, run).is_some() {
                        return Err(LevelDbError::corruption(
                            "parallel block planner produced a duplicate run result",
                        ));
                    }
                }
                DecodeMessage::Error(error) => return Err(error),
            }
        }
    }
}

struct TableLane {
    plan: Arc<NativeTablePlan>,
    next_run: usize,
    scheduled_until: usize,
    current: Option<DecodedRun>,
    block_index: usize,
    entry_index: usize,
    previous_block_last_key: Option<Vec<u8>>,
    current_is_value: bool,
}

impl TableLane {
    fn new(plan: Arc<NativeTablePlan>) -> Self {
        Self {
            plan,
            next_run: 0,
            scheduled_until: 0,
            current: None,
            block_index: 0,
            entry_index: 0,
            previous_block_last_key: None,
            current_is_value: false,
        }
    }

    fn current_entry_index(&self) -> Option<usize> {
        self.entry_index.checked_sub(1)
    }

    fn current_key(&self) -> Option<&[u8]> {
        let run = self.current.as_ref()?;
        Some(run.key(self.block_index, self.current_entry_index()?))
    }

    fn current_value(&self) -> Option<&[u8]> {
        if !self.current_is_value {
            return None;
        }
        let run = self.current.as_ref()?;
        Some(run.value(self.block_index, self.current_entry_index()?))
    }
}

struct SequentialSource {
    cursor: BorrowedTableCursor,
    key: Vec<u8>,
    is_value: bool,
    rank: usize,
}

#[cfg(windows)]
#[derive(Default)]
struct WindowsWorkerFileCache {
    files: Vec<(PathBuf, File)>,
    next_evict: usize,
}

#[cfg(windows)]
impl WindowsWorkerFileCache {
    fn read_run(&mut self, path: &Path, buffer: &mut [u8], offset: u64) -> std::io::Result<()> {
        let index = if let Some(index) = self
            .files
            .iter()
            .position(|(cached_path, _)| cached_path.as_path() == path)
        {
            index
        } else {
            let file = open_windows_worker_file(path)?;
            if self.files.len() < WINDOWS_WORKER_FILE_CACHE_CAPACITY {
                self.files.push((path.to_path_buf(), file));
                self.files.len() - 1
            } else {
                let index = self.next_evict % WINDOWS_WORKER_FILE_CACHE_CAPACITY;
                self.files[index] = (path.to_path_buf(), file);
                self.next_evict = (index + 1) % WINDOWS_WORKER_FILE_CACHE_CAPACITY;
                index
            }
        };
        read_exact_at(&self.files[index].1, buffer, offset)
    }
}

pub(crate) fn scan_tables_visible<F, S>(
    root: &Path,
    tables_newest_first: &[TableFileMeta],
    prefix: Option<&[u8]>,
    paranoid_checks: bool,
    options: &ReadOptions,
    shadowed: S,
    visitor: &mut F,
) -> Result<ScanOutcome>
where
    F: FnMut(&[u8], &[u8]) -> Result<VisitorControl> + Send,
    S: Fn(&[u8]) -> bool + Sync,
{
    if options.scan_mode != ScanMode::ParallelTables {
        return scan_sequential(
            root,
            tables_newest_first,
            prefix,
            paranoid_checks,
            options,
            &shadowed,
            visitor,
        );
    }

    match prepare_native_scan(root, tables_newest_first, prefix, paranoid_checks, options)? {
        PreparedScan::Native { plans, workers } => {
            scan_parallel_runs(plans, paranoid_checks, options, workers, &shadowed, visitor)
        }
        PreparedScan::Sequential => scan_sequential(
            root,
            tables_newest_first,
            prefix,
            paranoid_checks,
            options,
            &shadowed,
            visitor,
        ),
        PreparedScan::Empty => {
            let mut outcome = ScanOutcome::empty();
            outcome.worker_threads = 1;
            Ok(outcome)
        }
    }
}

pub(crate) fn scan_table_keys_partitioned<T, I, F, S>(
    root: &Path,
    tables_newest_first: &[TableFileMeta],
    prefix: Option<&[u8]>,
    paranoid_checks: bool,
    options: &ReadOptions,
    shadowed: S,
    init: I,
    visitor: F,
) -> Result<(ScanOutcome, Vec<T>)>
where
    T: Send,
    I: Fn() -> T + Send + Sync,
    F: Fn(&mut T, &[u8]) -> Result<VisitorControl> + Send + Sync,
    S: Fn(&[u8]) -> bool + Send + Sync,
{
    if options.scan_mode != ScanMode::ParallelTables {
        let mut partition = init();
        let outcome = scan_sequential(
            root,
            tables_newest_first,
            prefix,
            paranoid_checks,
            options,
            &shadowed,
            &mut |key, _value| visitor(&mut partition, key),
        )?;
        return Ok((outcome, vec![partition]));
    }

    match prepare_native_scan(root, tables_newest_first, prefix, paranoid_checks, options)? {
        PreparedScan::Native { plans, workers } => {
            let mut partitions = (0..workers).map(|_| init()).collect::<Vec<_>>();
            let visitor_ref = &visitor;
            let mut reduce = |key: &[u8], _value: &[u8]| {
                let partition = partition_for_key(key, partitions.len());
                visitor_ref(&mut partitions[partition], key)
            };
            let outcome = scan_parallel_runs(
                plans,
                paranoid_checks,
                options,
                workers,
                &shadowed,
                &mut reduce,
            )?;
            Ok((outcome, partitions))
        }
        PreparedScan::Sequential => {
            let mut partition = init();
            let outcome = scan_sequential(
                root,
                tables_newest_first,
                prefix,
                paranoid_checks,
                options,
                &shadowed,
                &mut |key, _value| visitor(&mut partition, key),
            )?;
            Ok((outcome, vec![partition]))
        }
        PreparedScan::Empty => {
            let mut outcome = ScanOutcome::empty();
            outcome.worker_threads = 1;
            Ok((outcome, vec![init()]))
        }
    }
}

fn scan_sequential<F, S>(
    root: &Path,
    tables_newest_first: &[TableFileMeta],
    prefix: Option<&[u8]>,
    paranoid_checks: bool,
    options: &ReadOptions,
    shadowed: &S,
    visitor: &mut F,
) -> Result<ScanOutcome>
where
    F: FnMut(&[u8], &[u8]) -> Result<VisitorControl>,
    S: Fn(&[u8]) -> bool,
{
    let (lower, upper) = prefix_bounds(prefix);
    let mut sources = open_sequential_sources(
        root,
        tables_newest_first,
        lower.as_deref(),
        upper.as_deref(),
        paranoid_checks,
    )?;
    let mut heap = Vec::<usize>::with_capacity(sources.len());
    for index in 0..sources.len() {
        seq_heap_push(&mut heap, index, &sources);
    }

    let mut outcome = ScanOutcome::empty();
    outcome.worker_threads = 1;
    outcome.tables_scanned = sources.len();
    let progress_interval = options.pipeline.resolve_progress_interval().max(1);
    let mut same_sources = Vec::<usize>::with_capacity(sources.len().min(8));

    while !heap.is_empty() {
        check_cancelled(options, &mut outcome)?;
        let first = seq_heap_pop(&mut heap, &sources).expect("heap was checked as non-empty");
        same_sources.clear();
        same_sources.push(first);

        {
            let winner_key = sources[first].key.as_slice();
            while let Some(index) = heap.first().copied() {
                if sources[index].key.as_slice() != winner_key {
                    break;
                }
                same_sources
                    .push(seq_heap_pop(&mut heap, &sources).expect("equal heap root was checked"));
            }

            if !shadowed(winner_key) && sources[first].is_value {
                let value = sources[first].cursor.current_value().ok_or_else(|| {
                    LevelDbError::corruption("borrowed table cursor lost current value")
                })?;
                outcome.record(value.len());
                if visitor(winner_key, value)? == VisitorControl::Stop {
                    outcome.stopped = true;
                    return Ok(outcome);
                }
                emit_progress(options, &outcome, progress_interval);
            }
        }

        for source_index in same_sources.iter().copied() {
            advance_sequential_source(source_index, &mut sources, &mut heap)?;
        }
    }
    Ok(outcome)
}

fn open_sequential_sources(
    root: &Path,
    tables_newest_first: &[TableFileMeta],
    lower: Option<&[u8]>,
    upper: Option<&[u8]>,
    paranoid_checks: bool,
) -> Result<Vec<SequentialSource>> {
    let mut sources = Vec::with_capacity(tables_newest_first.len());
    for (rank, table) in tables_newest_first.iter().enumerate() {
        if !table_overlaps(table, lower, upper) {
            continue;
        }
        let path = root.join(Manifest::table_name(table.number));
        if !path.exists() {
            continue;
        }
        let mut cursor = BorrowedTableCursor::open_range(&path, paranoid_checks, lower, upper)?;
        let mut key = Vec::with_capacity(48);
        let Some(is_value) = cursor.next_key_into(&mut key)? else {
            continue;
        };
        sources.push(SequentialSource {
            cursor,
            key,
            is_value,
            rank,
        });
    }
    Ok(sources)
}

fn advance_sequential_source(
    index: usize,
    sources: &mut [SequentialSource],
    heap: &mut Vec<usize>,
) -> Result<()> {
    let has_next = {
        let source = &mut sources[index];
        match source.cursor.next_key_into(&mut source.key)? {
            Some(is_value) => {
                source.is_value = is_value;
                true
            }
            None => {
                source.key.clear();
                source.is_value = false;
                false
            }
        }
    };
    if has_next {
        seq_heap_push(heap, index, sources);
    }
    Ok(())
}

fn seq_heap_less(left: usize, right: usize, sources: &[SequentialSource]) -> bool {
    sources[left]
        .key
        .cmp(&sources[right].key)
        .then_with(|| sources[left].rank.cmp(&sources[right].rank))
        .is_lt()
}

fn seq_heap_push(heap: &mut Vec<usize>, index: usize, sources: &[SequentialSource]) {
    heap.push(index);
    let mut child = heap.len() - 1;
    while child != 0 {
        let parent = (child - 1) / 2;
        if !seq_heap_less(heap[child], heap[parent], sources) {
            break;
        }
        heap.swap(child, parent);
        child = parent;
    }
}

fn seq_heap_pop(heap: &mut Vec<usize>, sources: &[SequentialSource]) -> Option<usize> {
    let last = heap.pop()?;
    if heap.is_empty() {
        return Some(last);
    }
    let result = std::mem::replace(&mut heap[0], last);
    let mut parent = 0usize;
    loop {
        let left = parent.saturating_mul(2).saturating_add(1);
        if left >= heap.len() {
            break;
        }
        let right = left + 1;
        let mut smallest = left;
        if right < heap.len() && seq_heap_less(heap[right], heap[left], sources) {
            smallest = right;
        }
        if !seq_heap_less(heap[smallest], heap[parent], sources) {
            break;
        }
        heap.swap(parent, smallest);
        parent = smallest;
    }
    Some(result)
}

fn prepare_native_scan(
    root: &Path,
    tables_newest_first: &[TableFileMeta],
    prefix: Option<&[u8]>,
    paranoid_checks: bool,
    options: &ReadOptions,
) -> Result<PreparedScan> {
    let (lower, upper) = prefix_bounds(prefix);
    let mut planning = PlanningScratch::default();
    let mut plans = Vec::<Arc<NativeTablePlan>>::new();
    for (rank, table) in tables_newest_first.iter().enumerate() {
        match NativeTablePlan::open(
            root,
            table,
            rank,
            lower.as_deref(),
            upper.as_deref(),
            paranoid_checks,
            &mut planning,
        )? {
            PlanOpen::Native(plan) => plans.push(Arc::new(plan)),
            PlanOpen::LegacyTable => return Ok(PreparedScan::Sequential),
            PlanOpen::Skip => {}
        }
    }
    if plans.is_empty() {
        return Ok(PreparedScan::Empty);
    }
    let total_runs = plans.iter().map(|plan| plan.runs.len()).sum::<usize>();
    let workers = options.threading.resolve_checked(total_runs.max(1))?;
    if workers <= 1 {
        return Ok(PreparedScan::Sequential);
    }
    Ok(PreparedScan::Native { plans, workers })
}

fn target_inflight_runs(
    options: &ReadOptions,
    workers: usize,
    table_count: usize,
    total_runs: usize,
) -> usize {
    let automatic = workers.saturating_add(workers.div_ceil(2));
    let requested = if options.pipeline.queue_depth == 0 {
        automatic
    } else {
        options.pipeline.queue_depth
    };
    requested
        .max(table_count)
        .min(total_runs.max(1))
        .min(64)
        .max(1)
}

fn scan_parallel_runs<F, S>(
    plans: Vec<Arc<NativeTablePlan>>,
    paranoid_checks: bool,
    options: &ReadOptions,
    workers: usize,
    shadowed: &S,
    visitor: &mut F,
) -> Result<ScanOutcome>
where
    F: FnMut(&[u8], &[u8]) -> Result<VisitorControl> + Send,
    S: Fn(&[u8]) -> bool + Sync,
{
    let table_count = plans.len();
    let total_runs = plans.iter().map(|plan| plan.runs.len()).sum::<usize>();
    let target_inflight = target_inflight_runs(options, workers, table_count, total_runs);
    let prefetch = target_inflight
        .div_ceil(table_count.max(1))
        .clamp(1, MAX_PREFETCH_RUNS_PER_TABLE);
    let window_count = plans
        .iter()
        .map(|plan| plan.runs.len().min(prefetch))
        .sum::<usize>()
        .max(MIN_RUN_BUFFER_POOL);

    log::debug!(
        "parallel block-range scan plan (tables={}, runs={}, workers={}, inflight={}, prefetch_per_table={}, run_target_kib={})",
        table_count,
        total_runs,
        workers,
        window_count,
        prefetch,
        RUN_TARGET_ENCODED_BYTES / 1024
    );

    let pool = scan_pool(workers)?;
    let stop = Arc::new(AtomicBool::new(false));
    let buffers = Arc::new(RunBufferPool::with_count(window_count));
    let (sender, receiver) = channel::<DecodeMessage>();
    let progress_interval = options.pipeline.resolve_progress_interval().max(1);
    let cancel = options.cancel.clone();

    pool.scope_fifo(|scope| -> Result<ScanOutcome> {
        let mut lanes = plans
            .iter()
            .cloned()
            .map(TableLane::new)
            .collect::<Vec<_>>();
        seed_all_lanes(
            scope,
            prefetch,
            &mut lanes,
            paranoid_checks,
            &sender,
            &buffers,
            &stop,
            cancel.clone(),
        );

        let mut router = ResultRouter::new(receiver, window_count);
        let mut heap = Vec::<usize>::with_capacity(lanes.len());
        for index in 0..lanes.len() {
            if advance_lane(
                scope,
                index,
                &mut lanes,
                &mut router,
                paranoid_checks,
                &sender,
                &buffers,
                &stop,
                cancel.clone(),
            )? {
                heap_push(&mut heap, index, &lanes);
            }
        }

        let mut outcome = ScanOutcome::empty();
        outcome.worker_threads = workers;
        outcome.tables_scanned = plans.len();
        let mut same_lanes = Vec::<usize>::with_capacity(lanes.len().min(8));

        while !heap.is_empty() {
            check_cancelled(options, &mut outcome)?;
            let first = heap_pop(&mut heap, &lanes).expect("heap was checked as non-empty");
            same_lanes.clear();
            same_lanes.push(first);
            {
                let winner_key = lanes[first].current_key().ok_or_else(|| {
                    LevelDbError::corruption("parallel table lane lost current key")
                })?;
                while let Some(index) = heap.first().copied() {
                    if lanes[index].current_key() != Some(winner_key) {
                        break;
                    }
                    same_lanes
                        .push(heap_pop(&mut heap, &lanes).expect("equal heap root was checked"));
                }

                if !shadowed(winner_key) && lanes[first].current_is_value {
                    let value = lanes[first].current_value().ok_or_else(|| {
                        LevelDbError::corruption("parallel table lane lost current value")
                    })?;
                    outcome.record(value.len());
                    if visitor(winner_key, value)? == VisitorControl::Stop {
                        outcome.stopped = true;
                        stop.store(true, Ordering::Relaxed);
                        return Ok(outcome);
                    }
                    emit_progress(options, &outcome, progress_interval);
                }
            }

            for lane_index in same_lanes.iter().copied() {
                if advance_lane(
                    scope,
                    lane_index,
                    &mut lanes,
                    &mut router,
                    paranoid_checks,
                    &sender,
                    &buffers,
                    &stop,
                    cancel.clone(),
                )? {
                    heap_push(&mut heap, lane_index, &lanes);
                }
            }
        }
        stop.store(true, Ordering::Relaxed);
        Ok(outcome)
    })
}

#[allow(clippy::too_many_arguments)]
fn seed_all_lanes<'scope>(
    scope: &ScopeFifo<'scope>,
    prefetch: usize,
    lanes: &mut [TableLane],
    paranoid_checks: bool,
    sender: &Sender<DecodeMessage>,
    buffers: &Arc<RunBufferPool>,
    stop: &Arc<AtomicBool>,
    cancel: Option<crate::options::ScanCancelFlag>,
) {
    for _ in 0..prefetch {
        for table_index in 0..lanes.len() {
            let _ = schedule_next_run(
                scope,
                table_index,
                lanes,
                paranoid_checks,
                sender,
                buffers,
                stop,
                cancel.clone(),
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn schedule_next_run<'scope>(
    scope: &ScopeFifo<'scope>,
    table_index: usize,
    lanes: &mut [TableLane],
    paranoid_checks: bool,
    sender: &Sender<DecodeMessage>,
    buffers: &Arc<RunBufferPool>,
    stop: &Arc<AtomicBool>,
    cancel: Option<crate::options::ScanCancelFlag>,
) -> bool {
    let lane = &mut lanes[table_index];
    if lane.scheduled_until >= lane.plan.runs.len() {
        return false;
    }
    let run_index = lane.scheduled_until;
    lane.scheduled_until = lane.scheduled_until.saturating_add(1);
    let plan = Arc::clone(&lane.plan);
    let sender = sender.clone();
    let buffers = Arc::clone(buffers);
    let stop = Arc::clone(stop);
    scope.spawn_fifo(move |_| {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        if cancel.as_ref().is_some_and(|flag| flag.is_cancelled()) {
            stop.store(true, Ordering::Relaxed);
            let _ = sender.send(DecodeMessage::Error(LevelDbError::cancelled(
                "parallel block-range scan",
            )));
            return;
        }

        let mut reusable = buffers.take();
        let result = RUN_READ_SCRATCH.with(|scratch| {
            let mut scratch = scratch.borrow_mut();
            decode_planned_run(
                &plan,
                run_index,
                paranoid_checks,
                &stop,
                cancel.as_ref(),
                &mut scratch,
                &mut reusable,
            )
        });
        match result {
            Ok(()) => {
                let _ = sender.send(DecodeMessage::Run(DecodedRun {
                    table_index,
                    run_index,
                    buffers: reusable,
                }));
            }
            Err(error) => {
                buffers.recycle(reusable);
                stop.store(true, Ordering::Relaxed);
                let _ = sender.send(DecodeMessage::Error(error));
            }
        }
    });
    true
}

#[allow(clippy::too_many_arguments)]
fn advance_lane<'scope>(
    scope: &ScopeFifo<'scope>,
    table_index: usize,
    lanes: &mut [TableLane],
    router: &mut ResultRouter,
    paranoid_checks: bool,
    sender: &Sender<DecodeMessage>,
    buffers: &Arc<RunBufferPool>,
    stop: &Arc<AtomicBool>,
    cancel: Option<crate::options::ScanCancelFlag>,
) -> Result<bool> {
    loop {
        {
            let lane = &mut lanes[table_index];
            if let Some(run) = lane.current.as_ref() {
                while lane.block_index < run.block_count() {
                    let entry_count = run.block_entry_count(lane.block_index);
                    while lane.entry_index < entry_count {
                        let entry_index = lane.entry_index;
                        lane.entry_index = lane.entry_index.saturating_add(1);
                        let key = run.key(lane.block_index, entry_index);
                        if entry_index == 0
                            && lane
                                .previous_block_last_key
                                .as_deref()
                                .is_some_and(|previous| previous == key)
                        {
                            continue;
                        }
                        lane.current_is_value = run.is_value(lane.block_index, entry_index);
                        return Ok(true);
                    }

                    if let Some(last_index) = entry_count.checked_sub(1) {
                        let key = run.key(lane.block_index, last_index);
                        let previous = lane
                            .previous_block_last_key
                            .get_or_insert_with(|| Vec::with_capacity(key.len().max(48)));
                        previous.clear();
                        previous.extend_from_slice(key);
                    }
                    lane.block_index = lane.block_index.saturating_add(1);
                    lane.entry_index = 0;
                }
            }
        }

        if let Some(run) = lanes[table_index].current.take() {
            buffers.recycle(run.buffers);
            let _ = schedule_next_run(
                scope,
                table_index,
                lanes,
                paranoid_checks,
                sender,
                buffers,
                stop,
                cancel.clone(),
            );
        }

        let next_run = lanes[table_index].next_run;
        if next_run >= lanes[table_index].plan.runs.len() {
            lanes[table_index].current_is_value = false;
            return Ok(false);
        }
        let next = router.wait_for(table_index, next_run)?;
        lanes[table_index].next_run = next_run.saturating_add(1);
        lanes[table_index].block_index = 0;
        lanes[table_index].entry_index = 0;
        lanes[table_index].current = Some(next);
    }
}

fn decode_planned_run(
    plan: &NativeTablePlan,
    run_index: usize,
    paranoid_checks: bool,
    stop: &AtomicBool,
    cancel: Option<&crate::options::ScanCancelFlag>,
    encoded_run: &mut Vec<u8>,
    output: &mut RunBuffers,
) -> Result<()> {
    let run = plan.runs[run_index];
    let encoded_len = usize::try_from(run.encoded_len)
        .map_err(|_| LevelDbError::corruption_at(&plan.path, "block run exceeds usize"))?;
    encoded_run.clear();
    encoded_run.resize(encoded_len, 0);
    read_planned_run(plan, encoded_run, run.offset)
        .map_err(|error| LevelDbError::io_at("read planned block run", &plan.path, error))?;

    output.prepare(run.block_count);
    let (decoded, keys, entries, block_meta, decoder_key, previous_user_key) = (
        &mut output.decoded,
        &mut output.keys,
        &mut output.entries,
        &mut output.block_meta,
        &mut output.decoder_key,
        &mut output.previous_user_key,
    );

    for local_block in 0..run.block_count {
        if stop.load(Ordering::Relaxed)
            || cancel.is_some_and(crate::options::ScanCancelFlag::is_cancelled)
        {
            return Err(LevelDbError::cancelled("parallel block-range decode"));
        }
        let global_block = run.first_block + local_block;
        let handle = plan.blocks[global_block];
        let relative = handle
            .offset
            .checked_sub(run.offset)
            .ok_or_else(|| LevelDbError::corruption_at(&plan.path, "block precedes run offset"))?;
        let relative = usize::try_from(relative)
            .map_err(|_| LevelDbError::corruption_at(&plan.path, "block offset exceeds usize"))?;
        let size = usize::try_from(handle.size)
            .map_err(|_| LevelDbError::corruption_at(&plan.path, "block size exceeds usize"))?;
        let trailer_end = relative
            .checked_add(size)
            .and_then(|end| end.checked_add(LEVELDB_BLOCK_TRAILER_LEN))
            .ok_or_else(|| LevelDbError::corruption_at(&plan.path, "block run range overflow"))?;
        if trailer_end > encoded_run.len() {
            return Err(LevelDbError::corruption_at(
                &plan.path,
                "planned block exceeds coalesced run",
            ));
        }
        let payload = &encoded_run[relative..relative + size];
        let compression_tag = encoded_run[relative + size];
        if paranoid_checks {
            let expected_crc = u32::from_le_bytes(
                encoded_run[relative + size + 1..trailer_end]
                    .try_into()
                    .map_err(|_| {
                        LevelDbError::corruption_at(&plan.path, "native block crc is invalid")
                    })?,
            );
            let actual_crc = masked_crc32c(&[payload, &[compression_tag]]);
            if actual_crc != expected_crc {
                return Err(LevelDbError::corruption_at(
                    &plan.path,
                    format!("native block checksum mismatch at offset {}", handle.offset),
                ));
            }
        }

        let decoded_start = decoded.len();
        decompress_append(compression_tag, payload, decoded)?;
        let decoded_end = decoded.len();
        let block = &decoded[decoded_start..decoded_end];

        decoder_key.clear();
        previous_user_key.clear();
        let entry_start = u32::try_from(entries.len())
            .map_err(|_| LevelDbError::corruption("run entry arena exceeds u32"))?;
        decode_block_entries(
            block,
            decoded_start,
            plan.lower.as_deref(),
            plan.upper.as_deref(),
            keys,
            entries,
            decoder_key,
            previous_user_key,
        )?;
        let entry_end = u32::try_from(entries.len())
            .map_err(|_| LevelDbError::corruption("run entry arena exceeds u32"))?;
        block_meta.push(BlockMeta {
            entry_start,
            entry_len: entry_end.saturating_sub(entry_start),
        });
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn decode_block_entries(
    decoded: &[u8],
    decoded_base: usize,
    lower: Option<&[u8]>,
    upper: Option<&[u8]>,
    keys: &mut Vec<u8>,
    entries: &mut Vec<BlockEntry>,
    decoder_key: &mut Vec<u8>,
    previous_user_key: &mut Vec<u8>,
) -> Result<()> {
    let entries_end = block_entries_end(decoded)?;
    let mut offset = 0usize;
    let mut has_previous_user_key = false;

    while offset < entries_end {
        let mut input = &decoded[offset..entries_end];
        let shared = usize::try_from(get_varint32(&mut input)?)
            .map_err(|_| LevelDbError::corruption("native block shared key length overflow"))?;
        let non_shared = usize::try_from(get_varint32(&mut input)?)
            .map_err(|_| LevelDbError::corruption("native block key delta length overflow"))?;
        let value_len = usize::try_from(get_varint32(&mut input)?)
            .map_err(|_| LevelDbError::corruption("native block value length overflow"))?;
        if shared > decoder_key.len() {
            return Err(LevelDbError::corruption(
                "native block shared prefix exceeds previous key",
            ));
        }
        if input.len() < non_shared.saturating_add(value_len) {
            return Err(LevelDbError::corruption("native block entry is truncated"));
        }
        decoder_key.truncate(shared);
        decoder_key.extend_from_slice(&input[..non_shared]);
        input = &input[non_shared..];
        let value_start = entries_end.saturating_sub(input.len());
        let value_end = value_start
            .checked_add(value_len)
            .ok_or_else(|| LevelDbError::corruption("native block value range overflow"))?;
        input = &input[value_len..];
        offset = entries_end.saturating_sub(input.len());

        let Some((user_key, is_value)) = split_internal_key(decoder_key) else {
            continue;
        };
        if has_previous_user_key && previous_user_key.as_slice() == user_key {
            continue;
        }
        previous_user_key.clear();
        previous_user_key.extend_from_slice(user_key);
        has_previous_user_key = true;
        if lower.is_some_and(|lower| user_key < lower) {
            continue;
        }
        if upper.is_some_and(|upper| user_key >= upper) {
            break;
        }

        let key_start = u32::try_from(keys.len())
            .map_err(|_| LevelDbError::corruption("run key arena exceeds u32"))?;
        let key_len = u32::try_from(user_key.len())
            .map_err(|_| LevelDbError::corruption("native user key exceeds u32"))?;
        let absolute_value_start = decoded_base
            .checked_add(value_start)
            .ok_or_else(|| LevelDbError::corruption("run decoded value offset overflow"))?;
        let value_start_u32 = u32::try_from(absolute_value_start)
            .map_err(|_| LevelDbError::corruption("native value offset exceeds u32"))?;
        let value_len_u32 = u32::try_from(value_end.saturating_sub(value_start))
            .map_err(|_| LevelDbError::corruption("native value length exceeds u32"))?;
        keys.extend_from_slice(user_key);
        entries.push(BlockEntry {
            key_start,
            key_len,
            value_start: value_start_u32,
            value_len: value_len_u32,
            is_value,
        });
    }
    Ok(())
}

fn decode_index_handles(
    block: &[u8],
    lower: Option<&[u8]>,
    upper: Option<&[u8]>,
    decoder_key: &mut Vec<u8>,
) -> Result<Vec<BlockHandle>> {
    let entries_end = block_entries_end(block)?;
    let mut handles = Vec::<BlockHandle>::new();
    decoder_key.clear();
    let mut offset = 0usize;
    let mut started = lower.is_none();

    while offset < entries_end {
        let mut input = &block[offset..entries_end];
        let shared = usize::try_from(get_varint32(&mut input)?)
            .map_err(|_| LevelDbError::corruption("native index shared key length overflow"))?;
        let non_shared = usize::try_from(get_varint32(&mut input)?)
            .map_err(|_| LevelDbError::corruption("native index key delta length overflow"))?;
        let value_len = usize::try_from(get_varint32(&mut input)?)
            .map_err(|_| LevelDbError::corruption("native index value length overflow"))?;
        if shared > decoder_key.len() || input.len() < non_shared.saturating_add(value_len) {
            return Err(LevelDbError::corruption(
                "native index block entry is truncated",
            ));
        }
        decoder_key.truncate(shared);
        decoder_key.extend_from_slice(&input[..non_shared]);
        input = &input[non_shared..];
        let value = &input[..value_len];
        input = &input[value_len..];
        offset = entries_end.saturating_sub(input.len());

        let limit_key = split_internal_key(decoder_key)
            .map_or(decoder_key.as_slice(), |(user_key, _)| user_key);
        if !started {
            if lower.is_some_and(|lower| limit_key < lower) {
                continue;
            }
            started = true;
        }

        let mut handle_input = value;
        handles.push(read_block_handle(&mut handle_input)?);
        if upper.is_some_and(|upper| limit_key >= upper) {
            break;
        }
    }
    Ok(handles)
}

fn plan_block_runs(blocks: &[BlockHandle]) -> Result<Vec<BlockRun>> {
    let Some(first) = blocks.first().copied() else {
        return Ok(Vec::new());
    };
    let mut runs = Vec::<BlockRun>::new();
    let mut first_block = 0usize;
    let mut run_offset = first.offset;
    let mut run_end = block_end(first)?;
    let mut block_count = 1usize;

    for (index, handle) in blocks.iter().copied().enumerate().skip(1) {
        let end = block_end(handle)?;
        let contiguous = handle.offset == run_end;
        let candidate_len = end.saturating_sub(run_offset);
        if contiguous && candidate_len <= RUN_TARGET_ENCODED_BYTES && block_count < RUN_MAX_BLOCKS {
            run_end = end;
            block_count = block_count.saturating_add(1);
            continue;
        }

        runs.push(BlockRun {
            first_block,
            block_count,
            offset: run_offset,
            encoded_len: run_end.saturating_sub(run_offset),
        });
        first_block = index;
        run_offset = handle.offset;
        run_end = end;
        block_count = 1;
    }

    runs.push(BlockRun {
        first_block,
        block_count,
        offset: run_offset,
        encoded_len: run_end.saturating_sub(run_offset),
    });
    Ok(runs)
}

fn block_end(handle: BlockHandle) -> Result<u64> {
    handle
        .offset
        .checked_add(handle.size)
        .and_then(|end| end.checked_add(LEVELDB_BLOCK_TRAILER_LEN as u64))
        .ok_or_else(|| LevelDbError::corruption("native block end offset overflow"))
}

fn partition_for_key(key: &[u8], partitions: usize) -> usize {
    if partitions <= 1 {
        return 0;
    }
    let first = u32::from(key.first().copied().unwrap_or(0));
    let second = u32::from(key.get(1).copied().unwrap_or(0));
    let last = u32::from(key.last().copied().unwrap_or(0));
    let len = u32::try_from(key.len()).unwrap_or(u32::MAX);
    let mixed = first | (second << 8) | (last << 16) | ((len & 0xff) << 24);
    let hash = mixed.wrapping_mul(0x9e37_79b1);
    (hash as usize) % partitions
}

fn table_overlaps(table: &TableFileMeta, lower: Option<&[u8]>, upper: Option<&[u8]>) -> bool {
    if let (Some(lower), Some(largest)) = (lower, table.largest_user_key())
        && largest < lower
    {
        return false;
    }
    if let (Some(upper), Some(smallest)) = (upper, table.smallest_user_key())
        && smallest >= upper
    {
        return false;
    }
    true
}

fn heap_less(left: usize, right: usize, lanes: &[TableLane]) -> bool {
    let left_key = lanes[left].current_key().unwrap_or(&[]);
    let right_key = lanes[right].current_key().unwrap_or(&[]);
    left_key
        .cmp(right_key)
        .then_with(|| lanes[left].plan.rank.cmp(&lanes[right].plan.rank))
        .is_lt()
}

fn heap_push(heap: &mut Vec<usize>, index: usize, lanes: &[TableLane]) {
    heap.push(index);
    let mut child = heap.len() - 1;
    while child != 0 {
        let parent = (child - 1) / 2;
        if !heap_less(heap[child], heap[parent], lanes) {
            break;
        }
        heap.swap(child, parent);
        child = parent;
    }
}

fn heap_pop(heap: &mut Vec<usize>, lanes: &[TableLane]) -> Option<usize> {
    let last = heap.pop()?;
    if heap.is_empty() {
        return Some(last);
    }
    let result = std::mem::replace(&mut heap[0], last);
    let mut parent = 0usize;
    loop {
        let left = parent.saturating_mul(2).saturating_add(1);
        if left >= heap.len() {
            break;
        }
        let right = left + 1;
        let mut smallest = left;
        if right < heap.len() && heap_less(heap[right], heap[left], lanes) {
            smallest = right;
        }
        if !heap_less(heap[smallest], heap[parent], lanes) {
            break;
        }
        heap.swap(parent, smallest);
        parent = smallest;
    }
    Some(result)
}

fn prefix_bounds(prefix: Option<&[u8]>) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    match prefix {
        None | Some([]) => (None, None),
        Some(prefix) => (Some(prefix.to_vec()), prefix_successor(prefix)),
    }
}

fn prefix_successor(prefix: &[u8]) -> Option<Vec<u8>> {
    let mut upper = prefix.to_vec();
    for index in (0..upper.len()).rev() {
        if upper[index] != u8::MAX {
            upper[index] = upper[index].saturating_add(1);
            upper.truncate(index + 1);
            return Some(upper);
        }
    }
    None
}

fn check_cancelled(options: &ReadOptions, outcome: &mut ScanOutcome) -> Result<()> {
    outcome.cancel_checks = outcome.cancel_checks.saturating_add(1);
    if options
        .cancel
        .as_ref()
        .is_some_and(|cancel| cancel.is_cancelled())
    {
        return Err(LevelDbError::cancelled("parallel block-range scan"));
    }
    Ok(())
}

fn emit_progress(options: &ReadOptions, outcome: &ScanOutcome, interval: usize) {
    if outcome.visited != 0
        && outcome.visited.is_multiple_of(interval)
        && let Some(progress) = &options.progress
    {
        progress.emit(crate::options::ScanProgress {
            visited: outcome.visited,
            bytes_read: outcome.bytes_read,
        });
    }
}

fn scan_pool(workers: usize) -> Result<Arc<rayon::ThreadPool>> {
    static POOLS: OnceLock<Mutex<HashMap<usize, Arc<rayon::ThreadPool>>>> = OnceLock::new();
    let pools = POOLS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(pools) = pools.lock()
        && let Some(pool) = pools.get(&workers)
    {
        return Ok(Arc::clone(pool));
    }

    let pool = Arc::new(
        rayon::ThreadPoolBuilder::new()
            .num_threads(workers.max(1))
            .thread_name(|index| format!("bedrock-leveldb-scan-{index}"))
            .build()
            .map_err(|error| {
                LevelDbError::join(format!("failed to create scan worker pool: {error}"))
            })?,
    );
    let mut pools = pools
        .lock()
        .map_err(|_| LevelDbError::join("scan worker pool registry poisoned"))?;
    Ok(Arc::clone(
        pools.entry(workers).or_insert_with(|| Arc::clone(&pool)),
    ))
}

#[cfg(windows)]
fn open_windows_worker_file(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_RANDOM_ACCESS: u32 = 0x1000_0000;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_RANDOM_ACCESS)
        .open(path)
}

#[cfg(windows)]
fn read_planned_run(plan: &NativeTablePlan, buffer: &mut [u8], offset: u64) -> std::io::Result<()> {
    WINDOWS_WORKER_FILES.with(|cache| cache.borrow_mut().read_run(&plan.path, buffer, offset))
}

#[cfg(not(windows))]
fn read_planned_run(plan: &NativeTablePlan, buffer: &mut [u8], offset: u64) -> std::io::Result<()> {
    read_exact_at(&plan.file, buffer, offset)
}

fn read_footer(file: &File, path: &Path) -> Result<[u8; LEVELDB_FOOTER_LEN]> {
    let file_len = file
        .metadata()
        .map_err(|error| LevelDbError::io_at("stat planned table", path, error))?
        .len();
    if file_len < LEVELDB_FOOTER_LEN as u64 {
        return Err(LevelDbError::corruption_at(
            path,
            "native table is truncated",
        ));
    }
    let mut footer = [0_u8; LEVELDB_FOOTER_LEN];
    read_exact_at(
        file,
        &mut footer,
        file_len.saturating_sub(LEVELDB_FOOTER_LEN as u64),
    )
    .map_err(|error| LevelDbError::io_at("read planned table footer", path, error))?;
    Ok(footer)
}

fn validate_footer_magic(footer: &[u8; LEVELDB_FOOTER_LEN], path: &Path) -> Result<()> {
    let magic_offset = LEVELDB_FOOTER_LEN - 8;
    let magic = u64::from_le_bytes(
        footer[magic_offset..]
            .try_into()
            .map_err(|_| LevelDbError::corruption_at(path, "native footer magic is invalid"))?,
    );
    if magic != LEVELDB_TABLE_MAGIC {
        return Err(LevelDbError::corruption_at(
            path,
            "native table magic mismatch",
        ));
    }
    Ok(())
}

fn read_one_block(
    file: &File,
    path: &Path,
    handle: BlockHandle,
    paranoid_checks: bool,
    encoded: &mut Vec<u8>,
    decoded: &mut Vec<u8>,
) -> Result<()> {
    let size = usize::try_from(handle.size)
        .map_err(|_| LevelDbError::corruption_at(path, "native block size overflows usize"))?;
    let total_size = size
        .checked_add(LEVELDB_BLOCK_TRAILER_LEN)
        .ok_or_else(|| LevelDbError::corruption_at(path, "native block trailer range overflow"))?;
    encoded.clear();
    encoded.resize(total_size, 0);
    read_exact_at(file, encoded, handle.offset)
        .map_err(|error| LevelDbError::io_at("read native index block", path, error))?;
    let compression_tag = encoded[size];
    if paranoid_checks {
        let expected_crc = u32::from_le_bytes(
            encoded[size + 1..size + LEVELDB_BLOCK_TRAILER_LEN]
                .try_into()
                .map_err(|_| LevelDbError::corruption_at(path, "native block crc is invalid"))?,
        );
        let actual_crc = masked_crc32c(&[&encoded[..size], &[compression_tag]]);
        if actual_crc != expected_crc {
            return Err(LevelDbError::corruption_at(
                path,
                format!("native block checksum mismatch at offset {}", handle.offset),
            ));
        }
    }
    if compression_tag == COMPRESSION_NONE {
        decoded.clear();
        decoded.extend_from_slice(&encoded[..size]);
        return Ok(());
    }
    decompress_into(compression_tag, &encoded[..size], decoded)
}

fn block_entries_end(block: &[u8]) -> Result<usize> {
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
        return Err(LevelDbError::corruption(
            "native restart array is truncated",
        ));
    }
    Ok(count_offset - restart_bytes)
}

fn read_block_handle(input: &mut &[u8]) -> Result<BlockHandle> {
    Ok(BlockHandle {
        offset: get_varint64(input)?,
        size: get_varint64(input)?,
    })
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
    use std::io::{Read, Seek, SeekFrom};
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
    fn prefix_successor_handles_binary_prefixes() {
        assert_eq!(prefix_successor(b"player_"), Some(b"player`".to_vec()));
        assert_eq!(prefix_successor(&[0x01, 0xff]), Some(vec![0x02]));
        assert_eq!(prefix_successor(&[0xff]), None);
    }

    #[test]
    fn run_planner_coalesces_contiguous_blocks() {
        let blocks = [
            BlockHandle {
                offset: 0,
                size: 100,
            },
            BlockHandle {
                offset: 105,
                size: 200,
            },
            BlockHandle {
                offset: 310,
                size: 50,
            },
        ];
        let runs = plan_block_runs(&blocks).expect("plan runs");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].block_count, 3);
        assert_eq!(runs[0].encoded_len, 365);
    }

    #[test]
    fn run_planner_splits_non_contiguous_blocks() {
        let blocks = [
            BlockHandle {
                offset: 0,
                size: 100,
            },
            BlockHandle {
                offset: 4096,
                size: 100,
            },
        ];
        let runs = plan_block_runs(&blocks).expect("plan runs");
        assert_eq!(runs.len(), 2);
    }

    #[test]
    fn run_buffer_pool_recycles_capacities() {
        let pool = RunBufferPool::with_count(1);
        let mut buffers = pool.take();
        buffers.keys.reserve(16 * 1024);
        buffers.decoded.reserve(64 * 1024);
        buffers.decoded.extend_from_slice(&vec![1_u8; 4096]);
        let key_capacity = buffers.keys.capacity();
        let decoded_capacity = buffers.decoded.capacity();
        pool.recycle(buffers);
        let buffers = pool.take();
        assert!(buffers.keys.capacity() >= key_capacity);
        assert!(buffers.decoded.capacity() >= decoded_capacity);
        assert!(buffers.decoded.is_empty());
    }

    #[test]
    fn partition_hash_is_bounded() {
        for key in [b"player_1".as_slice(), &[0, 1, 2, 3], &[255, 255]] {
            assert!(partition_for_key(key, 16) < 16);
        }
    }

    #[test]
    fn inflight_window_has_bounded_worker_headroom() {
        let default_options = ReadOptions::default();
        assert_eq!(target_inflight_runs(&default_options, 16, 5, 10_000), 24);
        assert_eq!(target_inflight_runs(&default_options, 4, 20, 100), 20);
        assert_eq!(target_inflight_runs(&default_options, 16, 5, 7), 7);

        let mut tuned = ReadOptions::default();
        tuned.pipeline.queue_depth = 12;
        assert_eq!(target_inflight_runs(&tuned, 16, 5, 10_000), 12);
    }
}
