pub(super) use super::helpers_legacy::*;

use super::model::{MapViewport, RETAIN_RADIUS, RenderCpuBudget};
use bedrock_render::RenderLayout;

const MIN_REGION_CACHE_ENTRIES: usize = 131_072;
const MAX_REGION_CACHE_ENTRIES: usize = 262_144;
const MIN_REGION_CACHE_BYTES: usize = 96 * 1024 * 1024;
const MAX_REGION_CACHE_BYTES: usize = 384 * 1024 * 1024;
const TARGET_RESIDENT_TILE_IMAGES: usize = 4_096;

pub(super) fn tile_cache_memory_limit(cpu_budget: RenderCpuBudget) -> usize {
    cpu_budget
        .thread_count()
        .saturating_mul(16_384)
        .clamp(MIN_REGION_CACHE_ENTRIES, MAX_REGION_CACHE_ENTRIES)
}

pub(super) fn ui_tile_memory_budget_bytes(
    viewport: MapViewport,
    texture_layout: RenderLayout,
) -> usize {
    let tile_size = texture_layout
        .tile_size()
        .map_or(128usize, |size| size as usize)
        .max(1);
    let tile_bytes = tile_size.saturating_mul(tile_size).saturating_mul(4);
    let visible_tiles =
        visible_tile_count(viewport, tile_size, RETAIN_RADIUS).min(TARGET_RESIDENT_TILE_IMAGES);
    let visible_budget = visible_tiles.saturating_mul(tile_bytes);
    let available_budget =
        usize::try_from(super::helpers_legacy::available_system_memory_bytes() / 24)
            .unwrap_or(MIN_REGION_CACHE_BYTES);

    visible_budget
        .max(available_budget)
        .clamp(MIN_REGION_CACHE_BYTES, MAX_REGION_CACHE_BYTES)
}

fn visible_tile_count(viewport: MapViewport, tile_size: usize, radius: i32) -> usize {
    let scale = viewport.scale.max(0.01);
    let tile_screen_size = tile_size as f32 * scale;
    let width = (viewport.width / tile_screen_size).ceil().max(1.0) as usize;
    let height = (viewport.height / tile_screen_size).ceil().max(1.0) as usize;
    let margin = usize::try_from(radius.max(0))
        .unwrap_or(0)
        .saturating_mul(2);
    width
        .saturating_add(margin)
        .saturating_mul(height.saturating_add(margin))
}
