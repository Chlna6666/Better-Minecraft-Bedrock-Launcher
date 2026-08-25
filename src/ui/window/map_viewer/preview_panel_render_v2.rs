use super::model::*;
use super::panels::*;
use super::prelude::*;
use super::preview_3d::{
    preview_3d_chunk_mesh_is_visible, preview_3d_local_draw_parameters,
    preview_3d_world_draw_parameters,
};
use super::preview_detached::Preview3dDetachDrag;

#[derive(Clone, Copy, Debug)]
struct Preview3dWorldFrame {
    center: [f32; 3],
    fit_scale: f32,
}

impl MapViewerWindowView {
    pub(super) fn render_preview_3d_panel(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let selection = self.preview_3d_selection_status();
        let stats = self.preview_3d_stats_label();
        let mesh = self.preview_3d.mesh.clone();
        let camera = self.preview_3d.camera;
        let model_rotation = self.preview_3d.model_rotation;
        let view = cx.entity();
        let detach_view = view.downgrade();

        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .px(px(10.0))
            .pt(px(10.0))
            .min_w(px(0.0))
            .cursor_move()
            .on_drag(
                Preview3dDetachDrag,
                move |_drag: &Preview3dDetachDrag, position, window, cx| {
                    let window_origin = window.bounds().origin;
                    let detached_origin = point(
                        window_origin.x + position.x - px(140.0),
                        window_origin.y + position.y - px(28.0),
                    );
                    if let Some(view) = detach_view.upgrade() {
                        view.update(cx, |this, cx| {
                            this.open_detached_preview_3d(Some(detached_origin), cx)
                        });
                    }
                    cx.new(|_| Preview3dDetachDrag)
                },
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.0))
                    .gap(px(3.0))
                    .child(panel_title(colors, "3D 预览"))
                    .child(
                        div()
                            .min_w(px(0.0))
                            .line_clamp(2)
                            .text_color(colors.text_muted)
                            .child(selection),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .flex_wrap()
                    .gap(px(6.0))
                    .child(toolbar_button(colors, "独立窗口").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.open_detached_preview_3d(None, cx)
                        }),
                    ))
                    .child(toolbar_button(colors, "加载/刷新").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.refresh_preview_3d_exact(cx)
                        }),
                    ))
                    .child(toolbar_button(colors, "重置视角").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.reset_preview_3d_camera(cx)
                        }),
                    ))
                    .child(dock_close_button(colors).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| this.close_right_panel(cx)),
                    )),
            );

        div()
            .size_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .p(px(10.0))
            .child(
                div()
                    .size_full()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .rounded(px(crate::ui::theme::tokens::radius::SM))
                    .border_1()
                    .border_color(Hsla {
                        a: 0.24,
                        ..colors.border
                    })
                    .bg(Hsla {
                        a: 0.38,
                        ..colors.surface_hover
                    })
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .overflow_hidden()
                    .text_size(px(12.0))
                    .text_color(colors.text_secondary)
                    .child(header)
                    // Loading state is deliberately not rendered as a placeholder.
                    // On first load this area is simply the canvas; the first one/few
                    // chunks replace the empty scene as soon as their mesh is ready.
                    .when(mesh.is_some(), |this| {
                        this.child(
                            div()
                                .px(px(10.0))
                                .min_w(px(0.0))
                                .line_clamp(2)
                                .text_color(colors.text_muted)
                                .child(stats),
                        )
                    })
                    .child(self.render_preview_3d_canvas(
                        colors,
                        mesh,
                        camera,
                        model_rotation,
                        view,
                        cx,
                    )),
            )
    }

    pub(super) fn render_preview_3d_canvas(
        &self,
        colors: &ThemeColors,
        mesh: Option<Arc<Preview3dMesh>>,
        camera: Preview3dCamera,
        model_rotation: Preview3dModelRotation,
        view: Entity<Self>,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut panel = div()
            .flex_1()
            .w_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .mx(px(10.0))
            .mb(px(10.0))
            .rounded(px(crate::ui::theme::tokens::radius::SM))
            .border_1()
            .border_color(Hsla {
                a: 0.20,
                ..colors.border
            })
            .bg(colors.surface)
            .track_focus(&self.preview_3d_focus_handle)
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.preview_3d_focus_handle.focus(window);
                    this.cancel_pointer_captures_for_panel_interaction("preview_3d mouse down", cx);
                    this.preview_3d_begin_drag(Preview3dDragMode::RotateModel, event.position, cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.preview_3d_focus_handle.focus(window);
                    this.cancel_pointer_captures_for_panel_interaction(
                        "preview_3d right mouse down",
                        cx,
                    );
                    this.preview_3d_begin_drag(Preview3dDragMode::OrbitCamera, event.position, cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.release_preview_3d_pointer_capture("preview_3d mouse up", cx) {
                        cx.stop_propagation();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.release_preview_3d_pointer_capture("preview_3d right mouse up", cx) {
                        cx.stop_propagation();
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.release_preview_3d_pointer_capture("preview_3d mouse up out", cx) {
                        cx.stop_propagation();
                    }
                }),
            )
            .on_mouse_up_out(
                MouseButton::Right,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    if this.release_preview_3d_pointer_capture("preview_3d right mouse up out", cx)
                    {
                        cx.stop_propagation();
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if event.pressed_button.is_none() {
                    this.release_preview_3d_pointer_capture(
                        "preview_3d mouse move without pressed button",
                        cx,
                    );
                    cx.stop_propagation();
                    return;
                }
                if this.preview_3d.drag.is_none() {
                    this.release_preview_3d_pointer_capture(
                        "preview_3d mouse move without preview drag",
                        cx,
                    );
                    cx.stop_propagation();
                    return;
                }
                match event.pressed_button {
                    Some(MouseButton::Left) => this.preview_3d_rotate_model_to(event.position, cx),
                    Some(MouseButton::Right) => this.preview_3d_orbit_camera_to(event.position, cx),
                    _ => {
                        this.release_preview_3d_pointer_capture(
                            "preview_3d mouse move with unsupported button",
                            cx,
                        );
                    }
                }
                cx.stop_propagation();
            }))
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
                let delta = event.delta.pixel_delta(px(48.0));
                let factor = if delta.y > px(0.0) { 1.12 } else { 0.90 };
                this.preview_3d_zoom_by(factor, cx);
                cx.stop_propagation();
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let key = event.keystroke.key.as_str();
                if is_preview_3d_navigation_key(key) {
                    this.preview_3d_press_navigation_key(
                        key,
                        event.keystroke.modifiers,
                        window,
                        cx,
                    );
                    cx.stop_propagation();
                }
            }))
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, _window, cx| {
                let key = event.keystroke.key.as_str();
                if is_preview_3d_navigation_key(key) {
                    this.preview_3d_release_navigation_key(key, cx);
                    cx.stop_propagation();
                }
            }))
            .on_modifiers_changed(cx.listener(
                |this, event: &ModifiersChangedEvent, window, cx| {
                    this.preview_3d_sync_modifier_navigation(event.modifiers, window, cx);
                    cx.stop_propagation();
                },
            ));

        if let Some(mesh) = mesh {
            let view_for_paint = view.clone();
            let selection_bounds = self.preview_3d.signature.map(|signature| signature.bounds);
            let world_frame = preview_3d_world_frame(mesh.as_ref(), selection_bounds);
            panel = panel.child(
                div()
                    .relative()
                    .size_full()
                    .overflow_hidden()
                    .child(
                        canvas(
                            move |bounds, _window, _cx| bounds,
                            move |bounds, _prepaint, window, _cx| {
                                let _ = &view_for_paint;
                                let width = f32::from(bounds.size.width);
                                let height = f32::from(bounds.size.height);
                                let aspect = if height <= 0.0 { 1.0 } else { width / height };
                                let world_parameters = preview_3d_world_draw_parameters(
                                    aspect,
                                    world_frame.center,
                                    world_frame.fit_scale,
                                    camera,
                                    model_rotation,
                                );
                                for chunk_mesh in &mesh.chunk_meshes {
                                    let gpu_mesh = chunk_mesh.selected_gpu_mesh(camera);
                                    let parameters = preview_3d_local_draw_parameters(
                                        &world_parameters,
                                        chunk_mesh.world_origin,
                                    );
                                    if !preview_3d_chunk_mesh_is_visible(chunk_mesh, &parameters) {
                                        continue;
                                    }
                                    window.paint_gpu_mesh_3d(bounds, gpu_mesh, parameters);
                                }
                            },
                        )
                        .absolute()
                        .inset_0(),
                    ),
            );
        }

        panel
    }
}

fn preview_3d_world_frame(
    mesh: &Preview3dMesh,
    selection_bounds: Option<bedrock_world::SlimeChunkBounds>,
) -> Preview3dWorldFrame {
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

    Preview3dWorldFrame {
        center: [center_x, center_y, center_z],
        fit_scale: 1.48 / fitted_span,
    }
}

fn is_preview_3d_navigation_key(key: &str) -> bool {
    matches!(
        key,
        "w" | "a" | "s" | "d" | "up" | "left" | "down" | "right" | "space" | "shift"
    )
}
