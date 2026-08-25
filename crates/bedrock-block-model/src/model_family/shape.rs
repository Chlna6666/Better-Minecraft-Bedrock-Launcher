use std::collections::BTreeMap;

use crate::material::BlockFace;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelShape {
    pub cuboids: Vec<ModelCuboid>,
    pub planes: Vec<ModelPlane>,
}

impl ModelShape {
    #[must_use]
    pub fn from_cuboids(cuboids: impl Into<Vec<ModelCuboid>>) -> Self {
        Self {
            cuboids: cuboids.into(),
            planes: Vec::new(),
        }
    }

    #[must_use]
    pub fn with_planes(mut self, planes: impl Into<Vec<ModelPlane>>) -> Self {
        self.planes = planes.into();
        self
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.cuboids.is_empty() && self.planes.is_empty()
    }

    #[must_use]
    pub fn cull_faces(mut self, hidden_faces: &[BlockFace]) -> Self {
        for cuboid in &mut self.cuboids {
            cuboid.cull_faces(hidden_faces);
        }
        self.planes.retain(|plane| {
            let face = match plane.normal {
                [0, 1, 0] => BlockFace::Up,
                [0, -1, 0] => BlockFace::Down,
                [0, 0, -1] => BlockFace::North,
                [0, 0, 1] => BlockFace::South,
                [1, 0, 0] => BlockFace::East,
                [-1, 0, 0] => BlockFace::West,
                _ => return true,
            };
            !hidden_faces.contains(&face)
        });
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelCuboid {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub material_slot: Option<String>,
    pub face_material_slots: BTreeMap<BlockFace, String>,
    pub face_uvs: BTreeMap<BlockFace, [[f32; 2]; 4]>,
}

impl ModelCuboid {
    #[must_use]
    pub fn new(min: [f32; 3], max: [f32; 3]) -> Self {
        Self {
            min,
            max,
            material_slot: None,
            face_material_slots: BTreeMap::new(),
            face_uvs: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_material_slot(mut self, material_slot: impl Into<String>) -> Self {
        self.material_slot = Some(material_slot.into());
        self
    }

    #[must_use]
    pub fn with_face_material_slot(mut self, face: BlockFace, slot: impl Into<String>) -> Self {
        self.face_material_slots.insert(face, slot.into());
        self
    }

    #[must_use]
    pub fn with_face_uv(mut self, face: BlockFace, uv: [[f32; 2]; 4]) -> Self {
        self.face_uvs.insert(face, uv);
        self
    }

    pub fn cull_faces(&mut self, hidden_faces: &[BlockFace]) {
        for face in hidden_faces {
            let is_on_boundary = match face {
                BlockFace::Up => (self.max[1] - 1.0).abs() < 0.001,
                BlockFace::Down => self.min[1].abs() < 0.001,
                BlockFace::North => self.min[2].abs() < 0.001,
                BlockFace::South => (self.max[2] - 1.0).abs() < 0.001,
                BlockFace::West => self.min[0].abs() < 0.001,
                BlockFace::East => (self.max[0] - 1.0).abs() < 0.001,
                _ => false,
            };
            if is_on_boundary {
                self.face_material_slots.remove(face);
                self.face_uvs.remove(face);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ModelPlaneSidedness {
    /// Thin/decal geometry such as vines and crossed plants is visible from both directions.
    #[default]
    DoubleSided,
    /// One physical surface of a solid or rotated element. Renderers must not synthesize a back face.
    FrontOnly,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelPlane {
    pub corners: [[f32; 3]; 4],
    pub normal: [i32; 3],
    pub material_slot: Option<String>,
    pub uv: Option<[[f32; 2]; 4]>,
    pub sidedness: ModelPlaneSidedness,
}

impl ModelPlane {
    #[must_use]
    pub fn new(corners: [[f32; 3]; 4], normal: [i32; 3]) -> Self {
        Self {
            corners,
            normal,
            material_slot: None,
            uv: None,
            sidedness: ModelPlaneSidedness::DoubleSided,
        }
    }

    /// Marks this quad as one physical face rather than a thin two-sided plane.
    #[must_use]
    pub const fn front_only(mut self) -> Self {
        self.sidedness = ModelPlaneSidedness::FrontOnly;
        self
    }

    #[must_use]
    pub fn with_material_slot(mut self, material_slot: impl Into<String>) -> Self {
        self.material_slot = Some(material_slot.into());
        self
    }

    #[must_use]
    pub fn with_uv(mut self, uv: [[f32; 2]; 4]) -> Self {
        self.uv = Some(uv);
        self
    }
}

#[must_use]
pub fn rect_uv(u0: f32, v0: f32, u1: f32, v1: f32) -> [[f32; 2]; 4] {
    [[u0, v0], [u1, v0], [u1, v1], [u0, v1]]
}

#[must_use]
pub fn uv16(u0: f32, v0: f32, u1: f32, v1: f32) -> [[f32; 2]; 4] {
    rect_uv(u0 / 16.0, v0 / 16.0, u1 / 16.0, v1 / 16.0)
}

#[must_use]
pub fn full_texture_uv() -> [[f32; 2]; 4] {
    uv16(0.0, 0.0, 16.0, 16.0)
}

#[must_use]
pub(crate) fn detail_cuboid_with_local_uv(cuboid: ModelCuboid) -> ModelCuboid {
    let min = cuboid.min;
    let max = cuboid.max;
    cuboid
        .with_face_uv(BlockFace::Up, rect_uv(min[0], min[2], max[0], max[2]))
        .with_face_uv(BlockFace::Down, rect_uv(min[0], min[2], max[0], max[2]))
        .with_face_uv(BlockFace::North, rect_uv(min[0], min[1], max[0], max[1]))
        .with_face_uv(BlockFace::South, rect_uv(min[0], min[1], max[0], max[1]))
        .with_face_uv(BlockFace::West, rect_uv(min[2], min[1], max[2], max[1]))
        .with_face_uv(BlockFace::East, rect_uv(min[2], min[1], max[2], max[1]))
}

#[must_use]
pub(crate) fn projected_cuboid_with_uv(cuboid: ModelCuboid) -> ModelCuboid {
    let min = cuboid.min;
    let max = cuboid.max;
    cuboid
        .with_face_uv(
            BlockFace::West,
            rect_uv(min[2], 1.0 - max[1], max[2], 1.0 - min[1]),
        )
        .with_face_uv(
            BlockFace::East,
            rect_uv(1.0 - max[2], 1.0 - max[1], 1.0 - min[2], 1.0 - min[1]),
        )
        .with_face_uv(
            BlockFace::Down,
            rect_uv(1.0 - max[0], 1.0 - max[2], 1.0 - min[0], 1.0 - min[2]),
        )
        .with_face_uv(
            BlockFace::Up,
            rect_uv(1.0 - max[0], min[2], 1.0 - min[0], max[2]),
        )
        .with_face_uv(
            BlockFace::North,
            rect_uv(min[0], 1.0 - max[1], max[0], 1.0 - min[1]),
        )
        .with_face_uv(
            BlockFace::South,
            rect_uv(1.0 - max[0], 1.0 - max[1], 1.0 - min[0], 1.0 - min[1]),
        )
}

#[must_use]
pub(super) fn jmc_face_uv(face: BlockFace, uv: [[f32; 2]; 4]) -> [[f32; 2]; 4] {
    match face {
        BlockFace::South => uv,
        BlockFace::Up => [uv[3], uv[2], uv[1], uv[0]],
        BlockFace::North | BlockFace::East | BlockFace::West | BlockFace::Down => {
            [uv[1], uv[0], uv[3], uv[2]]
        }
        BlockFace::Side | BlockFace::All | BlockFace::Default => uv,
    }
}

#[must_use]
pub(super) fn apply_jmc_box_uv(
    cuboid: ModelCuboid,
    top_uv: [[f32; 2]; 4],
    side_uv: [[f32; 2]; 4],
) -> ModelCuboid {
    cuboid
        .with_face_uv(BlockFace::Up, jmc_face_uv(BlockFace::Up, top_uv))
        .with_face_uv(BlockFace::Down, jmc_face_uv(BlockFace::Down, top_uv))
        .with_face_uv(BlockFace::North, jmc_face_uv(BlockFace::North, side_uv))
        .with_face_uv(BlockFace::South, jmc_face_uv(BlockFace::South, side_uv))
        .with_face_uv(BlockFace::West, jmc_face_uv(BlockFace::West, side_uv))
        .with_face_uv(BlockFace::East, jmc_face_uv(BlockFace::East, side_uv))
}

#[must_use]
pub fn ground_plane(
    corners: [[f32; 3]; 4],
    material_slot: Option<&str>,
    uv: [[f32; 2]; 4],
) -> ModelPlane {
    let plane = ModelPlane::new(corners, [0, 1, 0]).with_uv(uv);
    if let Some(material_slot) = material_slot {
        plane.with_material_slot(material_slot)
    } else {
        plane
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cull_faces_should_remove_outer_boundary_faces() {
        let cuboid =
            detail_cuboid_with_local_uv(ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]))
                .with_face_material_slot(BlockFace::Up, "up")
                .with_face_material_slot(BlockFace::Down, "down")
                .with_face_material_slot(BlockFace::North, "north");
        let shape =
            ModelShape::from_cuboids([cuboid]).cull_faces(&[BlockFace::North, BlockFace::Up]);

        assert!(
            !shape.cuboids[0]
                .face_material_slots
                .contains_key(&BlockFace::North)
        );
        assert!(
            !shape.cuboids[0]
                .face_material_slots
                .contains_key(&BlockFace::Up)
        );
        assert!(
            shape.cuboids[0]
                .face_material_slots
                .contains_key(&BlockFace::Down)
        );
    }

    #[test]
    fn model_planes_are_double_sided_by_default_but_can_be_front_only() {
        let plane = ModelPlane::new(
            [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]],
            [0, 0, -1],
        );
        assert_eq!(plane.sidedness, ModelPlaneSidedness::DoubleSided);
        assert_eq!(plane.front_only().sidedness, ModelPlaneSidedness::FrontOnly);
    }
}
