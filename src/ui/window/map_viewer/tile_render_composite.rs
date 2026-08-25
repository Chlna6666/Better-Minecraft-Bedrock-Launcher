pub(super) use super::tile_render_core::*;
use crate::ui::window::map_viewer::model::MapViewport;

use super::model::{CHUNKS_PER_TILE, MAX_VIEWPORT_COMPOSITE_DIMENSION};
use super::prelude::*;
use super::tile_cache::decoded_tile_byte_len;
use super::tile_render_core as core;
use super::tile_state::MapRenderRange;
use super::viewport::{region_render_range_for_viewport, tile_paint_rect};
use bedrock_render::RenderLayout;
use std::sync::atomic::{AtomicUsize, Ordering};

const PROGRESSIVE_PREVIEW_TILE_GROUP: usize = 8;
const MAX_PROGRESSIVE_PREVIEW_FRAMES: usize = 20;
const PROGRESSIVE_PREVIEW_PRESENT_INTERVAL: Duration = Duration::from_millis(12);

pub(super) const fn web_relief_render_layout() -> RenderLayout {
    RenderLayout {
        chunks_per_tile: CHUNKS_PER_TILE,
        blocks_per_pixel: 1,
        pixels_per_block: 1,
    }
}

pub(super) const fn tile_texture_render_layout(
    _scale: f32,
    _current: RenderLayout,
) -> RenderLayout {
    RenderLayout {
        chunks_per_tile: CHUNKS_PER_TILE,
        blocks_per_pixel: 1,
        pixels_per_block: 1,
    }
}

pub(super) fn render_viewport_composite_stream(
    request: core::ViewportCompositeRequest,
    event_sender: UnboundedSender<core::ViewportCompositeEvent>,
) -> Result<(), String> {
    let core::ViewportCompositeRequest {
        render_session,
        dimension,
        layout,
        viewport,
        center_tile,
        cache_policy,
        plans,
        cpu_budget,
        render_backend,
        render_gpu_backend,
        tile_cache_validation_seed,
        render_cancel,
    } = request;
    core::validate_ui_render_layout(layout)?;
    let render_range = region_render_range_for_viewport(viewport, layout)
        .ok_or_else(|| "视口合成范围无效".to_string())?;
    let (image_width, image_height, output_scale) = viewport_composite_image_size(viewport)?;
    let compositor = Arc::new(Mutex::new(ProgressiveViewportCompositor::new(
        viewport,
        layout,
        render_range,
        image_width,
        image_height,
        output_scale,
    )?));

    let requested_tiles = plans.iter().map(|plan| plan.coord).collect::<Vec<_>>();
    let requested_tile_count = requested_tiles.len();
    let mut planned_tiles = Vec::with_capacity(plans.len());
    for plan in plans {
        if plan.planned.job.coord.dimension != dimension {
            return Err(format!(
                "瓦片 {}, {} 维度与请求不匹配",
                plan.coord.0, plan.coord.1
            ));
        }
        if plan
            .planned
            .chunk_positions
            .as_deref()
            .is_some_and(|positions| positions.is_empty())
        {
            continue;
        }
        planned_tiles.push(plan.planned);
    }

    if planned_tiles.is_empty() {
        let stats = RenderPipelineStats {
            planned_tiles: requested_tile_count,
            ..RenderPipelineStats::default()
        };
        core::send_viewport_composite_event_or_cancel(
            &event_sender,
            &render_cancel,
            core::ViewportCompositeEvent::Complete {
                frame: None,
                requested_tiles,
                rendered_tiles: 0,
                failed_tiles: 0,
                diagnostics: RenderDiagnostics::default(),
                stats,
            },
        )
        .map_err(|error| format!("视口合成完成事件发送失败: {error}"))?;
        return Ok(());
    }

    let render_options = core::interactive_render_options(
        render_backend,
        render_gpu_backend,
        cpu_budget,
        RenderTilePriority::DistanceFrom {
            tile_x: center_tile.0,
            tile_z: center_tile.1,
        },
        render_cancel.clone(),
        cache_policy,
        tile_cache_validation_seed,
        planned_tiles.len(),
    );
    let output_options = RenderTileOutputOptions {
        pixel_format: TilePixelFormat::Rgba8,
    };
    let stream_cancel = render_cancel.clone();
    let failed_tiles = Arc::new(AtomicUsize::new(0));

    let render_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        render_session.render_web_tiles_streaming_blocking_v2(
            &planned_tiles,
            render_options,
            output_options,
            {
                let compositor = Arc::clone(&compositor);
                let requested_tiles = requested_tiles.clone();
                let failed_tiles = Arc::clone(&failed_tiles);
                move |event| {
                    if stream_cancel.is_cancelled() {
                        return Err(bedrock_render::BedrockRenderError::Cancelled);
                    }
                    match event {
                        TileStreamEventV2::Ready { planned, tile, .. } => {
                            let coord = (planned.job.coord.x, planned.job.coord.z);
                            let preview = {
                                let mut compositor = compositor.lock().map_err(|_| {
                                    bedrock_render::BedrockRenderError::Validation(
                                        "视口合成状态锁已损坏".to_string(),
                                    )
                                })?;
                                compositor
                                    .blend_tile(coord, &tile)
                                    .map_err(bedrock_render::BedrockRenderError::Validation)?
                            };
                            if let Some(preview) = preview {
                                core::send_viewport_composite_event_or_cancel(
                                    &event_sender,
                                    &stream_cancel,
                                    core::ViewportCompositeEvent::Tile { tile: preview },
                                )?;
                                std::thread::sleep(PROGRESSIVE_PREVIEW_PRESENT_INTERVAL);
                            }
                        }
                        TileStreamEventV2::Empty { .. } => {}
                        TileStreamEventV2::Failed { planned, error } => {
                            failed_tiles.fetch_add(1, Ordering::Relaxed);
                            tracing::debug!(
                                tile = ?(planned.job.coord.x, planned.job.coord.z),
                                %error,
                                "map_viewer viewport_composite_tile_failed"
                            );
                        }
                        TileStreamEventV2::Progress(_) => {}
                        TileStreamEventV2::Complete {
                            diagnostics,
                            mut stats,
                        } => {
                            stats.planned_tiles = requested_tile_count;
                            let (frame, rendered_tiles) = {
                                let mut compositor = compositor.lock().map_err(|_| {
                                    bedrock_render::BedrockRenderError::Validation(
                                        "视口合成状态锁已损坏".to_string(),
                                    )
                                })?;
                                let rendered_tiles = compositor.rendered_tiles();
                                let frame = compositor
                                    .finish_frame()
                                    .map_err(bedrock_render::BedrockRenderError::Validation)?;
                                (frame, rendered_tiles)
                            };
                            let failed = failed_tiles.load(Ordering::Relaxed);
                            if failed > 0 {
                                tracing::warn!(
                                    failed_tiles = failed,
                                    rendered_tiles,
                                    "map_viewer viewport_composite_completed_with_partial_failures"
                                );
                            }
                            core::send_viewport_composite_event_or_cancel(
                                &event_sender,
                                &stream_cancel,
                                core::ViewportCompositeEvent::Complete {
                                    frame,
                                    requested_tiles: requested_tiles.clone(),
                                    rendered_tiles,
                                    failed_tiles: 0,
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
    .map_err(|_| "视口合成任务崩溃".to_string())?
    .map_err(|error| format!("视口合成失败: {error}"));
    render_result?;
    Ok(())
}

#[derive(Clone, Copy)]
struct DirtyRect {
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
}

struct ProgressiveViewportCompositor {
    viewport: MapViewport,
    layout: RenderLayout,
    render_range: MapRenderRange,
    width: u32,
    height: u32,
    output_scale: f32,
    pixels: Vec<u8>,
    rendered_tiles: usize,
    dirty_tiles: usize,
    dirty: Option<DirtyRect>,
    preview_frames: usize,
}

impl ProgressiveViewportCompositor {
    fn new(
        viewport: MapViewport,
        layout: RenderLayout,
        render_range: MapRenderRange,
        width: u32,
        height: u32,
        output_scale: f32,
    ) -> Result<Self, String> {
        let byte_len = decoded_tile_byte_len(width, height)?;
        Ok(Self {
            viewport,
            layout,
            render_range,
            width,
            height,
            output_scale,
            pixels: vec![0; byte_len],
            rendered_tiles: 0,
            dirty_tiles: 0,
            dirty: None,
            preview_frames: 0,
        })
    }

    fn blend_tile(
        &mut self,
        coord: (i32, i32),
        tile: &DecodedTileImage,
    ) -> Result<Option<core::ViewportCompositeTile>, String> {
        if tile.pixel_format != TilePixelFormat::Rgba8 {
            return Err(format!("视口合成不支持像素格式: {:?}", tile.pixel_format));
        }
        let expected_len = decoded_tile_byte_len(tile.width, tile.height)?;
        let source_pixels = tile.pixels.as_ref();
        if source_pixels.len() != expected_len {
            return Err(format!(
                "视口合成瓦片像素长度不匹配: expected {expected_len}, got {}",
                source_pixels.len()
            ));
        }
        let Some(rect) = tile_paint_rect(
            self.viewport,
            self.layout,
            self.render_range,
            coord.0,
            coord.1,
        ) else {
            return Ok(None);
        };
        if rect.width() <= 0.0 || rect.height() <= 0.0 {
            return Ok(None);
        }

        let left = (rect.left * self.output_scale).floor().max(0.0) as u32;
        let top = (rect.top * self.output_scale).floor().max(0.0) as u32;
        let right = (rect.right * self.output_scale)
            .ceil()
            .min(self.width as f32)
            .max(0.0) as u32;
        let bottom = (rect.bottom * self.output_scale)
            .ceil()
            .min(self.height as f32)
            .max(0.0) as u32;
        if right <= left || bottom <= top {
            return Ok(None);
        }

        let output_stride = usize::try_from(self.width)
            .ok()
            .and_then(|width| width.checked_mul(4))
            .ok_or_else(|| "视口合成输出 stride 溢出".to_string())?;
        let source_stride = usize::try_from(tile.width)
            .ok()
            .and_then(|width| width.checked_mul(4))
            .ok_or_else(|| "视口合成源 stride 溢出".to_string())?;
        let source_width = tile.width.max(1);
        let source_height = tile.height.max(1);

        for output_y in top..bottom {
            let screen_y = (output_y as f32 + 0.5) / self.output_scale;
            let source_y = (((screen_y - rect.top) / rect.height()) * source_height as f32)
                .floor()
                .clamp(0.0, source_height.saturating_sub(1) as f32)
                as u32;
            let output_row = usize::try_from(output_y)
                .ok()
                .and_then(|row| row.checked_mul(output_stride))
                .ok_or_else(|| "视口合成输出行偏移溢出".to_string())?;
            let source_row = usize::try_from(source_y)
                .ok()
                .and_then(|row| row.checked_mul(source_stride))
                .ok_or_else(|| "视口合成源行偏移溢出".to_string())?;
            for output_x in left..right {
                let screen_x = (output_x as f32 + 0.5) / self.output_scale;
                let source_x = (((screen_x - rect.left) / rect.width()) * source_width as f32)
                    .floor()
                    .clamp(0.0, source_width.saturating_sub(1) as f32)
                    as u32;
                let output_index = output_row
                    .checked_add(
                        usize::try_from(output_x)
                            .ok()
                            .and_then(|column| column.checked_mul(4))
                            .ok_or_else(|| "视口合成输出列偏移溢出".to_string())?,
                    )
                    .ok_or_else(|| "视口合成输出像素偏移溢出".to_string())?;
                let source_index = source_row
                    .checked_add(
                        usize::try_from(source_x)
                            .ok()
                            .and_then(|column| column.checked_mul(4))
                            .ok_or_else(|| "视口合成源列偏移溢出".to_string())?,
                    )
                    .ok_or_else(|| "视口合成源像素偏移溢出".to_string())?;
                self.pixels
                    .get_mut(output_index..output_index + 4)
                    .ok_or_else(|| "视口合成输出像素越界".to_string())?
                    .copy_from_slice(
                        source_pixels
                            .get(source_index..source_index + 4)
                            .ok_or_else(|| "视口合成源像素越界".to_string())?,
                    );
            }
        }

        self.rendered_tiles = self.rendered_tiles.saturating_add(1);
        self.dirty_tiles = self.dirty_tiles.saturating_add(1);
        self.dirty = Some(match self.dirty {
            Some(dirty) => DirtyRect {
                left: dirty.left.min(left),
                top: dirty.top.min(top),
                right: dirty.right.max(right),
                bottom: dirty.bottom.max(bottom),
            },
            None => DirtyRect {
                left,
                top,
                right,
                bottom,
            },
        });

        let should_publish =
            self.preview_frames == 0 || self.dirty_tiles >= PROGRESSIVE_PREVIEW_TILE_GROUP;
        if should_publish && self.preview_frames < MAX_PROGRESSIVE_PREVIEW_FRAMES {
            return self.snapshot_dirty_region().map(Some);
        }
        Ok(None)
    }

    fn snapshot_dirty_region(&mut self) -> Result<core::ViewportCompositeTile, String> {
        let dirty = self
            .dirty
            .take()
            .ok_or_else(|| "视口合成没有待发布区域".to_string())?;
        let width = dirty.right.saturating_sub(dirty.left);
        let height = dirty.bottom.saturating_sub(dirty.top);
        let byte_len = decoded_tile_byte_len(width, height)?;
        let mut pixels = vec![0; byte_len];
        let source_stride = usize::try_from(self.width)
            .ok()
            .and_then(|value| value.checked_mul(4))
            .ok_or_else(|| "视口合成源 stride 溢出".to_string())?;
        let target_stride = usize::try_from(width)
            .ok()
            .and_then(|value| value.checked_mul(4))
            .ok_or_else(|| "视口合成预览 stride 溢出".to_string())?;
        let left_bytes = usize::try_from(dirty.left)
            .ok()
            .and_then(|value| value.checked_mul(4))
            .ok_or_else(|| "视口合成预览 X 偏移溢出".to_string())?;
        let top = usize::try_from(dirty.top).map_err(|_| "视口合成预览 Y 偏移无效".to_string())?;
        let rows = usize::try_from(height).map_err(|_| "视口合成预览高度无效".to_string())?;
        for row in 0..rows {
            let source_start = top
                .checked_add(row)
                .and_then(|value| value.checked_mul(source_stride))
                .and_then(|value| value.checked_add(left_bytes))
                .ok_or_else(|| "视口合成预览源偏移溢出".to_string())?;
            let source_end = source_start
                .checked_add(target_stride)
                .ok_or_else(|| "视口合成预览源末尾溢出".to_string())?;
            let target_start = row
                .checked_mul(target_stride)
                .ok_or_else(|| "视口合成预览目标偏移溢出".to_string())?;
            let target_end = target_start
                .checked_add(target_stride)
                .ok_or_else(|| "视口合成预览目标末尾溢出".to_string())?;
            pixels
                .get_mut(target_start..target_end)
                .ok_or_else(|| "视口合成预览目标越界".to_string())?
                .copy_from_slice(
                    self.pixels
                        .get(source_start..source_end)
                        .ok_or_else(|| "视口合成预览源越界".to_string())?,
                );
        }

        let estimated_bytes = pixels.len();
        let image = RenderImage::from_raw_pixels(width, height, ImagePixelFormat::Rgba8, pixels)
            .map_err(|error| format!("视口合成预览图创建失败: {error}"))?;
        self.dirty_tiles = 0;
        self.preview_frames = self.preview_frames.saturating_add(1);
        let inverse_scale = 1.0 / self.output_scale.max(0.001);
        Ok(core::ViewportCompositeTile {
            image: Arc::new(image),
            source_viewport: self.viewport,
            left: dirty.left as f32 * inverse_scale,
            top: dirty.top as f32 * inverse_scale,
            width: width as f32 * inverse_scale,
            height: height as f32 * inverse_scale,
            estimated_bytes,
        })
    }

    fn finish_frame(&mut self) -> Result<Option<core::ViewportCompositeFrame>, String> {
        if self.rendered_tiles == 0 {
            return Ok(None);
        }
        let pixels = std::mem::take(&mut self.pixels);
        let estimated_bytes = pixels.len();
        let image =
            RenderImage::from_raw_pixels(self.width, self.height, ImagePixelFormat::Rgba8, pixels)
                .map_err(|error| format!("视口合成图创建失败: {error}"))?;
        Ok(Some(core::ViewportCompositeFrame {
            image: Arc::new(image),
            source_viewport: self.viewport,
            width: self.width,
            height: self.height,
            estimated_bytes,
            rendered_tiles: self.rendered_tiles,
        }))
    }

    fn rendered_tiles(&self) -> usize {
        self.rendered_tiles
    }
}

fn viewport_composite_image_size(viewport: MapViewport) -> Result<(u32, u32, f32), String> {
    if !viewport.width.is_finite() || !viewport.height.is_finite() {
        return Err("视口合成尺寸无效".to_string());
    }
    let viewport_width = viewport.width.ceil().max(1.0);
    let viewport_height = viewport.height.ceil().max(1.0);
    let max_dimension = MAX_VIEWPORT_COMPOSITE_DIMENSION as f32;
    let output_scale = (max_dimension / viewport_width)
        .min(max_dimension / viewport_height)
        .min(1.0)
        .max(0.001);
    let width = (viewport_width * output_scale).ceil().max(1.0) as u32;
    let height = (viewport_height * output_scale).ceil().max(1.0) as u32;
    Ok((width, height, output_scale))
}
