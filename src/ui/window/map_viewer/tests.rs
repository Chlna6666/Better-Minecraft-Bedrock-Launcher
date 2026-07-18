use super::canvas::*;
use super::editor::*;
use super::helpers::*;
use super::interactions::*;
use super::layout::{hud_stack_rects, top_toolbar_layout};
use super::lifecycle::*;
use super::mcstructure;
use super::model::*;
use super::overlays::*;
use super::paint::*;
use super::panels::*;
use super::players::*;
use super::prelude::*;
use super::query_cache::*;
use super::tile_cache::*;
use super::tile_manifest::*;
use super::tile_plan::*;
use super::tile_render::*;
use super::tile_state::*;
use super::view::{MapLayerKind, map_render_layer_order};
use super::viewport::*;
use super::*;

fn context_menu_entry_labels(entries: &[ContextMenuEntry]) -> Vec<&str> {
    let mut labels = Vec::new();
    for entry in entries {
        match entry {
            ContextMenuEntry::Item(item) => labels.push(item.label.as_ref()),
            ContextMenuEntry::Submenu {
                label,
                expanded,
                items,
                ..
            } => {
                labels.push(label.as_ref());
                if *expanded {
                    labels.extend(items.iter().map(|item| item.label.as_ref()));
                }
            }
        }
    }
    labels
}

fn test_tile(color: [u8; 4]) -> ViewerTile {
    let image = gpui::image::RgbaImage::from_raw(1, 1, color.to_vec()).expect("test tile image");
    ViewerTile {
        image: Arc::new(RenderImage::new(vec![gpui::image::Frame::new(image)])),
        pixel_format: Some(TilePixelFormat::Rgba8),
        width: 1,
        height: 1,
        estimated_bytes: 4,
    }
}

fn test_ready_tile(coord: (i32, i32), source: TileReadySource) -> ReadyTile {
    ReadyTile {
        coord,
        tile: test_tile([coord.0 as u8, coord.1 as u8, 3, 255]),
        source,
        chunk_positions: None,
    }
}

fn test_paste_preview_image(color: [u8; 4], chunk_x: i32) -> PastePreviewImage {
    let image = gpui::image::RgbaImage::from_raw(1, 1, color.to_vec()).expect("test preview image");
    PastePreviewImage {
        target: ChunkPos {
            x: chunk_x,
            z: 0,
            dimension: Dimension::Overworld,
        },
        image: Arc::new(RenderImage::new(vec![gpui::image::Frame::new(image)])),
        width: 1,
        height: 1,
    }
}

#[::core::prelude::v1::test]
fn decoded_rgba_tile_wraps_without_channel_swap() {
    let pixels = vec![1, 2, 3, 255];
    let tile = DecodedTileImage {
        coord: TileCoord {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        },
        width: 1,
        height: 1,
        pixels: Arc::from(pixels.clone()),
        pixel_format: TilePixelFormat::Rgba8,
    };
    let (image, _pixel_format, _width, _height, estimated_bytes) =
        render_image_from_decoded_tile_parts(
            tile.width,
            tile.height,
            tile.pixel_format,
            tile.pixels,
        )
        .expect("render decoded tile");

    assert_eq!(estimated_bytes, pixels.len());
    assert_eq!(image.as_bytes(0).expect("resident image bytes"), pixels);
    assert_eq!(
        image.pixel_format(0),
        Some(gpui::RenderImagePixelFormat::Rgba8)
    );
}

#[::core::prelude::v1::test]
fn decoded_shared_rgba_tile_reuses_pixel_storage() {
    let pixels: Arc<[u8]> = Arc::<[u8]>::from([1, 2, 3, 255]);
    let tile = DecodedTileImage {
        coord: TileCoord {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        },
        width: 1,
        height: 1,
        pixels: pixels.clone(),
        pixel_format: TilePixelFormat::Rgba8,
    };

    let (image, _pixel_format, _width, _height, estimated_bytes) =
        render_image_from_decoded_tile_parts(
            tile.width,
            tile.height,
            tile.pixel_format,
            tile.pixels,
        )
        .expect("render decoded tile");

    assert_eq!(estimated_bytes, pixels.len());
    assert!(std::ptr::eq(
        image.as_bytes(0).expect("resident image bytes").as_ptr(),
        pixels.as_ptr(),
    ));
}

#[::core::prelude::v1::test]
fn empty_viewport_composite_frame_is_transparent_rgba() {
    let viewport = MapViewport::new(size(px(200.0), px(100.0)));
    let frame = empty_viewport_composite_frame(viewport).expect("empty composite frame");

    assert_eq!(frame.width, 200);
    assert_eq!(frame.height, 100);
    assert_eq!(frame.estimated_bytes, 200 * 100 * 4);
    assert_eq!(frame.rendered_tiles, 0);
    assert_eq!(frame.source_viewport, viewport);
    assert_eq!(
        frame.image.pixel_format(0),
        Some(gpui::RenderImagePixelFormat::Rgba8)
    );
    assert!(
        frame
            .image
            .as_bytes(0)
            .expect("empty composite image bytes")
            .iter()
            .all(|byte| *byte == 0)
    );
}

#[::core::prelude::v1::test]
fn viewport_composite_does_not_take_over_tile_scheduling() {
    assert!(!VIEWPORT_COMPOSITE_ENABLED);
    assert!(!viewport_composite_owns_viewport(false, false, false));
    assert!(!viewport_composite_owns_viewport(false, true, false));
    assert!(!viewport_composite_owns_viewport(false, false, true));
    assert!(!viewport_composite_owns_viewport(true, true, true));
}

#[::core::prelude::v1::test]
fn overlay_icons_grow_with_the_map_scale() {
    let layout = web_relief_render_layout();
    let mut viewport = test_viewport(0.0, 0.0, 512.0, 512.0);
    viewport.scale = 0.25;
    let overview_size = overlay_icon_size_px(viewport, layout);
    viewport.scale = 2.0;
    let detail_size = overlay_icon_size_px(viewport, layout);

    assert_eq!(overview_size, 16.0);
    assert!(detail_size > overview_size);
    assert!(detail_size <= 52.0);
}

#[::core::prelude::v1::test]
fn screen_image_bounds_tracks_current_viewport_drag() {
    let source_viewport = MapViewport::new(size(px(400.0), px(300.0)));
    let mut current_viewport = source_viewport;
    current_viewport.offset_x += 32.0;
    current_viewport.offset_y -= 16.0;
    let screen_image = ScreenPaintImage {
        image: test_tile([1, 2, 3, 255]).image,
        source_viewport,
        left: 0.0,
        top: 0.0,
        width: source_viewport.width,
        height: source_viewport.height,
        estimated_bytes: 400 * 300 * 4,
    };

    let bounds = super::canvas::screen_image_bounds(
        Bounds::new(point(px(0.0), px(0.0)), size(px(400.0), px(300.0))),
        current_viewport,
        &screen_image,
    )
    .expect("screen image bounds");

    assert_eq!(bounds.left() / px(1.0), 32.0);
    assert_eq!(bounds.top() / px(1.0), -16.0);
    assert_eq!(bounds.size.width / px(1.0), 400.0);
    assert_eq!(bounds.size.height / px(1.0), 300.0);
}

#[::core::prelude::v1::test]
fn viewport_composite_overscan_covers_bounded_drag() {
    let viewport = MapViewport::new(size(px(1000.0), px(600.0)));
    let source_viewport = viewport_with_composite_overscan(viewport);
    let overscan_x = source_viewport.offset_x - viewport.offset_x;
    let overscan_y = source_viewport.offset_y - viewport.offset_y;

    assert!(
        (MIN_VIEWPORT_COMPOSITE_OVERSCAN_PX..=MAX_VIEWPORT_COMPOSITE_OVERSCAN_PX)
            .contains(&overscan_x)
    );
    assert!(
        (MIN_VIEWPORT_COMPOSITE_OVERSCAN_PX..=MAX_VIEWPORT_COMPOSITE_OVERSCAN_PX)
            .contains(&overscan_y)
    );
    assert_eq!(source_viewport.width, viewport.width + overscan_x * 2.0);
    assert_eq!(source_viewport.height, viewport.height + overscan_y * 2.0);
}

#[::core::prelude::v1::test]
fn viewport_composite_cancel_error_is_control_flow() {
    assert!(viewport_composite_error_is_cancelled(
        "视口合成失败: render was cancelled"
    ));
    assert!(!viewport_composite_error_is_cancelled(
        "视口合成失败: LevelDB checksum mismatch"
    ));
}

#[::core::prelude::v1::test]
fn screen_image_bounds_projects_zoomed_viewport() {
    let source_viewport = MapViewport::new(size(px(400.0), px(300.0)));
    let mut current_viewport = source_viewport;
    current_viewport.scale *= 1.25;
    let screen_image = ScreenPaintImage {
        image: test_tile([1, 2, 3, 255]).image,
        source_viewport,
        left: 0.0,
        top: 0.0,
        width: source_viewport.width,
        height: source_viewport.height,
        estimated_bytes: 400 * 300 * 4,
    };

    let bounds = super::canvas::screen_image_bounds(
        Bounds::new(point(px(0.0), px(0.0)), size(px(400.0), px(300.0))),
        current_viewport,
        &screen_image,
    )
    .expect("screen image bounds");

    assert_eq!(bounds.left() / px(1.0), -50.0);
    assert_eq!(bounds.top() / px(1.0), -37.5);
    assert_eq!(bounds.size.width / px(1.0), 500.0);
    assert_eq!(bounds.size.height / px(1.0), 375.0);
}

#[::core::prelude::v1::test]
fn screen_image_bounds_projects_resized_viewport() {
    let source_viewport = MapViewport::new(size(px(400.0), px(300.0)));
    let mut current_viewport = source_viewport;
    assert!(current_viewport.set_size(size(px(480.0), px(300.0))));
    let screen_image = ScreenPaintImage {
        image: test_tile([1, 2, 3, 255]).image,
        source_viewport,
        left: 0.0,
        top: 0.0,
        width: source_viewport.width,
        height: source_viewport.height,
        estimated_bytes: 400 * 300 * 4,
    };

    let bounds = super::canvas::screen_image_bounds(
        Bounds::new(point(px(0.0), px(0.0)), size(px(480.0), px(300.0))),
        current_viewport,
        &screen_image,
    )
    .expect("screen image bounds");

    assert_eq!(bounds.left() / px(1.0), 40.0);
    assert_eq!(bounds.top() / px(1.0), 0.0);
    assert_eq!(bounds.size.width / px(1.0), 400.0);
    assert_eq!(bounds.size.height / px(1.0), 300.0);
}

#[::core::prelude::v1::test]
fn chunk_transfer_task_progress_sync_resets_on_phase_change() {
    let mut progress_sync = ChunkTransferTaskProgressSync::default();

    let first = progress_sync.next_delta(&ChunkTransferProgress {
        phase: SharedString::from("读取区块"),
        completed: 2,
        total: 4,
    });
    assert_eq!(first, (true, 2, Some(4)));

    let same_phase = progress_sync.next_delta(&ChunkTransferProgress {
        phase: SharedString::from("读取区块"),
        completed: 3,
        total: 4,
    });
    assert_eq!(same_phase, (false, 1, Some(4)));

    let next_phase = progress_sync.next_delta(&ChunkTransferProgress {
        phase: SharedString::from("写入文件"),
        completed: 1,
        total: 2,
    });
    assert_eq!(next_phase, (true, 1, Some(2)));
}

#[::core::prelude::v1::test]
fn interactive_render_defaults_request_gpu_with_cpu_fallback() {
    assert_eq!(default_interactive_render_backend(), RenderBackend::Auto);
    assert_eq!(
        default_interactive_render_gpu_backend(),
        RenderGpuBackend::Auto
    );

    let options = interactive_render_options(
        default_interactive_render_backend(),
        default_interactive_render_gpu_backend(),
        RenderCpuBudget::default(),
        RenderTilePriority::RowMajor,
        RenderCancelFlag::new(),
        RenderCachePolicy::Use,
        1,
        1,
    );

    assert_eq!(
        options.gpu.fallback_policy,
        RenderGpuFallbackPolicy::AllowCpu
    );
    assert_eq!(
        options.gpu.pipeline_level,
        RenderGpuPipelineLevel::ComposeOnly
    );
    assert_eq!(
        options.gpu.batch_pixels,
        DEFAULT_TILE_SIZE as usize * DEFAULT_TILE_SIZE as usize
    );
}

#[::core::prelude::v1::test]
fn gpu_status_text_reports_chinese_states() {
    let cpu_default = RenderPipelineStats::default();
    let cpu_default_text = gpu_status_text(&cpu_default);
    assert!(cpu_default_text.contains("交互默认 CPU"));
    assert_eq!(
        render_gpu_backend_status_zh(&cpu_default),
        "GPU 未启用".to_string()
    );

    let waiting = RenderPipelineStats {
        resolved_backend: ResolvedRenderBackend::Dx11,
        gpu_actual_backend: RenderGpuBackend::Dx11,
        ..RenderPipelineStats::default()
    };
    let waiting_text = gpu_status_text(&waiting);
    assert!(waiting_text.contains("GPU 合成已启用"));
    assert!(waiting_text.contains("等待可提交批次"));
    assert!(waiting_text.contains("DX11"));

    let fallback = RenderPipelineStats {
        gpu_fallback_reason: Some("gpu backend not compiled".to_string()),
        ..RenderPipelineStats::default()
    };
    let fallback_text = gpu_status_text(&fallback);
    assert!(fallback_text.contains("GPU 已回退 CPU"));
    assert!(fallback_text.contains("GPU 后端未编译"));
}

fn test_viewport(offset_x: f32, offset_y: f32, width: f32, height: f32) -> MapViewport {
    MapViewport {
        offset_x,
        offset_y,
        scale: 1.0,
        width,
        height,
        initialized: true,
    }
}

#[::core::prelude::v1::test]
fn render_range_aligns_camera_to_chunk_grid() {
    let layout = web_relief_render_layout();
    let viewport = test_viewport(0.0, 0.0, 128.0, 128.0);
    let range = tile_render_range_for_viewport(viewport, layout).expect("render range");

    assert_eq!(range.min_chunk_x, -1);
    assert_eq!(range.min_chunk_z, -1);
    assert_eq!(range.max_chunk_x, 1);
    assert_eq!(range.max_chunk_z, 1);
    assert_eq!(range.render_origin_x, -64.0);
    assert_eq!(range.render_origin_y, -64.0);
    assert_eq!(
        range.tile_bounds(),
        TileBounds {
            min_x: -1,
            max_x: 0,
            min_z: -1,
            max_z: 0,
        }
    );
}

#[::core::prelude::v1::test]
fn visible_tile_bounds_do_not_use_partial_manifest_bounds() {
    let layout = web_relief_render_layout();
    let viewport = test_viewport(0.0, 0.0, 4160.0, 4160.0);
    let range = region_render_range_for_viewport(viewport, layout).expect("render range");
    let bounds =
        visible_tile_bounds_for_render_range(range, viewport.center_tile(layout)).expect("bounds");
    let partial_manifest_bounds = ChunkBounds {
        dimension: Dimension::Overworld,
        min_chunk_x: 0,
        min_chunk_z: 0,
        max_chunk_x: 31,
        max_chunk_z: 31,
        chunk_count: 1,
    };
    let partial_manifest_tile = TileBounds {
        min_x: partial_manifest_bounds
            .min_chunk_x
            .div_euclid(i32::try_from(layout.chunks_per_tile).expect("chunks per tile")),
        max_x: partial_manifest_bounds
            .max_chunk_x
            .div_euclid(i32::try_from(layout.chunks_per_tile).expect("chunks per tile")),
        min_z: partial_manifest_bounds
            .min_chunk_z
            .div_euclid(i32::try_from(layout.chunks_per_tile).expect("chunks per tile")),
        max_z: partial_manifest_bounds
            .max_chunk_z
            .div_euclid(i32::try_from(layout.chunks_per_tile).expect("chunks per tile")),
    };

    assert!(bounds.min_x < partial_manifest_tile.min_x);
    assert!(bounds.max_x > partial_manifest_tile.max_x);
    assert!(bounds.min_z < partial_manifest_tile.min_z);
    assert!(bounds.max_z > partial_manifest_tile.max_z);
}

#[::core::prelude::v1::test]
fn precise_visible_tile_bounds_do_not_include_chunk_alignment_padding() {
    let layout = web_relief_render_layout();
    let viewport = test_viewport(0.0, 0.0, 128.0, 128.0);
    let center = viewport.center_tile(layout);
    let range = region_render_range_for_viewport(viewport, layout).expect("render range");
    let coarse_bounds = visible_tile_bounds_for_render_range(range, center).expect("coarse bounds");
    let precise_bounds =
        visible_tile_bounds_for_viewport(viewport, layout, center).expect("precise bounds");

    assert_eq!(
        coarse_bounds,
        TileBounds {
            min_x: -1,
            max_x: 0,
            min_z: -1,
            max_z: 0,
        }
    );
    assert_eq!(
        precise_bounds,
        TileBounds {
            min_x: 0,
            max_x: 0,
            min_z: 0,
            max_z: 0,
        }
    );
}

#[::core::prelude::v1::test]
fn minimum_zoom_visible_bounds_cover_full_viewport() {
    let layout = web_relief_render_layout();
    let mut viewport = test_viewport(4096.0, 4096.0, 1920.0, 1080.0);
    viewport.scale = MIN_VIEWPORT_SCALE;
    let center = viewport.center_tile(layout);
    let bounds = visible_tile_bounds_for_viewport(viewport, layout, center).expect("bounds");
    let width = bounds.max_x - bounds.min_x + 1;
    let height = bounds.max_z - bounds.min_z + 1;
    let visible_count = tile_bounds_area(bounds);

    assert!(width > MAX_TILE_SPAN_PER_AXIS);
    assert!(height > MAX_TILE_SPAN_PER_AXIS);
    assert!(visible_count > 64);
}

#[::core::prelude::v1::test]
fn physical_render_batch_budget_holds_capacity_until_permit_drop() {
    let budget = PhysicalRenderBatchBudget::default();
    let permit = budget.try_acquire(1).expect("first permit");

    assert_eq!(budget.active(), 1);
    assert!(budget.try_acquire(1).is_none());
    drop(permit);
    assert_eq!(budget.active(), 0);
    assert!(budget.try_acquire(1).is_some());
}

#[::core::prelude::v1::test]
fn low_zoom_paint_bounds_remain_span_limited_when_visible_bounds_are_large() {
    let layout = web_relief_render_layout();
    let mut viewport = test_viewport(4096.0, 4096.0, 1920.0, 1080.0);
    viewport.scale = MIN_VIEWPORT_SCALE;
    let center = viewport.center_tile(layout);
    let visible = visible_tile_bounds_for_viewport(viewport, layout, center).expect("visible");
    let paint_bounds =
        paint_tile_bounds_for_viewport(viewport, layout, DRAG_RETAIN_RADIUS).expect("paint bounds");

    assert!(tile_bounds_area(paint_bounds) < tile_bounds_area(visible.expand(DRAG_RETAIN_RADIUS)));
    assert!(tile_bounds_contains(paint_bounds, center));
}

#[::core::prelude::v1::test]
fn low_zoom_paint_bounds_are_not_capped_by_canvas_image_limit() {
    let layout = web_relief_render_layout();
    let mut viewport = test_viewport(4096.0, 4096.0, 520.0, 342.0);
    viewport.scale = MIN_VIEWPORT_SCALE;
    let center = viewport.center_tile(layout);
    let visible = visible_tile_bounds_for_viewport(viewport, layout, center).expect("visible");
    let canvas_budget = canvas_tile_image_budget(viewport, layout);

    let paint_bounds =
        paint_tile_bounds_for_viewport(viewport, layout, DRAG_RETAIN_RADIUS).expect("paint bounds");
    let retained = retained_tile_filter_for_viewport(viewport, layout, true).expect("retained");

    assert_eq!(canvas_budget, usize::MAX);
    assert_eq!(paint_bounds, visible.expand(DRAG_RETAIN_RADIUS));
    assert!(tile_bounds_contains(paint_bounds, center));
    assert!(retained.contains(center));
}

#[::core::prelude::v1::test]
fn canvas_tile_budget_has_no_image_limit_for_normal_zoom() {
    let layout = web_relief_render_layout();
    let viewport = test_viewport(0.0, 0.0, 1920.0, 1080.0);

    assert_eq!(canvas_tile_image_budget(viewport, layout), usize::MAX);
}

#[::core::prelude::v1::test]
fn canvas_tile_budget_has_no_image_limit_for_low_zoom_viewport_size() {
    let layout = web_relief_render_layout();
    let mut small_viewport = test_viewport(4096.0, 4096.0, 520.0, 342.0);
    small_viewport.scale = MIN_VIEWPORT_SCALE;
    let mut large_viewport = test_viewport(4096.0, 4096.0, 1920.0, 1080.0);
    large_viewport.scale = MIN_VIEWPORT_SCALE;

    let small_budget = canvas_tile_image_budget(small_viewport, layout);
    let large_budget = canvas_tile_image_budget(large_viewport, layout);

    assert_eq!(small_budget, usize::MAX);
    assert_eq!(large_budget, usize::MAX);
}

#[::core::prelude::v1::test]
fn low_zoom_ui_tile_memory_budget_covers_all_retained_tiles() {
    let mut viewport = test_viewport(4096.0, 4096.0, 1920.0, 1080.0);
    viewport.scale = MIN_VIEWPORT_SCALE;
    let retained_tiles = tile_count_for_viewport(viewport, RETAIN_RADIUS).expect("tile count");
    let retained_bytes = retained_tiles
        .saturating_mul(DEFAULT_TILE_SIZE as usize)
        .saturating_mul(DEFAULT_TILE_SIZE as usize)
        .saturating_mul(4);

    assert!(ui_tile_memory_budget_bytes(viewport) >= retained_bytes);
}

#[::core::prelude::v1::test]
fn zoom_input_clamps_to_expanded_minimum_scale() {
    let scale = parse_zoom_scale("1").expect("zoom scale");

    assert_eq!(scale, MIN_VIEWPORT_SCALE);
}

#[::core::prelude::v1::test]
fn empty_coordinate_input_uses_current_viewport_coordinate() {
    assert_eq!(
        parse_optional_i32_input("", "X", -128).expect("fallback coordinate"),
        -128
    );
    assert_eq!(
        parse_optional_i32_input(" 42 ", "Z", 0).expect("entered coordinate"),
        42
    );
}

#[::core::prelude::v1::test]
fn selection_edges_use_directional_resize_cursors() {
    assert_eq!(
        selection_cursor_for_target(
            ExistingSelectionTarget::Resize(SelectionResizeHandle::East),
            false,
        ),
        CursorStyle::ResizeLeftRight
    );
    assert_eq!(
        selection_cursor_for_target(
            ExistingSelectionTarget::Resize(SelectionResizeHandle::North),
            false,
        ),
        CursorStyle::ResizeUpDown
    );
    assert_eq!(
        selection_cursor_for_target(
            ExistingSelectionTarget::Resize(SelectionResizeHandle::NorthWest),
            false,
        ),
        CursorStyle::ResizeUpLeftDownRight
    );
    assert_eq!(
        selection_cursor_for_target(
            ExistingSelectionTarget::Resize(SelectionResizeHandle::NorthEast),
            false,
        ),
        CursorStyle::ResizeUpRightDownLeft
    );
}

#[::core::prelude::v1::test]
fn selection_cursor_uses_canvas_local_coordinates_and_tight_edge_tolerance() {
    let snapshot = SelectionHitSnapshot {
        stage_origin: point(px(100.0), px(50.0)),
        viewport: test_viewport(0.0, 0.0, 320.0, 240.0),
        layout: web_relief_render_layout(),
        selection: ChunkSelection {
            start: ChunkPos {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
            end: ChunkPos {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
        },
    };

    assert_eq!(
        selection_cursor_at(point(px(164.0), px(82.0)), snapshot, None),
        CursorStyle::ResizeLeftRight
    );
    assert_eq!(
        selection_cursor_at(point(px(95.0), px(82.0)), snapshot, None),
        CursorStyle::Arrow
    );
    assert_eq!(
        selection_cursor_at(point(px(169.0), px(82.0)), snapshot, None),
        CursorStyle::Arrow
    );
}

#[::core::prelude::v1::test]
fn paste_preview_becomes_non_interactive_after_write_starts() {
    let chunk = ChunkPos {
        x: 3,
        z: 4,
        dimension: Dimension::Overworld,
    };
    let preview = PastePreview {
        source_anchor: chunk,
        target_anchor: chunk,
        rotation: PasteRotation::NoRotation,
        transform: PasteTransform::default(),
        display_degrees: 0.0,
        drag: None,
        targets: vec![chunk],
        tools_expanded: true,
        auto_pan: None,
        write_progress: Some(PastePreviewWriteProgress {
            completed: 0,
            total: 1,
            awaiting_tile_refresh: false,
        }),
    };

    assert!(preview.is_writing());
}

#[::core::prelude::v1::test]
fn chunk_grid_is_enabled_by_default() {
    assert!(OverlayOptions::default().dense_grid);
}

#[::core::prelude::v1::test]
fn paste_completion_ignores_unrelated_render_batches() {
    assert!(paste_tile_refresh_can_finish(false, false, true));
    assert!(!paste_tile_refresh_can_finish(true, false, true));
    assert!(!paste_tile_refresh_can_finish(false, true, true));
    assert!(!paste_tile_refresh_can_finish(false, false, false));
}

#[::core::prelude::v1::test]
fn tile_rect_stays_within_its_chunk_aligned_world_bounds() {
    let layout = web_relief_render_layout();
    let viewport = test_viewport(0.0, 0.0, 128.0, 128.0);
    let range = tile_render_range_for_viewport(viewport, layout).expect("render range");
    let rect = tile_paint_rect(viewport, layout, range, 0, 0).expect("visible tile");

    assert_eq!(rect.left, 0.0);
    assert_eq!(rect.top, 0.0);
    assert_eq!(rect.right, DEFAULT_TILE_SIZE);
    assert_eq!(rect.bottom, DEFAULT_TILE_SIZE);
}

#[::core::prelude::v1::test]
fn tile_rect_tracks_viewport_after_drag_without_rebuilding_tile_snapshot() {
    let layout = web_relief_render_layout();
    let viewport = test_viewport(0.0, 0.0, 512.0, 512.0);
    let mut dragged = viewport;
    dragged.offset_x += 48.0;
    dragged.offset_y -= 24.0;
    let initial_range = tile_render_range_for_viewport(viewport, layout).expect("initial range");
    let dragged_range = tile_render_range_for_viewport(dragged, layout).expect("dragged range");
    let initial_rect =
        tile_paint_rect(viewport, layout, initial_range, 0, 0).expect("initial rect");
    let dragged_rect = tile_paint_rect(dragged, layout, dragged_range, 0, 0).expect("dragged rect");

    assert_ne!(initial_rect.left, dragged_rect.left);
    assert_ne!(initial_rect.top, dragged_rect.top);
}

#[::core::prelude::v1::test]
fn dragged_tile_rect_edges_stay_bound_to_grid_lines() {
    let layout = web_relief_render_layout();
    let mut viewport = test_viewport(0.0, 0.0, 1024.0, 1024.0);
    viewport.offset_x += 37.5;
    viewport.offset_y -= 19.25;
    let range = tile_render_range_for_viewport(viewport, layout).expect("render range");
    let rect = tile_paint_rect(viewport, layout, range, 0, 0).expect("visible tile");
    let next_x_rect = tile_paint_rect(viewport, layout, range, 1, 0).expect("next x tile");
    let next_z_rect = tile_paint_rect(viewport, layout, range, 0, 1).expect("next z tile");
    let bounds = Bounds::new(point(px(0.0), px(0.0)), size(px(1024.0), px(1024.0)));
    let tile_blocks = i32::try_from(layout.chunks_per_tile)
        .expect("chunks per tile should fit i32")
        .saturating_mul(16);
    let left_grid = screen_x_for_block(bounds, viewport, layout, 0).floor();
    let top_grid = screen_y_for_block(bounds, viewport, layout, 0).floor();
    let next_x_grid = screen_x_for_block(bounds, viewport, layout, tile_blocks).floor();
    let next_z_grid = screen_y_for_block(bounds, viewport, layout, tile_blocks).floor();

    assert!((rect.left - left_grid).abs() < 0.001);
    assert!((rect.top - top_grid).abs() < 0.001);
    assert!((next_x_rect.left - next_x_grid).abs() < 0.001);
    assert!((next_z_rect.top - next_z_grid).abs() < 0.001);
}

#[::core::prelude::v1::test]
fn grid_step_uses_largest_visible_axis_span() {
    let block_bounds = (0, -50_000, 1_000, 50_000);
    let step = grid_step_for_block_bounds(16, block_bounds, 280);

    assert!(step > 16);
    assert!(block_bounds.2.saturating_sub(block_bounds.0) / step <= 280);
    assert!(block_bounds.3.saturating_sub(block_bounds.1) / step <= 280);
}

#[::core::prelude::v1::test]
fn memory_snapshot_due_respects_throttle_interval() {
    let now = Instant::now();
    let recent = now
        .checked_sub(MAP_MEMORY_SNAPSHOT_INTERVAL / 2)
        .expect("recent instant");
    let stale = now
        .checked_sub(MAP_MEMORY_SNAPSHOT_INTERVAL + Duration::from_millis(1))
        .expect("stale instant");

    assert!(memory_snapshot_due(None, now));
    assert!(!memory_snapshot_due(Some(recent), now));
    assert!(memory_snapshot_due(Some(stale), now));
}

#[::core::prelude::v1::test]
fn retained_tile_filter_matches_circular_retain_tiles() {
    let layout = web_relief_render_layout();
    let mut viewport = test_viewport(37.5, -19.25, 1536.0, 960.0);
    viewport.scale = 0.75;
    let center = viewport.center_tile(layout);
    let visible = visible_tile_bounds_for_viewport(viewport, layout, center).expect("visible");
    let mut expanded = visible.expand(DRAG_RETAIN_RADIUS);
    clamp_tile_span(&mut expanded.min_x, &mut expanded.max_x, center.0);
    clamp_tile_span(&mut expanded.min_z, &mut expanded.max_z, center.1);
    let mut retained_bounds = expanded;
    let max_tiles = canvas_tile_image_budget(viewport, layout);
    if tile_bounds_count(retained_bounds) > max_tiles && tile_bounds_count(visible) <= max_tiles {
        retained_bounds = visible;
    } else {
        clamp_tile_count(&mut retained_bounds, center, max_tiles);
    }
    let radius_squared =
        i64::from(DRAG_RETAIN_RADIUS).saturating_mul(i64::from(DRAG_RETAIN_RADIUS));
    let filter = retained_tile_filter_for_viewport(viewport, layout, true).expect("filter");

    for z in expanded.min_z..=expanded.max_z {
        for x in expanded.min_x..=expanded.max_x {
            let expected = retained_bounds.contains((x, z))
                && squared_distance_to_tile_bounds(x, z, visible) <= radius_squared;
            assert_eq!(filter.contains((x, z)), expected);
        }
    }
}

fn tile_bounds_area(bounds: TileBounds) -> usize {
    let width = usize::try_from(bounds.max_x.saturating_sub(bounds.min_x).saturating_add(1))
        .expect("tile bounds width should fit usize");
    let height = usize::try_from(bounds.max_z.saturating_sub(bounds.min_z).saturating_add(1))
        .expect("tile bounds height should fit usize");
    width.saturating_mul(height)
}

#[::core::prelude::v1::test]
fn viewport_resize_preserves_map_center() {
    let layout = web_relief_render_layout();
    let mut viewport = test_viewport(24.0, -36.0, 900.0, 600.0);
    viewport.scale = 1.75;
    let center = viewport.center_block(layout);

    assert!(viewport.set_size(size(px(520.0), px(600.0))));

    assert_eq!(viewport.center_block(layout), center);
}

#[::core::prelude::v1::test]
fn viewport_center_chunk_uses_screen_center_not_hover() {
    let layout = web_relief_render_layout();
    let viewport = test_viewport(0.0, 0.0, 512.0, 512.0);
    let (block_x, block_z) = viewport.center_block(layout);

    assert_eq!(
        chunk_from_block(block_x, block_z, Dimension::Overworld),
        ChunkPos {
            x: 4,
            z: 4,
            dimension: Dimension::Overworld,
        }
    );
}

#[::core::prelude::v1::test]
fn tile_paint_snapshot_reuses_render_image_arc() {
    let layout = web_relief_render_layout();
    let viewport = test_viewport(0.0, 0.0, 512.0, 512.0);
    let mut manager = RegionManager::default();
    manager.mark_loaded((0, 0), test_tile([8, 16, 32, 255]));
    let source_image = manager
        .entries
        .get(&(0, 0))
        .and_then(|entry| entry.image.as_ref())
        .map(|tile| tile.image.clone())
        .expect("loaded test tile");

    let snapshot = build_tile_paint_snapshot(&manager, viewport, layout, false, RETAIN_RADIUS, 1);

    assert_eq!(snapshot.tiles.len(), 1);
    assert!(Arc::ptr_eq(&snapshot.tiles[0].image, &source_image));
}

#[::core::prelude::v1::test]
fn tile_paint_snapshot_keeps_retained_tiles_for_drag_headroom() {
    let layout = web_relief_render_layout();
    let viewport = test_viewport(0.0, 0.0, 512.0, 512.0);
    let mut manager = RegionManager::default();
    manager.mark_loaded((0, 0), test_tile([1, 1, 1, 255]));
    manager.mark_loaded((1, 0), test_tile([2, 2, 2, 255]));
    manager.mark_loaded((64, 64), test_tile([2, 2, 2, 255]));

    let snapshot = build_tile_paint_snapshot(&manager, viewport, layout, false, RETAIN_RADIUS, 1);

    assert_eq!(
        snapshot
            .tiles
            .iter()
            .map(|tile| tile.coord)
            .collect::<Vec<_>>(),
        vec![(0, 0), (1, 0)]
    );
}

#[::core::prelude::v1::test]
fn tile_paint_snapshot_records_current_paint_bounds() {
    let layout = web_relief_render_layout();
    let viewport = test_viewport(0.0, 0.0, 512.0, 512.0);
    let manager = RegionManager::default();

    let snapshot = build_tile_paint_snapshot(&manager, viewport, layout, false, RETAIN_RADIUS, 1);

    assert_eq!(
        snapshot.paint_bounds,
        paint_tile_bounds_for_viewport(viewport, layout, RETAIN_RADIUS)
    );
}

#[::core::prelude::v1::test]
fn tile_paint_snapshot_excludes_tiles_outside_retained_bounds() {
    let layout = web_relief_render_layout();
    let viewport = test_viewport(0.0, 0.0, 512.0, 512.0);
    let mut manager = RegionManager::default();
    manager.mark_loaded((0, 0), test_tile([1, 1, 1, 255]));
    manager.mark_loaded((2, 0), test_tile([2, 2, 2, 255]));

    let snapshot = build_tile_paint_snapshot(&manager, viewport, layout, false, RETAIN_RADIUS, 1);

    assert_eq!(
        snapshot
            .tiles
            .iter()
            .map(|tile| tile.coord)
            .collect::<Vec<_>>(),
        vec![(0, 0)]
    );
}

#[::core::prelude::v1::test]
fn tile_paint_snapshot_patch_replaces_visible_tile() {
    let layout = web_relief_render_layout();
    let viewport = test_viewport(0.0, 0.0, 512.0, 512.0);
    let mut manager = RegionManager::default();
    manager.mark_loaded((0, 0), test_tile([1, 1, 1, 255]));
    let snapshot = build_tile_paint_snapshot(&manager, viewport, layout, false, RETAIN_RADIUS, 1);
    manager.mark_loaded((0, 0), test_tile([2, 2, 2, 255]));
    let source_image = manager
        .entries
        .get(&(0, 0))
        .and_then(|entry| entry.image.as_ref())
        .map(|tile| tile.image.clone())
        .expect("replacement test tile");

    let patched = patch_tile_paint_snapshot(
        &snapshot,
        &manager,
        viewport,
        layout,
        false,
        RETAIN_RADIUS,
        &[(0, 0)],
        2,
    );

    let TilePaintSnapshotPatch::Patched(patched) = patched else {
        panic!("visible replacement should patch snapshot");
    };
    assert_eq!(patched.generation, 2);
    assert_eq!(patched.tiles.len(), 1);
    assert!(Arc::ptr_eq(&patched.tiles[0].image, &source_image));
}

#[::core::prelude::v1::test]
fn tile_paint_snapshot_patch_inserts_visible_tile_in_paint_order() {
    let layout = web_relief_render_layout();
    let mut viewport = test_viewport(0.0, 0.0, 512.0, 512.0);
    viewport.scale = 0.5;
    let mut manager = RegionManager::default();
    manager.mark_loaded((0, 0), test_tile([1, 1, 1, 255]));
    let snapshot = build_tile_paint_snapshot(&manager, viewport, layout, false, RETAIN_RADIUS, 1);
    manager.mark_loaded((1, 0), test_tile([2, 2, 2, 255]));

    let patched = patch_tile_paint_snapshot(
        &snapshot,
        &manager,
        viewport,
        layout,
        false,
        RETAIN_RADIUS,
        &[(1, 0)],
        2,
    );

    let TilePaintSnapshotPatch::Patched(patched) = patched else {
        panic!("new visible tile should patch snapshot");
    };
    assert_eq!(
        patched
            .tiles
            .iter()
            .map(|tile| tile.coord)
            .collect::<Vec<_>>(),
        vec![(0, 0), (1, 0)]
    );
}

#[::core::prelude::v1::test]
fn tile_paint_snapshot_patch_rebuilds_when_viewport_bounds_change() {
    let layout = web_relief_render_layout();
    let mut viewport = test_viewport(0.0, 0.0, 512.0, 512.0);
    viewport.scale = 0.5;
    let mut manager = RegionManager::default();
    manager.mark_loaded((0, 0), test_tile([1, 1, 1, 255]));
    let snapshot = build_tile_paint_snapshot(&manager, viewport, layout, false, RETAIN_RADIUS, 1);
    viewport.offset_x = -512.0;
    manager.mark_loaded((2, 0), test_tile([2, 2, 2, 255]));

    let patched = patch_tile_paint_snapshot(
        &snapshot,
        &manager,
        viewport,
        layout,
        false,
        RETAIN_RADIUS,
        &[(2, 0)],
        2,
    );

    assert!(matches!(patched, TilePaintSnapshotPatch::Rebuild));
}

#[::core::prelude::v1::test]
fn tile_paint_snapshot_patch_keeps_composite_underlay_across_drag_bounds() {
    let layout = web_relief_render_layout();
    let mut viewport = test_viewport(0.0, 0.0, 512.0, 512.0);
    viewport.scale = 0.5;
    let frame = empty_viewport_composite_frame(viewport).expect("composite frame");
    let source_image = frame.image.clone();
    let snapshot = TilePaintSnapshot {
        tiles: Arc::new(Vec::new()),
        screen_images: Arc::new(vec![ScreenPaintImage {
            image: frame.image,
            source_viewport: viewport,
            left: 0.0,
            top: 0.0,
            width: viewport.width,
            height: viewport.height,
            estimated_bytes: frame.estimated_bytes,
        }]),
        debug_overlays: Arc::new(Vec::new()),
        generation: 1,
        estimated_bytes: frame.estimated_bytes,
        paint_bounds: paint_tile_bounds_for_viewport(viewport, layout, RETAIN_RADIUS),
    };
    viewport.offset_x = -512.0;
    let coord = viewport.center_tile(layout);
    let mut manager = RegionManager::default();
    manager.mark_loaded(coord, test_tile([2, 2, 2, 255]));

    let patched = patch_tile_paint_snapshot(
        &snapshot,
        &manager,
        viewport,
        layout,
        false,
        RETAIN_RADIUS,
        &[coord],
        2,
    );

    let TilePaintSnapshotPatch::Patched(patched) = patched else {
        panic!("composite handoff should remain incremental");
    };
    assert_eq!(patched.screen_images.len(), 1);
    assert!(Arc::ptr_eq(&patched.screen_images[0].image, &source_image));
    assert_eq!(patched.tiles.len(), 1);
    assert_eq!(patched.tiles[0].coord, coord);
}

#[::core::prelude::v1::test]
fn tile_paint_snapshot_patch_replaces_existing_coord_without_duplicate() {
    let layout = web_relief_render_layout();
    let mut viewport = test_viewport(0.0, 0.0, 512.0, 512.0);
    viewport.scale = 0.5;
    let mut manager = RegionManager::default();
    manager.mark_loaded((0, 0), test_tile([3, 3, 3, 255]));
    manager.mark_loaded((1, 0), test_tile([4, 4, 4, 255]));
    let paint_bounds = paint_tile_bounds_for_viewport(viewport, layout, RETAIN_RADIUS);
    let old_tile = test_tile([1, 1, 1, 255]);
    let one_tile = manager
        .entries
        .get(&(1, 0))
        .and_then(|entry| entry.image.as_ref())
        .expect("second visible tile");
    let snapshot = TilePaintSnapshot {
        tiles: Arc::new(vec![
            PaintTile {
                coord: (1, 0),
                image: one_tile.image.clone(),
                pixel_format: one_tile.pixel_format,
                width: one_tile.width,
                height: one_tile.height,
                estimated_bytes: one_tile.estimated_bytes,
            },
            PaintTile {
                coord: (0, 0),
                image: old_tile.image,
                pixel_format: old_tile.pixel_format,
                width: old_tile.width,
                height: old_tile.height,
                estimated_bytes: old_tile.estimated_bytes,
            },
        ]),
        screen_images: Arc::new(Vec::new()),
        debug_overlays: Arc::new(Vec::new()),
        generation: 1,
        estimated_bytes: 8,
        paint_bounds,
    };

    let patched = patch_tile_paint_snapshot(
        &snapshot,
        &manager,
        viewport,
        layout,
        false,
        RETAIN_RADIUS,
        &[(0, 0)],
        2,
    );

    let TilePaintSnapshotPatch::Patched(patched) = patched else {
        panic!("existing coord replacement should patch snapshot");
    };
    let coords = patched
        .tiles
        .iter()
        .map(|tile| tile.coord)
        .collect::<Vec<_>>();
    assert_eq!(coords, vec![(0, 0), (1, 0)]);
    assert_eq!(coords.iter().filter(|coord| **coord == (0, 0)).count(), 1);
}

#[::core::prelude::v1::test]
fn tile_paint_snapshot_patch_removes_invalid_tile() {
    let layout = web_relief_render_layout();
    let viewport = test_viewport(0.0, 0.0, 512.0, 512.0);
    let mut manager = RegionManager::default();
    manager.mark_loaded((0, 0), test_tile([1, 1, 1, 255]));
    let snapshot = build_tile_paint_snapshot(&manager, viewport, layout, false, RETAIN_RADIUS, 1);
    manager.mark_invalid((0, 0), SharedString::from("empty"));

    let patched = patch_tile_paint_snapshot(
        &snapshot,
        &manager,
        viewport,
        layout,
        false,
        RETAIN_RADIUS,
        &[(0, 0)],
        2,
    );

    let TilePaintSnapshotPatch::Patched(patched) = patched else {
        panic!("invalid visible tile should patch snapshot");
    };
    assert!(patched.tiles.is_empty());
    assert!(patched.debug_overlays.is_empty());
}

#[::core::prelude::v1::test]
fn paste_preview_image_set_replacement_updates_current_images() {
    let old_image = test_paste_preview_image([1, 2, 3, 255], 0);
    let new_image = test_paste_preview_image([4, 5, 6, 255], 1);
    let new_render_image = new_image.image.clone();
    let mut current = Arc::new(vec![old_image]);

    replace_paste_preview_image_set(&mut current, vec![new_image]);

    assert_eq!(current.len(), 1);
    assert!(Arc::ptr_eq(&current[0].image, &new_render_image));
}

#[::core::prelude::v1::test]
fn canvas_tile_change_visibility_uses_visible_bleed_bounds() {
    let layout = web_relief_render_layout();
    let viewport = test_viewport(0.0, 0.0, 512.0, 512.0);
    let range = region_render_range_for_viewport(viewport, layout).expect("range");
    let bounds = visible_tile_bounds_for_render_range(range, viewport.center_tile(layout))
        .expect("visible bounds")
        .expand(1);

    assert!(tile_bounds_contains(bounds, (0, 0)));
    assert!(!tile_bounds_contains(bounds, (64, 64)));
}

#[::core::prelude::v1::test]
fn block_screen_position_matches_negative_block_math() {
    let layout = web_relief_render_layout();
    let viewport = test_viewport(17.0, -23.0, 300.0, 240.0);
    let (screen_x, screen_y) =
        viewport_screen_for_block(viewport, layout, -17, -33).expect("screen position");

    assert_eq!(
        screen_x,
        viewport.offset_x + block_to_map_pixel(-17, layout) * viewport.scale
    );
    assert_eq!(
        screen_y,
        viewport.offset_y + block_to_map_pixel(-33, layout) * viewport.scale
    );
}

#[::core::prelude::v1::test]
fn map_viewer_context_more_edit_entries_toggle_inline_items() {
    fn more_items() -> Vec<ContextMenuItem> {
        vec![
            ContextMenuItem::new("编辑 HSA 生成区"),
            ContextMenuItem::new("查看/编辑方块实体"),
            ContextMenuItem::new("编辑当前位置方块实体"),
            ContextMenuItem::new("查看/编辑实体 Actors"),
            ContextMenuItem::new("查看/编辑高度图"),
            ContextMenuItem::new("查看/编辑生物群系"),
        ]
    }

    let collapsed = context_more_edit_entries(false, more_items(), |_| {});
    let expanded = context_more_edit_entries(true, more_items(), |_| {});

    assert_eq!(context_menu_entry_labels(&collapsed), vec!["更多编辑操作"]);
    assert_eq!(
        context_menu_entry_labels(&expanded),
        vec![
            "收起更多编辑操作",
            "编辑 HSA 生成区",
            "查看/编辑方块实体",
            "编辑当前位置方块实体",
            "查看/编辑实体 Actors",
            "查看/编辑高度图",
            "查看/编辑生物群系",
        ]
    );
    assert!(
        collapsed
            .iter()
            .all(|entry| matches!(entry, ContextMenuEntry::Item(_)))
    );
    assert!(
        expanded
            .iter()
            .all(|entry| matches!(entry, ContextMenuEntry::Item(_)))
    );
}

#[::core::prelude::v1::test]
fn canvas_pointer_move_action_releases_stale_captures_without_matching_button() {
    assert_eq!(
        canvas_pointer_move_action(None, true, false, false, false),
        CanvasPointerMoveAction::ReleaseStaleCaptures
    );
    assert_eq!(
        canvas_pointer_move_action(Some(MouseButton::Left), false, true, false, false),
        CanvasPointerMoveAction::UpdateRightSelection
    );
    assert_eq!(
        canvas_pointer_move_action(None, false, true, false, false),
        CanvasPointerMoveAction::UpdateRightSelection
    );
    assert_eq!(
        canvas_pointer_move_action(None, false, false, true, false),
        CanvasPointerMoveAction::ReleaseStaleCaptures
    );
    assert_eq!(
        canvas_pointer_move_action(Some(MouseButton::Left), false, false, true, false),
        CanvasPointerMoveAction::Ignore
    );
    assert_eq!(
        canvas_pointer_move_action(Some(MouseButton::Right), false, false, true, false),
        CanvasPointerMoveAction::Ignore
    );
    assert_eq!(
        canvas_pointer_move_action(None, false, false, false, true),
        CanvasPointerMoveAction::ReleaseStaleCaptures
    );
    assert_eq!(
        canvas_pointer_move_action(Some(MouseButton::Left), true, false, false, false),
        CanvasPointerMoveAction::UpdateMapPointer
    );
    assert_eq!(
        canvas_pointer_move_action(Some(MouseButton::Right), false, true, false, false),
        CanvasPointerMoveAction::UpdateRightSelection
    );
}

#[::core::prelude::v1::test]
fn existing_selection_right_click_waits_for_release_and_respects_hit_target() {
    let selection = ChunkSelection {
        start: ChunkPos {
            x: 3,
            z: -4,
            dimension: Dimension::Overworld,
        },
        end: ChunkPos {
            x: 8,
            z: 2,
            dimension: Dimension::Overworld,
        },
    };
    let screen_bounds = SelectionScreenBounds {
        left: 100.0,
        top: 80.0,
        right: 300.0,
        bottom: 260.0,
    };
    assert_eq!(
        existing_selection_target(point(px(180.0), px(160.0)), screen_bounds, 7.0),
        ExistingSelectionTarget::Inside
    );
    assert_eq!(
        existing_selection_target(point(px(350.0), px(160.0)), screen_bounds, 7.0),
        ExistingSelectionTarget::Outside
    );
    assert_eq!(
        existing_selection_target(point(px(299.0), px(160.0)), screen_bounds, 7.0),
        ExistingSelectionTarget::Resize(SelectionResizeHandle::East)
    );
    assert_eq!(
        existing_selection_target(point(px(102.0), px(82.0)), screen_bounds, 7.0),
        ExistingSelectionTarget::Resize(SelectionResizeHandle::NorthWest)
    );
    assert_eq!(
        existing_selection_target(
            point(px(104.0), px(104.0)),
            SelectionScreenBounds {
                left: 100.0,
                top: 100.0,
                right: 108.0,
                bottom: 108.0,
            },
            7.0,
        ),
        ExistingSelectionTarget::Inside
    );

    assert_eq!(
        right_selection_release_action(
            SelectionPointerButton::Right,
            RightSelectionIntent::OpenMenu(selection),
            false,
        ),
        RightSelectionReleaseAction::OpenMenu
    );
    assert_eq!(
        right_selection_release_action(
            SelectionPointerButton::Right,
            RightSelectionIntent::OpenMenu(selection),
            true,
        ),
        RightSelectionReleaseAction::KeepSelection
    );
    assert_eq!(
        right_selection_release_action(
            SelectionPointerButton::Right,
            RightSelectionIntent::Cancel(selection),
            false,
        ),
        RightSelectionReleaseAction::CancelSelection
    );
    assert_eq!(
        right_selection_release_action(
            SelectionPointerButton::Left,
            RightSelectionIntent::Cancel(selection),
            true,
        ),
        RightSelectionReleaseAction::CancelSelection
    );
    assert_eq!(
        right_selection_release_action(
            SelectionPointerButton::Right,
            RightSelectionIntent::NewSelection,
            false,
        ),
        RightSelectionReleaseAction::ApplySelectionAndOpenMenu
    );
    assert_eq!(
        right_selection_release_action(
            SelectionPointerButton::Right,
            RightSelectionIntent::Resize {
                selection,
                handle: SelectionResizeHandle::SouthEast,
            },
            true,
        ),
        RightSelectionReleaseAction::ApplySelection
    );
    assert_eq!(
        right_selection_release_action(
            SelectionPointerButton::Left,
            RightSelectionIntent::Move(selection),
            false,
        ),
        RightSelectionReleaseAction::KeepSelection
    );
    assert_eq!(
        right_selection_release_action(
            SelectionPointerButton::Left,
            RightSelectionIntent::Move(selection),
            true,
        ),
        RightSelectionReleaseAction::ApplySelection
    );
}

#[::core::prelude::v1::test]
fn chunk_transfer_progress_ratio_is_clamped() {
    assert_eq!(
        ChunkTransferProgress {
            phase: SharedString::from("复制区块"),
            completed: 2,
            total: 4,
        }
        .ratio(),
        0.5
    );
    assert_eq!(
        ChunkTransferProgress {
            phase: SharedString::from("复制区块"),
            completed: 6,
            total: 4,
        }
        .ratio(),
        1.0
    );
    assert_eq!(
        ChunkTransferProgress {
            phase: SharedString::from("复制区块"),
            completed: 0,
            total: 0,
        }
        .ratio(),
        0.0
    );
}

#[::core::prelude::v1::test]
fn pointer_capture_release_clears_all_drag_state() {
    let mut drag = Some(DragState {
        start: point(px(1.0), px(2.0)),
        offset_x: 3.0,
        offset_y: 4.0,
        moved: true,
        last_position: point(px(1.0), px(2.0)),
        last_movement_x: 0.0,
        last_movement_y: 0.0,
    });
    let chunk = ChunkPos {
        x: 1,
        z: 2,
        dimension: Dimension::Overworld,
    };
    let mut right_selection = Some(RightSelectionDrag::new(point(px(0.0), px(0.0)), chunk));
    let mut preview_drag = Some(Preview3dDragState {
        mode: Preview3dDragMode::RotateModel,
        position: point(px(5.0), px(6.0)),
    });
    let mut dock_drag = Some(DockDragState {
        drag: DockDrag::RightPanel,
        start_x: 10.0,
        start_y: 20.0,
        start_size: 320.0,
    });

    let release = take_pointer_captures(
        &mut drag,
        &mut right_selection,
        &mut preview_drag,
        &mut dock_drag,
    );

    assert_eq!(
        release,
        PointerCaptureRelease {
            map_drag: true,
            right_selection: true,
            preview_3d_drag: true,
            dock_drag: true,
        }
    );
    assert!(drag.is_none());
    assert!(right_selection.is_none());
    assert!(preview_drag.is_none());
    assert!(dock_drag.is_none());
}

#[::core::prelude::v1::test]
fn preview_pointer_release_only_clears_preview_drag() {
    let mut preview_drag = Some(Preview3dDragState {
        mode: Preview3dDragMode::OrbitCamera,
        position: point(px(5.0), px(6.0)),
    });

    assert!(take_preview_3d_pointer_capture(&mut preview_drag));
    assert!(preview_drag.is_none());
    assert!(!take_preview_3d_pointer_capture(&mut preview_drag));
}

#[::core::prelude::v1::test]
fn slime_query_window_sizes_are_supported_ui_modes() {
    assert_eq!(SlimeQueryWindowSize::Three.value(), 3);
    assert_eq!(SlimeQueryWindowSize::Five.value(), 5);
    assert_eq!(SlimeQueryWindowSize::Seven.value(), 7);
    assert!(SlimeWindowSize::new(SlimeQueryWindowSize::Seven.value()).is_ok());
}

#[::core::prelude::v1::test]
fn map_layers_draw_grid_above_terrain_and_below_professional_overlay() {
    assert_eq!(
        map_render_layer_order(),
        [
            MapLayerKind::Terrain,
            MapLayerKind::Grid,
            MapLayerKind::ProfessionalOverlay,
            MapLayerKind::Markers
        ]
    );
}

#[::core::prelude::v1::test]
fn overlay_result_acceptance_requires_matching_generation_and_request() {
    let bounds = SlimeChunkBounds {
        dimension: Dimension::Overworld,
        min_chunk_x: -1,
        max_chunk_x: 1,
        min_chunk_z: -1,
        max_chunk_z: 1,
    };
    let options = RegionOverlayQueryOptions::default();

    assert!(accept_overlay_result(
        7,
        11,
        Some(bounds),
        Some(options),
        7,
        11,
        bounds,
        options
    ));
    assert!(!accept_overlay_result(
        7,
        12,
        Some(bounds),
        Some(options),
        7,
        11,
        bounds,
        options
    ));
    assert!(!accept_overlay_result(
        8,
        11,
        Some(bounds),
        Some(options),
        7,
        11,
        bounds,
        options
    ));
}

#[::core::prelude::v1::test]
fn map_info_query_scope_uses_sparse_world_index_after_metadata_load() {
    let visible_bounds = SlimeChunkBounds {
        dimension: Dimension::Overworld,
        min_chunk_x: 0,
        max_chunk_x: 7,
        min_chunk_z: 0,
        max_chunk_z: 7,
    };
    let chunk_bounds = ChunkBounds {
        dimension: Dimension::Overworld,
        min_chunk_x: -64,
        min_chunk_z: -32,
        max_chunk_x: 95,
        max_chunk_z: 127,
        chunk_count: 3,
    };
    let available_tiles = BTreeSet::from([(-8, -4), (0, 0), (11, 15)]);

    let scope = map_info_query_scope(
        true,
        Dimension::Overworld,
        Some(chunk_bounds),
        &available_tiles,
        Some(visible_bounds),
        8,
    )
    .expect("indexed query scope");

    assert!(scope.indexed_world);
    assert_eq!(scope.bounds.min_chunk_x, -64);
    assert_eq!(scope.bounds.max_chunk_z, 127);
    assert_eq!(scope.tile_coordinates, vec![(-8, -4), (0, 0), (11, 15)]);
}

#[::core::prelude::v1::test]
fn map_info_query_scope_falls_back_to_visible_tiles_before_metadata_load() {
    let visible_bounds = SlimeChunkBounds {
        dimension: Dimension::Overworld,
        min_chunk_x: -1,
        max_chunk_x: 8,
        min_chunk_z: -1,
        max_chunk_z: 8,
    };

    let scope = map_info_query_scope(
        false,
        Dimension::Overworld,
        None,
        &BTreeSet::new(),
        Some(visible_bounds),
        8,
    )
    .expect("visible query scope");

    assert!(!scope.indexed_world);
    assert_eq!(scope.bounds, visible_bounds);
    assert_eq!(
        scope.tile_coordinates,
        vec![
            (-1, -1),
            (0, -1),
            (1, -1),
            (-1, 0),
            (0, 0),
            (1, 0),
            (-1, 1),
            (0, 1),
            (1, 1)
        ]
    );
}

#[::core::prelude::v1::test]
fn map_query_budget_limits_background_queries_and_releases_permits() {
    let budget = MapQueryBudget::default();
    let first = budget.try_acquire().expect("first query permit");
    let second = budget.try_acquire().expect("second query permit");

    assert_eq!(budget.active(), MAP_QUERY_CONCURRENCY);
    assert!(budget.try_acquire().is_none());

    drop(first);
    assert_eq!(budget.active(), MAP_QUERY_CONCURRENCY - 1);
    drop(second);
    assert_eq!(budget.active(), 0);
}

#[::core::prelude::v1::test]
fn map_query_coordinator_reuses_typed_memory_snapshots_and_generations() {
    let coordinator = MapQueryBudget::default();
    let key = MapQueryCacheKey::new(
        MapQueryKind::Overlay,
        std::path::Path::new("world-a"),
        Dimension::Overworld.id(),
        (-4, 4, -8, 8),
        0,
    );
    coordinator.cache(key, Arc::new(String::from("cached overlay")));

    assert_eq!(
        coordinator
            .cached::<String>(key)
            .as_deref()
            .map(String::as_str),
        Some("cached overlay")
    );
    assert!(!coordinator.is_current(MapQueryKind::Overlay, 1));
    let generation = coordinator.next_generation(MapQueryKind::Overlay);
    assert!(coordinator.is_current(MapQueryKind::Overlay, generation));
    assert!(!coordinator.is_current(MapQueryKind::Overlay, generation.saturating_sub(1)));
    let village_generation = coordinator.next_generation(MapQueryKind::VillageIndex);
    assert!(coordinator.is_current(MapQueryKind::Overlay, generation));
    assert!(coordinator.is_current(MapQueryKind::VillageIndex, village_generation));
}

#[::core::prelude::v1::test]
fn slime_window_candidate_result_acceptance_rejects_stale_viewport_queries() {
    let bounds = SlimeChunkBounds {
        dimension: Dimension::Overworld,
        min_chunk_x: -1,
        max_chunk_x: 1,
        min_chunk_z: -1,
        max_chunk_z: 1,
    };

    assert!(accept_slime_window_candidate_result(
        7,
        11,
        Some(bounds),
        SlimeQueryWindowSize::Five,
        7,
        11,
        bounds,
        SlimeQueryWindowSize::Five,
    ));
    assert!(!accept_slime_window_candidate_result(
        7,
        12,
        Some(bounds),
        SlimeQueryWindowSize::Five,
        7,
        11,
        bounds,
        SlimeQueryWindowSize::Five,
    ));
    assert!(!accept_slime_window_candidate_result(
        7,
        11,
        Some(bounds),
        SlimeQueryWindowSize::Three,
        7,
        11,
        bounds,
        SlimeQueryWindowSize::Five,
    ));
}

#[::core::prelude::v1::test]
fn slime_overlay_runs_merge_adjacent_slime_chunks_on_same_row() {
    let bounds = SlimeChunkBounds {
        dimension: Dimension::Overworld,
        min_chunk_x: -8,
        max_chunk_x: 8,
        min_chunk_z: -8,
        max_chunk_z: 8,
    };
    let cache = SlimeOverlayRunCache::build(bounds).expect("small overworld cache");
    let run_slime_chunks: usize = cache
        .runs
        .iter()
        .map(|run| usize::try_from(run.max_chunk_x - run.min_chunk_x + 1).expect("positive run"))
        .sum();
    let naive_count = (bounds.min_chunk_z..=bounds.max_chunk_z)
        .flat_map(|z| (bounds.min_chunk_x..=bounds.max_chunk_x).map(move |x| (x, z)))
        .filter(|(x, z)| {
            is_slime_chunk(ChunkPos {
                x: *x,
                z: *z,
                dimension: bounds.dimension,
            })
        })
        .count();

    assert_eq!(run_slime_chunks, naive_count);
    assert!(cache.runs.len() <= naive_count);
}

#[::core::prelude::v1::test]
fn slime_overlay_runs_keep_exact_chunk_size_for_large_viewports() {
    let bounds = SlimeChunkBounds {
        dimension: Dimension::Overworld,
        min_chunk_x: -72,
        max_chunk_x: 72,
        min_chunk_z: -72,
        max_chunk_z: 72,
    };
    assert!(bounds.chunk_count() > 20_000);
    let cache = SlimeOverlayRunCache::build(bounds).expect("exact overworld cache");
    let run_slime_chunks: usize = cache
        .runs
        .iter()
        .map(|run| {
            usize::try_from(run.max_chunk_x - run.min_chunk_x + 1).expect("positive exact run")
        })
        .sum();
    let naive_count = (bounds.min_chunk_z..=bounds.max_chunk_z)
        .flat_map(|z| (bounds.min_chunk_x..=bounds.max_chunk_x).map(move |x| (x, z)))
        .filter(|(x, z)| {
            is_slime_chunk(ChunkPos {
                x: *x,
                z: *z,
                dimension: bounds.dimension,
            })
        })
        .count();
    assert_eq!(run_slime_chunks, naive_count);
}

#[::core::prelude::v1::test]
fn context_menu_chunk_uses_euclidean_negative_chunk_math() {
    let chunk = context_menu_chunk(
        ContextMenuState {
            position: point(px(0.0), px(0.0)),
            block_x: -1,
            block_z: -17,
        },
        Dimension::Overworld,
    );

    assert_eq!(chunk.x, -1);
    assert_eq!(chunk.z, -2);
    assert_eq!(chunk.dimension, Dimension::Overworld);
}

#[::core::prelude::v1::test]
fn stage_position_clamps_window_coordinates_to_preview_canvas() {
    let position = clamp_stage_position(point(px(-12.0), px(320.0)), 640.0, 240.0);
    assert_eq!(position.x, px(0.0));
    assert_eq!(position.y, px(240.0));

    let position = clamp_stage_position(point(px(128.0), px(96.0)), 640.0, 240.0);
    assert_eq!(position.x, px(128.0));
    assert_eq!(position.y, px(96.0));
}

#[::core::prelude::v1::test]
fn top_toolbar_moves_low_priority_commands_to_overflow() {
    let wide = top_toolbar_layout(1280.0);
    assert!(wide.show_modes);
    assert!(wide.show_y_controls);
    assert!(wide.show_zoom_controls);
    assert_eq!(wide.overflow_count, 0);

    let minimum = top_toolbar_layout(920.0);
    assert!(!minimum.show_modes);
    assert!(minimum.show_y_controls);
    assert!(minimum.show_zoom_controls);
    assert_eq!(minimum.overflow_count, 5);

    let medium = top_toolbar_layout(1080.0);
    assert!(medium.show_modes);
    assert_eq!(medium.overflow_count, 0);

    let small = top_toolbar_layout(480.0);
    assert!(!small.show_modes);
    assert!(!small.show_zoom_controls);
    assert_eq!(small.overflow_count, 9);
}

#[::core::prelude::v1::test]
fn center_stage_layout_accounts_for_stripe_and_docks() {
    let rect = center_stage_rect_for_layout(
        1280.0,
        860.0,
        true,
        true,
        420.0,
        true,
        260.0,
        MIN_CENTER_WIDTH,
        MIN_CENTER_HEIGHT,
    );
    assert_eq!(rect.left(), px(354.0));
    assert_eq!(rect.top(), px(62.0));
    assert_eq!(rect.size.width, px(500.0));
    assert_eq!(rect.size.height, px(502.0));

    let collapsed = center_stage_rect_for_layout(
        920.0,
        620.0,
        false,
        false,
        0.0,
        false,
        0.0,
        MIN_CENTER_WIDTH,
        MIN_CENTER_HEIGHT,
    );
    assert_eq!(collapsed.left(), px(77.0));
    assert!(collapsed.size.width >= px(MIN_CENTER_WIDTH));
}

#[::core::prelude::v1::test]
fn hud_stack_is_anchored_away_from_the_bottom_ruler() {
    let (ruler, coord) = hud_stack_rects(640.0, 360.0, true);
    let ruler = ruler.expect("ruler visible");
    assert!(ruler.bottom() <= coord.top() - px(8.0));
    assert!(coord.right() <= px(640.0));
    assert_eq!(ruler.right(), px(624.0));
    assert_eq!(ruler.top(), px(16.0));
    assert!(ruler.bottom() < px(120.0));
}

#[::core::prelude::v1::test]
fn drag_tile_snapshot_sync_is_limited_to_display_refresh_rate() {
    assert!(DRAG_CANVAS_SYNC_INTERVAL >= Duration::from_millis(16));
}

#[::core::prelude::v1::test]
fn interaction_snapshot_refreshes_after_drag_movement_or_missing_tiles() {
    assert!(interaction_needs_canvas_tile_snapshot_refresh(
        false, false, false
    ));
    assert!(!interaction_needs_canvas_tile_snapshot_refresh(
        true, false, true
    ));
    assert!(interaction_needs_canvas_tile_snapshot_refresh(
        true, false, false
    ));
    assert!(!interaction_needs_canvas_tile_snapshot_refresh(
        true, true, true
    ));
}

#[::core::prelude::v1::test]
fn retained_paint_bounds_absorb_small_viewport_movements() {
    let viewport = test_viewport(32.0, 32.0, 512.0, 512.0);
    let mut panned = viewport;
    panned.offset_x -= 8.0;
    panned.offset_y += 8.0;
    let layout = web_relief_render_layout();

    assert_eq!(
        paint_tile_bounds_for_viewport(viewport, layout, RETAIN_RADIUS),
        paint_tile_bounds_for_viewport(panned, layout, RETAIN_RADIUS),
    );
}

#[::core::prelude::v1::test]
fn entity_avatar_keys_accept_namespaced_identifiers() {
    assert_eq!(
        normalize_entity_avatar_key("minecraft:zombie"),
        Some("zombie".to_string())
    );
    assert_eq!(
        normalize_entity_avatar_key("entity.minecraft:glow-squid"),
        Some("glow_squid".to_string())
    );
    assert_eq!(normalize_entity_avatar_key("  "), None);
}

#[::core::prelude::v1::test]
fn cached_visible_tile_is_not_treated_as_a_loading_gap() {
    let mut manager = RegionManager::default();
    manager.mark_loaded((0, 0), test_tile([1, 2, 3, 255]));
    let visible = TileBounds {
        min_x: 0,
        max_x: 0,
        min_z: 0,
        max_z: 0,
    };

    assert!(visible_loaded_tile_missing_from_snapshot(
        &manager,
        &[],
        visible,
    ));

    let cached = manager
        .entries
        .get(&(0, 0))
        .and_then(|entry| entry.image.as_ref())
        .expect("cached tile");
    let snapshot_tile = PaintTile {
        coord: (0, 0),
        image: cached.image.clone(),
        pixel_format: cached.pixel_format,
        width: cached.width,
        height: cached.height,
        estimated_bytes: cached.estimated_bytes,
    };

    assert!(!visible_loaded_tile_missing_from_snapshot(
        &manager,
        &[snapshot_tile],
        visible,
    ));
}

#[::core::prelude::v1::test]
fn interaction_layer_sync_requests_an_immediate_parent_refresh() {
    assert!(should_notify_parent_after_interaction_layer_sync());
}

#[::core::prelude::v1::test]
fn map_tile_upload_budget_prioritizes_viewport_interaction() {
    assert_eq!(map_tile_new_image_budget(true), 16);
    assert_eq!(map_tile_new_image_budget(false), 8);
}

#[::core::prelude::v1::test]
fn paint_order_is_stable_spatial_order() {
    let mut coords = vec![(0, 1), (0, 0), (-1, 0), (-1, -1)];

    coords.sort_by_key(|coord| tile_paint_sort_key(*coord));

    assert_eq!(coords, vec![(-1, -1), (-1, 0), (0, 0), (0, 1)]);
}

#[::core::prelude::v1::test]
fn paint_order_matches_visible_range_traversal() {
    let layout = web_relief_render_layout();
    let viewport = test_viewport(0.0, 0.0, 128.0, 128.0);
    let range = tile_render_range_for_viewport(viewport, layout).expect("render range");
    let mut coords = tile_coords_for_paint_order(range.tile_bounds());
    let expected = coords.clone();

    coords.reverse();
    coords.sort_by_key(|coord| tile_paint_sort_key(*coord));

    assert_eq!(coords, expected);
}

#[::core::prelude::v1::test]
fn metadata_cancel_flag_is_taken_and_cancelled() {
    let mut cancel = Some(RenderTaskControl::new());
    let observed = cancel.as_ref().expect("cancel flag").clone();

    assert!(cancel_metadata_flag(&mut cancel));
    assert!(observed.is_cancelled());
    assert!(cancel.is_none());
    assert!(!cancel_metadata_flag(&mut cancel));
}

#[::core::prelude::v1::test]
fn active_render_tiles_keep_shared_tile_until_last_batch_finishes() {
    let mut active_tiles = ActiveRenderTiles::default();

    track_active_render_tiles(&mut active_tiles, &[(0, 0), (1, 0)]);
    track_active_render_tiles(&mut active_tiles, &[(1, 0), (2, 0)]);
    finish_active_render_tiles(&mut active_tiles, &[(0, 0), (1, 0)]);

    assert!(!active_tiles.contains_key(&(0, 0)));
    assert_eq!(active_tiles.get(&(1, 0)), Some(&1));
    assert_eq!(active_tiles.get(&(2, 0)), Some(&1));

    finish_active_render_tiles(&mut active_tiles, &[(1, 0), (2, 0)]);

    assert!(active_tiles.is_empty());
}

#[::core::prelude::v1::test]
fn tile_event_sender_reports_receiver_drop_without_external_lock() {
    let (sender, mut receiver) = unbounded::<TileRenderEvent>();

    assert!(send_tile_event(
        &sender,
        TileRenderEvent::Empty {
            coord: (0, 0),
            message: "empty".to_string(),
        },
    ));
    assert!(matches!(
        receiver.try_next(),
        Ok(Some(TileRenderEvent::Empty { coord: (0, 0), .. }))
    ));
    drop(receiver);

    assert!(!send_tile_event(
        &sender,
        TileRenderEvent::Empty {
            coord: (1, 0),
            message: "dropped".to_string(),
        },
    ));
}

#[::core::prelude::v1::test]
fn tile_ready_batcher_batches_cache_hits_outside_quick_reveal() {
    let mut batcher = TileReadyBatcher::default();

    for index in 0..TILE_READY_BATCH_LIMIT.saturating_sub(1) {
        assert!(
            batcher
                .push(test_ready_tile(
                    (index as i32, 0),
                    TileReadySource::MemoryCache
                ))
                .is_none()
        );
    }
    let disk_tiles = batcher
        .push(test_ready_tile(
            (TILE_READY_BATCH_LIMIT.saturating_sub(1) as i32, 0),
            TileReadySource::DiskCacheFresh,
        ))
        .expect("cache hits should be flushed as one UI batch");
    assert_eq!(disk_tiles.len(), TILE_READY_BATCH_LIMIT);
    assert!(batcher.pending.is_empty());
}

#[::core::prelude::v1::test]
fn tile_ready_batcher_batches_cache_hits_during_quick_reveal() {
    let mut batcher = TileReadyBatcher::new(true);

    for index in 0..FIRST_REVEAL_READY_BATCH_LIMIT.saturating_sub(1) {
        assert!(
            batcher
                .push(test_ready_tile(
                    (index as i32, 0),
                    TileReadySource::DiskCacheFresh,
                ))
                .is_none()
        );
    }
    let tiles = batcher
        .push(test_ready_tile(
            (FIRST_REVEAL_READY_BATCH_LIMIT.saturating_sub(1) as i32, 0),
            TileReadySource::DiskCacheFresh,
        ))
        .expect("quick reveal cache hits should flush a frame-sized upload batch");
    assert_eq!(tiles.len(), FIRST_REVEAL_READY_BATCH_LIMIT);
    assert!(batcher.pending.is_empty());
}

#[::core::prelude::v1::test]
fn cached_ready_batch_yields_after_quick_reveal_finishes() {
    let event = TileRenderEvent::ReadyBatch {
        tiles: vec![test_ready_tile((0, 0), TileReadySource::MemoryCache)],
    };

    assert!(should_yield_after_ready_batch(false, false, &event));
}

#[::core::prelude::v1::test]
fn rendered_ready_batch_yields_while_interacting_or_quick_revealing() {
    let event = TileRenderEvent::ReadyBatch {
        tiles: vec![test_ready_tile((0, 0), TileReadySource::Render)],
    };

    assert!(!should_yield_after_ready_batch(false, false, &event));
    assert!(should_yield_after_ready_batch(true, false, &event));
    assert!(should_yield_after_ready_batch(false, true, &event));
}

#[::core::prelude::v1::test]
fn ready_tile_events_request_a_window_refresh() {
    let event = TileRenderEvent::ReadyBatch {
        tiles: vec![test_ready_tile((0, 0), TileReadySource::Render)],
    };

    assert!(tile_event_needs_window_refresh(&event));
}

#[::core::prelude::v1::test]
fn tile_ready_batcher_buffers_render_tiles_until_flush() {
    let mut batcher = TileReadyBatcher::default();

    assert!(
        batcher
            .push(test_ready_tile((1, 0), TileReadySource::Render))
            .is_none()
    );
    assert_eq!(batcher.pending.len(), 1);

    let tiles = batcher
        .flush()
        .expect("manual flush should return pending tile");

    assert_eq!(tiles.len(), 1);
    assert_eq!(tiles[0].coord, (1, 0));
    assert!(batcher.pending.is_empty());
}

#[::core::prelude::v1::test]
fn tile_ready_batcher_flushes_render_tiles_during_quick_reveal() {
    let mut batcher = TileReadyBatcher::new(true);

    assert!(
        batcher
            .push(test_ready_tile((1, 0), TileReadySource::Render))
            .is_none()
    );
    let tiles = batcher
        .flush()
        .expect("completion should flush the first tile");
    assert_eq!(tiles.len(), 1);
}

#[::core::prelude::v1::test]
fn tile_ready_batcher_flushes_tiles_in_center_ring_order() {
    let mut batcher = TileReadyBatcher::with_center(false, (0, 0));
    batcher.push(test_ready_tile((1, 1), TileReadySource::Render));
    batcher.push(test_ready_tile((0, -1), TileReadySource::Render));
    batcher.push(test_ready_tile((0, 0), TileReadySource::Render));
    batcher.push(test_ready_tile((1, 0), TileReadySource::Render));

    let tiles = batcher.flush().expect("pending tiles should flush");
    assert_eq!(
        tiles.into_iter().map(|tile| tile.coord).collect::<Vec<_>>(),
        vec![(0, 0), (0, -1), (1, 0), (1, 1)]
    );
}

#[::core::prelude::v1::test]
fn tile_chunk_region_uses_eight_by_eight_tile_bounds() {
    let layout = web_relief_render_layout();
    assert_eq!(layout.chunks_per_tile, 8);
    assert_eq!(layout.blocks_per_pixel, 1);
    assert_eq!(layout.pixels_per_block, 4);
    assert_eq!(layout.tile_size(), Some(512));

    let region =
        tile_chunk_region(Dimension::Overworld, 1, -1, layout).expect("8x8 tile chunk bounds");

    assert_eq!(region.min_chunk_x, 8);
    assert_eq!(region.max_chunk_x, 15);
    assert_eq!(region.min_chunk_z, -8);
    assert_eq!(region.max_chunk_z, -1);
}

#[::core::prelude::v1::test]
fn selected_tile_work_estimate_counts_chunks_and_unique_regions() {
    let mut tile_chunk_index = TileChunkIndex::new();
    tile_chunk_index.insert(
        (0, 0),
        TileChunkPositions::from(vec![
            ChunkPos {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
            ChunkPos {
                x: 1,
                z: 1,
                dimension: Dimension::Overworld,
            },
        ]),
    );
    tile_chunk_index.insert(
        (4, 0),
        TileChunkPositions::from(vec![
            ChunkPos {
                x: 32,
                z: 0,
                dimension: Dimension::Overworld,
            },
            ChunkPos {
                x: 33,
                z: 1,
                dimension: Dimension::Overworld,
            },
        ]),
    );

    let estimate = selected_tile_work_estimate(&[(0, 0), (4, 0), (8, 8)], &tile_chunk_index);

    assert_eq!(
        estimate,
        SelectedTileWorkEstimate {
            chunk_count: 4,
            region_count: 2
        }
    );
}

#[::core::prelude::v1::test]
fn region_cache_identity_uses_eight_chunk_tile_layout() {
    let layout = web_relief_render_layout();
    let world_path = PathBuf::from("test-world");
    let manifest_path = bedrock_render::tile_manifest_cache_path(
        &file_ops::cache_subdir("bedrock-render"),
        &world_path,
        RenderBackend::Cpu,
        RenderGpuBackend::Auto,
        RenderMode::SurfaceBlocks,
        Dimension::Overworld,
        layout,
    );
    let manifest_path = manifest_path.to_string_lossy();

    assert_eq!(layout.chunks_per_tile, 8);
    assert_eq!(layout.blocks_per_pixel, 1);
    assert_eq!(layout.pixels_per_block, 4);
    assert!(manifest_path.contains("map-manifest-index"));
    assert!(manifest_path.contains("8c-1bpp-4ppb.bridx"));
    assert!(!manifest_path.contains("32c-1bpp-4ppb"));
}

#[::core::prelude::v1::test]
fn non_empty_tile_index_uses_exact_manifest_chunk_set() {
    let layout = web_relief_render_layout();
    let indexed_positions = vec![ChunkPos {
        x: 8,
        z: -8,
        dimension: Dimension::Overworld,
    }];

    let render_positions = ui_tile_chunk_positions_for_render(
        Dimension::Overworld,
        1,
        -1,
        layout,
        Some(indexed_positions.as_slice()),
    )
    .expect("render positions")
    .expect("known non-empty tile");
    assert_eq!(render_positions.len(), 1);
    assert!(render_positions.contains(&ChunkPos {
        x: 8,
        z: -8,
        dimension: Dimension::Overworld,
    }));
}

#[::core::prelude::v1::test]
fn indexed_tile_chunks_are_normalized_before_render() {
    let layout = web_relief_render_layout();
    let valid_a = ChunkPos {
        x: 8,
        z: -8,
        dimension: Dimension::Overworld,
    };
    let valid_b = ChunkPos {
        x: 9,
        z: -8,
        dimension: Dimension::Overworld,
    };
    let indexed_positions = vec![
        valid_b,
        ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        },
        valid_a,
        valid_b,
        ChunkPos {
            x: 8,
            z: -8,
            dimension: Dimension::Nether,
        },
    ];

    let render_positions = ui_tile_chunk_positions_for_render(
        Dimension::Overworld,
        1,
        -1,
        layout,
        Some(indexed_positions.as_slice()),
    )
    .expect("render positions")
    .expect("known non-empty tile");
    let mut expected = vec![valid_a, valid_b];
    expected.sort_unstable();
    assert_eq!(render_positions, expected);
}

#[::core::prelude::v1::test]
fn shared_tile_chunk_index_normalizes_cached_positions() {
    let layout = web_relief_render_layout();
    let valid_a = ChunkPos {
        x: 8,
        z: -8,
        dimension: Dimension::Overworld,
    };
    let valid_b = ChunkPos {
        x: 9,
        z: -8,
        dimension: Dimension::Overworld,
    };
    let mut cached_index = BTreeMap::new();
    cached_index.insert(
        (1, -1),
        vec![
            valid_b,
            valid_a,
            valid_b,
            ChunkPos {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
            ChunkPos {
                x: 8,
                z: -8,
                dimension: Dimension::Nether,
            },
        ],
    );

    let normalized = shared_tile_chunk_index(Dimension::Overworld, layout, cached_index)
        .expect("normalized tile index");

    let mut expected = vec![valid_a, valid_b];
    expected.sort_unstable();
    assert_eq!(
        normalized.get(&(1, -1)).map(|positions| positions.as_ref()),
        Some(expected.as_slice())
    );
}

#[::core::prelude::v1::test]
fn empty_tile_index_remains_empty_for_negative_cache() {
    let layout = web_relief_render_layout();
    let indexed_positions = Vec::new();

    let render_positions = ui_tile_chunk_positions_for_render(
        Dimension::Overworld,
        1,
        -1,
        layout,
        Some(indexed_positions.as_slice()),
    )
    .expect("render positions")
    .expect("known empty tile");
    assert!(render_positions.is_empty());
}

#[::core::prelude::v1::test]
fn missing_tile_index_uses_unculled_cpu_render_path() {
    let layout = web_relief_render_layout();

    let render_positions =
        ui_tile_chunk_positions_for_render(Dimension::Overworld, 1, -1, layout, None)
            .expect("unknown tile index should render without pre-cull");

    assert!(render_positions.is_none());
}

#[::core::prelude::v1::test]
fn pending_manifest_tiles_are_not_render_queue_candidates() {
    let mut manager = RegionManager::default();
    manager.ensure_pending_manifest(&[(0, 0)], TilePriority::Visible);
    manager.ensure_tiles(&[(1, 0)], TilePriority::Visible);

    let queued = manager.queued_coords((0, 0), None, false, true);

    assert_eq!(manager.pending_manifest_count(), 1);
    assert_eq!(queued, vec![(1, 0)]);
}

#[::core::prelude::v1::test]
fn pending_manifest_detection_is_limited_to_requested_tiles() {
    let mut manager = RegionManager::default();
    manager.ensure_pending_manifest(&[(0, 0), (8, 0)], TilePriority::Prefetch);

    assert!(manager.has_pending_manifest_for_tiles(&[(0, 0)]));
    assert!(!manager.has_pending_manifest_for_tiles(&[(1, 0)]));
}

#[::core::prelude::v1::test]
fn cancelled_probe_keeps_pending_manifest_tile_probeable() {
    let mut manager = RegionManager::default();
    manager.ensure_pending_manifest(&[(0, 0)], TilePriority::Visible);

    assert!(manager.is_pending_manifest((0, 0)));
    assert!(manager.has_pending_manifest_for_tiles(&[(0, 0)]));
    assert!(should_probe_manifest_tiles(
        false, false, false, true, false, false
    ));
}

#[::core::prelude::v1::test]
fn manifest_ready_tile_enters_render_queue_after_probe() {
    let mut manager = RegionManager::default();
    manager.ensure_pending_manifest(&[(0, 0)], TilePriority::Visible);
    manager.mark_manifest_ready((0, 0), TilePriority::Visible);

    assert_eq!(
        manager.queued_coords((0, 0), None, false, true),
        vec![(0, 0)]
    );
}

#[::core::prelude::v1::test]
fn edit_refresh_pending_manifest_keeps_refresh_priority_after_probe() {
    let mut manager = RegionManager::default();
    manager.ensure_pending_manifest(&[(3, -2)], TilePriority::EditRefresh);
    manager.mark_manifest_ready((3, -2), TilePriority::EditRefresh);

    assert_eq!(
        manager.entries.get(&(3, -2)).map(|entry| entry.priority),
        Some(TilePriority::EditRefresh)
    );
    assert_eq!(
        manager.queued_coords((0, 0), None, false, true),
        vec![(3, -2)]
    );
}

#[::core::prelude::v1::test]
fn mark_loaded_preserves_visible_priority_after_ready_return() {
    let mut manager = RegionManager::default();
    let coord = (4, -3);
    manager.ensure_tiles(&[coord], TilePriority::Visible);

    manager.mark_loaded(coord, test_tile([1, 2, 3, 255]));

    let entry = manager.entries.get(&coord).expect("loaded tile");
    assert_eq!(entry.state, TileLoadState::Loaded);
    assert_eq!(entry.priority, TilePriority::Visible);
}

#[::core::prelude::v1::test]
fn force_refresh_tiles_requeues_loaded_tile_and_releases_memory() {
    let mut manager = RegionManager::default();
    let coord = (2, -1);
    manager.ensure_tiles(&[coord], TilePriority::Visible);
    let tile = test_tile([1, 2, 3, 255]);
    let image = tile.image.clone();
    manager.mark_loaded(coord, tile);

    let dropped_images = manager.force_refresh_tiles(&[coord], TilePriority::EditRefresh);

    let entry = manager.entries.get(&coord).expect("forced refresh tile");
    assert_eq!(entry.state, TileLoadState::Queued);
    assert_eq!(entry.priority, TilePriority::EditRefresh);
    assert!(entry.image.is_none());
    assert_eq!(manager.loaded_estimated_bytes, 0);
    assert_eq!(dropped_images.len(), 1);
    assert!(Arc::ptr_eq(&dropped_images[0], &image));
}

#[::core::prelude::v1::test]
fn clear_removes_loaded_tiles_and_releases_estimated_memory() {
    let mut manager = RegionManager::default();
    let first = test_tile([1, 2, 3, 255]);
    let first_image = first.image.clone();
    let second = test_tile([4, 5, 6, 255]);
    let second_image = second.image.clone();
    manager.mark_loaded((0, 0), first);
    manager.mark_loaded((1, 0), second);

    let dropped_images = manager.clear();

    assert!(manager.entries.is_empty());
    assert_eq!(manager.loaded_estimated_bytes(), 0);
    assert_eq!(dropped_images.len(), 2);
    assert!(
        dropped_images
            .iter()
            .any(|image| Arc::ptr_eq(image, &first_image))
    );
    assert!(
        dropped_images
            .iter()
            .any(|image| Arc::ptr_eq(image, &second_image))
    );
}

#[::core::prelude::v1::test]
fn mark_invalid_removes_loaded_tile_and_releases_estimated_memory() {
    let mut manager = RegionManager::default();
    let tile = test_tile([1, 2, 3, 255]);
    let image = tile.image.clone();
    manager.mark_loaded((0, 0), tile);

    let dropped_image =
        manager.mark_invalid((0, 0), SharedString::from("索引确认该瓦片没有可渲染区块"));

    assert!(
        manager
            .entries
            .get(&(0, 0))
            .is_some_and(|entry| entry.image.is_none())
    );
    assert_eq!(manager.loaded_estimated_bytes(), 0);
    assert!(dropped_image.is_some_and(|dropped_image| Arc::ptr_eq(&dropped_image, &image)));
}

#[::core::prelude::v1::test]
fn region_manager_counts_track_state_transitions() {
    let mut manager = RegionManager::default();
    manager.ensure_tiles(&[(0, 0), (1, 0)], TilePriority::Visible);

    assert_eq!(manager.queued_count(), 2);
    assert_eq!(manager.loading_count(), 0);
    assert_eq!(manager.loaded_count(), 0);

    manager.mark_loading(&[(0, 0)]);
    assert_eq!(manager.queued_count(), 1);
    assert_eq!(manager.loading_count(), 1);

    manager.mark_failed((0, 0), SharedString::from("failed"));
    assert_eq!(manager.queued_count(), 2);
    assert_eq!(manager.loading_count(), 0);
    assert_eq!(manager.failed_count(), 1);

    manager.mark_invalid((1, 0), SharedString::from("empty"));
    assert_eq!(manager.queued_count(), 1);
    assert_eq!(manager.failed_count(), 1);
    assert_eq!(manager.invalid_count(), 1);

    manager.ensure_pending_manifest(&[(0, 0)], TilePriority::Visible);
    assert_eq!(manager.queued_count(), 0);
    assert_eq!(manager.pending_manifest_count(), 1);
    assert_eq!(manager.failed_count(), 0);

    manager.mark_manifest_ready((0, 0), TilePriority::Visible);
    assert_eq!(manager.queued_count(), 1);
    assert_eq!(manager.pending_manifest_count(), 0);

    manager.mark_loaded((0, 0), test_tile([1, 2, 3, 255]));
    assert_eq!(manager.loaded_count(), 1);
    assert_eq!(manager.queued_count(), 0);

    manager.remove_tile((0, 0));
    assert_eq!(manager.loaded_count(), 0);
    assert_eq!(manager.invalid_count(), 1);
}

#[::core::prelude::v1::test]
fn cancelled_loading_tiles_return_to_queue_without_backoff() {
    let mut manager = RegionManager::default();
    manager.ensure_tiles(&[(0, 0)], TilePriority::Visible);
    manager.mark_loading(&[(0, 0)]);

    manager.requeue_cancelled_loading(&[(0, 0)]);

    let entry = manager.entries.get(&(0, 0)).expect("queued tile");
    assert_eq!(entry.state, TileLoadState::Queued);
    assert_eq!(entry.priority, TilePriority::Visible);
    assert_eq!(entry.retry_after, None);
    assert_eq!(manager.loading_count(), 0);
    assert_eq!(manager.queued_count(), 1);
}

#[::core::prelude::v1::test]
fn cancelling_active_render_requeues_tiles_before_task_finishes() {
    let mut manager = RegionManager::default();
    let mut active_tiles = ActiveRenderTiles::default();
    let coords = [(0, 0), (1, 0)];
    manager.ensure_tiles(&coords, TilePriority::Visible);
    manager.mark_loading(&coords);
    track_active_render_tiles(&mut active_tiles, &coords);

    requeue_active_render_tiles_after_cancel(&mut manager, &mut active_tiles);

    assert_eq!(manager.queued_count(), coords.len());
    assert_eq!(manager.loading_count(), 0);
    assert!(active_tiles.is_empty());
}

#[::core::prelude::v1::test]
fn pending_edit_refresh_manifest_coords_keep_queue_order() {
    let mut manager = RegionManager::default();
    manager.ensure_pending_manifest(&[(5, 0)], TilePriority::Visible);
    manager.ensure_pending_manifest(&[(2, 0)], TilePriority::EditRefresh);
    manager.ensure_pending_manifest(&[(1, 0)], TilePriority::EditRefresh);

    assert_eq!(
        manager.pending_manifest_coords_with_priority(TilePriority::EditRefresh),
        vec![(2, 0), (1, 0)]
    );
}

#[::core::prelude::v1::test]
fn invalid_empty_manifest_tile_is_not_render_queue_candidate() {
    let mut manager = RegionManager::default();
    manager.ensure_pending_manifest(&[(0, 0)], TilePriority::Visible);
    manager.mark_invalid((0, 0), SharedString::from("索引确认该瓦片没有可渲染区块"));

    assert!(manager.queued_coords((0, 0), None, false, true).is_empty());
}

#[::core::prelude::v1::test]
fn render_tile_plan_rejects_empty_chunk_positions() {
    let plan = RenderTilePlan::new(
        Dimension::Overworld,
        RenderMode::SurfaceBlocks,
        web_relief_render_layout(),
        (0, 0),
        TileChunkPositions::from(Vec::new()),
    );

    assert!(plan.is_err());
}

#[::core::prelude::v1::test]
fn render_tile_plan_allows_missing_chunk_index_for_unculled_render() {
    let plan = RenderTilePlan::from_optional_chunk_positions(
        Dimension::Overworld,
        RenderMode::SurfaceBlocks,
        web_relief_render_layout(),
        (0, 0),
        None,
    )
    .expect("missing tile index should render through renderer-side culling");

    assert_eq!(plan.coord, (0, 0));
    assert!(plan.planned.chunk_positions.is_none());
}

#[::core::prelude::v1::test]
fn render_tile_plan_keeps_only_indexed_tile_chunks() {
    let layout = web_relief_render_layout();
    let plan = RenderTilePlan::new(
        Dimension::Overworld,
        RenderMode::SurfaceBlocks,
        layout,
        (1, -1),
        TileChunkPositions::from(vec![
            ChunkPos {
                x: 8,
                z: -8,
                dimension: Dimension::Overworld,
            },
            ChunkPos {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
        ]),
    )
    .expect("non-empty render tile plan");

    assert_eq!(plan.coord, (1, -1));
    assert_eq!(
        plan.planned.chunk_positions.as_deref(),
        Some(
            [ChunkPos {
                x: 8,
                z: -8,
                dimension: Dimension::Overworld,
            }]
            .as_slice()
        )
    );
}

#[::core::prelude::v1::test]
fn render_tile_plan_reuses_normalized_chunk_positions() {
    let layout = web_relief_render_layout();
    let chunk_positions = TileChunkPositions::from(vec![
        ChunkPos {
            x: 8,
            z: -8,
            dimension: Dimension::Overworld,
        },
        ChunkPos {
            x: 9,
            z: -8,
            dimension: Dimension::Overworld,
        },
    ]);

    let plan = RenderTilePlan::new(
        Dimension::Overworld,
        RenderMode::SurfaceBlocks,
        layout,
        (1, -1),
        Arc::clone(&chunk_positions),
    )
    .expect("non-empty render tile plan");

    let planned_positions = plan
        .planned
        .chunk_positions
        .as_ref()
        .expect("planned chunk positions");
    assert!(Arc::ptr_eq(&chunk_positions, planned_positions));
}

#[::core::prelude::v1::test]
fn interactive_session_config_culls_missing_chunks() {
    let world_path = std::path::Path::new("world");
    let config = interactive_map_render_session_config(
        world_path,
        RenderBackend::Auto,
        RenderGpuBackend::Auto,
    );

    assert!(config.cull_missing_chunks);
    assert_eq!(
        config.world_signature,
        bedrock_render::world_cache_signature(world_path)
    );
    assert_eq!(
        config.region_bake_cache_memory_limit,
        RENDER_REGION_CACHE_ENTRIES
    );
}

#[::core::prelude::v1::test]
fn interactive_tile_batch_defaults_are_conservative() {
    assert_eq!(RENDER_UI_BATCH_TILES, 24);
    assert_eq!(FIRST_VISIBLE_BATCH_LIMIT, 4);
    assert_eq!(FIRST_REVEAL_READY_BATCH_LIMIT, 4);
    assert_eq!(FIRST_REVEAL_READY_BATCH_INTERVAL, Duration::from_millis(16));
    assert_eq!(QUICK_REVEAL_TILE_FRAME_INTERVAL, Duration::from_millis(8));
    assert_eq!(DRAG_VISIBLE_BATCH_LIMIT, 16);
    assert_eq!(VISIBLE_TILE_FOREGROUND_WORK_LIMIT, 512);
    assert_eq!(INTERACTION_VISIBLE_TILE_FOREGROUND_WORK_LIMIT, 48);
    assert_eq!(VIEWPORT_WORK_REFRESH_INTERVAL, Duration::from_millis(16));
    assert_eq!(RENDER_STREAM_GROUP_TILES, 4);
    assert_eq!(MAX_CONCURRENT_RENDER_BATCHES, 2);
    assert_eq!(
        resolve_interactive_tile_batch_size(RenderBackend::Auto, RenderCpuBudget::default(), 8),
        RenderCpuBudget::default().tile_batch_size().min(8)
    );
    assert!(
        RenderCpuBudget::default()
            .render_cpu_pipeline(16)
            .chunk_batch_size
            <= 4
    );
    let cpu_budget = RenderCpuBudget::default();
    let pipeline = cpu_budget.render_cpu_pipeline(16);
    let worker_count = cpu_budget.thread_count().min(16);
    assert_eq!(pipeline.max_db_workers, worker_count);
    assert_eq!(pipeline.max_bake_workers, worker_count);
    assert_eq!(pipeline.max_compose_workers, worker_count);
    assert_eq!(pipeline.max_in_flight_regions, worker_count);
}

#[::core::prelude::v1::test]
fn visible_tile_foreground_work_limit_reduces_work_during_interaction() {
    assert_eq!(
        visible_tile_foreground_work_limit(false),
        VISIBLE_TILE_FOREGROUND_WORK_LIMIT
    );
    assert_eq!(
        visible_tile_foreground_work_limit(true),
        INTERACTION_VISIBLE_TILE_FOREGROUND_WORK_LIMIT
    );
    assert!(visible_tile_foreground_work_limit(false) > visible_tile_foreground_work_limit(true));
}

#[::core::prelude::v1::test]
fn drag_manifest_probe_prioritizes_unknown_visible_tiles() {
    assert!(drag_manifest_probe_needed(1, false));
    assert!(!drag_manifest_probe_needed(0, false));
    assert!(!drag_manifest_probe_needed(1, true));
}

#[::core::prelude::v1::test]
fn render_image_eviction_waits_until_viewport_interaction_is_idle() {
    assert!(should_defer_render_image_evictions(true));
    assert!(!should_defer_render_image_evictions(false));
}

#[::core::prelude::v1::test]
fn loaded_tile_without_manifest_keeps_cached_image_without_requeue() {
    let mut manager = RegionManager::default();
    manager.ensure_tiles(&[(0, 0)], TilePriority::Visible);
    manager.mark_loaded((0, 0), test_tile([1, 2, 3, 255]));

    let needs_cache_bypass = manager.ensure_pending_manifest(&[(0, 0)], TilePriority::Visible);

    let entry = manager
        .entries
        .get(&(0, 0))
        .expect("loaded tile should remain available while verification runs");
    assert!(!needs_cache_bypass);
    assert_eq!(entry.state, TileLoadState::Loaded);
    assert_eq!(entry.source_status, TileSourceStatus::Fresh);
    assert!(entry.image.is_some());
}

#[::core::prelude::v1::test]
fn loaded_tile_without_image_is_requeued_for_visible_render() {
    let mut manager = RegionManager::default();
    manager.ensure_tiles(&[(0, 0)], TilePriority::Visible);
    manager.mark_loaded((0, 0), test_tile([1, 2, 3, 255]));
    manager
        .entries
        .get_mut(&(0, 0))
        .expect("loaded tile entry")
        .image = None;

    manager.ensure_tiles(&[(0, 0)], TilePriority::Visible);

    let entry = manager.entries.get(&(0, 0)).expect("requeued tile entry");
    assert_eq!(entry.state, TileLoadState::Queued);
    assert_eq!(entry.source_status, TileSourceStatus::Miss);
    assert_eq!(
        manager.queued_coords((0, 0), None, false, true),
        vec![(0, 0)]
    );
}

#[::core::prelude::v1::test]
fn stale_render_batch_yields_to_the_new_viewport_center() {
    let visible_bounds = TileBounds {
        min_x: 8,
        max_x: 12,
        min_z: 8,
        max_z: 12,
    };

    assert!(render_batch_matches_current_viewport(
        (10, 10),
        (10, 10),
        visible_bounds
    ));
    assert!(!render_batch_matches_current_viewport(
        (9, 10),
        (10, 10),
        visible_bounds
    ));
    assert!(!render_batch_matches_current_viewport(
        (3, 3),
        (10, 10),
        visible_bounds
    ));
}

#[::core::prelude::v1::test]
fn tile_request_plan_keeps_intersecting_batches_and_drops_far_batches() {
    let visible_bounds = TileBounds {
        min_x: 0,
        max_x: 2,
        min_z: 0,
        max_z: 2,
    };
    let retained = RetainedTileFilter::new(visible_bounds, visible_bounds.expand(1), 1);

    assert!(tile_request_intersects_plan(
        &[(5, 5), (3, 2)],
        Some(visible_bounds),
        Some(retained),
    ));
    assert!(!tile_request_intersects_plan(
        &[(5, 5), (6, 6)],
        Some(visible_bounds),
        Some(retained),
    ));
}

#[::core::prelude::v1::test]
fn visible_render_batch_size_expands_for_large_overviews() {
    assert_eq!(
        visible_render_batch_size(8, OVERVIEW_VISIBLE_TILE_THRESHOLD - 1, false, true),
        8
    );
    assert_eq!(
        visible_render_batch_size(8, OVERVIEW_VISIBLE_TILE_THRESHOLD, false, true),
        OVERVIEW_VISIBLE_BATCH_LIMIT
    );
    assert_eq!(
        visible_render_batch_size(8, OVERVIEW_VISIBLE_TILE_THRESHOLD, false, false),
        OVERVIEW_VISIBLE_BATCH_LIMIT
    );
    assert_eq!(
        visible_render_batch_size(8, OVERVIEW_VISIBLE_TILE_THRESHOLD, true, false),
        DRAG_VISIBLE_BATCH_LIMIT
    );
}

#[::core::prelude::v1::test]
fn drag_render_batch_size_is_not_limited_by_first_reveal() {
    assert_eq!(
        visible_render_batch_size(8, OVERVIEW_VISIBLE_TILE_THRESHOLD - 1, true, false),
        8
    );
    assert_eq!(
        visible_render_batch_size(32, OVERVIEW_VISIBLE_TILE_THRESHOLD - 1, true, false),
        DRAG_VISIBLE_BATCH_LIMIT
    );
}

#[::core::prelude::v1::test]
fn map_cpu_budget_defaults_to_sixty_percent_with_interactive_cap() {
    let budget = RenderCpuBudget::default();
    assert_eq!(budget.percent(), 60);
    let threads = budget.thread_count();
    assert!(threads >= 1);
    let available = RenderCpuBudget::available_threads();
    let requested = available
        .saturating_mul(usize::from(budget.percent()))
        .saturating_add(99)
        / 100;
    assert_eq!(
        threads,
        requested.clamp(1, available.saturating_sub(1).max(1))
    );
}

#[::core::prelude::v1::test]
fn manifest_probe_worker_count_keeps_interaction_headroom() {
    let budget = RenderCpuBudget::default();

    let workers = manifest_probe_worker_count(budget);

    assert!(workers >= 1);
    assert!(workers <= TILE_MANIFEST_PROBE_MAX_WORKERS);
    assert!(workers <= budget.thread_count());
}

#[::core::prelude::v1::test]
fn circular_prefetch_keeps_axis_neighbors_and_excludes_outer_corners() {
    let coords = collect_circular_tile_coords(
        TileBounds {
            min_x: 0,
            max_x: 0,
            min_z: 0,
            max_z: 0,
        },
        TileBounds {
            min_x: -1,
            max_x: 1,
            min_z: -1,
            max_z: 1,
        },
        1,
        (0, 0),
    );
    let coords = coords.into_iter().collect::<BTreeSet<_>>();

    assert!(coords.contains(&(0, 0)));
    assert!(coords.contains(&(-1, 0)));
    assert!(coords.contains(&(1, 0)));
    assert!(coords.contains(&(0, -1)));
    assert!(coords.contains(&(0, 1)));
    assert!(!coords.contains(&(-1, -1)));
    assert!(!coords.contains(&(1, 1)));
}

#[::core::prelude::v1::test]
fn circular_tile_coords_are_generated_center_first() {
    let visible = TileBounds {
        min_x: -1,
        max_x: 1,
        min_z: -1,
        max_z: 1,
    };
    let expanded = visible.expand(2);
    let coords = collect_circular_tile_coords(visible, expanded, 2, (0, 0));
    let mut sorted = coords.clone();
    sort_tiles_center_first(&mut sorted, (0, 0));

    assert_eq!(coords, sorted);
    assert!(tiles_are_sorted_center_first(&coords, (0, 0)));
}

#[::core::prelude::v1::test]
fn visible_tile_coords_keep_center_ring_order_when_center_is_outside_bounds() {
    let visible = TileBounds {
        min_x: 4,
        max_x: 7,
        min_z: -2,
        max_z: 1,
    };
    let center = (0, 0);
    let coords = tile_coords_for_visible_bounds(visible, center);

    assert_eq!(coords.len(), 16);
    assert!(tiles_are_sorted_center_first(&coords, center));
}

#[::core::prelude::v1::test]
fn circular_tile_coords_rotate_clockwise_one_ring_at_a_time() {
    let bounds = TileBounds {
        min_x: -2,
        max_x: 2,
        min_z: -2,
        max_z: 2,
    };
    let coords = collect_circular_tile_coords(bounds, bounds, 0, (0, 0));

    assert_eq!(
        &coords[..9],
        &[
            (0, 0),
            (0, -1),
            (1, -1),
            (1, 0),
            (1, 1),
            (0, 1),
            (-1, 1),
            (-1, 0),
            (-1, -1),
        ]
    );
    assert_eq!(coords.len(), 25);
    assert_eq!(coords.iter().collect::<BTreeSet<_>>().len(), coords.len());
}

#[::core::prelude::v1::test]
fn center_first_order_detection_rejects_unsorted_tiles() {
    assert!(tiles_are_sorted_center_first(
        &[(0, 0), (0, -1), (1, -1), (1, 0)],
        (0, 0)
    ));
    assert!(!tiles_are_sorted_center_first(
        &[(0, 0), (0, -1), (1, 0), (1, -1)],
        (0, 0)
    ));
}

#[::core::prelude::v1::test]
fn queued_tiles_order_center_and_axis_neighbors_before_diagonal_tiles() {
    let mut manager = RegionManager::default();
    manager.ensure_tiles(
        &[(2, 0), (1, 1), (0, 1), (-1, 0), (0, 0), (1, 0), (0, -1)],
        TilePriority::Visible,
    );

    let queued = manager.queued_coords((0, 0), None, false, true);
    let diagonal_position = queued
        .iter()
        .position(|coord| *coord == (1, 1))
        .expect("diagonal tile");

    assert_eq!(queued.first(), Some(&(0, 0)));
    for axis_neighbor in [(0, -1), (-1, 0), (1, 0), (0, 1)] {
        let position = queued
            .iter()
            .position(|coord| *coord == axis_neighbor)
            .expect("axis neighbor tile");
        assert!(position < diagonal_position);
    }
}

#[::core::prelude::v1::test]
fn queued_tiles_keep_center_priority_over_later_sequence_tiles() {
    let mut manager = RegionManager::default();
    manager.ensure_tiles(&[(2, 0), (1, 0), (0, 0)], TilePriority::Visible);

    let queued = manager.queued_coords((0, 0), None, false, true);

    assert_eq!(queued.first(), Some(&(0, 0)));
    assert!(
        queued
            .iter()
            .position(|coord| *coord == (1, 0))
            .expect("near tile")
            < queued
                .iter()
                .position(|coord| *coord == (2, 0))
                .expect("far tile")
    );
}

#[::core::prelude::v1::test]
fn queued_tiles_can_preserve_sequence_when_center_priority_is_disabled() {
    let mut manager = RegionManager::default();
    manager.ensure_tiles(&[(2, 0), (1, 0), (0, 0)], TilePriority::Visible);

    let queued = manager.queued_coords((0, 0), None, false, false);

    assert_eq!(queued, vec![(2, 0), (1, 0), (0, 0)]);
}

#[::core::prelude::v1::test]
fn queued_tiles_limited_matches_full_queue_prefix() {
    let mut manager = RegionManager::default();
    let bounds = TileBounds {
        min_x: -3,
        max_x: 3,
        min_z: -3,
        max_z: 3,
    };
    let visible_tiles = tile_coords_from_bounds(bounds);
    manager.ensure_tiles(&visible_tiles, TilePriority::Visible);

    let full_queue = manager.queued_coords((0, 0), Some(bounds), false, true);
    let limited_queue = manager.queued_coords_limited((0, 0), Some(bounds), false, true, 12);

    assert_eq!(limited_queue, full_queue[..12]);
}

#[::core::prelude::v1::test]
fn queued_tiles_limited_drops_prefetch_candidates_when_visible_tile_appears_later() {
    let mut manager = RegionManager::default();
    manager.ensure_tiles(&[(-10, 0)], TilePriority::Prefetch);
    manager.ensure_tiles(&[(10, 0)], TilePriority::Visible);

    let queued = manager.queued_coords_limited((0, 0), None, true, true, 8);

    assert_eq!(queued, vec![(10, 0)]);
}

#[::core::prelude::v1::test]
fn drag_visible_queue_only_considers_current_visible_tiles() {
    let mut manager = RegionManager::default();
    manager.ensure_tiles(&[(-100, 0), (100, 0)], TilePriority::Visible);
    manager.ensure_tiles(&[(2, 0), (0, 0), (1, 0)], TilePriority::Visible);

    let queued = manager.queued_visible_coords_limited(&[(2, 0), (0, 0), (1, 0)], (0, 0), 2);

    assert_eq!(queued, vec![(0, 0), (1, 0)]);
}

#[::core::prelude::v1::test]
fn drag_visible_queue_keeps_edit_refresh_before_visible_tiles() {
    let mut manager = RegionManager::default();
    manager.ensure_tiles(&[(0, 0), (1, 0)], TilePriority::Visible);
    manager.ensure_tiles(&[(1, 0)], TilePriority::EditRefresh);

    let queued = manager.queued_visible_coords_limited(&[(0, 0), (1, 0)], (0, 0), 2);

    assert_eq!(queued, vec![(1, 0), (0, 0)]);
}

#[::core::prelude::v1::test]
fn tile_order_uses_center_ring_sort_key() {
    let mut coords = vec![(2, 0), (1, 1), (0, 1), (0, 0), (1, 0)];
    sort_tiles_center_first(&mut coords, (0, 0));

    assert_eq!(coords, vec![(0, 0), (1, 0), (1, 1), (0, 1), (2, 0)]);
}

#[::core::prelude::v1::test]
fn edit_refresh_tiles_are_queued_before_visible_tiles() {
    let mut manager = RegionManager::default();
    manager.ensure_tiles(&[(0, 0), (1, 0)], TilePriority::Visible);
    manager.ensure_tiles(&[(2, 0)], TilePriority::EditRefresh);
    manager.ensure_tiles(&[(2, 0)], TilePriority::Visible);

    let queued = manager.queued_coords((0, 0), None, false, true);

    assert_eq!(queued, vec![(2, 0)]);
    assert_eq!(
        manager.entries.get(&(2, 0)).map(|entry| entry.priority),
        Some(TilePriority::EditRefresh)
    );
}

#[::core::prelude::v1::test]
fn manifest_probe_selects_center_ring_before_outer_visible_tiles() {
    let visible_tiles = [(2, 0), (1, 1), (0, 1), (-1, 0), (0, 0), (1, 0), (0, -1)];
    let selected = select_manifest_probe_tiles(&visible_tiles, &[], (0, 0), &BTreeSet::new());

    assert_eq!(
        selected,
        vec![(0, 0), (0, -1), (1, 0), (1, 1), (0, 1), (-1, 0), (2, 0)]
    );
}

#[::core::prelude::v1::test]
fn manifest_probe_skips_scanned_center_and_batches_remaining_visible_tiles() {
    let visible_tiles = [(2, 0), (1, 1), (0, 1), (-1, 0), (0, 0), (1, 0), (0, -1)];
    let mut scanned_tiles = BTreeSet::new();
    scanned_tiles.insert((0, 0));

    let selected = select_manifest_probe_tiles(&visible_tiles, &[], (0, 0), &scanned_tiles);

    assert_eq!(
        selected,
        vec![(0, -1), (1, 0), (1, 1), (0, 1), (-1, 0), (2, 0)]
    );
}

#[::core::prelude::v1::test]
fn manifest_probe_prioritizes_visible_pending_even_with_render_work() {
    assert!(should_probe_manifest_tiles(
        false, false, false, true, false, true
    ));
    assert!(!should_probe_manifest_tiles(
        false, false, false, false, true, true
    ));
    assert!(should_probe_manifest_tiles(
        false, false, false, false, true, false
    ));
    assert!(should_probe_manifest_tiles(
        false, false, true, false, false, true
    ));
    assert!(should_probe_manifest_tiles(
        true, false, true, true, true, false
    ));
    assert!(!should_probe_manifest_tiles(
        false, true, true, true, true, false
    ));
}

#[::core::prelude::v1::test]
fn manifest_probe_allows_first_screen_visible_work_during_metadata_load() {
    assert!(should_probe_manifest_tiles(
        true, false, false, true, false, false
    ));
    assert!(!should_probe_manifest_tiles(
        true, false, false, false, true, false
    ));
}

#[::core::prelude::v1::test]
fn cached_manifest_marks_all_scanned_tiles_without_reprobing_empty_tiles() {
    let requested_tiles = vec![(0, 0), (1, 0)];
    let mut tile_chunk_index = BTreeMap::new();
    tile_chunk_index.insert(
        (0, 0),
        TileChunkPositions::from(vec![ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        }]),
    );

    let completed = complete_cached_tile_chunk_index(&requested_tiles, tile_chunk_index);

    assert_eq!(completed.len(), 2);
    assert!(
        completed
            .get(&(0, 0))
            .is_some_and(|chunks| !chunks.is_empty())
    );
    assert!(
        completed
            .get(&(1, 0))
            .is_some_and(|chunks| chunks.is_empty())
    );
}

#[::core::prelude::v1::test]
fn overlay_query_waits_for_visible_tile_pipeline_but_not_idle_viewport() {
    assert!(should_defer_overlay_query_for_visible_tiles(
        true, false, false
    ));
    assert!(should_defer_overlay_query_for_visible_tiles(
        false, true, false
    ));
    assert!(should_defer_overlay_query_for_visible_tiles(
        false, false, true
    ));
    assert!(!should_defer_overlay_query_for_visible_tiles(
        false, false, false
    ));
}

#[::core::prelude::v1::test]
fn renderer_cache_resolution_does_not_remove_tile_from_render_queue() {
    let mut manager = RegionManager::default();
    manager.ensure_tiles(&[(0, 0)], TilePriority::Visible);

    assert_eq!(
        manager.queued_coords((0, 0), None, false, true),
        vec![(0, 0)]
    );
}

#[::core::prelude::v1::test]
fn visible_region_growth_does_not_cancel_loading_regions() {
    let mut manager = RegionManager::default();
    manager.ensure_tiles(&[(0, 0)], TilePriority::Visible);
    manager.mark_loading(&[(0, 0)]);
    manager.ensure_tiles(&[(0, 0), (1, 0)], TilePriority::Visible);

    assert_eq!(manager.loading_count(), 1);
    assert_eq!(manager.queued_count(), 1);
    assert_eq!(
        manager.queued_coords((0, 0), None, false, true),
        vec![(1, 0)]
    );
}

#[::core::prelude::v1::test]
fn retain_tiles_removes_loading_tiles_outside_retained_region() {
    let mut manager = RegionManager::default();
    manager.ensure_tiles(&[(0, 0), (1, 0)], TilePriority::Visible);
    manager.mark_loading(&[(0, 0), (1, 0)]);

    let dropped_images = manager.retain_tiles(&BTreeSet::from([(0, 0)]));

    assert!(dropped_images.is_empty());
    assert!(manager.entries.contains_key(&(0, 0)));
    assert!(!manager.entries.contains_key(&(1, 0)));
    assert_eq!(manager.loading_count(), 1);
}

#[::core::prelude::v1::test]
fn retain_tiles_by_filter_removes_tiles_without_retained_set() {
    let mut manager = RegionManager::default();
    manager.ensure_tiles(&[(0, 0), (1, 0)], TilePriority::Visible);
    manager.mark_loading(&[(0, 0), (1, 0)]);

    let dropped_images = manager.retain_tiles_by(|coord| coord == (0, 0));

    assert!(dropped_images.is_empty());
    assert!(manager.entries.contains_key(&(0, 0)));
    assert!(!manager.entries.contains_key(&(1, 0)));
    assert_eq!(manager.loading_count(), 1);
}

#[::core::prelude::v1::test]
fn subsequent_visible_region_batches_keep_camera_center_priority() {
    let mut manager = RegionManager::default();
    let bounds = TileBounds {
        min_x: -4,
        max_x: 4,
        min_z: -4,
        max_z: 4,
    };
    let visible_tiles = tile_coords_from_bounds(bounds);
    manager.ensure_tiles(&visible_tiles, TilePriority::Visible);

    let queued = manager.queued_coords((0, 0), Some(bounds), false, true);
    let first_batch = queued.iter().take(20).copied().collect::<Vec<_>>();

    assert_eq!(queued.first(), Some(&(0, 0)));
    for axis_neighbor in [(0, -1), (-1, 0), (1, 0), (0, 1)] {
        let position = queued
            .iter()
            .position(|coord| *coord == axis_neighbor)
            .expect("axis neighbor tile");
        assert!(position < 9);
    }
    assert!(
        first_batch
            .iter()
            .all(|coord| (coord.0.abs().max(coord.1.abs())) <= 3)
    );
}

#[::core::prelude::v1::test]
fn stale_cache_hit_is_replaced_by_render_and_late_cache_does_not_overwrite_fresh() {
    let mut manager = RegionManager::default();
    manager.ensure_tiles(&[(0, 0)], TilePriority::Visible);

    let accepted = manager.mark_loaded_from_cache(
        (0, 0),
        test_tile([1, 2, 3, 255]),
        TileSourceFreshness::Stale,
    );
    assert!(accepted);
    assert_eq!(
        manager
            .entries
            .get(&(0, 0))
            .map(|entry| entry.source_status),
        Some(TileSourceStatus::DiskStale)
    );
    assert!(manager.requeue_stale_cache_for_refresh((0, 0)));
    let stale_entry = manager.entries.get(&(0, 0)).expect("stale cache entry");
    assert_eq!(stale_entry.state, TileLoadState::Queued);
    assert_eq!(stale_entry.source_status, TileSourceStatus::DiskStale);
    assert!(stale_entry.image.is_some());
    assert_eq!(
        manager.queued_coords((0, 0), None, false, true),
        vec![(0, 0)]
    );
    manager.mark_loading(&[(0, 0)]);

    let fresh = test_tile([4, 5, 6, 255]);
    let fresh_image = fresh.image.clone();
    manager.mark_loaded((0, 0), fresh);
    assert_eq!(
        manager
            .entries
            .get(&(0, 0))
            .map(|entry| entry.source_status),
        Some(TileSourceStatus::Fresh)
    );

    let accepted = manager.mark_loaded_from_cache(
        (0, 0),
        test_tile([7, 8, 9, 255]),
        TileSourceFreshness::Stale,
    );
    assert!(!accepted);
    let current = manager
        .entries
        .get(&(0, 0))
        .and_then(|entry| entry.image.as_ref())
        .expect("fresh image");
    assert!(Arc::ptr_eq(&current.image, &fresh_image));
}

#[::core::prelude::v1::test]
fn fresh_cache_hit_does_not_need_validation_render() {
    let mut manager = RegionManager::default();
    manager.ensure_tiles(&[(0, 0)], TilePriority::Visible);

    let accepted = manager.mark_loaded_from_cache(
        (0, 0),
        test_tile([1, 2, 3, 255]),
        TileSourceFreshness::Fresh,
    );
    assert!(accepted);

    assert_eq!(
        manager
            .entries
            .get(&(0, 0))
            .map(|entry| entry.source_status),
        Some(TileSourceStatus::Fresh)
    );
    assert!(manager.queued_coords((0, 0), None, false, true).is_empty());
}

#[::core::prelude::v1::test]
fn byte_budget_trim_preserves_visible_tiles() {
    let mut manager = RegionManager::default();
    manager.ensure_tiles(&[(0, 0)], TilePriority::Visible);
    manager.ensure_tiles(&[(2, 0)], TilePriority::Prefetch);
    manager.mark_loaded((0, 0), test_tile([1, 2, 3, 255]));
    let prefetch = test_tile([4, 5, 6, 255]);
    let prefetch_image = prefetch.image.clone();
    manager.mark_loaded((2, 0), prefetch);

    let visible_tiles = BTreeSet::from([(0, 0)]);
    let dropped_images = manager.trim_loaded_tiles_to_budget(&visible_tiles, 4);

    assert_eq!(
        manager.entries.get(&(0, 0)).map(|entry| entry.state),
        Some(TileLoadState::Loaded)
    );
    assert_eq!(
        manager.entries.get(&(2, 0)).map(|entry| entry.state),
        Some(TileLoadState::Queued)
    );
    assert!(
        manager
            .entries
            .get(&(2, 0))
            .is_some_and(|entry| entry.image.is_none())
    );
    assert_eq!(dropped_images.len(), 1);
    assert!(Arc::ptr_eq(&dropped_images[0], &prefetch_image));
}

#[::core::prelude::v1::test]
fn byte_budget_trim_by_filter_preserves_retained_tiles() {
    let mut manager = RegionManager::default();
    manager.ensure_tiles(&[(0, 0)], TilePriority::Visible);
    manager.ensure_tiles(&[(2, 0)], TilePriority::Prefetch);
    manager.mark_loaded((0, 0), test_tile([1, 2, 3, 255]));
    let prefetch = test_tile([4, 5, 6, 255]);
    let prefetch_image = prefetch.image.clone();
    manager.mark_loaded((2, 0), prefetch);

    let dropped_images = manager.trim_loaded_tiles_to_budget_by(|coord| coord == (0, 0), 4);

    assert_eq!(
        manager.entries.get(&(0, 0)).map(|entry| entry.state),
        Some(TileLoadState::Loaded)
    );
    assert_eq!(
        manager.entries.get(&(2, 0)).map(|entry| entry.state),
        Some(TileLoadState::Queued)
    );
    assert_eq!(dropped_images.len(), 1);
    assert!(Arc::ptr_eq(&dropped_images[0], &prefetch_image));
}

#[::core::prelude::v1::test]
fn byte_budget_trim_uses_last_access_for_warm_tile_lru() {
    let mut manager = RegionManager::default();
    let coords = [(0, 0), (1, 0)];
    manager.ensure_tiles(&coords, TilePriority::Visible);
    manager.mark_loaded((0, 0), test_tile([1, 2, 3, 255]));
    let recently_used = manager
        .entries
        .get(&(0, 0))
        .map(|entry| entry.last_access)
        .expect("first tile access stamp");
    manager.mark_loaded((1, 0), test_tile([4, 5, 6, 255]));

    // Re-entering a tile updates recency without changing its original request sequence.
    manager.ensure_tiles(&[(0, 0)], TilePriority::Visible);
    assert!(
        manager
            .entries
            .get(&(0, 0))
            .is_some_and(|entry| entry.last_access > recently_used)
    );

    let dropped_images = manager.trim_loaded_tiles_to_budget_by(|_| false, 4);

    assert_eq!(dropped_images.len(), 1);
    assert!(
        manager
            .entries
            .get(&(0, 0))
            .is_some_and(|entry| entry.image.is_some())
    );
    assert!(
        manager
            .entries
            .get(&(1, 0))
            .is_some_and(|entry| entry.image.is_none())
    );
}

#[::core::prelude::v1::test]
fn entry_capacity_trim_keeps_retained_and_recent_tiles() {
    let mut manager = RegionManager::default();
    let coords = [(0, 0), (1, 0), (2, 0), (3, 0)];
    manager.ensure_tiles(&coords, TilePriority::Visible);
    for (index, coord) in coords.into_iter().enumerate() {
        manager.mark_loaded(coord, test_tile([index as u8, 0, 0, 255]));
    }
    manager.ensure_tiles(&[(2, 0)], TilePriority::Visible);

    let dropped_images = manager.trim_entries_to_capacity_by(|coord| coord == (3, 0), 2);

    assert_eq!(manager.entries.len(), 2);
    assert!(manager.entries.contains_key(&(2, 0)));
    assert!(manager.entries.contains_key(&(3, 0)));
    assert_eq!(manager.loaded_count(), 2);
    assert_eq!(dropped_images.len(), 2);
}

#[::core::prelude::v1::test]
fn entry_capacity_trim_keeps_in_flight_state_ownership() {
    let mut manager = RegionManager::default();
    manager.ensure_tiles(&[(0, 0), (1, 0)], TilePriority::Visible);
    manager.mark_loading(&[(0, 0)]);
    manager.mark_invalid((1, 0), SharedString::from("empty tile"));

    manager.trim_entries_to_capacity_by(|_| false, 0);

    assert_eq!(manager.entries.len(), 1);
    assert_eq!(
        manager.entries.get(&(0, 0)).map(|entry| entry.state),
        Some(TileLoadState::Loading)
    );
}

#[::core::prelude::v1::test]
fn empty_manifest_tile_stays_invalid_negative_cache() {
    let mut manager = RegionManager::default();
    manager.ensure_tiles(&[(0, 0)], TilePriority::Visible);
    manager.mark_invalid((0, 0), SharedString::from("索引确认该瓦片没有可渲染区块"));

    assert_eq!(
        manager.entries.get(&(0, 0)).map(|entry| entry.state),
        Some(TileLoadState::Invalid)
    );
}

#[::core::prelude::v1::test]
fn edit_invalidation_maps_chunks_to_tiles() {
    let layout = RenderLayout {
        chunks_per_tile: 8,
        blocks_per_pixel: 1,
        pixels_per_block: 4,
    };
    let chunks = [
        ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        },
        ChunkPos {
            x: 7,
            z: 7,
            dimension: Dimension::Overworld,
        },
        ChunkPos {
            x: 8,
            z: -1,
            dimension: Dimension::Overworld,
        },
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();

    assert_eq!(
        tile_coords_for_chunks(&chunks, layout),
        vec![(0, 0), (1, -1)]
    );
}

#[::core::prelude::v1::test]
fn merge_chunks_into_tile_index_preserves_existing_tile_chunks() {
    let layout = RenderLayout {
        chunks_per_tile: 8,
        blocks_per_pixel: 1,
        pixels_per_block: 4,
    };
    let existing_chunk = ChunkPos {
        x: 8,
        z: -1,
        dimension: Dimension::Overworld,
    };
    let unrelated_chunk = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    let inserted_chunk = ChunkPos {
        x: 15,
        z: -8,
        dimension: Dimension::Overworld,
    };
    let ignored_chunk = ChunkPos {
        x: 7,
        z: 0,
        dimension: Dimension::Overworld,
    };
    let mut tile_chunk_index = BTreeMap::from([
        ((1, -1), TileChunkPositions::from(vec![existing_chunk])),
        ((0, 0), TileChunkPositions::from(vec![unrelated_chunk])),
    ]);
    let chunks = [inserted_chunk, ignored_chunk]
        .into_iter()
        .collect::<BTreeSet<_>>();

    merge_chunks_into_tile_index(&mut tile_chunk_index, (1, -1), &chunks, layout);

    assert_eq!(
        tile_chunk_index
            .get(&(1, -1))
            .map(|positions| positions.as_ref()),
        Some([existing_chunk, inserted_chunk].as_slice())
    );
    assert_eq!(
        tile_chunk_index
            .get(&(0, 0))
            .map(|positions| positions.as_ref()),
        Some([unrelated_chunk].as_slice())
    );
}

#[::core::prelude::v1::test]
fn chunk_patch_merge_only_replaces_matching_chunk_pixels() {
    let layout = RenderLayout {
        chunks_per_tile: 2,
        blocks_per_pixel: 16,
        pixels_per_block: 1,
    };
    let mut tile_pixels = vec![1, 1, 1, 255, 2, 2, 2, 255, 3, 3, 3, 255, 4, 4, 4, 255];
    let patch = DecodedTileImage {
        coord: TileCoord {
            x: 1,
            z: 1,
            dimension: Dimension::Overworld,
        },
        width: 1,
        height: 1,
        pixels: Arc::from(vec![9, 8, 7, 255]),
        pixel_format: TilePixelFormat::Rgba8,
    };

    merge_chunk_patch_into_tile_pixels(
        &mut tile_pixels,
        2,
        layout,
        ChunkPos {
            x: 1,
            z: 1,
            dimension: Dimension::Overworld,
        },
        patch,
    )
    .expect("merge patch");

    assert_eq!(
        tile_pixels,
        vec![1, 1, 1, 255, 2, 2, 2, 255, 3, 3, 3, 255, 9, 8, 7, 255,]
    );
}

#[::core::prelude::v1::test]
fn editor_confirmation_requires_matching_target_and_action() {
    let target = EditTarget::HsaChunk(ChunkPos {
        x: 1,
        z: 2,
        dimension: Dimension::Overworld,
    });
    let pending = PendingEditConfirmation {
        target: target.clone(),
        action: EditAction::Save,
    };

    assert!(pending.target == target && pending.action == EditAction::Save);
    assert!(pending.action != EditAction::Delete);
}

#[::core::prelude::v1::test]
fn delete_and_reset_chunk_actions_have_distinct_user_semantics() {
    let chunk = ChunkPos {
        x: 3,
        z: -4,
        dimension: Dimension::Overworld,
    };

    assert_eq!(
        QuickWriteAction::DeleteCurrentChunk(chunk).label(),
        "删除当前 chunk 3,-4"
    );
    assert_eq!(
        QuickWriteAction::ResetCurrentChunk(chunk).label(),
        "重置当前 chunk 3,-4"
    );
    assert!(chunk_record_tag_is_clear_target(
        ChunkRecordTag::SubChunkPrefix
    ));
    assert!(chunk_record_tag_is_clear_target(
        ChunkRecordTag::BlockEntity
    ));
    assert!(!chunk_record_tag_is_clear_target(ChunkRecordTag::Version));
}

#[::core::prelude::v1::test]
fn copy_safe_chunk_records_excludes_coordinate_sensitive_records() {
    let chunk = ChunkPos {
        x: 8,
        z: -3,
        dimension: Dimension::Overworld,
    };
    let records = vec![
        test_chunk_record(chunk, ChunkRecordTag::Data3D),
        test_chunk_record(chunk, ChunkRecordTag::SubChunkPrefix),
        test_chunk_record(chunk, ChunkRecordTag::BlockEntity),
        test_chunk_record(chunk, ChunkRecordTag::Entity),
        test_chunk_record(chunk, ChunkRecordTag::HardcodedSpawners),
        test_chunk_record(chunk, ChunkRecordTag::Version),
    ];

    let copied_tags = copy_safe_chunk_records(records)
        .into_iter()
        .map(|record| record.key.tag)
        .collect::<Vec<_>>();

    assert_eq!(
        copied_tags,
        vec![
            ChunkRecordTag::Data3D,
            ChunkRecordTag::SubChunkPrefix,
            ChunkRecordTag::Version,
        ]
    );
}

fn test_chunk_record(chunk: ChunkPos, tag: ChunkRecordTag) -> ChunkRecord {
    ChunkRecord {
        key: ChunkKey::new(chunk, tag),
        value: Vec::new().into(),
    }
}

#[::core::prelude::v1::test]
fn chunk_image_export_composes_multiple_chunks_to_png() {
    let chunks = vec![
        ChunkImageExportChunk {
            chunk: ChunkPos {
                x: 0,
                z: 0,
                dimension: Dimension::Overworld,
            },
            pixels: vec![255, 0, 0, 255],
            width: 1,
            height: 1,
        },
        ChunkImageExportChunk {
            chunk: ChunkPos {
                x: 1,
                z: 0,
                dimension: Dimension::Overworld,
            },
            pixels: vec![0, 255, 0, 255],
            width: 1,
            height: 1,
        },
    ];
    let export = chunk_image_export_from_chunks("test", chunks).expect("export source");

    let png = encode_chunk_image_export_png(&export).expect("encode png");
    let decoded = gpui::image::load_from_memory(&png)
        .expect("decode png")
        .to_rgba8();

    assert_eq!(decoded.dimensions(), (2, 1));
    assert_eq!(decoded.get_pixel(0, 0).0, [255, 0, 0, 255]);
    assert_eq!(decoded.get_pixel(1, 0).0, [0, 255, 0, 255]);
}

#[::core::prelude::v1::test]
fn quick_write_actions_that_change_map_pixels_force_tile_refresh() {
    let source = ChunkPos {
        x: 1,
        z: 2,
        dimension: Dimension::Overworld,
    };
    let target = ChunkPos {
        x: 10,
        z: -8,
        dimension: Dimension::Overworld,
    };

    assert!(!QuickWriteAction::DeleteCurrentChunk(target).reuses_known_tile_index_after_write());
    assert!(!QuickWriteAction::ResetCurrentChunk(target).reuses_known_tile_index_after_write());
    assert!(
        QuickWriteAction::DeleteCurrentChunkBlockEntities(target)
            .reuses_known_tile_index_after_write()
    );
    assert!(
        QuickWriteAction::DeleteCurrentChunkActors(target).reuses_known_tile_index_after_write()
    );
    assert!(
        QuickWriteAction::PasteCopiedChunks {
            source_anchor: source,
            target_anchor: target,
            chunk_count: 4,
            transform: PasteTransform::default(),
        }
        .prioritizes_tile_refresh()
    );
    assert!(
        !QuickWriteAction::PasteCopiedChunks {
            source_anchor: source,
            target_anchor: target,
            chunk_count: 4,
            transform: PasteTransform::default(),
        }
        .reuses_known_tile_index_after_write()
    );
    assert!(
        QuickWriteAction::PasteImportedStructure {
            source_anchor: source,
            target_anchor: target,
            chunk_count: 4,
            transform: PasteTransform::default(),
        }
        .prioritizes_tile_refresh()
    );
    assert!(
        !QuickWriteAction::PasteImportedStructure {
            source_anchor: source,
            target_anchor: target,
            chunk_count: 4,
            transform: PasteTransform::default(),
        }
        .reuses_known_tile_index_after_write()
    );
}

#[::core::prelude::v1::test]
fn paste_preview_rotation_snaps_to_nearest_quadrant() {
    assert_eq!(snapped_paste_rotation(44.0), PasteRotation::NoRotation);
    assert_eq!(snapped_paste_rotation(46.0), PasteRotation::Clockwise90);
    assert_eq!(snapped_paste_rotation(135.0), PasteRotation::Rotate180);
    assert_eq!(
        snapped_paste_rotation(315.0),
        PasteRotation::CounterClockwise90
    );
    assert_eq!(snapped_paste_rotation(359.0), PasteRotation::NoRotation);
}

#[::core::prelude::v1::test]
fn pasted_chunk_targets_keep_all_relative_offsets() {
    let source_anchor = ChunkPos {
        x: -2,
        z: 5,
        dimension: Dimension::Overworld,
    };
    let target_anchor = ChunkPos {
        x: 10,
        z: -7,
        dimension: Dimension::Nether,
    };
    let copied_chunk = CopiedChunkData {
        source: source_anchor,
        chunks: vec![
            CopiedChunkSnapshot {
                chunk: source_anchor,
                records: Vec::new(),
                block_entities: Vec::new(),
                hardcoded_spawn_areas: Vec::new(),
            },
            CopiedChunkSnapshot {
                chunk: ChunkPos {
                    x: -1,
                    z: 5,
                    dimension: Dimension::Overworld,
                },
                records: Vec::new(),
                block_entities: Vec::new(),
                hardcoded_spawn_areas: Vec::new(),
            },
            CopiedChunkSnapshot {
                chunk: ChunkPos {
                    x: -2,
                    z: 7,
                    dimension: Dimension::Overworld,
                },
                records: Vec::new(),
                block_entities: Vec::new(),
                hardcoded_spawn_areas: Vec::new(),
            },
        ],
    };

    assert_eq!(
        pasted_chunk_targets(
            &copied_chunk,
            source_anchor,
            target_anchor,
            PasteTransform::default()
        ),
        vec![
            ChunkPos {
                x: 10,
                z: -7,
                dimension: Dimension::Nether,
            },
            ChunkPos {
                x: 11,
                z: -7,
                dimension: Dimension::Nether,
            },
            ChunkPos {
                x: 10,
                z: -5,
                dimension: Dimension::Nether,
            },
        ]
    );
}

#[::core::prelude::v1::test]
fn pasted_chunk_targets_rotate_relative_offsets() {
    let source_anchor = ChunkPos {
        x: 4,
        z: 10,
        dimension: Dimension::Overworld,
    };
    let target_anchor = ChunkPos {
        x: -20,
        z: 30,
        dimension: Dimension::End,
    };
    let copied_chunk = CopiedChunkData {
        source: source_anchor,
        chunks: vec![
            CopiedChunkSnapshot {
                chunk: source_anchor,
                records: Vec::new(),
                block_entities: Vec::new(),
                hardcoded_spawn_areas: Vec::new(),
            },
            CopiedChunkSnapshot {
                chunk: ChunkPos {
                    x: 6,
                    z: 10,
                    dimension: Dimension::Overworld,
                },
                records: Vec::new(),
                block_entities: Vec::new(),
                hardcoded_spawn_areas: Vec::new(),
            },
            CopiedChunkSnapshot {
                chunk: ChunkPos {
                    x: 4,
                    z: 13,
                    dimension: Dimension::Overworld,
                },
                records: Vec::new(),
                block_entities: Vec::new(),
                hardcoded_spawn_areas: Vec::new(),
            },
        ],
    };

    assert_eq!(
        pasted_chunk_targets(
            &copied_chunk,
            source_anchor,
            target_anchor,
            PasteTransform::from_rotation(PasteRotation::Clockwise90)
        ),
        vec![
            ChunkPos {
                x: -20,
                z: 30,
                dimension: Dimension::End,
            },
            ChunkPos {
                x: -20,
                z: 32,
                dimension: Dimension::End,
            },
            ChunkPos {
                x: -23,
                z: 30,
                dimension: Dimension::End,
            },
        ]
    );

    assert_eq!(
        pasted_chunk_targets(
            &copied_chunk,
            source_anchor,
            target_anchor,
            PasteTransform::from_rotation(PasteRotation::Rotate180)
        ),
        vec![
            ChunkPos {
                x: -20,
                z: 30,
                dimension: Dimension::End,
            },
            ChunkPos {
                x: -22,
                z: 30,
                dimension: Dimension::End,
            },
            ChunkPos {
                x: -20,
                z: 27,
                dimension: Dimension::End,
            },
        ]
    );

    assert_eq!(
        pasted_chunk_targets(
            &copied_chunk,
            source_anchor,
            target_anchor,
            PasteTransform::from_rotation(PasteRotation::CounterClockwise90)
        ),
        vec![
            ChunkPos {
                x: -20,
                z: 30,
                dimension: Dimension::End,
            },
            ChunkPos {
                x: -20,
                z: 28,
                dimension: Dimension::End,
            },
            ChunkPos {
                x: -17,
                z: 30,
                dimension: Dimension::End,
            },
        ]
    );
}

#[::core::prelude::v1::test]
fn pasted_chunk_targets_mirror_relative_offsets() {
    let source_anchor = ChunkPos {
        x: 4,
        z: 10,
        dimension: Dimension::Overworld,
    };
    let target_anchor = ChunkPos {
        x: -20,
        z: 30,
        dimension: Dimension::End,
    };
    let copied_chunk = CopiedChunkData {
        source: source_anchor,
        chunks: vec![
            CopiedChunkSnapshot {
                chunk: source_anchor,
                records: Vec::new(),
                block_entities: Vec::new(),
                hardcoded_spawn_areas: Vec::new(),
            },
            CopiedChunkSnapshot {
                chunk: ChunkPos {
                    x: 6,
                    z: 10,
                    dimension: Dimension::Overworld,
                },
                records: Vec::new(),
                block_entities: Vec::new(),
                hardcoded_spawn_areas: Vec::new(),
            },
            CopiedChunkSnapshot {
                chunk: ChunkPos {
                    x: 4,
                    z: 13,
                    dimension: Dimension::Overworld,
                },
                records: Vec::new(),
                block_entities: Vec::new(),
                hardcoded_spawn_areas: Vec::new(),
            },
        ],
    };

    assert_eq!(
        pasted_chunk_targets(
            &copied_chunk,
            source_anchor,
            target_anchor,
            PasteTransform {
                mirror_x: true,
                ..PasteTransform::default()
            }
        ),
        vec![
            ChunkPos {
                x: -20,
                z: 30,
                dimension: Dimension::End,
            },
            ChunkPos {
                x: -22,
                z: 30,
                dimension: Dimension::End,
            },
            ChunkPos {
                x: -20,
                z: 33,
                dimension: Dimension::End,
            },
        ]
    );

    assert_eq!(
        pasted_chunk_targets(
            &copied_chunk,
            source_anchor,
            target_anchor,
            PasteTransform {
                mirror_z: true,
                ..PasteTransform::default()
            }
        ),
        vec![
            ChunkPos {
                x: -20,
                z: 30,
                dimension: Dimension::End,
            },
            ChunkPos {
                x: -18,
                z: 30,
                dimension: Dimension::End,
            },
            ChunkPos {
                x: -20,
                z: 27,
                dimension: Dimension::End,
            },
        ]
    );
}

#[::core::prelude::v1::test]
fn copied_chunk_snapshot_structure_placement_uses_full_height_and_single_target() {
    let source_anchor = ChunkPos {
        x: 4,
        z: 10,
        dimension: Dimension::Overworld,
    };
    let target_anchor = ChunkPos {
        x: -20,
        z: 30,
        dimension: Dimension::Overworld,
    };
    let copied_chunk = CopiedChunkData {
        source: source_anchor,
        chunks: vec![
            CopiedChunkSnapshot {
                chunk: ChunkPos {
                    x: 3,
                    z: 10,
                    dimension: Dimension::Overworld,
                },
                records: Vec::new(),
                block_entities: Vec::new(),
                hardcoded_spawn_areas: Vec::new(),
            },
            CopiedChunkSnapshot {
                chunk: source_anchor,
                records: Vec::new(),
                block_entities: Vec::new(),
                hardcoded_spawn_areas: Vec::new(),
            },
            CopiedChunkSnapshot {
                chunk: ChunkPos {
                    x: 5,
                    z: 12,
                    dimension: Dimension::Overworld,
                },
                records: Vec::new(),
                block_entities: Vec::new(),
                hardcoded_spawn_areas: Vec::new(),
            },
        ],
    };
    let transform = PasteTransform {
        rotation: PasteRotation::Clockwise90,
        mirror_x: true,
        mirror_z: false,
    };
    let targets = pasted_chunk_targets(&copied_chunk, source_anchor, target_anchor, transform);

    for (snapshot, target_chunk) in copied_chunk.chunks.iter().zip(targets) {
        let placement = copied_chunk_snapshot_structure_placement(snapshot, target_chunk)
            .expect("copied chunk structure placement");
        let target_chunks = placement
            .structure
            .target_chunks(bedrock_world::McStructurePlacement {
                source_anchor: placement.source_anchor,
                target_anchor: placement.target_anchor,
                origin_y: placement.origin_y,
                rotation: bedrock_world::McStructureRotation::Clockwise90,
                mirror_x: true,
                mirror_z: false,
            })
            .expect("target chunks");
        let expected_targets = [target_chunk].into_iter().collect::<BTreeSet<_>>();

        assert_eq!(placement.structure.size.x, 16);
        assert_eq!(placement.structure.size.y, 384);
        assert_eq!(placement.structure.size.z, 16);
        assert_eq!(placement.origin_y, -64);
        assert_eq!(target_chunks, expected_targets);
    }
}

#[::core::prelude::v1::test]
fn copied_chunk_snapshot_structure_placement_preserves_secondary_layer() {
    let source_chunk = ChunkPos {
        x: 4,
        z: 10,
        dimension: Dimension::Overworld,
    };
    let snapshot = CopiedChunkSnapshot {
        chunk: source_chunk,
        records: vec![test_two_layer_subchunk_record(source_chunk, 0, (1, 2, 3))],
        block_entities: Vec::new(),
        hardcoded_spawn_areas: Vec::new(),
    };

    let placement = copied_chunk_snapshot_structure_placement(&snapshot, source_chunk)
        .expect("copied chunk structure placement");
    let block_index = placement
        .structure
        .size
        .index(1, 66, 3)
        .expect("structure block index");
    let primary_index = placement.structure.primary_indices[block_index];
    let secondary_index = placement.structure.secondary_indices[block_index];

    assert_eq!(
        placement.structure.palette[usize::try_from(primary_index).expect("primary index")].name,
        "minecraft:stone"
    );
    assert_eq!(
        placement.structure.palette[usize::try_from(secondary_index).expect("secondary index")]
            .name,
        "minecraft:water"
    );
    assert_eq!(
        placement.structure.secondary_indices[placement
            .structure
            .size
            .index(1, 67, 3)
            .expect("air secondary index")],
        -1
    );
}

fn test_two_layer_subchunk_record(
    chunk: ChunkPos,
    subchunk_y: i8,
    block: (u8, u8, u8),
) -> ChunkRecord {
    ChunkRecord {
        key: ChunkKey::subchunk(chunk, subchunk_y),
        value: Bytes::from(test_two_layer_subchunk_bytes(block)),
    }
}

fn test_two_layer_subchunk_bytes(block: (u8, u8, u8)) -> Vec<u8> {
    let mut bytes = vec![8, 2];
    append_test_subchunk_palette_storage(&mut bytes, &["minecraft:air", "minecraft:stone"], block);
    append_test_subchunk_palette_storage(&mut bytes, &["minecraft:air", "minecraft:water"], block);
    bytes
}

fn append_test_subchunk_palette_storage(
    bytes: &mut Vec<u8>,
    palette: &[&str],
    filled_block: (u8, u8, u8),
) {
    let bits_per_block = 1_u8;
    let values_per_word = usize::from(32 / bits_per_block);
    let mut words = vec![0_u32; 128];
    let block_index =
        bedrock_world::block_storage_index(filled_block.0, filled_block.1, filled_block.2);
    let word_index = block_index / values_per_word;
    let bit_offset = (block_index % values_per_word) * usize::from(bits_per_block);
    words[word_index] |= 1_u32 << bit_offset;
    bytes.push(bits_per_block << 1);
    for word in words {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes.extend_from_slice(
        &i32::try_from(palette.len())
            .expect("test palette length")
            .to_le_bytes(),
    );
    for name in palette {
        let tag = bedrock_world::NbtTag::Compound(indexmap::IndexMap::from([
            (
                "name".to_string(),
                bedrock_world::NbtTag::String((*name).to_string()),
            ),
            (
                "states".to_string(),
                bedrock_world::NbtTag::Compound(indexmap::IndexMap::new()),
            ),
            ("version".to_string(), bedrock_world::NbtTag::Int(1)),
        ]));
        bytes.extend_from_slice(
            &bedrock_world::NbtWriter::write_root(&tag).expect("serialize test palette"),
        );
    }
}

#[::core::prelude::v1::test]
fn imported_structure_targets_rotate_relative_offsets() {
    let source_anchor = ChunkPos {
        x: 4,
        z: 10,
        dimension: Dimension::Overworld,
    };
    let target_anchor = ChunkPos {
        x: -20,
        z: 30,
        dimension: Dimension::End,
    };
    let size = bedrock_world::McStructureSize::new(33, 4, 49).expect("structure size");
    let imported_structure = ImportedStructureData {
        structure: Arc::new(
            bedrock_world::McStructureFile::new_air(size, [0, 64, 0]).expect("air structure"),
        ),
        source_anchor,
        origin_y: 64,
    };

    assert_eq!(
        mcstructure::imported_structure_targets(
            &imported_structure,
            target_anchor,
            PasteTransform::from_rotation(PasteRotation::Clockwise90)
        ),
        BTreeSet::from([
            ChunkPos {
                x: -20,
                z: 30,
                dimension: Dimension::End,
            },
            ChunkPos {
                x: -20,
                z: 31,
                dimension: Dimension::End,
            },
            ChunkPos {
                x: -20,
                z: 32,
                dimension: Dimension::End,
            },
            ChunkPos {
                x: -21,
                z: 30,
                dimension: Dimension::End,
            },
            ChunkPos {
                x: -21,
                z: 31,
                dimension: Dimension::End,
            },
            ChunkPos {
                x: -21,
                z: 32,
                dimension: Dimension::End,
            },
            ChunkPos {
                x: -22,
                z: 30,
                dimension: Dimension::End,
            },
            ChunkPos {
                x: -22,
                z: 31,
                dimension: Dimension::End,
            },
            ChunkPos {
                x: -22,
                z: 32,
                dimension: Dimension::End,
            },
            ChunkPos {
                x: -23,
                z: 30,
                dimension: Dimension::End,
            },
            ChunkPos {
                x: -23,
                z: 31,
                dimension: Dimension::End,
            },
            ChunkPos {
                x: -23,
                z: 32,
                dimension: Dimension::End,
            },
        ])
    );
}

#[::core::prelude::v1::test]
fn pasted_block_entity_updates_position_and_nbt_coordinates() {
    let source_chunk = ChunkPos {
        x: -114,
        z: 28,
        dimension: Dimension::Overworld,
    };
    let target_chunk = ChunkPos {
        x: -113,
        z: 31,
        dimension: Dimension::Overworld,
    };
    let source_position = [source_chunk.x * 16 + 7, 64, source_chunk.z * 16 + 11];
    let mut root = indexmap::IndexMap::new();
    root.insert("id".to_string(), NbtTag::String("Chest".to_string()));
    root.insert("x".to_string(), NbtTag::Int(source_position[0]));
    root.insert("y".to_string(), NbtTag::Int(source_position[1]));
    root.insert("z".to_string(), NbtTag::Int(source_position[2]));
    let entity = ParsedBlockEntity {
        id: Some("Chest".to_string()),
        position: Some(source_position),
        is_movable: None,
        custom_name: None,
        items: Vec::new(),
        nbt: NbtTag::Compound(root),
    };

    let pasted = pasted_block_entity_for_target(&entity, source_chunk, target_chunk);
    let expected_position = [target_chunk.x * 16 + 7, 64, target_chunk.z * 16 + 11];

    assert_eq!(pasted.position, Some(expected_position));
    let NbtTag::Compound(root) = pasted.nbt else {
        panic!("block entity nbt should stay a compound");
    };
    assert_eq!(root.get("x"), Some(&NbtTag::Int(expected_position[0])));
    assert_eq!(root.get("y"), Some(&NbtTag::Int(expected_position[1])));
    assert_eq!(root.get("z"), Some(&NbtTag::Int(expected_position[2])));
}

#[::core::prelude::v1::test]
fn player_quick_edit_preserves_unknown_fields_when_moving_player() {
    let mut root = indexmap::IndexMap::new();
    root.insert(
        "CustomModData".to_string(),
        NbtTag::String("keep".to_string()),
    );
    root.insert("DimensionId".to_string(), NbtTag::Int(-1));
    let mut tag = NbtTag::Compound(root);

    apply_player_quick_edit(
        &mut tag,
        &PlayerQuickEdit::MoveToMapCenter,
        (12, -8),
        Dimension::Overworld,
    )
    .expect("apply edit");

    let NbtTag::Compound(root) = tag else {
        panic!("player root remains a compound");
    };
    assert_eq!(
        root.get("CustomModData"),
        Some(&NbtTag::String("keep".to_string()))
    );
    assert_eq!(root.get("DimensionId"), Some(&NbtTag::Int(-1)));
    assert_eq!(
        nbt_vec3_f64(root.get("Pos")),
        Some([12.5_f64, 80.0_f64, -7.5_f64])
    );
}

#[::core::prelude::v1::test]
fn player_quick_edit_clear_inventory_does_not_drop_other_nbt() {
    let mut root = indexmap::IndexMap::new();
    root.insert(
        "UnknownList".to_string(),
        NbtTag::List(vec![NbtTag::Int(42)]),
    );
    root.insert(
        "Inventory".to_string(),
        NbtTag::List(vec![NbtTag::String("old".to_string())]),
    );
    let mut tag = NbtTag::Compound(root);

    apply_player_quick_edit(
        &mut tag,
        &PlayerQuickEdit::ClearInventory,
        (0, 0),
        Dimension::End,
    )
    .expect("apply edit");

    let NbtTag::Compound(root) = tag else {
        panic!("player root remains a compound");
    };
    assert_eq!(
        root.get("UnknownList"),
        Some(&NbtTag::List(vec![NbtTag::Int(42)]))
    );
    assert_eq!(root.get("Inventory"), Some(&NbtTag::List(Vec::new())));
    assert_eq!(root.get("DimensionId"), Some(&NbtTag::Int(2)));
}

#[::core::prelude::v1::test]
fn mcstructure_export_y_range_uses_dimension_build_height() {
    assert_eq!(
        mcstructure::export_y_range(Dimension::Overworld, 64),
        (-64, 319)
    );
    assert_eq!(
        mcstructure::export_y_range(Dimension::Overworld, -64),
        (-64, 319)
    );
    assert_eq!(
        mcstructure::export_y_range(Dimension::Nether, 127),
        (0, 127)
    );
    assert_eq!(mcstructure::export_y_range(Dimension::End, 64), (0, 255));
}

#[::core::prelude::v1::test]
fn hsa_structured_rows_keep_unknown_kind() {
    let area = ParsedHardcodedSpawnArea {
        kind: HardcodedSpawnAreaKind::Unknown(99),
        min: [0, 1, 2],
        max: [3, 4, 5],
    };
    let rows = hsa_rows(0, &area);

    assert_eq!(rows[0].value.as_ref(), "Unknown(99)");
    assert_eq!(rows[1].value.as_ref(), "0,1,2");
    assert_eq!(rows[2].value.as_ref(), "3,4,5");
}

#[::core::prelude::v1::test]
fn paste_672_chunks_reports_only_committed_batch_progress() {
    let source_anchor = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    let target_anchor = ChunkPos {
        x: 1_000,
        z: -1_000,
        dimension: Dimension::Overworld,
    };
    let chunks = (0..24)
        .flat_map(|z| {
            (0..28).map(move |x| {
                let chunk = ChunkPos {
                    x,
                    z,
                    dimension: Dimension::Overworld,
                };
                CopiedChunkSnapshot {
                    chunk,
                    records: vec![ChunkRecord {
                        key: ChunkKey::new(chunk, ChunkRecordTag::Version),
                        value: bytes::Bytes::from_static(b"\x2a"),
                    }],
                    block_entities: Vec::new(),
                    hardcoded_spawn_areas: Vec::new(),
                }
            })
        })
        .collect::<Vec<_>>();
    let copied_chunk = CopiedChunkData {
        source: source_anchor,
        chunks,
    };
    let storage = Arc::new(bedrock_world::MemoryStorage::new());
    let world = BedrockWorld::from_storage(
        "memory",
        storage.clone(),
        bedrock_world::OpenOptions {
            read_only: false,
            ..bedrock_world::OpenOptions::default()
        },
    );
    let guard = WriteGuard::confirmed("memory", "paste 672 chunk test");
    let mut progress = Vec::new();

    let (_, invalidation) = paste_copied_chunk_blocking(
        &world,
        &copied_chunk,
        source_anchor,
        target_anchor,
        PasteTransform::default(),
        &guard,
        None,
        &mut |update| {
            let committed_index = update.completed.saturating_sub(1);
            let committed_target = ChunkPos {
                x: target_anchor.x + i32::try_from(committed_index % 28).expect("target x"),
                z: target_anchor.z + i32::try_from(committed_index / 28).expect("target z"),
                dimension: target_anchor.dimension,
            };
            assert_eq!(
                bedrock_world::WorldStorage::get(
                    storage.as_ref(),
                    &ChunkKey::new(committed_target, ChunkRecordTag::Version).encode(),
                )
                .expect("read committed progress chunk"),
                Some(bytes::Bytes::from_static(b"\x2a"))
            );
            progress.push(update);
        },
    )
    .expect("paste 672 chunks");

    assert_eq!(invalidation.affected_chunks().len(), 672);
    assert_eq!(progress.len(), 42);
    assert!(progress.iter().enumerate().all(|(index, update)| {
        update.phase.as_ref() == "粘贴区块"
            && update.completed == (index + 1) * 16
            && update.total == 672
    }));
    for target in [
        target_anchor,
        ChunkPos {
            x: target_anchor.x + 27,
            z: target_anchor.z + 23,
            dimension: target_anchor.dimension,
        },
    ] {
        assert_eq!(
            bedrock_world::WorldStorage::get(
                storage.as_ref(),
                &ChunkKey::new(target, ChunkRecordTag::Version).encode(),
            )
            .expect("read pasted chunk record"),
            Some(bytes::Bytes::from_static(b"\x2a"))
        );
    }
}

#[::core::prelude::v1::test]
fn transformed_paste_writes_game_chunk_records_for_all_transform_modes() {
    let source = ChunkPos {
        x: 2,
        z: -3,
        dimension: Dimension::Overworld,
    };
    let mut biome_indices = vec![0_u16; 4096];
    biome_indices[bedrock_world::block_storage_index(1, 2, 3)] = 1;
    let biome = Biome3d::new(
        vec![64; 256],
        vec![bedrock_world::ParsedBiomeStorage {
            y: Some(-64),
            palette: vec![1, 42],
            indices: Some(biome_indices),
            counts: vec![4095, 1],
        }],
    )
    .expect("test biome");
    let copied_chunk = CopiedChunkData {
        source,
        chunks: vec![CopiedChunkSnapshot {
            chunk: source,
            records: vec![
                ChunkRecord {
                    key: ChunkKey::new(source, ChunkRecordTag::Version),
                    value: Bytes::from_static(b"\x2a"),
                },
                ChunkRecord {
                    key: ChunkKey::new(source, ChunkRecordTag::Data3D),
                    value: Bytes::from(biome.encode().expect("encode test biome")),
                },
                test_two_layer_subchunk_record(source, 0, (1, 2, 3)),
            ],
            block_entities: Vec::new(),
            hardcoded_spawn_areas: Vec::new(),
        }],
    };
    let storage = Arc::new(bedrock_world::MemoryStorage::new());
    let world = BedrockWorld::from_storage(
        "memory",
        storage,
        bedrock_world::OpenOptions {
            read_only: false,
            ..bedrock_world::OpenOptions::default()
        },
    );
    let guard = WriteGuard::confirmed("memory", "rotated paste test");

    let cases = [
        (
            PasteTransform {
                mirror_x: true,
                ..PasteTransform::default()
            },
            (14, 3),
        ),
        (
            PasteTransform {
                mirror_z: true,
                ..PasteTransform::default()
            },
            (1, 12),
        ),
        (
            PasteTransform::from_rotation(PasteRotation::Clockwise90),
            (12, 1),
        ),
        (
            PasteTransform::from_rotation(PasteRotation::CounterClockwise90),
            (3, 14),
        ),
        (
            PasteTransform::from_rotation(PasteRotation::Rotate180),
            (14, 12),
        ),
        (
            PasteTransform {
                rotation: PasteRotation::Clockwise90,
                mirror_x: true,
                mirror_z: false,
            },
            (12, 14),
        ),
    ];

    for (index, (transform, (target_x, target_z))) in cases.into_iter().enumerate() {
        let target = ChunkPos {
            x: 40 + i32::try_from(index).expect("target offset"),
            z: 50,
            dimension: Dimension::Overworld,
        };
        paste_copied_chunk_blocking(
            &world,
            &copied_chunk,
            source,
            target,
            transform,
            &guard,
            None,
            &mut |_| {},
        )
        .expect("transform and paste chunk");

        let subchunk = world
            .get_subchunk_blocking(target, 0)
            .expect("read target subchunk")
            .expect("target subchunk exists");
        assert_eq!(
            subchunk
                .block_state_at(target_x, 2, target_z)
                .map(|state| state.name.as_str()),
            Some("minecraft:stone")
        );
        let heightmap = world
            .get_heightmap_blocking(target)
            .expect("read transformed target heightmap")
            .expect("transformed target heightmap exists");
        assert_ne!(
            heightmap.values[usize::from(target_z) * 16 + usize::from(target_x)],
            0
        );
        assert!(
            world
                .get_chunk_blocking(target)
                .expect("read transformed target")
                .records
                .iter()
                .any(|record| {
                    record.key.tag == ChunkRecordTag::Version && record.value.as_ref() == b"\x2a"
                })
        );
        assert_eq!(
            world
                .get_biome_storage_blocking(target, -62)
                .expect("read transformed biome")
                .and_then(|storage| storage.biome_id_at(target_x, 2, target_z)),
            Some(42)
        );
    }
}

#[::core::prelude::v1::test]
fn pasted_chunk_record_survives_leveldb_reopen() {
    let unique = format!(
        "bmcbl-map-paste-reopen-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time after epoch")
            .as_nanos()
    );
    let world_path = std::env::temp_dir().join(unique);
    let database_path = world_path.join("db");
    let database =
        bedrock_leveldb::Db::open(&database_path, bedrock_leveldb::OpenOptions::default())
            .expect("initialize temporary world db");
    drop(database);
    let source = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    let target = ChunkPos {
        x: 7,
        z: -9,
        dimension: Dimension::Overworld,
    };
    let copied_chunk = CopiedChunkData {
        source,
        chunks: vec![CopiedChunkSnapshot {
            chunk: source,
            records: vec![ChunkRecord {
                key: ChunkKey::new(source, ChunkRecordTag::Version),
                value: Bytes::from_static(b"\x2a"),
            }],
            block_entities: Vec::new(),
            hardcoded_spawn_areas: Vec::new(),
        }],
    };
    {
        let world = BedrockWorld::open_blocking(
            &world_path,
            bedrock_world::OpenOptions {
                read_only: false,
                format: bedrock_world::WorldFormatHint::LevelDb,
                ..bedrock_world::OpenOptions::default()
            },
        )
        .expect("open temporary writable world");
        let guard = WriteGuard::confirmed(&world_path, "persistent paste test");
        paste_copied_chunk_blocking(
            &world,
            &copied_chunk,
            source,
            target,
            PasteTransform::default(),
            &guard,
            None,
            &mut |_| {},
        )
        .expect("paste into temporary world");
    }
    {
        let reopened =
            BedrockWorld::open_blocking(&world_path, bedrock_world::OpenOptions::default())
                .expect("reopen temporary world");
        let target_chunk = reopened
            .get_chunk_blocking(target)
            .expect("read persisted target chunk");
        assert!(target_chunk.records.iter().any(|record| {
            record.key.tag == ChunkRecordTag::Version && record.value.as_ref() == b"\x2a"
        }));
    }
    std::fs::remove_dir_all(&world_path).expect("remove temporary world");
}

#[::core::prelude::v1::test]
#[ignore = "requires BMCBL_REAL_WORLD and reads an installed Bedrock world"]
fn real_world_chunk_paste_survives_temporary_leveldb_reopen() {
    let source_world_path = std::env::var_os("BMCBL_REAL_WORLD")
        .map(PathBuf::from)
        .expect("BMCBL_REAL_WORLD must point to a Bedrock world root");
    let source_editor = MapWorldEditor::open_with_options(
        &source_world_path,
        bedrock_world::OpenOptions {
            read_only: true,
            format: bedrock_world::WorldFormatHint::LevelDb,
        },
    )
    .expect("open real source world read-only");
    let source = [(-14, -5), (16, 40), (0, 0), (-34, 4), (31, -1)]
        .into_iter()
        .map(|(x, z)| ChunkPos {
            x,
            z,
            dimension: Dimension::Overworld,
        })
        .find(|chunk| {
            source_editor
                .world()
                .get_chunk_blocking(*chunk)
                .is_ok_and(|data| {
                    data.records
                        .iter()
                        .any(|record| record.key.tag == ChunkRecordTag::SubChunkPrefix)
                })
        })
        .expect("a real candidate chunk must contain subchunk data");
    let copied = copy_chunks_blocking(&source_editor, source, vec![source], None, |_| {})
        .expect("copy real source chunk");
    let expected_records = copied.chunks[0].records.clone();

    let unique = format!(
        "bmcbl-real-map-paste-reopen-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time after epoch")
            .as_nanos()
    );
    let world_path = std::env::temp_dir().join(unique);
    let database = bedrock_leveldb::Db::open(
        world_path.join("db"),
        bedrock_leveldb::OpenOptions::default(),
    )
    .expect("initialize temporary target db");
    drop(database);
    let target = ChunkPos {
        x: 20_000,
        z: -20_000,
        dimension: Dimension::Overworld,
    };
    let transformed_target = ChunkPos {
        x: 20_001,
        z: -20_000,
        dimension: Dimension::Overworld,
    };
    {
        let target_world = BedrockWorld::open_blocking(
            &world_path,
            bedrock_world::OpenOptions {
                read_only: false,
                format: bedrock_world::WorldFormatHint::LevelDb,
            },
        )
        .expect("open temporary target world");
        let guard = WriteGuard::confirmed(&world_path, "real world paste persistence test");
        paste_copied_chunk_blocking(
            &target_world,
            &copied,
            source,
            target,
            PasteTransform::default(),
            &guard,
            None,
            &mut |_| {},
        )
        .expect("paste real chunk into temporary world");
        paste_copied_chunk_blocking(
            &target_world,
            &copied,
            source,
            transformed_target,
            PasteTransform::from_rotation(PasteRotation::Rotate180),
            &guard,
            None,
            &mut |_| {},
        )
        .expect("transform real chunk into temporary world");
    }
    {
        let reopened = BedrockWorld::open_blocking(
            &world_path,
            bedrock_world::OpenOptions {
                read_only: true,
                format: bedrock_world::WorldFormatHint::LevelDb,
            },
        )
        .expect("reopen temporary target world read-only");
        let actual = reopened
            .get_chunk_blocking(target)
            .expect("read persisted real target chunk");
        for expected in &expected_records {
            assert!(actual.records.iter().any(|record| {
                record.key.tag == expected.key.tag
                    && record.key.subchunk_y == expected.key.subchunk_y
                    && record.value == expected.value
            }));
        }
        let transformed = reopened
            .get_chunk_blocking(transformed_target)
            .expect("read persisted transformed target chunk");
        for version_tag in [
            ChunkRecordTag::Version,
            ChunkRecordTag::VersionOld,
            ChunkRecordTag::LegacyVersion,
        ] {
            for expected in expected_records
                .iter()
                .filter(|record| record.key.tag == version_tag)
            {
                assert!(transformed.records.iter().any(|record| {
                    record.key.tag == version_tag && record.value == expected.value
                }));
            }
        }
        assert!(
            transformed
                .records
                .iter()
                .any(|record| record.key.tag == ChunkRecordTag::Data3D)
        );
        assert!(
            transformed
                .records
                .iter()
                .any(|record| record.key.tag == ChunkRecordTag::SubChunkPrefix)
        );
        let renderer = bedrock_render::MapRenderer::new(
            Arc::new(reopened),
            bedrock_render::RenderPalette::default(),
        );
        let bake = renderer
            .bake_chunk_blocking(transformed_target, bedrock_render::BakeOptions::default())
            .expect("bake persisted transformed chunk");
        assert_eq!(bake.pos, transformed_target);
    }
    std::fs::remove_dir_all(&world_path).expect("remove temporary target world");
}
