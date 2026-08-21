use crate::error::{LevelDbError, Result};
use crate::manifest::{Manifest, TableFileMeta};
use crate::options::{
    ReadOptions, ScanMode, ScanOutcome, ScanProgress, ThreadingOptions, VisitorControl,
};
use crate::table_cursor::BorrowedTableCursor;
use rayon::prelude::*;
use std::path::Path;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel},
};
use std::thread;
use std::time::Instant;

const MAX_KEY_RANGES: usize = 256;
const BATCH_ENTRY_LIMIT: usize = 512;
const BATCH_BYTE_LIMIT: usize = 512 * 1024;

struct MergeSource {
    cursor: BorrowedTableCursor,
    key: Vec<u8>,
    is_value: bool,
    rank: usize,
}

#[derive(Debug, Clone, Copy)]
struct FlatEntry {
    key_start: usize,
    key_len: usize,
    value_start: usize,
    value_len: usize,
}

struct FlatBatch {
    bytes: Vec<u8>,
    entries: Vec<FlatEntry>,
}

impl FlatBatch {
    fn with_capacity() -> Self {
        Self {
            bytes: Vec::with_capacity(BATCH_BYTE_LIMIT),
            entries: Vec::with_capacity(BATCH_ENTRY_LIMIT),
        }
    }

    fn clear(&mut self) {
        self.bytes.clear();
        self.entries.clear();
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn should_flush(&self) -> bool {
        self.entries.len() >= BATCH_ENTRY_LIMIT || self.bytes.len() >= BATCH_BYTE_LIMIT
    }

    fn push(&mut self, key: &[u8], value: &[u8]) {
        let key_start = self.bytes.len();
        self.bytes.extend_from_slice(key);
        let value_start = self.bytes.len();
        self.bytes.extend_from_slice(value);
        self.entries.push(FlatEntry {
            key_start,
            key_len: key.len(),
            value_start,
            value_len: value.len(),
        });
    }

    fn entry(&self, entry: FlatEntry) -> (&[u8], &[u8]) {
        let key = &self.bytes[entry.key_start..entry.key_start + entry.key_len];
        let value = &self.bytes[entry.value_start..entry.value_start + entry.value_len];
        (key, value)
    }
}

enum WorkerMessage {
    Batch(FlatBatch),
    Done {
        queue_wait_ms: u128,
        cancel_checks: usize,
    },
    Error(LevelDbError),
}

struct WorkerPipe {
    receiver: Receiver<WorkerMessage>,
    recycle: SyncSender<FlatBatch>,
}

#[derive(Clone)]
struct KeyRange {
    lower: Option<Vec<u8>>,
    upper: Option<Vec<u8>>,
}

/// Scans current SSTables with LevelDB newest-table visibility semantics.
///
/// Sequential scans borrow values directly from cursor-owned reusable block buffers.
/// `ParallelTables` is implemented as independent key-range merges, so a single large
/// SST can still use all requested workers. Cross-thread delivery uses recycled flat
/// batches instead of one allocation per key/value pair.
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
    let workers = scan_worker_count(options)?;
    if options.scan_mode != ScanMode::ParallelTables || workers <= 1 {
        let (lower, upper) = prefix_bounds(prefix);
        return scan_range_borrowed(
            root,
            tables_newest_first,
            lower.as_deref(),
            upper.as_deref(),
            paranoid_checks,
            options,
            &shadowed,
            visitor,
        );
    }

    scan_parallel_ranges(
        root,
        tables_newest_first,
        prefix,
        paranoid_checks,
        options,
        workers,
        &shadowed,
        visitor,
    )
}

/// Runs visibility-correct key reduction directly inside range workers.
///
/// No key/value batch crosses threads in this path. Each worker owns its reduction
/// state and scans its disjoint key range with the same k-way newest-wins merge.
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
    let workers = scan_worker_count(options)?;
    let ranges = if options.scan_mode == ScanMode::ParallelTables && workers > 1 {
        partition_ranges(prefix, workers)
    } else {
        let (lower, upper) = prefix_bounds(prefix);
        vec![KeyRange { lower, upper }]
    };
    let actual_workers = ranges.len();
    let tables_scanned = count_overlapping_tables(tables_newest_first, prefix);

    if actual_workers == 1 {
        let range = ranges.into_iter().next().expect("one range was created");
        let mut partition = init();
        let mut worker_options = options.clone();
        worker_options.scan_mode = ScanMode::Sequential;
        worker_options.threading = ThreadingOptions::Single;
        let outcome = scan_range_borrowed(
            root,
            tables_newest_first,
            range.lower.as_deref(),
            range.upper.as_deref(),
            paranoid_checks,
            &worker_options,
            &shadowed,
            &mut |key, _value| visitor(&mut partition, key),
        )?;
        return Ok((outcome, vec![partition]));
    }

    let pool = scan_pool(actual_workers)?;
    let stop = AtomicBool::new(false);
    let mut worker_options = options.clone();
    worker_options.scan_mode = ScanMode::Sequential;
    worker_options.threading = ThreadingOptions::Single;
    worker_options.progress = None;

    let results = pool.install(|| {
        ranges
            .into_par_iter()
            .map(|range| {
                let mut partition = init();
                let outcome = scan_range_borrowed(
                    root,
                    tables_newest_first,
                    range.lower.as_deref(),
                    range.upper.as_deref(),
                    paranoid_checks,
                    &worker_options,
                    &shadowed,
                    &mut |key, _value| {
                        if stop.load(Ordering::Relaxed) {
                            return Ok(VisitorControl::Stop);
                        }
                        match visitor(&mut partition, key)? {
                            VisitorControl::Continue => Ok(VisitorControl::Continue),
                            VisitorControl::Stop => {
                                stop.store(true, Ordering::Relaxed);
                                Ok(VisitorControl::Stop)
                            }
                        }
                    },
                )?;
                Ok::<_, LevelDbError>((outcome, partition))
            })
            .collect::<Result<Vec<_>>>()
    })?;

    let mut outcome = ScanOutcome::empty();
    outcome.worker_threads = actual_workers;
    let mut partitions = Vec::with_capacity(results.len());
    for (worker_outcome, partition) in results {
        outcome.merge(worker_outcome);
        partitions.push(partition);
    }
    // The same SST may be opened by every disjoint range. Report logical tables,
    // not table-range probes.
    outcome.tables_scanned = tables_scanned;
    outcome.worker_threads = actual_workers;
    emit_progress(options, &outcome, 1);
    Ok((outcome, partitions))
}

fn scan_worker_count(options: &ReadOptions) -> Result<usize> {
    if options.scan_mode != ScanMode::ParallelTables {
        return options.threading.resolve_checked(1);
    }
    options.threading.resolve_checked(MAX_KEY_RANGES)
}

fn scan_range_borrowed<F, S>(
    root: &Path,
    tables_newest_first: &[TableFileMeta],
    lower: Option<&[u8]>,
    upper: Option<&[u8]>,
    paranoid_checks: bool,
    options: &ReadOptions,
    shadowed: &S,
    visitor: &mut F,
) -> Result<ScanOutcome>
where
    F: FnMut(&[u8], &[u8]) -> Result<VisitorControl>,
    S: Fn(&[u8]) -> bool,
{
    let mut sources = open_sources(
        root,
        tables_newest_first,
        lower,
        upper,
        paranoid_checks,
    )?;
    let tables_scanned = sources.len();
    let mut heap = Vec::<usize>::with_capacity(sources.len());
    for index in 0..sources.len() {
        heap_push(&mut heap, index, &sources);
    }

    let mut outcome = ScanOutcome::empty();
    outcome.worker_threads = 1;
    outcome.tables_scanned = tables_scanned;
    let progress_interval = options.pipeline.resolve_progress_interval();
    let mut same_sources = Vec::<usize>::with_capacity(sources.len().min(8));

    while !heap.is_empty() {
        check_cancelled(options, &mut outcome)?;
        let first = heap_pop(&mut heap, &sources).expect("heap was checked as non-empty");
        same_sources.clear();
        same_sources.push(first);

        {
            // The heap orders equal keys by ascending rank, and rank 0 is newest.
            // Therefore `first` is already the visibility winner for this user key.
            let winner_key = sources[first].key.as_slice();
            while let Some(index) = heap.first().copied() {
                if sources[index].key.as_slice() != winner_key {
                    break;
                }
                same_sources.push(
                    heap_pop(&mut heap, &sources).expect("equal heap root was checked"),
                );
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
            advance_source(source_index, &mut sources, &mut heap)?;
        }
    }
    Ok(outcome)
}

fn open_sources(
    root: &Path,
    tables_newest_first: &[TableFileMeta],
    lower: Option<&[u8]>,
    upper: Option<&[u8]>,
    paranoid_checks: bool,
) -> Result<Vec<MergeSource>> {
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
        sources.push(MergeSource {
            cursor,
            key,
            is_value,
            rank,
        });
    }
    Ok(sources)
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

fn count_overlapping_tables(tables: &[TableFileMeta], prefix: Option<&[u8]>) -> usize {
    let (lower, upper) = prefix_bounds(prefix);
    tables
        .iter()
        .filter(|table| table_overlaps(table, lower.as_deref(), upper.as_deref()))
        .count()
}

fn advance_source(index: usize, sources: &mut [MergeSource], heap: &mut Vec<usize>) -> Result<()> {
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
        heap_push(heap, index, sources);
    }
    Ok(())
}

fn heap_less(left: usize, right: usize, sources: &[MergeSource]) -> bool {
    sources[left]
        .key
        .cmp(&sources[right].key)
        .then_with(|| sources[left].rank.cmp(&sources[right].rank))
        .is_lt()
}

fn heap_push(heap: &mut Vec<usize>, index: usize, sources: &[MergeSource]) {
    heap.push(index);
    let mut child = heap.len() - 1;
    while child != 0 {
        let parent = (child - 1) / 2;
        if !heap_less(heap[child], heap[parent], sources) {
            break;
        }
        heap.swap(child, parent);
        child = parent;
    }
}

fn heap_pop(heap: &mut Vec<usize>, sources: &[MergeSource]) -> Option<usize> {
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
        if right < heap.len() && heap_less(heap[right], heap[left], sources) {
            smallest = right;
        }
        if !heap_less(heap[smallest], heap[parent], sources) {
            break;
        }
        heap.swap(parent, smallest);
        parent = smallest;
    }
    Some(result)
}

#[allow(clippy::too_many_arguments)]
fn scan_parallel_ranges<F, S>(
    root: &Path,
    tables_newest_first: &[TableFileMeta],
    prefix: Option<&[u8]>,
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
    let ranges = partition_ranges(prefix, workers);
    let workers = ranges.len();
    if workers <= 1 {
        let range = ranges.into_iter().next().unwrap_or(KeyRange {
            lower: None,
            upper: None,
        });
        return scan_range_borrowed(
            root,
            tables_newest_first,
            range.lower.as_deref(),
            range.upper.as_deref(),
            paranoid_checks,
            options,
            shadowed,
            visitor,
        );
    }

    let pool = scan_pool(workers)?;
    let stop = Arc::new(AtomicBool::new(false));
    let root = root.to_path_buf();
    let tables = Arc::<[TableFileMeta]>::from(tables_newest_first.to_vec());
    let queue_depth = options
        .pipeline
        .resolve_queue_depth(workers, tables.len())
        .div_ceil(workers)
        .max(1);
    let progress_interval = options.pipeline.resolve_progress_interval();
    let options_for_workers = options.clone();
    let mut final_outcome = ScanOutcome::empty();
    final_outcome.worker_threads = workers;
    final_outcome.tables_scanned = count_overlapping_tables(tables_newest_first, prefix);

    pool.scope(|scope| -> Result<()> {
        let mut pipes = Vec::<WorkerPipe>::with_capacity(workers);
        for range in ranges {
            let (sender, receiver) = sync_channel::<WorkerMessage>(queue_depth);
            let (recycle, recycled) = sync_channel::<FlatBatch>(queue_depth.saturating_add(1));
            pipes.push(WorkerPipe { receiver, recycle });
            let root = root.clone();
            let tables = Arc::clone(&tables);
            let stop = Arc::clone(&stop);
            let mut worker_options = options_for_workers.clone();
            worker_options.scan_mode = ScanMode::Sequential;
            worker_options.threading = ThreadingOptions::Single;
            // Progress is emitted once in global visitor order below.
            worker_options.progress = None;
            scope.spawn(move |_| {
                let result = produce_range(
                    &root,
                    &tables,
                    range,
                    paranoid_checks,
                    &worker_options,
                    &stop,
                    &sender,
                    &recycled,
                );
                if let Err(error) = result {
                    let _ = try_send_message(&sender, WorkerMessage::Error(error), &stop);
                }
            });
        }

        // Ranges were created in lexical order. Draining pipes in the same order
        // preserves deterministic global key order without a cross-worker heap.
        for pipe in &pipes {
            loop {
                let message = pipe.receiver.recv().map_err(|_| {
                    LevelDbError::corruption("parallel scan worker stopped early")
                })?;
                match message {
                    WorkerMessage::Batch(mut batch) => {
                        for entry in batch.entries.iter().copied() {
                            if stop.load(Ordering::Relaxed) {
                                break;
                            }
                            let (key, value) = batch.entry(entry);
                            if shadowed(key) {
                                continue;
                            }
                            final_outcome.record(value.len());
                            if visitor(key, value)? == VisitorControl::Stop {
                                final_outcome.stopped = true;
                                stop.store(true, Ordering::Relaxed);
                                break;
                            }
                            emit_progress(options, &final_outcome, progress_interval);
                        }
                        batch.clear();
                        let _ = pipe.recycle.try_send(batch);
                        if final_outcome.stopped {
                            break;
                        }
                    }
                    WorkerMessage::Done {
                        queue_wait_ms,
                        cancel_checks,
                    } => {
                        final_outcome.queue_wait_ms =
                            final_outcome.queue_wait_ms.saturating_add(queue_wait_ms);
                        final_outcome.cancel_checks =
                            final_outcome.cancel_checks.saturating_add(cancel_checks);
                        break;
                    }
                    WorkerMessage::Error(error) => {
                        stop.store(true, Ordering::Relaxed);
                        return Err(error);
                    }
                }
            }
            if final_outcome.stopped {
                break;
            }
        }
        stop.store(true, Ordering::Relaxed);
        Ok(())
    })?;
    Ok(final_outcome)
}

#[allow(clippy::too_many_arguments)]
fn produce_range(
    root: &Path,
    tables: &[TableFileMeta],
    range: KeyRange,
    paranoid_checks: bool,
    options: &ReadOptions,
    stop: &AtomicBool,
    sender: &SyncSender<WorkerMessage>,
    recycled: &Receiver<FlatBatch>,
) -> Result<()> {
    let mut batch = take_recycled_batch(recycled);
    let mut queue_wait_ms = 0u128;
    let worker_outcome = scan_range_borrowed(
        root,
        tables,
        range.lower.as_deref(),
        range.upper.as_deref(),
        paranoid_checks,
        options,
        &|_| false,
        &mut |key, value| {
            if stop.load(Ordering::Relaxed) {
                return Ok(VisitorControl::Stop);
            }
            batch.push(key, value);
            if batch.should_flush() {
                let next = take_recycled_batch(recycled);
                let ready = std::mem::replace(&mut batch, next);
                let wait_started = Instant::now();
                if !try_send_message(sender, WorkerMessage::Batch(ready), stop) {
                    return Ok(VisitorControl::Stop);
                }
                queue_wait_ms = queue_wait_ms.saturating_add(wait_started.elapsed().as_millis());
            }
            Ok(VisitorControl::Continue)
        },
    )?;

    if !batch.is_empty() && !stop.load(Ordering::Relaxed) {
        let wait_started = Instant::now();
        if !try_send_message(sender, WorkerMessage::Batch(batch), stop) {
            return Ok(());
        }
        queue_wait_ms = queue_wait_ms.saturating_add(wait_started.elapsed().as_millis());
    }
    let _ = try_send_message(
        sender,
        WorkerMessage::Done {
            queue_wait_ms,
            cancel_checks: worker_outcome.cancel_checks,
        },
        stop,
    );
    Ok(())
}

fn take_recycled_batch(recycled: &Receiver<FlatBatch>) -> FlatBatch {
    match recycled.try_recv() {
        Ok(mut batch) => {
            batch.clear();
            batch
        }
        Err(TryRecvError::Empty | TryRecvError::Disconnected) => FlatBatch::with_capacity(),
    }
}

fn try_send_message(
    sender: &SyncSender<WorkerMessage>,
    mut message: WorkerMessage,
    stop: &AtomicBool,
) -> bool {
    loop {
        match sender.try_send(message) {
            Ok(()) => return true,
            Err(TrySendError::Full(returned)) => {
                message = returned;
                if stop.load(Ordering::Relaxed) {
                    return false;
                }
                thread::yield_now();
            }
            Err(TrySendError::Disconnected(_)) => return false,
        }
    }
}

fn partition_ranges(prefix: Option<&[u8]>, workers: usize) -> Vec<KeyRange> {
    let workers = workers.clamp(1, MAX_KEY_RANGES);
    if workers == 1 {
        let (lower, upper) = prefix_bounds(prefix);
        return vec![KeyRange { lower, upper }];
    }

    let base = prefix.unwrap_or(&[]);
    let prefix_upper = prefix.and_then(prefix_successor);
    let mut ranges = Vec::with_capacity(workers);
    for worker in 0..workers {
        let start = worker * 256 / workers;
        let end = (worker + 1) * 256 / workers;
        let lower = if worker == 0 {
            (!base.is_empty()).then(|| base.to_vec())
        } else {
            let mut lower = base.to_vec();
            lower.push(u8::try_from(start).unwrap_or(u8::MAX));
            Some(lower)
        };
        let upper = if worker + 1 == workers {
            if base.is_empty() {
                None
            } else {
                prefix_upper.clone()
            }
        } else {
            let mut upper = base.to_vec();
            upper.push(u8::try_from(end).unwrap_or(u8::MAX));
            Some(upper)
        };
        ranges.push(KeyRange { lower, upper });
    }
    ranges
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
    if options.cancel.as_ref().is_some_and(|cancel| cancel.is_cancelled()) {
        return Err(LevelDbError::cancelled("table scan"));
    }
    Ok(())
}

fn emit_progress(options: &ReadOptions, outcome: &ScanOutcome, interval: usize) {
    if outcome.visited != 0 && outcome.visited.is_multiple_of(interval.max(1)) {
        if let Some(progress) = &options.progress {
            progress.emit(ScanProgress {
                visited: outcome.visited,
                bytes_read: outcome.bytes_read,
            });
        }
    }
}

fn scan_pool(workers: usize) -> Result<Arc<rayon::ThreadPool>> {
    static POOLS: OnceLock<Mutex<std::collections::HashMap<usize, Arc<rayon::ThreadPool>>>> =
        OnceLock::new();
    let pools = POOLS.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
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
    fn partitions_are_contiguous() {
        let ranges = partition_ranges(Some(b"player_"), 4);
        assert_eq!(ranges.len(), 4);
        for pair in ranges.windows(2) {
            assert_eq!(pair[0].upper, pair[1].lower);
        }
        assert_eq!(ranges[0].lower.as_deref(), Some(b"player_".as_slice()));
        assert_eq!(
            ranges.last().and_then(|range| range.upper.as_deref()),
            Some(b"player`".as_slice())
        );
    }

    #[test]
    fn full_keyspace_partitions_cover_all_first_bytes() {
        let ranges = partition_ranges(None, 16);
        assert_eq!(ranges.len(), 16);
        assert!(ranges.first().is_some_and(|range| range.lower.is_none()));
        assert!(ranges.last().is_some_and(|range| range.upper.is_none()));
        for pair in ranges.windows(2) {
            assert_eq!(pair[0].upper, pair[1].lower);
        }
    }
}
