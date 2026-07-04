use super::model::*;
use super::panels::*;
use super::prelude::*;
use super::preview_3d::preview_3d_draw_parameters;

impl MapViewerWindowView {
    pub(super) fn render_preview_3d_panel(
        &self,
        colors: &ThemeColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let selection = self.preview_3d_selection_status();
        let status = self.preview_3d_status_label();
        let stats = self.preview_3d_stats_label();
        let mesh = self.preview_3d.mesh.clone();
        let camera = self.preview_3d.camera;
        let model_rotation = self.preview_3d.model_rotation;
        let view = cx.entity();
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
                .rounded(px(6.0))
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
                .gap(px(10.0))
                .overflow_hidden()
                .text_size(px(12.0))
                .text_color(colors.text_secondary)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8.0))
                        .px(px(10.0))
                        .pt(px(10.0))
                        .min_w(px(0.0))
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
                                .child(toolbar_button(colors, "加载/刷新").on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _event, _window, cx| {
                                        this.refresh_preview_3d(cx)
                                    }),
                                ))
                                .child(toolbar_button(colors, "重置视角").on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _event, _window, cx| {
                                        this.reset_preview_3d_camera(cx)
                                    }),
                                ))
                                .child(toolbar_button(colors, "收起").on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _event, _window, cx| {
                                        this.toggle_right_panel(cx)
                                    }),
                                )),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap(px(8.0))
                        .px(px(10.0))
                        .min_w(px(0.0))
                        .child(status_badge(colors, status))
                        .child(
                            div()
                                .min_w(px(0.0))
                                .line_clamp(3)
                                .text_color(colors.text_muted)
                                .child(stats),
                        ),
                )
                .child(
                    div()
                        .px(px(10.0))
                        .min_w(px(0.0))
                        .text_color(colors.text_muted)
                        .child(
                            "左键拖动模型，右键拖动镜头，WASD / 方向键 / Space / Shift 移动视角",
                        ),
                )
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
            .rounded(px(6.0))
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
                    this.release_pointer_captures("preview_3d mouse up", cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.release_pointer_captures("preview_3d right mouse up", cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.release_pointer_captures("preview_3d mouse up out", cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Right,
                cx.listener(|this, _event: &MouseUpEvent, _window, cx| {
                    this.release_pointer_captures("preview_3d right mouse up out", cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if event.pressed_button.is_none() {
                    this.release_pointer_captures(
                        "preview_3d mouse move without pressed button",
                        cx,
                    );
                    cx.stop_propagation();
                    return;
                }
                if this.preview_3d.drag.is_none() {
                    this.release_pointer_captures("preview_3d mouse move without preview drag", cx);
                    cx.stop_propagation();
                    return;
                }
                match event.pressed_button {
                    Some(MouseButton::Left) => this.preview_3d_rotate_model_to(event.position, cx),
                    Some(MouseButton::Right) => this.preview_3d_orbit_camera_to(event.position, cx),
                    _ => {
                        this.release_pointer_captures(
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
                                for chunk_mesh in &mesh.chunk_meshes {
                                    let parameters = preview_3d_draw_parameters(
                                        aspect,
                                        &chunk_mesh.gpu_mesh,
                                        camera,
                                        model_rotation,
                                    );
                                    window.paint_gpu_mesh_3d(
                                        bounds,
                                        chunk_mesh.gpu_mesh.clone(),
                                        parameters,
                                    );
                                }
                            },
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .child(preview_3d_axis_overlay(colors, camera, model_rotation)),
            );
        } else {
            panel = panel
                .relative()
                .flex()
                .items_center()
                .justify_center()
                .child(self.preview_3d_empty_label())
                .child(preview_3d_axis_overlay(colors, camera, model_rotation));
        }
        panel
    }
}

fn is_preview_3d_navigation_key(key: &str) -> bool {
    matches!(
        key,
        "w" | "a" | "s" | "d" | "up" | "left" | "down" | "right" | "space" | "shift"
    )
}

fn preview_3d_axis_overlay(
    colors: &ThemeColors,
    camera: Preview3dCamera,
    model_rotation: Preview3dModelRotation,
) -> Div {
    div()
        .absolute()
        .top(px(8.0))
        .right(px(8.0))
        .w(px(72.0))
        .h(px(72.0))
        .rounded(px(6.0))
        .border_1()
        .border_color(Hsla {
            a: 0.16,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.72,
            ..colors.surface
        })
        .child(
            canvas(
                move |bounds, _window, _cx| bounds,
                move |bounds, _prepaint, window, _cx| {
                    draw_preview_3d_axis_gizmo(bounds, camera, model_rotation, window);
                },
            )
            .absolute()
            .inset_0(),
        )
}

fn draw_preview_3d_axis_gizmo(
    bounds: Bounds<Pixels>,
    camera: Preview3dCamera,
    model_rotation: Preview3dModelRotation,
    window: &mut Window,
) {
    let center = point(
        bounds.left() + bounds.size.width * 0.45,
        bounds.top() + bounds.size.height * 0.58,
    );
    let axis_length = 30.0;
    let axes = [
        ([axis_length, 0.0, 0.0], rgb(0xff2020)),
        ([0.0, axis_length, 0.0], rgb(0x20ff5a)),
        ([0.0, 0.0, axis_length], rgb(0x1ea7ff)),
    ];
    for (axis, color) in axes {
        let (end_x, end_y) = preview_3d_axis_gizmo_endpoint(axis, camera, model_rotation);
        let mut builder = PathBuilder::stroke(px(1.6));
        builder.move_to(center);
        builder.line_to(point(center.x + px(end_x), center.y + px(end_y)));
        if let Ok(path) = builder.build() {
            window.paint_path(path, color);
        }
    }
}

fn preview_3d_axis_gizmo_endpoint(
    axis: [f32; 3],
    camera: Preview3dCamera,
    model_rotation: Preview3dModelRotation,
) -> (f32, f32) {
    let mut axis = preview_3d_rotate_axis(axis, model_rotation);
    if model_rotation.mirror_x {
        axis[0] = -axis[0];
    }
    if model_rotation.mirror_z {
        axis[2] = -axis[2];
    }
    let right = camera.right();
    let up = preview_3d_axis_gizmo_up(camera);
    (
        preview_3d_axis_vec3_dot(axis, right),
        -preview_3d_axis_vec3_dot(axis, up),
    )
}

fn preview_3d_rotate_axis(axis: [f32; 3], model_rotation: Preview3dModelRotation) -> [f32; 3] {
    let (yaw_sin, yaw_cos) = model_rotation.yaw.sin_cos();
    let (pitch_sin, pitch_cos) = model_rotation.pitch.sin_cos();
    let pitched = [
        axis[0],
        axis[1] * pitch_cos - axis[2] * pitch_sin,
        axis[1] * pitch_sin + axis[2] * pitch_cos,
    ];
    [
        pitched[0] * yaw_cos + pitched[2] * yaw_sin,
        pitched[1],
        -pitched[0] * yaw_sin + pitched[2] * yaw_cos,
    ]
}

fn preview_3d_axis_gizmo_up(camera: Preview3dCamera) -> [f32; 3] {
    let forward = camera.forward();
    let right = camera.right();
    preview_3d_axis_vec3_normalize(preview_3d_axis_vec3_cross(right, forward))
}

fn preview_3d_axis_vec3_dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn preview_3d_axis_vec3_cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn preview_3d_axis_vec3_normalize(value: [f32; 3]) -> [f32; 3] {
    let length = preview_3d_axis_vec3_dot(value, value).sqrt().max(0.0001);
    [value[0] / length, value[1] / length, value[2] / length]
}
