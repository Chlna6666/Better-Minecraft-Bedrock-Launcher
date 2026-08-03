pub(super) use super::canvas_legacy::*;

use gpui::SharedString;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

const SUSTAINED_PAINT_RESOURCE_FAILURE_LIMIT: usize = 240;
static PAINT_RESOURCE_FAILURE_STREAK: AtomicUsize = AtomicUsize::new(0);

pub(super) fn take_map_tile_paint_resources_unavailable() -> bool {
    if !super::canvas_legacy::take_map_tile_paint_resources_unavailable() {
        PAINT_RESOURCE_FAILURE_STREAK.store(0, Ordering::Release);
        return false;
    }

    let streak = PAINT_RESOURCE_FAILURE_STREAK
        .fetch_add(1, Ordering::AcqRel)
        .saturating_add(1);
    if streak < SUSTAINED_PAINT_RESOURCE_FAILURE_LIMIT {
        return false;
    }

    PAINT_RESOURCE_FAILURE_STREAK.store(0, Ordering::Release);
    true
}

pub(super) fn build_tile_paint_snapshot(
    tile_manager: &super::tile_state::RegionManager,
    viewport: super::model::MapViewport,
    layout: bedrock_render::RenderLayout,
    diagnostics_open: bool,
    paint_radius: i32,
    generation: u64,
) -> TilePaintSnapshot {
    let paint_bounds =
        super::viewport::paint_tile_bounds_for_viewport(viewport, layout, paint_radius);
    let paint_capacity = paint_bounds
        .map(super::viewport::tile_bounds_count)
        .unwrap_or(0)
        .min(tile_manager.loaded_count());
    let mut tiles = Vec::with_capacity(paint_capacity);
    let mut debug_overlays = Vec::new();

    for (&coord, entry) in &tile_manager.entries {
        if !paint_bounds.is_some_and(|bounds| bounds.contains(coord)) {
            continue;
        }
        if let Some(tile) = entry.image.as_ref() {
            tiles.push(super::tile_state::PaintTile {
                coord,
                image: tile.image.clone(),
                pixel_format: tile.pixel_format,
                width: tile.width,
                height: tile.height,
                estimated_bytes: tile.estimated_bytes,
            });
        } else if diagnostics_open
            && matches!(
                entry.state,
                super::tile_state::TileLoadState::Failed
                    | super::tile_state::TileLoadState::Invalid
            )
        {
            debug_overlays.push(TileDebugOverlay {
                coord,
                label: if entry.state == super::tile_state::TileLoadState::Invalid {
                    SharedString::from("空")
                } else {
                    SharedString::from("失败")
                },
            });
        }
    }

    tiles.sort_unstable_by_key(|tile| super::viewport::tile_paint_sort_key(tile.coord));
    debug_overlays
        .sort_unstable_by_key(|overlay| super::viewport::tile_paint_sort_key(overlay.coord));
    let estimated_bytes = tiles
        .iter()
        .map(|tile| tile.estimated_bytes)
        .sum::<usize>();

    TilePaintSnapshot {
        tiles: Arc::new(tiles),
        screen_images: Arc::new(Vec::new()),
        debug_overlays: Arc::new(debug_overlays),
        generation,
        estimated_bytes,
        paint_bounds,
    }
}

pub(super) fn patch_tile_paint_snapshot(
    current: &TilePaintSnapshot,
    tile_manager: &super::tile_state::RegionManager,
    viewport: super::model::MapViewport,
    layout: bedrock_render::RenderLayout,
    diagnostics_open: bool,
    paint_radius: i32,
    changed_tiles: &[(i32, i32)],
    generation: u64,
) -> TilePaintSnapshotPatch {
    let paint_bounds =
        super::viewport::paint_tile_bounds_for_viewport(viewport, layout, paint_radius);
    if current.paint_bounds != paint_bounds {
        // A zoom or camera move changes the drawable world range. Rebuild from the
        // retained tile cache so the new snapshot contains every already-loaded tile
        // inside the new range and no off-screen GPU image references from the old range.
        return TilePaintSnapshotPatch::Rebuild;
    }
    if changed_tiles.is_empty() {
        return TilePaintSnapshotPatch::Unchanged;
    }

    let mut tiles = current.tiles.as_ref().clone();
    let mut debug_overlays = current.debug_overlays.as_ref().clone();
    let mut changed = false;
    let mut coords = changed_tiles.to_vec();
    coords.sort_unstable();
    coords.dedup();

    for coord in coords {
        changed |= patch_tile(&mut tiles, tile_manager, paint_bounds, coord);
        changed |= patch_overlay(
            &mut debug_overlays,
            tile_manager,
            paint_bounds,
            coord,
            diagnostics_open,
        );
    }

    if !changed {
        return TilePaintSnapshotPatch::Unchanged;
    }

    let estimated_bytes = tiles
        .iter()
        .map(|tile| tile.estimated_bytes)
        .sum::<usize>()
        .saturating_add(
            current
                .screen_images
                .iter()
                .map(|image| image.estimated_bytes)
                .sum::<usize>(),
        );

    TilePaintSnapshotPatch::Patched(TilePaintSnapshot {
        tiles: Arc::new(tiles),
        screen_images: current.screen_images.clone(),
        debug_overlays: Arc::new(debug_overlays),
        generation,
        estimated_bytes,
        paint_bounds,
    })
}

fn patch_tile(
    tiles: &mut Vec<super::tile_state::PaintTile>,
    tile_manager: &super::tile_state::RegionManager,
    paint_bounds: Option<super::viewport::TileBounds>,
    coord: (i32, i32),
) -> bool {
    let key = super::viewport::tile_paint_sort_key(coord);
    let existing = tiles.binary_search_by_key(&key, |tile| {
        super::viewport::tile_paint_sort_key(tile.coord)
    });
    let replacement = paint_bounds
        .filter(|bounds| bounds.contains(coord))
        .and_then(|_| tile_manager.entries.get(&coord))
        .and_then(|entry| entry.image.as_ref())
        .map(|tile| super::tile_state::PaintTile {
            coord,
            image: tile.image.clone(),
            pixel_format: tile.pixel_format,
            width: tile.width,
            height: tile.height,
            estimated_bytes: tile.estimated_bytes,
        });

    match (existing, replacement) {
        (Ok(index), Some(replacement)) => {
            let current = &tiles[index];
            if Arc::ptr_eq(&current.image, &replacement.image)
                && current.pixel_format == replacement.pixel_format
                && current.width == replacement.width
                && current.height == replacement.height
                && current.estimated_bytes == replacement.estimated_bytes
            {
                return false;
            }
            tiles[index] = replacement;
            true
        }
        (Ok(index), None) => {
            tiles.remove(index);
            true
        }
        (Err(index), Some(replacement)) => {
            tiles.insert(index, replacement);
            true
        }
        (Err(_), None) => false,
    }
}

fn patch_overlay(
    overlays: &mut Vec<TileDebugOverlay>,
    tile_manager: &super::tile_state::RegionManager,
    paint_bounds: Option<super::viewport::TileBounds>,
    coord: (i32, i32),
    diagnostics_open: bool,
) -> bool {
    let key = super::viewport::tile_paint_sort_key(coord);
    let existing = overlays.binary_search_by_key(&key, |overlay| {
        super::viewport::tile_paint_sort_key(overlay.coord)
    });
    let replacement = paint_bounds
        .filter(|bounds| bounds.contains(coord))
        .and_then(|_| tile_manager.entries.get(&coord))
        .and_then(|entry| {
            if !diagnostics_open
                || !matches!(
                    entry.state,
                    super::tile_state::TileLoadState::Failed
                        | super::tile_state::TileLoadState::Invalid
                )
            {
                return None;
            }
            Some(TileDebugOverlay {
                coord,
                label: if entry.state == super::tile_state::TileLoadState::Invalid {
                    SharedString::from("空")
                } else {
                    SharedString::from("失败")
                },
            })
        });

    match (existing, replacement) {
        (Ok(index), Some(replacement)) => {
            if overlays[index].label == replacement.label {
                return false;
            }
            overlays[index] = replacement;
            true
        }
        (Ok(index), None) => {
            overlays.remove(index);
            true
        }
        (Err(index), Some(replacement)) => {
            overlays.insert(index, replacement);
            true
        }
        (Err(_), None) => false,
    }
}