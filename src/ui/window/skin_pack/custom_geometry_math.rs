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

pub(super) fn bedrock_to_preview(position: [f32; 3]) -> [f32; 3] {
    [position[0], position[1] - 16.0, position[2]]
}
