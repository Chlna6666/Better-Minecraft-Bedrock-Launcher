use super::editor::*;
use super::helpers::*;
use super::interactions::*;
use super::layout::{hud_stack_rects, top_toolbar_layout};
use super::mcstructure;
use super::model::*;
use super::overlays::*;
use super::paint::*;
use super::panels::*;
use super::players::*;
use super::prelude::*;
use super::tile_cache::*;
use super::tile_manifest::*;
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
fn minimum_zoom_allows_wider_tile_overview() {
    let layout = web_relief_render_layout();
    let mut viewport = test_viewport(4096.0, 4096.0, 1920.0, 1080.0);
    viewport.scale = MIN_VIEWPORT_SCALE;
    let center = viewport.center_tile(layout);
    let bounds = visible_tile_bounds_for_viewport(viewport, layout, center).expect("bounds");
    let width = bounds.max_x - bounds.min_x + 1;
    let height = bounds.max_z - bounds.min_z + 1;

    assert!(width > 32, "expected wider overview than old 32-tile cap");
    assert!(width <= MAX_TILE_SPAN_PER_AXIS);
    assert!(height <= MAX_TILE_SPAN_PER_AXIS);
}

#[::core::prelude::v1::test]
fn zoom_input_clamps_to_expanded_minimum_scale() {
    let scale = parse_zoom_scale("1").expect("zoom scale");

    assert_eq!(scale, MIN_VIEWPORT_SCALE);
}

#[::core::prelude::v1::test]
fn tile_rect_uses_chunk_aligned_render_origin() {
    let layout = web_relief_render_layout();
    let viewport = test_viewport(0.0, 0.0, 128.0, 128.0);
    let range = tile_render_range_for_viewport(viewport, layout).expect("render range");
    let rect = tile_paint_rect(viewport, layout, range, 0, 0).expect("visible tile");

    assert_eq!(rect.left, -TILE_SEAM_BLEED_PX);
    assert_eq!(rect.top, -TILE_SEAM_BLEED_PX);
    assert_eq!(rect.right, DEFAULT_TILE_SIZE + TILE_SEAM_BLEED_PX);
    assert_eq!(rect.bottom, DEFAULT_TILE_SIZE + TILE_SEAM_BLEED_PX);
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

    let snapshot = build_tile_paint_snapshot(&manager, viewport, layout, false, 1);

    assert_eq!(snapshot.tiles.len(), 1);
    assert!(Arc::ptr_eq(&snapshot.tiles[0].image, &source_image));
}

#[::core::prelude::v1::test]
fn tile_paint_snapshot_only_keeps_visible_tiles() {
    let layout = web_relief_render_layout();
    let viewport = test_viewport(0.0, 0.0, 512.0, 512.0);
    let mut manager = RegionManager::default();
    manager.mark_loaded((0, 0), test_tile([1, 1, 1, 255]));
    manager.mark_loaded((64, 64), test_tile([2, 2, 2, 255]));

    let snapshot = build_tile_paint_snapshot(&manager, viewport, layout, false, 1);

    assert_eq!(snapshot.tiles.len(), 1);
    assert_eq!(snapshot.tiles[0].coord, (0, 0));
}

#[::core::prelude::v1::test]
fn tile_paint_snapshot_records_current_paint_bounds() {
    let layout = web_relief_render_layout();
    let viewport = test_viewport(0.0, 0.0, 512.0, 512.0);
    let manager = RegionManager::default();

    let snapshot = build_tile_paint_snapshot(&manager, viewport, layout, false, 1);

    assert_eq!(
        snapshot.paint_bounds,
        visible_tile_bounds_for_viewport(viewport, layout, viewport.center_tile(layout))
    );
}

#[::core::prelude::v1::test]
fn tile_paint_snapshot_uses_precise_visible_bounds() {
    let layout = web_relief_render_layout();
    let viewport = test_viewport(0.0, 0.0, 512.0, 512.0);
    let mut manager = RegionManager::default();
    manager.mark_loaded((0, 0), test_tile([1, 1, 1, 255]));
    manager.mark_loaded((1, 0), test_tile([2, 2, 2, 255]));

    let snapshot = build_tile_paint_snapshot(&manager, viewport, layout, false, 1);

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
    let snapshot = build_tile_paint_snapshot(&manager, viewport, layout, false, 1);
    manager.mark_loaded((0, 0), test_tile([2, 2, 2, 255]));
    let source_image = manager
        .entries
        .get(&(0, 0))
        .and_then(|entry| entry.image.as_ref())
        .map(|tile| tile.image.clone())
        .expect("replacement test tile");

    let patched =
        patch_tile_paint_snapshot(&snapshot, &manager, viewport, layout, false, &[(0, 0)], 2);

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
    let snapshot = build_tile_paint_snapshot(&manager, viewport, layout, false, 1);
    manager.mark_loaded((1, 0), test_tile([2, 2, 2, 255]));

    let patched =
        patch_tile_paint_snapshot(&snapshot, &manager, viewport, layout, false, &[(1, 0)], 2);

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
    let snapshot = build_tile_paint_snapshot(&manager, viewport, layout, false, 1);
    viewport.offset_x = -512.0;
    manager.mark_loaded((2, 0), test_tile([2, 2, 2, 255]));

    let patched =
        patch_tile_paint_snapshot(&snapshot, &manager, viewport, layout, false, &[(2, 0)], 2);

    assert!(matches!(patched, TilePaintSnapshotPatch::Rebuild));
}

#[::core::prelude::v1::test]
fn tile_paint_snapshot_patch_replaces_existing_coord_without_duplicate() {
    let layout = web_relief_render_layout();
    let mut viewport = test_viewport(0.0, 0.0, 512.0, 512.0);
    viewport.scale = 0.5;
    let mut manager = RegionManager::default();
    manager.mark_loaded((0, 0), test_tile([3, 3, 3, 255]));
    manager.mark_loaded((1, 0), test_tile([4, 4, 4, 255]));
    let paint_bounds =
        visible_tile_bounds_for_viewport(viewport, layout, viewport.center_tile(layout));
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
        debug_overlays: Arc::new(Vec::new()),
        generation: 1,
        estimated_bytes: 8,
        paint_bounds,
    };

    let patched =
        patch_tile_paint_snapshot(&snapshot, &manager, viewport, layout, false, &[(0, 0)], 2);

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
    let snapshot = build_tile_paint_snapshot(&manager, viewport, layout, false, 1);
    manager.mark_invalid((0, 0), SharedString::from("empty"));

    let patched =
        patch_tile_paint_snapshot(&snapshot, &manager, viewport, layout, false, &[(0, 0)], 2);

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
        CanvasPointerMoveAction::ReleaseStaleCaptures
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
    assert!(wide.show_dock_commands);
    assert_eq!(wide.overflow_count, 0);

    let minimum = top_toolbar_layout(920.0);
    assert!(!minimum.show_modes);
    assert!(!minimum.show_y_controls);
    assert!(minimum.show_zoom_controls);
    assert!(!minimum.show_dock_commands);
    assert!(minimum.overflow_count >= 10);

    let medium = top_toolbar_layout(1080.0);
    assert!(medium.show_modes);
    assert!(!medium.show_dock_commands);
    assert!(medium.overflow_count >= 3);

    let small = top_toolbar_layout(480.0);
    assert!(!small.show_modes);
    assert!(!small.show_zoom_controls);
    assert!(small.overflow_count >= 12);
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
    assert_eq!(rect.left(), px(282.0));
    assert_eq!(rect.top(), px(58.0));
    assert_eq!(rect.size.width, px(572.0));
    assert_eq!(rect.size.height, px(506.0));

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
    assert_eq!(collapsed.left(), px(43.0));
    assert!(collapsed.size.width >= px(MIN_CENTER_WIDTH));
}

#[::core::prelude::v1::test]
fn hud_stack_rectangles_do_not_overlap() {
    let (ruler, coord) = hud_stack_rects(640.0, 360.0, true);
    let ruler = ruler.expect("ruler visible");
    assert!(ruler.bottom() <= coord.top() - px(8.0));
    assert!(coord.right() <= px(640.0));
    assert!(coord.bottom() <= px(360.0));
}

#[::core::prelude::v1::test]
fn paint_order_uses_bedrockmap_column_then_row_order() {
    let layout = web_relief_render_layout();
    let viewport = test_viewport(0.0, 0.0, 128.0, 128.0);
    let range = tile_render_range_for_viewport(viewport, layout).expect("render range");
    let mut coords = vec![(0, 1), (0, 0), (-1, 0), (-1, -1)];

    coords.sort_by_key(|coord| tile_paint_sort_key(*coord, range));

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
    coords.sort_by_key(|coord| tile_paint_sort_key(*coord, range));

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
fn interactive_session_config_culls_missing_chunks() {
    let config = interactive_map_render_session_config(
        std::path::Path::new("world"),
        RenderBackend::Auto,
        RenderGpuBackend::Auto,
    );

    assert!(config.cull_missing_chunks);
}

#[::core::prelude::v1::test]
fn interactive_tile_batch_defaults_are_conservative() {
    assert_eq!(RENDER_UI_BATCH_TILES, 8);
    assert_eq!(FIRST_VISIBLE_BATCH_LIMIT, 4);
    assert_eq!(DRAG_VISIBLE_BATCH_LIMIT, 4);
    assert_eq!(RENDER_STREAM_GROUP_TILES, 4);
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
}

#[::core::prelude::v1::test]
fn visible_render_batch_size_expands_for_large_overviews() {
    assert_eq!(
        visible_render_batch_size(8, OVERVIEW_VISIBLE_TILE_THRESHOLD - 1, false, true),
        FIRST_VISIBLE_BATCH_LIMIT
    );
    assert_eq!(
        visible_render_batch_size(8, OVERVIEW_VISIBLE_TILE_THRESHOLD, false, true),
        OVERVIEW_FIRST_VISIBLE_BATCH_LIMIT
    );
    assert_eq!(
        visible_render_batch_size(8, OVERVIEW_VISIBLE_TILE_THRESHOLD, false, false),
        OVERVIEW_VISIBLE_BATCH_LIMIT
    );
}

#[::core::prelude::v1::test]
fn map_cpu_budget_defaults_to_sixty_percent_with_interactive_cap() {
    let budget = RenderCpuBudget::default();
    assert_eq!(budget.percent, 60);
    let threads = budget.thread_count();
    assert!(threads >= 1);
    assert!(
        threads
            <= RenderCpuBudget::available_threads()
                .saturating_sub(1)
                .clamp(1, 8)
    );
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
fn tile_order_uses_center_ring_sort_key() {
    let mut coords = vec![(2, 0), (1, 1), (0, 1), (0, 0), (1, 0)];
    sort_tiles_center_first(&mut coords, (0, 0));

    assert_eq!(coords, vec![(0, 0), (1, 0), (0, 1), (1, 1), (2, 0)]);
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
        vec![(0, 0), (0, -1), (-1, 0), (1, 0), (0, 1), (1, 1), (2, 0)]
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
        vec![(0, -1), (-1, 0), (1, 0), (0, 1), (1, 1), (2, 0)]
    );
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
fn quick_write_confirmation_label_requires_same_action() {
    let chunk = ChunkPos {
        x: 3,
        z: -4,
        dimension: Dimension::Overworld,
    };
    let pending = QuickWriteAction::DeleteCurrentChunk(chunk);

    assert_eq!(
        confirming_quick_label(
            Some(&pending),
            pending.clone(),
            "删除当前 chunk（清空为空气）"
        ),
        "确认删除当前 chunk（清空为空气）"
    );
    assert_eq!(
        confirming_quick_label(
            Some(&pending),
            QuickWriteAction::DeleteCurrentChunkActors(chunk),
            "删除当前 chunk 实体"
        ),
        "删除当前 chunk 实体"
    );
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
