//! World-space camera math for the independent 3D editor.

use super::types::{
    WORLD3D_DEFAULT_FOV_Y_RADIANS, WORLD3D_DEFAULT_SPEED, WORLD3D_DEFAULT_VIEW_DISTANCE,
    WORLD3D_NEAR_PLANE, WORLD3D_SURFACE_EYE_OFFSET, World3dCameraSnapshot, World3dProjectionKind,
    World3dSelection,
};
use bedrock_render::Dimension;
use gpui_3d::{Camera, Vec3};
use std::{
    collections::HashSet,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Instant,
};

const MAX_ABS_PITCH: f32 = 1.553_343; // 89 degrees
const MIN_SPEED: f32 = 2.0;
const MAX_SPEED: f32 = 120.0;
const MIN_FOV: f32 = 25.0_f32.to_radians();
const MAX_FOV: f32 = 110.0_f32.to_radians();
const MIN_ORTHO_HEIGHT: f32 = 2.0;
const MAX_ORTHO_HEIGHT: f32 = 512.0;

/// Mutable world camera owned by one 3D editor window only.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct World3dCamera {
    /// Absolute world-space eye position in blocks.
    pub position: [f64; 3],
    /// Horizontal rotation; zero looks north (-Z), positive turns toward east (+X).
    pub yaw: f32,
    /// Vertical rotation; positive looks down.
    pub pitch: f32,
    pub fov_y_radians: f32,
    pub view_distance: f32,
    pub projection: World3dProjectionKind,
    pub ortho_height: f32,
    pub speed: f32,
}

/// Navigation state sampled by the 3D renderer independently of GPUI view updates.
#[derive(Clone)]
pub(super) struct World3dNavigation(Arc<Mutex<NavigationState>>);

struct NavigationState {
    camera: World3dCamera,
    render_origin: [i32; 3],
    keys_down: HashSet<String>,
    last_motion_at: Option<Instant>,
}

impl World3dNavigation {
    pub(super) fn new(camera: World3dCamera) -> Self {
        Self(Arc::new(Mutex::new(NavigationState {
            camera,
            render_origin: [0; 3],
            keys_down: HashSet::new(),
            last_motion_at: None,
        })))
    }

    fn state(&self) -> MutexGuard<'_, NavigationState> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn camera(&self) -> World3dCamera {
        self.state().camera
    }

    pub(super) fn set_camera(&self, camera: World3dCamera) {
        let mut state = self.state();
        state.camera = camera;
        state.last_motion_at = Some(Instant::now());
    }

    pub(super) fn update_camera(&self, update: impl FnOnce(&mut World3dCamera)) -> World3dCamera {
        let mut state = self.state();
        update(&mut state.camera);
        state.camera
    }

    pub(super) fn set_origin(&self, origin: [i32; 3]) {
        self.state().render_origin = origin;
    }

    pub(super) fn set_key(&self, key: &str, pressed: bool) -> bool {
        let mut state = self.state();
        let changed = if pressed {
            state.keys_down.insert(key.to_owned())
        } else {
            state.keys_down.remove(key)
        };
        if changed {
            state.last_motion_at = (!state.keys_down.is_empty()).then(Instant::now);
        }
        changed
    }

    pub(super) fn clear_keys(&self) {
        let mut state = self.state();
        state.keys_down.clear();
        state.last_motion_at = None;
    }

    pub(super) fn sample_camera(&self, now: Instant) -> Camera {
        let mut state = self.state();
        state.tick_motion(now);
        state.camera.gpu_camera(state.render_origin)
    }
}

impl NavigationState {
    fn tick_motion(&mut self, now: Instant) {
        if self.keys_down.is_empty() {
            self.last_motion_at = None;
            return;
        }
        let delta_seconds = self
            .last_motion_at
            .map_or(0.0, |previous| {
                now.saturating_duration_since(previous).as_secs_f32()
            })
            .clamp(0.0, 0.08);
        let forward =
            f64::from(self.keys_down.contains("w")) - f64::from(self.keys_down.contains("s"));
        let strafe =
            f64::from(self.keys_down.contains("d")) - f64::from(self.keys_down.contains("a"));
        let vertical = f64::from(self.keys_down.contains("space"))
            - f64::from(self.keys_down.contains("shift"));
        self.camera
            .move_by(forward, strafe, vertical, delta_seconds);
        self.last_motion_at = Some(now);
    }
}

impl Default for World3dCamera {
    fn default() -> Self {
        Self {
            position: [0.5, 96.0, 0.5],
            yaw: 0.0,
            pitch: 0.35,
            fov_y_radians: WORLD3D_DEFAULT_FOV_Y_RADIANS,
            view_distance: WORLD3D_DEFAULT_VIEW_DISTANCE,
            projection: World3dProjectionKind::Perspective,
            ortho_height: 48.0,
            speed: WORLD3D_DEFAULT_SPEED,
        }
    }
}

impl World3dCamera {
    #[must_use]
    pub fn forward(&self) -> [f64; 3] {
        let cos_pitch = self.pitch.cos() as f64;
        [
            -(self.yaw.sin() as f64) * cos_pitch,
            -(self.pitch.sin() as f64),
            -(self.yaw.cos() as f64) * cos_pitch,
        ]
    }

    #[must_use]
    pub fn forward_f32(&self) -> [f32; 3] {
        self.forward().map(|value| value as f32)
    }

    /// Horizontal right vector on the XZ plane.
    #[must_use]
    pub fn right(&self) -> [f64; 3] {
        [self.yaw.cos() as f64, 0.0, -self.yaw.sin() as f64]
    }

    #[must_use]
    pub fn up(&self) -> [f64; 3] {
        let f = self.forward();
        let r = self.right();
        [
            r[1] * f[2] - r[2] * f[1],
            r[2] * f[0] - r[0] * f[2],
            r[0] * f[1] - r[1] * f[0],
        ]
    }

    pub fn rotate_look(&mut self, delta_yaw: f32, delta_pitch: f32) {
        self.yaw = wrap_angle(self.yaw + delta_yaw);
        self.pitch = (self.pitch + delta_pitch).clamp(-MAX_ABS_PITCH, MAX_ABS_PITCH);
    }

    pub fn add_speed(&mut self, factor: f32) {
        self.speed = (self.speed * factor).clamp(MIN_SPEED, MAX_SPEED);
    }

    pub fn add_fov(&mut self, delta_radians: f32) {
        self.fov_y_radians = (self.fov_y_radians + delta_radians).clamp(MIN_FOV, MAX_FOV);
    }

    pub fn add_ortho_height(&mut self, factor: f32) {
        self.ortho_height = (self.ortho_height * factor).clamp(MIN_ORTHO_HEIGHT, MAX_ORTHO_HEIGHT);
    }

    pub fn toggle_projection(&mut self) {
        self.projection = match self.projection {
            World3dProjectionKind::Perspective => World3dProjectionKind::Orthographic,
            World3dProjectionKind::Orthographic => World3dProjectionKind::Perspective,
        };
    }

    /// Applies WASD/Space/Shift-style movement.
    pub fn move_by(&mut self, forward: f64, strafe: f64, vertical: f64, delta_seconds: f32) {
        if delta_seconds <= 0.0 {
            return;
        }
        let distance = f64::from(self.speed) * f64::from(delta_seconds.clamp(0.0, 0.2));
        let f = self.forward();
        let r = self.right();
        self.position = [
            self.position[0] + f[0] * forward * distance + r[0] * strafe * distance,
            self.position[1] + f[1] * forward * distance + vertical * distance,
            self.position[2] + f[2] * forward * distance + r[2] * strafe * distance,
        ];
        self.position[1] = self.position[1].clamp(-512.0, 1024.0);
    }

    #[must_use]
    pub fn snapshot(
        &self,
        dimension: Dimension,
        aspect: f32,
        generation: u64,
    ) -> World3dCameraSnapshot {
        World3dCameraSnapshot {
            dimension,
            position: self.position,
            yaw: self.yaw,
            pitch: self.pitch,
            vertical_fov_radians: self.fov_y_radians,
            aspect: aspect.clamp(0.05, 20.0),
            view_distance: self.view_distance,
            projection: self.projection,
            ortho_height: self.ortho_height,
            generation,
        }
    }

    /// Builds the GPU camera for a camera-relative render origin.
    #[must_use]
    pub fn gpu_camera(&self, render_origin: [i32; 3]) -> Camera {
        let origin = [
            f64::from(render_origin[0]),
            0.0,
            f64::from(render_origin[2]),
        ];
        let eye = Vec3::new(
            (self.position[0] - origin[0]) as f32,
            self.position[1] as f32,
            (self.position[2] - origin[2]) as f32,
        );
        let f = self.forward_f32();
        let target = eye + Vec3::new(f[0], f[1], f[2]);
        let far = self.view_distance.max(64.0) * 2.0;
        match self.projection {
            World3dProjectionKind::Perspective => Camera::perspective(
                eye,
                target,
                Vec3::Y,
                self.fov_y_radians,
                WORLD3D_NEAR_PLANE,
                far,
            ),
            World3dProjectionKind::Orthographic => Camera::orthographic(
                eye,
                target,
                Vec3::Y,
                self.ortho_height,
                WORLD3D_NEAR_PLANE,
                far,
            ),
        }
    }

    /// Casts a normalized-device-coordinate ray in world space with f64 precision.
    #[must_use]
    pub fn ray(&self, ndc_x: f32, ndc_y: f32, aspect: f32) -> ([f64; 3], [f64; 3]) {
        let f = self.forward();
        let r = self.right();
        let u = self.up();
        let aspect = f64::from(aspect.clamp(0.05, 20.0));
        match self.projection {
            World3dProjectionKind::Perspective => {
                let half_h = f64::from(self.fov_y_radians * 0.5).tan();
                let half_w = half_h * aspect;
                let dx = f64::from(ndc_x) * half_w;
                let dy = f64::from(ndc_y) * half_h;
                let dir = normalize3([
                    f[0] + r[0] * dx + u[0] * dy,
                    f[1] + r[1] * dx + u[1] * dy,
                    f[2] + r[2] * dx + u[2] * dy,
                ]);
                (self.position, dir)
            }
            World3dProjectionKind::Orthographic => {
                let half_h = f64::from(self.ortho_height) * 0.5;
                let half_w = half_h * aspect;
                let dx = f64::from(ndc_x) * half_w;
                let dy = f64::from(ndc_y) * half_h;
                let origin = [
                    self.position[0] + r[0] * dx + u[0] * dy,
                    self.position[1] + r[1] * dx + u[1] * dy,
                    self.position[2] + r[2] * dx + u[2] * dy,
                ];
                (origin, f)
            }
        }
    }

    /// Places the camera at a block column with an explicit world Y.
    pub fn teleport_to(&mut self, x: i32, y: f64, z: i32) {
        self.position = [f64::from(x) + 0.5, y, f64::from(z) + 0.5];
    }

    /// Places the camera above a safe surface block.
    pub fn teleport_to_surface(&mut self, x: i32, surface_y: i32, z: i32) {
        self.teleport_to(x, f64::from(surface_y) + WORLD3D_SURFACE_EYE_OFFSET, z);
    }

    /// Frames a selection from outside its center.
    pub fn frame_selection(&mut self, selection: &World3dSelection) {
        let center = [
            (f64::from(selection.min[0]) + f64::from(selection.max[0] + 1)) * 0.5,
            (f64::from(selection.min[1]) + f64::from(selection.max[1] + 1)) * 0.5,
            (f64::from(selection.min[2]) + f64::from(selection.max[2] + 1)) * 0.5,
        ];
        let size = selection.size();
        let span = f64::from(size[0].max(size[2]).max(size[1])).max(8.0);
        self.position = [
            center[0] + span * 0.9,
            center[1] + span * 0.8,
            center[2] + span * 0.9,
        ];
        self.yaw = -2.356_194_5;
        self.pitch = 0.7;
    }
}

fn normalize3(value: [f64; 3]) -> [f64; 3] {
    let len = (value[0] * value[0] + value[1] * value[1] + value[2] * value[2])
        .sqrt()
        .max(1.0e-9);
    [value[0] / len, value[1] / len, value[2] / len]
}

fn wrap_angle(value: f32) -> f32 {
    let two_pi = std::f32::consts::TAU;
    let mut wrapped = value % two_pi;
    if wrapped > std::f32::consts::PI {
        wrapped -= two_pi;
    } else if wrapped < -std::f32::consts::PI {
        wrapped += two_pi;
    }
    wrapped
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::window::map_viewer::world3d::types::{World3dProjectionKind, World3dSelection};
    use bedrock_render::Dimension;

    fn assert_close(left: f32, right: f32) {
        assert!((left - right).abs() < 1.0e-4, "{left} != {right}");
    }

    #[test]
    fn forward_uses_minecraft_yaw_pitch_convention() {
        let mut camera = World3dCamera::default();
        camera.yaw = 0.0;
        camera.pitch = 0.0;
        let forward = camera.forward();
        assert!(forward[0].abs() < 1.0e-6);
        assert!(forward[1].abs() < 1.0e-6);
        assert_close(forward[2] as f32, -1.0);
        camera.yaw = std::f32::consts::FRAC_PI_2;
        let forward = camera.forward();
        assert_close(forward[0] as f32, -1.0);
        assert_close(forward[2] as f32, 0.0);
    }

    #[test]
    fn horizontal_fov_is_wider_than_vertical_for_landscape_viewports() {
        let camera = World3dCamera::default();
        let snapshot = camera.snapshot(Dimension::Overworld, 16.0 / 9.0, 1);
        assert!(snapshot.horizontal_fov_radians() > snapshot.vertical_fov_radians);
        let square = camera.snapshot(Dimension::Overworld, 1.0, 1);
        assert!((square.horizontal_fov_radians() - square.vertical_fov_radians).abs() < 1.0e-5);
    }

    #[test]
    fn gpu_camera_is_relative_to_the_render_origin() {
        let mut camera = World3dCamera::default();
        camera.position = [10_000.5, 120.0, -40_000.5];
        camera.yaw = 0.0;
        camera.pitch = 0.0;
        let gpu = camera.gpu_camera([9_984, 0, -40_000]);
        assert!((gpu.eye.x - 16.5).abs() < 1.0e-3);
        assert!((gpu.eye.z + 0.5).abs() < 1.0e-3);
    }

    #[test]
    fn navigation_moves_on_renderer_samples_without_ui_render() {
        let navigation = World3dNavigation::new(World3dCamera::default());
        navigation.set_key("w", true);
        let frame = Instant::now() + std::time::Duration::from_millis(16);
        let initial_z = navigation.camera().position[2];
        let sampled = navigation.sample_camera(frame);
        assert!(sampled.eye.z < initial_z as f32);
        navigation.clear_keys();
        let stopped_z = navigation.camera().position[2];
        navigation.sample_camera(frame + std::time::Duration::from_millis(16));
        assert_eq!(navigation.camera().position[2], stopped_z);
        navigation.update_camera(|camera| camera.position[0] = 512.5);
        navigation.set_origin([512, 0, 0]);
        assert_close(
            navigation
                .sample_camera(frame + std::time::Duration::from_millis(32))
                .eye
                .x,
            0.5,
        );
    }

    #[test]
    fn rays_are_finite_and_normalized_at_extreme_pitch() {
        let mut camera = World3dCamera::default();
        camera.pitch = 1.5;
        let (origin, direction) = camera.ray(0.0, 0.0, 16.0 / 9.0);
        assert!(origin.iter().all(|value| value.is_finite()));
        assert!(direction.iter().all(|value| value.is_finite()));
        let length = (direction[0] * direction[0]
            + direction[1] * direction[1]
            + direction[2] * direction[2])
            .sqrt();
        assert!((length - 1.0).abs() < 1.0e-5);
    }

    #[test]
    fn orthographic_projection_uses_parallel_rays() {
        let mut camera = World3dCamera::default();
        camera.projection = World3dProjectionKind::Orthographic;
        let (left_origin, left_direction) = camera.ray(-1.0, 0.0, 2.0);
        let (right_origin, right_direction) = camera.ray(1.0, 0.0, 2.0);
        assert!((left_origin[0] - right_origin[0]).abs() > 0.1);
        assert!((left_direction[0] - right_direction[0]).abs() < 1.0e-6);
        assert!((left_direction[1] - right_direction[1]).abs() < 1.0e-6);
        assert!((left_direction[2] - right_direction[2]).abs() < 1.0e-6);
    }

    #[test]
    fn framing_a_selection_stays_outside_its_volume() {
        let mut camera = World3dCamera::default();
        camera.frame_selection(&World3dSelection {
            dimension: Dimension::Overworld,
            min: [0, 64, 0],
            max: [15, 79, 15],
        });
        assert!(camera.position[0] > 0.0);
        assert!(camera.position[1] > 79.0);
        assert!(camera.position[2] > 0.0);
    }
}
