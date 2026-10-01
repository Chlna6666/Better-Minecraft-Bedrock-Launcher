use std::collections::BTreeMap;

use crate::geometry::{BlockGeometry, GeometryCube};
use crate::material::BlockFace;
use crate::model_family::{ModelCuboid, ModelPlane, ModelShape};

/// Converts parsed Bedrock block geometry into a renderer-neutral model shape.
///
/// Cube coordinates use Bedrock's 16-unit block space, and UVs keep the current 64-unit
/// texture-span interpretation. Cubes without an origin or size are skipped. The function does
/// not resolve textures, read world data, or create renderer resources.
#[must_use]
pub fn shape_from_geometry(geometry: &BlockGeometry) -> Option<ModelShape> {
    let mut shape = ModelShape::default();
    for bone in &geometry.bones {
        for cube in &bone.cubes {
            push_cube_shape(bone.pivot, bone.rotation, cube, &mut shape);
        }
    }
    (!shape.is_empty()).then_some(shape)
}

fn push_cube_shape(
    bone_pivot: Option<[f32; 3]>,
    bone_rotation: Option<[f32; 3]>,
    cube: &GeometryCube,
    shape: &mut ModelShape,
) {
    let (Some(origin), Some(size)) = (cube.origin, cube.size) else {
        return;
    };
    let raw_min = geometry_point_to_block(origin);
    let raw_max = geometry_point_to_block([
        origin[0] + size[0],
        origin[1] + size[1],
        origin[2] + size[2],
    ]);
    let min = std::array::from_fn(|axis| raw_min[axis].min(raw_max[axis]));
    let max = std::array::from_fn(|axis| raw_min[axis].max(raw_max[axis]));
    let mut cuboid = ModelCuboid::new(min, max);
    cuboid.material_slot = material_slot(cube.material_instance.as_deref());
    cuboid.face_material_slots = face_material_slots(cube);
    if let Some(face_uvs) = cube_face_uvs(cube, size) {
        cuboid.face_uvs = face_uvs;
    }

    let rotation = cube.rotation.or(bone_rotation).unwrap_or([0.0, 0.0, 0.0]);
    if rotation_is_zero(rotation) {
        shape.cuboids.push(cuboid);
        return;
    }
    let pivot = cube.pivot.or(bone_pivot).unwrap_or([0.0, 8.0, 0.0]);
    shape.planes.extend(rotated_cuboid_planes(
        cuboid,
        geometry_point_to_block(pivot),
        rotation,
    ));
}

fn geometry_point_to_block(point: [f32; 3]) -> [f32; 3] {
    [
        (point[0] + 8.0) / 16.0,
        point[1] / 16.0,
        (point[2] + 8.0) / 16.0,
    ]
}

fn rotation_is_zero(rotation: [f32; 3]) -> bool {
    rotation.iter().all(|value| value.abs() < 0.001)
}

fn rotated_cuboid_planes(
    cuboid: ModelCuboid,
    pivot: [f32; 3],
    rotation_degrees: [f32; 3],
) -> Vec<ModelPlane> {
    let [x0, y0, z0] = cuboid.min;
    let [x1, y1, z1] = cuboid.max;
    let points = [
        [x0, y0, z0],
        [x1, y0, z0],
        [x1, y1, z0],
        [x0, y1, z0],
        [x0, y0, z1],
        [x1, y0, z1],
        [x1, y1, z1],
        [x0, y1, z1],
    ]
    .map(|point| rotate_point(point, pivot, rotation_degrees));
    let plane_indices = [
        ([0, 1, 2, 3], [0, 0, -1]),
        ([5, 4, 7, 6], [0, 0, 1]),
        ([4, 0, 3, 7], [-1, 0, 0]),
        ([1, 5, 6, 2], [1, 0, 0]),
        ([3, 2, 6, 7], [0, 1, 0]),
        ([4, 5, 1, 0], [0, -1, 0]),
    ];
    plane_indices
        .into_iter()
        .map(|(indices, normal)| {
            let rotated_normal = rotated_axis_normal(normal, rotation_degrees);
            let mut plane = ModelPlane::new(indices.map(|index| points[index]), rotated_normal);
            if let Some(slot) = material_slot_for_normal(&cuboid, normal) {
                plane = plane.with_material_slot(slot);
            }
            if let Some(uv) = face_uv_for_normal(&cuboid, normal) {
                plane = plane.with_uv(uv);
            }
            plane
        })
        .collect()
}

fn face_material_slots(cube: &GeometryCube) -> BTreeMap<BlockFace, String> {
    cube.face_material_instances
        .iter()
        .filter_map(|(face, slot)| material_slot(Some(slot)).map(|slot| (*face, slot)))
        .collect()
}

fn material_slot(slot: Option<&str>) -> Option<String> {
    slot.map(str::trim)
        .filter(|slot| !slot.is_empty())
        .map(ToOwned::to_owned)
}

fn cube_face_uvs(
    cube: &GeometryCube,
    size: [f32; 3],
) -> Option<BTreeMap<BlockFace, [[f32; 2]; 4]>> {
    let raw = cube.uv.as_ref()?.raw.as_array()?;
    let u = number_to_f32(raw.first()?)?;
    let v = number_to_f32(raw.get(1)?)?;
    let width = size[0].abs();
    let height = size[1].abs();
    let depth = size[2].abs();
    let texture_span = 64.0_f32;
    let face_bounds = [
        (BlockFace::Up, [u + depth, v, u + depth + width, v + depth]),
        (
            BlockFace::Down,
            [u + depth + width, v, u + depth + width + width, v + depth],
        ),
        (
            BlockFace::North,
            [u + depth, v + depth, u + depth + width, v + depth + height],
        ),
        (
            BlockFace::South,
            [
                u + depth + width + depth,
                v + depth,
                u + depth + width + depth + width,
                v + depth + height,
            ],
        ),
        (
            BlockFace::West,
            [u, v + depth, u + depth, v + depth + height],
        ),
        (
            BlockFace::East,
            [
                u + depth + width,
                v + depth,
                u + depth + width + depth,
                v + depth + height,
            ],
        ),
    ];
    Some(
        face_bounds
            .into_iter()
            .map(|(face, bounds)| (face, uv_pixels(texture_span, bounds)))
            .collect(),
    )
}

fn number_to_f32(value: &serde_json::Value) -> Option<f32> {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "Bedrock geometry UV coordinates are authored as JSON numbers and consumed as renderer f32 values."
    )]
    value.as_f64().map(|number| number as f32)
}

fn uv_pixels(texture_span: f32, [u0, v0, u1, v1]: [f32; 4]) -> [[f32; 2]; 4] {
    [
        [u0 / texture_span, v0 / texture_span],
        [u1 / texture_span, v0 / texture_span],
        [u1 / texture_span, v1 / texture_span],
        [u0 / texture_span, v1 / texture_span],
    ]
}

fn material_slot_for_normal(cuboid: &ModelCuboid, normal: [i32; 3]) -> Option<String> {
    let face = crate::obj::block_face_for_normal(normal);
    cuboid
        .face_material_slots
        .get(&face)
        .or_else(|| {
            matches!(
                face,
                BlockFace::North | BlockFace::South | BlockFace::East | BlockFace::West
            )
            .then(|| cuboid.face_material_slots.get(&BlockFace::Side))
            .flatten()
        })
        .cloned()
        .or_else(|| cuboid.material_slot.clone())
}

fn face_uv_for_normal(cuboid: &ModelCuboid, normal: [i32; 3]) -> Option<[[f32; 2]; 4]> {
    let face = crate::obj::block_face_for_normal(normal);
    cuboid
        .face_uvs
        .get(&face)
        .or_else(|| {
            matches!(
                face,
                BlockFace::North | BlockFace::South | BlockFace::East | BlockFace::West
            )
            .then(|| cuboid.face_uvs.get(&BlockFace::Side))
            .flatten()
        })
        .copied()
}

fn rotated_axis_normal(normal: [i32; 3], rotation_degrees: [f32; 3]) -> [i32; 3] {
    #[expect(
        clippy::cast_precision_loss,
        reason = "Block face normals contain only -1, 0, and 1 components."
    )]
    let rotated = rotate_point(
        [normal[0] as f32, normal[1] as f32, normal[2] as f32],
        [0.0, 0.0, 0.0],
        rotation_degrees,
    );
    nearest_axis_normal(rotated)
}

fn nearest_axis_normal(normal: [f32; 3]) -> [i32; 3] {
    let axis = (0..3)
        .max_by(|left, right| {
            normal[*left]
                .abs()
                .partial_cmp(&normal[*right].abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(1);
    let mut result = [0, 0, 0];
    result[axis] = if normal[axis].is_sign_negative() {
        -1
    } else {
        1
    };
    result
}

fn rotate_point(point: [f32; 3], pivot: [f32; 3], rotation_degrees: [f32; 3]) -> [f32; 3] {
    let mut point = [
        point[0] - pivot[0],
        point[1] - pivot[1],
        point[2] - pivot[2],
    ];
    for (axis, degrees) in rotation_degrees.into_iter().enumerate() {
        point = rotate_point_axis(point, axis, degrees.to_radians());
    }
    [
        point[0] + pivot[0],
        point[1] + pivot[1],
        point[2] + pivot[2],
    ]
}

fn rotate_point_axis(point: [f32; 3], axis: usize, angle: f32) -> [f32; 3] {
    if angle.abs() < 0.0001 {
        return point;
    }
    let (sin, cos) = angle.sin_cos();
    match axis {
        0 => [
            point[0],
            point[1] * cos - point[2] * sin,
            point[1] * sin + point[2] * cos,
        ],
        1 => [
            point[0] * cos + point[2] * sin,
            point[1],
            -point[0] * sin + point[2] * cos,
        ],
        2 => [
            point[0] * cos - point[1] * sin,
            point[0] * sin + point[1] * cos,
            point[2],
        ],
        _ => point,
    }
}
