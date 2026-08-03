pub(super) use super::canvas_legacy::*;

use gpui::{RenderImage, RenderImagePixelFormat, SharedString};
use std::collections::BTreeMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{
    Arc, Mutex, OnceLock, Weak,
    atomic::{AtomicUsize, Ordering},
};

const SUSTAINED_PAINT_RESOURCE_FAILURE_LIMIT: usize = 240;
static PAINT_RESOURCE_FAILURE_STREAK: AtomicUsize = AtomicUsize::new(0);

// Macro pages only change the GPU submission unit. Every source tile stays at one block per
// pixel, so zoom never switches to a blurry LOD or creates a second low-resolution cache.
const MACRO_PAGE_SCALE_THRESHOLD: f32 = 0.20;
const MACRO_PAGE_TILES_PER_AXIS: i32 = 8;
const MACRO_PAGE_MIN_SOURCE_TILES: usize = 48;
const MACRO_PAGE_INITIAL_COMPACTION_LIMIT: usize = 16;
const MACRO_PAGE_INCREMENTAL_COMPACTION_LIMIT: usize = 1;
const MACRO_PAGE_CACHE_PRUNE_THRESHOLD: usize = 1_024;

#[derive(Default)]
struct MacroPageImageCache {
    images: BTreeMap<u64, Weak<RenderImage>>,
}

fn macro_page_image_cache() -> &'static Mutex<MacroPageImageCache> {
    static CACHE: OnceLock<Mutex<MacroPageImageCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(MacroPageImageCache::default()))
}

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
    let mut screen_images = Vec::new();
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
    if macro_page_mode_enabled(viewport) {
        let compaction = compact_macro_pages(
            &mut tiles,
            &mut screen_images,
            tile_manager,
            viewport,
            layout,
            &std::collections::BTreeSet::new(),
            MACRO_PAGE_INITIAL_COMPACTION_LIMIT,
        );
        tracing::debug!(
            generation,
            viewport_scale = viewport.scale,
            macro_pages = screen_images.len(),
            individual_tiles = tiles.len(),
            compacted_pages = compaction.compacted_pages,
            reused_pages = compaction.reused_pages,
            deferred_unsettled_pages = compaction.deferred_unsettled_pages,
            deferred_sparse_pages = compaction.deferred_sparse_pages,
            "map_viewer macro_page_snapshot_built"
        );
    }
    debug_overlays
        .sort_unstable_by_key(|overlay| super::viewport::tile_paint_sort_key(overlay.coord));
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

    TilePaintSnapshot {
        tiles: Arc::new(tiles),
        screen_images: Arc::new(screen_images),
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
        return TilePaintSnapshotPatch::Rebuild;
    }
    if !macro_page_mode_enabled(viewport) && !current.screen_images.is_empty() {
        return TilePaintSnapshotPatch::Rebuild;
    }
    if changed_tiles.is_empty() {
        return TilePaintSnapshotPatch::Unchanged;
    }

    let mut tiles = current.tiles.as_ref().clone();
    let mut screen_images = current.screen_images.as_ref().clone();
    let mut debug_overlays = current.debug_overlays.as_ref().clone();
    let mut changed = false;
    let mut coords = changed_tiles.to_vec();
    coords.sort_unstable();
    coords.dedup();
    let mut dissolved_page_keys = std::collections::BTreeSet::new();

    for coord in coords {
        // A macro page is an immutable low-zoom base. When one tile or one chunk changes,
        // dissolve only that 8x8 page back into its current original tiles. This keeps edit
        // refreshes tile/chunk-granular and avoids re-uploading a 1024x1024 page for one change.
        if dissolve_macro_page_for_coord(
            &mut tiles,
            &mut screen_images,
            tile_manager,
            paint_bounds,
            layout,
            coord,
        ) {
            dissolved_page_keys.insert(macro_page_key(coord));
            changed = true;
        }
        changed |= patch_tile(&mut tiles, tile_manager, paint_bounds, coord);
        changed |= patch_overlay(
            &mut debug_overlays,
            tile_manager,
            paint_bounds,
            coord,
            diagnostics_open,
        );
    }

    let mut compaction = MacroPageCompactionStats::default();
    if macro_page_mode_enabled(viewport) {
        compaction = compact_macro_pages(
            &mut tiles,
            &mut screen_images,
            tile_manager,
            viewport,
            layout,
            &dissolved_page_keys,
            MACRO_PAGE_INCREMENTAL_COMPACTION_LIMIT,
        );
        changed |= compaction.compacted_pages > 0;
    }

    if !changed {
        return TilePaintSnapshotPatch::Unchanged;
    }

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
        generation,
        changed_tiles = changed_tiles.len(),
        dissolved_pages = dissolved_page_keys.len(),
        macro_pages = screen_images.len(),
        individual_tiles = tiles.len(),
        compacted_pages = compaction.compacted_pages,
        reused_pages = compaction.reused_pages,
        deferred_unsettled_pages = compaction.deferred_unsettled_pages,
        deferred_sparse_pages = compaction.deferred_sparse_pages,
        viewport_scale = viewport.scale,
        "map_viewer macro_page_snapshot_patched"
    );

    TilePaintSnapshotPatch::Patched(TilePaintSnapshot {
        tiles: Arc::new(tiles),
        screen_images: Arc::new(screen_images),
        debug_overlays: Arc::new(debug_overlays),
        generation,
        estimated_bytes,
        paint_bounds,
    })
}

fn macro_page_mode_enabled(viewport: super::model::MapViewport) -> bool {
    viewport.scale.is_finite() && viewport.scale <= MACRO_PAGE_SCALE_THRESHOLD
}

fn macro_page_key(coord: (i32, i32)) -> (i32, i32) {
    (
        coord.0.div_euclid(MACRO_PAGE_TILES_PER_AXIS),
        coord.1.div_euclid(MACRO_PAGE_TILES_PER_AXIS),
    )
}

fn macro_page_covers_coord(
    pages: &[ScreenPaintImage],
    layout: bedrock_render::RenderLayout,
    coord: (i32, i32),
) -> bool {
    const EPSILON: f32 = 0.01;
    pages.iter().any(|page| {
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
    })
}

fn dissolve_macro_page_for_coord(
    tiles: &mut Vec<super::tile_state::PaintTile>,
    screen_images: &mut Vec<ScreenPaintImage>,
    tile_manager: &super::tile_state::RegionManager,
    paint_bounds: Option<super::viewport::TileBounds>,
    layout: bedrock_render::RenderLayout,
    coord: (i32, i32),
) -> bool {
    let Some(page_index) = screen_images
        .iter()
        .position(|page| macro_page_covers_coord(std::slice::from_ref(page), layout, coord))
    else {
        return false;
    };

    let page = screen_images.remove(page_index);
    let page_key = macro_page_key(coord);
    let min_x = page_key.0.saturating_mul(MACRO_PAGE_TILES_PER_AXIS);
    let min_z = page_key.1.saturating_mul(MACRO_PAGE_TILES_PER_AXIS);
    for local_z in 0..MACRO_PAGE_TILES_PER_AXIS {
        for local_x in 0..MACRO_PAGE_TILES_PER_AXIS {
            let tile_coord = (
                min_x.saturating_add(local_x),
                min_z.saturating_add(local_z),
            );
            if !paint_bounds.is_some_and(|bounds| bounds.contains(tile_coord)) {
                continue;
            }
            let Some(tile) = tile_manager
                .entries
                .get(&tile_coord)
                .and_then(|entry| entry.image.as_ref())
            else {
                continue;
            };
            upsert_paint_tile(
                tiles,
                super::tile_state::PaintTile {
                    coord: tile_coord,
                    image: tile.image.clone(),
                    pixel_format: tile.pixel_format,
                    width: tile.width,
                    height: tile.height,
                    estimated_bytes: tile.estimated_bytes,
                },
            );
        }
    }
    tracing::debug!(
        page = ?page_key,
        page_image_id = ?page.image.id,
        restored_tiles = tiles
            .iter()
            .filter(|tile| macro_page_key(tile.coord) == page_key)
            .count(),
        "map_viewer macro_page_dissolved_for_tile_update"
    );
    true
}

fn upsert_paint_tile(
    tiles: &mut Vec<super::tile_state::PaintTile>,
    replacement: super::tile_state::PaintTile,
) {
    let key = super::viewport::tile_paint_sort_key(replacement.coord);
    match tiles.binary_search_by_key(&key, |tile| {
        super::viewport::tile_paint_sort_key(tile.coord)
    }) {
        Ok(index) => tiles[index] = replacement,
        Err(index) => tiles.insert(index, replacement),
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct MacroPageCompactionStats {
    compacted_pages: usize,
    reused_pages: usize,
    deferred_unsettled_pages: usize,
    deferred_sparse_pages: usize,
}

fn compact_macro_pages(
    tiles: &mut Vec<super::tile_state::PaintTile>,
    screen_images: &mut Vec<ScreenPaintImage>,
    tile_manager: &super::tile_state::RegionManager,
    viewport: super::model::MapViewport,
    layout: bedrock_render::RenderLayout,
    excluded_page_keys: &std::collections::BTreeSet<(i32, i32)>,
    max_pages: usize,
) -> MacroPageCompactionStats {
    let mut stats = MacroPageCompactionStats::default();
    if max_pages == 0 || tiles.len() < MACRO_PAGE_MIN_SOURCE_TILES {
        return stats;
    }

    let mut groups: BTreeMap<(i32, i32), Vec<super::tile_state::PaintTile>> = BTreeMap::new();
    for tile in tiles.iter() {
        let page_key = macro_page_key(tile.coord);
        if excluded_page_keys.contains(&page_key)
            || macro_page_covers_coord(screen_images, layout, tile.coord)
        {
            continue;
        }
        groups.entry(page_key).or_default().push(tile.clone());
    }

    let mut compacted_keys = Vec::new();
    for (page_key, source_tiles) in groups {
        if compacted_keys.len() >= max_pages {
            break;
        }
        if source_tiles.len() < MACRO_PAGE_MIN_SOURCE_TILES {
            stats.deferred_sparse_pages = stats.deferred_sparse_pages.saturating_add(1);
            continue;
        }
        if !macro_page_is_settled(tile_manager, page_key) {
            stats.deferred_unsettled_pages = stats.deferred_unsettled_pages.saturating_add(1);
            continue;
        }
        let Some((page, reused)) = build_macro_page(page_key, &source_tiles, viewport, layout) else {
            continue;
        };
        screen_images.push(page);
        compacted_keys.push(page_key);
        stats.compacted_pages = stats.compacted_pages.saturating_add(1);
        if reused {
            stats.reused_pages = stats.reused_pages.saturating_add(1);
        }
    }

    if compacted_keys.is_empty() {
        return stats;
    }
    tiles.retain(|tile| !compacted_keys.contains(&macro_page_key(tile.coord)));
    screen_images.sort_by(|left, right| {
        left.top
            .total_cmp(&right.top)
            .then_with(|| left.left.total_cmp(&right.left))
    });
    stats
}

fn macro_page_is_settled(
    tile_manager: &super::tile_state::RegionManager,
    page_key: (i32, i32),
) -> bool {
    let min_x = page_key.0.saturating_mul(MACRO_PAGE_TILES_PER_AXIS);
    let min_z = page_key.1.saturating_mul(MACRO_PAGE_TILES_PER_AXIS);
    for local_z in 0..MACRO_PAGE_TILES_PER_AXIS {
        for local_x in 0..MACRO_PAGE_TILES_PER_AXIS {
            let coord = (
                min_x.saturating_add(local_x),
                min_z.saturating_add(local_z),
            );
            let Some(entry) = tile_manager.entries.get(&coord) else {
                return false;
            };
            let settled = entry.state == super::tile_state::TileLoadState::Invalid
                || (entry.state == super::tile_state::TileLoadState::Loaded
                    && entry.image.is_some());
            if !settled {
                return false;
            }
        }
    }
    true
}

fn macro_page_content_key(
    page_key: (i32, i32),
    source_tiles: &[super::tile_state::PaintTile],
    page_width: u32,
    page_height: u32,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    "bmcbl-map-macro-page-v2".hash(&mut hasher);
    page_key.hash(&mut hasher);
    page_width.hash(&mut hasher);
    page_height.hash(&mut hasher);
    let mut ordered = source_tiles.iter().collect::<Vec<_>>();
    ordered.sort_unstable_by_key(|tile| tile.coord);
    for tile in ordered {
        tile.coord.hash(&mut hasher);
        tile.image.id.hash(&mut hasher);
        tile.width.hash(&mut hasher);
        tile.height.hash(&mut hasher);
    }
    hasher.finish()
}

fn cached_macro_page_image(
    content_key: u64,
    page_width: u32,
    page_height: u32,
    pixels: Vec<u8>,
) -> Option<(Arc<RenderImage>, bool)> {
    if let Some(image) = macro_page_image_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .images
        .get(&content_key)
        .and_then(Weak::upgrade)
    {
        return Some((image, true));
    }

    let image = Arc::new(
        RenderImage::from_raw_pixels(
            page_width,
            page_height,
            RenderImagePixelFormat::Rgba8,
            pixels,
        )
        .ok()?,
    );
    let mut cache = macro_page_image_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if cache.images.len() >= MACRO_PAGE_CACHE_PRUNE_THRESHOLD {
        cache.images.retain(|_, image| image.strong_count() > 0);
    }
    cache.images.insert(content_key, Arc::downgrade(&image));
    Some((image, false))
}

fn build_macro_page(
    page_key: (i32, i32),
    source_tiles: &[super::tile_state::PaintTile],
    viewport: super::model::MapViewport,
    layout: bedrock_render::RenderLayout,
) -> Option<(ScreenPaintImage, bool)> {
    let first = source_tiles.first()?;
    if first.pixel_format != Some(bedrock_render::TilePixelFormat::Rgba8)
        || first.width == 0
        || first.height == 0
    {
        return None;
    }
    let tile_width = first.width;
    let tile_height = first.height;
    let page_width = tile_width.checked_mul(MACRO_PAGE_TILES_PER_AXIS as u32)?;
    let page_height = tile_height.checked_mul(MACRO_PAGE_TILES_PER_AXIS as u32)?;
    let page_byte_len = usize::try_from(page_width)
        .ok()?
        .checked_mul(usize::try_from(page_height).ok()?)?
        .checked_mul(4)?;
    let mut pixels = vec![0u8; page_byte_len];
    let page_stride = usize::try_from(page_width).ok()?.checked_mul(4)?;
    let tile_stride = usize::try_from(tile_width).ok()?.checked_mul(4)?;
    let tile_rows = usize::try_from(tile_height).ok()?;

    for tile in source_tiles {
        if macro_page_key(tile.coord) != page_key
            || tile.pixel_format != Some(bedrock_render::TilePixelFormat::Rgba8)
            || tile.width != tile_width
            || tile.height != tile_height
            || tile.image.pixel_format(0) != Some(RenderImagePixelFormat::Rgba8)
        {
            return None;
        }
        let source = tile.image.as_bytes(0)?;
        if source.len() != tile_stride.checked_mul(tile_rows)? {
            return None;
        }
        let local_x = usize::try_from(tile.coord.0.rem_euclid(MACRO_PAGE_TILES_PER_AXIS)).ok()?;
        let local_z = usize::try_from(tile.coord.1.rem_euclid(MACRO_PAGE_TILES_PER_AXIS)).ok()?;
        let destination_x = local_x.checked_mul(usize::try_from(tile_width).ok()?)?;
        let destination_y = local_z.checked_mul(tile_rows)?;
        let destination_x_bytes = destination_x.checked_mul(4)?;
        for row in 0..tile_rows {
            let source_start = row.checked_mul(tile_stride)?;
            let source_end = source_start.checked_add(tile_stride)?;
            let destination_start = destination_y
                .checked_add(row)?
                .checked_mul(page_stride)?
                .checked_add(destination_x_bytes)?;
            let destination_end = destination_start.checked_add(tile_stride)?;
            pixels
                .get_mut(destination_start..destination_end)?
                .copy_from_slice(source.get(source_start..source_end)?);
        }
    }

    let render_range = super::viewport::region_render_range_for_viewport(viewport, layout)?;
    let min_coord = (
        page_key.0.saturating_mul(MACRO_PAGE_TILES_PER_AXIS),
        page_key.1.saturating_mul(MACRO_PAGE_TILES_PER_AXIS),
    );
    let tile_rect = super::viewport::tile_paint_rect(
        viewport,
        layout,
        render_range,
        min_coord.0,
        min_coord.1,
    )?;
    let estimated_bytes = pixels.len();
    let content_key = macro_page_content_key(page_key, source_tiles, page_width, page_height);
    let (image, reused) =
        cached_macro_page_image(content_key, page_width, page_height, pixels)?;

    tracing::debug!(
        page = ?page_key,
        source_tiles = source_tiles.len(),
        page_width,
        page_height,
        estimated_bytes,
        content_key,
        image_id = ?image.id,
        reused,
        "map_viewer macro_page_compacted"
    );

    Some((
        ScreenPaintImage {
            image,
            source_viewport: viewport,
            left: tile_rect.left,
            top: tile_rect.top,
            width: tile_rect.width() * MACRO_PAGE_TILES_PER_AXIS as f32,
            height: tile_rect.height() * MACRO_PAGE_TILES_PER_AXIS as f32,
            estimated_bytes,
        },
        reused,
    ))
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
