// Keep canvas_base as the macro-page builder, but make the frontend transition atomic.
//
// A macro page and its original 128x128 tiles must never be visible in the same frame. The map
// image painter uploads screen images before tile images and applies a per-frame new-image
// budget. Showing both representations therefore exposes the macro image through whichever
// source tiles were deferred in that frame, producing the spreading rectangular flicker seen at
// low zoom.
//
// New macro pages are now prewarmed offscreen while their source tiles remain the only visible
// representation. Once the page has survived the warm-up interval, the same already-uploaded
// ImageId is moved to its real bounds and the covered source tiles are removed in one snapshot.
// Camera changes do not demote already-promoted pages: screen_image_bounds can transform the same
// immutable page across wheel zoom without rebuilding or alternating representations.
pub(super) use super::canvas_base::*;

use gpui::ImageId;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const MACRO_PAGE_PREWARM: Duration = Duration::from_millis(750);
const OFFSCREEN_PREWARM_ORIGIN: f32 = -1_000_000_000.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MacroPageViewportKey {
    scale_bits: u32,
    paint_bounds: Option<(i32, i32, i32, i32)>,
}

#[derive(Default)]
struct MacroPageTransitionState {
    promoted: BTreeSet<ImageId>,
    warming_since: BTreeMap<ImageId, Instant>,
    original_pages: BTreeMap<ImageId, ScreenPaintImage>,
    viewport_key: Option<MacroPageViewportKey>,
}

static MACRO_PAGE_TRANSITION_STATE: OnceLock<Mutex<MacroPageTransitionState>> = OnceLock::new();

fn macro_page_transition_state() -> &'static Mutex<MacroPageTransitionState> {
    MACRO_PAGE_TRANSITION_STATE.get_or_init(|| Mutex::new(MacroPageTransitionState::default()))
}

fn macro_page_viewport_key(
    snapshot: &TilePaintSnapshot,
    viewport: super::model::MapViewport,
) -> MacroPageViewportKey {
    MacroPageViewportKey {
        scale_bits: viewport.scale.to_bits(),
        paint_bounds: snapshot
            .paint_bounds
            .map(|bounds| (bounds.min_x, bounds.max_x, bounds.min_z, bounds.max_z)),
    }
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

fn offscreen_preload_page(page: &ScreenPaintImage) -> ScreenPaintImage {
    let mut page = page.clone();
    page.left = OFFSCREEN_PREWARM_ORIGIN;
    page.top = OFFSCREEN_PREWARM_ORIGIN;
    page
}

fn restore_base_snapshot(snapshot: &TilePaintSnapshot) -> TilePaintSnapshot {
    let state = macro_page_transition_state()
        .lock()
        .expect("macro page transition lock poisoned");
    let screen_images = snapshot
        .screen_images
        .iter()
        .map(|page| {
            state
                .original_pages
                .get(&page.image.id)
                .cloned()
                .unwrap_or_else(|| page.clone())
        })
        .collect::<Vec<_>>();
    TilePaintSnapshot {
        tiles: snapshot.tiles.clone(),
        screen_images: Arc::new(screen_images),
        debug_overlays: snapshot.debug_overlays.clone(),
        generation: snapshot.generation,
        estimated_bytes: snapshot.estimated_bytes,
        paint_bounds: snapshot.paint_bounds,
    }
}

fn classify_macro_pages(
    snapshot: &TilePaintSnapshot,
    viewport: super::model::MapViewport,
) -> (BTreeSet<ImageId>, BTreeSet<ImageId>, usize, bool, u128) {
    let current_ids = snapshot
        .screen_images
        .iter()
        .map(|page| page.image.id)
        .collect::<BTreeSet<_>>();
    let viewport_key = macro_page_viewport_key(snapshot, viewport);
    let now = Instant::now();
    let mut state = macro_page_transition_state()
        .lock()
        .expect("macro page transition lock poisoned");

    for page in snapshot.screen_images.iter() {
        state.original_pages.insert(page.image.id, page.clone());
    }
    state.promoted.retain(|id| current_ids.contains(id));
    state.warming_since.retain(|id, _| current_ids.contains(id));
    state.original_pages.retain(|id, _| current_ids.contains(id));

    let viewport_changed = state.viewport_key != Some(viewport_key);
    state.viewport_key = Some(viewport_key);

    // A camera transform changes only where an immutable macro page is painted. It must not
    // demote a page that was already visible; doing so is exactly the macro/source alternation
    // that caused wheel-zoom flicker. Only a new ImageId enters prewarm.
    for id in &current_ids {
        if !state.promoted.contains(id) {
            state.warming_since.entry(*id).or_insert(now);
        }
    }

    let ready_to_promote = state
        .warming_since
        .iter()
        .filter_map(|(id, started_at)| {
            (now.saturating_duration_since(*started_at) >= MACRO_PAGE_PREWARM).then_some(*id)
        })
        .collect::<Vec<_>>();
    let promoted_now = ready_to_promote.len();
    for id in ready_to_promote {
        state.warming_since.remove(&id);
        state.promoted.insert(id);
    }

    let promoted = current_ids
        .intersection(&state.promoted)
        .copied()
        .collect::<BTreeSet<_>>();
    let warming = current_ids
        .difference(&promoted)
        .copied()
        .collect::<BTreeSet<_>>();
    let minimum_warmup_ms = warming
        .iter()
        .filter_map(|id| state.warming_since.get(id))
        .map(|started_at| now.saturating_duration_since(*started_at).as_millis())
        .min()
        .unwrap_or(0);

    (
        promoted,
        warming,
        promoted_now,
        viewport_changed,
        minimum_warmup_ms,
    )
}

fn apply_atomic_macro_page_transition(
    snapshot: TilePaintSnapshot,
    tile_manager: &super::tile_state::RegionManager,
    viewport: super::model::MapViewport,
    layout: bedrock_render::RenderLayout,
) -> TilePaintSnapshot {
    if snapshot.screen_images.is_empty() {
        if let Ok(mut state) = macro_page_transition_state().lock() {
            state.promoted.clear();
            state.warming_since.clear();
            state.original_pages.clear();
            state.viewport_key = None;
        }
        return snapshot;
    }

    let (
        promoted_ids,
        warming_ids,
        promoted_now,
        viewport_changed,
        minimum_warmup_ms,
    ) = classify_macro_pages(&snapshot, viewport);
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

    // Promoted pages own their area, so overlapping source tiles are removed. Warming pages keep
    // all source tiles and are moved offscreen: the GPU still resolves their ImageIds, but users
    // can see only the source representation until the atomic promotion snapshot.
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

    let screen_images = snapshot
        .screen_images
        .iter()
        .map(|page| {
            if warming_ids.contains(&page.image.id) {
                offscreen_preload_page(page)
            } else {
                page.clone()
            }
        })
        .collect::<Vec<_>>();
    let estimated_bytes = tiles
        .iter()
        .map(|tile| tile.estimated_bytes)
        .sum::<usize>()
        .saturating_add(
            screen_images
                .iter()
                .map(|image| image.estimated_bytes)
                .sum::<usize>(),
        );

    tracing::debug!(
        generation = snapshot.generation,
        viewport_scale = viewport.scale,
        macro_pages = screen_images.len(),
        visible_macro_pages = promoted_ids.len(),
        offscreen_preload_pages = warming_ids.len(),
        promoted_now,
        viewport_changed,
        minimum_warmup_ms,
        required_warmup_ms = MACRO_PAGE_PREWARM.as_millis(),
        fallback_tiles,
        source_tiles_removed,
        submitted_tiles = tiles.len(),
        visible_dual_representation_pages = 0,
        "map_viewer macro_page_atomic_transition_applied"
    );

    TilePaintSnapshot {
        tiles: Arc::new(tiles),
        screen_images: Arc::new(screen_images),
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
    apply_atomic_macro_page_transition(snapshot, tile_manager, viewport, layout)
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
    let current = restore_base_snapshot(current);
    match super::canvas_base::patch_tile_paint_snapshot(
        &current,
        tile_manager,
        viewport,
        layout,
        diagnostics_open,
        paint_radius,
        changed_tiles,
        generation,
    ) {
        TilePaintSnapshotPatch::Patched(snapshot) => TilePaintSnapshotPatch::Patched(
            apply_atomic_macro_page_transition(snapshot, tile_manager, viewport, layout),
        ),
        TilePaintSnapshotPatch::Unchanged => TilePaintSnapshotPatch::Unchanged,
        TilePaintSnapshotPatch::Rebuild => TilePaintSnapshotPatch::Rebuild,
    }
}
