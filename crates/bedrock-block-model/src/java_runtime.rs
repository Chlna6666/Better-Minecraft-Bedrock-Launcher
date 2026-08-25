use std::mem;

use crate::{
    BlockFace, BlockStateQuery, JavaModelApplication, JavaModelAxis, JavaModelDatabase,
    JavaPackedElement, JavaPackedFace, ModelCuboid, ModelPlane, ModelShape,
    java_block_id_for_bedrock_state, java_properties_for_bedrock_state,
    vanilla_java_model_database,
};

/// Resolves a Bedrock block state through the embedded Java Edition model database and expands
/// the selected packed applications into renderer-neutral geometry.
///
/// The database remains zero-copy. Allocation only occurs for the final `ModelShape`, so callers
/// that render repeated palette entries should cache the resulting shape.
#[must_use]
pub fn java_model_shape_for_bedrock_state(
    state: &BlockStateQuery,
    seed: u64,
) -> Option<ModelShape> {
    java_model_shape_for_bedrock_state_in_database(*vanilla_java_model_database(), state, seed)
}

fn java_model_shape_for_bedrock_state_in_database(
    database: JavaModelDatabase<'_>,
    state: &BlockStateQuery,
    seed: u64,
) -> Option<ModelShape> {
    let java_block_id = java_block_id_for_bedrock_state(state);
    let properties = java_properties_for_bedrock_state(state);
    let mut shape = ModelShape::default();
    let mut applied = false;

    let block_exists = database.for_each_model_application(
        &java_block_id,
        &properties,
        seed,
        |application| {
            let Some(model) = database.model(application.model) else {
                return;
            };
            applied = true;
            for element in model.elements() {
                push_packed_element(&mut shape, element, application);
            }
        },
    );

    (block_exists && applied && !shape.is_empty()).then_some(shape)
}

fn push_packed_element(
    shape: &mut ModelShape,
    element: JavaPackedElement<'_>,
    application: JavaModelApplication,
) {
    let (min, max) = ordered_bounds(element.from_block(), element.to_block());
    let axis_aligned = element.rotation_axis.is_none() || element.rotation_angle_hundredths == 0;
    let complete_cube_faces = element.faces().len() == 6;

    // Preserve complete axis-aligned elements as cuboids. This keeps vanilla full blocks on the
    // renderer's mergeable full-cube fast path instead of turning them into six detail planes.
    if axis_aligned && complete_cube_faces {
        let mut cuboid = ModelCuboid::new(min, max);
        for face in element.faces() {
            cuboid = cuboid
                .with_face_material_slot(face.face, face.material_slot)
                .with_face_uv(face.face, packed_face_uv(face, min, max));
        }
        rotate_cuboid_for_application(&mut cuboid, application);
        shape.cuboids.push(cuboid);
        return;
    }

    // Partial-face elements and rotated Java elements are emitted as explicit quads. ModelPlane
    // already carries arbitrary corners, so ±22.5°/±45° geometry does not need an AABB fallback.
    for face in element.faces() {
        let Some((corners, normal)) = face_geometry(face.face, min, max) else {
            continue;
        };
        let corners = corners.map(|point| transform_model_point(point, element, application));
        let normal = transform_model_normal(normal, element, application);
        shape.planes.push(
            ModelPlane::new(corners, nearest_axis_normal(normal))
                .with_material_slot(face.material_slot)
                .with_uv(packed_face_uv(face, min, max)),
        );
    }
}

fn ordered_bounds(from: [f32; 3], to: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    (
        [
            from[0].min(to[0]),
            from[1].min(to[1]),
            from[2].min(to[2]),
        ],
        [
            from[0].max(to[0]),
            from[1].max(to[1]),
            from[2].max(to[2]),
        ],
    )
}

fn packed_face_uv(
    face: JavaPackedFace<'_>,
    min: [f32; 3],
    max: [f32; 3],
) -> [[f32; 2]; 4] {
    let [u0, v0, u1, v1] = face
        .uv_normalized()
        .unwrap_or_else(|| default_face_uv_rect(face.face, min, max));
    rotate_uv_quarter_turns(
        [[u0, v0], [u1, v0], [u1, v1], [u0, v1]],
        face.rotation_quarter_turns,
    )
}

fn default_face_uv_rect(face: BlockFace, min: [f32; 3], max: [f32; 3]) -> [f32; 4] {
    match face {
        BlockFace::Up | BlockFace::Down => [min[0], min[2], max[0], max[2]],
        BlockFace::North | BlockFace::South => [min[0], min[1], max[0], max[1]],
        BlockFace::West | BlockFace::East => [min[2], min[1], max[2], max[1]],
        BlockFace::Side | BlockFace::All | BlockFace::Default => [0.0, 0.0, 1.0, 1.0],
    }
}

fn rotate_uv_quarter_turns(mut uv: [[f32; 2]; 4], turns: u8) -> [[f32; 2]; 4] {
    for _ in 0..turns.rem_euclid(4) {
        uv = [uv[3], uv[0], uv[1], uv[2]];
    }
    uv
}

fn face_geometry(
    face: BlockFace,
    min: [f32; 3],
    max: [f32; 3],
) -> Option<([[f32; 3]; 4], [f32; 3])> {
    let [x0, y0, z0] = min;
    let [x1, y1, z1] = max;
    match face {
        BlockFace::Up => Some((
            [[x0, y1, z0], [x1, y1, z0], [x1, y1, z1], [x0, y1, z1]],
            [0.0, 1.0, 0.0],
        )),
        BlockFace::Down => Some((
            [[x0, y0, z1], [x1, y0, z1], [x1, y0, z0], [x0, y0, z0]],
            [0.0, -1.0, 0.0],
        )),
        BlockFace::East => Some((
            [[x1, y0, z0], [x1, y0, z1], [x1, y1, z1], [x1, y1, z0]],
            [1.0, 0.0, 0.0],
        )),
        BlockFace::West => Some((
            [[x0, y0, z1], [x0, y0, z0], [x0, y1, z0], [x0, y1, z1]],
            [-1.0, 0.0, 0.0],
        )),
        BlockFace::South => Some((
            [[x1, y0, z1], [x0, y0, z1], [x0, y1, z1], [x1, y1, z1]],
            [0.0, 0.0, 1.0],
        )),
        BlockFace::North => Some((
            [[x0, y0, z0], [x1, y0, z0], [x1, y1, z0], [x0, y1, z0]],
            [0.0, 0.0, -1.0],
        )),
        BlockFace::Side | BlockFace::All | BlockFace::Default => None,
    }
}

fn transform_model_point(
    mut point: [f32; 3],
    element: JavaPackedElement<'_>,
    application: JavaModelApplication,
) -> [f32; 3] {
    if let Some(axis) = element.rotation_axis {
        let angle = element.rotation_angle_degrees().to_radians();
        if angle.abs() > f32::EPSILON {
            let origin = element.rotation_origin_block();
            let mut local = sub3(point, origin);
            if element.rescale {
                let cosine = angle.cos().abs();
                if cosine > 0.000_001 {
                    let factor = cosine.recip();
                    match axis {
                        JavaModelAxis::X => {
                            local[1] *= factor;
                            local[2] *= factor;
                        }
                        JavaModelAxis::Y => {
                            local[0] *= factor;
                            local[2] *= factor;
                        }
                        JavaModelAxis::Z => {
                            local[0] *= factor;
                            local[1] *= factor;
                        }
                    }
                }
            }
            point = add3(origin, rotate_vector_axis(local, axis, angle));
        }
    }
    rotate_point_for_application(point, application)
}

fn transform_model_normal(
    mut normal: [f32; 3],
    element: JavaPackedElement<'_>,
    application: JavaModelApplication,
) -> [f32; 3] {
    if let Some(axis) = element.rotation_axis {
        normal = rotate_vector_axis(normal, axis, element.rotation_angle_degrees().to_radians());
    }
    rotate_vector_for_application(normal, application)
}

fn rotate_vector_axis([x, y, z]: [f32; 3], axis: JavaModelAxis, angle: f32) -> [f32; 3] {
    let (sin, cos) = angle.sin_cos();
    match axis {
        JavaModelAxis::X => [x, y * cos - z * sin, y * sin + z * cos],
        JavaModelAxis::Y => [x * cos - z * sin, y, x * sin + z * cos],
        JavaModelAxis::Z => [x * cos - y * sin, x * sin + y * cos, z],
    }
}

fn rotate_point_for_application(
    mut point: [f32; 3],
    application: JavaModelApplication,
) -> [f32; 3] {
    for _ in 0..quarter_turns(application.x_degrees) {
        point = rotate_point_x_90(point);
    }
    for _ in 0..quarter_turns(application.y_degrees) {
        point = rotate_point_y_90(point);
    }
    point
}

fn rotate_vector_for_application(
    mut vector: [f32; 3],
    application: JavaModelApplication,
) -> [f32; 3] {
    for _ in 0..quarter_turns(application.x_degrees) {
        vector = [vector[0], -vector[2], vector[1]];
    }
    for _ in 0..quarter_turns(application.y_degrees) {
        vector = [-vector[2], vector[1], vector[0]];
    }
    vector
}

fn rotate_cuboid_for_application(cuboid: &mut ModelCuboid, application: JavaModelApplication) {
    for _ in 0..quarter_turns(application.x_degrees) {
        rotate_cuboid(cuboid, rotate_point_x_90, rotate_face_x_90);
    }
    for _ in 0..quarter_turns(application.y_degrees) {
        rotate_cuboid(cuboid, rotate_point_y_90, rotate_face_y_90);
    }
}

fn rotate_cuboid(
    cuboid: &mut ModelCuboid,
    rotate_point: fn([f32; 3]) -> [f32; 3],
    rotate_face: fn(BlockFace) -> BlockFace,
) {
    let [x0, y0, z0] = cuboid.min;
    let [x1, y1, z1] = cuboid.max;
    let corners = [
        [x0, y0, z0],
        [x0, y0, z1],
        [x0, y1, z0],
        [x0, y1, z1],
        [x1, y0, z0],
        [x1, y0, z1],
        [x1, y1, z0],
        [x1, y1, z1],
    ]
    .map(rotate_point);

    cuboid.min = [f32::INFINITY; 3];
    cuboid.max = [f32::NEG_INFINITY; 3];
    for corner in corners {
        for axis in 0..3 {
            cuboid.min[axis] = cuboid.min[axis].min(corner[axis]);
            cuboid.max[axis] = cuboid.max[axis].max(corner[axis]);
        }
    }

    cuboid.face_material_slots = mem::take(&mut cuboid.face_material_slots)
        .into_iter()
        .map(|(face, slot)| (rotate_face(face), slot))
        .collect();
    cuboid.face_uvs = mem::take(&mut cuboid.face_uvs)
        .into_iter()
        .map(|(face, uv)| (rotate_face(face), uv))
        .collect();
}

fn quarter_turns(degrees: i16) -> u8 {
    u8::try_from(i32::from(degrees).rem_euclid(360) / 90)
        .expect("validated Java blockstate quarter turn")
}

fn rotate_point_x_90([x, y, z]: [f32; 3]) -> [f32; 3] {
    [x, 1.0 - z, y]
}

fn rotate_point_y_90([x, y, z]: [f32; 3]) -> [f32; 3] {
    [1.0 - z, y, x]
}

fn rotate_face_x_90(face: BlockFace) -> BlockFace {
    match face {
        BlockFace::North => BlockFace::Up,
        BlockFace::Up => BlockFace::South,
        BlockFace::South => BlockFace::Down,
        BlockFace::Down => BlockFace::North,
        other => other,
    }
}

fn rotate_face_y_90(face: BlockFace) -> BlockFace {
    match face {
        BlockFace::North => BlockFace::East,
        BlockFace::East => BlockFace::South,
        BlockFace::South => BlockFace::West,
        BlockFace::West => BlockFace::North,
        other => other,
    }
}

fn nearest_axis_normal(normal: [f32; 3]) -> [i32; 3] {
    let absolute = [normal[0].abs(), normal[1].abs(), normal[2].abs()];
    let axis = if absolute[1] > absolute[0] && absolute[1] >= absolute[2] {
        1
    } else if absolute[2] > absolute[0] && absolute[2] > absolute[1] {
        2
    } else {
        0
    };
    let mut result = [0, 0, 0];
    result[axis] = if normal[axis].is_sign_negative() { -1 } else { 1 };
    result
}

fn sub3(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

fn add3(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] + right[0], left[1] + right[1], left[2] + right[2]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_stone_stays_on_full_cube_fast_path() {
        let shape = java_model_shape_for_bedrock_state(&BlockStateQuery::new("minecraft:stone"), 0)
            .expect("Java stone model");
        assert_eq!(shape.cuboids.len(), 1);
        assert!(shape.planes.is_empty());
        assert_eq!(shape.cuboids[0].min, [0.0, 0.0, 0.0]);
        assert_eq!(shape.cuboids[0].max, [1.0, 1.0, 1.0]);
    }

    #[test]
    fn embedded_trapdoor_resolves_bedrock_direction_state() {
        let state = BlockStateQuery::new("minecraft:oak_trapdoor")
            .with_state("direction", 0)
            .with_state("open_bit", false)
            .with_state("upside_down_bit", false);
        let shape = java_model_shape_for_bedrock_state(&state, 0x1234)
            .expect("Java trapdoor model");
        assert!(!shape.is_empty());
        assert!(shape.cuboids.iter().all(|cuboid| {
            cuboid.min != [0.0, 0.0, 0.0] || cuboid.max != [1.0, 1.0, 1.0]
        }));
    }

    #[test]
    fn java_y_rotation_keeps_north_to_east_convention() {
        assert_eq!(rotate_point_y_90([0.5, 0.5, 0.0]), [1.0, 0.5, 0.5]);
        assert_eq!(
            nearest_axis_normal(rotate_vector_axis(
                [0.0, 0.0, -1.0],
                JavaModelAxis::Y,
                90.0_f32.to_radians(),
            )),
            [1, 0, 0],
        );
    }

    #[test]
    fn face_uv_quarter_turn_rotates_corner_assignment() {
        let uv = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        assert_eq!(
            rotate_uv_quarter_turns(uv, 1),
            [[0.0, 1.0], [0.0, 0.0], [1.0, 0.0], [1.0, 1.0]],
        );
    }
}
