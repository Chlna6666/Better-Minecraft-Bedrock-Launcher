use super::*;

#[derive(Clone, Debug)]
pub(super) struct DetailBlock {
    pub(super) key: BlockKey,
    pub(super) normalized_name: Arc<str>,
    pub(super) neighbor_model: NeighborModel,
    pub(super) stair_state: Option<StairState>,
    pub(super) shape: Arc<DetailShape>,
    pub(super) color: [f32; 4],
    pub(super) material: MaterialName,
}

#[derive(Clone, Debug, Default)]
pub(super) struct DetailShape {
    pub(super) cuboids: Vec<Cuboid>,
    pub(super) planes: Vec<Plane>,
}

impl DetailShape {
    #[cfg(test)]
    pub(super) fn from_cuboids(cuboids: impl Into<Vec<Cuboid>>) -> Self {
        Self {
            cuboids: cuboids.into(),
            planes: Vec::new(),
        }
    }


    pub(super) const fn is_empty(&self) -> bool {
        self.cuboids.is_empty() && self.planes.is_empty()
    }
}

#[derive(Clone, Debug)]
pub(super) struct Cuboid {
    pub(super) min: [f32; 3],
    pub(super) max: [f32; 3],
    pub(super) material_slot: Option<MaterialSlot>,
    pub(super) face_material_slots: BTreeMap<BlockFace, MaterialSlot>,
    pub(super) face_uvs: BTreeMap<BlockFace, [[f32; 2]; 4]>,
}

impl Cuboid {
    #[cfg(test)]
    pub(super) fn new(min: [f32; 3], max: [f32; 3]) -> Self {
        Self {
            min,
            max,
            material_slot: None,
            face_material_slots: BTreeMap::new(),
            face_uvs: BTreeMap::new(),
        }
    }


    pub(super) fn face_material_slot(&self, normal: [i32; 3]) -> Option<MaterialSlot> {
        let face = obj::block_face_from_normal(normal);
        self.face_material_slots
            .get(&face)
            .or_else(|| {
                if matches!(
                    face,
                    BlockFace::North | BlockFace::South | BlockFace::East | BlockFace::West
                ) {
                    self.face_material_slots.get(&BlockFace::Side)
                } else {
                    None
                }
            })
            .cloned()
            .or_else(|| self.material_slot.clone())
    }


    pub(super) fn face_uv(&self, normal: [i32; 3]) -> Option<[[f32; 2]; 4]> {
        let face = obj::block_face_from_normal(normal);
        self.face_uvs
            .get(&face)
            .or_else(|| {
                if matches!(
                    face,
                    BlockFace::North | BlockFace::South | BlockFace::East | BlockFace::West
                ) {
                    self.face_uvs.get(&BlockFace::Side)
                } else {
                    None
                }
            })
            .copied()
    }

}

pub(super) fn rect_uv(u0: f32, v0: f32, u1: f32, v1: f32) -> [[f32; 2]; 4] {
    [[u0, v0], [u1, v0], [u1, v1], [u0, v1]]
}

pub(super) fn uv16(u0: f32, v0: f32, u1: f32, v1: f32) -> [[f32; 2]; 4] {
    rect_uv(u0 / 16.0, v0 / 16.0, u1 / 16.0, v1 / 16.0)
}

pub(super) fn full_texture_uv() -> [[f32; 2]; 4] {
    uv16(0.0, 0.0, 16.0, 16.0)
}

#[derive(Clone, Debug)]
pub(super) struct Plane {
    pub(super) corners: [[f32; 3]; 4],
    pub(super) normal: [i32; 3],
    pub(super) material_slot: Option<MaterialSlot>,
    pub(super) uv: Option<[[f32; 2]; 4]>,
    /// Front-only planes (the glass pane cards) must not receive the reversed back
    /// copy: on the blended glass passes the two coplanar quads stack their alpha and
    /// turn a single vanilla quad nearly opaque.
    pub(super) front_only: bool,
}

pub(super) fn detail_shape_from_model_ref(shape: &ModelShape) -> DetailShape {
    DetailShape {
        cuboids: shape
            .cuboids
            .iter()
            .cloned()
            .map(cuboid_from_model)
            .collect(),
        planes: shape.planes.iter().cloned().map(plane_from_model).collect(),
    }
}

pub(super) fn detail_shape_from_model(shape: ModelShape) -> DetailShape {
    DetailShape {
        cuboids: shape.cuboids.into_iter().map(cuboid_from_model).collect(),
        planes: shape.planes.into_iter().map(plane_from_model).collect(),
    }
}

pub(super) fn cuboid_from_model(cuboid: ModelCuboid) -> Cuboid {
    // Bedrock gives most partial blocks a dedicated 16x16 texture per face, so a cuboid with no
    // authored UVs must sample the whole tile. Projecting by block bounds instead cropped the
    // art to the element's slice, which is why thin models (torches, buttons, doors) showed an
    // arbitrary strip. Shapes with unfolded art set their own per-face UVs explicitly.
    let cuboid = if cuboid.face_uvs.is_empty() {
        let uv = full_texture_uv();
        let mut cuboid = cuboid;
        for face in [
            BlockFace::Up,
            BlockFace::Down,
            BlockFace::North,
            BlockFace::South,
            BlockFace::East,
            BlockFace::West,
        ] {
            cuboid = cuboid.with_face_uv(face, uv);
        }
        cuboid
    } else {
        cuboid
    };
    Cuboid {
        min: cuboid.min,
        max: cuboid.max,
        material_slot: cuboid
            .material_slot
            .and_then(|slot| material_slot_from_value(&slot)),
        face_material_slots: cuboid
            .face_material_slots
            .into_iter()
            .filter_map(|(face, slot)| material_slot_from_value(&slot).map(|slot| (face, slot)))
            .collect(),
        face_uvs: cuboid.face_uvs,
    }
}

pub(super) fn plane_from_model(plane: ModelPlane) -> Plane {
    Plane {
        corners: plane.corners,
        normal: plane.normal,
        material_slot: plane
            .material_slot
            .and_then(|slot| material_slot_from_value(&slot)),
        uv: plane.uv,
        front_only: matches!(plane.sidedness, ModelPlaneSidedness::FrontOnly),
    }
}
