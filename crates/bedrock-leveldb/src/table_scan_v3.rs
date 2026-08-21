use crate::coding::{get_varint32, get_varint64, masked_crc32c};
use crate::compression::{COMPRESSION_NONE, decompress_into};
use crate::error::{LevelDbError, Result};
use crate::manifest::{Manifest, TableFileMeta};
use crate::options::{ReadOptions, ScanMode, ScanOutcome, VisitorControl};
use rayon::Scope;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, Sender, channel},
};

const CUSTOM_TABLE_MAGIC: &[u8; 9] = b"BWLDBTBL1";
const LEVELDB_TABLE_MAGIC: u64 = 0xdb47_7524_8b80_fb57;
const LEVELDB_FOOTER_LEN: usize = 48;
const LEVELDB_BLOCK_TRAILER_LEN: usize = 5;
const MAX_PREFETCH_PER_TABLE: usize = 16;
const MIN_BUFFER_POOL: usize = 4;

#[derive(Debug, Clone, Copy)]
struct BlockHandle {
    offset: u64,
    size: u64,
}

#[derive(Debug, Clone)]
struct IndexEntry {
    limit_user_key: Vec<u8>,
    handle: BlockHandle,
}

struct NativeTablePlan {
    rank: usize,
    path: PathBuf,
    file: Arc<File>,
    blocks: Vec<BlockHandle>,
    lower: Option<Vec<u8>>,
    upper: Option<Vec<u8>>,
}

impl NativeTablePlan {
    fn open(
        root: &Path,
        table: &TableFileMeta,
        rank: usize,
        lower: Option<&[u8]>,
        upper: Option<&[u8]>,
        paranoid_checks: bool,
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

        let mut buffers = BlockBuffers::default();
        read_block_reused(
            &file,
            &path,
            index_handle,
            paranoid_checks,
            &mut buffers.encoded,
            &mut buffers.decoded,
        )?;
        let index = decode_index_block(&buffers.decoded)?;
        let start = lower.map_or(0, |lower| {
            index.partition_point(|entry| entry.limit_user_key.as_slice() < lower)
        });
        let end = upper.map_or(index.len(), |upper| {
            let first_at_or_after =
                index.partition_point(|entry| entry.limit_user_key.as_slice() < upper);
            first_at_or_after.saturating_add(1).min(index.len())
        });
        if start >= end {
            return Ok(PlanOpen::Skip);
        }
        let blocks = index[start..end]
            .iter()
            .map(|entry| entry.handle)
            .collect::<Vec<_>>();
        if blocks.is_empty() {
            return Ok(PlanOpen::Skip);
        }
        Ok(PlanOpen::Native(Self {
            rank,
            path,
            file: Arc::new(file),
            blocks,
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

#[derive(Debug, Clone, Copy)]
struct BlockEntry {
    key_start: usize,
    key_len: usize,
    value_start: usize,
    value_len: usize,
    is_value: bool,
}

#[derive(Default)]
struct BlockBuffers {
    encoded: Vec<u8>,
    decoded: Vec<u8>,
    keys: Vec<u8>,
    entries: Vec<BlockEntry>,
    decoder_key: Vec<u8>,
    previous_user_key: Vec<u8>,
}

impl BlockBuffers {
    fn reset_metadata(&mut self) {
        self.keys.clear();
        self.entries.clear();
        self.decoder_key.clear();
        self.previous_user_key.clear();
    }
}

struct DecodedBlock {
    table_index: usize,
    block_index: usize,
    buffers: BlockBuffers,
}

impl DecodedBlock {
    fn key(&self, index: usize) -> &[u8] {
        let entry = self.buffers.entries[index];
        &self.buffers.keys[entry.key_start..entry.key_start + entry.key_len]
    }

    fn value(&self, index: usize) -> &[u8] {
        let entry = self.buffers.entries[index];
        &self.buffers.decoded[entry.value_start..entry.value_start + entry.value_len]
    }

    fn is_value(&self, index: usize) -> bool {
        self.buffers.entries[index].is_value
    }

    fn len(&self) -> usize {
        self.buffers.entries.len()
    }
}

#[derive(Default)]
struct BufferPool {
    buffers: Mutex<Vec<BlockBuffers>>,
}

impl BufferPool {
    fn with_capacity(count: usize) -> Self {
        let mut buffers = Vec::with_capacity(count);
        buffers.resize_with(count, BlockBuffers::default);
        Self {
            buffers: Mutex::new(buffers),
        }
    }

    fn take(&self) -> BlockBuffers {
        self.buffers
            .lock()
            .ok()
            .and_then(|mut buffers| buffers.pop())
            .unwrap_or_default()
    }

    fn recycle(&self, mut buffers: BlockBuffers) {
        buffers.reset_metadata();
        if let Ok(mut pool) = self.buffers.lock() {
            pool.push(buffers);
        }
    }
}

enum DecodeMessage {
    Block(DecodedBlock),
    Error(LevelDbError),
}

struct ResultRouter {
    receiver: Receiver<DecodeMessage>,
    pending: Vec<Vec<Option<DecodedBlock>>>,
}

impl ResultRouter {
    fn new(receiver: Receiver<DecodeMessage>, plans: &[Arc<NativeTablePlan>]) -> Self {
        let pending = plans
            .iter()
            .map(|plan| {
                let mut slots = Vec::with_capacity(plan.blocks.len());
                slots.resize_with(plan.blocks.len(), || None);
                slots
            })
            .collect();
        Self { receiver, pending }
    }

    fn wait_for(&mut self, table_index: usize, block_index: usize) -> Result<DecodedBlock> {
        if let Some(block) = self.pending[table_index][block_index].take() {
            return Ok(block);
        }
        loop {
            match self.receiver.recv().map_err(|_| {
                LevelDbError::corruption("parallel block worker stopped before producing a result")
            })? {
                DecodeMessage::Block(block) => {
                    if block.table_index == table_index && block.block_index == block_index {
                        return Ok(block);
                    }
                    let table = block.table_index;
                    let index = block.block_index;
                    self.pending[table][index] = Some(block);
                }
                DecodeMessage::Error(error) => return Err(error),
            }
        }
    }
}

struct TableLane {
    plan: Arc<NativeTablePlan>,
    next_block: usize,
    scheduled_until: usize,
    current: Option<DecodedBlock>,
    entry_index: usize,
    previous_block_last_key: Vec<u8>,
    current_is_value: bool,
}

impl TableLane {
    fn new(plan: Arc<NativeTablePlan>) -> Self {
        Self {
            plan,
            next_block: 0,
            scheduled_until: 0,
            current: None,
            entry_index: 0,
            previous_block_last_key: Vec::with_capacity(48),
            current_is_value: false,
        }
    }

    fn current_key(&self) -> Option<&[u8]> {
        let block = self.current.as_ref()?;
        let index = self.entry_index.checked_sub(1)?;
        Some(block.key(index))
    }

    fn current_value(&self) -> Option<&[u8]> {
        if !self.current_is_value {
            return None;
        }
        let block = self.current.as_ref()?;
        let index = self.entry_index.checked_sub(1)?;
        Some(block.value(index))
    }
}

/// Visibility-correct SST scan entry point.
///
/// Sequential scans retain the allocation-light borrowed cursor. Native parallel
/// scans use one-time SST index planning and data-block work items. Custom tables
/// conservatively fall back to the legacy key-range implementation.
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
        return crate::table_scan_legacy::scan_tables_visible(
            root,
            tables_newest_first,
            prefix,
            paranoid_checks,
            options,
            shadowed,
            visitor,
        );
    }

    let (lower, upper) = prefix_bounds(prefix);
    let mut plans = Vec::<Arc<NativeTablePlan>>::new();
    for (rank, table) in tables_newest_first.iter().enumerate() {
        match NativeTablePlan::open(
            root,
            table,
            rank,
            lower.as_deref(),
            upper.as_deref(),
            paranoid_checks,
        )? {
            PlanOpen::Native(plan) => plans.push(Arc::new(plan)),
            PlanOpen::LegacyTable => {
                return crate::table_scan_legacy::scan_tables_visible(
                    root,
                    tables_newest_first,
                    prefix,
                    paranoid_checks,
                    options,
                    shadowed,
                    visitor,
                );
            }
            PlanOpen::Skip => {}
        }
    }
    if plans.is_empty() {
        let mut outcome = ScanOutcome::empty();
        outcome.worker_threads = 1;
        return Ok(outcome);
    }

    let total_blocks = plans.iter().map(|plan| plan.blocks.len()).sum::<usize>();
    let workers = options.threading.resolve_checked(total_blocks.max(1))?;
    if workers <= 1 {
        return crate::table_scan_legacy::scan_tables_visible(
            root,
            tables_newest_first,
            prefix,
            paranoid_checks,
            options,
            shadowed,
            visitor,
        );
    }

    scan_parallel_blocks(plans, paranoid_checks, options, workers, &shadowed, visitor)
}

/// Key-only partitioned scans retain the visibility-correct legacy range reducer
/// for now. Entry/value scans use the block planner above; the reducer is kept
/// separate so world-level worker-local aggregation semantics remain unchanged.
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
    crate::table_scan_legacy::scan_table_keys_partitioned(
        root,
        tables_newest_first,
        prefix,
        paranoid_checks,
        options,
        shadowed,
        init,
        visitor,
    )
}

fn scan_parallel_blocks<F, S>(
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
    let pool = crate::table_scan_legacy::scan_pool_for_v3(workers)?;
    let stop = Arc::new(AtomicBool::new(false));
    let buffer_count = workers.saturating_mul(2).max(MIN_BUFFER_POOL);
    let buffers = Arc::new(BufferPool::with_capacity(buffer_count));
    let (sender, receiver) = channel::<DecodeMessage>();
    let table_count = plans.len();
    let target_inflight = workers.saturating_mul(2).max(table_count);
    let prefetch = target_inflight
        .div_ceil(table_count.max(1))
        .clamp(1, MAX_PREFETCH_PER_TABLE);
    let progress_interval = options.pipeline.resolve_progress_interval().max(1);
    let cancel = options.cancel.clone();

    pool.scope(|scope| -> Result<ScanOutcome> {
        let mut lanes = plans
            .iter()
            .cloned()
            .map(TableLane::new)
            .collect::<Vec<_>>();
        for table_index in 0..lanes.len() {
            seed_lane(
                scope,
                table_index,
                prefetch,
                &mut lanes,
                paranoid_checks,
                &sender,
                &buffers,
                &stop,
                cancel.clone(),
            );
        }

        let mut router = ResultRouter::new(receiver, &plans);
        let mut heap = Vec::<usize>::with_capacity(lanes.len());
        for index in 0..lanes.len() {
            if advance_lane(
                scope,
                index,
                prefetch,
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
                let winner_key = lanes[first]
                    .current_key()
                    .ok_or_else(|| LevelDbError::corruption("parallel table lane lost current key"))?;
                while let Some(index) = heap.first().copied() {
                    if lanes[index].current_key() != Some(winner_key) {
                        break;
                    }
                    same_lanes.push(
                        heap_pop(&mut heap, &lanes).expect("equal heap root was checked"),
                    );
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
                    prefetch,
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
fn seed_lane<'scope>(
    scope: &Scope<'scope>,
    table_index: usize,
    prefetch: usize,
    lanes: &mut [TableLane],
    paranoid_checks: bool,
    sender: &Sender<DecodeMessage>,
    buffers: &Arc<BufferPool>,
    stop: &Arc<AtomicBool>,
    cancel: Option<crate::options::ScanCancelFlag>,
) {
    for _ in 0..prefetch {
        if !schedule_next(
            scope,
            table_index,
            lanes,
            paranoid_checks,
            sender,
            buffers,
            stop,
            cancel.clone(),
        ) {
            break;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn schedule_next<'scope>(
    scope: &Scope<'scope>,
    table_index: usize,
    lanes: &mut [TableLane],
    paranoid_checks: bool,
    sender: &Sender<DecodeMessage>,
    buffers: &Arc<BufferPool>,
    stop: &Arc<AtomicBool>,
    cancel: Option<crate::options::ScanCancelFlag>,
) -> bool {
    let lane = &mut lanes[table_index];
    if lane.scheduled_until >= lane.plan.blocks.len() {
        return false;
    }
    let block_index = lane.scheduled_until;
    lane.scheduled_until = lane.scheduled_until.saturating_add(1);
    let plan = Arc::clone(&lane.plan);
    let sender = sender.clone();
    let buffers = Arc::clone(buffers);
    let stop = Arc::clone(stop);
    scope.spawn(move |_| {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        if cancel.as_ref().is_some_and(|flag| flag.is_cancelled()) {
            stop.store(true, Ordering::Relaxed);
            let _ = sender.send(DecodeMessage::Error(LevelDbError::cancelled("parallel block scan")));
            return;
        }
        let mut reusable = buffers.take();
        let result = decode_planned_block(
            &plan,
            table_index,
            block_index,
            paranoid_checks,
            &stop,
            cancel.as_ref(),
            &mut reusable,
        );
        match result {
            Ok(()) => {
                let _ = sender.send(DecodeMessage::Block(DecodedBlock {
                    table_index,
                    block_index,
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
    scope: &Scope<'scope>,
    table_index: usize,
    prefetch: usize,
    lanes: &mut [TableLane],
    router: &mut ResultRouter,
    paranoid_checks: bool,
    sender: &Sender<DecodeMessage>,
    buffers: &Arc<BufferPool>,
    stop: &Arc<AtomicBool>,
    cancel: Option<crate::options::ScanCancelFlag>,
) -> Result<bool> {
    loop {
        let lane = &mut lanes[table_index];
        if let Some(block) = lane.current.as_ref() {
            while lane.entry_index < block.len() {
                let index = lane.entry_index;
                lane.entry_index = lane.entry_index.saturating_add(1);
                let key = block.key(index);
                if index == 0
                    && !lane.previous_block_last_key.is_empty()
                    && lane.previous_block_last_key.as_slice() == key
                {
                    continue;
                }
                lane.current_is_value = block.is_value(index);
                return Ok(true);
            }
        }

        if let Some(block) = lane.current.take() {
            if let Some(last_index) = block.len().checked_sub(1) {
                lane.previous_block_last_key.clear();
                lane.previous_block_last_key.extend_from_slice(block.key(last_index));
            }
            buffers.recycle(block.buffers);
            let _ = schedule_next(
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

        let lane = &mut lanes[table_index];
        if lane.next_block >= lane.plan.blocks.len() {
            lane.current_is_value = false;
            return Ok(false);
        }
        let next = router.wait_for(table_index, lane.next_block)?;
        lane.next_block = lane.next_block.saturating_add(1);
        lane.entry_index = 0;
        lane.current = Some(next);

        // Keep the per-table window full even when an early block decodes to no
        // visible records. `schedule_next` after recycle preserves the bound.
        let _ = prefetch;
    }
}

fn decode_planned_block(
    plan: &NativeTablePlan,
    _table_index: usize,
    block_index: usize,
    paranoid_checks: bool,
    stop: &AtomicBool,
    cancel: Option<&crate::options::ScanCancelFlag>,
    buffers: &mut BlockBuffers,
) -> Result<()> {
    let handle = plan.blocks[block_index];
    buffers.reset_metadata();
    read_block_reused(
        &plan.file,
        &plan.path,
        handle,
        paranoid_checks,
        &mut buffers.encoded,
        &mut buffers.decoded,
    )?;
    let entries_end = block_entries_end(&buffers.decoded)?;
    let mut offset = 0usize;
    while offset < entries_end {
        if stop.load(Ordering::Relaxed)
            || cancel.is_some_and(crate::options::ScanCancelFlag::is_cancelled)
        {
            return Err(LevelDbError::cancelled("parallel block decode"));
        }
        let mut input = &buffers.decoded[offset..entries_end];
        let shared = usize::try_from(get_varint32(&mut input)?)
            .map_err(|_| LevelDbError::corruption("native block shared key length overflow"))?;
        let non_shared = usize::try_from(get_varint32(&mut input)?)
            .map_err(|_| LevelDbError::corruption("native block key delta length overflow"))?;
        let value_len = usize::try_from(get_varint32(&mut input)?)
            .map_err(|_| LevelDbError::corruption("native block value length overflow"))?;
        if shared > buffers.decoder_key.len() {
            return Err(LevelDbError::corruption(
                "native block shared prefix exceeds previous key",
            ));
        }
        if input.len() < non_shared.saturating_add(value_len) {
            return Err(LevelDbError::corruption("native block entry is truncated"));
        }
        buffers.decoder_key.truncate(shared);
        buffers.decoder_key.extend_from_slice(&input[..non_shared]);
        input = &input[non_shared..];
        let value_start = entries_end.saturating_sub(input.len());
        let value_end = value_start
            .checked_add(value_len)
            .ok_or_else(|| LevelDbError::corruption("native block value range overflow"))?;
        input = &input[value_len..];
        offset = entries_end.saturating_sub(input.len());

        let Some((user_key, is_value)) = split_internal_key(&buffers.decoder_key) else {
            continue;
        };
        if buffers.previous_user_key.as_slice() == user_key {
            continue;
        }
        buffers.previous_user_key.clear();
        buffers.previous_user_key.extend_from_slice(user_key);
        if plan.lower.as_deref().is_some_and(|lower| user_key < lower) {
            continue;
        }
        if plan.upper.as_deref().is_some_and(|upper| user_key >= upper) {
            break;
        }

        let key_start = buffers.keys.len();
        buffers.keys.extend_from_slice(user_key);
        buffers.entries.push(BlockEntry {
            key_start,
            key_len: user_key.len(),
            value_start,
            value_len: value_end.saturating_sub(value_start),
            is_value,
        });
    }
    Ok(())
}

fn decode_index_block(block: &[u8]) -> Result<Vec<IndexEntry>> {
    let entries_end = block_entries_end(block)?;
    let mut entries = Vec::new();
    let mut offset = 0usize;
    let mut key = Vec::<u8>::with_capacity(48);
    while offset < entries_end {
        let mut input = &block[offset..entries_end];
        let shared = usize::try_from(get_varint32(&mut input)?)
            .map_err(|_| LevelDbError::corruption("native index shared key length overflow"))?;
        let non_shared = usize::try_from(get_varint32(&mut input)?)
            .map_err(|_| LevelDbError::corruption("native index key delta length overflow"))?;
        let value_len = usize::try_from(get_varint32(&mut input)?)
            .map_err(|_| LevelDbError::corruption("native index value length overflow"))?;
        if shared > key.len() || input.len() < non_shared.saturating_add(value_len) {
            return Err(LevelDbError::corruption("native index block entry is truncated"));
        }
        key.truncate(shared);
        key.extend_from_slice(&input[..non_shared]);
        input = &input[non_shared..];
        let value = &input[..value_len];
        input = &input[value_len..];
        offset = entries_end.saturating_sub(input.len());
        let mut handle_input = value;
        let handle = read_block_handle(&mut handle_input)?;
        let limit_user_key = split_internal_key(&key)
            .map_or_else(|| key.clone(), |(user_key, _)| user_key.to_vec());
        entries.push(IndexEntry {
            limit_user_key,
            handle,
        });
    }
    Ok(entries)
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
    if options.cancel.as_ref().is_some_and(|cancel| cancel.is_cancelled()) {
        return Err(LevelDbError::cancelled("parallel block scan"));
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

fn read_footer(file: &File, path: &Path) -> Result<[u8; LEVELDB_FOOTER_LEN]> {
    let file_len = file
        .metadata()
        .map_err(|error| LevelDbError::io_at("stat planned table", path, error))?
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
        return Err(LevelDbError::corruption_at(path, "native table magic mismatch"));
    }
    Ok(())
}

fn read_block_reused(
    file: &File,
    path: &Path,
    handle: BlockHandle,
    paranoid_checks: bool,
    encoded: &mut Vec<u8>,
    decoded: &mut Vec<u8>,
) -> Result<()> {
    let size = usize::try_from(handle.size)
        .map_err(|_| LevelDbError::corruption_at(path, "native block size overflows usize"))?;
    let total_size = size.checked_add(LEVELDB_BLOCK_TRAILER_LEN).ok_or_else(|| {
        LevelDbError::corruption_at(path, "native block trailer range overflow")
    })?;
    encoded.clear();
    encoded.resize(total_size, 0);
    read_exact_at(file, encoded, handle.offset)
        .map_err(|error| LevelDbError::io_at("read planned native block", path, error))?;
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
        return Err(LevelDbError::corruption("native restart array is truncated"));
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
    fn buffer_pool_recycles_capacities() {
        let pool = BufferPool::with_capacity(1);
        let mut buffers = pool.take();
        buffers.encoded.reserve(16 * 1024);
        let capacity = buffers.encoded.capacity();
        pool.recycle(buffers);
        let buffers = pool.take();
        assert!(buffers.encoded.capacity() >= capacity);
    }
}
