// Keep the existing stable macro-page implementation as the source of truth, then add a
// frontend transition layer around it. A macro page is painted underneath its original 128x128
// source tiles for a real-time warm-up interval. Only after the same ImageId and camera bounds
// remain stable for that interval may the page take ownership and remove its source tiles.
// This prevents one ReadyBatch or one wheel event from promoting a page after only a few
// milliseconds, before GPUI has had enough frames to make the 1024x1024 image resident.
pub(super) use super::canvas_base::*;

use gpui::ImageId;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const MACRO_PAGE_RESIDENCY_WARMUP: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MacroPageViewportKey {
    scale_bits: u32,
    paint_bounds: Option<(i32, i32, i32, i32)>,
}

#[derive(Default)]
struct MacroPageTransitionState {
    promoted: BTreeSet<ImageId>,
    warming_since: BTreeMap<ImageId, Instant>,
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

fn classify_macro_page_transition(
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

    let viewport_changed = state.viewport_key != Some(viewport_key);
    if viewport_changed {
        // A wheel step changes page geometry even when the macro ImageId is reused. Return every
        // page to warm-up so the already visible source tiles remain authoritative throughout
        // the camera transition instead of disappearing halfway through the zoom sequence.
        state.promoted.clear();
        state.warming_since.clear();
        state.viewport_key = Some(viewport_key);
    }

    state.promoted.retain(|id| current_ids.contains(id));
    state.warming_since.retain(|id, _| current_ids.contains(id));
    for id in &current_ids {
        if !state.promoted.contains(id) {
            state.warming_since.entry(*id).or_insert(now);
        }
    }

    let ready_to_promote = state
        .warming_since
        .iter()
        .filter_map(|(id, started_at)| {
            (now.saturating_duration_since(*started_at) >= MACRO_PAGE_RESIDENCY_WARMUP)
                .then_some(*id)
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

fn apply_macro_page_transition(
    snapshot: TilePaintSnapshot,
    tile_manager: &super::tile_state::RegionManager,
    viewport: super::model::MapViewport,
    layout: bedrock_render::RenderLayout,
) -> TilePaintSnapshot {
    if snapshot.screen_images.is_empty() {
        if let Ok(mut state) = macro_page_transition_state().lock() {
            state.promoted.clear();
            state.warming_since.clear();
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
    ) = classify_macro_page_transition(&snapshot, viewport);
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

    // Warming pages always retain the original tile images above the macro page. GPUI therefore
    // has at least 500 ms and many real paint opportunities to upload the macro page while the
    // user continues to see the previous stable representation.
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
        viewport_changed,
        minimum_warmup_ms,
        required_warmup_ms = MACRO_PAGE_RESIDENCY_WARMUP.as_millis(),
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
