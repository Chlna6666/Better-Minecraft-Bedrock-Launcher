pub(super) fn texture_edge_length(a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = a[0] - b[0];
    let dy = a[1] - b[1];
    (dx * dx + dy * dy).sqrt()
}

pub(super) fn barycentric2(a: [f32; 2], b: [f32; 2], c: [f32; 2], weights: [f32; 3]) -> [f32; 2] {
    [
        a[0] * weights[0] + b[0] * weights[1] + c[0] * weights[2],
        a[1] * weights[0] + b[1] * weights[1] + c[1] * weights[2],
    ]
}

pub(super) fn barycentric3(a: [f32; 3], b: [f32; 3], c: [f32; 3], weights: [f32; 3]) -> [f32; 3] {
    [
        a[0] * weights[0] + b[0] * weights[1] + c[0] * weights[2],
        a[1] * weights[0] + b[1] * weights[1] + c[1] * weights[2],
        a[2] * weights[0] + b[2] * weights[1] + c[2] * weights[2],
    ]
}

pub(super) fn average2(values: [[f32; 2]; 3]) -> [f32; 2] {
    [
        (values[0][0] + values[1][0] + values[2][0]) / 3.0,
        (values[0][1] + values[1][1] + values[2][1]) / 3.0,
    ]
}

pub(super) fn average3(values: [[f32; 3]; 3]) -> [f32; 3] {
    [
        (values[0][0] + values[1][0] + values[2][0]) / 3.0,
        (values[0][1] + values[1][1] + values[2][1]) / 3.0,
        (values[0][2] + values[1][2] + values[2][2]) / 3.0,
    ]
}

pub(super) fn clamp_image_index(value: f32, size: u32) -> u32 {
    if size == 0 || !value.is_finite() {
        return 0;
    }
    value.floor().clamp(0.0, size.saturating_sub(1) as f32) as u32
}

pub(super) fn rotate_point_around(
    point: [f32; 3],
    pivot: [f32; 3],
    rotation: [f32; 3],
) -> [f32; 3] {
    add3(pivot, rotate_vector(sub3(point, pivot), rotation))
}

pub(super) fn rotate_vector(vector: [f32; 3], rotation: [f32; 3]) -> [f32; 3] {
    let [x, y, z] = rotation.map(f32::to_radians);
    let vector = rotate_x(vector, x);
    let vector = rotate_y(vector, y);
    rotate_z(vector, z)
}

fn rotate_x([x, y, z]: [f32; 3], angle: f32) -> [f32; 3] {
    let (sin, cos) = angle.sin_cos();
    [x, y * cos - z * sin, y * sin + z * cos]
}

fn rotate_y([x, y, z]: [f32; 3], angle: f32) -> [f32; 3] {
    let (sin, cos) = angle.sin_cos();
    [x * cos + z * sin, y, -x * sin + z * cos]
}

fn rotate_z([x, y, z]: [f32; 3], angle: f32) -> [f32; 3] {
    let (sin, cos) = angle.sin_cos();
    [x * cos - y * sin, x * sin + y * cos, z]
}

pub(super) fn normal_from_corners(corners: [[f32; 3]; 4]) -> [f32; 3] {
    normalize(cross(
        sub3(corners[1], corners[0]),
        sub3(corners[2], corners[0]),
    ))
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub(super) fn normalize(vector: [f32; 3]) -> [f32; 3] {
    let length = (vector[0] * vector[0] + vector[1] * vector[1] + vector[2] * vector[2]).sqrt();
    if length <= f32::EPSILON {
        [0.0, 1.0, 0.0]
    } else {
        [vector[0] / length, vector[1] / length, vector[2] / length]
    }
}

pub(super) fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub(super) fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// Axis-aligned bounds of authored geometry in its own player coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct GeometryBounds {
    pub(super) min: [f32; 3],
    pub(super) max: [f32; 3],
}

impl GeometryBounds {
    pub(super) fn from_point(point: [f32; 3]) -> Self {
        Self {
            min: point,
            max: point,
        }
    }

    pub(super) fn include(&mut self, point: [f32; 3]) {
        for axis in 0..3 {
            self.min[axis] = self.min[axis].min(point[axis]);
            self.max[axis] = self.max[axis].max(point[axis]);
        }
    }

    fn is_finite(self) -> bool {
        self.min.iter().chain(self.max.iter()).all(|value| value.is_finite())
    }
}

/// Uniform map from authored geometry space into the preview player skeleton.
///
/// Minecraft skin-pack geometry is authored in its own player model space: vanilla humanoid
/// geometry spans 32 units from feet to head top, and a custom model can be any size. The preview
/// skin skeleton places feet at `PREVIEW_FEET_Y` and the head top at `PREVIEW_HEAD_TOP_Y`, so
/// authored geometry is fitted into that box instead of being shifted by a fixed amount.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ModelSpace {
    scale: f32,
    translation: [f32; 3],
}

/// Lowest preview Y of the reference player skeleton used by the skin preview.
pub(super) const PREVIEW_FEET_Y: f32 = -4.0;
/// Highest preview Y of the reference player skeleton used by the skin preview.
pub(super) const PREVIEW_HEAD_TOP_Y: f32 = 16.0;

const MODEL_SPACE_MIN_SCALE: f32 = 0.02;
const MODEL_SPACE_MAX_SCALE: f32 = 4.0;
const MODEL_SPACE_MIN_HEIGHT: f32 = 0.05;

impl ModelSpace {
    /// Maps authored bounds into the preview skeleton box, preserving aspect ratio.
    ///
    /// Degenerate or non-finite bounds fall back to authored units so a malformed geometry cannot
    /// produce a non-finite transform.
    pub(super) fn fit(bounds: GeometryBounds) -> Self {
        if !bounds.is_finite() {
            return Self::authored_units();
        }
        let height = bounds.max[1] - bounds.min[1];
        let center_x = (bounds.min[0] + bounds.max[0]) * 0.5;
        if height < MODEL_SPACE_MIN_HEIGHT {
            return Self {
                scale: 1.0,
                translation: [-center_x, PREVIEW_FEET_Y - bounds.min[1], 0.0],
            };
        }
        let scale = ((PREVIEW_HEAD_TOP_Y - PREVIEW_FEET_Y) / height)
            .clamp(MODEL_SPACE_MIN_SCALE, MODEL_SPACE_MAX_SCALE);
        Self {
            scale,
            translation: [
                -center_x * scale,
                PREVIEW_FEET_Y - bounds.min[1] * scale,
                0.0,
            ],
        }
    }

    /// Keeps authored units and only moves the model so its feet rest on the preview ground.
    pub(super) const fn authored_units() -> Self {
        Self {
            scale: 1.0,
            translation: [0.0, 0.0, 0.0],
        }
    }

    pub(super) const fn scale(self) -> f32 {
        self.scale
    }

    pub(super) fn point(self, point: [f32; 3]) -> [f32; 3] {
        add3(scale3(point, self.scale), self.translation)
    }

    pub(super) fn length(self, value: f32) -> f32 {
        value * self.scale
    }
}

fn scale3(vector: [f32; 3], factor: f32) -> [f32; 3] {
    [vector[0] * factor, vector[1] * factor, vector[2] * factor]
}
