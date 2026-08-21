use crate::coding::{get_varint32, get_varint64, masked_crc32c};
use crate::compression::{COMPRESSION_NONE, decompress_into};
use crate::error::{LevelDbError, Result};
use crate::manifest::{Manifest, TableFileMeta};
use crate::options::{ReadOptions, ScanMode, ScanOutcome, VisitorControl};
use rayon::ScopeFifo;
use std::collections::HashMap;
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

struct NativeTablePlan {
    rank: usize,
    path: PathBuf,
    file: Arc<File>,
    blocks: Vec<BlockHandle>,
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
        planning_buffers: &mut BlockBuffers,
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

        planning_buffers.reset_metadata();
        read_block_reused(
            &file,
            &path,
            index_handle,
            paranoid_checks,
            &mut planning_buffers.encoded,
            &mut planning_buffers.decoded,
        )?;
        let blocks = decode_index_handles(
            &planning_buffers.decoded,
            lower,
            upper,
            &mut planning_buffers.decoder_key,
        )?;
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

enum PreparedScan {
    Native {
        plans: Vec<Arc<NativeTablePlan>>,
        workers: usize,
    },
    Legacy,
    Empty,
}

#[derive(Debug, Clone, Copy)]
struct BlockEntry {
    key_start: u32,
    key_len: u32,
    value_start: u32,
    value_len: u32,
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
        let start = entry.key_start as usize;
        let end = start + entry.key_len as usize;
        &self.buffers.keys[start..end]
    }

    fn value(&self, index: usize) -> &[u8] {
        let entry = self.buffers.entries[index];
        let start = entry.value_start as usize;
        let end = start + entry.value_len as usize;
        &self.buffers.decoded[start..end]
    }

    fn value_len(&self, index: usize) -> usize {
        self.buffers.entries[index].value_len as usize
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
    fn with_count(count: usize) -> Self {
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
    pending: HashMap<(usize, usize), DecodedBlock>,
}

impl ResultRouter {
    fn new(receiver: Receiver<DecodeMessage>, max_inflight: usize) -> Self {
        Self {
            receiver,
            pending: HashMap::with_capacity(max_inflight.max(1)),
        }
    }

    fn wait_for(&mut self, table_index: usize, block_index: usize) -> Result<DecodedBlock> {
        let requested = (table_index, block_index);
        if let Some(block) = self.pending.remove(&requested) {
            return Ok(block);
        }
        loop {
            match self.receiver.recv().map_err(|_| {
                LevelDbError::corruption("parallel block worker stopped before producing a result")
            })? {
                DecodeMessage::Block(block) => {
                    let id = (block.table_index, block.block_index);
                    if id == requested {
                        return Ok(block);
                    }
                    if self.pending.insert(id, block).is_some() {
                        return Err(LevelDbError::corruption(
                            "parallel block planner produced a duplicate block result",
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

    fn current_entry_index(&self) -> Option<usize> {
        self.entry_index.checked_sub(1)
    }

    fn current_key(&self) -> Option<&[u8]> {
        let block = self.current.as_ref()?;
        Some(block.key(self.current_entry_index()?))
    }

    fn current_value(&self) -> Option<&[u8]> {
        if !self.current_is_value {
            return None;
        }
        let block = self.current.as_ref()?;
        Some(block.value(self.current_entry_index()?))
    }

    fn current_value_len(&self) -> Option<usize> {
        let block = self.current.as_ref()?;
        Some(block.value_len(self.current_entry_index()?))
    }
}

/// Scans current SSTables with visibility-correct newest-table semantics.
///
/// Sequential mode keeps the direct borrowed cursor. Native `ParallelTables`
/// mode parses each SST index once, schedules each selected data block exactly
/// once, decompresses blocks on a reusable Rayon worker pool, and merges the
/// resulting table lanes in user-key order. Values remain borrowed from the
/// decoded block backing allocation until the visitor returns.
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
        return legacy_scan_tables(
            root,
            tables_newest_first,
            prefix,
            paranoid_checks,
            options,
            shadowed,
            visitor,
        );
    }

    match prepare_native_scan(
        root,
        tables_newest_first,
        prefix,
        paranoid_checks,
        options,
    )? {
        PreparedScan::Native { plans, workers } => scan_parallel_blocks(
            plans,
            paranoid_checks,
            options,
            workers,
            &shadowed,
            visitor,
        ),
        PreparedScan::Legacy => legacy_scan_tables(
            root,
            tables_newest_first,
            prefix,
            paranoid_checks,
            options,
            shadowed,
            visitor,
        ),
        PreparedScan::Empty => {
            let mut outcome = ScanOutcome::empty();
            outcome.worker_threads = 1;
            Ok(outcome)
        }
    }
}

/// Reduces visible keys into independent caller-owned partitions.
///
/// The expensive SST I/O, checksum and decompression work uses the same data-block
/// planner as entry scans, so no SST block is reopened per logical key range. The
/// lightweight reduction callback runs while the globally visible key is already
/// borrowed from a decoded block; keys are never copied into cross-thread batches.
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
        return crate::table_scan_legacy::scan_table_keys_partitioned(
            root,
            tables_newest_first,
            prefix,
            paranoid_checks,
            options,
            shadowed,
            init,
            visitor,
        );
    }

    match prepare_native_scan(
        root,
        tables_newest_first,
        prefix,
        paranoid_checks,
        options,
    )? {
        PreparedScan::Native { plans, workers } => {
            let mut partitions = (0..workers).map(|_| init()).collect::<Vec<_>>();
            let visitor_ref = &visitor;
            let mut reduce = |key: &[u8], _value: &[u8]| {
                let partition = partition_for_key(key, partitions.len());
                visitor_ref(&mut partitions[partition], key)
            };
            let outcome = scan_parallel_blocks(
                plans,
                paranoid_checks,
                options,
                workers,
                &shadowed,
                &mut reduce,
            )?;
            Ok((outcome, partitions))
        }
        PreparedScan::Legacy => crate::table_scan_legacy::scan_table_keys_partitioned(
            root,
            tables_newest_first,
            prefix,
            paranoid_checks,
            options,
            shadowed,
            init,
            visitor,
        ),
        PreparedScan::Empty => Ok((ScanOutcome::empty(), vec![init()])),
    }
}

fn legacy_scan_tables<F, S>(
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
    crate::table_scan_legacy::scan_tables_visible(
        root,
        tables_newest_first,
        prefix,
        paranoid_checks,
        options,
        shadowed,
        visitor,
    )
}

fn prepare_native_scan(
    root: &Path,
    tables_newest_first: &[TableFileMeta],
    prefix: Option<&[u8]>,
    paranoid_checks: bool,
    options: &ReadOptions,
) -> Result<PreparedScan> {
    let (lower, upper) = prefix_bounds(prefix);
    let mut planning_buffers = BlockBuffers::default();
    let mut plans = Vec::<Arc<NativeTablePlan>>::new();
    for (rank, table) in tables_newest_first.iter().enumerate() {
        match NativeTablePlan::open(
            root,
            table,
            rank,
            lower.as_deref(),
            upper.as_deref(),
            paranoid_checks,
            &mut planning_buffers,
        )? {
            PlanOpen::Native(plan) => plans.push(Arc::new(plan)),
            PlanOpen::LegacyTable => return Ok(PreparedScan::Legacy),
            PlanOpen::Skip => {}
        }
    }
    if plans.is_empty() {
        return Ok(PreparedScan::Empty);
    }
    let total_blocks = plans.iter().map(|plan| plan.blocks.len()).sum::<usize>();
    let workers = options.threading.resolve_checked(total_blocks.max(1))?;
    if workers <= 1 {
        return Ok(PreparedScan::Legacy);
    }
    Ok(PreparedScan::Native { plans, workers })
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
    let table_count = plans.len();
    let target_inflight = workers.saturating_mul(2).max(table_count);
    let prefetch = target_inflight
        .div_ceil(table_count.max(1))
        .clamp(1, MAX_PREFETCH_PER_TABLE);
    let window_count = plans
        .iter()
        .map(|plan| plan.blocks.len().min(prefetch))
        .sum::<usize>()
        .max(MIN_BUFFER_POOL);

    let pool = crate::table_scan_legacy::scan_pool_for_v3(workers)?;
    let stop = Arc::new(AtomicBool::new(false));
    let buffers = Arc::new(BufferPool::with_count(window_count));
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
    buffers: &Arc<BufferPool>,
    stop: &Arc<AtomicBool>,
    cancel: Option<crate::options::ScanCancelFlag>,
) {
    // Round-robin seeding plus FIFO scope means block 0 of every table is made
    // runnable before deep prefetch from one table. This minimizes merge startup
    // latency while still leaving enough independent DEFLATE work to fill CPUs.
    for _ in 0..prefetch {
        for table_index in 0..lanes.len() {
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
    }
}

#[allow(clippy::too_many_arguments)]
fn schedule_next<'scope>(
    scope: &ScopeFifo<'scope>,
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
    scope.spawn_fifo(move |_| {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        if cancel.as_ref().is_some_and(|flag| flag.is_cancelled()) {
            stop.store(true, Ordering::Relaxed);
            let _ = sender.send(DecodeMessage::Error(LevelDbError::cancelled(
                "parallel block scan",
            )));
            return;
        }
        let mut reusable = buffers.take();
        let result = decode_planned_block(
            &plan,
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
    scope: &ScopeFifo<'scope>,
    table_index: usize,
    lanes: &mut [TableLane],
    router: &mut ResultRouter,
    paranoid_checks: bool,
    sender: &Sender<DecodeMessage>,
    buffers: &Arc<BufferPool>,
    stop: &Arc<AtomicBool>,
    cancel: Option<crate::options::ScanCancelFlag>,
) -> Result<bool> {
    loop {
        {
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
        }

        let finished = lanes[table_index].current.take();
        if let Some(block) = finished {
            if let Some(last_index) = block.len().checked_sub(1) {
                let last_key = block.key(last_index).to_vec();
                lanes[table_index].previous_block_last_key.clear();
                lanes[table_index]
                    .previous_block_last_key
                    .extend_from_slice(&last_key);
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

        let next_block = lanes[table_index].next_block;
        if next_block >= lanes[table_index].plan.blocks.len() {
            lanes[table_index].current_is_value = false;
            return Ok(false);
        }
        let next = router.wait_for(table_index, next_block)?;
        lanes[table_index].next_block = next_block.saturating_add(1);
        lanes[table_index].entry_index = 0;
        lanes[table_index].current = Some(next);
    }
}

fn decode_planned_block(
    plan: &NativeTablePlan,
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

        let key_start = u32::try_from(buffers.keys.len())
            .map_err(|_| LevelDbError::corruption("native block key slab exceeds u32"))?;
        let key_len = u32::try_from(user_key.len())
            .map_err(|_| LevelDbError::corruption("native user key exceeds u32"))?;
        let value_start = u32::try_from(value_start)
            .map_err(|_| LevelDbError::corruption("native value offset exceeds u32"))?;
        let value_len = u32::try_from(value_end.saturating_sub(value_start as usize))
            .map_err(|_| LevelDbError::corruption("native value length exceeds u32"))?;
        buffers.keys.extend_from_slice(user_key);
        buffers.entries.push(BlockEntry {
            key_start,
            key_len,
            value_start,
            value_len,
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
            return Err(LevelDbError::corruption("native index block entry is truncated"));
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

fn partition_for_key(key: &[u8], partitions: usize) -> usize {
    if partitions <= 1 {
        return 0;
    }
    let first = u32::from(key.first().copied().unwrap_or(0));
    let second = u32::from(key.get(1).copied().unwrap_or(0));
    let last = u32::from(key.last().copied().unwrap_or(0));
    let len = u32::try_from(key.len()).unwrap_or(u32::MAX);
    let mixed = first
        | (second << 8)
        | (last << 16)
        | ((len & 0xff) << 24);
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
        let pool = BufferPool::with_count(1);
        let mut buffers = pool.take();
        buffers.encoded.reserve(16 * 1024);
        let capacity = buffers.encoded.capacity();
        pool.recycle(buffers);
        let buffers = pool.take();
        assert!(buffers.encoded.capacity() >= capacity);
    }

    #[test]
    fn partition_hash_is_bounded() {
        for key in [b"player_1".as_slice(), &[0, 1, 2, 3], &[255, 255]] {
            assert!(partition_for_key(key, 16) < 16);
        }
    }
}
