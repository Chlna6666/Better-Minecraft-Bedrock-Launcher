use super::helpers::*;
use super::model::*;
use super::prelude::*;
use super::tile_cache::*;
use super::tile_render::*;
use super::viewport::*;

pub(super) fn load_tile_manifest_from_disk(
    world_path: PathBuf,
    render_backend: RenderBackend,
    render_gpu_backend: RenderGpuBackend,
    mode: RenderMode,
    dimension: Dimension,
    layout: RenderLayout,
    cancel: RenderTaskControl,
) -> Result<Option<TileManifestDiskResult>, String> {
    validate_ui_render_layout(layout)?;
    check_metadata_cancelled(&cancel)?;
    let path = tile_manifest_cache_path(
        &world_path,
        render_backend,
        render_gpu_backend,
        mode,
        dimension,
        layout,
    );
    let encoded = match std::fs::read(&path) {
        Ok(encoded) => encoded,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "读取地图索引缓存失败: {} ({error})",
                path.display()
            ));
        }
    };
    check_metadata_cancelled(&cancel)?;
    let json = zstd::stream::decode_all(encoded.as_slice())
        .map_err(|error| format!("解压地图索引缓存失败: {} ({error})", path.display()))?;
    check_metadata_cancelled(&cancel)?;
    let manifest = serde_json::from_slice::<TileManifestDisk>(&json)
        .map_err(|error| format!("解析地图索引缓存失败: {} ({error})", path.display()))?;
    let renderer_signature =
        render_preset_cache_signature(&world_path, render_backend, render_gpu_backend);
    let current_world_signature = world_cache_signature(&world_path);
    if manifest.world_signature != current_world_signature {
        remove_ui_decoded_tile_cache_scope(
            &world_path,
            render_backend,
            render_gpu_backend,
            mode,
            dimension,
            layout,
            "manifest_world_signature_mismatch",
        );
        return Ok(None);
    }
    if manifest.version != TILE_MANIFEST_VERSION
        || manifest.renderer_signature != renderer_signature
        || manifest.dimension_id != dimension.id()
        || manifest.mode != render_mode_cache_slug(mode)
        || manifest.chunks_per_tile != layout.chunks_per_tile
        || manifest.blocks_per_pixel != layout.blocks_per_pixel
        || manifest.pixels_per_block != layout.pixels_per_block
    {
        return Ok(None);
    }

    let mut tile_chunk_index = BTreeMap::<(i32, i32), Vec<ChunkPos>>::new();
    for entry in manifest.tiles {
        check_metadata_cancelled(&cancel)?;
        let coord = (entry.tile_x, entry.tile_z);
        let mut positions = entry
            .chunks
            .into_iter()
            .filter_map(|chunk| {
                (chunk.dimension_id == dimension.id()).then_some(ChunkPos {
                    x: chunk.x,
                    z: chunk.z,
                    dimension,
                })
            })
            .collect::<Vec<_>>();
        positions.sort();
        positions.dedup();
        tile_chunk_index.insert(coord, positions);
    }
    let positions = tile_chunk_index
        .values()
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    let bounds = chunk_bounds_from_positions(dimension, &positions);
    Ok(Some(TileManifestDiskResult {
        tile_chunk_index,
        bounds,
        center_block_x: manifest.center_block_x,
        center_block_z: manifest.center_block_z,
    }))
}

pub(super) fn save_tile_manifest_to_disk(
    world_path: &std::path::Path,
    render_backend: RenderBackend,
    render_gpu_backend: RenderGpuBackend,
    mode: RenderMode,
    dimension: Dimension,
    layout: RenderLayout,
    tile_chunk_index: &BTreeMap<(i32, i32), Vec<ChunkPos>>,
    center_block: Option<(i32, i32)>,
) -> Result<(), String> {
    validate_ui_render_layout(layout)?;
    let mut tiles = Vec::with_capacity(tile_chunk_index.len());
    for (&(tile_x, tile_z), positions) in tile_chunk_index {
        tiles.push(TileManifestEntryDisk {
            tile_x,
            tile_z,
            validation_value: 0,
            chunks: positions
                .iter()
                .map(|position| TileManifestChunkDisk {
                    x: position.x,
                    z: position.z,
                    dimension_id: position.dimension.id(),
                })
                .collect(),
        });
    }
    let manifest = TileManifestDisk {
        version: TILE_MANIFEST_VERSION,
        world_signature: world_cache_signature(world_path),
        renderer_signature: render_preset_cache_signature(
            world_path,
            render_backend,
            render_gpu_backend,
        ),
        dimension_id: dimension.id(),
        mode: render_mode_cache_slug(mode),
        chunks_per_tile: layout.chunks_per_tile,
        blocks_per_pixel: layout.blocks_per_pixel,
        pixels_per_block: layout.pixels_per_block,
        center_block_x: center_block.map(|center| center.0),
        center_block_z: center_block.map(|center| center.1),
        tiles,
    };
    let json = serde_json::to_vec(&manifest)
        .map_err(|error| format!("序列化地图索引缓存失败: {error}"))?;
    let encoded = zstd::stream::encode_all(json.as_slice(), 3)
        .map_err(|error| format!("压缩地图索引缓存失败: {error}"))?;
    let path = tile_manifest_cache_path(
        world_path,
        render_backend,
        render_gpu_backend,
        mode,
        dimension,
        layout,
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建地图索引缓存目录失败: {} ({error})", parent.display()))?;
    }
    let temp_path = path.with_extension("brmanifest.tmp");
    {
        let mut file = std::fs::File::create(&temp_path)
            .map_err(|error| format!("写入地图索引缓存失败: {} ({error})", temp_path.display()))?;
        file.write_all(&encoded)
            .map_err(|error| format!("写入地图索引缓存失败: {} ({error})", temp_path.display()))?;
        file.flush()
            .map_err(|error| format!("刷新地图索引缓存失败: {} ({error})", temp_path.display()))?;
    }
    match std::fs::rename(&temp_path, &path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            std::fs::remove_file(&path).map_err(|remove_error| {
                format!("替换地图索引缓存失败: {} ({remove_error})", path.display())
            })?;
            std::fs::rename(&temp_path, &path).map_err(|rename_error| {
                format!("替换地图索引缓存失败: {} ({rename_error})", path.display())
            })
        }
        Err(error) => Err(format!(
            "替换地图索引缓存失败: {} ({error})",
            path.display()
        )),
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
    let started = Instant::now();
    validate_ui_render_layout(layout)?;
    check_metadata_cancelled(&cancel)?;
    let worker_count = cpu_budget.thread_count().max(1);
    let result = render_session
        .probe_tile_manifest_blocking(
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
        mode = %render_mode_cache_slug(mode),
        "map_viewer manifest_probe_loaded"
    );
    Ok(TileManifestProbeResult {
        requested_tiles: result.requested_tiles,
        tile_chunk_index: result.tile_chunk_index,
        bounds: result.bounds,
    })
}
