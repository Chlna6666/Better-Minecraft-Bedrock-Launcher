use super::*;

#[derive(Clone, Debug)]
pub(super) struct Face {
    pub(super) corners: [[f64; 3]; 4],
    pub(super) color: [f32; 4],
    pub(super) shade: f32,
    pub(super) normal: [i32; 3],
    pub(super) material: MaterialName,
    pub(super) uv: Option<[[f32; 2]; 4]>,
}

#[derive(Clone, Copy)]
pub(super) struct FaceDefinition {
    pub(super) normal: [i32; 3],
    #[cfg(test)]
    pub(super) corners: [[f32; 3]; 4],
    pub(super) shade: f32,
}

impl FaceDefinition {
    pub(super) fn axis(self) -> Option<usize> {
        self.normal.iter().position(|normal| *normal != 0)
    }
}

pub(super) const FACE_DEFINITIONS: [FaceDefinition; 6] = [
    FaceDefinition {
        normal: [0, 1, 0],
        #[cfg(test)]
        corners: [
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
            [1.0, 1.0, 1.0],
            [0.0, 1.0, 1.0],
        ],
        shade: 1.0,
    },
    FaceDefinition {
        normal: [0, -1, 0],
        #[cfg(test)]
        corners: [
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 0.0],
        ],
        shade: 0.42,
    },
    FaceDefinition {
        normal: [1, 0, 0],
        #[cfg(test)]
        corners: [
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
            [1.0, 1.0, 0.0],
        ],
        shade: 0.70,
    },
    FaceDefinition {
        normal: [-1, 0, 0],
        #[cfg(test)]
        corners: [
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 1.0, 1.0],
        ],
        shade: 0.62,
    },
    FaceDefinition {
        normal: [0, 0, 1],
        #[cfg(test)]
        corners: [
            [1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [0.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
        ],
        shade: 0.56,
    },
    FaceDefinition {
        normal: [0, 0, -1],
        #[cfg(test)]
        corners: [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ],
        shade: 0.78,
    },
];

#[cfg(test)]
pub(super) fn block_face(block: BlockKey, face: FaceDefinition, color: [f32; 4]) -> Face {
    Face {
        corners: face.corners.map(|corner| {
            [
                f64::from(block.x) + f64::from(corner[0]),
                f64::from(block.y) + f64::from(corner[1]),
                f64::from(block.z) + f64::from(corner[2]),
            ]
        }),
        color,
        shade: face.shade,
        normal: face.normal,
        material: Arc::from("minecraft_test"),
        uv: None,
    }
}

pub(super) fn push_detail_block_faces(
    block: &DetailBlock,
    origin: BlockKey,
    shape: &DetailShape,
    faces: &mut Vec<Face>,
) {
    for cuboid in &shape.cuboids {
        push_cuboid_faces(origin, cuboid, block.color, block.material.clone(), faces);
    }
    for plane in &shape.planes {
        push_plane_face(origin, plane, block.color, block.material.clone(), faces);
        push_plane_face(
            origin,
            &Plane {
                corners: [
                    plane.corners[3],
                    plane.corners[2],
                    plane.corners[1],
                    plane.corners[0],
                ],
                normal: [-plane.normal[0], -plane.normal[1], -plane.normal[2]],
                material_slot: plane.material_slot.clone(),
                uv: plane.uv.map(|[a, b, c, d]| [d, c, b, a]),
            },
            block.color,
            block.material.clone(),
            faces,
        );
    }
}

pub(super) fn push_cuboid_faces(
    block: BlockKey,
    cuboid: &Cuboid,
    color: [f32; 4],
    material: MaterialName,
    faces: &mut Vec<Face>,
) {
    if cuboid.min[0] >= cuboid.max[0]
        || cuboid.min[1] >= cuboid.max[1]
        || cuboid.min[2] >= cuboid.max[2]
    {
        return;
    }
    let [x0, y0, z0] = cuboid.min;
    let [x1, y1, z1] = cuboid.max;
    let planes = [
        Plane {
            corners: [[x0, y1, z0], [x1, y1, z0], [x1, y1, z1], [x0, y1, z1]],
            normal: [0, 1, 0],
            material_slot: cuboid.face_material_slot([0, 1, 0]),
            uv: cuboid.face_uv([0, 1, 0]),
        },
        Plane {
            corners: [[x0, y0, z1], [x1, y0, z1], [x1, y0, z0], [x0, y0, z0]],
            normal: [0, -1, 0],
            material_slot: cuboid.face_material_slot([0, -1, 0]),
            uv: cuboid.face_uv([0, -1, 0]),
        },
        Plane {
            corners: [[x1, y0, z0], [x1, y0, z1], [x1, y1, z1], [x1, y1, z0]],
            normal: [1, 0, 0],
            material_slot: cuboid.face_material_slot([1, 0, 0]),
            uv: cuboid.face_uv([1, 0, 0]),
        },
        Plane {
            corners: [[x0, y0, z1], [x0, y0, z0], [x0, y1, z0], [x0, y1, z1]],
            normal: [-1, 0, 0],
            material_slot: cuboid.face_material_slot([-1, 0, 0]),
            uv: cuboid.face_uv([-1, 0, 0]),
        },
        Plane {
            corners: [[x1, y0, z1], [x0, y0, z1], [x0, y1, z1], [x1, y1, z1]],
            normal: [0, 0, 1],
            material_slot: cuboid.face_material_slot([0, 0, 1]),
            uv: cuboid.face_uv([0, 0, 1]),
        },
        Plane {
            corners: [[x0, y0, z0], [x1, y0, z0], [x1, y1, z0], [x0, y1, z0]],
            normal: [0, 0, -1],
            material_slot: cuboid.face_material_slot([0, 0, -1]),
            uv: cuboid.face_uv([0, 0, -1]),
        },
    ];
    for plane in planes {
        push_plane_face(block, &plane, color, material.clone(), faces);
    }
}

pub(super) fn push_plane_face(
    block: BlockKey,
    plane: &Plane,
    color: [f32; 4],
    material: MaterialName,
    faces: &mut Vec<Face>,
) {
    faces.push(Face {
        corners: plane.corners.map(|corner| {
            [
                f64::from(block.x) + f64::from(corner[0]),
                f64::from(block.y) + f64::from(corner[1]),
                f64::from(block.z) + f64::from(corner[2]),
            ]
        }),
        color,
        shade: normal_shade(plane.normal),
        normal: plane.normal,
        material: plane_material_name(material, plane),
        uv: plane.uv,
    });
}

pub(super) fn normal_shade(normal: [i32; 3]) -> f32 {
    FACE_DEFINITIONS
        .iter()
        .find(|face| face.normal == normal)
        .map_or(0.72, |face| face.shade)
}
pub(super) fn stream_face_from_preview(face: Face, pass: StreamPass) -> StreamFace {
    StreamFace {
        corners: face.corners,
        color: face.color,
        shade: face.shade,
        ambient_light: [1.0; 4],
        normal: face.normal,
        material: face.material,
        uv: face.uv,
        pass,
    }
}

pub(super) fn stream_cube_face(
    block: BlockKey,
    face: FaceDefinition,
    color: [f32; 4],
    material: MaterialName,
    pass: StreamPass,
) -> StreamFace {
    let corners = stream_face_corners(face.normal).map(|corner| {
        [
            f64::from(block.x) + f64::from(corner[0]),
            f64::from(block.y) + f64::from(corner[1]),
            f64::from(block.z) + f64::from(corner[2]),
        ]
    });
    StreamFace {
        corners,
        color,
        shade: face.shade,
        ambient_light: [1.0; 4],
        normal: face.normal,
        material,
        uv: Some(cube_face_uv(face.normal)),
        pass,
    }
}

/// Approximate fluid surface height from the persisted Bedrock liquid depth.
///
/// Bedrock encodes sources as depth 0 and flowing blocks as 1..=15. Levels 1..=7 follow the Java
/// (8 - level) / 9 surface curve; 8..=15 are falling columns and stay full height.
/// Fluid side faces use the flowing texture; top and bottom use the still texture.
///
/// Bedrock keeps separate still/flow images (water_still/water_flow, lava_still/lava_flow); the
pub(super) fn stream_face_corners(normal: [i32; 3]) -> [[f32; 3]; 4] {
    match normal {
        [0, 1, 0] => [
            [0.0, 1.0, 0.0],
            [1.0, 1.0, 0.0],
            [1.0, 1.0, 1.0],
            [0.0, 1.0, 1.0],
        ],
        [0, -1, 0] => [
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 0.0],
        ],
        [1, 0, 0] => [
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
            [1.0, 1.0, 0.0],
        ],
        [-1, 0, 0] => [
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 1.0, 1.0],
        ],
        [0, 0, 1] => [
            [1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [0.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
        ],
        _ => [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ],
    }
}
