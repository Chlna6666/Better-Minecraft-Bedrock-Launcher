pub(super) use super::tile_manifest_legacy::*;

use super::model::*;
use super::prelude::*;
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

const PROBE_GROUP_TILE_SPAN: i32 = 8;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ProbeGroupKey {
    session: usize,
    dimension: i32,
    mode: String,
    chunks_per_tile: u32,
    blocks_per_pixel: u32,
    pixels_per_block: u32,
    group_x: i32,
    group_z: i32,
}

#[derive(Clone)]
struct CachedProbeGroup {
    tile_chunk_index: TileChunkIndex,
    bounds: Option<ChunkBounds>,
}

type SharedProbeResult = Result<CachedProbeGroup, String>;

enum ProbeGroupState {
    Loading,
    Ready(SharedProbeResult),
}

struct ProbeGroupSlot {
    state: Mutex<ProbeGroupState>,
    ready: Condvar,
}

static PROBE_GROUPS: OnceLock<Mutex<HashMap<ProbeGroupKey, Arc<ProbeGroupSlot>>>> = OnceLock::new();

fn probe_groups() -> &'static Mutex<HashMap<ProbeGroupKey, Arc<ProbeGroupSlot>>> {
    PROBE_GROUPS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn aligned_probe_group(coord: (i32, i32)) -> (i32, i32) {
    (
        coord.0.div_euclid(PROBE_GROUP_TILE_SPAN),
        coord.1.div_euclid(PROBE_GROUP_TILE_SPAN),
    )
}

fn probe_group_tiles(group_x: i32, group_z: i32) -> Vec<(i32, i32)> {
    let start_x = group_x.saturating_mul(PROBE_GROUP_TILE_SPAN);
    let start_z = group_z.saturating_mul(PROBE_GROUP_TILE_SPAN);
    let mut tiles = Vec::with_capacity((PROBE_GROUP_TILE_SPAN * PROBE_GROUP_TILE_SPAN) as usize);
    for z in 0..PROBE_GROUP_TILE_SPAN {
        for x in 0..PROBE_GROUP_TILE_SPAN {
            tiles.push((start_x.saturating_add(x), start_z.saturating_add(z)));
        }
    }
    tiles
}

fn wait_for_probe_group(
    slot: &ProbeGroupSlot,
    cancel: &RenderTaskControl,
) -> Result<CachedProbeGroup, String> {
    let mut state = slot
        .state
        .lock()
        .map_err(|_| "地图区域探测缓存锁已损坏".to_string())?;
    loop {
        check_metadata_cancelled(cancel)?;
        match &*state {
            ProbeGroupState::Ready(result) => return result.clone(),
            ProbeGroupState::Loading => {
                let (next, _) = slot
                    .ready
                    .wait_timeout(state, Duration::from_millis(25))
                    .map_err(|_| "地图区域探测等待锁已损坏".to_string())?;
                state = next;
            }
        }
    }
}

pub(super) fn load_tile_manifest_probe(
    render_session: Arc<MapRenderSession>,
    render_backend: RenderBackend,
    render_gpu_backend: RenderGpuBackend,
    mode: RenderMode,
    dimension: Dimension,
    layout: RenderLayout,
    requested_tiles: Vec<(i32, i32)>,
    cpu_budget: RenderCpuBudget,
    cancel: RenderTaskControl,
) -> Result<TileManifestProbeResult, String> {
    if requested_tiles.is_empty() {
        return Ok(TileManifestProbeResult {
            requested_tiles,
            tile_chunk_index: TileChunkIndex::new(),
            bounds: None,
            center_block_x: None,
            center_block_z: None,
        });
    }

    let mut grouped = HashMap::<(i32, i32), Vec<(i32, i32)>>::new();
    for coord in &requested_tiles {
        grouped
            .entry(aligned_probe_group(*coord))
            .or_default()
            .push(*coord);
    }

    let session = Arc::as_ptr(&render_session) as usize;
    let mut tile_chunk_index = TileChunkIndex::new();
    let mut bounds = None;

    for ((group_x, group_z), _) in grouped {
        check_metadata_cancelled(&cancel)?;
        let key = ProbeGroupKey {
            session,
            dimension: dimension.id(),
            mode: bedrock_render::render_mode_cache_slug(mode).to_string(),
            chunks_per_tile: layout.chunks_per_tile,
            blocks_per_pixel: layout.blocks_per_pixel,
            pixels_per_block: layout.pixels_per_block,
            group_x,
            group_z,
        };

        let (slot, leader) = {
            let mut groups = probe_groups()
                .lock()
                .map_err(|_| "地图区域探测表锁已损坏".to_string())?;
            if let Some(slot) = groups.get(&key) {
                (Arc::clone(slot), false)
            } else {
                let slot = Arc::new(ProbeGroupSlot {
                    state: Mutex::new(ProbeGroupState::Loading),
                    ready: Condvar::new(),
                });
                groups.insert(key.clone(), Arc::clone(&slot));
                (slot, true)
            }
        };

        let cached = if leader {
            let group_tiles = probe_group_tiles(group_x, group_z);
            let result = super::tile_manifest_legacy::load_tile_manifest_probe(
                Arc::clone(&render_session),
                render_backend,
                render_gpu_backend,
                mode,
                dimension,
                layout,
                group_tiles,
                cpu_budget,
                cancel.clone(),
            )
            .map(|result| CachedProbeGroup {
                tile_chunk_index: result.tile_chunk_index,
                bounds: result.bounds,
            });
            {
                let mut state = slot
                    .state
                    .lock()
                    .map_err(|_| "地图区域探测结果锁已损坏".to_string())?;
                *state = ProbeGroupState::Ready(result.clone());
                slot.ready.notify_all();
            }
            if result.is_err() {
                if let Ok(mut groups) = probe_groups().lock() {
                    groups.remove(&key);
                }
            }
            result?
        } else {
            wait_for_probe_group(&slot, &cancel)?
        };

        tile_chunk_index.extend(cached.tile_chunk_index);
        bounds = merge_chunk_bounds(bounds, cached.bounds);
    }

    Ok(TileManifestProbeResult {
        requested_tiles,
        tile_chunk_index,
        bounds,
        center_block_x: None,
        center_block_z: None,
    })
}
