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
const FRONTEND_TILE_REPAINT_INTERVAL: Duration = Duration::from_millis(8);
const FRONTEND_NEW_IMAGE_BUDGET_PER_REPAINT: usize = 8;
const FRONTEND_REPAINT_SAFETY_PASSES: usize = 2;
const FRONTEND_REPAINT_PROGRESS_LOG_INTERVAL: usize = 32;

fn frontend_repaint_passes(image_count: usize) -> usize {
    if image_count == 0 {
        return 0;
    }
    image_count
        .saturating_add(FRONTEND_NEW_IMAGE_BUDGET_PER_REPAINT - 1)
        / FRONTEND_NEW_IMAGE_BUDGET_PER_REPAINT
        + FRONTEND_REPAINT_SAFETY_PASSES
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
        let window_handle = cx.windows();
        cx.spawn(async move |handle, cx| {
            // GPUI deliberately uploads only a small number of previously unseen images per
            // paint. A normal refresh can replay the absolute layer cache without running the
            // canvas paint closure, so deferred images would never get another upload attempt.
            // Keep a small upload latch and pair every pass with a non-destructive window cache
            // invalidation. Resident atlas entries and RenderImage handles remain intact.
            let mut last_frontend_generation = u64::MAX;
            let mut last_frontend_image_count = 0usize;
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
                let force_frontend_repaint = view.update(cx, |this, cx| {
                    if this.render_session.is_none() || this.session_loading {
                        return false;
                    }
                    let visible_tiles = this.tile_coords_for_viewport(0);
                    if visible_tiles.is_empty() {
                        return false;
                    }

                    let frontend_generation = this.canvas_tile_snapshot.generation;
                    if frontend_generation != last_frontend_generation {
                        let image_count = this
                            .canvas_tile_snapshot
                            .tiles
                            .len()
                            .saturating_add(this.canvas_tile_snapshot.screen_images.len());
                        let paint_bounds = this.canvas_tile_snapshot.paint_bounds;
                        let full_snapshot_change = paint_bounds != last_frontend_paint_bounds
                            || image_count < last_frontend_image_count
                            || last_frontend_generation == u64::MAX;
                        let added_or_replaced_images = if full_snapshot_change {
                            image_count
                        } else {
                            image_count
                                .saturating_sub(last_frontend_image_count)
                                .max(1)
                        };
                        let added_passes = frontend_repaint_passes(added_or_replaced_images);
                        let maximum_passes = frontend_repaint_passes(image_count);
                        if full_snapshot_change {
                            frontend_repaint_passes_remaining = maximum_passes;
                        } else {
                            frontend_repaint_passes_remaining = frontend_repaint_passes_remaining
                                .saturating_add(added_passes)
                                .min(maximum_passes);
                        }
                        frontend_repaint_passes_total = frontend_repaint_passes_remaining;
                        last_frontend_generation = frontend_generation;
                        last_frontend_image_count = image_count;
                        last_frontend_paint_bounds = paint_bounds;
                        tracing::debug!(
                            frontend_generation,
                            image_count,
                            added_or_replaced_images,
                            full_snapshot_change,
                            repaint_passes = frontend_repaint_passes_remaining,
                            viewport_scale = this.viewport.scale,
                            ?paint_bounds,
                            "map_viewer frontend_tile_upload_latch_armed"
                        );
                    }

                    this.prepare_visible_manifest_probe(&visible_tiles, cx);

                    let composite_frontend_active = this.viewport_composite_request_id.is_some()
                        || !this.canvas_tile_snapshot.screen_images.is_empty();
                    let orphaned_loading = if composite_frontend_active {
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

                    let incomplete = if composite_frontend_active {
                        this.viewport_composite_request_id.is_some()
                            || this.pending_viewport_refresh
                            || this.canvas_tile_snapshot.screen_images.len() != 1
                    } else {
                        visible_tiles.iter().any(|coord| {
                            !matches!(
                                this.tile_manager.entries.get(coord).map(|entry| entry.state),
                                Some(TileLoadState::Loaded | TileLoadState::Invalid)
                            )
                        })
                    };
                    let frontend_repaint_pending = frontend_repaint_passes_remaining > 0;
                    if !incomplete && orphaned_loading.is_empty() && !frontend_repaint_pending {
                        return false;
                    }

                    let mut force_frontend_repaint = false;
                    if frontend_repaint_pending {
                        // Bypass the semantic snapshot-key guard and force the window's retained
                        // view cache to refresh after this update. Merely requesting an animation
                        // frame is insufficient because GPUI can replay the previous scene.
                        this.last_synced_tile_layer_snapshot_key = None;
                        let colors = this.theme_colors(cx);
                        this.sync_tile_layer_snapshot(colors, cx);
                        force_frontend_repaint = true;

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
                                image_count = last_frontend_image_count,
                                viewport_scale = this.viewport.scale,
                                paint_bounds = ?this.canvas_tile_snapshot.paint_bounds,
                                "map_viewer frontend_tile_upload_repaint"
                            );
                        }
                        if frontend_repaint_passes_remaining == 0 {
                            tracing::debug!(
                                frontend_generation,
                                image_count = last_frontend_image_count,
                                viewport_scale = this.viewport.scale,
                                paint_bounds = ?this.canvas_tile_snapshot.paint_bounds,
                                "map_viewer frontend_tile_upload_latch_drained"
                            );
                        }
                    }

                    if incomplete || !orphaned_loading.is_empty() {
                        this.pending_viewport_refresh = true;
                        this.ensure_visible_tiles(cx);
                        if this.pending_viewport_refresh {
                            this.schedule_viewport_work_refresh(cx);
                        }
                    }
                    cx.notify();
                    force_frontend_repaint
                })?;

                if force_frontend_repaint {
                    let mut refresh_error = None;
                    let refreshed = window_handle.iter().any(|window_handle| {
                        match window_handle.update(cx, |_, window, _| {
                            window.refresh_map_image_uploads();
                        }) {
                            Ok(()) => true,
                            Err(error) => {
                                refresh_error = Some(error);
                                false
                            }
                        }
                    });
                    if !refreshed {
                        if let Some(error) = refresh_error {
                            tracing::debug!(
                                %error,
                                "map_viewer frontend_tile_upload_window_refresh_failed"
                            );
                        }
                    }
                }
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
