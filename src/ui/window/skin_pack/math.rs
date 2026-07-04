pub(super) fn mat4_identity() -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

pub(super) fn mat4_translation(offset: [f32; 3]) -> [[f32; 4]; 4] {
    let mut matrix = mat4_identity();
    matrix[3][0] = offset[0];
    matrix[3][1] = offset[1];
    matrix[3][2] = offset[2];
    matrix
}

pub(super) fn mat4_scale(scale: [f32; 3]) -> [[f32; 4]; 4] {
    [
        [scale[0], 0.0, 0.0, 0.0],
        [0.0, scale[1], 0.0, 0.0],
        [0.0, 0.0, scale[2], 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

pub(super) fn mat4_rotation_x(angle: f32) -> [[f32; 4]; 4] {
    let (sin, cos) = angle.sin_cos();
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, cos, sin, 0.0],
        [0.0, -sin, cos, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

pub(super) fn mat4_rotation_y(angle: f32) -> [[f32; 4]; 4] {
    let (sin, cos) = angle.sin_cos();
    [
        [cos, 0.0, -sin, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [sin, 0.0, cos, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

pub(super) fn mat4_mul(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = mat4_identity();
    for col in 0..4 {
        for row in 0..4 {
            out[col][row] = a[0][row] * b[col][0]
                + a[1][row] * b[col][1]
                + a[2][row] * b[col][2]
                + a[3][row] * b[col][3];
        }
    }
    out
}
