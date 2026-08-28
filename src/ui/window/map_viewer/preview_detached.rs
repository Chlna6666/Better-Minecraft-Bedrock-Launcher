use super::model::*;
use super::panels::*;
use super::prelude::*;
use super::preview_3d::{
    preview_3d_chunk_mesh_is_visible, preview_3d_local_draw_parameters,
    preview_3d_world_draw_parameters,
};
use std::cell::RefCell;

const DETACHED_PREVIEW_WIDTH: f32 = 920.0;
const DETACHED_PREVIEW_HEIGHT: f32 = 680.0;
const DETACHED_PREVIEW_MIN_WIDTH: f32 = 460.0;
const DETACHED_PREVIEW_MIN_HEIGHT: f32 = 340.0;

thread_local! {
    static DETACHED_PREVIEW_WINDOWS: RefCell<BTreeMap<u64, AnyWindowHandle>> =
        RefCell::new(BTreeMap::new());
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Preview3dDetachDrag;

impl Render for Preview3dDetachDrag {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = detached_theme_colors(cx);
        let i18n = cx.global::<I18n>().clone();
        div()
            .px(px(12.0))
            .py(px(7.0))
            .rounded(px(crate::ui::theme::tokens::radius::MD))
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .text_size(px(12.0))
            .text_color(colors.text_primary)
            .child(t!("MapViewer.preview_model"))
    }
}

#[derive(Clone, Copy, Debug)]
enum DetachedPreviewDrag {
    RotateModel(Point<Pixels>),
    OrbitCamera(Point<Pixels>),
}

pub(super) struct DetachedPreview3dView {
    owner: WeakEntity<MapViewerWindowView>,
    owner_id: u64,
    focus: FocusHandle,
    drag: Option<DetachedPreviewDrag>,
    _subscriptions: Vec<Subscription>,
}

impl Drop for DetachedPreview3dView {
    fn drop(&mut self) {
        let owner_id = self.owner_id;
        DETACHED_PREVIEW_WINDOWS.with(|windows| {
            windows.borrow_mut().remove(&owner_id);
        });
    }
}

impl DetachedPreview3dView {
    fn new(
        owner: Entity<MapViewerWindowView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let owner_id = owner.entity_id().as_u64();
        let mut subscriptions = Vec::new();
        subscriptions.push(cx.observe_in(&owner, window, |_this, owner, window, cx| {
            let main = owner.read(cx);
            if main.ui_state.right_panel_open
                && main.ui_state.active_right_panel == MapViewerRightPanel::Preview3d
            {
                window.remove_window();
                return;
            }
            cx.notify();
        }));
        subscriptions.push(
            cx.observe_release_in(&owner, window, |_this, _owner, window, _cx| {
                window.remove_window();
            }),
        );
        subscriptions.push(cx.observe_global::<ThemeState>(|_this, cx| cx.notify()));
        subscriptions.push(cx.observe_global::<I18n>(|_this, cx| cx.notify()));

        Self {
            owner: owner.downgrade(),
            owner_id,
            focus: cx.focus_handle().tab_stop(true),
            drag: None,
            _subscriptions: subscriptions,
        }
    }

    fn with_owner(
        &self,
        cx: &mut Context<Self>,
        update: impl FnOnce(&mut MapViewerWindowView, &mut Context<MapViewerWindowView>),
    ) {
        if let Some(owner) = self.owner.upgrade() {
            owner.update(cx, update);
        }
    }

    fn begin_drag(&mut self, mode: Preview3dDragMode, position: Point<Pixels>) {
        self.drag = Some(match mode {
            Preview3dDragMode::RotateModel => DetachedPreviewDrag::RotateModel(position),
            Preview3dDragMode::OrbitCamera => DetachedPreviewDrag::OrbitCamera(position),
        });
    }

    fn update_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(drag) = self.drag else {
            return;
        };
        let previous = match drag {
            DetachedPreviewDrag::RotateModel(previous)
            | DetachedPreviewDrag::OrbitCamera(previous) => previous,
        };
        let delta_x = (position.x - previous.x) / px(1.0);
        let delta_y = (position.y - previous.y) / px(1.0);
        match drag {
            DetachedPreviewDrag::RotateModel(_) => {
                self.with_owner(cx, |main, cx| {
                    main.preview_3d.model_rotation.rotate_drag(delta_x, delta_y);
                    cx.notify();
                });
                self.drag = Some(DetachedPreviewDrag::RotateModel(position));
            }
            DetachedPreviewDrag::OrbitCamera(_) => {
                self.with_owner(cx, |main, cx| {
                    main.preview_3d.camera.rotate_view(delta_x, delta_y);
                    cx.notify();
                });
                self.drag = Some(DetachedPreviewDrag::OrbitCamera(position));
            }
        }
        cx.notify();
    }
}

impl Render for DetachedPreview3dView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = detached_theme_colors(cx);
        let i18n = cx.global::<I18n>().clone();
        let title = t!("MapViewer.preview_model");
        window.set_title(&title);
        let snapshot = self.owner.upgrade().map(|owner| {
            let main = owner.read(cx);
            (
                main.preview_3d.mesh.clone(),
                main.preview_3d.camera,
                main.preview_3d.model_rotation,
                main.preview_3d.signature.map(|signature| signature.bounds),
                main.preview_3d.render_in_flight,
                main.preview_3d_stats_label(),
            )
        });

        let (mesh, camera, model_rotation, selection_bounds, loading, stats) =
            snapshot.unwrap_or((
                None,
                Preview3dCamera::default(),
                Preview3dModelRotation::default(),
                None,
                false,
                SharedString::from(""),
            ));

        let owner_for_refresh = self.owner.clone();
        let owner_for_reset = self.owner.clone();
        let owner_for_dock = self.owner.clone();

        div()
            .size_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .bg(colors.bg)
            .track_focus(&self.focus)
            .child(
                div()
                    .h(px(52.0))
                    .flex_none()
                    .px(px(12.0))
                    .border_b_1()
                    .border_color(colors.border)
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(panel_title(&colors, t!("MapViewer.preview_model")))
                            .when(mesh.is_some() && loading, |this| {
                                this.child(status_badge(&colors, t!("MapViewer.streaming_update")))
                            }),
                    )
                    .child(
                        toolbar_button(&colors, t!("MapViewer.refresh")).on_mouse_down(
                            MouseButton::Left,
                            move |_event, _window, cx| {
                                if let Some(owner) = owner_for_refresh.upgrade() {
                                    owner.update(cx, |main, cx| main.refresh_preview_3d_exact(cx));
                                }
                            },
                        ),
                    )
                    .child(
                        toolbar_button(&colors, t!("MapViewer.reset_view")).on_mouse_down(
                            MouseButton::Left,
                            move |_event, _window, cx| {
                                if let Some(owner) = owner_for_reset.upgrade() {
                                    owner.update(cx, |main, cx| main.reset_preview_3d_camera(cx));
                                }
                            },
                        ),
                    )
                    .child(
                        toolbar_button(&colors, t!("MapViewer.redock")).on_mouse_down(
                            MouseButton::Left,
                            move |_event, _window, cx| {
                                if let Some(owner) = owner_for_dock.upgrade() {
                                    owner.update(cx, |main, cx| {
                                        main.show_right_preview_3d_panel(cx);
                                        cx.notify();
                                    });
                                }
                            },
                        ),
                    ),
            )
            .when(mesh.is_some(), |this| {
                this.child(
                    div()
                        .h(px(28.0))
                        .flex_none()
                        .px(px(12.0))
                        .flex()
                        .items_center()
                        .text_size(px(11.0))
                        .text_color(colors.text_muted)
                        .child(stats),
                )
            })
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .m(px(10.0))
                    .rounded(px(crate::ui::theme::tokens::radius::SM))
                    .border_1()
                    .border_color(colors.border)
                    .bg(colors.surface)
                    .overflow_hidden()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            this.focus.focus(window);
                            this.begin_drag(Preview3dDragMode::RotateModel, event.position);
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            this.focus.focus(window);
                            this.begin_drag(Preview3dDragMode::OrbitCamera, event.position);
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                        if event.pressed_button.is_some() && this.drag.is_some() {
                            this.update_drag(event.position, cx);
                            cx.stop_propagation();
                        } else if event.pressed_button.is_none() {
                            this.drag = None;
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                            this.drag = None;
                            cx.notify();
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Right,
                        cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                            this.drag = None;
                            cx.notify();
                            cx.stop_propagation();
                        }),
                    )
                    .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
                        let delta = event.delta.pixel_delta(px(48.0));
                        let factor = if delta.y > px(0.0) { 1.12 } else { 0.90 };
                        this.with_owner(cx, |main, cx| main.preview_3d_zoom_by(factor, cx));
                        cx.stop_propagation();
                    }))
                    .when_some(mesh, |this, mesh| {
                        let frame = detached_preview_world_frame(mesh.as_ref(), selection_bounds);
                        this.child(
                            canvas(
                                move |bounds, _window, _cx| bounds,
                                move |bounds, _prepaint, window, _cx| {
                                    let width = f32::from(bounds.size.width);
                                    let height = f32::from(bounds.size.height);
                                    let aspect = if height <= 0.0 { 1.0 } else { width / height };
                                    let world_parameters = preview_3d_world_draw_parameters(
                                        aspect,
                                        frame.center,
                                        frame.fit_scale,
                                        camera,
                                        model_rotation,
                                    );
                                    for chunk_mesh in &mesh.chunk_meshes {
                                        let gpu_mesh = chunk_mesh.selected_gpu_mesh(camera);
                                        let parameters = preview_3d_local_draw_parameters(
                                            &world_parameters,
                                            chunk_mesh.world_origin,
                                        );
                                        if preview_3d_chunk_mesh_is_visible(chunk_mesh, &parameters)
                                        {
                                            window.paint_gpu_mesh_3d(bounds, gpu_mesh, parameters);
                                        }
                                    }
                                },
                            )
                            .absolute()
                            .inset_0(),
                        )
                    }),
            )
    }
}

#[derive(Clone, Copy)]
struct DetachedPreviewWorldFrame {
    center: [f32; 3],
    fit_scale: f32,
}

fn detached_preview_world_frame(
    mesh: &Preview3dMesh,
    selection_bounds: Option<bedrock_world::SlimeChunkBounds>,
) -> DetachedPreviewWorldFrame {
    let (center_x, center_z, horizontal_span) = selection_bounds.map_or_else(
        || {
            let min_x = f64::from(mesh.min_x);
            let max_x = f64::from(mesh.max_x) + 1.0;
            let min_z = f64::from(mesh.min_z);
            let max_z = f64::from(mesh.max_z) + 1.0;
            (
                ((min_x + max_x) * 0.5) as f32,
                ((min_z + max_z) * 0.5) as f32,
                (max_x - min_x).max(max_z - min_z).max(1.0) as f32,
            )
        },
        |bounds| {
            let min_x = f64::from(bounds.min_chunk_x.saturating_mul(16));
            let max_x = f64::from(bounds.max_chunk_x.saturating_add(1).saturating_mul(16));
            let min_z = f64::from(bounds.min_chunk_z.saturating_mul(16));
            let max_z = f64::from(bounds.max_chunk_z.saturating_add(1).saturating_mul(16));
            (
                ((min_x + max_x) * 0.5) as f32,
                ((min_z + max_z) * 0.5) as f32,
                (max_x - min_x).max(max_z - min_z).max(1.0) as f32,
            )
        },
    );
    let min_y = f64::from(mesh.min_y);
    let max_y = f64::from(mesh.max_y) + 1.0;
    let center_y = ((min_y + max_y) * 0.5) as f32;
    let vertical_span = (max_y - min_y).max(1.0) as f32;
    let fitted_span = horizontal_span.max(vertical_span * 1.25).max(1.0);
    DetachedPreviewWorldFrame {
        center: [center_x, center_y, center_z],
        fit_scale: 1.48 / fitted_span,
    }
}

fn detached_theme_colors(cx: &App) -> ThemeColors {
    let theme = cx.global::<ThemeState>();
    lerp_theme_colors(
        &LightColors::colors(),
        &DarkColors::colors(),
        theme.factor(Instant::now()),
        theme.accent,
    )
}

impl MapViewerWindowView {
    pub(super) fn open_detached_preview_3d(
        &mut self,
        origin: Option<Point<Pixels>>,
        cx: &mut Context<Self>,
    ) {
        let owner_id = cx.entity_id().as_u64();
        let existing =
            DETACHED_PREVIEW_WINDOWS.with(|windows| windows.borrow().get(&owner_id).copied());
        if let Some(handle) = existing {
            if handle
                .update(cx, |_view, window, _cx| window.activate_window())
                .is_ok()
            {
                self.ui_state.active_right_panel = MapViewerRightPanel::Nbt;
                self.ui_state.set_right_panel_open(false);
                self.update_viewport_after_dock_change(cx);
                cx.notify();
                return;
            }
            DETACHED_PREVIEW_WINDOWS.with(|windows| {
                windows.borrow_mut().remove(&owner_id);
            });
        }

        let preview_size = size(px(DETACHED_PREVIEW_WIDTH), px(DETACHED_PREVIEW_HEIGHT));
        let mut options = WindowOptions::default();
        options.window_bounds = Some(match origin {
            Some(origin) => WindowBounds::Windowed(Bounds::new(origin, preview_size)),
            None => WindowBounds::centered(preview_size, cx),
        });
        options.window_min_size = Some(size(
            px(DETACHED_PREVIEW_MIN_WIDTH),
            px(DETACHED_PREVIEW_MIN_HEIGHT),
        ));
        options.is_resizable = true;
        options.is_minimizable = true;
        options.is_movable = true;
        #[cfg(windows)]
        {
            let title = t!("MapViewer.preview_model");
            options.titlebar = Some(TitlebarOptions {
                title: Some(title),
                appears_transparent: false,
                ..Default::default()
            });
            options.window_background = WindowBackgroundAppearance::Opaque;
        }

        let owner = cx.entity();
        let result = cx.open_window(options, move |window, cx| {
            let title = t!("MapViewer.preview_model");
            window.set_title(&title);
            window.activate_window();
            let owner_for_close = owner_id;
            window.on_window_should_close(cx, move |window, _cx| {
                DETACHED_PREVIEW_WINDOWS.with(|windows| {
                    windows.borrow_mut().remove(&owner_for_close);
                });
                window.remove_window();
                true
            });
            cx.new(|cx| DetachedPreview3dView::new(owner.clone(), window, cx))
        });

        match result {
            Ok(handle) => {
                DETACHED_PREVIEW_WINDOWS.with(|windows| {
                    windows.borrow_mut().insert(owner_id, handle.into());
                });
                // Do not call close_right_panel: that method releases the shared 3D
                // state. The independent window and map view intentionally share one
                // model/camera state, while only one of them is visible at a time.
                self.ui_state.active_right_panel = MapViewerRightPanel::Nbt;
                self.ui_state.set_right_panel_open(false);
                self.update_viewport_after_dock_change(cx);
                self.status = t!("MapViewer.preview_detached_opened");
                cx.notify();
            }
            Err(error) => {
                self.status = t!(
                    "MapViewer.preview_detached_failed",
                    message = &error.to_string()
                );
                cx.notify();
            }
        }
    }
}
