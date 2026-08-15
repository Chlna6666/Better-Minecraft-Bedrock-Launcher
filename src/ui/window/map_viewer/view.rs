use super::mcstructure;
use super::model::*;
use super::prelude::*;
use super::region_package;
use super::tile_state::TileLoadState;
use super::viewport::TileBounds;
use crate::ui::window::map_viewer::lifecycle::VIEWPORT_COMPOSITE_ENABLED;
use std::time::Duration;

pub use super::model::MapViewerWindowInit;

const MAP_VIEWER_DEFAULT_WINDOW_WIDTH: f32 = 1500.0;
const MAP_VIEWER_DEFAULT_WINDOW_HEIGHT: f32 = 860.0;
const MAP_VIEWER_MIN_WINDOW_WIDTH: f32 = 1040.0;
const MAP_VIEWER_MIN_WINDOW_HEIGHT: f32 = 680.0;
const MAP_VIEWER_MAX_DISPLAY_RATIO: f32 = 0.96;
const VIEWPORT_WATCHDOG_INTERVAL: Duration = Duration::from_millis(80);
const FRONTEND_TILE_REPAINT_INTERVAL: Duration = Duration::from_millis(16);
const FRONTEND_NEW_IMAGE_BUDGET_PER_REPAINT: usize = 8;
const FRONTEND_REPAINT_SAFETY_PASSES: usize = 2;
const FRONTEND_REPAINT_PROGRESS_LOG_INTERVAL: usize = 8;

fn frontend_repaint_passes(image_count: usize) -> usize {
    if image_count == 0 {
        return 0;
    }
    image_count.saturating_add(FRONTEND_NEW_IMAGE_BUDGET_PER_REPAINT - 1)
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
        matches!(entry.state, TileLoadState::Empty | TileLoadState::Invalid)
            || (entry.state == TileLoadState::Loaded && entry.image.is_some())
    })
}

impl Drop for MapViewerWindowView {
    fn drop(&mut self) {
        if let Some(completion) = self.pending_paste_task_completion.take() {
            task_manager::finish_task(
                &completion.task_id,
                "completed",
                Some(format!("{}；地图窗口已关闭", completion.message)),
            );
        }
        self.cancel_metadata_scan();
        self.cancel_active_render();
        self.cancel_professional_overlay_query();
        self.cancel_slime_window_candidate_query();
        self.preview_3d.clear_resources(true);
        self.session_generation = self.session_generation.saturating_add(1);
        self.metadata_generation = self.metadata_generation.saturating_add(1);
        self.render_generation = self.render_generation.saturating_add(1);
        crate::utils::memory_diagnostics::clear_map_viewer_memory();
        tracing::debug!(
            session_generation = self.session_generation,
            metadata_generation = self.metadata_generation,
            render_generation = self.render_generation,
            "map_viewer dropped; cancelled background render lifecycle"
        );
    }
}

impl MapViewerWindowView {
    fn release_window_resources(&mut self, cx: &mut Context<Self>) {
        self.cancel_metadata_scan();
        self.cancel_active_render();
        self.cancel_professional_overlay_query();
        self.cancel_slime_window_candidate_query();
        self.preview_3d.clear_resources(true);

        // Stored tasks are window-scoped. Dropping their handles prevents delayed refreshes
        // from keeping work alive while the native window is being removed.
        self.viewport_idle_task.take();
        self.task_updates_task.take();

        self.session_generation = self.session_generation.saturating_add(1);
        self.metadata_generation = self.metadata_generation.saturating_add(1);
        self.render_generation = self.render_generation.saturating_add(1);
        self.viewport_idle_generation = self.viewport_idle_generation.saturating_add(1);
        self.viewport_plan_generation = self.viewport_plan_generation.saturating_add(1);
        self.pending_render_image_eviction_generation = self
            .pending_render_image_eviction_generation
            .saturating_add(1);
        self.pending_viewport_refresh = false;
        self.viewport_work_refresh_scheduled = false;
        self.viewport_composite_signature = None;
        self.viewport_composite_request_id = None;
        self.metadata_loading = false;
        self.session_loading = false;

        if let Some(session) = self.render_session.take() {
            cx.background_spawn(async move {
                drop(session);
            })
            .detach();
        }

        // A tile can be referenced simultaneously by RegionManager, the retained canvas
        // snapshot and a delayed eviction entry. GPUI image resources must be released once,
        // so collect every window-owned RenderImage by ImageId before clearing the owners.
        let mut render_images = BTreeMap::<ImageId, Arc<RenderImage>>::new();
        let mut collect_image = |image: Arc<RenderImage>| {
            render_images.entry(image.id).or_insert(image);
        };

        for image in self.tile_manager.clear() {
            collect_image(image);
        }
        for tile in self.canvas_tile_snapshot.tiles.iter() {
            collect_image(tile.image.clone());
        }
        for image in self.canvas_tile_snapshot.screen_images.iter() {
            collect_image(image.image.clone());
        }
        for (_, image) in self.pending_render_image_evictions.drain(..) {
            collect_image(image);
        }
        for image in self.paste_preview_images.iter() {
            collect_image(image.image.clone());
        }
        for image in self.professional.copied_chunk_preview_images.values() {
            collect_image(image.image.clone());
        }
        for image in self.professional.entity_avatar_pool.values() {
            collect_image(image.clone());
        }
        drop(collect_image);

        self.canvas_tile_generation = self.canvas_tile_generation.saturating_add(1);
        self.canvas_tile_snapshot = Arc::new(TilePaintSnapshot {
            generation: self.canvas_tile_generation,
            ..TilePaintSnapshot::default()
        });
        self.paste_preview_images = Arc::new(Vec::new());
        self.paste_preview_images_generation =
            self.paste_preview_images_generation.saturating_add(1);
        self.professional.copied_chunk_preview_images.clear();
        self.professional.entity_avatar_pool = Arc::new(BTreeMap::new());
        self.pending_interaction_ready_tiles.clear();
        self.active_render_tiles.clear();
        self.active_render_center_tiles.clear();
        self.active_render_request_tiles.clear();
        self.last_synced_canvas_snapshot_key = None;
        self.last_synced_tile_layer_snapshot_key = None;

        let released_render_images = render_images.len();
        for image in render_images.into_values() {
            cx.drop_image(image, None);
        }

        crate::utils::memory_diagnostics::clear_map_viewer_memory();
        tracing::debug!(
            released_render_images,
            session_generation = self.session_generation,
            metadata_generation = self.metadata_generation,
            render_generation = self.render_generation,
            "map_viewer window resources released before close"
        );
    }

    fn render_external_file_drop_target(&self, cx: &mut Context<Self>) -> Div {
        div()
            .absolute()
            .inset_0()
            .can_drop(|value, _window, _cx| {
                value
                    .downcast_ref::<ExternalPaths>()
                    .is_some_and(external_paths_are_importable)
            })
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _window, cx| {
                let paths = paths.paths().to_vec();
                this.import_structure_paths_from_drop(&paths, cx);
            }))
    }
}

impl Render for MapViewerWindowView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let preview_3d_motion_active = self
            .preview_3d
            .tick_motion(now, self.preview_3d_focus_handle.is_focused(window));
        let paste_preview_auto_pan_active = self.tick_paste_preview_auto_pan(cx);
        let viewport_size_changed = self.update_viewport_size(window);
        let initial_tile_plan_pending = self.render_session.is_some()
            && self.last_visible_tile_signature.is_none()
            && !self.session_loading;
        if viewport_size_changed || initial_tile_plan_pending {
            self.ensure_visible_tiles(cx);
            self.refresh_professional_render_caches(cx);
        }
        self.frame_stats.record_frame();
        self.sync_input_values(window, cx);
        request_animation_frame_if(
            window,
            preview_3d_motion_active || paste_preview_auto_pan_active,
        );
        let colors = self.theme_colors(cx);
        let top_bar_snapshot = self.top_bar_snapshot();
        let tool_stripe_snapshot = self.tool_stripe_snapshot();
        let menu_overlay_snapshot = self.menu_overlay_snapshot();
        let top_bar_view = self.top_bar_view.clone();
        top_bar_view.update(cx, |view, cx| view.set_snapshot(top_bar_snapshot, cx));
        let tool_stripe_view = self.tool_stripe_view.clone();
        tool_stripe_view.update(cx, |view, cx| {
            view.set_snapshot(tool_stripe_snapshot, cx);
        });
        let menu_overlay_view = self.menu_overlay_view.clone();
        menu_overlay_view.update(cx, |view, cx| {
            view.set_snapshot(menu_overlay_snapshot, cx);
        });
        if !self.viewport_interaction_active() {
            self.sync_canvas_snapshot(colors, cx);
        }

        let mut root = div()
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(colors.bg)
            .key_context("MapViewer")
            .on_action(cx.listener(|this, _: &MapViewerCopyChunks, window, cx| {
                if !this.map_shortcuts_allowed(window, cx) {
                    return;
                }
                this.copy_context_chunks(cx);
            }))
            .on_action(
                cx.listener(|this, _: &MapViewerExportChunksImage, window, cx| {
                    if !this.map_shortcuts_allowed(window, cx) {
                        return;
                    }
                    this.export_chunks_image(cx);
                }),
            )
            .on_action(
                cx.listener(|this, _: &MapViewerStartPastePreview, window, cx| {
                    if !this.map_shortcuts_allowed(window, cx) {
                        return;
                    }
                    this.start_paste_preview_from_keyboard(cx);
                }),
            )
            .on_action(cx.listener(
                |this, _: &MapViewerRotatePastePreviewClockwise, window, cx| {
                    if !this.map_shortcuts_allowed(window, cx) {
                        return;
                    }
                    this.rotate_paste_preview(true, cx);
                },
            ))
            .on_action(cx.listener(
                |this, _: &MapViewerRotatePastePreviewCounterClockwise, window, cx| {
                    if !this.map_shortcuts_allowed(window, cx) {
                        return;
                    }
                    this.rotate_paste_preview(false, cx);
                },
            ))
            .on_action(
                cx.listener(|this, _: &MapViewerConfirmPastePreview, window, cx| {
                    if !this.map_shortcuts_allowed(window, cx) {
                        return;
                    }
                    this.confirm_paste_preview(cx);
                }),
            )
            .on_action(
                cx.listener(|this, _: &MapViewerCancelPastePreview, window, cx| {
                    if !this.map_shortcuts_allowed(window, cx) {
                        return;
                    }
                    if !this.cancel_paste_preview(cx) {
                        this.close_all_menus(cx);
                    }
                }),
            )
            .on_action(cx.listener(|this, _: &MapViewerUndoEdit, window, cx| {
                if !this.map_shortcuts_allowed(window, cx) {
                    return;
                }
                this.undo_map_edit(cx);
            }))
            .on_action(cx.listener(|this, _: &MapViewerRedoEdit, window, cx| {
                if !this.map_shortcuts_allowed(window, cx) {
                    return;
                }
                this.redo_map_edit(cx);
            }))
            .on_action(cx.listener(|this, _: &MapViewerOpenHistory, window, cx| {
                if !this.map_shortcuts_allowed(window, cx) {
                    return;
                }
                this.open_history_tab(cx);
            }))
            .on_action(cx.listener(|this, _: &MapViewerCreateBackup, window, cx| {
                if !this.map_shortcuts_allowed(window, cx) {
                    return;
                }
                this.create_map_backup(cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.release_pointer_captures("root left mouse up", cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.release_pointer_captures("root left mouse up out", cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.release_pointer_captures("root right mouse up", cx);
                }),
            )
            .on_mouse_up_out(
                MouseButton::Right,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.release_pointer_captures("root right mouse up out", cx);
                }),
            )
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .child(self.top_bar_view.clone())
                    .child(self.render_workspace(&colors, cx))
                    .when(self.ui_state.bottom_panel_open, |this| {
                        this.child(
                            split_handle(SplitPaneAxis::Vertical, colors.border).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, _window, cx| {
                                    this.begin_bottom_panel_resize(event.position, cx)
                                }),
                            ),
                        )
                        .child(self.render_bottom_dock(&colors, cx))
                    })
                    .child(self.render_map_status_bar(&colors, cx)),
            )
            .when(self.ui_state.dock_drag.is_some(), |this| {
                this.child(self.render_dock_drag_overlay(cx))
            })
            .child(self.render_menu_overlay(&colors, cx))
            .child(self.render_external_file_drop_target(cx));

        root
    }
}

fn external_paths_are_importable(paths: &ExternalPaths) -> bool {
    paths.paths().iter().any(|path| {
        region_package::is_region_package_path(path) || mcstructure::is_mcstructure_path(path)
    })
}

pub fn open_map_viewer_window(init: MapViewerWindowInit, cx: &mut App) {
    let title = format!("地图预览 - {}", init.asset.display_name);
    let options = map_viewer_window_options(cx);
    let window = cx.open_window(options, move |window, cx| {
        window.set_title(&title);
        window.activate_window();
        let view = cx.new(|cx| MapViewerWindowView::new(init, window, cx));
        let close_view = view.clone();
        window.on_window_should_close(cx, move |window, cx| {
            let restored_bounds = window.window_bounds().bounds();
            let prefs = crate::core::ui_prefs::MapViewerWindowPrefs {
                width: restored_bounds.size.width / px(1.0),
                height: restored_bounds.size.height / px(1.0),
            };
            if let Err(error) = crate::core::ui_prefs::save_map_viewer_window_prefs(&prefs) {
                tracing::warn!(%error, "failed to save map viewer window size");
            }
            close_view.update(cx, |this, cx| this.release_window_resources(cx));
            window.remove_window();
            true
        });
        view.update(cx, |this, cx| this.spawn_viewport_watchdog(cx));
        cx.new(|cx| crate::ui::runtime::root_view::RootView::new(view, window, cx))
    });
    if let Err(error) = window {
        eprintln!("Failed to open map viewer window: {error:?}");
    }
}

fn map_viewer_window_options(cx: &mut App) -> WindowOptions {
    let mut options = WindowOptions::default();
    options.window_bounds = Some(WindowBounds::centered(map_viewer_window_size(cx), cx));
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

fn map_viewer_window_size(cx: &App) -> Size<Pixels> {
    let saved = crate::core::ui_prefs::load_map_viewer_window_prefs();
    let display_size = cx.primary_display().map(|display| display.bounds().size);
    map_viewer_window_size_for_display(saved, display_size)
}

pub(super) fn map_viewer_window_size_for_display(
    saved: Option<crate::core::ui_prefs::MapViewerWindowPrefs>,
    display_size: Option<Size<Pixels>>,
) -> Size<Pixels> {
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
            .max(MAP_VIEWER_MIN_WINDOW_WIDTH.min(maximum_width))),
        px(requested_height
            .min(maximum_height)
            .max(MAP_VIEWER_MIN_WINDOW_HEIGHT.min(maximum_height))),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MapLayerKind {
    Terrain,
    Grid,
    ProfessionalOverlay,
    Markers,
}

pub(super) fn map_render_layer_order() -> [MapLayerKind; 4] {
    [
        MapLayerKind::Terrain,
        MapLayerKind::Grid,
        MapLayerKind::ProfessionalOverlay,
        MapLayerKind::Markers,
    ]
}

impl MapViewerWindowView {
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
                        visible_tiles
                            .iter()
                            .copied()
                            .any(|coord| !visible_tile_frontend_ready(&this.tile_manager, coord))
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
