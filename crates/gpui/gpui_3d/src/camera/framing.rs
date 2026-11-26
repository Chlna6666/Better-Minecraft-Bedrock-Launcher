use super::{CameraError, OrbitCamera, Projection};
use crate::{Aabb, Vec3};

struct FramingBounds {
    center: Vec3,
    corners: [[f32; 3]; 8],
}

#[derive(Clone, Copy)]
struct FramingOptions {
    aspect: f32,
    padding: f32,
}

impl FramingBounds {
    fn new(bounds: Aabb, orbit: OrbitCamera) -> Result<Self, CameraError> {
        if Aabb::new(bounds.min, bounds.max).is_none() {
            return Err(CameraError::InvalidBounds);
        }
        let half_size = (bounds.max - bounds.min) * 0.5;
        let center = bounds.min + half_size;
        if !half_size.is_finite() || !center.is_finite() {
            return Err(CameraError::InvalidBounds);
        }
        let camera = orbit.camera();
        let forward = (camera.target - camera.eye)
            .normalized()
            .ok_or(CameraError::InvalidPose)?;
        let right = forward
            .cross(camera.up)
            .normalized()
            .ok_or(CameraError::InvalidPose)?;
        let up = right
            .cross(forward)
            .normalized()
            .ok_or(CameraError::InvalidPose)?;
        let mut corners = [[0.0; 3]; 8];
        let mut index = 0;
        for x in [bounds.min.x, bounds.max.x] {
            for y in [bounds.min.y, bounds.max.y] {
                for z in [bounds.min.z, bounds.max.z] {
                    let offset = Vec3::new(x, y, z) - center;
                    let projected = [offset.dot(right), offset.dot(up), offset.dot(forward)];
                    if !projected.iter().all(|component| component.is_finite()) {
                        return Err(CameraError::InvalidBounds);
                    }
                    corners[index] = projected;
                    index += 1;
                }
            }
        }
        Ok(Self { center, corners })
    }
}

impl OrbitCamera {
    /// Fits world-space bounds while preserving the current orbit direction and projection mode.
    ///
    /// `aspect` is the projected viewport width divided by its height. `padding` is a scale
    /// factor: `1.0` fits the bounds to the viewport edges, while larger values leave more margin.
    /// Perspective fitting moves the camera; orthographic fitting changes the view height. Both
    /// modes set the target to the bounds center and adjust camera distance only as needed to place
    /// every corner between the existing near and far planes. Callers are responsible for computing
    /// bounds that include the scene content they want to frame.
    ///
    /// # Errors
    ///
    /// Returns [`CameraError::InvalidBounds`] for non-finite or reversed bounds,
    /// [`CameraError::InvalidFraming`] for a non-finite padding factor or one below `1.0`,
    /// [`CameraError::BoundsOutsideClipRange`] when the current clipping planes cannot contain the
    /// bounds, and otherwise propagates pose or projection errors.
    pub fn fit_bounds(self, bounds: Aabb, aspect: f32, padding: f32) -> Result<Self, CameraError> {
        if !padding.is_finite() || padding < 1.0 {
            return Err(CameraError::InvalidFraming);
        }
        let _projection = self.camera().projection_matrix(aspect)?;
        let bounds = FramingBounds::new(bounds, self)?;
        let options = FramingOptions { aspect, padding };
        let fitted = match self.projection {
            Projection::Perspective { .. } => fit_perspective(self, bounds, options)?,
            Projection::Orthographic { .. } => fit_orthographic(self, bounds, options)?,
        };
        let _view_projection = fitted.camera().view_projection(aspect)?;
        Ok(fitted)
    }
}

fn fit_perspective(
    orbit: OrbitCamera,
    bounds: FramingBounds,
    options: FramingOptions,
) -> Result<OrbitCamera, CameraError> {
    let Projection::Perspective {
        fov_y_radians,
        near,
        far,
    } = orbit.projection
    else {
        return Err(CameraError::InvalidProjection);
    };
    let tan_y = (fov_y_radians * 0.5).tan();
    let tan_x = tan_y * options.aspect;
    let mut distance = f32::MIN_POSITIVE;
    let mut farthest_distance = f32::INFINITY;
    for [x, y, depth] in bounds.corners {
        distance = distance
            .max(options.padding * x.abs() / tan_x - depth)
            .max(options.padding * y.abs() / tan_y - depth)
            .max(near - depth);
        farthest_distance = farthest_distance.min(far - depth);
    }
    if !distance.is_finite() || distance > farthest_distance {
        return Err(CameraError::BoundsOutsideClipRange);
    }
    Ok(OrbitCamera {
        target: bounds.center,
        distance,
        ..orbit
    })
}

fn fit_orthographic(
    orbit: OrbitCamera,
    bounds: FramingBounds,
    options: FramingOptions,
) -> Result<OrbitCamera, CameraError> {
    let Projection::Orthographic { near, far, .. } = orbit.projection else {
        return Err(CameraError::InvalidProjection);
    };
    let mut height = f32::EPSILON;
    let mut nearest_distance = f32::NEG_INFINITY;
    let mut farthest_distance = f32::INFINITY;
    for [x, y, depth] in bounds.corners {
        height = height.max(2.0 * options.padding * y.abs());
        height = height.max(2.0 * options.padding * x.abs() / options.aspect);
        nearest_distance = nearest_distance.max(near - depth);
        farthest_distance = farthest_distance.min(far - depth);
    }
    if !height.is_finite() {
        return Err(CameraError::InvalidBounds);
    }
    if nearest_distance > farthest_distance {
        return Err(CameraError::BoundsOutsideClipRange);
    }
    let distance = orbit.distance.clamp(nearest_distance, farthest_distance);
    Ok(OrbitCamera {
        target: bounds.center,
        distance,
        projection: Projection::Orthographic { height, near, far },
        ..orbit
    })
}
