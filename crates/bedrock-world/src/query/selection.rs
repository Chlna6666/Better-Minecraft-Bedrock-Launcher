//! Exact non-rectangular chunk selection primitives and queries.

use crate::chunk::{ChunkPos, Dimension};
use crate::error::{BedrockWorldError, BedrockWorldErrorKind, Result};
use crate::query::{
    ChunkRecordQuery, ChunkValue, RegionOverlayQueryOptions, SelectionStats,
    SlimeChunkBounds, VillageOverlayIndex, is_slime_chunk, load_chunks,
};
use crate::storage::{CancelFlag, MemoryStorage, WorldStorage};
use crate::world::{OpenOptions, World, StorageBackend};
use std::collections::BTreeSet;

/// A validated, non-empty, exact set of chunks from one Bedrock dimension.
///
/// This is the canonical selection type for non-rectangular world operations.
/// Its bounding rectangle is metadata only: membership is always determined by
/// the explicit chunk set. L-shaped, T-shaped, cross-shaped and disconnected
/// selections therefore keep their holes and disconnected regions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExactChunkSelection {
    dimension: Dimension,
    chunks: BTreeSet<ChunkPos>,
}

impl ExactChunkSelection {
    /// Creates an exact selection from arbitrary chunk positions.
    ///
    /// Duplicate positions are removed. The selection must contain at least one
    /// chunk and all chunks must belong to the same dimension.
    pub fn new<I>(positions: I) -> Result<Self>
    where
        I: IntoIterator<Item = ChunkPos>,
    {
        let chunks = positions.into_iter().collect::<BTreeSet<_>>();
        let first = *chunks.first().ok_or_else(|| {
            BedrockWorldError::Validation("exact chunk selection is empty".to_string())
        })?;
        if chunks
            .iter()
            .any(|position| position.dimension != first.dimension)
        {
            return Err(BedrockWorldError::Validation(
                "exact chunk selection cannot span multiple dimensions".to_string(),
            ));
        }
        Ok(Self {
            dimension: first.dimension,
            chunks,
        })
    }

    /// Creates a one-chunk exact selection.
    #[must_use]
    pub fn single(chunk: ChunkPos) -> Self {
        Self {
            dimension: chunk.dimension,
            chunks: BTreeSet::from([chunk]),
        }
    }

    /// Returns the dimension shared by all selected chunks.
    #[must_use]
    pub const fn dimension(&self) -> Dimension {
        self.dimension
    }

    /// Returns the exact number of selected chunks.
    #[must_use]
    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    /// Returns whether the exact selection is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// Returns whether a chunk is part of the exact selection.
    #[must_use]
    pub fn contains(&self, chunk: ChunkPos) -> bool {
        self.chunks.contains(&chunk)
    }

    /// Iterates the selected chunks in the set's stable key order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = ChunkPos> + '_ {
        self.chunks.iter().copied()
    }

    /// Returns the selected chunks in stable row-major `z, x` order.
    #[must_use]
    pub fn to_vec(&self) -> Vec<ChunkPos> {
        let mut chunks = self.iter().collect::<Vec<_>>();
        chunks.sort_unstable_by_key(|chunk| (chunk.z, chunk.x));
        chunks
    }

    /// Returns the minimal bounding rectangle around the exact selection.
    #[must_use]
    pub fn bounds(&self) -> SlimeChunkBounds {
        let first = *self
            .chunks
            .first()
            .expect("ExactChunkSelection invariant requires at least one chunk");
        let mut min_chunk_x = first.x;
        let mut max_chunk_x = first.x;
        let mut min_chunk_z = first.z;
        let mut max_chunk_z = first.z;
        for position in self.chunks.iter().copied().skip(1) {
            min_chunk_x = min_chunk_x.min(position.x);
            max_chunk_x = max_chunk_x.max(position.x);
            min_chunk_z = min_chunk_z.min(position.z);
            max_chunk_z = max_chunk_z.max(position.z);
        }
        SlimeChunkBounds {
            dimension: self.dimension,
            min_chunk_x,
            max_chunk_x,
            min_chunk_z,
            max_chunk_z,
        }
    }

    /// Returns whether this exact selection completely fills its bounding box.
    #[must_use]
    pub fn is_rectangular(&self) -> bool {
        self.len() == self.bounds().chunk_count()
    }

    /// Returns a new selection containing this selection plus the supplied chunks.
    pub fn union<I>(&self, positions: I) -> Result<Self>
    where
        I: IntoIterator<Item = ChunkPos>,
    {
        let mut chunks = self.chunks.clone();
        for position in positions {
            if position.dimension != self.dimension {
                return Err(BedrockWorldError::Validation(
                    "exact chunk selection union cannot span multiple dimensions".to_string(),
                ));
            }
            chunks.insert(position);
        }
        Ok(Self {
            dimension: self.dimension,
            chunks,
        })
    }

    /// Returns a translated copy of the exact selection.
    #[must_use]
    pub fn translated(&self, delta_x: i32, delta_z: i32) -> Self {
        let chunks = self
            .chunks
            .iter()
            .copied()
            .map(|chunk| ChunkPos {
                x: chunk.x.saturating_add(delta_x),
                z: chunk.z.saturating_add(delta_z),
                dimension: self.dimension,
            })
            .collect();
        Self {
            dimension: self.dimension,
            chunks,
        }
    }

    /// Decomposes the exact set into fully-selected rectangles.
    #[must_use]
    pub fn rectangle_cover(&self) -> Vec<SlimeChunkBounds> {
        let mut remaining = self
            .chunks
            .iter()
            .map(|chunk| (chunk.z, chunk.x))
            .collect::<BTreeSet<_>>();
        let mut rectangles = Vec::new();

        while let Some(&(start_z, start_x)) = remaining.first() {
            let mut max_x = start_x;
            while max_x < i32::MAX && remaining.contains(&(start_z, max_x.saturating_add(1))) {
                max_x = max_x.saturating_add(1);
            }

            let mut max_z = start_z;
            while max_z < i32::MAX {
                let next_z = max_z.saturating_add(1);
                if (start_x..=max_x).all(|x| remaining.contains(&(next_z, x))) {
                    max_z = next_z;
                } else {
                    break;
                }
            }

            for z in start_z..=max_z {
                for x in start_x..=max_x {
                    remaining.remove(&(z, x));
                }
            }
            rectangles.push(SlimeChunkBounds {
                dimension: self.dimension,
                min_chunk_x: start_x,
                max_chunk_x: max_x,
                min_chunk_z: start_z,
                max_chunk_z: max_z,
            });
        }

        rectangles
    }

    fn intersects_bounds(&self, bounds: SlimeChunkBounds) -> bool {
        self.chunks.iter().any(|position| {
            position.dimension == bounds.dimension
                && position.x >= bounds.min_chunk_x
                && position.x <= bounds.max_chunk_x
                && position.z >= bounds.min_chunk_z
                && position.z <= bounds.max_chunk_z
        })
    }
}

/// Rasterizes a chunk-grid line including both endpoints.
pub fn rasterize_chunk_line(start: ChunkPos, end: ChunkPos) -> Result<Vec<ChunkPos>> {
    if start.dimension != end.dimension {
        return Err(BedrockWorldError::Validation(
            "chunk selection line cannot span multiple dimensions".to_string(),
        ));
    }

    let mut x = i64::from(start.x);
    let mut z = i64::from(start.z);
    let end_x = i64::from(end.x);
    let end_z = i64::from(end.z);
    let dx = (end_x - x).abs();
    let dz = -(end_z - z).abs();
    let step_x = if x < end_x { 1 } else { -1 };
    let step_z = if z < end_z { 1 } else { -1 };
    let mut error = dx + dz;
    let mut chunks =
        Vec::with_capacity(usize::try_from(dx.max(-dz).saturating_add(1)).unwrap_or(usize::MAX));

    loop {
        chunks.push(ChunkPos {
            x: x as i32,
            z: z as i32,
            dimension: start.dimension,
        });
        if x == end_x && z == end_z {
            break;
        }
        let doubled_error = error.saturating_mul(2);
        if doubled_error >= dz {
            error = error.saturating_add(dz);
            x = x.saturating_add(step_x);
        }
        if doubled_error <= dx {
            error = error.saturating_add(dx);
            z = z.saturating_add(step_z);
        }
    }
    Ok(chunks)
}

/// Queries aggregate statistics for a validated exact chunk selection.
pub fn exact_selection_stats<S>(
    world: &World<S>,
    selection: &ExactChunkSelection,
    options: RegionOverlayQueryOptions,
) -> Result<SelectionStats>
where
    S: StorageBackend,
{
    if selection.len() > options.max_chunks {
        return Err(BedrockWorldError::Validation(format!(
            "query covers {} exact chunks, limit is {}",
            selection.len(),
            options.max_chunks
        )));
    }

    let mut stats = SelectionStats {
        bounds: Some(selection.bounds()),
        chunk_count: selection.len(),
        slime_chunks: if options.include_slime {
            selection
                .iter()
                .filter(|position| is_slime_chunk(*position))
                .count()
        } else {
            0
        },
        ..SelectionStats::default()
    };

    if exact_stats_need_chunk_records(options) {
        let records = load_chunks(
            world,
            selection.iter(),
            ChunkRecordQuery {
                entities: options.include_entities,
                block_entities: options.include_block_entities,
                pending_ticks: options.include_pending_ticks,
                hardcoded_spawn_areas: options.include_hardcoded_spawn_areas,
            },
            None,
        )?;
        for chunk in records {
            if chunk.records.is_empty() {
                stats.missing_chunks = stats.missing_chunks.saturating_add(1);
                continue;
            }
            stats.loaded_chunks = stats.loaded_chunks.saturating_add(1);
            for record in chunk.records {
                match record.value {
                    ChunkValue::Entities(entities) if options.include_entities => {
                        stats.entity_count = capped_add(
                            stats.entity_count,
                            entities.len(),
                            options.max_items_per_kind,
                        );
                    }
                    ChunkValue::BlockEntities(block_entities)
                        if options.include_block_entities =>
                    {
                        stats.block_entity_count = capped_add(
                            stats.block_entity_count,
                            block_entities.len(),
                            options.max_items_per_kind,
                        );
                    }
                    ChunkValue::PendingTicks(ticks)
                        if options.include_pending_ticks =>
                    {
                        stats.pending_tick_count = capped_add(
                            stats.pending_tick_count,
                            ticks.len(),
                            options.max_items_per_kind,
                        );
                    }
                    ChunkValue::HardcodedSpawnAreas(areas)
                        if options.include_hardcoded_spawn_areas =>
                    {
                        stats.hardcoded_spawn_area_count = capped_add(
                            stats.hardcoded_spawn_area_count,
                            areas.len(),
                            options.max_items_per_kind,
                        );
                    }
                    _ => {}
                }
            }
        }
    }

    if options.include_villages && options.max_items_per_kind > 0 {
        let cancel = CancelFlag::new();
        let index = VillageOverlayIndex::build(world, &cancel)?;
        stats.village_count = index
            .villages
            .iter()
            .filter(|village| {
                village
                    .bounds
                    .is_some_and(|village_bounds| selection.intersects_bounds(village_bounds))
            })
            .take(options.max_items_per_kind)
            .count();
    }

    Ok(stats)
}

/// Queries aggregate statistics for an arbitrary set of chunk positions.
pub fn chunk_selection_stats<S, I>(
    world: &World<S>,
    positions: I,
    options: RegionOverlayQueryOptions,
) -> Result<SelectionStats>
where
    S: StorageBackend,
    I: IntoIterator<Item = ChunkPos>,
{
    let positions = positions.into_iter().collect::<Vec<_>>();
    if positions.is_empty() {
        return Ok(SelectionStats::default());
    }
    let selection = ExactChunkSelection::new(positions)?;
    exact_selection_stats(world, &selection, options)
}

fn exact_stats_need_chunk_records(options: RegionOverlayQueryOptions) -> bool {
    options.include_hardcoded_spawn_areas
        || options.include_entities
        || options.include_block_entities
        || options.include_pending_ticks
}

fn capped_add(current: usize, additional: usize, limit: usize) -> usize {
    current.saturating_add(additional).min(limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn chunk(x: i32, z: i32) -> ChunkPos {
        ChunkPos {
            x,
            z,
            dimension: Dimension::Overworld,
        }
    }

    #[test]
    fn exact_selection_keeps_holes_and_disconnected_chunks() {
        let selection =
            ExactChunkSelection::new([chunk(0, 0), chunk(1, 0), chunk(0, 1), chunk(4, 4)])
                .expect("exact selection");

        assert_eq!(selection.len(), 4);
        assert!(selection.contains(chunk(0, 1)));
        assert!(!selection.contains(chunk(1, 1)));
        assert!(!selection.is_rectangular());
        assert_eq!(
            selection
                .rectangle_cover()
                .iter()
                .map(|bounds| bounds.chunk_count())
                .sum::<usize>(),
            4
        );
    }

    #[test]
    fn exact_selection_to_vec_is_row_major() {
        let selection =
            ExactChunkSelection::new([chunk(2, 0), chunk(0, 1), chunk(1, 0), chunk(0, 0)])
                .expect("exact selection");
        assert_eq!(
            selection.to_vec(),
            vec![chunk(0, 0), chunk(1, 0), chunk(2, 0), chunk(0, 1)]
        );
    }

    #[test]
    fn rasterized_line_does_not_fill_bounding_rectangle() {
        let line = rasterize_chunk_line(chunk(0, 0), chunk(3, 2)).expect("line");
        assert_eq!(line.first(), Some(&chunk(0, 0)));
        assert_eq!(line.last(), Some(&chunk(3, 2)));
        assert!(line.len() < 12);
    }

    #[test]
    fn exact_stats_do_not_fill_bounding_rectangle() {
        let storage = Arc::new(MemoryStorage::default()) as Arc<dyn WorldStorage>;
        let world = World::from_storage("memory", storage, OpenOptions::default());
        let selection =
            ExactChunkSelection::new([chunk(0, 0), chunk(2, 0)]).expect("exact selection");
        let stats = exact_selection_stats(
            &world,
            &selection,
            RegionOverlayQueryOptions {
                include_slime: true,
                include_hardcoded_spawn_areas: false,
                include_entities: false,
                include_block_entities: false,
                include_pending_ticks: false,
                include_villages: false,
                max_chunks: 8,
                max_items_per_kind: 8,
            },
        )
        .expect("exact selection stats");

        assert_eq!(stats.chunk_count, 2);
        assert_eq!(stats.bounds.expect("bounds").chunk_count(), 3);
    }

    #[test]
    fn exact_selection_rejects_mixed_dimensions() {
        let error = ExactChunkSelection::new([
            chunk(0, 0),
            ChunkPos {
                x: 1,
                z: 0,
                dimension: Dimension::Nether,
            },
        ])
        .expect_err("mixed dimensions must fail");

        assert_eq!(error.kind(), BedrockWorldErrorKind::Validation);
    }
}
