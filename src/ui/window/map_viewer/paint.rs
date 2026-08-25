use super::model::*;
use super::prelude::*;
use super::selection::{exact_selection_chunks, selection_chunks_are_rectangular};
use super::tile_state::MapRenderRange;
use super::viewport::*;
use std::cell::RefCell;
use std::collections::HashSet;

const ENTITY_EXACT_LOD_MIN_CHUNK_PX: f32 = 18.0;
const ENTITY_CHUNK_LOD_MIN_CHUNK_PX: f32 = 10.0;
const ENTITY_SCREEN_CLUSTER_CELL_PX: f32 = 24.0;
const ENTITY_AVATAR_UPLOAD_BUDGET: usize = 32;
const ENTITY_VISIBILITY_MARGIN_PX: f32 = 52.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntityLodMode {
    Exact,
    ChunkType,
    ScreenCluster,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct EntityChunkClusterKey {
    chunk_x: i32,
    chunk_z: i32,
    image_identity: usize,
}

#[derive(Clone, Copy, Debug, Default)]
struct EntityChunkClusterAccum {
    sum_block_x: f64,
    sum_block_z: f64,
    count: u32,
    representative_index: usize,
}

#[derive(Clone, Copy, Debug, Default)]
struct EntityScreenClusterCell {
    generation: u32,
    sum_block_x: f64,
    sum_block_z: f64,
    count: u32,
    representative_index: usize,
    representative_has_avatar: bool,
}

#[derive(Default)]
struct EntityLodScratch {
    chunk_clusters: HashMap<EntityChunkClusterKey, EntityChunkClusterAccum>,
    chunk_cluster_order: Vec<EntityChunkClusterKey>,
    screen_cells: Vec<EntityScreenClusterCell>,
    active_screen_cells: Vec<usize>,
    screen_generation: u32,
}

impl EntityLodScratch {
    fn begin_chunk_frame(&mut self) {
        self.chunk_clusters.clear();
        self.chunk_cluster_order.clear();
    }

    fn begin_screen_frame(&mut self, required_cells: usize) -> u32 {
        self.screen_generation = self.screen_generation.wrapping_add(1);
        if self.screen_generation == 0 {
            for cell in &mut self.screen_cells {
                cell.generation = 0;
            }
            self.screen_generation = 1;
        }
        if self.screen_cells.len() < required_cells {
            self.screen_cells
                .resize(required_cells, EntityScreenClusterCell::default());
        }
        self.active_screen_cells.clear();
        self.screen_generation
    }
}

thread_local! {
    static ENTITY_LOD_SCRATCH: RefCell<EntityLodScratch> = RefCell::new(EntityLodScratch::default());
}

pub(super) fn draw_map_canvas(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    overlays: OverlayOptions,
    colors: ThemeColors,
    window: &mut Window,
) {
    let block_bounds = viewport_block_bounds(viewport, layout);
    let tile_step = grid_step_for_block_bounds(TILE_WORLD_BLOCKS, block_bounds, 140);
    draw_grid_lines(
        bounds,
        viewport,
        layout,
        tile_step,
        Hsla {
            a: 0.20,
            ..colors.text_primary
        },
        px(1.25),
        window,
    );

    let chunk_pixels =
        16.0 * layout.pixels_per_block as f32 / layout.blocks_per_pixel as f32 * viewport.scale;
    if overlays.dense_grid || chunk_pixels >= 18.0 {
        let chunk_step = grid_step_for_block_bounds(16, block_bounds, 280);
        draw_grid_lines(
            bounds,
            viewport,
            layout,
            chunk_step,
            Hsla {
                a: 0.16,
                ..colors.accent
            },
            px(1.0),
            window,
        );
    }
    if overlays.axis {
        draw_axes(bounds, viewport, layout, window);
    }
    if overlays.ruler {
        draw_ruler(bounds, viewport, layout, colors, window);
    }
}

pub(super) fn grid_step_for_block_bounds(
    base_step: i32,
    block_bounds: (i32, i32, i32, i32),
    max_lines: i32,
) -> i32 {
    adjusted_grid_step(base_step, block_bounds.0, block_bounds.2, max_lines).max(
        adjusted_grid_step(base_step, block_bounds.1, block_bounds.3, max_lines),
    )
}

fn entity_chunk_screen_size_px(viewport: MapViewport, layout: RenderLayout) -> f32 {
    16.0 * layout.pixels_per_block as f32 / layout.blocks_per_pixel as f32 * viewport.scale
}

fn entity_lod_mode(viewport: MapViewport, layout: RenderLayout) -> EntityLodMode {
    let chunk_px = entity_chunk_screen_size_px(viewport, layout);
    if !chunk_px.is_finite() || chunk_px >= ENTITY_EXACT_LOD_MIN_CHUNK_PX {
        EntityLodMode::Exact
    } else if chunk_px >= ENTITY_CHUNK_LOD_MIN_CHUNK_PX {
        EntityLodMode::ChunkType
    } else {
        EntityLodMode::ScreenCluster
    }
}

fn entity_avatar_arc<'a>(
    point: &EntityOverlayPoint,
    entity_avatar_pool: &'a BTreeMap<String, Arc<RenderImage>>,
) -> Option<&'a Arc<RenderImage>> {
    let key = point.avatar_key.as_deref()?;
    if let Some(image) = entity_avatar_pool.get(key) {
        return Some(image);
    }
    let alias = match key {
        "experience_orb" => "xp_orb",
        "experience_bottle" => "xp_bottle",
        _ => return None,
    };
    entity_avatar_pool.get(alias)
}

fn entity_screen_position(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    point: &EntityOverlayPoint,
    margin: f32,
) -> Option<(f32, f32)> {
    let screen_x = overlay_marker_screen_x(bounds, viewport, layout, point.block_x);
    let screen_y = overlay_marker_screen_y(bounds, viewport, layout, point.block_z);
    if !screen_x.is_finite() || !screen_y.is_finite() {
        return None;
    }
    let left = bounds.left() / px(1.0);
    let top = bounds.top() / px(1.0);
    let right = bounds.right() / px(1.0);
    let bottom = bounds.bottom() / px(1.0);
    if screen_x < left - margin
        || screen_y < top - margin
        || screen_x > right + margin
        || screen_y > bottom + margin
    {
        return None;
    }
    Some((screen_x, screen_y))
}

fn paint_entity_overlay_lod(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    overlay_paint: &ProfessionalOverlayPaintCache,
    entity_avatar_pool: &BTreeMap<String, Arc<RenderImage>>,
    window: &mut Window,
) {
    match entity_lod_mode(viewport, layout) {
        EntityLodMode::Exact => paint_exact_entity_avatars(
            bounds,
            viewport,
            layout,
            overlay_paint,
            entity_avatar_pool,
            window,
        ),
        EntityLodMode::ChunkType => paint_chunk_clustered_entity_avatars(
            bounds,
            viewport,
            layout,
            overlay_paint,
            entity_avatar_pool,
            window,
        ),
        EntityLodMode::ScreenCluster => paint_screen_clustered_entity_avatars(
            bounds,
            viewport,
            layout,
            overlay_paint,
            entity_avatar_pool,
            window,
        ),
    }
}

fn paint_exact_entity_avatars(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    overlay_paint: &ProfessionalOverlayPaintCache,
    entity_avatar_pool: &BTreeMap<String, Arc<RenderImage>>,
    window: &mut Window,
) {
    let mut avatar_requests = Vec::with_capacity(overlay_paint.entity_points.len().min(4_096));
    for point in &overlay_paint.entity_points {
        if entity_screen_position(
            bounds,
            viewport,
            layout,
            point,
            ENTITY_VISIBILITY_MARGIN_PX,
        )
        .is_none()
        {
            continue;
        }
        let Some(image) = entity_avatar_arc(point, entity_avatar_pool) else {
            paint_point_marker(
                bounds,
                viewport,
                layout,
                point.block_x,
                point.block_z,
                rgb(0xf97316).into(),
                window,
            );
            continue;
        };
        avatar_requests.push(entity_avatar_request(
            bounds,
            viewport,
            layout,
            point.block_x,
            point.block_z,
            image.as_ref(),
        ));
    }
    paint_entity_avatar_requests(avatar_requests, window);
}

fn paint_chunk_clustered_entity_avatars(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    overlay_paint: &ProfessionalOverlayPaintCache,
    entity_avatar_pool: &BTreeMap<String, Arc<RenderImage>>,
    window: &mut Window,
) {
    ENTITY_LOD_SCRATCH.with(|scratch| {
        let mut scratch = scratch.borrow_mut();
        scratch.begin_chunk_frame();
        for (index, point) in overlay_paint.entity_points.iter().enumerate() {
            if entity_screen_position(
                bounds,
                viewport,
                layout,
                point,
                ENTITY_VISIBILITY_MARGIN_PX,
            )
            .is_none()
            {
                continue;
            }
            let image_identity = entity_avatar_arc(point, entity_avatar_pool)
                .map(|image| Arc::as_ptr(image) as usize)
                .unwrap_or(0);
            let key = EntityChunkClusterKey {
                chunk_x: (point.block_x / 16.0).floor() as i32,
                chunk_z: (point.block_z / 16.0).floor() as i32,
                image_identity,
            };
            if let Some(cluster) = scratch.chunk_clusters.get_mut(&key) {
                cluster.sum_block_x += f64::from(point.block_x);
                cluster.sum_block_z += f64::from(point.block_z);
                cluster.count = cluster.count.saturating_add(1);
            } else {
                scratch.chunk_cluster_order.push(key);
                scratch.chunk_clusters.insert(
                    key,
                    EntityChunkClusterAccum {
                        sum_block_x: f64::from(point.block_x),
                        sum_block_z: f64::from(point.block_z),
                        count: 1,
                        representative_index: index,
                    },
                );
            }
        }

        let chunk_px = entity_chunk_screen_size_px(viewport, layout);
        let icon_size = (chunk_px * 0.85).clamp(11.0, 16.0);
        let mut avatar_requests = Vec::with_capacity(scratch.chunk_cluster_order.len());
        for key in scratch.chunk_cluster_order.iter().copied() {
            let Some(cluster) = scratch.chunk_clusters.get(&key).copied() else {
                continue;
            };
            let count = cluster.count.max(1);
            let block_x = (cluster.sum_block_x / f64::from(count)) as f32;
            let block_z = (cluster.sum_block_z / f64::from(count)) as f32;
            let representative = &overlay_paint.entity_points[cluster.representative_index];
            paint_entity_cluster_backdrop(
                bounds,
                viewport,
                layout,
                block_x,
                block_z,
                count,
                icon_size,
                window,
            );
            let Some(image) = entity_avatar_arc(representative, entity_avatar_pool) else {
                paint_entity_cluster_fallback(
                    bounds, viewport, layout, block_x, block_z, count, window,
                );
                continue;
            };
            avatar_requests.push(entity_avatar_request_sized(
                bounds,
                viewport,
                layout,
                block_x,
                block_z,
                icon_size,
                image.as_ref(),
            ));
        }
        paint_entity_avatar_requests(avatar_requests, window);
    });
}

fn paint_screen_clustered_entity_avatars(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    overlay_paint: &ProfessionalOverlayPaintCache,
    entity_avatar_pool: &BTreeMap<String, Arc<RenderImage>>,
    window: &mut Window,
) {
    let left = bounds.left() / px(1.0);
    let top = bounds.top() / px(1.0);
    let width = (bounds.size.width / px(1.0)).max(1.0);
    let height = (bounds.size.height / px(1.0)).max(1.0);
    let columns = (width / ENTITY_SCREEN_CLUSTER_CELL_PX).ceil().max(1.0) as usize;
    let rows = (height / ENTITY_SCREEN_CLUSTER_CELL_PX).ceil().max(1.0) as usize;
    let required_cells = columns.saturating_mul(rows);

    ENTITY_LOD_SCRATCH.with(|scratch| {
        let mut scratch = scratch.borrow_mut();
        let generation = scratch.begin_screen_frame(required_cells);
        for (index, point) in overlay_paint.entity_points.iter().enumerate() {
            let Some((screen_x, screen_y)) =
                entity_screen_position(bounds, viewport, layout, point, 0.0)
            else {
                continue;
            };
            let cell_x = (((screen_x - left) / ENTITY_SCREEN_CLUSTER_CELL_PX).floor() as usize)
                .min(columns.saturating_sub(1));
            let cell_y = (((screen_y - top) / ENTITY_SCREEN_CLUSTER_CELL_PX).floor() as usize)
                .min(rows.saturating_sub(1));
            let cell_index = cell_y.saturating_mul(columns).saturating_add(cell_x);
            if cell_index >= required_cells {
                continue;
            }
            let has_avatar = entity_avatar_arc(point, entity_avatar_pool).is_some();
            let is_new = scratch.screen_cells[cell_index].generation != generation;
            if is_new {
                scratch.active_screen_cells.push(cell_index);
                scratch.screen_cells[cell_index] = EntityScreenClusterCell {
                    generation,
                    sum_block_x: f64::from(point.block_x),
                    sum_block_z: f64::from(point.block_z),
                    count: 1,
                    representative_index: index,
                    representative_has_avatar: has_avatar,
                };
                continue;
            }
            let cell = &mut scratch.screen_cells[cell_index];
            cell.sum_block_x += f64::from(point.block_x);
            cell.sum_block_z += f64::from(point.block_z);
            cell.count = cell.count.saturating_add(1);
            if !cell.representative_has_avatar && has_avatar {
                cell.representative_index = index;
                cell.representative_has_avatar = true;
            }
        }

        let icon_size = 13.0;
        let mut avatar_requests = Vec::with_capacity(scratch.active_screen_cells.len());
        for cell_index in scratch.active_screen_cells.iter().copied() {
            let cell = scratch.screen_cells[cell_index];
            if cell.generation != generation || cell.count == 0 {
                continue;
            }
            let block_x = (cell.sum_block_x / f64::from(cell.count)) as f32;
            let block_z = (cell.sum_block_z / f64::from(cell.count)) as f32;
            let representative = &overlay_paint.entity_points[cell.representative_index];
            paint_entity_cluster_backdrop(
                bounds,
                viewport,
                layout,
                block_x,
                block_z,
                cell.count,
                icon_size,
                window,
            );
            let Some(image) = entity_avatar_arc(representative, entity_avatar_pool) else {
                paint_entity_cluster_fallback(
                    bounds,
                    viewport,
                    layout,
                    block_x,
                    block_z,
                    cell.count,
                    window,
                );
                continue;
            };
            avatar_requests.push(entity_avatar_request_sized(
                bounds,
                viewport,
                layout,
                block_x,
                block_z,
                icon_size,
                image.as_ref(),
            ));
        }
        paint_entity_avatar_requests(avatar_requests, window);
    });
}

fn paint_entity_cluster_backdrop(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    block_x: f32,
    block_z: f32,
    count: u32,
    icon_size: f32,
    window: &mut Window,
) {
    if count <= 1 {
        return;
    }
    let x = overlay_marker_screen_x(bounds, viewport, layout, block_x);
    let y = overlay_marker_screen_y(bounds, viewport, layout, block_z);
    let density = ((count as f32 + 1.0).log2() * 1.15).clamp(1.5, 7.0);
    let outer_size = icon_size + density * 2.0;
    let outer = px(outer_size);
    window.paint_quad(
        fill(
            Bounds {
                origin: point(px(x) - outer / 2.0, px(y) - outer / 2.0),
                size: size(outer, outer),
            },
            Hsla {
                a: 0.72,
                ..rgb(0x0f172a).into()
            },
        )
        .corner_radii(px((outer_size * 0.28).clamp(3.5, 7.0))),
    );
    let badge_size = (4.0 + (count as f32 + 1.0).log2()).clamp(5.0, 10.0);
    let badge = px(badge_size);
    let icon_half = px(icon_size) / 2.0;
    window.paint_quad(
        fill(
            Bounds {
                origin: point(
                    px(x) + icon_half - badge * 0.62,
                    px(y) + icon_half - badge * 0.62,
                ),
                size: size(badge, badge),
            },
            Hsla {
                a: 0.96,
                ..rgb(0x22c55e).into()
            },
        )
        .corner_radii(badge / 2.0),
    );
}

fn paint_entity_cluster_fallback(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    block_x: f32,
    block_z: f32,
    count: u32,
    window: &mut Window,
) {
    let x = overlay_marker_screen_x(bounds, viewport, layout, block_x);
    let y = overlay_marker_screen_y(bounds, viewport, layout, block_z);
    let marker_size = (7.0 + (count as f32 + 1.0).log2() * 1.4).clamp(7.0, 18.0);
    let marker = px(marker_size);
    window.paint_quad(
        fill(
            Bounds {
                origin: point(px(x) - marker / 2.0, px(y) - marker / 2.0),
                size: size(marker, marker),
            },
            Hsla {
                a: 0.90,
                ..rgb(0xf97316).into()
            },
        )
        .corner_radii(px((marker_size * 0.28).clamp(2.5, 5.0))),
    );
}

fn paint_entity_avatar_requests<'a>(
    requests: Vec<ImagePaintRequest<'a>>,
    window: &mut Window,
) {
    if requests.is_empty() {
        return;
    }
    match window.paint_images_budgeted(requests, ENTITY_AVATAR_UPLOAD_BUDGET) {
        Ok(progress) if progress.deferred_requests > 0 => {
            // 普通 animation frame 可能直接重放 retained absolute subtree，导致本帧
            // 因上传预算延期的头像永远不再进入实际 paint。强制刷新图片层但保留 atlas。
            window.refresh_map_image_uploads();
        }
        Ok(_) => {}
        Err(error) => {
            tracing::debug!(?error, "failed to paint entity avatars");
        }
    }
}

pub(super) fn draw_professional_overlay_canvas(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    dimension: Dimension,
    overlays: OverlayOptions,
    overlay_paint: Option<&ProfessionalOverlayPaintCache>,
    entity_avatar_pool: &BTreeMap<String, Arc<RenderImage>>,
    slime_runs: Option<&SlimeOverlayRunCache>,
    selection: Option<ChunkSelection>,
    paste_preview: Option<&PastePreview>,
    paste_preview_images: &[PastePreviewImage],
    highlighted_window: Option<&SlimeChunkWindow>,
    colors: ThemeColors,
    window: &mut Window,
) {
    let Some(range) = region_render_range_for_viewport(viewport, layout) else {
        return;
    };
    if overlays.slime_chunks {
        let cached_bounds = slime_runs.and_then(|cache| {
            if cache.bounds.dimension != dimension || cache.runs.is_empty() {
                return None;
            }
            paint_cached_slime_runs(bounds, viewport, layout, range, cache, window)
        });
        if dimension == Dimension::Overworld {
            paint_slime_grid_overlay(bounds, viewport, layout, range, cached_bounds, window);
        }
    }

    if let Some(overlay_paint) = overlay_paint {
        if overlays.hardcoded_spawn_areas {
            for rect in &overlay_paint.hardcoded_spawn_rects {
                paint_block_rect(
                    bounds,
                    viewport,
                    layout,
                    rect.min_block_x,
                    rect.min_block_z,
                    rect.max_block_x,
                    rect.max_block_z,
                    Hsla {
                        a: 0.20,
                        ..rgb(0xf2b84b).into()
                    },
                    Some(Hsla {
                        a: 0.70,
                        ..rgb(0xf2b84b).into()
                    }),
                    window,
                );
            }
        }
        if overlays.villages {
            for rect in &overlay_paint.village_rects {
                paint_chunk_rect(
                    bounds,
                    viewport,
                    layout,
                    rect.min_chunk_x,
                    rect.min_chunk_z,
                    rect.max_chunk_x,
                    rect.max_chunk_z,
                    Hsla {
                        a: 0.08,
                        ..rgb(0x2f9bff).into()
                    },
                    Some(Hsla {
                        a: 0.88,
                        ..rgb(0x2f9bff).into()
                    }),
                    window,
                );
            }
        }
        if overlays.entities {
            paint_entity_overlay_lod(
                bounds,
                viewport,
                layout,
                overlay_paint,
                entity_avatar_pool,
                window,
            );
        }
        if overlays.block_entities {
            for point in &overlay_paint.block_entity_points {
                paint_point_marker(
                    bounds,
                    viewport,
                    layout,
                    point.block_x,
                    point.block_z,
                    rgb(0xc084fc).into(),
                    window,
                );
            }
        }
        if overlays.pending_ticks {
            for marker in &overlay_paint.pending_tick_chunk_markers {
                paint_chunk_marker(
                    bounds,
                    viewport,
                    layout,
                    *marker,
                    rgb(0xfbbf24).into(),
                    window,
                );
            }
        }
    }

    if let Some(selection) = selection {
        let exact_chunks = exact_selection_chunks(selection);
        let irregular = exact_chunks
            .as_deref()
            .is_some_and(|chunks| !selection_chunks_are_rectangular(selection, Some(chunks)));
        if irregular {
            if let Some(exact_chunks) = exact_chunks.as_deref() {
                paint_exact_chunk_selection(
                    bounds,
                    viewport,
                    layout,
                    dimension,
                    exact_chunks,
                    colors,
                    window,
                );
            }
        } else {
            let selection_bounds = selection.bounds();
            paint_chunk_rect(
                bounds,
                viewport,
                layout,
                selection_bounds.min_chunk_x,
                selection_bounds.min_chunk_z,
                selection_bounds.max_chunk_x,
                selection_bounds.max_chunk_z,
                Hsla {
                    a: 0.10,
                    ..colors.accent
                },
                Some(Hsla {
                    a: 0.92,
                    ..colors.accent
                }),
                window,
            );
            paint_selection_resize_handles(
                bounds,
                viewport,
                layout,
                selection_bounds,
                colors,
                window,
            );
        }
    }

    if let Some(preview) = paste_preview {
        paint_paste_preview_images(bounds, viewport, layout, paste_preview_images, window);
        paint_pending_paste_chunks(bounds, viewport, layout, preview, window);
        if !preview.is_writing() {
            paint_paste_preview_outline(bounds, viewport, layout, preview, window);
        }
    }

    if let Some(window_candidate) = highlighted_window {
        paint_chunk_rect(
            bounds,
            viewport,
            layout,
            window_candidate.min_chunk_x,
            window_candidate.min_chunk_z,
            window_candidate.max_chunk_x,
            window_candidate.max_chunk_z,
            Hsla {
                a: 0.14,
                ..rgb(0x9ef01a).into()
            },
            Some(Hsla {
                a: 0.95,
                ..rgb(0x9ef01a).into()
            }),
            window,
        );
    }
}

fn paint_selection_resize_handles(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    selection: SlimeChunkBounds,
    colors: ThemeColors,
    window: &mut Window,
) {
    let left = screen_x_for_block(
        bounds,
        viewport,
        layout,
        selection.min_chunk_x.saturating_mul(16),
    );
    let right = screen_x_for_block(
        bounds,
        viewport,
        layout,
        selection.max_chunk_x.saturating_add(1).saturating_mul(16),
    );
    let top = screen_y_for_block(bounds, viewport, layout, selection.min_chunk_z.saturating_mul(16));
    let bottom = screen_y_for_block(
        bounds,
        viewport,
        layout,
        selection.max_chunk_z.saturating_add(1).saturating_mul(16),
    );
    let center_x = (left + right) * 0.5;
    let center_y = (top + bottom) * 0.5;
    for (x, y) in [
        (left, top),
        (center_x, top),
        (right, top),
        (right, center_y),
        (right, bottom),
        (center_x, bottom),
        (left, bottom),
        (left, center_y),
    ] {
        let outer = Bounds {
            origin: point(px(x - 4.5), px(y - 4.5)),
            size: size(px(9.0), px(9.0)),
        };
        let inner = Bounds {
            origin: point(px(x - 3.0), px(y - 3.0)),
            size: size(px(6.0), px(6.0)),
        };
        window.paint_quad(fill(outer, colors.surface));
        window.paint_quad(fill(inner, colors.accent));
    }
}

fn paint_exact_chunk_selection(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    dimension: Dimension,
    chunks: &[ChunkPos],
    colors: ThemeColors,
    window: &mut Window,
) {
    let selected = chunks
        .iter()
        .copied()
        .filter(|chunk| chunk.dimension == dimension)
        .collect::<HashSet<_>>();
    if selected.is_empty() {
        return;
    }

    for chunk in &selected {
        paint_chunk_rect(
            bounds,
            viewport,
            layout,
            chunk.x,
            chunk.z,
            chunk.x,
            chunk.z,
            Hsla {
                a: 0.10,
                ..colors.accent
            },
            None,
            window,
        );
    }

    let stroke = Hsla {
        a: 0.92,
        ..colors.accent
    };
    for chunk in &selected {
        let left = screen_x_for_block(bounds, viewport, layout, chunk.x.saturating_mul(16));
        let right = screen_x_for_block(
            bounds,
            viewport,
            layout,
            chunk.x.saturating_add(1).saturating_mul(16),
        );
        let top = screen_y_for_block(bounds, viewport, layout, chunk.z.saturating_mul(16));
        let bottom = screen_y_for_block(
            bounds,
            viewport,
            layout,
            chunk.z.saturating_add(1).saturating_mul(16),
        );
        if right <= left || bottom <= top {
            continue;
        }

        let edge_width = (right - left).min(bottom - top).clamp(0.75, 2.0);
        let thickness = px(edge_width);
        let rect_left = px(left.floor());
        let rect_top = px(top.floor());
        let rect_right = px(right.ceil());
        let rect_bottom = px(bottom.ceil());
        let rect_width = rect_right - rect_left;
        let rect_height = rect_bottom - rect_top;

        if !selected_chunk_neighbor(&selected, *chunk, 0, -1) {
            window.paint_quad(fill(
                Bounds::new(point(rect_left, rect_top), size(rect_width, thickness)),
                stroke,
            ));
        }
        if !selected_chunk_neighbor(&selected, *chunk, 1, 0) {
            window.paint_quad(fill(
                Bounds::new(
                    point(rect_right - thickness, rect_top),
                    size(thickness, rect_height),
                ),
                stroke,
            ));
        }
        if !selected_chunk_neighbor(&selected, *chunk, 0, 1) {
            window.paint_quad(fill(
                Bounds::new(
                    point(rect_left, rect_bottom - thickness),
                    size(rect_width, thickness),
                ),
                stroke,
            ));
        }
        if !selected_chunk_neighbor(&selected, *chunk, -1, 0) {
            window.paint_quad(fill(
                Bounds::new(point(rect_left, rect_top), size(thickness, rect_height)),
                stroke,
            ));
        }
    }
}

fn selected_chunk_neighbor(
    selected: &HashSet<ChunkPos>,
    chunk: ChunkPos,
    delta_x: i32,
    delta_z: i32,
) -> bool {
    let Some(x) = chunk.x.checked_add(delta_x) else {
        return false;
    };
    let Some(z) = chunk.z.checked_add(delta_z) else {
        return false;
    };
    selected.contains(&ChunkPos {
        x,
        z,
        dimension: chunk.dimension,
    })
}

fn paint_pending_paste_chunks(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    preview: &PastePreview,
    window: &mut Window,
) {
    let Some(progress) = preview.write_progress else {
        return;
    };
    if progress.awaiting_tile_refresh {
        return;
    }
    for chunk in preview.targets.iter().skip(progress.completed) {
        paint_chunk_rect(
            bounds,
            viewport,
            layout,
            chunk.x,
            chunk.z,
            chunk.x,
            chunk.z,
            Hsla {
                a: 0.58,
                ..rgb(0x64748b).into()
            },
            Some(Hsla {
                a: 0.72,
                ..rgb(0x94a3b8).into()
            }),
            window,
        );
    }
}

fn paint_cached_slime_runs(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    range: MapRenderRange,
    cache: &SlimeOverlayRunCache,
    window: &mut Window,
) -> Option<(i32, i32, i32, i32)> {
    let min_chunk_x = cache.bounds.min_chunk_x.max(range.min_chunk_x);
    let max_chunk_x = cache.bounds.max_chunk_x.min(range.max_chunk_x);
    let min_chunk_z = cache.bounds.min_chunk_z.max(range.min_chunk_z);
    let max_chunk_z = cache.bounds.max_chunk_z.min(range.max_chunk_z);
    if min_chunk_x > max_chunk_x || min_chunk_z > max_chunk_z {
        return None;
    }
    for run in &cache.runs {
        let run_min_x = run.min_chunk_x.max(min_chunk_x);
        let run_max_x = run.max_chunk_x.min(max_chunk_x);
        let run_min_z = run.min_chunk_z.max(min_chunk_z);
        let run_max_z = run.max_chunk_z.min(max_chunk_z);
        if run_min_x > run_max_x || run_min_z > run_max_z {
            continue;
        }
        paint_chunk_rect(
            bounds,
            viewport,
            layout,
            run_min_x,
            run_min_z,
            run_max_x,
            run_max_z,
            Hsla {
                a: 0.22,
                ..rgb(0x43d17a).into()
            },
            None,
            window,
        );
    }
    Some((min_chunk_x, min_chunk_z, max_chunk_x, max_chunk_z))
}

fn paint_slime_grid_overlay(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    range: MapRenderRange,
    covered_bounds: Option<(i32, i32, i32, i32)>,
    window: &mut Window,
) {
    let block_bounds = viewport_block_bounds(viewport, layout);
    let chunk_step = grid_step_for_block_bounds(16, block_bounds, 280).max(1);
    let mut chunk_z = range.min_chunk_z;
    while chunk_z <= range.max_chunk_z {
        let max_chunk_z = chunk_z
            .saturating_add(chunk_step.saturating_sub(1))
            .min(range.max_chunk_z);
        let mut chunk_x = range.min_chunk_x;
        while chunk_x <= range.max_chunk_x {
            let max_chunk_x = chunk_x
                .saturating_add(chunk_step.saturating_sub(1))
                .min(range.max_chunk_x);
            let sample = ChunkPos {
                x: chunk_x.saturating_add(max_chunk_x.saturating_sub(chunk_x) / 2),
                z: chunk_z.saturating_add(max_chunk_z.saturating_sub(chunk_z) / 2),
                dimension: Dimension::Overworld,
            };
            let sample_is_cached = covered_bounds.is_some_and(|(min_x, min_z, max_x, max_z)| {
                sample.x >= min_x && sample.x <= max_x && sample.z >= min_z && sample.z <= max_z
            });
            if !sample_is_cached && is_slime_chunk(sample) {
                paint_chunk_rect(
                    bounds,
                    viewport,
                    layout,
                    sample.x,
                    sample.z,
                    sample.x,
                    sample.z,
                    Hsla {
                        a: 0.22,
                        ..rgb(0x43d17a).into()
                    },
                    None,
                    window,
                );
            }
            if chunk_x == range.max_chunk_x {
                break;
            }
            chunk_x = max_chunk_x.saturating_add(1);
        }
        if chunk_z == range.max_chunk_z {
            break;
        }
        chunk_z = max_chunk_z.saturating_add(1);
    }
}

fn paint_paste_preview_outline(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    preview: &PastePreview,
    window: &mut Window,
) {
    let Some(min_x) = preview.targets.iter().map(|chunk| chunk.x).min() else {
        return;
    };
    let Some(max_x) = preview.targets.iter().map(|chunk| chunk.x).max() else {
        return;
    };
    let Some(min_z) = preview.targets.iter().map(|chunk| chunk.z).min() else {
        return;
    };
    let Some(max_z) = preview.targets.iter().map(|chunk| chunk.z).max() else {
        return;
    };
    paint_chunk_rect(
        bounds,
        viewport,
        layout,
        min_x,
        min_z,
        max_x,
        max_z,
        Hsla {
            a: 0.0,
            ..rgb(0xf59e0b).into()
        },
        Some(Hsla {
            a: 0.95,
            ..rgb(0xf59e0b).into()
        }),
        window,
    );
    paint_chunk_rect(
        bounds,
        viewport,
        layout,
        preview.target_anchor.x,
        preview.target_anchor.z,
        preview.target_anchor.x,
        preview.target_anchor.z,
        Hsla {
            a: 0.0,
            ..rgb(0x22c55e).into()
        },
        Some(Hsla {
            a: 0.95,
            ..rgb(0x22c55e).into()
        }),
        window,
    );
}

fn paint_paste_preview_images(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    images: &[PastePreviewImage],
    window: &mut Window,
) {
    let requests = images.iter().filter_map(|image| {
        let left = screen_x_for_block(bounds, viewport, layout, image.target.x.saturating_mul(16));
        let top = screen_y_for_block(bounds, viewport, layout, image.target.z.saturating_mul(16));
        let right = screen_x_for_block(
            bounds,
            viewport,
            layout,
            image.target.x.saturating_add(1).saturating_mul(16),
        );
        let bottom = screen_y_for_block(
            bounds,
            viewport,
            layout,
            image.target.z.saturating_add(1).saturating_mul(16),
        );
        if right <= left || bottom <= top {
            return None;
        }
        let image_bounds = Bounds {
            origin: point(px(left.floor()), px(top.floor())),
            size: size(px((right - left).ceil()), px((bottom - top).ceil())),
        };
        Some(ImagePaintRequest::new(image_bounds, image.image.as_ref()))
    });
    if let Err(error) = window.paint_images(requests) {
        tracing::debug!(?error, "failed to paint paste preview chunk images");
    }
}

fn entity_avatar_request<'a>(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    block_x: f32,
    block_z: f32,
    image: &'a RenderImage,
) -> ImagePaintRequest<'a> {
    entity_avatar_request_sized(
        bounds,
        viewport,
        layout,
        block_x,
        block_z,
        overlay_icon_size_px(viewport, layout),
        image,
    )
}

fn entity_avatar_request_sized<'a>(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    block_x: f32,
    block_z: f32,
    icon_size: f32,
    image: &'a RenderImage,
) -> ImagePaintRequest<'a> {
    let x = overlay_marker_screen_x(bounds, viewport, layout, block_x);
    let y = overlay_marker_screen_y(bounds, viewport, layout, block_z);
    let size_px = px(icon_size);
    ImagePaintRequest::new(
        Bounds {
            origin: point(px(x) - size_px / 2.0, px(y) - size_px / 2.0),
            size: size(size_px, size_px),
        },
        image,
    )
}

pub(super) fn overlay_icon_size_px(viewport: MapViewport, layout: RenderLayout) -> f32 {
    let chunk_screen_size = entity_chunk_screen_size_px(viewport, layout);
    if !chunk_screen_size.is_finite() {
        return 12.0;
    }
    (chunk_screen_size * 0.35).clamp(16.0, 52.0)
}

pub(super) fn paint_chunk_rect(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    min_chunk_x: i32,
    min_chunk_z: i32,
    max_chunk_x: i32,
    max_chunk_z: i32,
    fill_color: Hsla,
    stroke_color: Option<Hsla>,
    window: &mut Window,
) {
    paint_block_rect(
        bounds,
        viewport,
        layout,
        min_chunk_x.saturating_mul(16),
        min_chunk_z.saturating_mul(16),
        max_chunk_x.saturating_add(1).saturating_mul(16),
        max_chunk_z.saturating_add(1).saturating_mul(16),
        fill_color,
        stroke_color,
        window,
    );
}

pub(super) fn paint_block_rect(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    min_block_x: i32,
    min_block_z: i32,
    max_block_x: i32,
    max_block_z: i32,
    fill_color: Hsla,
    stroke_color: Option<Hsla>,
    window: &mut Window,
) {
    let left = screen_x_for_block(bounds, viewport, layout, min_block_x);
    let top = screen_y_for_block(bounds, viewport, layout, min_block_z);
    let right = screen_x_for_block(bounds, viewport, layout, max_block_x);
    let bottom = screen_y_for_block(bounds, viewport, layout, max_block_z);
    if right <= left || bottom <= top {
        return;
    }
    let rect = Bounds {
        origin: point(px(left.floor()), px(top.floor())),
        size: size(px((right - left).ceil()), px((bottom - top).ceil())),
    };
    if fill_color.a > 0.0 {
        window.paint_quad(fill(rect, fill_color));
    }
    if let Some(stroke_color) = stroke_color {
        let stroke_width = (right - left).min(bottom - top).clamp(0.5, 2.0);
        let mut builder = PathBuilder::stroke(px(stroke_width));
        builder.move_to(rect.origin);
        builder.line_to(point(rect.right(), rect.top()));
        builder.line_to(point(rect.right(), rect.bottom()));
        builder.line_to(point(rect.left(), rect.bottom()));
        builder.line_to(rect.origin);
        if let Ok(path) = builder.build() {
            window.paint_path(path, stroke_color);
        }
    }
}

pub(super) fn paint_point_marker(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    block_x: f32,
    block_z: f32,
    color: Hsla,
    window: &mut Window,
) {
    let x = overlay_marker_screen_x(bounds, viewport, layout, block_x);
    let y = overlay_marker_screen_y(bounds, viewport, layout, block_z);
    let size_px = px((overlay_icon_size_px(viewport, layout) * 0.36).clamp(6.0, 18.0));
    window.paint_quad(
        fill(
            Bounds {
                origin: point(px(x) - size_px / 2.0, px(y) - size_px / 2.0),
                size: size(size_px, size_px),
            },
            Hsla { a: 0.88, ..color },
        )
        .corner_radii(px(3.5)),
    );
}

pub(super) fn overlay_marker_screen_x(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    block_x: f32,
) -> f32 {
    bounds.left() / px(1.0)
        + region_render_range_for_viewport(viewport, layout).map_or_else(
            || {
                viewport.offset_x
                    + block_x * layout.pixels_per_block as f32 / layout.blocks_per_pixel as f32
                        * viewport.scale
            },
            |range| {
                range.render_origin_x
                    + (block_x - range.min_chunk_x as f32 * 16.0) * range.block_screen_size
            },
        )
}

pub(super) fn overlay_marker_screen_y(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    block_z: f32,
) -> f32 {
    bounds.top() / px(1.0)
        + region_render_range_for_viewport(viewport, layout).map_or_else(
            || {
                viewport.offset_y
                    + block_z * layout.pixels_per_block as f32 / layout.blocks_per_pixel as f32
                        * viewport.scale
            },
            |range| {
                range.render_origin_y
                    + (block_z - range.min_chunk_z as f32 * 16.0) * range.block_screen_size
            },
        )
}

pub(super) fn paint_chunk_marker(
    bounds: Bounds<Pixels>,
    viewport: MapViewport,
    layout: RenderLayout,
    marker: ChunkOverlayMarker,
    color: Hsla,
    window: &mut Window,
) {
    let block_x = marker.chunk_x as f32 * 16.0 + 8.0;
    let block_z = marker.chunk_z as f32 * 16.0 + 8.0;
    let x = bounds.left() / px(1.0)
        + region_render_range_for_viewport(viewport, layout).map_or_else(
            || {
                viewport.offset_x
                    + block_x * layout.pixels_per_block as f32 / layout.blocks_per_pixel as f32
                        * viewport.scale
            },
            |range| {
                range.render_origin_x
                    + (block_x - range.min_chunk_x as f32 * 16.0) * range.block_screen_size
            },
        );
    let y = bounds.top() / px(1.0)
        + region_render_range_for_viewport(viewport, layout).map_or_else(
            || {
                viewport.offset_y
                    + block_z * layout.pixels_per_block as f32 / layout.blocks_per_pixel as f32
                        * viewport.scale
            },
            |range| {
                range.render_origin_y
                    + (block_z - range.min_chunk_z as f32 * 16.0) * range.block_screen_size
            },
        );
    let marker_size = (overlay_icon_size_px(viewport, layout) * 0.55 + marker.count.min(9) as f32)
        .clamp(7.0, 30.0);
    let size_px = px(marker_size);
    window.paint_quad(fill(
        Bounds {
            origin: point(
                px(x) - size_px / 2.0 - px(1.0),
                px(y) - size_px / 2.0 - px(1.0),
            ),
            size: size(size_px + px(2.0), size_px + px(2.0)),
        },
        rgb(0x000000),
    ));
    window.paint_quad(fill(
        Bounds {
            origin: point(px(x) - size_px / 2.0, px(y) - size_px / 2.0),
            size: size(size_px, size_px),
        },
        Hsla { a: 0.82, ..color },
    ));
}
