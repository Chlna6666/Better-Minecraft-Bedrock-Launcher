use std::ops::{Add, Div, Mul, Neg, Sub};

/// Two-dimensional texture coordinate.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    /// Horizontal component.
    pub x: f32,
    /// Vertical component.
    pub y: f32,
}

impl Vec2 {
    /// Zero texture coordinate.
    pub const ZERO: Self = Self::new(0.0, 0.0);

    /// Creates a texture coordinate.
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// Three-dimensional vector.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[must_use]
pub struct Vec3 {
    /// X component.
    pub x: f32,
    /// Y component.
    pub y: f32,
    /// Z component.
    pub z: f32,
}

impl Vec3 {
    /// Zero vector.
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);
    /// Unit vector along positive X.
    pub const X: Self = Self::new(1.0, 0.0, 0.0);
    /// Unit vector along positive Y.
    pub const Y: Self = Self::new(0.0, 1.0, 0.0);
    /// Unit vector along positive Z.
    pub const Z: Self = Self::new(0.0, 0.0, 1.0);

    /// Creates a vector.
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    /// Dot product.
    pub fn dot(self, rhs: Self) -> f32 {
        self.x * rhs.x + self.y * rhs.y + self.z * rhs.z
    }

    /// Cross product.
    pub fn cross(self, rhs: Self) -> Self {
        Self::new(
            self.y * rhs.z - self.z * rhs.y,
            self.z * rhs.x - self.x * rhs.z,
            self.x * rhs.y - self.y * rhs.x,
        )
    }

    /// Squared length.
    pub fn length_squared(self) -> f32 {
        self.dot(self)
    }

    /// Length.
    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    /// Returns a normalized vector, or `None` for zero or non-finite length.
    pub fn normalized(self) -> Option<Self> {
        let length = self.length();
        (length.is_finite() && length > f32::EPSILON).then(|| self / length)
    }

    /// Component-wise minimum.
    pub fn min(self, rhs: Self) -> Self {
        Self::new(self.x.min(rhs.x), self.y.min(rhs.y), self.z.min(rhs.z))
    }

    /// Component-wise maximum.
    pub fn max(self, rhs: Self) -> Self {
        Self::new(self.x.max(rhs.x), self.y.max(rhs.y), self.z.max(rhs.z))
    }

    /// Returns whether every component is finite.
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
}

impl Add for Vec3 {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        Self::new(self.x + rhs.x, self.y + rhs.y, self.z + rhs.z)
    }
}

impl Sub for Vec3 {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self::Output {
        Self::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z)
    }
}

impl Mul<f32> for Vec3 {
    type Output = Self;
    fn mul(self, rhs: f32) -> Self::Output {
        Self::new(self.x * rhs, self.y * rhs, self.z * rhs)
    }
}

impl Div<f32> for Vec3 {
    type Output = Self;
    fn div(self, rhs: f32) -> Self::Output {
        Self::new(self.x / rhs, self.y / rhs, self.z / rhs)
    }
}

impl Neg for Vec3 {
    type Output = Self;
    fn neg(self) -> Self::Output {
        Self::new(-self.x, -self.y, -self.z)
    }
}

/// Unit quaternion used for node rotation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quat {
    /// X component.
    pub x: f32,
    /// Y component.
    pub y: f32,
    /// Z component.
    pub z: f32,
    /// Scalar component.
    pub w: f32,
}

impl Default for Quat {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Quat {
    /// Identity rotation.
    pub const IDENTITY: Self = Self::new(0.0, 0.0, 0.0, 1.0);

    /// Creates a quaternion.
    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }

    /// Creates a rotation around `axis` by `radians`.
    pub fn from_axis_angle(axis: Vec3, radians: f32) -> Option<Self> {
        let axis = axis.normalized()?;
        if !radians.is_finite() {
            return None;
        }
        let half_angle = radians * 0.5;
        let sin = half_angle.sin();
        Some(Self::new(
            axis.x * sin,
            axis.y * sin,
            axis.z * sin,
            half_angle.cos(),
        ))
    }

    /// Normalizes the quaternion, or returns `None` when it has no finite direction.
    pub fn normalized(self) -> Option<Self> {
        let length = (self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w).sqrt();
        (length.is_finite() && length > f32::EPSILON).then(|| {
            Self::new(
                self.x / length,
                self.y / length,
                self.z / length,
                self.w / length,
            )
        })
    }

    /// Dot product of two quaternions.
    pub fn dot(self, rhs: Self) -> f32 {
        self.x * rhs.x + self.y * rhs.y + self.z * rhs.z + self.w * rhs.w
    }

    /// Spherical interpolation along the shortest arc between two rotations.
    ///
    /// The interpolation amount is clamped to `0..=1`; invalid quaternions or a non-finite
    /// amount return `None`.
    pub fn slerp(self, target: Self, amount: f32) -> Option<Self> {
        if !amount.is_finite() {
            return None;
        }
        let start = self.normalized()?;
        let mut end = target.normalized()?;
        let mut dot = start.dot(end);
        if dot < 0.0 {
            end = Self::new(-end.x, -end.y, -end.z, -end.w);
            dot = -dot;
        }
        let amount = amount.clamp(0.0, 1.0);
        if dot > 0.9995 {
            return Self::new(
                start.x + (end.x - start.x) * amount,
                start.y + (end.y - start.y) * amount,
                start.z + (end.z - start.z) * amount,
                start.w + (end.w - start.w) * amount,
            )
            .normalized();
        }
        let angle = dot.clamp(-1.0, 1.0).acos();
        let sine = angle.sin();
        if sine.abs() <= f32::EPSILON {
            return Some(start);
        }
        let start_weight = ((1.0 - amount) * angle).sin() / sine;
        let end_weight = (amount * angle).sin() / sine;
        Self::new(
            start.x * start_weight + end.x * end_weight,
            start.y * start_weight + end.y * end_weight,
            start.z * start_weight + end.z * end_weight,
            start.w * start_weight + end.w * end_weight,
        )
        .normalized()
    }

    /// Rotates a vector.
    pub fn rotate(self, vector: Vec3) -> Vec3 {
        let q = Vec3::new(self.x, self.y, self.z);
        let t = q.cross(vector) * 2.0;
        vector + t * self.w + q.cross(t)
    }
}

/// Translation, rotation, and scale for a scene node.
#[derive(Clone, Copy, Debug, PartialEq)]
#[must_use]
pub struct Transform {
    /// Local translation.
    pub translation: Vec3,
    /// Local rotation.
    pub rotation: Quat,
    /// Local scale.
    pub scale: Vec3,
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Transform {
    /// Identity transform.
    pub const IDENTITY: Self = Self {
        translation: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::new(1.0, 1.0, 1.0),
    };

    /// Converts this transform to a column-major matrix.
    pub fn matrix(self) -> Mat4 {
        let q = self.rotation.normalized().unwrap_or(Quat::IDENTITY);
        let (x2, y2, z2) = (q.x + q.x, q.y + q.y, q.z + q.z);
        let (xx, xy, xz) = (q.x * x2, q.x * y2, q.x * z2);
        let (yy, yz, zz) = (q.y * y2, q.y * z2, q.z * z2);
        let (wx, wy, wz) = (q.w * x2, q.w * y2, q.w * z2);
        Mat4([
            [
                (1.0 - (yy + zz)) * self.scale.x,
                (xy + wz) * self.scale.x,
                (xz - wy) * self.scale.x,
                0.0,
            ],
            [
                (xy - wz) * self.scale.y,
                (1.0 - (xx + zz)) * self.scale.y,
                (yz + wx) * self.scale.y,
                0.0,
            ],
            [
                (xz + wy) * self.scale.z,
                (yz - wx) * self.scale.z,
                (1.0 - (xx + yy)) * self.scale.z,
                0.0,
            ],
            [
                self.translation.x,
                self.translation.y,
                self.translation.z,
                1.0,
            ],
        ])
    }
}

/// Column-major 4x4 matrix.
#[derive(Clone, Copy, Debug, PartialEq)]
#[must_use]
pub struct Mat4([[f32; 4]; 4]);

impl Default for Mat4 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Mat4 {
    /// Identity matrix.
    pub const IDENTITY: Self = Self([
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]);

    /// Creates a matrix from four column vectors.
    pub const fn from_columns(columns: [[f32; 4]; 4]) -> Self {
        Self(columns)
    }

    /// Returns the column-major matrix values.
    pub const fn columns(self) -> [[f32; 4]; 4] {
        self.0
    }

    /// Builds a right-handed view matrix from eye, target, and up vectors.
    pub fn look_at(eye: Vec3, target: Vec3, up: Vec3) -> Option<Self> {
        let forward = (target - eye).normalized()?;
        let right = forward.cross(up).normalized()?;
        let up = right.cross(forward);
        let matrix = Self([
            [right.x, up.x, -forward.x, 0.0],
            [right.y, up.y, -forward.y, 0.0],
            [right.z, up.z, -forward.z, 0.0],
            [-right.dot(eye), -up.dot(eye), forward.dot(eye), 1.0],
        ]);
        matrix.is_finite().then_some(matrix)
    }

    /// Builds a right-handed perspective projection with a zero-to-one depth range.
    pub fn perspective(fov_y_radians: f32, aspect: f32, near: f32, far: f32) -> Option<Self> {
        if !fov_y_radians.is_finite()
            || !aspect.is_finite()
            || !near.is_finite()
            || !far.is_finite()
            || fov_y_radians <= 0.0
            || fov_y_radians >= std::f32::consts::PI
            || aspect <= 0.0
            || near <= 0.0
            || far <= near
        {
            return None;
        }
        let focal = 1.0 / (fov_y_radians * 0.5).tan();
        let matrix = Self([
            [focal / aspect, 0.0, 0.0, 0.0],
            [0.0, focal, 0.0, 0.0],
            [0.0, 0.0, far / (near - far), -1.0],
            [0.0, 0.0, (near * far) / (near - far), 0.0],
        ]);
        matrix.is_finite().then_some(matrix)
    }

    /// Multiplies a homogeneous vector.
    pub fn transform4(self, value: [f32; 4]) -> [f32; 4] {
        let mut result = [0.0; 4];
        for (column, component) in value.into_iter().enumerate() {
            for (row, output) in result.iter_mut().enumerate() {
                *output += self.0[column][row] * component;
            }
        }
        result
    }

    /// Transforms a point, performing perspective division when `w` is nonzero.
    pub fn transform_point(self, point: Vec3) -> Vec3 {
        let value = self.transform4([point.x, point.y, point.z, 1.0]);
        if value[3].abs() > f32::EPSILON {
            Vec3::new(
                value[0] / value[3],
                value[1] / value[3],
                value[2] / value[3],
            )
        } else {
            Vec3::new(value[0], value[1], value[2])
        }
    }

    /// Transforms a direction without translation.
    pub fn transform_vector(self, vector: Vec3) -> Vec3 {
        let value = self.transform4([vector.x, vector.y, vector.z, 0.0]);
        Vec3::new(value[0], value[1], value[2])
    }

    /// Returns the matrix inverse, or `None` for a singular/non-finite matrix.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "The inverse is computed in f64 then rounded to the crate's f32 matrix representation; overflow is rejected below."
    )]
    pub fn inverse(self) -> Option<Self> {
        let mut rows = [[0.0_f64; 8]; 4];
        for (column, values) in self.0.iter().enumerate() {
            for (row_index, row) in rows.iter_mut().enumerate() {
                row[column] = f64::from(values[row_index]);
            }
        }
        for (row_index, row) in rows.iter_mut().enumerate() {
            row[row_index + 4] = 1.0;
        }
        for pivot in 0..4 {
            let best = (pivot..4).max_by(|left, right| {
                rows[*left][pivot]
                    .abs()
                    .total_cmp(&rows[*right][pivot].abs())
            })?;
            if rows[best][pivot].abs() <= f64::EPSILON {
                return None;
            }
            rows.swap(pivot, best);
            let divisor = rows[pivot][pivot];
            for entry in &mut rows[pivot] {
                *entry /= divisor;
            }
            let pivot_row = rows[pivot];
            for (row_index, row) in rows.iter_mut().enumerate() {
                if row_index == pivot {
                    continue;
                }
                let factor = row[pivot];
                for (entry, pivot_entry) in row.iter_mut().zip(pivot_row) {
                    *entry -= factor * pivot_entry;
                }
            }
        }
        let mut inverse = [[0.0; 4]; 4];
        for (row_index, row) in rows.iter().enumerate() {
            for (column_index, value) in row[4..].iter().copied().enumerate() {
                let value = value as f32;
                if !value.is_finite() {
                    return None;
                }
                inverse[column_index][row_index] = value;
            }
        }
        Some(Self(inverse))
    }

    pub(crate) fn is_finite(self) -> bool {
        self.0.into_iter().flatten().all(f32::is_finite)
    }

    pub(crate) fn row(self, row: usize) -> [f32; 4] {
        [
            self.0[0][row],
            self.0[1][row],
            self.0[2][row],
            self.0[3][row],
        ]
    }
}

impl Mul for Mat4 {
    type Output = Self;

    fn mul(self, rhs: Self) -> Self::Output {
        let mut result = [[0.0; 4]; 4];
        for (column_index, result_column) in result.iter_mut().enumerate() {
            for (row_index, result_value) in result_column.iter_mut().enumerate() {
                for (left_column_index, left_column) in self.0.iter().enumerate() {
                    *result_value +=
                        left_column[row_index] * rhs.0[column_index][left_column_index];
                }
            }
        }
        Self(result)
    }
}

/// Axis-aligned bounding box in local or world space.
#[derive(Clone, Copy, Debug, PartialEq)]
#[must_use]
pub struct Aabb {
    /// Minimum corner.
    pub min: Vec3,
    /// Maximum corner.
    pub max: Vec3,
}

impl Aabb {
    /// Creates bounds when both corners are finite and ordered.
    pub fn new(min: Vec3, max: Vec3) -> Option<Self> {
        (min.is_finite() && max.is_finite() && min.x <= max.x && min.y <= max.y && min.z <= max.z)
            .then_some(Self { min, max })
    }

    /// Transforms the bounds using all eight corners.
    pub fn transformed(self, matrix: Mat4) -> Self {
        let mut min = Vec3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
        let mut max = Vec3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
        for x in [self.min.x, self.max.x] {
            for y in [self.min.y, self.max.y] {
                for z in [self.min.z, self.max.z] {
                    let point = matrix.transform_point(Vec3::new(x, y, z));
                    min = min.min(point);
                    max = max.max(point);
                }
            }
        }
        Self { min, max }
    }

    pub(crate) fn ray_entry(self, origin: Vec3, direction: Vec3, max_distance: f32) -> Option<f32> {
        let mut near = 0.0_f32;
        let mut far = max_distance;
        for (origin, direction, min, max) in [
            (origin.x, direction.x, self.min.x, self.max.x),
            (origin.y, direction.y, self.min.y, self.max.y),
            (origin.z, direction.z, self.min.z, self.max.z),
        ] {
            if direction.abs() <= f32::EPSILON {
                if origin < min || origin > max {
                    return None;
                }
                continue;
            }
            let inv_direction = direction.recip();
            let mut a = (min - origin) * inv_direction;
            let mut b = (max - origin) * inv_direction;
            if a > b {
                std::mem::swap(&mut a, &mut b);
            }
            near = near.max(a);
            far = far.min(b);
            if near > far {
                return None;
            }
        }
        (far >= 0.0 && near.is_finite()).then_some(near)
    }
}

/// Ray with a normalized direction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    /// Ray origin.
    pub origin: Vec3,
    /// Unit direction.
    pub direction: Vec3,
}

impl Ray {
    /// Creates a ray, normalizing its direction.
    pub fn new(origin: Vec3, direction: Vec3) -> Option<Self> {
        (origin.is_finite())
            .then(|| {
                direction
                    .normalized()
                    .map(|direction| Self { origin, direction })
            })
            .flatten()
    }

    /// Returns the point at distance `t`.
    pub fn at(self, t: f32) -> Vec3 {
        self.origin + self.direction * t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_matrix_round_trips_points_through_inverse() {
        let transform = Transform {
            translation: Vec3::new(4.0, -2.0, 3.0),
            rotation: Quat::from_axis_angle(Vec3::Y, 0.7).unwrap(),
            scale: Vec3::new(2.0, 1.5, 0.5),
        };
        let matrix = transform.matrix();
        let point = Vec3::new(1.0, 2.0, -3.0);
        let actual = matrix
            .inverse()
            .unwrap()
            .transform_point(matrix.transform_point(point));
        assert!((actual - point).length() < 1e-4);
    }

    #[test]
    fn perspective_maps_near_and_far_to_zero_and_one_depth() {
        let projection = Mat4::perspective(1.0, 1.5, 0.5, 50.0).unwrap();
        let near = projection.transform_point(Vec3::new(0.0, 0.0, -0.5));
        let far = projection.transform_point(Vec3::new(0.0, 0.0, -50.0));
        assert!(near.z.abs() < 1e-5);
        assert!((far.z - 1.0).abs() < 1e-5);
    }
}
