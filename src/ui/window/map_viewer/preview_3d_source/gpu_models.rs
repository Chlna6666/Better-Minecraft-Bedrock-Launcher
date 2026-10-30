//! Reusable model templates. Per-block vertices are expanded by the GPU, not this adapter.
use super::*;
use std::{
    hash::{Hash, Hasher},
    ops::Range,
};

#[derive(Clone, Debug, Eq, PartialEq)]
struct QueryKey(BlockStateQuery);
impl Hash for QueryKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.name.hash(state);
        for (name, value) in &self.0.states {
            name.hash(state);
            match value {
                bedrock_block_model::BlockStateValue::Bool(v) => {
                    0u8.hash(state);
                    v.hash(state);
                }
                bedrock_block_model::BlockStateValue::Int(v) => {
                    1u8.hash(state);
                    v.hash(state);
                }
                bedrock_block_model::BlockStateValue::String(v) => {
                    2u8.hash(state);
                    v.hash(state);
                }
            }
        }
    }
}

#[derive(Eq, PartialEq, Hash)]
enum ModelKey {
    Cube(Preview3dMaterialName, [[u32; 4]; 6], bool),
    Detail(
        usize,
        Preview3dMaterialName,
        [u32; 4],
        bool,
        Option<QueryKey>,
    ),
    Fluid(Preview3dMaterialName, [u32; 4], bool, bool),
}

pub(super) struct ModelRecord {
    pub key: BlockKey,
    pub faces: Range<u32>,
    pub height: f32,
    pub mask: u32,
}
pub(super) struct ModelPlan {
    pub faces: Vec<Preview3dStreamFace>,
    pub neighbours: Vec<[i32; 3]>,
    pub records: Vec<ModelRecord>,
    pub min_offset: [i32; 3],
    pub max_offset: [i32; 3],
    pub centers: Vec<(f32, f32)>,
    range_centers: HashMap<(u32, u32), [f32; 2]>,
}

impl ModelPlan {
    pub(super) fn new(builder: &Preview3dMeshBuilder, blocks: &Preview3dChunkBlocks) -> Self {
        let mut plan = Self {
            faces: Vec::new(),
            neighbours: Vec::new(),
            records: Vec::new(),
            min_offset: [-1; 3],
            max_offset: [1; 3],
            centers: Vec::new(),
            range_centers: HashMap::default(),
        };
        let mut models = HashMap::<ModelKey, Range<u32>>::default();
        for (records, glass) in [(&blocks.opaque_blocks, false), (&blocks.glass_blocks, true)] {
            for block in records {
                let colors = FACE_DEFINITIONS
                    .map(|face| block.colors.color_for_normal(face.normal).map(f32::to_bits));
                let key = ModelKey::Cube(block.material.clone(), colors, glass);
                let range = models
                    .entry(key)
                    .or_insert_with(|| {
                        let start = plan.faces.len() as u32;
                        for face in FACE_DEFINITIONS {
                            plan.push(
                                preview_3d_stream_cube_face(
                                    BlockKey { x: 0, y: 0, z: 0 },
                                    face,
                                    block.colors.color_for_normal(face.normal),
                                    preview_3d_material_name_for_face(
                                        block.material.clone(),
                                        face.normal,
                                    ),
                                    if glass {
                                        Preview3dStreamPass::Glass
                                    } else {
                                        Preview3dStreamPass::Opaque
                                    },
                                ),
                                face.normal,
                            );
                        }
                        start..plan.faces.len() as u32
                    })
                    .clone();
                plan.records.push(ModelRecord {
                    key: block.key,
                    faces: range,
                    height: 1.0,
                    mask: if glass { 3 } else { 1 },
                });
            }
        }
        for (records, glass) in [
            (&blocks.detail_blocks, false),
            (&blocks.glass_detail_blocks, true),
        ] {
            for block in records {
                let query = builder.preview_3d_detail_shape_query(block);
                let key = ModelKey::Detail(
                    Arc::as_ptr(&block.shape) as usize,
                    block.material.clone(),
                    block.color.map(f32::to_bits),
                    glass,
                    query.clone().map(QueryKey),
                );
                let range = models
                    .entry(key)
                    .or_insert_with(|| {
                        let start = plan.faces.len() as u32;
                        let inferred = query
                            .as_ref()
                            .and_then(model_shape_for_block_state)
                            .map(preview_3d_detail_shape_from_model_shape);
                        let mut prototype = block.clone();
                        prototype.key = BlockKey { x: 0, y: 0, z: 0 };
                        let mut faces = Vec::new();
                        preview_3d_push_detail_block_faces(
                            &prototype,
                            inferred.as_ref().unwrap_or(&block.shape),
                            &mut faces,
                        );
                        let unshaded = block.normalized_name.ends_with("redstone_wire")
                            || is_preview_3d_unshaded_detail(&block.normalized_name);
                        for mut face in faces {
                            if unshaded {
                                face.shade = 1.0;
                            }
                            let mut neighbour = [0; 3];
                            if face.normal != [0, 0, 0] {
                                for axis in 0..3 {
                                    let min = face
                                        .corners
                                        .iter()
                                        .map(|p| p[axis])
                                        .fold(f64::INFINITY, f64::min);
                                    neighbour[axis] =
                                        (min - f64::from(face.normal[axis]) * 0.5).floor() as i32
                                            + face.normal[axis];
                                }
                            }
                            plan.push(
                                preview_3d_stream_face_from_preview(
                                    face,
                                    if glass {
                                        Preview3dStreamPass::DetailGlass
                                    } else {
                                        Preview3dStreamPass::DetailOpaque
                                    },
                                ),
                                neighbour,
                            );
                        }
                        start..plan.faces.len() as u32
                    })
                    .clone();
                if !range.is_empty() {
                    plan.records.push(ModelRecord {
                        key: block.key,
                        faces: range,
                        height: 1.0,
                        mask: 1,
                    });
                }
            }
        }
        for (records, water) in [(&blocks.water_blocks, true), (&blocks.lava_blocks, false)] {
            for block in records {
                let key = ModelKey::Fluid(
                    block.material.clone(),
                    block.color.map(f32::to_bits),
                    block.surface_only,
                    water,
                );
                let range = models
                    .entry(key)
                    .or_insert_with(|| {
                        let start = plan.faces.len() as u32;
                        for face in FACE_DEFINITIONS {
                            if block.surface_only && face.normal != [0, 1, 0] {
                                continue;
                            }
                            plan.push(
                                preview_3d_stream_fluid_face(
                                    BlockKey { x: 0, y: 0, z: 0 },
                                    face,
                                    block.color,
                                    preview_3d_fluid_material(block, face.normal),
                                    if water {
                                        Preview3dStreamPass::Water
                                    } else {
                                        Preview3dStreamPass::Lava
                                    },
                                    1.0,
                                ),
                                face.normal,
                            );
                        }
                        start..plan.faces.len() as u32
                    })
                    .clone();
                plan.records.push(ModelRecord {
                    key: block.key,
                    faces: range,
                    height: block.height.clamp(0.05, 1.0),
                    mask: if water { 5 } else { 9 },
                });
            }
        }
        for record in &plan.records {
            plan.range_centers
                .entry((record.faces.start, record.faces.end))
                .or_insert_with(|| {
                    plan.centers[record.faces.start as usize..record.faces.end as usize]
                        .iter()
                        .fold([f32::MAX, -f32::MAX], |[min, max], &(a, b)| {
                            [min.min(a), max.max(b)]
                        })
                });
        }
        plan
    }
    fn push(&mut self, face: Preview3dStreamFace, neighbour: [i32; 3]) {
        for axis in 0..3 {
            self.min_offset[axis] = self.min_offset[axis].min(neighbour[axis]);
            self.max_offset[axis] = self.max_offset[axis].max(neighbour[axis]);
        }
        let center = face.corners.iter().map(|p| p[1] as f32).sum::<f32>() * 0.25;
        self.faces.push(face);
        self.neighbours.push(neighbour);
        self.centers.push((center, center));
    }
    pub(super) fn band_range(&self, record: &ModelRecord) -> (i32, i32) {
        let [min, max] = self.range_centers[&(record.faces.start, record.faces.end)];
        (
            ((record.key.y as f32 + min * record.height) / 64.0).floor() as i32,
            ((record.key.y as f32 + max * record.height) / 64.0).floor() as i32,
        )
    }
}
