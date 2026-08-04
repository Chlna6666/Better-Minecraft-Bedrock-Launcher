// Keep the existing stable macro-page implementation as the source of truth, then add a
// frontend-safe fallback layer around it. A newly built macro page is subject to GPUI's
// per-frame image-upload budget; removing its 128x128 source tiles in the same snapshot creates
// a rectangular hole until the 1024x1024 page becomes resident. Retaining the original tiles
// underneath the page makes the transition atomic from the user's point of view.
pub(super) use super::canvas_base::*;

use std::collections::BTreeMap;
use std::sync::Arc;

fn macro_page_covers_coord(
    page: &ScreenPaintImage,
    layout: bedrock_render::RenderLayout,
    coord: (i32, i32),
) -> bool {
    const EPSILON: f32 = 0.01;
    let Some(render_range) = super::viewport::region_render_range_for_viewport(
        page.source_viewport,
        layout,
    ) else {
        return false;
    };
    let Some(rect) = super::viewport::tile_paint_rect(
        page.source_viewport,
        layout,
        render_range,
        coord.0,
        coord.1,
    ) else {
        return false;
    };

    rect.left >= page.left - EPSILON
        && rect.top >= page.top - EPSILON
        && rect.right <= page.left + page.width + EPSILON
        && rect.bottom <= page.top + page.height + EPSILON
}

fn retain_macro_page_source_fallbacks(
    snapshot: TilePaintSnapshot,
    tile_manager: &super::tile_state::RegionManager,
    viewport: super::model::MapViewport,
    layout: bedrock_render::RenderLayout,
) -> TilePaintSnapshot {
    if snapshot.screen_images.is_empty() {
        return snapshot;
    }

    let mut tiles = snapshot
        .tiles
        .iter()
        .cloned()
        .map(|tile| (tile.coord, tile))
        .collect::<BTreeMap<_, _>>();
    let original_tile_count = tiles.len();

    for (&coord, entry) in &tile_manager.entries {
        if !snapshot
            .paint_bounds
            .is_some_and(|paint_bounds| paint_bounds.contains(coord))
            || tiles.contains_key(&coord)
            || !snapshot
                .screen_images
                .iter()
                .any(|page| macro_page_covers_coord(page, layout, coord))
        {
            continue;
        }
        let Some(tile) = entry.image.as_ref() else {
            continue;
        };
        tiles.insert(
            coord,
            super::tile_state::PaintTile {
                coord,
                image: tile.image.clone(),
                pixel_format: tile.pixel_format,
                width: tile.width,
                height: tile.height,
                estimated_bytes: tile.estimated_bytes,
            },
        );
    }

    let fallback_tiles = tiles.len().saturating_sub(original_tile_count);
    if fallback_tiles == 0 {
        return snapshot;
    }

    let mut tiles = tiles.into_values().collect::<Vec<_>>();
    tiles.sort_unstable_by_key(|tile| super::viewport::tile_paint_sort_key(tile.coord));
    let estimated_bytes = tiles
        .iter()
        .map(|tile| tile.estimated_bytes)
        .sum::<usize>()
        .saturating_add(
            snapshot
                .screen_images
                .iter()
                .map(|image| image.estimated_bytes)
                .sum::<usize>(),
        );

    tracing::debug!(
        generation = snapshot.generation,
        viewport_scale = viewport.scale,
        macro_pages = snapshot.screen_images.len(),
        fallback_tiles,
        submitted_tiles = tiles.len(),
        "map_viewer macro_page_source_fallbacks_retained"
    );

    TilePaintSnapshot {
        tiles: Arc::new(tiles),
        screen_images: snapshot.screen_images,
        debug_overlays: snapshot.debug_overlays,
        generation: snapshot.generation,
        estimated_bytes,
        paint_bounds: snapshot.paint_bounds,
    }
}

pub(super) fn build_tile_paint_snapshot(
    tile_manager: &super::tile_state::RegionManager,
    viewport: super::model::MapViewport,
    layout: bedrock_render::RenderLayout,
    diagnostics_open: bool,
    paint_radius: i32,
    generation: u64,
) -> TilePaintSnapshot {
    let snapshot = super::canvas_base::build_tile_paint_snapshot(
        tile_manager,
        viewport,
        layout,
        diagnostics_open,
        paint_radius,
        generation,
    );
    retain_macro_page_source_fallbacks(snapshot, tile_manager, viewport, layout)
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
    match super::canvas_base::patch_tile_paint_snapshot(
        current,
        tile_manager,
        viewport,
        layout,
        diagnostics_open,
        paint_radius,
        changed_tiles,
        generation,
    ) {
        TilePaintSnapshotPatch::Patched(snapshot) => TilePaintSnapshotPatch::Patched(
            retain_macro_page_source_fallbacks(snapshot, tile_manager, viewport, layout),
        ),
        TilePaintSnapshotPatch::Unchanged => TilePaintSnapshotPatch::Unchanged,
        TilePaintSnapshotPatch::Rebuild => TilePaintSnapshotPatch::Rebuild,
    }
}
