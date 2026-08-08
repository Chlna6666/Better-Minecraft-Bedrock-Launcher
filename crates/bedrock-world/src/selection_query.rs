//! Exact non-rectangular selection queries.

use crate::query::{
    ChunkRecordQuery, RegionOverlayQueryOptions, SelectionStats, SlimeChunkBounds,
    VillageOverlayIndex, is_slime_chunk, query_chunk_records_many_blocking,
};
use crate::{
    BedrockWorld, CancelFlag, ChunkPos, ParsedChunkRecordValue, Result, WorldStorageHandle,
};
use crate::error::BedrockWorldError;
use std::collections::BTreeSet;

/// Queries aggregate statistics for an explicit set of chunk positions.
///
/// Unlike [`crate::query_selection_stats_blocking`], this function never expands
/// the input to its bounding rectangle. L-shaped, T-shaped, cross-shaped and
/// disconnected selections therefore keep their holes and disconnected regions.
pub fn query_selection_stats_chunks_blocking<S, I>(
    world: &BedrockWorld<S>,
    positions: I,
    options: RegionOverlayQueryOptions,
) -> Result<SelectionStats>
where
    S: WorldStorageHandle,
    I: IntoIterator<Item = ChunkPos>,
{
    let positions = positions.into_iter().collect::<BTreeSet<_>>();
    if positions.is_empty() {
        return Ok(SelectionStats::default());
    }
    if positions.len() > options.max_chunks {
        return Err(BedrockWorldError::Validation(format!(
            "query covers {} exact chunks, limit is {}",
            positions.len(),
            options.max_chunks
        )));
    }

    let bounds = exact_chunk_bounds(&positions)?;
    let mut stats = SelectionStats {
        bounds: Some(bounds),
        chunk_count: positions.len(),
        slime_chunks: if options.include_slime {
            positions
                .iter()
                .copied()
                .filter(|position| is_slime_chunk(*position))
                .count()
        } else {
            0
        },
        ..SelectionStats::default()
    };

    if exact_stats_need_chunk_records(options) {
        let records = query_chunk_records_many_blocking(
            world,
            positions.iter().copied(),
            ChunkRecordQuery {
                entities: options.include_entities,
                block_entities: options.include_block_entities,
                pending_ticks: options.include_pending_ticks,
                hardcoded_spawn_areas: options.include_hardcoded_spawn_areas,
            },
        )?;
        for chunk in records {
            if chunk.records.is_empty() {
                stats.missing_chunks = stats.missing_chunks.saturating_add(1);
                continue;
            }
            stats.loaded_chunks = stats.loaded_chunks.saturating_add(1);
            for record in chunk.records {
                match record.value {
                    ParsedChunkRecordValue::Entities(entities) if options.include_entities => {
                        stats.entity_count = capped_add(
                            stats.entity_count,
                            entities.len(),
                            options.max_items_per_kind,
                        );
                    }
                    ParsedChunkRecordValue::BlockEntities(block_entities)
                        if options.include_block_entities =>
                    {
                        stats.block_entity_count = capped_add(
                            stats.block_entity_count,
                            block_entities.len(),
                            options.max_items_per_kind,
                        );
                    }
                    ParsedChunkRecordValue::PendingTicks(ticks)
                        if options.include_pending_ticks =>
                    {
                        stats.pending_tick_count = capped_add(
                            stats.pending_tick_count,
                            ticks.len(),
                            options.max_items_per_kind,
                        );
                    }
                    ParsedChunkRecordValue::HardcodedSpawnAreas(areas)
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
        let index = VillageOverlayIndex::build_blocking_with_control(world, &cancel)?;
        stats.village_count = index
            .villages
            .iter()
            .filter(|village| {
                village
                    .bounds
                    .is_some_and(|village_bounds| exact_bounds_intersect_chunks(village_bounds, &positions))
            })
            .take(options.max_items_per_kind)
            .count();
    }

    Ok(stats)
}

fn exact_stats_need_chunk_records(options: RegionOverlayQueryOptions) -> bool {
    options.include_hardcoded_spawn_areas
        || options.include_entities
        || options.include_block_entities
        || options.include_pending_ticks
}

fn exact_chunk_bounds(positions: &BTreeSet<ChunkPos>) -> Result<SlimeChunkBounds> {
    let first = *positions
        .first()
        .ok_or_else(|| BedrockWorldError::Validation("exact selection is empty".to_string()))?;
    if positions
        .iter()
        .any(|position| position.dimension != first.dimension)
    {
        return Err(BedrockWorldError::Validation(
            "exact selection cannot span multiple dimensions".to_string(),
        ));
    }

    let mut min_chunk_x = first.x;
    let mut max_chunk_x = first.x;
    let mut min_chunk_z = first.z;
    let mut max_chunk_z = first.z;
    for position in positions.iter().copied().skip(1) {
        min_chunk_x = min_chunk_x.min(position.x);
        max_chunk_x = max_chunk_x.max(position.x);
        min_chunk_z = min_chunk_z.min(position.z);
        max_chunk_z = max_chunk_z.max(position.z);
    }
    Ok(SlimeChunkBounds {
        dimension: first.dimension,
        min_chunk_x,
        max_chunk_x,
        min_chunk_z,
        max_chunk_z,
    })
}

fn exact_bounds_intersect_chunks(
    bounds: SlimeChunkBounds,
    positions: &BTreeSet<ChunkPos>,
) -> bool {
    positions.iter().any(|position| {
        position.dimension == bounds.dimension
            && position.x >= bounds.min_chunk_x
            && position.x <= bounds.max_chunk_x
            && position.z >= bounds.min_chunk_z
            && position.z <= bounds.max_chunk_z
    })
}

fn capped_add(current: usize, additional: usize, limit: usize) -> usize {
    current.saturating_add(additional).min(limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Dimension, MemoryStorage, OpenOptions};
    use std::sync::Arc;

    fn chunk(x: i32, z: i32) -> ChunkPos {
        ChunkPos {
            x,
            z,
            dimension: Dimension::Overworld,
        }
    }

    #[test]
    fn exact_stats_do_not_fill_bounding_rectangle() {
        let storage = Arc::new(MemoryStorage::default()) as Arc<dyn crate::WorldStorage>;
        let world = BedrockWorld::from_storage("memory", storage, OpenOptions::default());
        let stats = query_selection_stats_chunks_blocking(
            &world,
            [chunk(0, 0), chunk(2, 0)],
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
    fn exact_stats_reject_mixed_dimensions() {
        let storage = Arc::new(MemoryStorage::default()) as Arc<dyn crate::WorldStorage>;
        let world = BedrockWorld::from_storage("memory", storage, OpenOptions::default());
        let error = query_selection_stats_chunks_blocking(
            &world,
            [
                chunk(0, 0),
                ChunkPos {
                    x: 1,
                    z: 0,
                    dimension: Dimension::Nether,
                },
            ],
            RegionOverlayQueryOptions::default(),
        )
        .expect_err("mixed dimensions must fail");

        assert_eq!(error.kind(), crate::BedrockWorldErrorKind::Validation);
    }
}
