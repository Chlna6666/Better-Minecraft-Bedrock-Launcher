use super::mcstructure;
use super::model::*;
use super::prelude::*;
use super::region_package;

pub use super::model::MapViewerWindowInit;

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
        window.on_window_should_close(cx, |window, _cx| {
            window.remove_window();
            true
        });
        window.activate_window();
        let view = cx.new(|cx| MapViewerWindowView::new(init, window, cx));
        cx.new(|cx| crate::ui::runtime::root_view::RootView::new(view, window, cx))
    });
    if let Err(error) = window {
        eprintln!("Failed to open map viewer window: {error:?}");
    }
}

fn map_viewer_window_options(cx: &mut App) -> WindowOptions {
    let mut options = WindowOptions::default();
    options.window_bounds = Some(WindowBounds::centered(size(px(1280.0), px(860.0)), cx));
    options.window_min_size = Some(size(px(920.0), px(620.0)));
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
