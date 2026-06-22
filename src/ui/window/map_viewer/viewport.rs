use super::model::*;
use super::prelude::*;
use super::tile_state::*;

pub(super) fn viewport_block_bounds(
    viewport: MapViewport,
    layout: RenderLayout,
) -> (i32, i32, i32, i32) {
    if let Some(range) = region_render_range_for_viewport(viewport, layout) {
        return (
            range.min_chunk_x.saturating_mul(16),
            range.min_chunk_z.saturating_mul(16),
            range.max_chunk_x.saturating_add(1).saturating_mul(16),
            range.max_chunk_z.saturating_add(1).saturating_mul(16),
        );
    }
    let min_map_x = (-viewport.offset_x) / viewport.scale;
    let max_map_x = (viewport.width - viewport.offset_x) / viewport.scale;
    let min_map_z = (-viewport.offset_y) / viewport.scale;
    let max_map_z = (viewport.height - viewport.offset_y) / viewport.scale;
    (
        map_pixel_to_block(min_map_x.min(max_map_x), layout),
        map_pixel_to_block(min_map_z.min(max_map_z), layout),
        map_pixel_to_block(min_map_x.max(max_map_x), layout),
        map_pixel_to_block(min_map_z.max(max_map_z), layout),
    )
}

pub(super) fn draw_grid_lines(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    step: i32,
    color: Hsla,
    width: Pixels,
    window: &mut Window,
) {
    let (min_x, min_z, max_x, max_z) = viewport_block_bounds(viewport, layout);
    let step = step.max(1);
    let start_x = min_x.div_euclid(step).saturating_mul(step);
    let end_x = max_x.saturating_add(step);
    let start_z = min_z.div_euclid(step).saturating_mul(step);
    let end_z = max_z.saturating_add(step);
    let mut builder = PathBuilder::stroke(width);
    let mut x = start_x;
    while x <= end_x {
        let screen_x = screen_x_for_block(bounds, viewport, layout, x);
        builder.move_to(point(px(screen_x), bounds.top()));
        builder.line_to(point(px(screen_x), bounds.bottom()));
        x = x.saturating_add(step);
        if x == i32::MAX {
            break;
        }
    }
    let mut z = start_z;
    while z <= end_z {
        let screen_y = screen_y_for_block(bounds, viewport, layout, z);
        builder.move_to(point(bounds.left(), px(screen_y)));
        builder.line_to(point(bounds.right(), px(screen_y)));
        z = z.saturating_add(step);
        if z == i32::MAX {
            break;
        }
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

pub(super) fn draw_axes(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    window: &mut Window,
) {
    let origin_x = screen_x_for_block(bounds, viewport, layout, 0);
    if origin_x >= bounds.left() / px(1.0) && origin_x <= bounds.right() / px(1.0) {
        let mut builder = PathBuilder::stroke(px(2.0));
        builder.move_to(point(px(origin_x), bounds.top()));
        builder.line_to(point(px(origin_x), bounds.bottom()));
        if let Ok(path) = builder.build() {
            window.paint_path(path, rgb(0xff5656));
        }
    }
    let origin_y = screen_y_for_block(bounds, viewport, layout, 0);
    if origin_y >= bounds.top() / px(1.0) && origin_y <= bounds.bottom() / px(1.0) {
        let mut builder = PathBuilder::stroke(px(2.0));
        builder.move_to(point(bounds.left(), px(origin_y)));
        builder.line_to(point(bounds.right(), px(origin_y)));
        if let Ok(path) = builder.build() {
            window.paint_path(path, rgb(0x59a5ff));
        }
    }
}

pub(super) fn draw_ruler(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    colors: ThemeColors,
    window: &mut Window,
) {
    let blocks = ruler_blocks(viewport.scale, layout);
    let ruler_pixels = blocks as f32 * layout.pixels_per_block as f32
        / layout.blocks_per_pixel as f32
        * viewport.scale;
    let x = bounds.right() / px(1.0) - ruler_pixels - 24.0;
    let y = bounds.bottom() / px(1.0) - 36.0;
    if ruler_pixels <= 0.0 || x <= bounds.left() / px(1.0) {
        return;
    }
    window.paint_quad(fill(
        Bounds {
            origin: point(px(x - 8.0), px(y - 18.0)),
            size: size(px(ruler_pixels + 16.0), px(30.0)),
        },
        Hsla {
            a: 0.58,
            ..colors.surface
        },
    ));
    let mut builder = PathBuilder::stroke(px(2.0));
    builder.move_to(point(px(x), px(y)));
    builder.line_to(point(px(x + ruler_pixels), px(y)));
    builder.move_to(point(px(x), px(y - 5.0)));
    builder.line_to(point(px(x), px(y + 5.0)));
    builder.move_to(point(px(x + ruler_pixels), px(y - 5.0)));
    builder.line_to(point(px(x + ruler_pixels), px(y + 5.0)));
    if let Ok(path) = builder.build() {
        window.paint_path(path, colors.text_primary);
    }
}

pub(super) fn adjusted_grid_step(
    base_step: i32,
    min_value: i32,
    max_value: i32,
    max_lines: i32,
) -> i32 {
    let mut step = base_step.max(1);
    while max_value.saturating_sub(min_value) / step > max_lines {
        step = step.saturating_mul(2).max(step + 1);
    }
    step
}

pub(super) fn screen_x_for_block(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    block_x: i32,
) -> f32 {
    bounds.left() / px(1.0)
        + region_render_range_for_viewport(viewport, layout).map_or_else(
            || viewport.offset_x + block_to_map_pixel(block_x, layout) * viewport.scale,
            |range| range.screen_x_for_block(block_x),
        )
}

pub(super) fn screen_y_for_block(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    block_z: i32,
) -> f32 {
    bounds.top() / px(1.0)
        + region_render_range_for_viewport(viewport, layout).map_or_else(
            || viewport.offset_y + block_to_map_pixel(block_z, layout) * viewport.scale,
            |range| range.screen_y_for_block(block_z),
        )
}

pub(super) fn viewport_screen_for_block(
    viewport: MapViewport,
    layout: RenderLayout,
    block_x: i32,
    block_z: i32,
) -> Option<(f32, f32)> {
    region_render_range_for_viewport(viewport, layout).map_or_else(
        || {
            Some((
                viewport.offset_x + block_to_map_pixel(block_x, layout) * viewport.scale,
                viewport.offset_y + block_to_map_pixel(block_z, layout) * viewport.scale,
            ))
        },
        |range| {
            Some((
                range.screen_x_for_block(block_x),
                range.screen_y_for_block(block_z),
            ))
        },
    )
}

pub(super) fn ruler_blocks(scale: f32, layout: RenderLayout) -> i32 {
    let candidates = [16, 32, 64, 128, 256, 512, 1024, 2048, 4096];
    let mut selected = candidates[0];
    for candidate in candidates {
        let pixels = candidate as f32 * layout.pixels_per_block as f32
            / layout.blocks_per_pixel as f32
            * scale;
        if (90.0..=190.0).contains(&pixels) {
            selected = candidate;
            break;
        }
        if pixels < 90.0 {
            selected = candidate;
        }
    }
    selected
}

impl TilePaintRect {
    pub(super) fn width(self) -> f32 {
        self.right - self.left
    }

    pub(super) fn height(self) -> f32 {
        self.bottom - self.top
    }

    pub(super) fn to_bounds(self, bounds: Bounds<Pixels>) -> Option<Bounds<Pixels>> {
        let bounds_left = bounds.left() / px(1.0);
        let bounds_top = bounds.top() / px(1.0);
        let left = (bounds_left + self.left).floor();
        let top = (bounds_top + self.top).floor();
        let right = (bounds_left + self.right).ceil();
        let bottom = (bounds_top + self.bottom).ceil();
        let clip_left = bounds.left() / px(1.0);
        let clip_top = bounds.top() / px(1.0);
        let clip_right = bounds.right() / px(1.0);
        let clip_bottom = bounds.bottom() / px(1.0);
        if right <= left
            || bottom <= top
            || right < clip_left
            || bottom < clip_top
            || left > clip_right
            || top > clip_bottom
        {
            return None;
        }
        Some(Bounds {
            origin: point(px(left), px(top)),
            size: size(px(right - left), px(bottom - top)),
        })
    }
}

impl MapRenderRange {
    pub(super) fn tile_bounds(self) -> TileBounds {
        TileBounds {
            min_x: self.min_chunk_x.div_euclid(self.chunks_per_tile),
            max_x: self.max_chunk_x.div_euclid(self.chunks_per_tile),
            min_z: self.min_chunk_z.div_euclid(self.chunks_per_tile),
            max_z: self.max_chunk_z.div_euclid(self.chunks_per_tile),
        }
    }

    fn screen_x_for_block(self, block_x: i32) -> f32 {
        let chunk_x = block_x.div_euclid(16);
        let block_in_chunk = block_x - chunk_x.saturating_mul(16);
        self.render_origin_x
            + (chunk_x - self.min_chunk_x) as f32 * self.chunk_screen_size
            + block_in_chunk as f32 * self.block_screen_size
    }

    fn screen_y_for_block(self, block_z: i32) -> f32 {
        let chunk_z = block_z.div_euclid(16);
        let block_in_chunk = block_z - chunk_z.saturating_mul(16);
        self.render_origin_y
            + (chunk_z - self.min_chunk_z) as f32 * self.chunk_screen_size
            + block_in_chunk as f32 * self.block_screen_size
    }
}

pub(super) fn tile_render_range_for_viewport(
    viewport: MapViewport,
    layout: RenderLayout,
) -> Option<MapRenderRange> {
    let chunks_per_tile = i32::try_from(layout.chunks_per_tile).ok()?.max(1);
    let map_pixels_per_block = layout.pixels_per_block as f32 / layout.blocks_per_pixel as f32;
    if !map_pixels_per_block.is_finite() || map_pixels_per_block <= 0.0 {
        return None;
    }
    let block_screen_size = (map_pixels_per_block * viewport.scale).max(0.001);
    let chunk_screen_size = (block_screen_size * 16.0).max(0.001);
    let (min_chunk_x, render_origin_x) =
        aligned_camera_chunk(viewport.offset_x, viewport.width, chunk_screen_size);
    let (min_chunk_z, render_origin_y) =
        aligned_camera_chunk(viewport.offset_y, viewport.height, chunk_screen_size);
    let chunk_w = ((viewport.width - render_origin_x) / chunk_screen_size)
        .ceil()
        .max(1.0) as i32;
    let chunk_h = ((viewport.height - render_origin_y) / chunk_screen_size)
        .ceil()
        .max(1.0) as i32;
    Some(MapRenderRange {
        min_chunk_x,
        max_chunk_x: min_chunk_x.saturating_add(chunk_w.saturating_sub(1)),
        min_chunk_z,
        max_chunk_z: min_chunk_z.saturating_add(chunk_h.saturating_sub(1)),
        render_origin_x,
        render_origin_y,
        chunk_screen_size,
        block_screen_size,
        chunks_per_tile,
    })
}

pub(super) fn region_render_range_for_viewport(
    viewport: MapViewport,
    layout: RenderLayout,
) -> Option<MapRenderRange> {
    tile_render_range_for_viewport(viewport, layout)
}

pub(super) fn aligned_camera_chunk(
    offset: f32,
    camera_size: f32,
    chunk_screen_size: f32,
) -> (i32, f32) {
    let mut min_chunk = ((-offset) / chunk_screen_size).floor() as i32;
    let mut render_origin = offset + min_chunk as f32 * chunk_screen_size;
    if render_origin >= 0.0 {
        min_chunk = min_chunk.saturating_sub(1);
        render_origin -= chunk_screen_size;
    }
    if render_origin + chunk_screen_size > camera_size {
        min_chunk = min_chunk.saturating_sub(1);
        render_origin -= chunk_screen_size;
    }
    (min_chunk, render_origin)
}

pub(super) fn tile_paint_sort_key(
    coord: (i32, i32),
    range: MapRenderRange,
) -> (i64, i64, i32, i32) {
    let bounds = range.tile_bounds();
    (
        i64::from(coord.0) - i64::from(bounds.min_x),
        i64::from(coord.1) - i64::from(bounds.min_z),
        coord.0,
        coord.1,
    )
}

pub(super) fn tile_coords_for_paint_order(bounds: TileBounds) -> Vec<(i32, i32)> {
    if bounds.min_x > bounds.max_x || bounds.min_z > bounds.max_z {
        return Vec::new();
    }
    let mut coords = Vec::new();
    for x in bounds.min_x..=bounds.max_x {
        for z in bounds.min_z..=bounds.max_z {
            coords.push((x, z));
        }
    }
    coords
}

pub(super) fn tile_paint_rect(
    viewport: MapViewport,
    layout: RenderLayout,
    render_range: MapRenderRange,
    tile_x: i32,
    tile_z: i32,
) -> Option<TilePaintRect> {
    let chunks_per_tile = i32::try_from(layout.chunks_per_tile).ok()?.max(1);
    let tile_min_chunk_x = tile_x.checked_mul(chunks_per_tile)?;
    let tile_min_chunk_z = tile_z.checked_mul(chunks_per_tile)?;
    let tile_screen_size = render_range.chunk_screen_size * chunks_per_tile as f32;
    let left = render_range.render_origin_x
        + (tile_min_chunk_x - render_range.min_chunk_x) as f32 * render_range.chunk_screen_size;
    let top = render_range.render_origin_y
        + (tile_min_chunk_z - render_range.min_chunk_z) as f32 * render_range.chunk_screen_size;
    let right = left + tile_screen_size;
    let bottom = top + tile_screen_size;
    if right < 0.0 || bottom < 0.0 || left > viewport.width || top > viewport.height {
        return None;
    }
    Some(TilePaintRect {
        left: left.floor() - TILE_SEAM_BLEED_PX,
        top: top.floor() - TILE_SEAM_BLEED_PX,
        right: right.ceil() + TILE_SEAM_BLEED_PX,
        bottom: bottom.ceil() + TILE_SEAM_BLEED_PX,
    })
}

pub(super) fn clamp_tile_span(min_value: &mut i32, max_value: &mut i32, center: i32) {
    let span = max_value.saturating_sub(*min_value).saturating_add(1);
    if span <= MAX_TILE_SPAN_PER_AXIS {
        return;
    }
    let half = MAX_TILE_SPAN_PER_AXIS / 2;
    *min_value = center.saturating_sub(half);
    *max_value = center.saturating_add(half);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct TileBounds {
    pub(super) min_x: i32,
    pub(super) max_x: i32,
    pub(super) min_z: i32,
    pub(super) max_z: i32,
}

impl TileBounds {
    pub(super) fn expand(self, radius: i32) -> Self {
        Self {
            min_x: self.min_x.saturating_sub(radius),
            max_x: self.max_x.saturating_add(radius),
            min_z: self.min_z.saturating_sub(radius),
            max_z: self.max_z.saturating_add(radius),
        }
    }
}

pub(super) fn visible_tile_bounds_for_render_range(
    render_range: MapRenderRange,
    center: (i32, i32),
) -> Option<TileBounds> {
    let chunks_per_tile = render_range.chunks_per_tile.max(1);
    let mut bounds = TileBounds {
        min_x: render_range.min_chunk_x.div_euclid(chunks_per_tile),
        max_x: render_range.max_chunk_x.div_euclid(chunks_per_tile),
        min_z: render_range.min_chunk_z.div_euclid(chunks_per_tile),
        max_z: render_range.max_chunk_z.div_euclid(chunks_per_tile),
    };
    clamp_tile_span(&mut bounds.min_x, &mut bounds.max_x, center.0);
    clamp_tile_span(&mut bounds.min_z, &mut bounds.max_z, center.1);
    (bounds.min_x <= bounds.max_x && bounds.min_z <= bounds.max_z).then_some(bounds)
}

pub(super) fn visible_tile_bounds_for_viewport(
    viewport: MapViewport,
    layout: RenderLayout,
    center: (i32, i32),
) -> Option<TileBounds> {
    let tile_size = layout_tile_size(layout);
    if !tile_size.is_finite() || tile_size <= 0.0 || !viewport.scale.is_finite() {
        return None;
    }
    let scale = viewport.scale.max(0.001);
    let raw_min_map_x = (-viewport.offset_x) / scale;
    let raw_max_map_x = (viewport.width - viewport.offset_x) / scale;
    let raw_min_map_z = (-viewport.offset_y) / scale;
    let raw_max_map_z = (viewport.height - viewport.offset_y) / scale;
    let min_map_x = raw_min_map_x.min(raw_max_map_x);
    let max_map_x = raw_min_map_x.max(raw_max_map_x);
    let min_map_z = raw_min_map_z.min(raw_max_map_z);
    let max_map_z = raw_min_map_z.max(raw_max_map_z);
    let edge_epsilon_x = ((max_map_x - min_map_x) * 0.000_001).clamp(0.001, 0.25);
    let edge_epsilon_z = ((max_map_z - min_map_z) * 0.000_001).clamp(0.001, 0.25);
    let min_tile_x = (min_map_x / tile_size).floor() as i32;
    let max_tile_x = ((max_map_x - edge_epsilon_x) / tile_size).floor() as i32;
    let min_tile_z = (min_map_z / tile_size).floor() as i32;
    let max_tile_z = ((max_map_z - edge_epsilon_z) / tile_size).floor() as i32;
    let mut bounds = TileBounds {
        min_x: min_tile_x,
        max_x: max_tile_x.max(min_tile_x),
        min_z: min_tile_z,
        max_z: max_tile_z.max(min_tile_z),
    };
    clamp_tile_span(&mut bounds.min_x, &mut bounds.max_x, center.0);
    clamp_tile_span(&mut bounds.min_z, &mut bounds.max_z, center.1);
    (bounds.min_x <= bounds.max_x && bounds.min_z <= bounds.max_z).then_some(bounds)
}

pub(super) fn tile_bounds_from_coords(coords: &[(i32, i32)]) -> Option<TileBounds> {
    let first = coords.first().copied()?;
    let mut bounds = TileBounds {
        min_x: first.0,
        max_x: first.0,
        min_z: first.1,
        max_z: first.1,
    };
    for &(x, z) in &coords[1..] {
        bounds.min_x = bounds.min_x.min(x);
        bounds.max_x = bounds.max_x.max(x);
        bounds.min_z = bounds.min_z.min(z);
        bounds.max_z = bounds.max_z.max(z);
    }
    Some(bounds)
}

pub(super) fn tile_coords_from_bounds(bounds: TileBounds) -> Vec<(i32, i32)> {
    if bounds.min_x > bounds.max_x || bounds.min_z > bounds.max_z {
        return Vec::new();
    }
    let mut coords = Vec::new();
    for z in bounds.min_z..=bounds.max_z {
        for x in bounds.min_x..=bounds.max_x {
            coords.push((x, z));
        }
    }
    coords
}

pub(super) fn collect_circular_tile_coords(
    visible: TileBounds,
    expanded: TileBounds,
    radius: i32,
    center: (i32, i32),
) -> Vec<(i32, i32)> {
    if expanded.min_x > expanded.max_x || expanded.min_z > expanded.max_z {
        return Vec::new();
    }
    let radius_squared = i64::from(radius.max(0)).saturating_mul(i64::from(radius.max(0)));
    let mut coords = Vec::new();
    for z in expanded.min_z..=expanded.max_z {
        for x in expanded.min_x..=expanded.max_x {
            if radius <= 0 || squared_distance_to_tile_bounds(x, z, visible) <= radius_squared {
                coords.push((x, z));
            }
        }
    }
    ordered_tiles_for_viewport(coords, center)
}

pub(super) fn squared_distance_to_tile_bounds(x: i32, z: i32, bounds: TileBounds) -> i64 {
    let dx = if x < bounds.min_x {
        i64::from(bounds.min_x) - i64::from(x)
    } else if x > bounds.max_x {
        i64::from(x) - i64::from(bounds.max_x)
    } else {
        0
    };
    let dz = if z < bounds.min_z {
        i64::from(bounds.min_z) - i64::from(z)
    } else if z > bounds.max_z {
        i64::from(z) - i64::from(bounds.max_z)
    } else {
        0
    };
    dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz))
}

pub(super) fn ordered_tiles_for_viewport(
    mut coords: Vec<(i32, i32)>,
    center: (i32, i32),
) -> Vec<(i32, i32)> {
    sort_tiles_center_first(&mut coords, center);
    coords
}

pub(super) fn sort_tiles_center_first(coords: &mut [(i32, i32)], center: (i32, i32)) {
    coords.sort_by_key(|coord| tile_distance_sort_key(*coord, center));
}

pub(super) fn tile_distance_sort_key(
    coord: (i32, i32),
    center: (i32, i32),
) -> (i64, i64, i64, i32, i32) {
    let dx = i64::from(coord.0) - i64::from(center.0);
    let dz = i64::from(coord.1) - i64::from(center.1);
    let absolute_x = dx.abs();
    let absolute_z = dz.abs();
    (
        absolute_x.max(absolute_z),
        dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)),
        absolute_x.saturating_add(absolute_z),
        coord.1,
        coord.0,
    )
}

pub(super) fn select_manifest_probe_tiles(
    visible_tiles: &[(i32, i32)],
    prefetch_tiles: &[(i32, i32)],
    center: (i32, i32),
    scanned_tiles: &BTreeSet<(i32, i32)>,
) -> Vec<(i32, i32)> {
    let visible_bounds = tile_bounds_from_coords(visible_tiles);
    let visible_set = visible_tiles.iter().copied().collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    let mut candidates = Vec::new();

    for coord in visible_tiles.iter().copied() {
        if scanned_tiles.contains(&coord) || !seen.insert(coord) {
            continue;
        }
        let (ring, distance_squared, manhattan, z, x) = tile_distance_sort_key(coord, center);
        candidates.push((0_u8, ring, distance_squared, 0_i64, manhattan, z, x, coord));
    }
    for coord in prefetch_tiles.iter().copied() {
        if visible_set.contains(&coord) || scanned_tiles.contains(&coord) || !seen.insert(coord) {
            continue;
        }
        let (ring, distance_squared, manhattan, z, x) = tile_distance_sort_key(coord, center);
        let visible_distance = visible_bounds
            .map(|bounds| squared_distance_to_tile_bounds(coord.0, coord.1, bounds))
            .unwrap_or(distance_squared);
        candidates.push((
            1_u8,
            ring,
            distance_squared,
            visible_distance,
            manhattan,
            z,
            x,
            coord,
        ));
    }
    candidates.sort_by_key(|candidate| {
        (
            candidate.0,
            candidate.1,
            candidate.2,
            candidate.3,
            candidate.4,
            candidate.5,
            candidate.6,
        )
    });

    candidates
        .into_iter()
        .take(TILE_MANIFEST_PROBE_BATCH_TILES)
        .map(|candidate| candidate.7)
        .collect()
}

pub(super) fn tile_center_distance_squared(coord: (i32, i32), center: (i32, i32)) -> i64 {
    let dx = i64::from(coord.0) - i64::from(center.0);
    let dz = i64::from(coord.1) - i64::from(center.1);
    dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz))
}

pub(super) fn max_tile_distance_squared(coords: &[(i32, i32)], center: (i32, i32)) -> Option<i64> {
    coords
        .iter()
        .map(|(x, z)| {
            let dx = i64::from(*x) - i64::from(center.0);
            let dz = i64::from(*z) - i64::from(center.1);
            dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz))
        })
        .max()
}

pub(super) fn layout_tile_size(layout: RenderLayout) -> f32 {
    layout.tile_size().unwrap_or(DEFAULT_TILE_SIZE as u32) as f32
}

pub(super) fn block_to_map_pixel(value: i32, layout: RenderLayout) -> f32 {
    value as f32 * layout.pixels_per_block as f32 / layout.blocks_per_pixel as f32
}

pub(super) fn map_pixel_to_block(value: f32, layout: RenderLayout) -> i32 {
    (value * layout.blocks_per_pixel as f32 / layout.pixels_per_block as f32).floor() as i32
}

pub(super) fn coordinate_text(block_x: i32, block_z: i32) -> String {
    format!("X {block_x} · Z {block_z}")
}
