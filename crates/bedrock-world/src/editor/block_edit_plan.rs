//! Optimistic block-edit plans validated against source chunk snapshots.
//!
//! Heavy chunk decode, palette rewrite, heightmap recomputation and block-entity encoding can be
//! prepared away from a latency-sensitive authority thread. The resulting raw mutations are not
//! written until the caller revalidates the exact source chunk records and stages them into a world
//! transaction under its own write-serialization boundary.

use super::block_edit::{BlockEdit, BlockEditOptions, apply_block_edits};
use crate::storage::{MemoryStorage, StorageOp, WorldStorage};
use crate::{
    BedrockWorldError, BlockPos, BlockState, ChunkPos, Dimension, OpenOptions, Result,
    StorageBackend, World, WorldTransaction, WriteGuard,
};
use bytes::Bytes;
use std::collections::{BTreeMap, BTreeSet};

// One atomic LevelDB batch is encoded into a contiguous buffer before the WAL write. Keep the
// prepared payload below this bound rather than falling back to partially committed chunk batches.
const MAX_ATOMIC_BATCH_BYTES: usize = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceChunk {
    pos: ChunkPos,
    records: BTreeMap<Bytes, Bytes>,
}

/// Exact primary-layer Bedrock block state required by a prepared edit.
///
/// Conditions are evaluated only after every involved chunk has been copied into the isolated
/// [`MemoryStorage`] snapshot. The expected state and replacement encoding therefore come from the
/// same immutable source snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockStateCondition {
    /// Dimension containing the block.
    pub dimension: Dimension,
    /// Absolute block position to match.
    pub position: BlockPos,
    /// Exact persisted primary-layer block state required at `position`.
    /// An unversioned `minecraft:air` with no properties also matches an omitted all-air
    /// SubChunk. Source chunk records are still checked and the typed editor requires terrain.
    pub expected: BlockState,
}

impl BlockStateCondition {
    /// Creates one exact primary-layer block-state condition.
    #[must_use]
    pub const fn new(dimension: Dimension, position: BlockPos, expected: BlockState) -> Self {
        Self {
            dimension,
            position,
            expected,
        }
    }
}

/// Source-validation state for prepared block edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanStatus {
    /// Every source chunk still has exactly the raw records observed during preparation.
    Current,
    /// One or more source chunks changed after preparation and the edits must not be staged.
    Stale {
        /// Chunks whose raw record sets no longer match the preparation snapshot.
        chunks: BTreeSet<ChunkPos>,
    },
}

/// Fully encoded typed block edits that have not yet mutated persistent storage.
///
/// The value owns the exact raw source snapshots consulted during preparation and the raw mutations
/// produced by running the ordinary typed editor against an isolated in-memory copy. Callers must
/// stage the edits into one transaction. Commit rechecks every source chunk's complete raw record
/// set under the world mutation lock before writing the batch. Direct raw storage writes and
/// external Minecraft processes do not participate in that lock.
#[derive(Debug, Clone)]
pub struct BlockEditPlan {
    edited_blocks: usize,
    affected_chunks: BTreeSet<ChunkPos>,
    source_chunks: Vec<SourceChunk>,
    operations: Vec<StorageOp>,
}

impl BlockEditPlan {
    pub(crate) fn source_world<S>(&self, world: &World<S>) -> Result<World<MemoryStorage>>
    where
        S: StorageBackend,
    {
        let memory = MemoryStorage::new();
        for source in &self.source_chunks {
            for (key, value) in &source.records {
                memory.put(key, value)?;
            }
        }
        Ok(World::from_typed_storage_with_format(
            world.path().to_path_buf(),
            memory,
            OpenOptions {
                read_only: true,
                ..OpenOptions::default()
            },
            world.format(),
        ))
    }

    /// Returns the number of typed block edits represented by this preparation.
    #[must_use]
    pub const fn edited_blocks(&self) -> usize {
        self.edited_blocks
    }

    /// Returns the chunks whose typed terrain records are affected.
    #[must_use]
    pub fn affected_chunks(&self) -> &BTreeSet<ChunkPos> {
        &self.affected_chunks
    }

    /// Returns whether preparation produced persistent storage changes.
    #[must_use]
    pub fn has_changes(&self) -> bool {
        !self.operations.is_empty()
    }

    /// Re-reads every source chunk and reports whether the prepared encoding still matches storage.
    ///
    /// This is an early advisory check; [`Self::stage`] also registers the source records for
    /// authoritative revalidation under the transaction's commit lock. Palette decode, heightmap
    /// recomputation and NBT re-encoding are not repeated on this validation path.
    pub fn validate<S>(&self, world: &World<S>) -> Result<PlanStatus>
    where
        S: StorageBackend,
    {
        let mut stale = BTreeSet::new();
        for source in &self.source_chunks {
            if raw_chunk_records(world, source.pos)? != source.records {
                stale.insert(source.pos);
            }
        }
        if stale.is_empty() {
            Ok(PlanStatus::Current)
        } else {
            Ok(PlanStatus::Stale { chunks: stale })
        }
    }

    /// Stages the already encoded raw mutations into an existing world transaction.
    ///
    /// This registers the exact source record sets for commit-time validation and stages the raw
    /// mutations. It does not write storage; all changes remain discardable until commit.
    pub fn stage<S>(&self, transaction: &mut WorldTransaction<'_, S>)
    where
        S: StorageBackend,
    {
        for source in &self.source_chunks {
            transaction.require_chunk_records(source.pos, source.records.clone());
        }
        for operation in &self.operations {
            match operation {
                StorageOp::Put { key, value } => {
                    transaction.put_raw_key(key.clone(), value.clone());
                }
                StorageOp::Delete { key } => {
                    transaction.delete_raw_key(key.clone());
                }
            }
        }
    }
}

/// Prepares one atomic typed block-edit set without changing the source world.
///
/// `conditions` are exact primary-layer [`BlockState`] requirements evaluated against the same
/// isolated chunk snapshot used by the typed editor. `Ok(None)` means at least one condition did not
/// match. No persistent write occurs and no typed edit is encoded in that case. Passing an empty
/// condition slice performs an unconditional preparation and therefore always returns `Some` on
/// success.
///
/// The source chunks are copied into [`MemoryStorage`] and the ordinary typed editor runs against that
/// isolated copy. This preserves the canonical chunk compatibility, palette, secondary-layer,
/// heightmap and block-entity semantics instead of maintaining a second encoder. Condition-only
/// chunks are retained in the raw source snapshot as well, so successful preparation revalidates them
/// before staging.
///
/// A preparation is intentionally limited to at most `options.commit_batch_chunks` distinct source
/// chunks because its eventual persistent stage is one atomic transaction.
pub fn plan_block_edits<S>(
    world: &World<S>,
    edits: &[BlockEdit],
    conditions: &[BlockStateCondition],
    guard: &WriteGuard,
    options: BlockEditOptions,
) -> Result<Option<BlockEditPlan>>
where
    S: StorageBackend,
{
    guard.validate(world)?;
    if options.commit_batch_chunks == 0 {
        return Err(BedrockWorldError::Validation(
            "commit_batch_chunks must be greater than zero".to_string(),
        ));
    }

    let affected_chunks = edits
        .iter()
        .map(|edit| edit.position.to_chunk_pos(edit.dimension))
        .collect::<BTreeSet<_>>();
    let mut source_chunk_positions = affected_chunks.clone();
    source_chunk_positions.extend(
        conditions
            .iter()
            .map(|condition| condition.position.to_chunk_pos(condition.dimension)),
    );
    if source_chunk_positions.len() > options.commit_batch_chunks {
        return Err(BedrockWorldError::Validation(format!(
            "prepared block edit observes {} chunks but atomic batch limit is {}",
            source_chunk_positions.len(),
            options.commit_batch_chunks
        )));
    }

    let source_chunks = source_chunk_positions
        .iter()
        .copied()
        .map(|pos| raw_chunk_records(world, pos).map(|records| SourceChunk { pos, records }))
        .collect::<Result<Vec<_>>>()?;

    let memory = MemoryStorage::new();
    for source in &source_chunks {
        for (key, value) in &source.records {
            memory.put(key, value)?;
        }
    }
    let prepared_world = World::from_typed_storage_with_format(
        world.path().to_path_buf(),
        memory,
        OpenOptions {
            read_only: false,
            ..OpenOptions::default()
        },
        world.format(),
    );

    if !conditions_match(&prepared_world, conditions)? {
        return Ok(None);
    }

    if edits.is_empty() {
        return Ok(Some(BlockEditPlan {
            edited_blocks: 0,
            affected_chunks,
            source_chunks,
            operations: Vec::new(),
        }));
    }

    let result = apply_block_edits(
        &prepared_world,
        edits,
        guard,
        BlockEditOptions {
            commit_batch_chunks: affected_chunks.len().max(1),
            compact_empty_subchunks: options.compact_empty_subchunks,
        },
    )?;

    let mut operations = Vec::new();
    for source in &source_chunks {
        let current = raw_chunk_records(&prepared_world, source.pos)?;
        append_record_diff(&source.records, &current, &mut operations);
    }
    operations.sort_unstable_by(|left, right| operation_key(left).cmp(operation_key(right)));
    let batch_bytes = encoded_batch_bytes(&operations);
    if operations.len() > u32::MAX as usize || batch_bytes > MAX_ATOMIC_BATCH_BYTES {
        return Err(BedrockWorldError::Validation(format!(
            "prepared atomic block batch has {} operations and {batch_bytes} encoded bytes; limit is {MAX_ATOMIC_BATCH_BYTES} bytes",
            operations.len()
        )));
    }

    Ok(Some(BlockEditPlan {
        edited_blocks: result.edited_blocks,
        affected_chunks: result.affected_chunks,
        source_chunks,
        operations,
    }))
}

fn conditions_match<S>(world: &World<S>, conditions: &[BlockStateCondition]) -> Result<bool>
where
    S: StorageBackend,
{
    let mut grouped = BTreeMap::<Dimension, Vec<&BlockStateCondition>>::new();
    for condition in conditions {
        grouped
            .entry(condition.dimension)
            .or_default()
            .push(condition);
    }

    for (dimension, group) in grouped {
        let positions = group.iter().map(|condition| condition.position);
        let states = world.block_states(dimension, positions)?;
        if states.len() != group.len() {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "BlockState condition query returned {} states for {} positions",
                states.len(),
                group.len()
            )));
        }
        for (condition, actual) in group.into_iter().zip(states) {
            let omitted_air = actual.state.is_none()
                && condition.expected.name == "minecraft:air"
                && condition.expected.states.is_empty()
                && condition.expected.version.is_none();
            if actual.pos != condition.position
                || (!omitted_air && actual.state.as_ref() != Some(&condition.expected))
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn raw_chunk_records<S>(world: &World<S>, pos: ChunkPos) -> Result<BTreeMap<Bytes, Bytes>>
where
    S: StorageBackend,
{
    let chunk = world.chunk(pos)?;
    let mut records = BTreeMap::new();
    for record in chunk.records {
        let key = Bytes::from(record.key.encode());
        if records.insert(key, record.value).is_some() {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "duplicate raw chunk record while preparing typed edit for {pos:?}"
            )));
        }
    }
    Ok(records)
}

fn append_record_diff(
    before: &BTreeMap<Bytes, Bytes>,
    after: &BTreeMap<Bytes, Bytes>,
    operations: &mut Vec<StorageOp>,
) {
    for (key, before_value) in before {
        match after.get(key) {
            Some(after_value) if after_value == before_value => {}
            Some(after_value) => operations.push(StorageOp::Put {
                key: key.clone(),
                value: after_value.clone(),
            }),
            None => operations.push(StorageOp::Delete { key: key.clone() }),
        }
    }
    for (key, value) in after {
        if !before.contains_key(key) {
            operations.push(StorageOp::Put {
                key: key.clone(),
                value: value.clone(),
            });
        }
    }
}

fn operation_key(operation: &StorageOp) -> &[u8] {
    match operation {
        StorageOp::Put { key, .. } | StorageOp::Delete { key } => key.as_ref(),
    }
}

fn encoded_batch_bytes(operations: &[StorageOp]) -> usize {
    operations.iter().fold(12usize, |total, operation| {
        let (key_len, value_len) = match operation {
            StorageOp::Put { key, value } => (key.len(), Some(value.len())),
            StorageOp::Delete { key } => (key.len(), None),
        };
        let total = total
            .saturating_add(1)
            .saturating_add(5)
            .saturating_add(key_len);
        if let Some(value_len) = value_len {
            total.saturating_add(5).saturating_add(value_len)
        } else {
            total
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_diff_is_deterministic_and_preserves_deletes() {
        let before = BTreeMap::from([
            (Bytes::from_static(b"a"), Bytes::from_static(b"old")),
            (Bytes::from_static(b"b"), Bytes::from_static(b"same")),
            (Bytes::from_static(b"c"), Bytes::from_static(b"gone")),
        ]);
        let after = BTreeMap::from([
            (Bytes::from_static(b"a"), Bytes::from_static(b"new")),
            (Bytes::from_static(b"b"), Bytes::from_static(b"same")),
            (Bytes::from_static(b"d"), Bytes::from_static(b"added")),
        ]);
        let mut operations = Vec::new();
        append_record_diff(&before, &after, &mut operations);
        operations.sort_unstable_by(|left, right| operation_key(left).cmp(operation_key(right)));
        assert_eq!(
            operations,
            vec![
                StorageOp::Put {
                    key: Bytes::from_static(b"a"),
                    value: Bytes::from_static(b"new"),
                },
                StorageOp::Delete {
                    key: Bytes::from_static(b"c"),
                },
                StorageOp::Put {
                    key: Bytes::from_static(b"d"),
                    value: Bytes::from_static(b"added"),
                },
            ]
        );
    }

    #[test]
    fn atomic_batch_size_includes_leveldb_encoding_overhead() {
        let operations = [
            StorageOp::Put {
                key: Bytes::from_static(b"a"),
                value: Bytes::from_static(b"bc"),
            },
            StorageOp::Delete {
                key: Bytes::from_static(b"def"),
            },
        ];
        assert_eq!(
            encoded_batch_bytes(&operations),
            12 + (1 + 5 + 1 + 5 + 2) + (1 + 5 + 3)
        );
    }
}
