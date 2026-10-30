//! 2D camera overlay layer showing the live 3D editor camera pose.
//!
//! Updates are isolated to this retained layer: camera movement never touches tile
//! snapshots, tile images or tile decoding.

use super::super::model::MapViewport;
use super::super::paint::{overlay_marker_screen_x, overlay_marker_screen_y};
use super::types::{CameraOverlayMode, World3dCameraSnapshot, World3dProjectionKind};
use crate::ui::theme::colors::ThemeColors;
use bedrock_render::{Dimension, RenderLayout};
use gpui::*;

/// Snapshot used to project world coordinates into the 2D map canvas.
#[derive(Clone, Copy)]
pub(crate) struct CameraOverlayContext {
    pub(crate) stage_origin: Point<Pixels>,
    pub(crate) viewport: MapViewport,
    pub(crate) layout: RenderLayout,
    pub(crate) colors: ThemeColors,
    pub(crate) dimension: Dimension,
}

#[derive(Clone, Copy)]
struct CameraOverlaySnapshot {
    context: Option<CameraOverlayContext>,
    camera: Option<World3dCameraSnapshot>,
    mode: CameraOverlayMode,
}

impl CameraOverlaySnapshot {
    fn same_as(&self, other: &Self) -> bool {
        self.mode == other.mode
            && option_camera_key(self.camera) == option_camera_key(other.camera)
            && option_context_key(self.context) == option_context_key(other.context)
    }
}

fn option_camera_key(camera: Option<World3dCameraSnapshot>) -> Option<(u64, [u64; 3], u32, u32)> {
    camera.map(|camera| {
        (
            camera.generation,
            camera.position.map(f64::to_bits),
            camera.yaw.to_bits(),
            camera.pitch.to_bits(),
        )
    })
}

fn option_context_key(
    context: Option<CameraOverlayContext>,
) -> Option<([u32; 2], [u32; 4], u32, u32)> {
    context.map(|context| {
        (
            [
                (context.stage_origin.x / px(1.0)).to_bits(),
                (context.stage_origin.y / px(1.0)).to_bits(),
            ],
            [
                context.viewport.offset_x.to_bits(),
                context.viewport.offset_y.to_bits(),
                context.viewport.scale.to_bits(),
                context.viewport.width.to_bits(),
            ],
            context.layout.pixels_per_block,
            context.dimension.id() as u32,
        )
    })
}

/// Retained overlay entity owned by the 2D map canvas.
pub(crate) struct MapCameraOverlayView {
    snapshot: CameraOverlaySnapshot,
}

impl Default for MapCameraOverlayView {
    fn default() -> Self {
        Self {
            snapshot: CameraOverlaySnapshot {
                context: None,
                camera: None,
                mode: CameraOverlayMode::Hidden,
            },
        }
    }
}

impl MapCameraOverlayView {
    pub(crate) fn set_context(&mut self, context: CameraOverlayContext, cx: &mut Context<Self>) {
        let mut next = self.snapshot;
        next.context = Some(context);
        if !next.same_as(&self.snapshot) {
            self.snapshot = next;
            cx.notify();
        }
    }

    pub(crate) fn set_camera(
        &mut self,
        camera: Option<World3dCameraSnapshot>,
        mode: CameraOverlayMode,
        cx: &mut Context<Self>,
    ) {
        let mut next = self.snapshot;
        next.camera = camera;
        next.mode = mode;
        if !next.same_as(&self.snapshot) {
            self.snapshot = next;
            cx.notify();
        }
    }

    pub(super) fn clear(&mut self, cx: &mut Context<Self>) {
        let mut next = self.snapshot;
        next.camera = None;
        next.mode = CameraOverlayMode::Hidden;
        if !next.same_as(&self.snapshot) {
            self.snapshot = next;
            cx.notify();
        }
    }

    fn active_camera(&self) -> Option<(CameraOverlayContext, World3dCameraSnapshot)> {
        let context = self.snapshot.context?;
        let camera = self.snapshot.camera?;
        if self.snapshot.mode == CameraOverlayMode::Hidden || camera.dimension != context.dimension
        {
            return None;
        }
        Some((context, camera))
    }
}

impl Render for MapCameraOverlayView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let Some((context, camera)) = self.active_camera() else {
            return div().absolute().inset_0();
        };
        let mode = self.snapshot.mode;
        div().absolute().inset_0().child(
            canvas(
                move |bounds, _window, _cx| bounds,
                move |bounds, _bounds, window, _cx| {
                    paint_world3d_camera_overlay(bounds, context, camera, mode, window);
                },
            )
            .size_full(),
        )
    }
}

fn paint_world3d_camera_overlay(
    bounds: Bounds<Pixels>,
    context: CameraOverlayContext,
    camera: World3dCameraSnapshot,
    mode: CameraOverlayMode,
    window: &mut Window,
) {
    let colors = context.colors;
    let marker_x = overlay_marker_screen_x(
        bounds,
        context.viewport,
        context.layout,
        camera.position[0] as f32,
    );
    let marker_y = overlay_marker_screen_y(
        bounds,
        context.viewport,
        context.layout,
        camera.position[2] as f32,
    );
    if !marker_x.is_finite() || !marker_y.is_finite() {
        return;
    }
    let accent = colors.accent;
    let point_for = |block_x: f32, block_z: f32| {
        (
            overlay_marker_screen_x(bounds, context.viewport, context.layout, block_x),
            overlay_marker_screen_y(bounds, context.viewport, context.layout, block_z),
        )
    };
    if mode.shows_frustum() {
        let points = frustum_polygon_points(&camera, mode);
        if points.len() >= 3 {
            let mut builder = PathBuilder::fill();
            let mut first = true;
            for (block_x, block_z) in &points {
                let (x, y) = point_for(*block_x, *block_z);
                if !x.is_finite() || !y.is_finite() || x.abs() > 1.0e7 || y.abs() > 1.0e7 {
                    return;
                }
                if first {
                    builder.move_to(point(px(x), px(y)));
                    first = false;
                } else {
                    builder.line_to(point(px(x), px(y)));
                }
            }
            builder.close();
            if let Ok(path) = builder.build() {
                window.paint_path(path, Hsla { a: 0.10, ..accent });
            }
            let mut outline = PathBuilder::stroke(px(1.2));
            let mut first = true;
            for (block_x, block_z) in &points {
                let (x, y) = point_for(*block_x, *block_z);
                if first {
                    outline.move_to(point(px(x), px(y)));
                    first = false;
                } else {
                    outline.line_to(point(px(x), px(y)));
                }
            }
            outline.line_to(point(px(marker_x), px(marker_y)));
            if let Ok(path) = outline.build() {
                window.paint_path(path, Hsla { a: 0.55, ..accent });
            }
        }
    }
    let arrow_length = 20.0_f32;
    let forward = camera.forward_xz();
    let right_xz = [-forward[1], forward[0]];
    let arrow_point = |angle: f32, scale: f32| {
        let (sin_angle, cos_angle) = angle.sin_cos();
        (
            marker_x + (forward[0] * cos_angle + right_xz[0] * sin_angle) * arrow_length * scale,
            marker_y + (forward[1] * cos_angle + right_xz[1] * sin_angle) * arrow_length * scale,
        )
    };
    let (tip_x, tip_y) = arrow_point(0.0, 1.0);
    let (left_x, left_y) = arrow_point(2.5, 0.62);
    let (right_x, right_y) = arrow_point(-2.5, 0.62);
    let tip = point(px(tip_x), px(tip_y));
    let left = point(px(left_x), px(left_y));
    let right = point(px(right_x), px(right_y));
    let mut arrow = PathBuilder::fill();
    arrow.move_to(tip);
    arrow.line_to(left);
    arrow.line_to(right);
    arrow.close();
    if let Ok(path) = arrow.build() {
        window.paint_path(path, Hsla { a: 0.85, ..accent });
    }
    window.paint_quad(
        fill(
            Bounds {
                origin: point(px(marker_x - 5.0), px(marker_y - 5.0)),
                size: size(px(10.0), px(10.0)),
            },
            Hsla { a: 0.92, ..accent },
        )
        .corner_radii(px(3.0))
        .border_widths(px(1.2))
        .border_color(Hsla::white()),
    );
}

fn frustum_polygon_points(
    camera: &World3dCameraSnapshot,
    mode: CameraOverlayMode,
) -> Vec<(f32, f32)> {
    let center_x = camera.position[0] as f32;
    let center_z = camera.position[2] as f32;
    let forward = camera.forward_xz();
    let right_xz = [-forward[1], forward[0]];
    if camera.projection == World3dProjectionKind::Orthographic {
        let half_width = camera.ortho_height * camera.aspect.clamp(0.1, 10.0) * 0.5;
        let half_depth = camera.ortho_height * 0.5;
        let corner = |right_scale: f32, forward_scale: f32| {
            (
                center_x
                    + right_xz[0] * half_width * right_scale
                    + forward[0] * half_depth * forward_scale,
                center_z
                    + right_xz[1] * half_width * right_scale
                    + forward[1] * half_depth * forward_scale,
            )
        };
        return vec![
            corner(-1.0, 1.0),
            corner(1.0, 1.0),
            corner(1.0, -1.0),
            corner(-1.0, -1.0),
        ];
    }
    let half_fov = camera.horizontal_fov_radians() * 0.5;
    let distance = match mode {
        CameraOverlayMode::PositionFrustumDistance => camera.view_distance.clamp(8.0, 512.0),
        _ => camera.view_distance.clamp(8.0, 96.0),
    };
    let mut points = vec![(center_x, center_z)];
    for step in 0..=10 {
        let (sin_angle, cos_angle) = (-half_fov + half_fov * 2.0 * step as f32 / 10.0).sin_cos();
        points.push((
            center_x + (forward[0] * cos_angle + right_xz[0] * sin_angle) * distance,
            center_z + (forward[1] * cos_angle + right_xz[1] * sin_angle) * distance,
        ));
    }
    points
}
#[cfg(test)]
mod tests {
    use super::{
        CameraOverlayMode, World3dCameraSnapshot, World3dProjectionKind, frustum_polygon_points,
    };
    use bedrock_render::Dimension;

    fn camera() -> World3dCameraSnapshot {
        World3dCameraSnapshot {
            dimension: Dimension::Overworld,
            position: [100.0, 70.0, -40.0],
            yaw: 0.3,
            pitch: 0.4,
            vertical_fov_radians: 70.0_f32.to_radians(),
            aspect: 16.0 / 9.0,
            view_distance: 256.0,
            projection: World3dProjectionKind::Perspective,
            ortho_height: 32.0,
            generation: 3,
        }
    }

    #[test]
    fn perspective_frustum_is_finite_and_bounded() {
        let camera = camera();
        let points = frustum_polygon_points(&camera, CameraOverlayMode::PositionFrustum);
        assert!(points.len() > 3);
        for (x, z) in points {
            assert!(x.is_finite() && z.is_finite());
            assert!((x - 100.0).abs() < 400.0);
            assert!((z + 40.0).abs() < 400.0);
        }
    }

    #[test]
    fn distance_mode_reaches_farther_than_icon_mode() {
        let camera = camera();
        let near = frustum_polygon_points(&camera, CameraOverlayMode::PositionFrustum);
        let far = frustum_polygon_points(&camera, CameraOverlayMode::PositionFrustumDistance);
        let radius = |points: &[(f32, f32)]| {
            points
                .iter()
                .skip(1)
                .map(|(x, z)| ((x - 100.0).powi(2) + (z + 40.0).powi(2)).sqrt())
                .fold(0.0_f32, f32::max)
        };
        assert!(radius(&far) > radius(&near));
    }

    #[test]
    fn sector_points_toward_the_camera_forward_direction() {
        let mut camera = camera();
        camera.yaw = 0.0;
        let points = frustum_polygon_points(&camera, CameraOverlayMode::PositionFrustum);
        // The middle arc point of a yaw=0 camera must lie north (negative world Z).
        let (x, z) = points[1 + 5];
        assert!((x - camera.position[0] as f32).abs() < 1.0);
        assert!(z < camera.position[2] as f32 - 10.0);
        let mut east = camera;
        east.yaw = std::f32::consts::FRAC_PI_2;
        let points = frustum_polygon_points(&east, CameraOverlayMode::PositionFrustum);
        let (x, z) = points[1 + 5];
        // yaw = +90° looks west (-X) under the shared camera convention.
        assert!(x < camera.position[0] as f32 - 10.0);
        assert!((z - camera.position[2] as f32).abs() < 1.0);
    }

    #[test]
    fn orthographic_overlay_uses_a_rectangle_instead_of_a_sector() {
        let mut camera = camera();
        camera.projection = World3dProjectionKind::Orthographic;
        let points = frustum_polygon_points(&camera, CameraOverlayMode::PositionFrustum);
        assert_eq!(points.len(), 4);
        for (x, z) in points {
            assert!(x.is_finite() && z.is_finite());
        }
    }
}
