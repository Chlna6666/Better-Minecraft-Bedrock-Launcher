use super::helpers::*;
use super::model::*;
use super::prelude::*;
use super::tile_cache::*;
use super::tile_state::*;
use super::viewport::*;

pub(super) fn open_map_render_session(
    world_path: PathBuf,
    render_backend: RenderBackend,
    render_gpu_backend: RenderGpuBackend,
) -> Result<MapRenderSession, String> {
    tracing::debug!(
        backend = ?render_backend,
        gpu_backend = ?render_gpu_backend,
        world = %world_path.display(),
        "map_viewer open_render_session"
    );
    let config =
        interactive_map_render_session_config(&world_path, render_backend, render_gpu_backend);
    MapRenderSession::open_leveldb_read_only(
        world_path,
        config,
        RenderPalette::builtin_shared().as_ref().clone(),
    )
    .map_err(|error| format!("打开地图渲染会话失败: {error}"))
}

pub(super) fn interactive_map_render_session_config(
    world_path: &std::path::Path,
    render_backend: RenderBackend,
    render_gpu_backend: RenderGpuBackend,
) -> MapRenderSessionConfig {
    let mut config = MapRenderSessionConfig::max_speed(
        file_ops::cache_subdir("bedrock-render"),
        bedrock_render::world_cache_id(world_path),
        bedrock_render::render_preset_cache_signature(
            world_path,
            render_backend,
            render_gpu_backend,
        ),
    );
    config.tile_cache_memory_limit = tile_cache_memory_limit(RenderCpuBudget::default());
    config.renderer_version = RENDERER_CACHE_VERSION;
    config.palette_version = DEFAULT_PALETTE_VERSION;
    config.cull_missing_chunks = true;
    config.gpu_backend = render_gpu_backend;
    config
}

#[derive(Clone)]
pub(super) struct RenderTilePlan {
    pub(super) coord: (i32, i32),
    pub(super) planned: PlannedTile,
}

impl RenderTilePlan {
    pub(super) fn new(
        dimension: Dimension,
        mode: RenderMode,
        layout: RenderLayout,
        coord: (i32, i32),
        chunk_positions: TileChunkPositions,
    ) -> Result<Self, String> {
        if chunk_positions.is_empty() {
            return Err(format!("瓦片 {}, {} 没有可渲染区块", coord.0, coord.1));
        }
        let chunk_positions = ui_tile_chunk_positions_for_render(
            dimension,
            coord.0,
            coord.1,
            layout,
            Some(chunk_positions.as_ref()),
        )?
        .ok_or_else(|| format!("瓦片 {}, {} 尚未完成索引", coord.0, coord.1))?;
        if chunk_positions.is_empty() {
            return Err(format!("瓦片 {}, {} 没有可渲染区块", coord.0, coord.1));
        }
        let job = RenderJob::chunk_tile(
            TileCoord {
                x: coord.0,
                z: coord.1,
                dimension,
            },
            mode,
            layout,
        )
        .map_err(|error| format!("瓦片布局无效: {error}"))?;
        let region = tile_chunk_region(dimension, coord.0, coord.1, layout)?;
        Ok(Self {
            coord,
            planned: PlannedTile {
                job,
                region,
                layout,
                chunk_positions: Some(TileChunkPositions::from(chunk_positions)),
            },
        })
    }
}

pub(super) struct TileBatchRequest {
    pub(super) render_session: Arc<MapRenderSession>,
    pub(super) dimension: Dimension,
    pub(super) layout: RenderLayout,
    pub(super) center_tile: (i32, i32),
    pub(super) cache_policy: RenderCachePolicy,
    pub(super) plans: Vec<RenderTilePlan>,
    pub(super) cpu_budget: RenderCpuBudget,
    pub(super) render_backend: RenderBackend,
    pub(super) render_gpu_backend: RenderGpuBackend,
    pub(super) tile_cache_validation_seed: u64,
    pub(super) quick_reveal: bool,
    pub(super) render_cancel: RenderCancelFlag,
}

#[derive(Clone)]
pub(super) struct ChunkPatchRenderRequest {
    pub(super) render_session: Arc<MapRenderSession>,
    pub(super) mode: RenderMode,
    pub(super) layout: RenderLayout,
    pub(super) tile_coord: (i32, i32),
    pub(super) chunks: Vec<ChunkPos>,
    pub(super) base_tile: ViewerTile,
    pub(super) cpu_budget: RenderCpuBudget,
    pub(super) render_backend: RenderBackend,
    pub(super) render_gpu_backend: RenderGpuBackend,
    pub(super) render_cancel: RenderCancelFlag,
}

#[derive(Clone)]
pub(super) struct ChunkPatchRefreshPlan {
    pub(super) coord: (i32, i32),
    pub(super) chunks: Vec<ChunkPos>,
    pub(super) base_tile: ViewerTile,
}

#[derive(Clone)]
pub(super) struct ChunkPatchRenderResult {
    pub(super) coord: (i32, i32),
    pub(super) tile: ViewerTile,
    pub(super) refreshed_chunks: Vec<ChunkPos>,
    pub(super) diagnostics: RenderDiagnostics,
    pub(super) stats: RenderPipelineStats,
}

pub(super) fn render_tile_batch_stream(
    request: TileBatchRequest,
    event_sender: UnboundedSender<TileRenderEvent>,
) -> Result<(), String> {
    let TileBatchRequest {
        render_session,
        dimension,
        layout,
        center_tile,
        cache_policy,
        plans,
        cpu_budget,
        render_backend,
        render_gpu_backend,
        tile_cache_validation_seed,
        quick_reveal,
        render_cancel,
    } = request;
    validate_ui_render_layout(layout)?;
    let event_sender = Arc::new(Mutex::new(event_sender));
    let ready_batcher = Arc::new(Mutex::new(TileReadyBatcher::new(quick_reveal)));
    let requested_tiles = plans.iter().map(|plan| plan.coord).collect::<Vec<_>>();
    let requested_tile_count = requested_tiles.len();
    let stream_cancel = render_cancel.clone();
    let mut planned_tiles = Vec::with_capacity(plans.len());
    for plan in plans {
        if plan.planned.job.coord.dimension != dimension {
            return Err(format!(
                "瓦片 {}, {} 维度与请求不匹配",
                plan.coord.0, plan.coord.1
            ));
        }
        let Some(chunk_positions) = plan.planned.chunk_positions.as_deref() else {
            return Err(format!(
                "瓦片 {}, {} 尚未完成索引",
                plan.coord.0, plan.coord.1
            ));
        };
        if chunk_positions.is_empty() {
            return Err(format!(
                "瓦片 {}, {} 没有可渲染区块",
                plan.coord.0, plan.coord.1
            ));
        }
        tracing::trace!(
            tile = ?plan.coord,
            render_chunks = chunk_positions.len(),
            "map_viewer planned_tile"
        );
        planned_tiles.push(plan.planned);
    }

    let render_options = interactive_render_options(
        render_backend,
        render_gpu_backend,
        cpu_budget,
        RenderTilePriority::DistanceFrom {
            tile_x: center_tile.0,
            tile_z: center_tile.1,
        },
        render_cancel,
        cache_policy,
        tile_cache_validation_seed,
        planned_tiles.len(),
    );
    let output_options = RenderTileOutputOptions {
        pixel_format: TilePixelFormat::Rgba8,
    };

    let render_planned_tiles = planned_tiles.clone();

    let render_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        render_session.render_web_tiles_streaming_blocking_v2(
            &render_planned_tiles,
            render_options,
            output_options,
            {
                let event_sender = Arc::clone(&event_sender);
                let ready_batcher = Arc::clone(&ready_batcher);
                let requested_tiles = requested_tiles.clone();
                move |event| {
                    if stream_cancel.is_cancelled() {
                        return Err(bedrock_render::BedrockRenderError::Cancelled);
                    }
                    match event {
                        TileStreamEventV2::Ready {
                            planned,
                            tile,
                            source,
                        } => {
                            let coord = (planned.job.coord.x, planned.job.coord.z);
                            let tile_width = tile.width;
                            let tile_height = tile.height;
                            let tile_pixel_format = tile.pixel_format;
                            tracing::trace!(
                                tile = ?coord,
                                width = tile_width,
                                height = tile_height,
                                pixel_format = ?tile_pixel_format,
                                ?source,
                                "map_viewer tile_ready"
                            );
                            let (image, pixel_format, width, height, estimated_bytes) =
                                match render_image_from_decoded_tile_parts(
                                    tile_width,
                                    tile_height,
                                    tile_pixel_format,
                                    tile.pixels,
                                ) {
                                    Ok(rendered) => rendered,
                                    Err(error) => {
                                        send_tile_event_or_cancel(
                                            &event_sender,
                                            &stream_cancel,
                                            TileRenderEvent::Failed {
                                                coord,
                                                message: error.clone(),
                                            },
                                        )?;
                                        return Err(
                                            bedrock_render::BedrockRenderError::Validation(error),
                                        );
                                    }
                                };
                            if !ready_batcher
                                .lock()
                                .map_err(|_| render_io_error("渲染瓦片批处理状态锁已损坏"))?
                                .push(
                                    &event_sender,
                                    ReadyTile {
                                        coord,
                                        tile: ViewerTile {
                                            image,
                                            pixel_format: Some(pixel_format),
                                            width,
                                            height,
                                            estimated_bytes,
                                        },
                                        source,
                                    },
                                )
                            {
                                return cancel_render_stream(&stream_cancel);
                            }
                        }
                        TileStreamEventV2::Empty { planned } => {
                            let coord = (planned.job.coord.x, planned.job.coord.z);
                            tracing::trace!(tile = ?coord, "map_viewer tile_empty");
                            send_tile_event_or_cancel(
                                &event_sender,
                                &stream_cancel,
                                TileRenderEvent::Empty {
                                    coord,
                                    message: "tile has no renderable chunks".to_string(),
                                },
                            )?;
                        }
                        TileStreamEventV2::Failed { planned, error } => {
                            tracing::warn!(
                                tile = ?(planned.job.coord.x, planned.job.coord.z),
                                %error,
                                "map_viewer tile_stream_failed"
                            );
                            send_tile_event_or_cancel(
                                &event_sender,
                                &stream_cancel,
                                TileRenderEvent::Failed {
                                    coord: (planned.job.coord.x, planned.job.coord.z),
                                    message: error,
                                },
                            )?;
                        }
                        TileStreamEventV2::Progress(_) => {}
                        TileStreamEventV2::Complete {
                            diagnostics,
                            mut stats,
                        } => {
                            if !ready_batcher
                                .lock()
                                .map_err(|_| render_io_error("渲染瓦片批处理状态锁已损坏"))?
                                .flush(&event_sender)
                            {
                                return cancel_render_stream(&stream_cancel);
                            }
                            stats.planned_tiles = requested_tile_count;
                            send_tile_event_or_cancel(
                                &event_sender,
                                &stream_cancel,
                                TileRenderEvent::Complete {
                                    requested_tiles: requested_tiles.clone(),
                                    diagnostics,
                                    stats,
                                },
                            )?;
                        }
                    }
                    Ok(())
                }
            },
        )
    }))
    .map_err(|payload| format!("渲染瓦片任务崩溃: {}", panic_payload_message(payload)))?
    .map_err(|error| format!("渲染瓦片失败: {error}"));
    render_result?;
    Ok(())
}

pub(super) fn render_chunk_patches_blocking(
    request: ChunkPatchRenderRequest,
) -> Result<ChunkPatchRenderResult, String> {
    let ChunkPatchRenderRequest {
        render_session,
        mode,
        layout,
        tile_coord,
        chunks,
        base_tile,
        cpu_budget,
        render_backend,
        render_gpu_backend,
        render_cancel,
    } = request;
    validate_ui_render_layout(layout)?;
    if chunks.is_empty() {
        return Err("没有需要局部刷新的 chunk".to_string());
    }
    let tile_size = layout
        .tile_size()
        .ok_or_else(|| "UI 地图瓦片布局尺寸无效".to_string())?;
    if base_tile.width != tile_size || base_tile.height != tile_size {
        return Err(format!(
            "旧瓦片格式不支持局部合并: {:?} {}x{}",
            base_tile.pixel_format, base_tile.width, base_tile.height
        ));
    }
    let (base_pixels, base_pixel_format) = render_image_pixels(
        base_tile.image.as_ref(),
        base_tile.pixel_format,
        base_tile.width,
        base_tile.height,
    )?;
    if base_pixel_format != TilePixelFormat::Rgba8 {
        return Err(format!(
            "旧瓦片格式不支持局部合并: {:?} {}x{}",
            base_tile.pixel_format, base_tile.width, base_tile.height
        ));
    }

    let mut merged_pixels = Vec::from(base_pixels);
    let mut diagnostics = RenderDiagnostics::default();
    let mut stats = RenderPipelineStats {
        planned_tiles: chunks.len(),
        ..RenderPipelineStats::default()
    };
    let patch_layout = RenderLayout {
        chunks_per_tile: 1,
        blocks_per_pixel: layout.blocks_per_pixel,
        pixels_per_block: layout.pixels_per_block,
    };
    let patch_size = patch_layout
        .tile_size()
        .ok_or_else(|| "局部 chunk 渲染布局尺寸无效".to_string())?;
    let render_options = interactive_render_options(
        render_backend,
        render_gpu_backend,
        cpu_budget,
        RenderTilePriority::DistanceFrom {
            tile_x: tile_coord.0,
            tile_z: tile_coord.1,
        },
        render_cancel.clone(),
        RenderCachePolicy::Refresh,
        0,
        chunks.len(),
    );
    let patch_region_layout = RegionLayout {
        chunks_per_region: 1,
    };
    let mut render_options = render_options;
    render_options.region_layout = patch_region_layout;
    render_options.gpu.pipeline_level = RenderGpuPipelineLevel::ComposeOnly;
    render_options.gpu.batch_pixels = usize::try_from(patch_size).unwrap_or(64).saturating_pow(2);

    for chunk in chunks.iter().copied() {
        if render_cancel.is_cancelled() {
            return Err("局部 chunk 刷新已取消".to_string());
        }
        let job = RenderJob::chunk_tile(
            TileCoord {
                x: chunk.x,
                z: chunk.z,
                dimension: chunk.dimension,
            },
            mode,
            patch_layout,
        )
        .map_err(|error| format!("局部 chunk 渲染布局无效: {error}"))?;
        let patch = render_session
            .renderer()
            .render_tile_with_options_blocking(job, &render_options)
            .map_err(|error| format!("局部 chunk {},{} 渲染失败: {error}", chunk.x, chunk.z))?;
        stats.cpu_tiles = stats.cpu_tiles.saturating_add(1);
        let patch = DecodedTileImage {
            coord: patch.coord,
            width: patch.width,
            height: patch.height,
            pixels: patch.rgba,
            pixel_format: TilePixelFormat::Rgba8,
        };
        merge_chunk_patch_into_tile_pixels(&mut merged_pixels, tile_size, layout, chunk, patch)?;
    }

    let (image, pixel_format, width, height, estimated_bytes) =
        render_image_from_decoded_tile_parts(
            tile_size,
            tile_size,
            TilePixelFormat::Rgba8,
            Arc::from(merged_pixels),
        )?;
    Ok(ChunkPatchRenderResult {
        coord: tile_coord,
        tile: ViewerTile {
            image,
            pixel_format: Some(pixel_format),
            width,
            height,
            estimated_bytes,
        },
        refreshed_chunks: chunks,
        diagnostics,
        stats,
    })
}

pub(super) fn merge_chunk_patch_into_tile_pixels(
    tile_pixels: &mut [u8],
    tile_size: u32,
    layout: RenderLayout,
    chunk: ChunkPos,
    patch: DecodedTileImage,
) -> Result<(), String> {
    if patch.pixel_format != TilePixelFormat::Rgba8 {
        return Err(format!(
            "局部 chunk 像素格式不支持: {:?}",
            patch.pixel_format
        ));
    }
    let patch_layout = RenderLayout {
        chunks_per_tile: 1,
        blocks_per_pixel: layout.blocks_per_pixel,
        pixels_per_block: layout.pixels_per_block,
    };
    let patch_size = patch_layout
        .tile_size()
        .ok_or_else(|| "局部 chunk 渲染布局尺寸无效".to_string())?;
    if patch.width != patch_size || patch.height != patch_size {
        return Err(format!(
            "局部 chunk 尺寸不匹配: expected {patch_size}x{patch_size}, got {}x{}",
            patch.width, patch.height
        ));
    }
    let expected_patch_len = decoded_tile_byte_len(patch.width, patch.height)?;
    let patch_pixels = patch.pixels.as_ref();
    if patch_pixels.len() != expected_patch_len {
        return Err(format!(
            "局部 chunk 像素长度不匹配: expected {expected_patch_len}, got {}",
            patch_pixels.len()
        ));
    }
    let expected_tile_len = decoded_tile_byte_len(tile_size, tile_size)?;
    if tile_pixels.len() != expected_tile_len {
        return Err(format!(
            "目标瓦片像素长度不匹配: expected {expected_tile_len}, got {}",
            tile_pixels.len()
        ));
    }
    let chunks_per_tile = i32::try_from(layout.chunks_per_tile)
        .unwrap_or(CHUNKS_PER_TILE as i32)
        .max(1);
    let local_chunk_x = chunk.x.rem_euclid(chunks_per_tile);
    let local_chunk_z = chunk.z.rem_euclid(chunks_per_tile);
    let dest_x = u32::try_from(local_chunk_x)
        .map_err(|_| "局部 chunk X 超出瓦片范围".to_string())?
        .checked_mul(patch_size)
        .ok_or_else(|| "局部 chunk X 偏移溢出".to_string())?;
    let dest_z = u32::try_from(local_chunk_z)
        .map_err(|_| "局部 chunk Z 超出瓦片范围".to_string())?
        .checked_mul(patch_size)
        .ok_or_else(|| "局部 chunk Z 偏移溢出".to_string())?;
    if dest_x.saturating_add(patch_size) > tile_size
        || dest_z.saturating_add(patch_size) > tile_size
    {
        return Err(format!(
            "局部 chunk 偏移超出瓦片: dest=({dest_x},{dest_z}) patch={patch_size} tile={tile_size}"
        ));
    }
    let tile_stride = usize::try_from(tile_size)
        .ok()
        .and_then(|value| value.checked_mul(4))
        .ok_or_else(|| "目标瓦片 stride 溢出".to_string())?;
    let patch_stride = usize::try_from(patch_size)
        .ok()
        .and_then(|value| value.checked_mul(4))
        .ok_or_else(|| "局部 chunk stride 溢出".to_string())?;
    let dest_x_bytes = usize::try_from(dest_x)
        .ok()
        .and_then(|value| value.checked_mul(4))
        .ok_or_else(|| "局部 chunk X 字节偏移溢出".to_string())?;
    let dest_z =
        usize::try_from(dest_z).map_err(|_| "局部 chunk Z 偏移不适合 usize".to_string())?;
    let patch_rows =
        usize::try_from(patch_size).map_err(|_| "局部 chunk 行数不适合 usize".to_string())?;
    for row in 0..patch_rows {
        let source_start = row
            .checked_mul(patch_stride)
            .ok_or_else(|| "局部 chunk 源行偏移溢出".to_string())?;
        let source_end = source_start
            .checked_add(patch_stride)
            .ok_or_else(|| "局部 chunk 源行末尾溢出".to_string())?;
        let dest_start = dest_z
            .checked_add(row)
            .and_then(|row| row.checked_mul(tile_stride))
            .and_then(|offset| offset.checked_add(dest_x_bytes))
            .ok_or_else(|| "目标瓦片行偏移溢出".to_string())?;
        let dest_end = dest_start
            .checked_add(patch_stride)
            .ok_or_else(|| "目标瓦片行末尾溢出".to_string())?;
        tile_pixels
            .get_mut(dest_start..dest_end)
            .ok_or_else(|| "目标瓦片局部区域越界".to_string())?
            .copy_from_slice(
                patch_pixels
                    .get(source_start..source_end)
                    .ok_or_else(|| "局部 chunk 源区域越界".to_string())?,
            );
    }
    Ok(())
}

pub(super) fn chunks_for_tile(
    chunks: &BTreeSet<ChunkPos>,
    tile_coord: (i32, i32),
    layout: RenderLayout,
) -> Vec<ChunkPos> {
    let chunks_per_tile = i32::try_from(layout.chunks_per_tile)
        .unwrap_or(CHUNKS_PER_TILE as i32)
        .max(1);
    chunks
        .iter()
        .copied()
        .filter(|chunk| {
            (
                chunk.x.div_euclid(chunks_per_tile),
                chunk.z.div_euclid(chunks_per_tile),
            ) == tile_coord
        })
        .collect()
}

pub(super) fn send_tile_event(
    sender: &Arc<Mutex<UnboundedSender<TileRenderEvent>>>,
    event: TileRenderEvent,
) -> bool {
    let Ok(sender) = sender.lock() else {
        tracing::warn!("map tile event sender lock was poisoned");
        return false;
    };
    if sender.unbounded_send(event).is_err() {
        tracing::debug!("map tile event receiver was dropped");
        return false;
    }
    true
}

pub(super) fn send_tile_event_or_cancel(
    sender: &Arc<Mutex<UnboundedSender<TileRenderEvent>>>,
    cancel: &RenderCancelFlag,
    event: TileRenderEvent,
) -> Result<(), bedrock_render::BedrockRenderError> {
    if send_tile_event(sender, event) {
        Ok(())
    } else {
        cancel_render_stream(cancel)
    }
}

pub(super) fn cancel_render_stream(
    cancel: &RenderCancelFlag,
) -> Result<(), bedrock_render::BedrockRenderError> {
    cancel.cancel();
    Err(bedrock_render::BedrockRenderError::Cancelled)
}

pub(super) fn cancel_metadata_flag(cancel: &mut Option<RenderTaskControl>) -> bool {
    let Some(cancel) = cancel.take() else {
        return false;
    };
    cancel.cancel();
    true
}

pub(super) fn check_metadata_cancelled(cancel: &RenderTaskControl) -> Result<(), String> {
    if cancel.is_cancelled() {
        return Err("地图索引任务已取消".to_string());
    }
    Ok(())
}

pub(super) fn chunk_bounds_from_positions(
    dimension: Dimension,
    positions: &[ChunkPos],
) -> Option<ChunkBounds> {
    let first = positions.first()?;
    let mut bounds = ChunkBounds {
        dimension,
        min_chunk_x: first.x,
        min_chunk_z: first.z,
        max_chunk_x: first.x,
        max_chunk_z: first.z,
        chunk_count: 0,
    };
    for position in positions {
        bounds.min_chunk_x = bounds.min_chunk_x.min(position.x);
        bounds.min_chunk_z = bounds.min_chunk_z.min(position.z);
        bounds.max_chunk_x = bounds.max_chunk_x.max(position.x);
        bounds.max_chunk_z = bounds.max_chunk_z.max(position.z);
        bounds.chunk_count = bounds.chunk_count.saturating_add(1);
    }
    Some(bounds)
}

pub(super) fn merge_chunk_bounds(
    existing: Option<ChunkBounds>,
    incoming: Option<ChunkBounds>,
) -> Option<ChunkBounds> {
    match (existing, incoming) {
        (None, None) => None,
        (Some(bounds), None) | (None, Some(bounds)) => Some(bounds),
        (Some(mut existing), Some(incoming)) => {
            existing.min_chunk_x = existing.min_chunk_x.min(incoming.min_chunk_x);
            existing.min_chunk_z = existing.min_chunk_z.min(incoming.min_chunk_z);
            existing.max_chunk_x = existing.max_chunk_x.max(incoming.max_chunk_x);
            existing.max_chunk_z = existing.max_chunk_z.max(incoming.max_chunk_z);
            existing.chunk_count = existing.chunk_count.saturating_add(incoming.chunk_count);
            Some(existing)
        }
    }
}

pub(super) fn web_relief_render_layout() -> RenderLayout {
    RenderLayout {
        chunks_per_tile: CHUNKS_PER_TILE,
        blocks_per_pixel: UI_BLOCKS_PER_PIXEL,
        pixels_per_block: UI_PIXELS_PER_BLOCK,
    }
}

pub(super) fn validate_ui_render_layout(layout: RenderLayout) -> Result<(), String> {
    let expected_size = DEFAULT_TILE_SIZE as u32;
    let actual_size = layout
        .tile_size()
        .ok_or_else(|| "UI 地图瓦片布局尺寸无效".to_string())?;
    if layout.chunks_per_tile != CHUNKS_PER_TILE
        || layout.blocks_per_pixel != UI_BLOCKS_PER_PIXEL
        || layout.pixels_per_block != UI_PIXELS_PER_BLOCK
        || actual_size != expected_size
    {
        return Err(format!(
            "UI 地图只支持 8x8/512px/4ppb tile，当前 chunks_per_tile={} blocks_per_pixel={} pixels_per_block={} tile_size={actual_size}",
            layout.chunks_per_tile, layout.blocks_per_pixel, layout.pixels_per_block
        ));
    }
    Ok(())
}

pub(super) fn web_relief_region_layout() -> RegionLayout {
    RegionLayout {
        chunks_per_region: CHUNKS_PER_REGION,
    }
}

pub(super) fn interactive_tile_batch_size(
    render_backend: RenderBackend,
    cpu_budget: RenderCpuBudget,
) -> usize {
    resolve_interactive_tile_batch_size(render_backend, cpu_budget, map_render_batch_tiles())
}

pub(super) fn resolve_interactive_tile_batch_size(
    _render_backend: RenderBackend,
    cpu_budget: RenderCpuBudget,
    ui_batch_tiles: usize,
) -> usize {
    cpu_budget
        .tile_batch_size()
        .min(ui_batch_tiles.max(1))
        .max(1)
}

pub(super) fn selected_tile_chunk_count(
    selected_tiles: &[(i32, i32)],
    layout: RenderLayout,
    tile_chunk_index: &TileChunkIndex,
) -> usize {
    let mut estimated_chunks = 0usize;
    for coord in selected_tiles {
        match tile_chunk_index.get(coord) {
            Some(positions) if positions.is_empty() => {}
            Some(positions) => estimated_chunks = estimated_chunks.saturating_add(positions.len()),
            None => {}
        }
    }
    estimated_chunks
}

pub(super) fn selected_tile_region_count(
    selected_tiles: &[(i32, i32)],
    _layout: RenderLayout,
    tile_chunk_index: &TileChunkIndex,
) -> usize {
    let chunks_per_region = i32::try_from(CHUNKS_PER_REGION).unwrap_or(32).max(1);
    let mut regions = BTreeSet::new();
    for coord in selected_tiles {
        match tile_chunk_index.get(coord) {
            Some(positions) if positions.is_empty() => {}
            Some(positions) => {
                for position in positions.iter() {
                    regions.insert((
                        position.x.div_euclid(chunks_per_region),
                        position.z.div_euclid(chunks_per_region),
                    ));
                }
            }
            None => {}
        }
    }
    regions.len()
}

pub(super) fn interactive_render_options(
    render_backend: RenderBackend,
    render_gpu_backend: RenderGpuBackend,
    cpu_budget: RenderCpuBudget,
    priority: RenderTilePriority,
    render_cancel: RenderCancelFlag,
    cache_policy: RenderCachePolicy,
    tile_cache_validation_seed: u64,
    work_items: usize,
) -> RenderOptions {
    let mut options = RenderOptions::max_speed_interactive();
    options.format = ImageFormat::Rgba;
    options.backend = render_backend;
    let tile_pixels = DEFAULT_TILE_SIZE as usize * DEFAULT_TILE_SIZE as usize;
    let gpu_in_flight = cpu_budget.thread_count().clamp(2, 6);
    options.gpu = RenderGpuOptions {
        backend: render_gpu_backend,
        fallback_policy: RenderGpuFallbackPolicy::AllowCpu,
        pipeline_level: RenderGpuPipelineLevel::ComposeOnly,
        max_in_flight: gpu_in_flight,
        batch_pixels: tile_pixels,
        staging_pool_bytes: render_staging_pool_bytes(work_items, cpu_budget),
        diagnostics: true,
    };
    options.cpu = cpu_budget.render_cpu_pipeline(work_items.max(1));
    options.cpu_workers = cpu_budget.render_threading(work_items.max(1));
    options.priority = priority;
    options.threading = cpu_budget.render_threading(work_items.max(1));
    options.execution_profile = RenderExecutionProfile::Interactive;
    options.memory_budget =
        RenderMemoryBudget::FixedBytes(render_memory_budget_bytes(work_items, cpu_budget));
    options.pipeline_depth = RENDER_PIPELINE_DEPTH;
    options.cancel = Some(render_cancel);
    options.cache_policy = cache_policy;
    options.tile_cache_validation_seed = tile_cache_validation_seed;
    options.surface = web_relief_surface_options();
    options.region_layout = web_relief_region_layout();
    options
}

pub(super) fn map_render_batch_tiles() -> usize {
    map_env_usize("BMCBL_MAP_RENDER_BATCH_TILES", RENDER_UI_BATCH_TILES).max(1)
}

pub(super) fn map_viewer_prefetch_radius() -> i32 {
    i32::try_from(map_env_usize(
        "BMCBL_MAP_PREFETCH_RADIUS",
        usize::try_from(PREFETCH_RADIUS.max(0)).unwrap_or(1),
    ))
    .unwrap_or(PREFETCH_RADIUS)
    .clamp(0, RETAIN_RADIUS.max(0))
}

pub(super) fn map_env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| parse_size_value(&value))
        .unwrap_or(default)
}

pub(super) fn parse_size_value(value: &str) -> Option<usize> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let split_at = trimmed
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(trimmed.len());
    let (number, suffix) = trimmed.split_at(split_at);
    let base = number.parse::<usize>().ok()?;
    let multiplier = match suffix.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "k" | "kb" | "kib" => 1024,
        "m" | "mb" | "mib" => 1024 * 1024,
        "g" | "gb" | "gib" => 1024 * 1024 * 1024,
        _ => return None,
    };
    base.checked_mul(multiplier)
}

pub(super) fn web_relief_surface_options() -> SurfaceRenderOptions {
    SurfaceRenderOptions {
        lighting: TerrainLightingOptions {
            enabled: true,
            light_azimuth_degrees: 315.0,
            light_elevation_degrees: 40.0,
            normal_strength: 2.35,
            shadow_strength: 0.66,
            highlight_strength: 0.42,
            ambient_occlusion: 0.075,
            max_shadow: 50.0,
            land_slope_softness: 7.0,
            edge_relief_strength: 0.38,
            edge_relief_threshold: 2.0,
            edge_relief_max_shadow: 24.0,
            edge_relief_highlight: 0.14,
            underwater_relief_enabled: true,
            underwater_relief_strength: 1.35,
            underwater_depth_fade: 12.0,
            underwater_min_light: 0.25,
        },
        block_boundaries: BlockBoundaryRenderOptions {
            enabled: false,
            strength: 0.0,
            flat_strength: 0.0,
            height_threshold: 1.0,
            max_shadow: 0.0,
            highlight_strength: 0.0,
            softness: 4.0,
            line_width_pixels: 1.0,
        },
        block_volume: BlockVolumeRenderOptions {
            enabled: false,
            face_width_pixels: 1.35,
            face_shadow_strength: 0.72,
            contact_shadow_strength: 0.66,
            cast_shadow_strength: 0.55,
            cast_shadow_max_blocks: 5,
            cast_shadow_height_scale: 0.65,
            highlight_strength: 0.22,
            max_shadow: 30.0,
            max_highlight: 16.0,
            height_threshold: 1.0,
            softness: 5.0,
        },
        atlas: AtlasRenderOptions {
            enabled: false,
            texture_detail_strength: 0.0,
            height_contour_interval: 4,
            height_contour_strength: 0.0,
            slope_hatching_strength: 0.0,
            forest_canopy_strength: 0.0,
            snow_ridge_strength: 0.0,
            water_grid_strength: 0.0,
            shoreline_shadow_strength: 0.0,
            chunk_grid_strength: 0.0,
            material_edge_strength: 0.0,
            cast_shadow_strength: 0.0,
            ambient_occlusion_strength: 0.0,
        },
        ..SurfaceRenderOptions::default()
    }
}

pub(super) fn tile_chunk_region(
    dimension: Dimension,
    tile_x: i32,
    tile_z: i32,
    layout: RenderLayout,
) -> Result<ChunkRegion, String> {
    let chunks_per_tile = i32::try_from(layout.chunks_per_tile)
        .map_err(|_| "瓦片布局 chunks_per_tile 超出范围".to_string())?
        .max(1);
    let min_chunk_x = tile_x
        .checked_mul(chunks_per_tile)
        .ok_or_else(|| "瓦片 X 范围溢出".to_string())?;
    let min_chunk_z = tile_z
        .checked_mul(chunks_per_tile)
        .ok_or_else(|| "瓦片 Z 范围溢出".to_string())?;
    let max_chunk_x = min_chunk_x
        .checked_add(chunks_per_tile.saturating_sub(1))
        .ok_or_else(|| "瓦片 X 范围溢出".to_string())?;
    let max_chunk_z = min_chunk_z
        .checked_add(chunks_per_tile.saturating_sub(1))
        .ok_or_else(|| "瓦片 Z 范围溢出".to_string())?;
    Ok(ChunkRegion::new(
        dimension,
        min_chunk_x,
        min_chunk_z,
        max_chunk_x,
        max_chunk_z,
    ))
}

pub(super) fn tile_bounds_contains(bounds: TileBounds, coord: (i32, i32)) -> bool {
    coord.0 >= bounds.min_x
        && coord.0 <= bounds.max_x
        && coord.1 >= bounds.min_z
        && coord.1 <= bounds.max_z
}

pub(super) fn ui_tile_chunk_positions_for_render(
    dimension: Dimension,
    tile_x: i32,
    tile_z: i32,
    layout: RenderLayout,
    indexed_positions: Option<&[ChunkPos]>,
) -> Result<Option<Vec<ChunkPos>>, String> {
    match indexed_positions {
        Some([]) => Ok(Some(Vec::new())),
        Some(positions) => {
            let region = tile_chunk_region(dimension, tile_x, tile_z, layout)?;
            let mut positions = positions
                .iter()
                .copied()
                .filter(|position| {
                    position.dimension == dimension
                        && position.x >= region.min_chunk_x
                        && position.x <= region.max_chunk_x
                        && position.z >= region.min_chunk_z
                        && position.z <= region.max_chunk_z
                })
                .collect::<Vec<_>>();
            positions.sort();
            positions.dedup();
            Ok(Some(positions))
        }
        None => Ok(None),
    }
}
