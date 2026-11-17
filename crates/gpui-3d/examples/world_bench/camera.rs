//! Free-flight camera shared by both rendering paths.
//!
//! The sidecar path installs [`FlightCamera`] behind an `Arc<Mutex<_>>` and hands a
//! `SceneView::with_camera_sampler` closure to the renderer thread; GPUI input handlers mutate the
//! same state, so moving the camera never rebuilds the GPUI element tree. The direct path drives
//! the same camera with a scripted per-frame orbit/advance so it traverses a comparable view
//! without a window.

use gpui_3d::{Camera, Vec3};
use std::sync::{Arc, Mutex};

/// A yaw/pitch free-flight camera positioned at `eye`.
#[derive(Clone, Debug)]
pub(crate) struct FlightCamera {
    pub eye: Vec3,
    /// Heading around the world Y axis, in radians. 0 looks toward +Z.
    pub yaw: f32,
    /// Pitch above the horizon, in radians.
    pub pitch: f32,
    fov: f32,
    near: f32,
    far: f32,
}

impl FlightCamera {
    /// Builds a free camera from a framed perspective camera's pose and projection.
    pub fn from_camera(camera: Camera) -> Self {
        let delta = camera.target - camera.eye;
        let yaw = delta.z.atan2(delta.x);
        let horizontal = (delta.x * delta.x + delta.z * delta.z).sqrt();
        let pitch = delta.y.atan2(horizontal);
        let (fov, near, far) = match camera.projection {
            gpui_3d::Projection::Perspective {
                fov_y_radians,
                near,
                far,
            } => (fov_y_radians, near, far),
            gpui_3d::Projection::Orthographic { height, near, far } => (height, near, far),
        };
        Self {
            eye: camera.eye,
            yaw,
            pitch,
            fov,
            near,
            far,
        }
    }

    /// Current normalized look direction.
    pub fn forward(&self) -> Vec3 {
        let cp = self.pitch.cos();
        Vec3::new(self.yaw.sin() * cp, self.pitch.sin(), self.yaw.cos() * cp)
            .normalized()
            .unwrap_or(Vec3::Z)
    }

    /// Current normalized right vector (perpendicular to forward and world up).
    pub fn right(&self) -> Vec3 {
        self.forward()
            .cross(Vec3::Y)
            .normalized()
            .unwrap_or(Vec3::X)
    }

    /// Produces the immutable camera consumed by both render paths.
    pub fn camera(&self) -> Camera {
        let target = self.eye + self.forward();
        Camera::perspective(
            self.eye,
            target,
            Vec3::Y,
            self.fov,
            self.near,
            self.far,
        )
    }

    /// Translates along the current view direction by `distance` world units.
    pub fn move_forward(&mut self, distance: f32) {
        self.eye = self.eye + self.forward() * distance;
    }

    /// Strafes right by `distance` world units.
    pub fn strafe(&mut self, distance: f32) {
        self.eye = self.eye + self.right() * distance;
    }

    /// Raises or lowers the camera by `distance` world units.
    pub fn move_up(&mut self, distance: f32) {
        self.eye = self.eye + Vec3::Y * distance;
    }

    /// Rotates by a mouse-drag delta in pixels.
    pub fn look(&mut self, dx: f32, dy: f32) {
        const SENSITIVITY: f32 = 0.004;
        self.yaw -= dx * SENSITIVITY;
        self.pitch = (self.pitch + dy * SENSITIVITY).clamp(-1.5, 1.5);
    }

    /// Scripted orbit used by the direct path: rotates the heading around the world Y axis.
    pub fn orbit(&mut self, radians: f32) {
        self.yaw += radians;
    }

    /// Shared state handle used by the sidecar camera sampler and input handlers.
    pub fn shared(self) -> SharedCamera {
        SharedCamera(Arc::new(Mutex::new(self)))
    }
}

/// Thread-safe camera state shared between GPUI input handlers and the sidecar sampler.
#[derive(Clone)]
pub(crate) struct SharedCamera(Arc<Mutex<FlightCamera>>);

impl SharedCamera {
    /// Samples the current camera on the renderer thread.
    pub fn sample(&self) -> Camera {
        match self.0.lock() {
            Ok(camera) => camera.camera(),
            Err(poisoned) => poisoned.into_inner().camera(),
        }
    }

    /// Applies a mutation from a GPUI input handler.
    pub fn update(&self, f: impl FnOnce(&mut FlightCamera)) {
        match self.0.lock() {
            Ok(mut camera) => f(&mut camera),
            Err(poisoned) => f(&mut poisoned.into_inner()),
        }
    }
}
