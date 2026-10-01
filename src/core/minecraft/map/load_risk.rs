//! Offline screening of saved chunk contents, never measured MSPT or TPS.

use super::viewer_cache::MapInfoOverlaySnapshot;
use bedrock_world::nbt::NbtTag;
use bedrock_world::query::RegionOverlayQuery;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RiskLevel {
    Orange,
    Red,
}

/// Saved-record counts; thresholds are project heuristics, not tick timings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ChunkLoad {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub entities: u32,
    pub ticking_block_entities: u32,
    pub pending_ticks: u32,
}

impl ChunkLoad {
    pub(crate) fn level(self) -> Option<RiskLevel> {
        if self.entities >= 128 || self.ticking_block_entities >= 64 || self.pending_ticks >= 1024 {
            Some(RiskLevel::Red)
        } else if self.entities >= 64
            || self.ticking_block_entities >= 32
            || self.pending_ticks >= 256
        {
            Some(RiskLevel::Orange)
        } else {
            None
        }
    }
}

/// Known ticking-capable NBT ids. Presence does not prove the block is active.
pub(crate) fn is_ticking_block_entity(id: Option<&str>) -> bool {
    matches!(
        id,
        Some(
            "Hopper"
                | "Furnace"
                | "BlastFurnace"
                | "Smoker"
                | "BrewingStand"
                | "CommandBlock"
                | "MobSpawner"
                | "Beacon"
                | "Campfire"
        )
    )
}

/// Counts tickList entries in a saved PendingTicks root. Some query fixtures
/// expose individual coordinate-bearing ticks; neither representation is timing.
pub(crate) fn pending_tick_count(root: &NbtTag) -> u32 {
    let NbtTag::Compound(fields) = root else {
        return 0;
    };
    if let Some(NbtTag::List(ticks)) = fields.get("tickList") {
        u32::try_from(ticks.len()).unwrap_or(u32::MAX)
    } else if ["x", "y", "z"].iter().all(|key| fields.contains_key(*key)) {
        1
    } else {
        0
    }
}

#[derive(Default)]
struct Loads {
    chunks: BTreeMap<(i32, i32), ChunkLoad>,
    actors: BTreeSet<(i32, i64)>,
}

impl Loads {
    fn chunk(&mut self, x: i32, z: i32) -> &mut ChunkLoad {
        self.chunks.entry((x, z)).or_insert(ChunkLoad {
            chunk_x: x,
            chunk_z: z,
            ..ChunkLoad::default()
        })
    }

    fn entity(&mut self, position: [f64; 2], dimension: i32, id: Option<i64>) {
        if !position.iter().all(|value| value.is_finite()) {
            return;
        }
        if let Some(id) = id {
            if !self.actors.insert((dimension, id)) {
                return;
            }
        }
        let chunk = self.chunk(
            (position[0] / 16.0).floor() as i32,
            (position[1] / 16.0).floor() as i32,
        );
        chunk.entities = chunk.entities.saturating_add(1);
    }
}

pub(crate) fn from_snapshot(snapshot: &MapInfoOverlaySnapshot) -> Vec<ChunkLoad> {
    let mut loads = Loads::default();
    for entity in &snapshot.entities {
        loads.entity(
            [f64::from(entity.block_x), f64::from(entity.block_z)],
            entity.dimension_id,
            entity.unique_id,
        );
    }
    for count in &snapshot.ticking_block_entity_counts {
        let chunk = loads.chunk(count.chunk_x, count.chunk_z);
        chunk.ticking_block_entities = chunk.ticking_block_entities.saturating_add(count.count);
    }
    for count in &snapshot.pending_tick_counts {
        let chunk = loads.chunk(count.chunk_x, count.chunk_z);
        chunk.pending_ticks = chunk.pending_ticks.saturating_add(count.count);
    }
    loads.chunks.into_values().collect()
}

pub(crate) fn from_query(query: &RegionOverlayQuery) -> Vec<ChunkLoad> {
    let mut loads = Loads::default();
    for entity in &query.entities {
        loads.entity(
            [entity.position[0], entity.position[2]],
            entity.chunk.dimension.id(),
            entity.unique_id,
        );
    }
    for entity in &query.block_entities {
        if is_ticking_block_entity(entity.id.as_deref()) {
            let chunk = loads.chunk(
                entity.position[0].div_euclid(16),
                entity.position[2].div_euclid(16),
            );
            chunk.ticking_block_entities = chunk.ticking_block_entities.saturating_add(1);
        }
    }
    for tick in &query.pending_ticks {
        let count = pending_tick_count(&tick.tick);
        if count == 0 {
            continue;
        }
        let chunk = loads.chunk(tick.chunk.x, tick.chunk.z);
        chunk.pending_ticks = chunk.pending_ticks.saturating_add(count);
    }
    loads.chunks.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_ticks_counts_the_nested_queue_not_the_root_compound() {
        let tick = NbtTag::Compound(
            [
                ("x".to_owned(), NbtTag::Int(1)),
                ("y".to_owned(), NbtTag::Int(64)),
                ("z".to_owned(), NbtTag::Int(2)),
                ("time".to_owned(), NbtTag::Int(20)),
            ]
            .into_iter()
            .collect(),
        );
        let mut fields = [("tickList".to_owned(), NbtTag::List(vec![tick.clone(); 256]))]
            .into_iter()
            .collect();
        assert_eq!(pending_tick_count(&NbtTag::Compound(fields)), 256);
        fields = [
            ("currentTick".to_owned(), NbtTag::Int(100)),
            ("tickList".to_owned(), NbtTag::List(Vec::new())),
        ]
        .into_iter()
        .collect();
        assert_eq!(pending_tick_count(&NbtTag::Compound(fields)), 0);
        assert_eq!(pending_tick_count(&tick), 1);
        assert_eq!(pending_tick_count(&NbtTag::Int(20)), 0);
    }

    #[test]
    fn threshold_boundaries_and_strongest_signal() {
        assert_eq!(ChunkLoad::default().level(), None);
        assert_eq!(
            ChunkLoad {
                entities: 63,
                ..ChunkLoad::default()
            }
            .level(),
            None
        );
        assert_eq!(
            ChunkLoad {
                entities: 64,
                ..ChunkLoad::default()
            }
            .level(),
            Some(RiskLevel::Orange)
        );
        assert_eq!(
            ChunkLoad {
                ticking_block_entities: 32,
                ..ChunkLoad::default()
            }
            .level(),
            Some(RiskLevel::Orange)
        );
        assert_eq!(
            ChunkLoad {
                pending_ticks: 256,
                ..ChunkLoad::default()
            }
            .level(),
            Some(RiskLevel::Orange)
        );
        assert_eq!(
            ChunkLoad {
                entities: 64,
                pending_ticks: 1024,
                ..ChunkLoad::default()
            }
            .level(),
            Some(RiskLevel::Red)
        );
        assert!(!is_ticking_block_entity(Some("Chest")));
        assert!(!is_ticking_block_entity(Some("Sign")));
        assert!(is_ticking_block_entity(Some("Hopper")));
    }

    #[test]
    fn actors_use_actual_negative_coordinates_and_deduplicate_ids() {
        let mut loads = Loads::default();
        loads.entity([-0.5, -16.5], 0, Some(42));
        loads.entity([-0.5, -16.5], 0, Some(42));
        loads.entity([f64::NAN, 0.0], 0, None);
        let chunk = loads.chunks.get(&(-1, -2)).unwrap();
        assert_eq!(chunk.entities, 1);
        assert_eq!(loads.chunks.len(), 1);
    }
}
