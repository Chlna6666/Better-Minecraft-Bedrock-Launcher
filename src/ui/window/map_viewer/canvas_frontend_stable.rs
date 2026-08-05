// Keep the existing stable macro-page implementation as the source of truth, then add a
// frontend transition layer around it. A newly built macro page first paints underneath its
// original 128x128 source tiles. After the same pending macro-page set survives one complete
// snapshot cycle, ownership is promoted to those macro pages and the overlapping source tiles
// are removed together. This prevents the two representations from alternating indefinitely.
pub(super) use super::canvas_base::*;

use gpui::ImageId;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Default)]
struct MacroPageTransitionState {
    promoted: BTreeSet<ImageId>,
    last_pending_signature: BTreeSet<ImageId>,
}

static MACRO_PAGE_TRANSITION_STATE: OnceLock<Mutex<MacroPageTransitionState>> = OnceLock::new();

fn macro_page_transition_state() -> &'static Mutex<MacroPageTransitionState> {
    MACRO_PAGE_TRANSITION_STATE.get_or_init(|| Mutex::new(MacroPageTransitionState::default()))
}

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

fn classify_macro_page_transition(
    screen_images: &[ScreenPaintImage],
) -> (BTreeSet<ImageId>, BTreeSet<ImageId>, usize) {
    let current_ids = screen_images
        .iter()
        .map(|page| page.image.id)
        .collect::<BTreeSet<_>>();
    let mut state = macro_page_transition_state()
        .lock()
        .expect("macro page transition lock poisoned");

    // Pages that left the current snapshot cannot own source tiles in this snapshot. Forgetting
    // them also makes a later re-entry warm up safely again after GPUI may have evicted the image.
    state.promoted.retain(|id| current_ids.contains(id));
    let pending = current_ids
        .difference(&state.promoted)
        .copied()
        .collect::<BTreeSet<_>>();

    let mut promoted_now = 0usize;
    if !pending.is_empty() && pending == state.last_pending_signature {
        promoted_now = pending.len();
        state.promoted.extend(pending.iter().copied());
        state.last_pending_signature.clear();
    } else {
        state.last_pending_signature = pending;
    }

    let promoted = current_ids
        .intersection(&state.promoted)
        .copied()
        .collect::<BTreeSet<_>>();
    let warming = current_ids
        .difference(&promoted)
        .copied()
        .collect::<BTreeSet<_>>();
    (promoted, warming, promoted_now)
}

fn apply_macro_page_transition(
    snapshot: TilePaintSnapshot,
    tile_manager: &super::tile_state::RegionManager,
    viewport: super::model::MapViewport,
    layout: bedrock_render::RenderLayout,
) -> TilePaintSnapshot {
    if snapshot.screen_images.is_empty() {
        if let Ok(mut state) = macro_page_transition_state().lock() {
            state.promoted.clear();
            state.last_pending_signature.clear();
        }
        return snapshot;
    }

    let (promoted_ids, warming_ids, promoted_now) =
        classify_macro_page_transition(snapshot.screen_images.as_ref());
    let promoted_pages = snapshot
        .screen_images
        .iter()
        .filter(|page| promoted_ids.contains(&page.image.id))
        .collect::<Vec<_>>();
    let warming_pages = snapshot
        .screen_images
        .iter()
        .filter(|page| warming_ids.contains(&page.image.id))
        .collect::<Vec<_>>();

    let mut tiles = snapshot
        .tiles
        .iter()
        .filter(|tile| {
            !promoted_pages
                .iter()
                .any(|page| macro_page_covers_coord(page, layout, tile.coord))
        })
        .cloned()
        .map(|tile| (tile.coord, tile))
        .collect::<BTreeMap<_, _>>();
    let source_tiles_removed = snapshot.tiles.len().saturating_sub(tiles.len());
    let original_tile_count = tiles.len();

    // Only pages still in their warm-up phase keep original tiles above the macro image. Once a
    // page group is promoted, it stays macro-owned until that ImageId leaves the snapshot or a
    // tile/chunk update dissolves the page in canvas_base.
    for (&coord, entry) in &tile_manager.entries {
        if !snapshot
            .paint_bounds
            .is_some_and(|paint_bounds| paint_bounds.contains(coord))
            || tiles.contains_key(&coord)
            || !warming_pages
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
        promoted_pages = promoted_ids.len(),
        warming_pages = warming_ids.len(),
        promoted_now,
        fallback_tiles,
        source_tiles_removed,
        submitted_tiles = tiles.len(),
        "map_viewer macro_page_transition_applied"
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
    apply_macro_page_transition(snapshot, tile_manager, viewport, layout)
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
            apply_macro_page_transition(snapshot, tile_manager, viewport, layout),
        ),
        TilePaintSnapshotPatch::Unchanged => TilePaintSnapshotPatch::Unchanged,
        TilePaintSnapshotPatch::Rebuild => TilePaintSnapshotPatch::Rebuild,
    }
}
