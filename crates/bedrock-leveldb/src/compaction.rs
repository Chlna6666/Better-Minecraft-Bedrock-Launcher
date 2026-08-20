use crate::error::{LevelDbError, Result};
use crate::manifest::{Manifest, TableFileMeta};
use crate::options::VisitorControl;
use crate::table;
use bytes::Bytes;
use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeMap, BinaryHeap, HashSet, VecDeque};
use std::path::Path;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::thread;

pub(crate) const MAX_LEVEL: u32 = 6;
const LEVEL_ZERO_FILE_TRIGGER: usize = 4;
const MAX_LEVEL_ZERO_INPUTS_PER_PASS: usize = 8;
const MAX_COMPACTION_SOURCE_BYTES: u64 = 16 * 1024 * 1024;
const TARGET_OUTPUT_FILE_BYTES: usize = 2 * 1024 * 1024;
const STREAM_QUEUE_DEPTH: usize = 4;
const COMPACTION_STREAM_STACK_BYTES: usize = 1024 * 1024;

pub(crate) struct CompactionPlan {
    pub(crate) inputs: Vec<TableFileMeta>,
    pub(crate) output_level: u32,
}

impl CompactionPlan {
    pub(crate) fn input_numbers(&self) -> HashSet<u64> {
        self.inputs.iter().map(|table| table.number).collect()
    }
}

#[derive(Debug)]
struct PendingInput {
    priority: usize,
    table: TableFileMeta,
}

#[derive(Debug)]
struct StreamEntry {
    key: Vec<u8>,
    value: Option<Bytes>,
}

#[derive(Debug)]
enum StreamMessage {
    Entry(StreamEntry),
    Done,
    Error(LevelDbError),
}

#[derive(Debug)]
struct StreamHandle {
    receiver: Receiver<StreamMessage>,
    recycle: SyncSender<Vec<u8>>,
}

#[derive(Debug)]
struct HeapEntry {
    key: Vec<u8>,
    value: Option<Bytes>,
    priority: usize,
}

impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.key == other.key
    }
}

impl Eq for HeapEntry {}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is a max-heap. Reverse the key ordering so the smallest
        // user key is popped first. Priority is only a deterministic tie-break;
        // all equal-key heads are reconciled before an output entry is emitted.
        other
            .key
            .cmp(&self.key)
            .then_with(|| self.priority.cmp(&other.priority))
    }
}

pub(crate) fn plan(manifest: &Manifest, force: bool) -> Option<CompactionPlan> {
    let input_level = choose_input_level(manifest, force)?;
    let mut inputs = if input_level == 0 {
        let mut level_zero = manifest
            .table_files
            .iter()
            .filter(|table| table.level == input_level)
            .cloned()
            .collect::<Vec<_>>();
        level_zero.sort_by_key(|table| table.number);
        take_bounded_level_zero_inputs(level_zero)
    } else {
        // Leveled tables are compacted one source range at a time. Besides
        // bounding the k-way merge fan-in, this avoids force compaction turning
        // a large level into one enormous in-memory scheduling operation.
        manifest
            .table_files
            .iter()
            .filter(|table| table.level == input_level)
            .min_by_key(|table| table.number)
            .cloned()
            .into_iter()
            .collect()
    };
    let output_level = input_level.saturating_add(1).min(MAX_LEVEL);
    let range = table_range(&inputs);
    inputs.extend(
        manifest
            .table_files
            .iter()
            .filter(|table| table.level == output_level && overlaps(table, range.as_ref()))
            .cloned(),
    );
    Some(CompactionPlan {
        inputs,
        output_level,
    })
}

fn take_bounded_level_zero_inputs(level_zero: Vec<TableFileMeta>) -> Vec<TableFileMeta> {
    let mut selected = Vec::with_capacity(MAX_LEVEL_ZERO_INPUTS_PER_PASS.min(level_zero.len()));
    let mut selected_bytes = 0_u64;

    for table in level_zero {
        if selected.len() >= MAX_LEVEL_ZERO_INPUTS_PER_PASS {
            break;
        }
        let table_bytes = compaction_budget_bytes(&table);
        if !selected.is_empty()
            && selected_bytes.saturating_add(table_bytes) > MAX_COMPACTION_SOURCE_BYTES
        {
            break;
        }
        selected_bytes = selected_bytes.saturating_add(table_bytes);
        selected.push(table);
    }

    selected
}

fn compaction_budget_bytes(table: &TableFileMeta) -> u64 {
    if table.file_size == 0 {
        TARGET_OUTPUT_FILE_BYTES as u64
    } else {
        table.file_size
    }
}

/// Merges compaction inputs with a bounded streaming k-way merge.
///
/// Table scans feed small bounded queues and are activated lazily from manifest
/// key ranges. The heap therefore retains only one current entry per active
/// input instead of materializing every input table into a temporary map before
/// merging. Output partitions keep the existing `Db` write contract and are
/// capped by [`TARGET_OUTPUT_FILE_BYTES`]. Duplicate input-key buffers are
/// recycled back to active producers so repeated versions do not continuously
/// churn the allocator.
pub(crate) fn merge(
    root: &Path,
    plan: &CompactionPlan,
    paranoid_checks: bool,
) -> Result<Vec<BTreeMap<Vec<u8>, Option<Bytes>>>> {
    let mut priority_order = plan.inputs.clone();
    // Preserve the previous last-write-wins ordering: output-level tables are
    // older than the source level, while larger L0 file numbers are newer.
    priority_order.sort_by_key(|table| (Reverse(table.level), table.number));

    let mut pending = priority_order
        .into_iter()
        .enumerate()
        .map(|(priority, table)| PendingInput { priority, table })
        .collect::<Vec<_>>();
    pending.sort_by(compare_pending_inputs);
    let input_count = pending.len();
    let mut pending = VecDeque::from(pending);

    thread::scope(|scope| -> Result<Vec<BTreeMap<Vec<u8>, Option<Bytes>>>> {
        let spawn_stream = |path: std::path::PathBuf, table_number: u64| -> Result<StreamHandle> {
            let (sender, receiver) = sync_channel(STREAM_QUEUE_DEPTH);
            let (recycle, recycled_keys) = sync_channel::<Vec<u8>>(STREAM_QUEUE_DEPTH);
            let error_path = path.clone();
            thread::Builder::new()
                .name(format!("bedrock-leveldb-compact-{table_number}"))
                .stack_size(COMPACTION_STREAM_STACK_BYTES)
                .spawn_scoped(scope, move || {
                    let result = table::for_each_table_lookup(
                        &path,
                        paranoid_checks,
                        None,
                        |key, value| {
                            let mut owned_key = recycled_keys
                                .try_recv()
                                .unwrap_or_else(|_| Vec::with_capacity(key.len()));
                            owned_key.clear();
                            owned_key.extend_from_slice(key);
                            let message = StreamMessage::Entry(StreamEntry {
                                key: owned_key,
                                value: value.cloned(),
                            });
                            if sender.send(message).is_err() {
                                return Ok(VisitorControl::Stop);
                            }
                            Ok(VisitorControl::Continue)
                        },
                    );
                    let final_message = match result {
                        Ok(_) => StreamMessage::Done,
                        Err(error) => StreamMessage::Error(error),
                    };
                    let _ = sender.send(final_message);
                })
                .map_err(|error| {
                    LevelDbError::io_at("spawn compaction stream", &error_path, error)
                })?;
            Ok(StreamHandle { receiver, recycle })
        };

        let mut streams = std::iter::repeat_with(|| None)
            .take(input_count)
            .collect::<Vec<Option<StreamHandle>>>();
        let mut heap = BinaryHeap::<HeapEntry>::new();
        let mut outputs = Vec::<BTreeMap<Vec<u8>, Option<Bytes>>>::new();
        let mut current = BTreeMap::<Vec<u8>, Option<Bytes>>::new();
        let mut current_bytes = 0_usize;

        loop {
            // A not-yet-opened table cannot contain a key below its manifest
            // smallest key. Activate only the streams that could affect the
            // current heap minimum. Unknown ranges are conservatively activated
            // first; normal native tables carry exact user-key bounds.
            loop {
                let activate = pending.front().is_some_and(|next| {
                    let current_key = heap.peek().map(|entry| entry.key.as_slice());
                    should_activate(next, current_key)
                });
                if !activate {
                    break;
                }

                let next = pending
                    .pop_front()
                    .expect("front was checked before compaction stream activation");
                let path = root.join(Manifest::table_name(next.table.number));
                streams[next.priority] = Some(spawn_stream(path, next.table.number)?);
                advance_stream(next.priority, &mut streams, &mut heap)?;
            }

            let Some(first) = heap.pop() else {
                if pending.is_empty() {
                    break;
                }
                // An empty table may have been activated above. With an empty
                // heap the next pending table must be opened before continuing.
                continue;
            };

            let mut winner_key = first.key;
            let mut winner_priority = first.priority;
            let mut winner_value = first.value;
            advance_stream(first.priority, &mut streams, &mut heap)?;

            while heap
                .peek()
                .is_some_and(|entry| entry.key.as_slice() == winner_key.as_slice())
            {
                let same_key = heap
                    .pop()
                    .expect("heap entry was checked before equal-key pop");
                let same_priority = same_key.priority;
                if same_priority >= winner_priority {
                    recycle_key(winner_priority, winner_key, &streams);
                    winner_key = same_key.key;
                    winner_priority = same_priority;
                    winner_value = same_key.value;
                } else {
                    recycle_key(same_priority, same_key.key, &streams);
                }
                advance_stream(same_priority, &mut streams, &mut heap)?;
            }

            if plan.output_level == MAX_LEVEL && winner_value.is_none() {
                recycle_key(winner_priority, winner_key, &streams);
                continue;
            }

            let entry_bytes = winner_key
                .len()
                .saturating_add(winner_value.as_ref().map_or(0, Bytes::len))
                .saturating_add(24);
            if !current.is_empty()
                && current_bytes.saturating_add(entry_bytes) > TARGET_OUTPUT_FILE_BYTES
            {
                outputs.push(std::mem::take(&mut current));
                current_bytes = 0;
            }
            current_bytes = current_bytes.saturating_add(entry_bytes);
            current.insert(winner_key, winner_value);
        }

        if !current.is_empty() {
            outputs.push(current);
        }
        Ok(outputs)
    })
}

fn recycle_key(priority: usize, key: Vec<u8>, streams: &[Option<StreamHandle>]) {
    if let Some(stream) = streams.get(priority).and_then(Option::as_ref) {
        let _ = stream.recycle.try_send(key);
    }
}

fn compare_pending_inputs(left: &PendingInput, right: &PendingInput) -> Ordering {
    match (
        left.table.smallest_key.as_deref(),
        right.table.smallest_key.as_deref(),
    ) {
        (None, None) => left.priority.cmp(&right.priority),
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(left_key), Some(right_key)) => left_key
            .cmp(right_key)
            .then_with(|| left.priority.cmp(&right.priority)),
    }
}

fn should_activate(input: &PendingInput, current_key: Option<&[u8]>) -> bool {
    match current_key {
        None => true,
        Some(current_key) => input
            .table
            .smallest_key
            .as_deref()
            .is_none_or(|smallest_key| smallest_key <= current_key),
    }
}

fn advance_stream(
    priority: usize,
    streams: &mut [Option<StreamHandle>],
    heap: &mut BinaryHeap<HeapEntry>,
) -> Result<()> {
    let message = streams
        .get(priority)
        .and_then(Option::as_ref)
        .ok_or_else(|| {
            LevelDbError::corruption(format!(
                "compaction stream {priority} is not active while advancing"
            ))
        })?
        .receiver
        .recv()
        .map_err(|_| {
            LevelDbError::corruption(format!(
                "compaction stream {priority} terminated before completion"
            ))
        })?;

    match message {
        StreamMessage::Entry(entry) => {
            heap.push(HeapEntry {
                key: entry.key,
                value: entry.value,
                priority,
            });
        }
        StreamMessage::Done => {
            streams[priority] = None;
        }
        StreamMessage::Error(error) => {
            streams[priority] = None;
            return Err(error);
        }
    }
    Ok(())
}

fn choose_input_level(manifest: &Manifest, force: bool) -> Option<u32> {
    let level_zero_count = manifest
        .table_files
        .iter()
        .filter(|table| table.level == 0)
        .count();
    if level_zero_count >= LEVEL_ZERO_FILE_TRIGGER || (force && level_zero_count != 0) {
        return Some(0);
    }
    for level in 1..MAX_LEVEL {
        let tables = manifest
            .table_files
            .iter()
            .filter(|table| table.level == level)
            .collect::<Vec<_>>();
        let bytes = tables
            .iter()
            .fold(0_u64, |total, table| total.saturating_add(table.file_size));
        if bytes > level_size_limit(level) || (force && !tables.is_empty()) {
            return Some(level);
        }
    }
    None
}

fn level_size_limit(level: u32) -> u64 {
    let exponent = level.saturating_sub(1).min(5);
    10_u64
        .saturating_pow(exponent)
        .saturating_mul(10 * 1024 * 1024)
}

fn table_range(tables: &[TableFileMeta]) -> Option<(Vec<u8>, Vec<u8>)> {
    if tables
        .iter()
        .any(|table| table.smallest_key.is_none() || table.largest_key.is_none())
    {
        return None;
    }
    let smallest = tables
        .iter()
        .filter_map(|table| table.smallest_key.as_ref())
        .min()?
        .clone();
    let largest = tables
        .iter()
        .filter_map(|table| table.largest_key.as_ref())
        .max()?
        .clone();
    Some((smallest, largest))
}

fn overlaps(table: &TableFileMeta, range: Option<&(Vec<u8>, Vec<u8>)>) -> bool {
    let Some((smallest, largest)) = range else {
        return true;
    };
    table.largest_key.as_ref().is_none_or(|key| key >= smallest)
        && table.smallest_key.as_ref().is_none_or(|key| key <= largest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::CompressionPolicy;
    use tempfile::tempdir;

    fn write_table(
        root: &Path,
        number: u64,
        level: u32,
        entries: BTreeMap<Vec<u8>, Option<Bytes>>,
    ) -> TableFileMeta {
        let written = table::write_native_memtable(
            &root.join(Manifest::table_name(number)),
            &entries,
            number,
            CompressionPolicy::None,
        )
        .expect("write compaction test table");
        TableFileMeta::native(
            number,
            level,
            written.file_size,
            written.smallest_internal_key,
            written.largest_internal_key,
        )
    }

    fn put(entries: &mut BTreeMap<Vec<u8>, Option<Bytes>>, key: &[u8], value: &[u8]) {
        entries.insert(key.to_vec(), Some(Bytes::copy_from_slice(value)));
    }

    fn merged_value<'a>(
        entries: &'a BTreeMap<Vec<u8>, Option<Bytes>>,
        key: &[u8],
    ) -> Option<&'a [u8]> {
        entries.get(key).and_then(|value| value.as_deref())
    }

    #[test]
    fn streaming_merge_preserves_newest_table_priority() {
        let dir = tempdir().expect("tempdir");

        let mut base_entries = BTreeMap::new();
        put(&mut base_entries, b"a", b"base");
        put(&mut base_entries, b"c", b"base-c");
        let base = write_table(dir.path(), 10, 1, base_entries);

        let mut older_l0_entries = BTreeMap::new();
        put(&mut older_l0_entries, b"a", b"older-l0");
        put(&mut older_l0_entries, b"b", b"older-b");
        let older_l0 = write_table(dir.path(), 20, 0, older_l0_entries);

        let mut newer_l0_entries = BTreeMap::new();
        put(&mut newer_l0_entries, b"a", b"newest");
        put(&mut newer_l0_entries, b"d", b"newer-d");
        let newer_l0 = write_table(dir.path(), 21, 0, newer_l0_entries);

        let plan = CompactionPlan {
            inputs: vec![newer_l0, base, older_l0],
            output_level: 1,
        };
        let partitions = merge(dir.path(), &plan, true).expect("streaming merge");
        let mut merged = BTreeMap::new();
        for mut partition in partitions {
            merged.append(&mut partition);
        }

        assert_eq!(merged_value(&merged, b"a"), Some(b"newest".as_slice()));
        assert_eq!(merged_value(&merged, b"b"), Some(b"older-b".as_slice()));
        assert_eq!(merged_value(&merged, b"c"), Some(b"base-c".as_slice()));
        assert_eq!(merged_value(&merged, b"d"), Some(b"newer-d".as_slice()));
    }

    #[test]
    fn streaming_merge_drops_terminal_level_tombstones() {
        let dir = tempdir().expect("tempdir");

        let mut old_entries = BTreeMap::new();
        put(&mut old_entries, b"gone", b"old-value");
        let old = write_table(dir.path(), 30, MAX_LEVEL, old_entries);

        let mut delete_entries = BTreeMap::new();
        delete_entries.insert(b"gone".to_vec(), None);
        let delete = write_table(dir.path(), 31, MAX_LEVEL - 1, delete_entries);

        let plan = CompactionPlan {
            inputs: vec![old, delete],
            output_level: MAX_LEVEL,
        };
        let partitions = merge(dir.path(), &plan, true).expect("terminal streaming merge");
        assert!(
            partitions
                .iter()
                .all(|partition| !partition.contains_key(b"gone".as_slice()))
        );
    }

    #[test]
    fn level_zero_plan_bounds_streaming_fan_in() {
        let mut manifest = Manifest::default();
        for number in 2..22 {
            let mut table = TableFileMeta::without_range(number);
            table.level = 0;
            manifest.table_numbers.push(number);
            manifest.table_files.push(table);
        }

        let plan = plan(&manifest, true).expect("forced level-zero plan");
        let level_zero_inputs = plan
            .inputs
            .iter()
            .filter(|table| table.level == 0)
            .count();
        assert_eq!(level_zero_inputs, MAX_LEVEL_ZERO_INPUTS_PER_PASS);
    }

    #[test]
    fn level_zero_plan_bounds_source_bytes() {
        let mut manifest = Manifest::default();
        for number in 2..8 {
            let mut table = TableFileMeta::without_range(number);
            table.level = 0;
            table.file_size = 6 * 1024 * 1024;
            manifest.table_numbers.push(number);
            manifest.table_files.push(table);
        }

        let plan = plan(&manifest, true).expect("byte-bounded level-zero plan");
        let level_zero_inputs = plan
            .inputs
            .iter()
            .filter(|table| table.level == 0)
            .collect::<Vec<_>>();
        assert_eq!(level_zero_inputs.len(), 2);
        assert!(
            level_zero_inputs
                .iter()
                .map(|table| table.file_size)
                .sum::<u64>()
                <= MAX_COMPACTION_SOURCE_BYTES
        );
    }

    #[test]
    fn level_zero_plan_keeps_one_oversized_source() {
        let mut manifest = Manifest::default();
        let mut oversized = TableFileMeta::without_range(2);
        oversized.level = 0;
        oversized.file_size = MAX_COMPACTION_SOURCE_BYTES.saturating_mul(2);
        manifest.table_numbers.push(oversized.number);
        manifest.table_files.push(oversized);

        let plan = plan(&manifest, true).expect("oversized level-zero plan");
        assert_eq!(
            plan.inputs
                .iter()
                .filter(|table| table.level == 0)
                .count(),
            1
        );
    }
}
