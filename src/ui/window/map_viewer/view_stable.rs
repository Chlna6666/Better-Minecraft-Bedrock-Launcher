use crate::ui::window::map_viewer::lifecycle::VIEWPORT_COMPOSITE_ENABLED;
pub(super) use super::view_legacy::*;

use super::model::{MapViewerWindowInit, MapViewerWindowView};
use super::prelude::*;
use super::tile_state::{TileLoadState, TilePriority};
use super::viewport::TileBounds;
use std::time::Duration;

const MAP_VIEWER_DEFAULT_WINDOW_WIDTH: f32 = 1120.0;
const MAP_VIEWER_DEFAULT_WINDOW_HEIGHT: f32 = 720.0;
const MAP_VIEWER_MIN_WINDOW_WIDTH: f32 = 920.0;
const MAP_VIEWER_MIN_WINDOW_HEIGHT: f32 = 620.0;
const MAP_VIEWER_MAX_DISPLAY_RATIO: f32 = 0.9;
const VIEWPORT_WATCHDOG_INTERVAL: Duration = Duration::from_millis(80);
const FRONTEND_TILE_REPAINT_INTERVAL: Duration = Duration::from_millis(16);
const FRONTEND_NEW_IMAGE_BUDGET_PER_REPAINT: usize = 8;
const FRONTEND_REPAINT_SAFETY_PASSES: usize = 2;
const FRONTEND_REPAINT_PROGRESS_LOG_INTERVAL: usize = 8;

fn frontend_repaint_passes(image_count: usize) -> usize {
    if image_count == 0 {
        return 0;
    }
    image_count
        .saturating_add(FRONTEND_NEW_IMAGE_BUDGET_PER_REPAINT - 1)
        / FRONTEND_NEW_IMAGE_BUDGET_PER_REPAINT
        + FRONTEND_REPAINT_SAFETY_PASSES
}

fn frontend_snapshot_image_ids(snapshot: &TilePaintSnapshot) -> BTreeSet<ImageId> {
    snapshot
        .screen_images
        .iter()
        .map(|image| image.image.id)
        .chain(snapshot.tiles.iter().map(|tile| tile.image.id))
        .collect()
}

fn visible_tile_frontend_ready(
    tile_manager: &super::tile_state::RegionManager,
    coord: (i32, i32),
) -> bool {
    tile_manager.entries.get(&coord).is_some_and(|entry| {
        entry.state == TileLoadState::Invalid
            || (entry.state == TileLoadState::Loaded && entry.image.is_some())
    })
}

impl MapViewerWindowView {
    fn prepare_visible_manifest_probe(
        &mut self,
        visible_tiles: &[(i32, i32)],
        cx: &mut Context<Self>,
    ) {
        if self.viewport_interaction_active() || self.manifest_probe_in_flight {
            return;
        }

        let unresolved_tiles = visible_tiles
            .iter()
            .copied()
            .filter(|coord| {
                !self.tile_chunk_index.contains_key(coord)
                    && !self.tile_manager.entries.get(coord).is_some_and(|entry| {
                        entry.state == TileLoadState::Invalid
                    })
            })
            .collect::<Vec<_>>();
        if unresolved_tiles.is_empty() {
            return;
        }

        self.tile_manager
            .ensure_pending_manifest(&unresolved_tiles, TilePriority::Visible);
        self.pending_viewport_refresh = true;
        let center_tile = self.viewport.center_tile(self.active_layout);
        self.schedule_tile_manifest_probe(visible_tiles, &[], center_tile, cx);
    }

    fn spawn_viewport_watchdog(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |handle, cx| {
            // Track only genuinely new ImageIds. A ReadyBatch usually adds at most a handful of
            // tiles, so it must not restart a repaint plan sized for every image already resident
            // in the snapshot. The previous additive plan grew to hundreds of forced full-window
            // redraws and made retained source tiles and macro pages alternate visibly.
            let mut last_frontend_generation = u64::MAX;
            let mut last_frontend_image_ids = BTreeSet::new();
            let mut last_frontend_paint_bounds: Option<TileBounds> = None;
            let mut frontend_repaint_passes_remaining = 0usize;
            let mut frontend_repaint_passes_total = 0usize;

            loop {
                let interval = if frontend_repaint_passes_remaining > 0 {
                    FRONTEND_TILE_REPAINT_INTERVAL
                } else {
                    VIEWPORT_WATCHDOG_INTERVAL
                };
                Timer::after(interval).await;
                let Some(view) = handle.upgrade() else {
                    break;
                };
                view.update(cx, |this, cx| {
                    if this.render_session.is_none() || this.session_loading {
                        return;
                    }
                    let visible_tiles = this.tile_coords_for_viewport(0);
                    if visible_tiles.is_empty() {
                        return;
                    }

                    let frontend_generation = this.canvas_tile_snapshot.generation;
                    if frontend_generation != last_frontend_generation {
                        let current_image_ids =
                            frontend_snapshot_image_ids(&this.canvas_tile_snapshot);
                        let image_count = current_image_ids.len();
                        let added_or_replaced_images = current_image_ids
                            .difference(&last_frontend_image_ids)
                            .count();
                        let removed_images = last_frontend_image_ids
                            .difference(&current_image_ids)
                            .count();
                        let paint_bounds = this.canvas_tile_snapshot.paint_bounds;
                        let viewport_bounds_changed = paint_bounds != last_frontend_paint_bounds;

                        if added_or_replaced_images > 0 {
                            let requested_passes =
                                frontend_repaint_passes(added_or_replaced_images);
                            // Coalesce bursts instead of adding every generation's pass count.
                            // At most the largest currently pending burst remains scheduled.
                            if requested_passes > frontend_repaint_passes_remaining {
                                frontend_repaint_passes_remaining = requested_passes;
                                frontend_repaint_passes_total = requested_passes;
                            }
                        }

                        last_frontend_generation = frontend_generation;
                        last_frontend_image_ids = current_image_ids;
                        last_frontend_paint_bounds = paint_bounds;
                        tracing::debug!(
                            frontend_generation,
                            image_count,
                            added_or_replaced_images,
                            removed_images,
                            viewport_bounds_changed,
                            macro_pages = this.canvas_tile_snapshot.screen_images.len(),
                            individual_tiles = this.canvas_tile_snapshot.tiles.len(),
                            repaint_passes = frontend_repaint_passes_remaining,
                            viewport_scale = this.viewport.scale,
                            ?paint_bounds,
                            "map_viewer frontend_tile_upload_latch_updated"
                        );
                    }

                    let frontend_repaint_pending = frontend_repaint_passes_remaining > 0;
                    if frontend_repaint_pending {
                        // Invalidate only the retained tile-layer cache. Do not call
                        // Window::refresh_map_image_uploads here: that forces a full-window cache
                        // refresh every 8 ms and visibly alternates the source-tile and macro-page
                        // scenes. set_tile_snapshot increments the tile-layer revision and is
                        // sufficient to execute the budgeted image paint again.
                        this.last_synced_tile_layer_snapshot_key = None;
                        let colors = this.theme_colors(cx);
                        this.sync_tile_layer_snapshot(colors, cx);

                        let repaint_pass = frontend_repaint_passes_total
                            .saturating_sub(frontend_repaint_passes_remaining)
                            .saturating_add(1);
                        frontend_repaint_passes_remaining =
                            frontend_repaint_passes_remaining.saturating_sub(1);
                        if repaint_pass == 1
                            || frontend_repaint_passes_remaining <= 1
                            || repaint_pass % FRONTEND_REPAINT_PROGRESS_LOG_INTERVAL == 0
                        {
                            tracing::debug!(
                                frontend_generation,
                                repaint_pass,
                                repaint_passes_total = frontend_repaint_passes_total,
                                repaint_passes_remaining = frontend_repaint_passes_remaining,
                                image_count = last_frontend_image_ids.len(),
                                viewport_scale = this.viewport.scale,
                                paint_bounds = ?this.canvas_tile_snapshot.paint_bounds,
                                refresh_scope = "tile_layer",
                                "map_viewer frontend_tile_upload_repaint"
                            );
                        }
                        if frontend_repaint_passes_remaining == 0 {
                            tracing::debug!(
                                frontend_generation,
                                image_count = last_frontend_image_ids.len(),
                                viewport_scale = this.viewport.scale,
                                paint_bounds = ?this.canvas_tile_snapshot.paint_bounds,
                                "map_viewer frontend_tile_upload_latch_drained"
                            );
                        }
                        // Keep upload work isolated from manifest/render scheduling. Mixing both
                        // in the same 16 ms tick creates another snapshot before the previous
                        // retained tile layer has reached the renderer.
                        return;
                    }

                    this.prepare_visible_manifest_probe(&visible_tiles, cx);

                    // screen_images is also used by low-zoom macro pages. It must not activate
                    // the legacy single-frame viewport-composite state machine, whose
                    // `screen_images.len() != 1` condition otherwise keeps the viewport marked
                    // incomplete forever and rebuilds the tile snapshot every watchdog tick.
                    let viewport_composite_active = VIEWPORT_COMPOSITE_ENABLED
                        && (this.viewport_composite_request_id.is_some()
                            || !this.canvas_tile_snapshot.screen_images.is_empty());
                    let orphaned_loading = if viewport_composite_active {
                        Vec::new()
                    } else {
                        visible_tiles
                            .iter()
                            .copied()
                            .filter(|coord| {
                                this.tile_manager.entries.get(coord).is_some_and(|entry| {
                                    entry.state == TileLoadState::Loading
                                        && !this.active_render_tiles.contains(coord)
                                })
                            })
                            .collect::<Vec<_>>()
                    };
                    if !orphaned_loading.is_empty() {
                        this.tile_manager
                            .requeue_cancelled_loading(&orphaned_loading);
                    }

                    let incomplete = if viewport_composite_active {
                        this.viewport_composite_request_id.is_some()
                            || this.pending_viewport_refresh
                            || this.canvas_tile_snapshot.screen_images.len() != 1
                    } else {
                        visible_tiles.iter().copied().any(|coord| {
                            !visible_tile_frontend_ready(&this.tile_manager, coord)
                        })
                    };
                    if !incomplete && orphaned_loading.is_empty() {
                        return;
                    }

                    this.pending_viewport_refresh = true;
                    this.ensure_visible_tiles(cx);
                    if this.pending_viewport_refresh {
                        this.schedule_viewport_work_refresh(cx);
                    }
                })?;
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }
}

pub fn open_map_viewer_window(init: MapViewerWindowInit, cx: &mut App) {
    let title = format!("地图预览 - {}", init.asset.display_name);
    let options = stable_map_viewer_window_options(cx);
    let window = cx.open_window(options, move |window, cx| {
        window.set_title(&title);
        window.on_window_should_close(cx, |window, _cx| {
            let restored_bounds = window.window_bounds().bounds();
            let prefs = crate::core::ui_prefs::MapViewerWindowPrefs {
                width: restored_bounds.size.width / px(1.0),
                height: restored_bounds.size.height / px(1.0),
            };
            if let Err(error) = crate::core::ui_prefs::save_map_viewer_window_prefs(&prefs) {
                tracing::warn!(%error, "failed to save map viewer window size");
            }
            window.remove_window();
            true
        });
        window.activate_window();
        let view = cx.new(|cx| MapViewerWindowView::new(init, window, cx));
        view.update(cx, |this, cx| this.spawn_viewport_watchdog(cx));
        cx.new(|cx| crate::ui::runtime::root_view::RootView::new(view, window, cx))
    });
    if let Err(error) = window {
        eprintln!("Failed to open map viewer window: {error:?}");
    }
}

fn stable_map_viewer_window_options(cx: &mut App) -> WindowOptions {
    let mut options = WindowOptions::default();
    options.window_bounds = Some(WindowBounds::centered(stable_map_viewer_window_size(cx), cx));
    options.window_min_size = Some(size(
        px(MAP_VIEWER_MIN_WINDOW_WIDTH),
        px(MAP_VIEWER_MIN_WINDOW_HEIGHT),
    ));
    options.is_resizable = true;
    options.is_minimizable = true;
    options.is_movable = true;
    #[cfg(windows)]
    {
        options.titlebar = Some(TitlebarOptions {
            title: Some(SharedString::from("地图预览")),
            appears_transparent: false,
            ..Default::default()
        });
        options.window_background = WindowBackgroundAppearance::Opaque;
    }
    options
}

fn stable_map_viewer_window_size(cx: &App) -> Size<Pixels> {
    let saved = crate::core::ui_prefs::load_map_viewer_window_prefs();
    let display_size = cx.primary_display().map(|display| display.bounds().size);
    let restored = saved.filter(|prefs| {
        prefs.width.is_finite()
            && prefs.height.is_finite()
            && prefs.width >= MAP_VIEWER_MIN_WINDOW_WIDTH
            && prefs.height >= MAP_VIEWER_MIN_WINDOW_HEIGHT
    });
    let requested_width = restored.map_or(MAP_VIEWER_DEFAULT_WINDOW_WIDTH, |prefs| prefs.width);
    let requested_height = restored.map_or(MAP_VIEWER_DEFAULT_WINDOW_HEIGHT, |prefs| prefs.height);
    let (maximum_width, maximum_height) =
        display_size.map_or((f32::MAX, f32::MAX), |display_size| {
            (
                display_size.width / px(1.0) * MAP_VIEWER_MAX_DISPLAY_RATIO,
                display_size.height / px(1.0) * MAP_VIEWER_MAX_DISPLAY_RATIO,
            )
        });

    size(
        px(requested_width
            .min(maximum_width)
            .max(MAP_VIEWER_MIN_WINDOW_WIDTH)),
        px(requested_height
            .min(maximum_height)
            .max(MAP_VIEWER_MIN_WINDOW_HEIGHT)),
    )
}
