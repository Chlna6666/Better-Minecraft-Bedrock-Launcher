//! Optimistic prepare/stage support for typed Bedrock block edits.
//!
//! Heavy chunk decode, palette rewrite, heightmap recomputation and block-entity encoding can be
//! prepared away from a latency-sensitive authority thread. The resulting raw mutations are not
//! written until the caller revalidates the exact source chunk records and stages them into a world
//! transaction under its own write-serialization boundary.

use super::block_edit::{BlockEdit, BlockEditOptions, apply_block_edits_blocking};
use crate::database::{MemoryStorage, StorageOp, WorldStorage};
use crate::{
    BedrockWorld, BedrockWorldError, BedrockWorldOpenOptions, BlockPos, BlockState, ChunkPos,
    Dimension, Result, WorldStorageHandle, WorldTransaction, WriteGuard,
};
use bytes::Bytes;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreparedChunkSource {
    pos: ChunkPos,
    records: BTreeMap<Bytes, Bytes>,
}

/// Exact primary-layer Bedrock block state that must exist in the preparation snapshot.
///
/// Expectations are evaluated only after every involved chunk has been copied into the isolated
/// [`MemoryStorage`] snapshot. This closes the observation-to-preparation window for conditional
/// edits such as paired-door breaking: the expected state and the encoded replacement are derived
/// from the same immutable chunk snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct PrimaryBlockStateExpectation {
    /// Dimension containing the expected block.
    pub dimension: Dimension,
    /// Absolute block position to match.
    pub position: BlockPos,
    /// Exact persisted primary-layer block state required at `position`.
    pub expected: BlockState,
}

impl PrimaryBlockStateExpectation {
    /// Creates one exact primary-layer block-state expectation.
    #[must_use]
    pub const fn new(dimension: Dimension, position: BlockPos, expected: BlockState) -> Self {
        Self {
            dimension,
            position,
            expected,
        }
    }
}

/// Result of revalidating the raw chunk records used by a prepared edit batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreparedBlockEditValidation {
    /// Every source chunk still has exactly the raw records observed during preparation.
    Current,
    /// One or more source chunks changed after preparation and the batch must not be staged.
    Stale {
        /// Chunks whose raw record sets no longer match the preparation snapshot.
        chunks: BTreeSet<ChunkPos>,
    },
}

/// A fully encoded typed block-edit batch that has not yet mutated persistent storage.
///
/// The object owns the exact raw source snapshots consulted during preparation and the raw mutations
/// produced by running the ordinary typed editor against an isolated in-memory copy. Callers must
/// revalidate the sources and stage the batch while holding the same external serialization boundary
/// that excludes competing writers for every affected chunk; validation alone is intentionally not a
/// storage-level compare-and-swap primitive.
#[derive(Debug, Clone)]
pub struct PreparedBlockEditBatch {
    edited_blocks: usize,
    affected_chunks: BTreeSet<ChunkPos>,
    source_chunks: Vec<PreparedChunkSource>,
    operations: Vec<StorageOp>,
}

impl PreparedBlockEditBatch {
    /// Returns the number of typed block edits represented by this batch.
    #[must_use]
    pub const fn edited_blocks(&self) -> usize {
        self.edited_blocks
    }

    /// Returns the chunks whose typed terrain records are affected by this batch.
    #[must_use]
    pub fn affected_chunks(&self) -> &BTreeSet<ChunkPos> {
        &self.affected_chunks
    }

    /// Returns whether preparation produced no raw storage mutations.
    #[must_use]
    pub fn is_noop(&self) -> bool {
        self.operations.is_empty()
    }

    /// Re-reads the raw records for every source chunk and reports whether the prepared encoding is
    /// still based on the current persisted representation.
    ///
    /// The caller must keep competing writers excluded from the returned `Current` decision through
    /// the subsequent [`Self::stage`] and transaction commit. This method deliberately performs only
    /// raw record loading/comparison; palette decode, heightmap recomputation and NBT re-encoding are
    /// not repeated on the commit path.
    pub fn validate_sources_blocking<S>(
        &self,
        world: &BedrockWorld<S>,
    ) -> Result<PreparedBlockEditValidation>
    where
        S: WorldStorageHandle,
    {
        let mut stale = BTreeSet::new();
        for source in &self.source_chunks {
            if raw_chunk_records(world, source.pos)? != source.records {
                stale.insert(source.pos);
            }
        }
        if stale.is_empty() {
            Ok(PreparedBlockEditValidation::Current)
        } else {
            Ok(PreparedBlockEditValidation::Stale { chunks: stale })
        }
    }

    /// Stages the already encoded raw mutations into an existing world transaction.
    ///
    /// This does not validate sources or commit the transaction. Callers are expected to call
    /// [`Self::validate_sources_blocking`] after acquiring their authoritative chunk-write boundary,
    /// then stage and commit without releasing that boundary in between.
    pub fn stage<S>(&self, transaction: &mut WorldTransaction<'_, S>)
    where
        S: WorldStorageHandle,
    {
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

/// Prepares one atomic typed block-edit batch without changing the source world.
///
/// The source chunks are copied into [`MemoryStorage`] and the ordinary typed editor runs against that
/// isolated copy. Consequently this API shares exactly the same chunk compatibility, palette,
/// secondary-layer, heightmap and block-entity semantics as [`apply_block_edits_blocking`] instead of
/// maintaining a second encoder. The returned batch can later be revalidated and staged with no
/// repeated palette/NBT encoding work.
///
/// A prepared batch is intentionally limited to at most `options.commit_batch_chunks` distinct
/// chunks because its eventual persistent stage is one atomic transaction. Use multiple prepared
/// batches when a caller wants a wider bounded write.
pub fn prepare_block_edits_blocking<S>(
    world: &BedrockWorld<S>,
    edits: &[BlockEdit],
    guard: &WriteGuard,
    options: BlockEditOptions,
) -> Result<PreparedBlockEditBatch>
where
    S: WorldStorageHandle,
{
    prepare_block_edits_with_primary_expectations_blocking(world, edits, &[], guard, options)?
        .ok_or_else(|| {
            BedrockWorldError::Validation(
                "an empty primary BlockState expectation set cannot mismatch".to_string(),
            )
        })
}

/// Prepares one atomic typed block-edit batch only when exact primary block states match the same
/// source snapshot used by the typed editor.
///
/// `Ok(None)` means at least one expected primary [`BlockState`] did not match the isolated source
/// snapshot. No persistent write occurs and no typed edit is encoded in that case. Expectation chunks
/// are included in the batch's raw source snapshot even when an expectation is outside the edited
/// positions, so a successful preparation also revalidates those chunks before staging.
pub fn prepare_block_edits_if_primary_states_match_blocking<S>(
    world: &BedrockWorld<S>,
    edits: &[BlockEdit],
    expectations: &[PrimaryBlockStateExpectation],
    guard: &WriteGuard,
    options: BlockEditOptions,
) -> Result<Option<PreparedBlockEditBatch>>
where
    S: WorldStorageHandle,
{
    prepare_block_edits_with_primary_expectations_blocking(
        world,
        edits,
        expectations,
        guard,
        options,
    )
}

fn prepare_block_edits_with_primary_expectations_blocking<S>(
    world: &BedrockWorld<S>,
    edits: &[BlockEdit],
    expectations: &[PrimaryBlockStateExpectation],
    guard: &WriteGuard,
    options: BlockEditOptions,
) -> Result<Option<PreparedBlockEditBatch>>
where
    S: WorldStorageHandle,
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
        expectations
            .iter()
            .map(|expectation| expectation.position.to_chunk_pos(expectation.dimension)),
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
        .map(|pos| {
            raw_chunk_records(world, pos).map(|records| PreparedChunkSource { pos, records })
        })
        .collect::<Result<Vec<_>>>()?;

    let memory = MemoryStorage::new();
    for source in &source_chunks {
        for (key, value) in &source.records {
            memory.put(key, value)?;
        }
    }
    let prepared_world = BedrockWorld::from_typed_storage_with_format(
        world.path().to_path_buf(),
        memory,
        BedrockWorldOpenOptions {
            read_only: false,
            ..BedrockWorldOpenOptions::default()
        },
        world.format(),
    );

    if !primary_expectations_match_blocking(&prepared_world, expectations)? {
        return Ok(None);
    }

    if edits.is_empty() {
        return Ok(Some(PreparedBlockEditBatch {
            edited_blocks: 0,
            affected_chunks,
            source_chunks,
            operations: Vec::new(),
        }));
    }

    let result = apply_block_edits_blocking(
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

    Ok(Some(PreparedBlockEditBatch {
        edited_blocks: result.edited_blocks,
        affected_chunks: result.affected_chunks,
        source_chunks,
        operations,
    }))
}

fn primary_expectations_match_blocking<S>(
    world: &BedrockWorld<S>,
    expectations: &[PrimaryBlockStateExpectation],
) -> Result<bool>
where
    S: WorldStorageHandle,
{
    let mut grouped = BTreeMap::<Dimension, Vec<&PrimaryBlockStateExpectation>>::new();
    for expectation in expectations {
        grouped
            .entry(expectation.dimension)
            .or_default()
            .push(expectation);
    }

    for (dimension, group) in grouped {
        let positions = group.iter().map(|expectation| expectation.position);
        let states = world.get_block_states_at_blocking(dimension, positions)?;
        if states.len() != group.len() {
            return Err(BedrockWorldError::CorruptWorld(format!(
                "primary BlockState expectation query returned {} states for {} positions",
                states.len(),
                group.len()
            )));
        }
        for (expectation, actual) in group.into_iter().zip(states) {
            if actual.pos != expectation.position
                || actual.state.as_ref() != Some(&expectation.expected)
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn raw_chunk_records<S>(
    world: &BedrockWorld<S>,
    pos: ChunkPos,
) -> Result<BTreeMap<Bytes, Bytes>>
where
    S: WorldStorageHandle,
{
    let chunk = world.get_chunk_blocking(pos)?;
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
}
