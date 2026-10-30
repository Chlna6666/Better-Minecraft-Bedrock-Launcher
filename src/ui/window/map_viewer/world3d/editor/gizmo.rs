//! Screen-sized world-axis handles. Dragging only changes a preview or selection.
use super::*;

#[derive(Clone, Copy)]
enum DragTarget {
    Import(ChunkPos, i32),
    Selection(World3dSelection),
}

#[derive(Clone, Copy)]
pub(super) struct AxisDrag {
    axis: usize,
    start: [f32; 2],
    pixels_per_unit: [f32; 2],
    target: DragTarget,
    last_offset: i32,
}

#[derive(Clone, Copy)]
struct AxisHandle {
    axis: usize,
    start: [f32; 2],
    end: [f32; 2],
    pixels_per_unit: [f32; 2],
}

fn dot(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

fn subtract(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}

fn snapped_offset(delta: [f32; 2], direction: [f32; 2], step: i32) -> i32 {
    let units = dot(delta, direction) / dot(direction, direction);
    ((units / step as f32).round() as i32).saturating_mul(step)
}

fn hit_handle(handles: Vec<AxisHandle>, pointer: [f32; 2]) -> Option<AxisHandle> {
    handles
        .into_iter()
        .filter(|h| {
            let axis = subtract(h.end, h.start);
            let delta = subtract(pointer, h.start);
            let t = (dot(delta, axis) / dot(axis, axis)).clamp(0.18, 1.0);
            let distance = subtract(delta, axis.map(|v| v * t));
            dot(distance, distance) <= 81.0
        })
        .min_by(|a, b| {
            let a = subtract(pointer, a.end);
            let b = subtract(pointer, b.end);
            dot(a, a).total_cmp(&dot(b, b))
        })
}

fn resize_selection(mut selection: World3dSelection, axis: usize, offset: i32) -> World3dSelection {
    selection.max[axis] = selection.max[axis]
        .saturating_add(offset)
        .max(selection.min[axis]);
    if axis == 1 {
        let (_, max_y) = ChunkPos {
            x: 0,
            z: 0,
            dimension: selection.dimension,
        }
        .y_range(::bedrock_world::ChunkVersion::New);
        selection.max[axis] = selection.max[axis].min(max_y);
    }
    selection
}

fn project(camera: &World3dCamera, position: [f64; 3], size: (f32, f32)) -> Option<[f32; 2]> {
    let relative = std::array::from_fn::<_, 3, _>(|i| position[i] - camera.position[i]);
    let dot3 = |v: [f64; 3]| relative.iter().zip(v).map(|(a, b)| a * b).sum::<f64>();
    let depth = dot3(camera.forward());
    if depth <= 0.1 {
        return None;
    }
    let half_height = match camera.projection {
        World3dProjectionKind::Perspective => depth * f64::from(camera.fov_y_radians * 0.5).tan(),
        World3dProjectionKind::Orthographic => f64::from(camera.ortho_height) * 0.5,
    };
    let scale = f64::from(size.1) / (2.0 * half_height);
    Some([
        size.0 * 0.5 + (dot3(camera.right()) * scale) as f32,
        size.1 * 0.5 - (dot3(camera.up()) * scale) as f32,
    ])
}

impl World3dEditorView {
    fn axis_target(&self) -> Option<(DragTarget, [f64; 3])> {
        match self.tool {
            World3dTool::Import => {
                let preview = self.import_preview.as_ref()?;
                let anchor = preview.target_anchor;
                if anchor.dimension != self.dimension || preview.is_writing {
                    return None;
                }
                Some((
                    DragTarget::Import(anchor, preview.origin_y),
                    [
                        f64::from(anchor.x) * 16.0,
                        f64::from(preview.origin_y),
                        f64::from(anchor.z) * 16.0,
                    ],
                ))
            }
            World3dTool::Select => {
                let selection = self.selection?;
                Some((
                    DragTarget::Selection(selection),
                    selection.max.map(|v| f64::from(v) + 1.0),
                ))
            }
            _ => None,
        }
    }

    fn axis_handles(&self) -> Vec<AxisHandle> {
        let Some((_, anchor)) = self.axis_target() else {
            return Vec::new();
        };
        let Some(start) = project(&self.camera, anchor, self.stage_size) else {
            return Vec::new();
        };
        let mut handles = Vec::with_capacity(3);
        for axis in 0..3 {
            if axis == 1
                && self.tool == World3dTool::Import
                && self
                    .import_preview
                    .as_ref()
                    .is_some_and(|p| !p.is_structure)
            {
                continue;
            }
            let mut endpoint = anchor;
            // A short differential preserves the projection near the anchor.
            endpoint[axis] += 0.01;
            let Some(projected) = project(&self.camera, endpoint, self.stage_size) else {
                continue;
            };
            let direction = subtract(projected, start).map(|v| v * 100.0);
            let length = dot(direction, direction).sqrt();
            if length < 0.1 {
                continue;
            }
            handles.push(AxisHandle {
                axis,
                start,
                pixels_per_unit: direction,
                end: [
                    start[0] + direction[0] * 88.0 / length,
                    start[1] + direction[1] * 88.0 / length,
                ],
            });
        }
        handles
    }

    pub(super) fn start_axis_drag(&mut self, position: Point<Pixels>, window: &Window) -> bool {
        let (left, top, width, height) = self.stage_rect(window);
        let pointer = [position.x / px(1.0) - left, position.y / px(1.0) - top];
        if pointer[0] < 0.0 || pointer[1] < 0.0 || pointer[0] > width || pointer[1] > height {
            return false;
        }
        let Some(handle) = hit_handle(self.axis_handles(), pointer) else {
            return false;
        };
        let Some((target, _)) = self.axis_target() else {
            return false;
        };
        self.keys_down.clear();
        self.look_drag = None;
        self.axis_drag = Some(AxisDrag {
            axis: handle.axis,
            start: pointer,
            pixels_per_unit: handle.pixels_per_unit,
            target,
            last_offset: 0,
        });
        self.status = "拖动轴箭头调整；松开保留预览，Esc 撤回本次拖拽".into();
        true
    }

    pub(super) fn drag_axis_to(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(mut drag) = self.axis_drag else {
            return;
        };
        let left = if self.left_rail_open {
            LEFT_RAIL_WIDTH
        } else {
            0.0
        };
        let pointer = [
            position.x / px(1.0) - left,
            position.y / px(1.0) - TOP_BAR_HEIGHT,
        ];
        let step = if matches!(drag.target, DragTarget::Import(..)) && drag.axis != 1 {
            16
        } else {
            1
        };
        let offset = snapped_offset(subtract(pointer, drag.start), drag.pixels_per_unit, step);
        if offset == drag.last_offset {
            return;
        }
        drag.last_offset = offset;
        self.apply_axis_offset(drag, offset, cx);
        self.axis_drag = Some(drag);
        cx.notify();
    }

    fn apply_axis_offset(&mut self, drag: AxisDrag, offset: i32, cx: &mut Context<Self>) {
        match drag.target {
            DragTarget::Import(mut anchor, mut y) => {
                match drag.axis {
                    0 => anchor.x = anchor.x.saturating_add(offset / 16),
                    1 => y = y.saturating_add(offset),
                    _ => anchor.z = anchor.z.saturating_add(offset / 16),
                }
                self.request_import_action(World3dImportAction::MoveTo(anchor, y), cx);
            }
            DragTarget::Selection(selection) => {
                self.selection = Some(resize_selection(selection, drag.axis, offset));
                self.update_target_scene();
            }
        }
    }

    pub(super) fn finish_axis_drag(&mut self, cx: &mut Context<Self>) {
        if let Some(drag) = self.axis_drag.take() {
            if matches!(drag.target, DragTarget::Selection(_)) {
                self.publish_selection_to_2d(cx);
            }
            cx.notify();
        }
    }

    pub(super) fn cancel_axis_drag(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.axis_drag.take() else {
            return false;
        };
        self.apply_axis_offset(drag, 0, cx);
        self.status = "已撤回本次轴拖拽".into();
        cx.notify();
        true
    }

    pub(super) fn render_axis_gizmo(&self) -> Div {
        let handles = self.axis_handles();
        let active = self.axis_drag.map(|d| d.axis);
        let mut overlay = div().absolute().inset_0();
        for handle in &handles {
            overlay = overlay.child(
                div()
                    .absolute()
                    .left(px(handle.end[0] + 8.0))
                    .top(px(handle.end[1] - 10.0))
                    .text_size(px(13.0))
                    .text_color(axis_color(handle.axis))
                    .child(["X", "Y", "Z"][handle.axis]),
            );
        }
        overlay.child(
            canvas(
                move |_, _, _| (),
                move |bounds, _, window, _| {
                    for handle in &handles {
                        paint_handle(*handle, active, bounds.origin, window);
                    }
                },
            )
            .size_full(),
        )
    }
}

fn axis_color(axis: usize) -> Hsla {
    rgb([0xef5350, 0x43d575, 0x4d9fff][axis]).into()
}

fn paint_handle(
    handle: AxisHandle,
    active: Option<usize>,
    origin: Point<Pixels>,
    window: &mut Window,
) {
    let local = |p: [f32; 2]| origin + point(px(p[0]), px(p[1]));
    let direction = subtract(handle.end, handle.start).map(|v| v / 88.0);
    let color = if active == Some(handle.axis) {
        Hsla::white()
    } else {
        axis_color(handle.axis)
    };
    let mut line = PathBuilder::stroke(px(3.0));
    line.move_to(local(handle.start));
    line.line_to(local(handle.end));
    if let Ok(path) = line.build() {
        window.paint_path(path, color);
    }
    let mut arrow = PathBuilder::fill();
    arrow.move_to(local(handle.end));
    for sign in [-1.0, 1.0] {
        arrow.line_to(local([
            handle.end[0] - direction[0] * 14.0 - direction[1] * 7.0 * sign,
            handle.end[1] - direction[1] * 14.0 + direction[0] * 7.0 * sign,
        ]));
    }
    arrow.close();
    if let Ok(path) = arrow.build() {
        window.paint_path(path, color);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AxisHandle, Dimension, World3dCamera, World3dProjectionKind, World3dSelection, hit_handle,
        project, resize_selection, snapped_offset,
    };

    #[test]
    fn axis_drag_snaps_in_both_directions_without_cross_axis_motion() {
        assert_eq!(snapped_offset([31.0, 100.0], [2.0, 0.0], 16), 16);
        assert_eq!(snapped_offset([-31.0, 100.0], [2.0, 0.0], 16), -16);
        assert_eq!(snapped_offset([100.0, -7.0], [0.0, -2.0], 1), 4);
    }

    #[test]
    fn projection_matches_camera_rays_in_both_modes() {
        for projection in [
            World3dProjectionKind::Perspective,
            World3dProjectionKind::Orthographic,
        ] {
            let camera = World3dCamera {
                projection,
                ..World3dCamera::default()
            };
            let (origin, ray) = camera.ray(0.3, -0.4, 2.0);
            let position = std::array::from_fn(|i| origin[i] + ray[i] * 30.0);
            let projected = project(&camera, position, (800.0, 400.0)).unwrap();
            assert!((projected[0] - 520.0).abs() < 0.01);
            assert!((projected[1] - 280.0).abs() < 0.01);
            assert!(project(&camera, camera.position, (800.0, 400.0)).is_none());
        }
    }

    #[test]
    fn selection_resize_preserves_other_axes_and_can_restore_start() {
        let selection =
            World3dSelection::from_corners(Dimension::Overworld, [-5, 60, -3], [4, 70, 8]);
        let resized = resize_selection(selection, 1, 12);
        assert_eq!(resized.max, [4, 82, 8]);
        assert_eq!(resized.min, selection.min);
        assert_eq!(resize_selection(selection, 1, -100).max, [4, 60, 8]);
        assert_eq!(resize_selection(selection, 1, 1000).max, [4, 319, 8]);
        assert_eq!(resize_selection(selection, 1, 0), selection);
    }

    #[test]
    fn handle_hit_test_uses_visible_shaft_and_tip() {
        let handle = AxisHandle {
            axis: 0,
            start: [100.0, 100.0],
            end: [188.0, 100.0],
            pixels_per_unit: [2.0, 0.0],
        };
        assert!(hit_handle(vec![handle], [140.0, 105.0]).is_some());
        assert!(hit_handle(vec![handle], [190.0, 100.0]).is_some());
        assert!(hit_handle(vec![handle], [140.0, 120.0]).is_none());
        assert!(hit_handle(vec![handle], [100.0, 100.0]).is_none());
    }
}
