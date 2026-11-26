use crate::{Mat4, Ray, Vec2, Vec3};
use std::fmt;

mod framing;

/// Camera projection type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Projection {
    /// Perspective projection with vertical field of view in radians.
    Perspective {
        /// Vertical field of view in radians.
        fov_y_radians: f32,
        /// Near clipping distance, greater than zero.
        near: f32,
        /// Far clipping distance, greater than `near`.
        far: f32,
    },
    /// Orthographic projection with vertical world-space size.
    Orthographic {
        /// Vertical world-space view size, greater than zero.
        height: f32,
        /// Near clipping distance, greater than zero.
        near: f32,
        /// Far clipping distance, greater than `near`.
        far: f32,
    },
}

/// Invalid camera pose or projection parameters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CameraError {
    /// Eye and target coincide, or the up vector is parallel to the view direction.
    InvalidPose,
    /// Aspect ratio, near/far range, or projection scale is invalid.
    InvalidProjection,
    /// Normalized device coordinates are not finite or cannot be unprojected.
    InvalidNdcPoint,
    /// Orbit camera target, angles, distance, pan offset, or zoom factor are invalid.
    InvalidOrbit,
    /// Framing bounds are non-finite or have reversed corners.
    InvalidBounds,
    /// A framing padding factor is non-finite or less than one.
    InvalidFraming,
    /// The fixed near/far planes cannot contain the requested bounds.
    BoundsOutsideClipRange,
}

impl fmt::Display for CameraError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidPose => "camera pose has no finite view basis",
            Self::InvalidProjection => "camera projection parameters are invalid",
            Self::InvalidNdcPoint => "NDC point cannot be unprojected by the camera",
            Self::InvalidOrbit => "orbit camera parameters are invalid",
            Self::InvalidBounds => "camera framing bounds are invalid",
            Self::InvalidFraming => "camera framing padding must be finite and at least one",
            Self::BoundsOutsideClipRange => "camera clip range cannot contain the framing bounds",
        })
    }
}

impl std::error::Error for CameraError {}

/// View pose and projection for a scene render or query.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// Camera position in world space.
    pub eye: Vec3,
    /// World-space look target.
    pub target: Vec3,
    /// World-space up direction.
    pub up: Vec3,
    /// Projection configuration.
    pub projection: Projection,
}

impl Camera {
    /// Creates a perspective camera.
    pub fn perspective(
        eye: Vec3,
        target: Vec3,
        up: Vec3,
        fov_y_radians: f32,
        near: f32,
        far: f32,
    ) -> Self {
        Self {
            eye,
            target,
            up,
            projection: Projection::Perspective {
                fov_y_radians,
                near,
                far,
            },
        }
    }

    /// Creates an orthographic camera.
    pub fn orthographic(
        eye: Vec3,
        target: Vec3,
        up: Vec3,
        height: f32,
        near: f32,
        far: f32,
    ) -> Self {
        Self {
            eye,
            target,
            up,
            projection: Projection::Orthographic { height, near, far },
        }
    }

    /// Builds the right-handed world-to-view matrix.
    ///
    /// # Errors
    ///
    /// Returns [`CameraError::InvalidPose`] when the eye and target coincide, or the up vector
    /// is parallel to the view direction.
    pub fn view(self) -> Result<Mat4, CameraError> {
        Mat4::look_at(self.eye, self.target, self.up).ok_or(CameraError::InvalidPose)
    }

    /// Projection matrix with a zero-to-one depth range.
    ///
    /// # Errors
    ///
    /// Returns [`CameraError::InvalidProjection`] for an invalid aspect ratio, clipping range,
    /// field of view, or orthographic height.
    pub fn projection_matrix(self, aspect: f32) -> Result<Mat4, CameraError> {
        let matrix = match self.projection {
            Projection::Perspective {
                fov_y_radians,
                near,
                far,
            } => Mat4::perspective(fov_y_radians, aspect, near, far),
            Projection::Orthographic { height, near, far } => {
                if !height.is_finite()
                    || height <= 0.0
                    || !aspect.is_finite()
                    || aspect <= 0.0
                    || !near.is_finite()
                    || !far.is_finite()
                    || near <= 0.0
                    || far <= near
                {
                    None
                } else {
                    let width = height * aspect;
                    let depth = far - near;
                    Some(Mat4::from_columns([
                        [2.0 / width, 0.0, 0.0, 0.0],
                        [0.0, 2.0 / height, 0.0, 0.0],
                        [0.0, 0.0, -1.0 / depth, 0.0],
                        [0.0, 0.0, -near / depth, 1.0],
                    ]))
                }
            }
        };
        matrix
            .filter(|matrix| matrix.is_finite())
            .ok_or(CameraError::InvalidProjection)
    }

    /// Builds the camera's view-projection matrix.
    ///
    /// # Errors
    ///
    /// Returns the pose or projection error reported by [`Camera::view`] or
    /// [`Camera::projection_matrix`].
    pub fn view_projection(self, aspect: f32) -> Result<Mat4, CameraError> {
        Ok(self.projection_matrix(aspect)? * self.view()?)
    }

    /// Creates a world-space ray through a point in normalized device coordinates.
    ///
    /// `ndc` uses `(-1, -1)` at the lower-left and `(1, 1)` at the upper-right. The projection
    /// uses a zero-to-one depth range, matching Nova's Vulkan, Direct3D 12, and Metal backends.
    ///
    /// # Errors
    ///
    /// Returns [`CameraError::InvalidNdcPoint`] for non-finite coordinates or a point that
    /// cannot be unprojected, and otherwise propagates pose/projection errors.
    pub fn ray(self, ndc: Vec2, aspect: f32) -> Result<Ray, CameraError> {
        if !ndc.x.is_finite() || !ndc.y.is_finite() {
            return Err(CameraError::InvalidNdcPoint);
        }
        let inverse = self
            .view_projection(aspect)?
            .inverse()
            .ok_or(CameraError::InvalidNdcPoint)?;
        let unproject = |depth| {
            let point = inverse.transform4([ndc.x, ndc.y, depth, 1.0]);
            (point[3].is_finite() && point[3].abs() > f32::EPSILON).then(|| {
                Vec3::new(
                    point[0] / point[3],
                    point[1] / point[3],
                    point[2] / point[3],
                )
            })
        };
        let near = unproject(0.0).filter(|point| point.is_finite());
        let far = unproject(1.0).filter(|point| point.is_finite());
        let (Some(near), Some(far)) = (near, far) else {
            return Err(CameraError::InvalidNdcPoint);
        };
        let origin = if matches!(self.projection, Projection::Perspective { .. }) {
            self.eye
        } else {
            near
        };
        Ray::new(origin, far - origin).ok_or(CameraError::InvalidNdcPoint)
    }
}

/// Y-up orbit controller that produces a [`Camera`] snapshot.
///
/// Yaw and pitch are measured in radians. Positive yaw moves the eye toward positive X; positive
/// pitch moves it above the target. Orbit pitch is clamped just short of the poles so the camera
/// basis stays defined. Pan offsets are world-space distances along camera right and up. Zoom
/// factors greater than one move a perspective camera farther from its target or enlarge an
/// orthographic view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbitCamera {
    target: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
    projection: Projection,
}

impl OrbitCamera {
    const POLE_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 1.0e-4;

    /// Creates an orbit controller around `target`.
    ///
    /// `distance` must be positive and finite. Projection parameters follow the same validation
    /// rules as [`Camera::projection_matrix`]. The initial pitch is clamped to keep the camera
    /// away from the up-vector poles.
    ///
    /// # Errors
    ///
    /// Returns [`CameraError::InvalidOrbit`] for a non-finite target, angle, or invalid distance,
    /// and propagates pose/projection errors when the camera snapshot is invalid.
    pub fn new(
        target: Vec3,
        yaw: f32,
        pitch: f32,
        distance: f32,
        projection: Projection,
    ) -> Result<Self, CameraError> {
        if !target.is_finite()
            || !yaw.is_finite()
            || !pitch.is_finite()
            || !distance.is_finite()
            || distance <= 0.0
        {
            return Err(CameraError::InvalidOrbit);
        }
        let orbit = Self {
            target,
            yaw: yaw.rem_euclid(std::f32::consts::TAU),
            pitch: pitch.clamp(-Self::POLE_LIMIT, Self::POLE_LIMIT),
            distance,
            projection,
        };
        let camera = orbit.camera();
        if !camera.view()?.is_finite() {
            return Err(CameraError::InvalidPose);
        }
        if !camera.projection_matrix(1.0)?.is_finite() {
            return Err(CameraError::InvalidProjection);
        }
        Ok(orbit)
    }

    /// Returns the current camera snapshot.
    #[must_use]
    pub fn camera(self) -> Camera {
        let horizontal = self.pitch.cos() * self.distance;
        let offset = Vec3::new(
            self.yaw.sin() * horizontal,
            self.pitch.sin() * self.distance,
            self.yaw.cos() * horizontal,
        );
        Camera {
            eye: self.target + offset,
            target: self.target,
            up: Vec3::Y,
            projection: self.projection,
        }
    }

    /// Applies a yaw and pitch delta in radians.
    ///
    /// # Errors
    ///
    /// Returns [`CameraError::InvalidOrbit`] if a delta is non-finite or overflows the yaw.
    pub fn orbit(self, delta: Vec2) -> Result<Self, CameraError> {
        let yaw = self.yaw + delta.x;
        let pitch = self.pitch + delta.y;
        if !delta.x.is_finite() || !delta.y.is_finite() || !yaw.is_finite() || !pitch.is_finite() {
            return Err(CameraError::InvalidOrbit);
        }
        Ok(Self {
            yaw: yaw.rem_euclid(std::f32::consts::TAU),
            pitch: pitch.clamp(-Self::POLE_LIMIT, Self::POLE_LIMIT),
            ..self
        })
    }

    /// Moves the orbit target along the camera's right and up axes, in world-space units.
    ///
    /// # Errors
    ///
    /// Returns [`CameraError::InvalidOrbit`] if the offset or resulting target is non-finite, or
    /// propagates [`CameraError::InvalidPose`] if the new view basis cannot be built.
    pub fn pan(self, offset: Vec2) -> Result<Self, CameraError> {
        if !offset.x.is_finite() || !offset.y.is_finite() {
            return Err(CameraError::InvalidOrbit);
        }
        let camera = self.camera();
        let forward = (camera.target - camera.eye)
            .normalized()
            .ok_or(CameraError::InvalidPose)?;
        let right = forward
            .cross(camera.up)
            .normalized()
            .ok_or(CameraError::InvalidPose)?;
        let camera_up = right
            .cross(forward)
            .normalized()
            .ok_or(CameraError::InvalidPose)?;
        let target = self.target + right * offset.x + camera_up * offset.y;
        if !target.is_finite() {
            return Err(CameraError::InvalidOrbit);
        }
        let panned = Self { target, ..self };
        if !panned.camera().view()?.is_finite() {
            return Err(CameraError::InvalidPose);
        }
        Ok(panned)
    }

    /// Changes the camera distance or orthographic view height by `factor`.
    ///
    /// A factor greater than one zooms out. A perspective camera changes its distance; an
    /// orthographic camera changes its vertical world-space view size.
    ///
    /// # Errors
    ///
    /// Returns [`CameraError::InvalidOrbit`] for a non-positive/non-finite factor or an invalid
    /// resulting distance/height, and propagates [`CameraError::InvalidProjection`] if the new
    /// projection is not representable.
    pub fn zoom(self, factor: f32) -> Result<Self, CameraError> {
        if !factor.is_finite() || factor <= 0.0 {
            return Err(CameraError::InvalidOrbit);
        }
        let mut orbit = self;
        match orbit.projection {
            Projection::Perspective { .. } => {
                orbit.distance *= factor;
                if !orbit.distance.is_finite() || orbit.distance <= 0.0 {
                    return Err(CameraError::InvalidOrbit);
                }
            }
            Projection::Orthographic { height, near, far } => {
                let height = height * factor;
                if !height.is_finite() || height <= 0.0 {
                    return Err(CameraError::InvalidOrbit);
                }
                orbit.projection = Projection::Orthographic { height, near, far };
            }
        }
        if !orbit.camera().projection_matrix(1.0)?.is_finite() {
            return Err(CameraError::InvalidProjection);
        }
        Ok(orbit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Aabb;

    #[test]
    fn both_projection_modes_reject_invalid_ranges() {
        let perspective = Camera::perspective(Vec3::ZERO, -Vec3::Z, Vec3::Y, 1.0, 0.1, 100.0);
        assert!(perspective.projection_matrix(1.0).is_ok());
        let orthographic = Camera::orthographic(Vec3::ZERO, -Vec3::Z, Vec3::Y, 10.0, 0.1, 100.0);
        assert!(orthographic.projection_matrix(2.0).is_ok());
        assert_eq!(
            orthographic.projection_matrix(0.0),
            Err(CameraError::InvalidProjection)
        );
    }

    #[test]
    fn projection_rejects_finite_inputs_that_overflow_matrix_values() {
        let tiny_fov = Camera::perspective(
            Vec3::ZERO,
            -Vec3::Z,
            Vec3::Y,
            f32::MIN_POSITIVE / 100.0,
            0.1,
            100.0,
        );
        assert_eq!(
            tiny_fov.projection_matrix(1.0),
            Err(CameraError::InvalidProjection)
        );

        let overflowing_depth =
            Camera::perspective(Vec3::ZERO, -Vec3::Z, Vec3::Y, 1.0, 1.0e30, 1.0e35);
        assert_eq!(
            overflowing_depth.projection_matrix(1.0),
            Err(CameraError::InvalidProjection)
        );
    }

    #[test]
    fn camera_ray_uses_projection_mode_origin() {
        let perspective = Camera::perspective(Vec3::ZERO, -Vec3::Z, Vec3::Y, 1.0, 0.1, 100.0);
        let perspective_ray = perspective.ray(Vec2::ZERO, 1.0).unwrap();
        assert_eq!(perspective_ray.origin, Vec3::ZERO);
        assert!(perspective_ray.direction.z < 0.0);

        let orthographic = Camera::orthographic(Vec3::ZERO, -Vec3::Z, Vec3::Y, 4.0, 0.1, 100.0);
        let orthographic_ray = orthographic.ray(Vec2::ZERO, 1.0).unwrap();
        assert!((orthographic_ray.origin.z + 0.1).abs() < 1e-4);
        assert!((orthographic_ray.direction - -Vec3::Z).length() < 1e-4);
    }

    #[test]
    fn orbit_pan_and_zoom_update_the_camera_pose() {
        let orbit = OrbitCamera::new(
            Vec3::ZERO,
            0.0,
            0.0,
            4.0,
            Projection::Perspective {
                fov_y_radians: 1.0,
                near: 0.1,
                far: 100.0,
            },
        )
        .unwrap();
        let updated = orbit
            .orbit(Vec2::new(std::f32::consts::FRAC_PI_2, 0.0))
            .unwrap()
            .pan(Vec2::new(2.0, 1.0))
            .unwrap()
            .zoom(2.0)
            .unwrap();
        let camera = updated.camera();

        assert!((camera.eye.x - 8.0).abs() < 1.0e-4);
        assert!((camera.eye.y - 1.0).abs() < 1.0e-4);
        assert!((camera.eye.z + 2.0).abs() < 1.0e-4);
        assert!((camera.target.x - 0.0).abs() < 1.0e-4);
        assert!((camera.target.y - 1.0).abs() < 1.0e-4);
        assert!((camera.target.z + 2.0).abs() < 1.0e-4);
    }

    #[test]
    fn orthographic_zoom_changes_view_height_without_moving_eye() {
        let orbit = OrbitCamera::new(
            Vec3::ZERO,
            0.0,
            0.0,
            4.0,
            Projection::Orthographic {
                height: 8.0,
                near: 0.1,
                far: 100.0,
            },
        )
        .unwrap();
        let zoomed = orbit.zoom(0.5).unwrap();
        assert_eq!(zoomed.camera().eye, orbit.camera().eye);
        assert!(matches!(
            zoomed.camera().projection,
            Projection::Orthographic { height: 4.0, .. }
        ));
    }

    #[test]
    fn perspective_framing_contains_every_bounds_corner_and_preserves_direction() {
        let bounds = Aabb::new(Vec3::new(-2.0, -1.0, -0.5), Vec3::new(2.0, 1.0, 0.5)).unwrap();
        let orbit = OrbitCamera::new(
            Vec3::ZERO,
            0.4,
            0.2,
            2.0,
            Projection::Perspective {
                fov_y_radians: 1.0,
                near: 0.1,
                far: 100.0,
            },
        )
        .unwrap();
        let aspect = 16.0 / 9.0;
        let padding = 1.25;
        let fitted = orbit.fit_bounds(bounds, aspect, padding).unwrap();
        let camera = fitted.camera();
        let expected_center = (bounds.min + bounds.max) * 0.5;
        let expected_direction = (orbit.camera().target - orbit.camera().eye)
            .normalized()
            .unwrap();
        let actual_direction = (camera.target - camera.eye).normalized().unwrap();
        assert!((camera.target - expected_center).length() < 1.0e-5);
        assert!((actual_direction - expected_direction).length() < 1.0e-5);

        let view_projection = camera.view_projection(aspect).unwrap();
        for x in [bounds.min.x, bounds.max.x] {
            for y in [bounds.min.y, bounds.max.y] {
                for z in [bounds.min.z, bounds.max.z] {
                    let clip = view_projection.transform4([x, y, z, 1.0]);
                    let ndc = [clip[0] / clip[3], clip[1] / clip[3], clip[2] / clip[3]];
                    assert!(ndc[0].abs() <= 1.0 / padding + 1.0e-5);
                    assert!(ndc[1].abs() <= 1.0 / padding + 1.0e-5);
                    assert!((0.0..=1.0).contains(&ndc[2]));
                }
            }
        }
    }

    #[test]
    fn orthographic_framing_contains_every_bounds_corner() {
        let bounds = Aabb::new(Vec3::new(-2.0, -1.0, -0.5), Vec3::new(2.0, 1.0, 0.5)).unwrap();
        let orbit = OrbitCamera::new(
            Vec3::ZERO,
            0.3,
            0.1,
            1.0,
            Projection::Orthographic {
                height: 1.0,
                near: 0.1,
                far: 100.0,
            },
        )
        .unwrap();
        let aspect = 16.0 / 9.0;
        let padding = 1.2;
        let fitted = orbit.fit_bounds(bounds, aspect, padding).unwrap();
        let camera = fitted.camera();
        let view_projection = camera.view_projection(aspect).unwrap();
        for x in [bounds.min.x, bounds.max.x] {
            for y in [bounds.min.y, bounds.max.y] {
                for z in [bounds.min.z, bounds.max.z] {
                    let clip = view_projection.transform4([x, y, z, 1.0]);
                    assert!(clip[0].abs() <= 1.0 / padding + 1.0e-5);
                    assert!(clip[1].abs() <= 1.0 / padding + 1.0e-5);
                    assert!((0.0..=1.0).contains(&clip[2]));
                }
            }
        }
    }

    #[test]
    fn framing_rejects_invalid_bounds_padding_and_clip_ranges() {
        let perspective = OrbitCamera::new(
            Vec3::ZERO,
            0.0,
            0.0,
            1.0,
            Projection::Perspective {
                fov_y_radians: 1.0,
                near: 0.1,
                far: 1.0,
            },
        )
        .unwrap();
        let bounds = Aabb::new(Vec3::new(-2.0, -2.0, -2.0), Vec3::new(2.0, 2.0, 2.0)).unwrap();

        assert_eq!(
            perspective.fit_bounds(
                Aabb {
                    min: Vec3::new(1.0, 1.0, 1.0),
                    max: Vec3::ZERO,
                },
                1.0,
                1.0,
            ),
            Err(CameraError::InvalidBounds)
        );
        assert_eq!(
            perspective.fit_bounds(bounds, 1.0, 0.9),
            Err(CameraError::InvalidFraming)
        );
        assert_eq!(
            perspective.fit_bounds(bounds, 1.0, 1.0),
            Err(CameraError::BoundsOutsideClipRange)
        );
    }

    #[test]
    fn orbit_controls_reject_invalid_parameters() {
        assert_eq!(
            OrbitCamera::new(
                Vec3::ZERO,
                0.0,
                0.0,
                0.0,
                Projection::Orthographic {
                    height: 1.0,
                    near: 0.1,
                    far: 10.0,
                }
            ),
            Err(CameraError::InvalidOrbit)
        );
        let orbit = OrbitCamera::new(
            Vec3::ZERO,
            0.0,
            0.0,
            1.0,
            Projection::Perspective {
                fov_y_radians: 1.0,
                near: 0.1,
                far: 10.0,
            },
        )
        .unwrap();
        assert_eq!(orbit.zoom(f32::NAN), Err(CameraError::InvalidOrbit));
        assert_eq!(
            orbit.pan(Vec2::new(f32::INFINITY, 0.0)),
            Err(CameraError::InvalidOrbit)
        );
    }
}
