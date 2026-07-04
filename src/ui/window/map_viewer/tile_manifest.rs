use super::helpers::{panic_payload_message, render_cpu_chunk_batch_size};
use super::model::*;
use super::prelude::*;
use super::tile_render::*;
use super::viewport::tile_bounds_from_coords;

pub(super) fn load_tile_manifest_from_disk(
    world_path: PathBuf,
    render_backend: RenderBackend,
    render_gpu_backend: RenderGpuBackend,
    mode: RenderMode,
    dimension: Dimension,
    layout: RenderLayout,
    cancel: RenderTaskControl,
) -> Result<Option<TileManifestProbeResult>, String> {
    validate_ui_render_layout(layout)?;
    check_metadata_cancelled(&cancel)?;
    let cache_root = file_ops::cache_subdir("bedrock-render");
    let key = bedrock_render::TileManifestCacheKey::new(
        &world_path,
        render_backend,
        render_gpu_backend,
        mode,
        dimension,
        layout,
    );
    let cache = bedrock_render::TileManifestCache::new(cache_root);
    let result = cache
        .load(&key)
        .map_err(|error| format!("读取地图索引缓存失败: {error}"))?;
    check_metadata_cancelled(&cancel)?;
    Ok(result.map(|snapshot| TileManifestProbeResult {
        requested_tiles: snapshot.requested_tiles,
        tile_chunk_index: shared_tile_chunk_index(snapshot.tile_chunk_index),
        bounds: snapshot.bounds,
        center_block_x: snapshot.center_block_x,
        center_block_z: snapshot.center_block_z,
    }))
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
    let started = Instant::now();
    validate_ui_render_layout(layout)?;
    check_metadata_cancelled(&cancel)?;
    let worker_count = cpu_budget.thread_count().max(1);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        render_session.probe_tile_manifest_blocking(
            TileManifestProbeRequest {
                dimension,
                layout,
                requested_tiles,
                queue_depth: worker_count,
                table_batch_size: render_cpu_chunk_batch_size(worker_count),
                progress_interval: 128,
            },
            &cancel,
        )
    }))
    .map_err(|payload| format!("探测地图瓦片索引崩溃: {}", panic_payload_message(payload)))?
    .map_err(|error| format!("探测地图瓦片索引失败: {error}"))?;
    check_metadata_cancelled(&cancel)?;
    let chunk_count = result
        .tile_chunk_index
        .values()
        .map(Vec::len)
        .sum::<usize>();
    let tile_bounds = tile_bounds_from_coords(&result.requested_tiles);
    tracing::debug!(
        requested = result.requested_tiles.len(),
        scanned = result.requested_tiles.len(),
        chunks = chunk_count,
        min = ?tile_bounds.map(|bounds| (bounds.min_x, bounds.min_z)),
        max = ?tile_bounds.map(|bounds| (bounds.max_x, bounds.max_z)),
        elapsed_ms = started.elapsed().as_millis(),
        backend = ?render_backend,
        gpu_backend = ?render_gpu_backend,
        mode = %bedrock_render::render_mode_cache_slug(mode),
        "map_viewer manifest_probe_loaded"
    );
    Ok(TileManifestProbeResult {
        requested_tiles: result.requested_tiles,
        tile_chunk_index: shared_tile_chunk_index(result.tile_chunk_index),
        bounds: result.bounds,
        center_block_x: None,
        center_block_z: None,
    })
}

fn shared_tile_chunk_index(index: BTreeMap<(i32, i32), Vec<ChunkPos>>) -> TileChunkIndex {
    index
        .into_iter()
        .map(|(coord, positions)| (coord, TileChunkPositions::from(positions)))
        .collect()
}
