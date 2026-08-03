pub(super) use super::tile_render_stable::*;

use super::prelude::*;
use bedrock_render::RenderLayout;

pub(super) const fn tile_texture_render_layout(
    world_layout: RenderLayout,
    _viewport_scale: f32,
) -> RenderLayout {
    RenderLayout {
        chunks_per_tile: world_layout.chunks_per_tile,
        blocks_per_pixel: 1,
        pixels_per_block: 1,
    }
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

    let patch_layout = RenderLayout {
        chunks_per_tile: 1,
        blocks_per_pixel: 1,
        pixels_per_block: 1,
    };
    let patch_size = patch_layout
        .tile_size()
        .ok_or_else(|| "局部 chunk 渲染布局尺寸无效".to_string())?;
    let mut render_options = interactive_render_options(
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
    render_options.region_layout = RegionLayout {
        chunks_per_region: 1,
    };
    render_options.gpu.pipeline_level = RenderGpuPipelineLevel::ComposeOnly;
    render_options.gpu.batch_pixels = usize::try_from(patch_size)
        .unwrap_or(16)
        .saturating_pow(2);

    let mut stats = RenderPipelineStats {
        planned_tiles: chunks.len(),
        ..RenderPipelineStats::default()
    };
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
        render_session
            .renderer()
            .render_tile_with_options_blocking(job, &render_options)
            .map_err(|error| {
                format!("局部 chunk {},{} 渲染失败: {error}", chunk.x, chunk.z)
            })?;
        stats.cpu_tiles = stats.cpu_tiles.saturating_add(1);
    }

    Ok(ChunkPatchRenderResult {
        coord: tile_coord,
        tile: base_tile,
        refreshed_chunks: chunks,
        diagnostics: RenderDiagnostics::default(),
        stats,
    })
}
