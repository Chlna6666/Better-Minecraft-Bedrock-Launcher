//! Incremental faces for selected chunks and section bands.

use super::*;

/// A material-tagged quad in world coordinates.
#[derive(Clone, Debug)]
pub struct StreamFace {
    pub corners: [[f64; 3]; 4],
    pub color: [f32; 4],
    pub shade: f32,
    pub ambient_light: [f32; 4],
    pub normal: [i32; 3],
    pub material: Arc<str>,
    pub uv: Option<[[f32; 2]; 4]>,
    pub pass: StreamPass,
}

/// Render class of one streamed face.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamPass {
    Opaque,
    DetailOpaque,
    Glass,
    DetailGlass,
    Water,
    Lava,
}

/// Per-chunk face batch used by the streaming world renderer.
#[derive(Clone, Debug)]
pub struct ChunkFaces {
    pub pos: ChunkPos,
    pub faces: Vec<StreamFace>,
}

/// A cube band can be skipped only when every band its faces can occupy is retained.
pub struct BandSelection {
    /// Inclusive band ranges whose geometry must be rebuilt.
    pub affected: Vec<(i32, i32)>,
    /// Band indices for which previous geometry can be reused.
    pub retained: std::collections::HashSet<i32>,
    /// Positive band height in blocks.
    pub band_height: i32,
}

/// Optional per-chunk masks for selecting which faces to build.
#[derive(Clone, Copy, Default)]
pub struct StreamFaceOptions<'a> {
    pub selections: Option<&'a FxHashMap<ChunkPos, BandSelection>>,
    pub gpu_cubes: Option<&'a FxHashSet<ChunkPos>>,
    pub hidden_bands: Option<&'a BTreeSet<(ChunkPos, i32)>>,
}

impl BandSelection {
    pub(super) fn includes_cube(&self, y: i32) -> bool {
        [
            y.div_euclid(self.band_height),
            (y + 1).div_euclid(self.band_height),
        ]
        .into_iter()
        .any(|band| {
            !self.retained.contains(&band)
                || self
                    .affected
                    .iter()
                    .any(|&(min, max)| (min..=max).contains(&band))
        })
    }
}
impl SceneBuilder {
    /// Returns the world-height range for a fully collected streaming column.
    pub fn stream_height_range(&self, pos: ChunkPos) -> Option<(i32, i32)> {
        let blocks = self.block_chunks.get(&ChunkKey::from_pos(pos))?;
        (blocks.min_y <= blocks.max_y).then_some((i32::from(blocks.min_y), i32::from(blocks.max_y)))
    }

    /// Only full opaque cubes have a bounded one-cell cross-column dependency here.
    pub fn stream_is_plain_opaque(&self, pos: ChunkPos) -> bool {
        self.block_chunks
            .get(&ChunkKey::from_pos(pos))
            .is_some_and(|blocks| {
                blocks.glass.is_empty()
                    && blocks.water.is_empty()
                    && blocks.lava.is_empty()
                    && blocks.detail_blocks.is_empty()
                    && blocks.glass_detail_blocks.is_empty()
            })
    }

    /// Creates a builder for incremental per-chunk streaming face extraction.
    pub fn new_stream(bounds: SlimeChunkBounds) -> Self {
        let mut builder = Self::new(bounds, 0);
        builder.mask_unprocessed_neighbors = false;
        builder
    }

    /// Inserts collected block records at their source chunk position.
    ///
    /// Supply positions from the builder's dimension.
    pub fn insert_stream_chunk(&mut self, pos: ChunkPos, mut blocks: ChunkBlocks) {
        blocks.rebuild_neighbor_indexes();
        let key = ChunkKey::from_pos(pos);
        self.processed_chunk_keys.insert(key);
        self.processed_chunk_count = self.processed_chunk_count.saturating_add(1);
        self.block_chunks.insert(key, blocks);
    }

    /// Provides a cached neighbor for cross-column culling without restoring its full blocks.
    ///
    /// Supply positions from the builder's dimension.
    pub fn insert_stream_boundary(&mut self, pos: ChunkPos, boundary: Arc<ChunkBoundary>) {
        let key = ChunkKey::from_pos(pos);
        self.processed_chunk_keys.insert(key);
        self.processed_chunk_count = self.processed_chunk_count.saturating_add(1);
        self.boundary_chunks.insert(key, boundary);
    }

    /// Removes and returns every cached chunk block set, preserving them for later batches.
    pub fn take_stream_chunks(&mut self) -> Vec<(ChunkPos, ChunkBlocks)> {
        let dimension = self.bounds.dimension;
        let chunks = std::mem::take(&mut self.block_chunks);
        self.boundary_chunks.clear();
        self.processed_chunk_keys.clear();
        self.processed_chunk_count = 0;
        chunks
            .into_iter()
            .map(|(key, blocks)| {
                (
                    ChunkPos {
                        x: key.x,
                        z: key.z,
                        dimension,
                    },
                    blocks,
                )
            })
            .collect()
    }

    /// Emits textured faces using optional band selection, GPU cube, and hidden-band data.
    /// Certified hidden sections are skipped before CPU face expansion while adjacent block data
    /// remains available for shared-face and ambient-occlusion queries.
    pub fn build_stream_faces(
        &self,
        targets: &[ChunkPos],
        options: StreamFaceOptions<'_>,
    ) -> Vec<ChunkFaces> {
        let dimension = self.bounds.dimension;
        let empty_hidden_bands = BTreeSet::new();
        let hidden = options.hidden_bands.unwrap_or(&empty_hidden_bands);
        targets
            .par_iter()
            .filter_map(|pos| {
                let blocks = self.block_chunks.get(&ChunkKey::from_pos(*pos))?;
                if options
                    .gpu_cubes
                    .is_some_and(|gpu_cubes| gpu_cubes.contains(pos))
                {
                    return None;
                }
                let mut faces = Vec::new();
                self.push_stream_cube_faces(
                    blocks,
                    &mut faces,
                    options
                        .selections
                        .and_then(|selections| selections.get(pos)),
                    true,
                    *pos,
                    hidden,
                );
                self.push_stream_detail_faces(blocks, &mut faces, *pos, hidden);
                self.push_stream_fluid_faces(blocks, &mut faces, *pos, hidden);
                (!faces.is_empty()).then_some(ChunkFaces {
                    pos: ChunkPos {
                        x: pos.x,
                        z: pos.z,
                        dimension,
                    },
                    faces,
                })
            })
            .collect()
    }
    pub(super) fn push_stream_cube_faces(
        &self,
        blocks: &ChunkBlocks,
        faces: &mut Vec<StreamFace>,
        selection: Option<&BandSelection>,
        emit_opaque: bool,
        pos: ChunkPos,
        hidden: &BTreeSet<(ChunkPos, i32)>,
    ) {
        let opaque = if emit_opaque {
            blocks.opaque_blocks.as_slice()
        } else {
            &[]
        };
        for block in opaque {
            if section_is_hidden_with_face_halo(hidden, pos, block.key.y)
                || selection.is_some_and(|selection| !selection.includes_cube(block.key.y))
            {
                continue;
            }
            for face in FACE_DEFINITIONS {
                if self.is_opaque_neighbor(block.key, face) {
                    continue;
                }
                let mut emitted = stream_cube_face(
                    block.key,
                    face,
                    block.colors.color_from_normal(face.normal),
                    face_material_name(block.material.clone(), face.normal),
                    StreamPass::Opaque,
                );
                emitted.ambient_light = self.stream_ambient_light(block.key, face.normal);
                faces.push(emitted);
            }
        }
        for block in &blocks.glass_blocks {
            if section_is_hidden_with_face_halo(hidden, pos, block.key.y) {
                continue;
            }
            for face in FACE_DEFINITIONS {
                if self.is_glass_neighbor(block.key, face) {
                    continue;
                }
                faces.push(stream_cube_face(
                    block.key,
                    face,
                    block.colors.color_from_normal(face.normal),
                    face_material_name(block.material.clone(), face.normal),
                    StreamPass::Glass,
                ));
            }
        }
    }

    pub(super) fn push_stream_detail_faces(
        &self,
        blocks: &ChunkBlocks,
        output: &mut Vec<StreamFace>,
        pos: ChunkPos,
        hidden: &BTreeSet<(ChunkPos, i32)>,
    ) {
        let mut faces = Vec::new();
        // Neighbor-derived models (fences, panes, walls, stairs, redstone)
        // have few distinct permutations compared with their block count.
        // Resolve each immutable state once for this build rather than
        // tessellating its model description for every placed block.
        let mut shared_shapes =
            FxHashMap::<gpu_models::QueryKey, Option<Arc<DetailShape>>>::default();
        for (detail_blocks, pass) in [
            (&blocks.detail_blocks, StreamPass::DetailOpaque),
            (&blocks.glass_detail_blocks, StreamPass::DetailGlass),
        ] {
            for block in detail_blocks {
                // Do not resolve neighbor-dependent models or tessellate
                // vertices for an unreachable/offscreen Section. Detail
                // meshes can overhang their cell, so retain two blocks of
                // vertical halo. Unknown custom namespaces fail open.
                if (block.normalized_name.starts_with("minecraft:")
                    || !block.normalized_name.contains(':'))
                    && (-2_i32..=2).all(|dy| {
                        hidden.contains(&(pos, block.key.y.saturating_add(dy).div_euclid(64)))
                    })
                {
                    continue;
                }
                let shape = if let Some(query) = self.detail_shape_query(block) {
                    shared_shapes
                        .entry(gpu_models::QueryKey(query))
                        .or_insert_with_key(|key| {
                            model_family::resolve_shape(&key.0)
                                .map(detail_shape_from_model)
                                .map(Arc::new)
                        })
                        .as_deref()
                        .unwrap_or(&block.shape)
                } else {
                    &block.shape
                };
                push_detail_block_faces(block, block.key, shape, &mut faces);
                // Vanilla plants and redstone dust render without directional shading.
                let unshaded = block.normalized_name.ends_with("redstone_wire")
                    || is_unshaded_detail(&block.normalized_name);
                for mut face in faces.drain(..) {
                    if unshaded {
                        face.shade = 1.0;
                    }
                    if pass == StreamPass::DetailGlass && self.is_glass_detail_face_hidden(&face) {
                        continue;
                    }
                    if self.detail_face_hidden_by_opaque(&face) {
                        continue;
                    }
                    output.push(stream_face_from_preview(face, pass));
                }
            }
        }
    }

    /// Culls detail faces that lie on a cell boundary against a full opaque neighbor.
    ///
    /// Walls and fences emit faces on their cell boundary; when the neighboring cell is a solid
    /// block those faces are hidden, and keeping them z-fights with the neighbor's own face.
    pub(super) fn detail_face_hidden_by_opaque(&self, face: &Face) -> bool {
        let normal = face.normal;
        if normal == [0, 0, 0] {
            return false;
        }
        let mut min = [f64::INFINITY; 3];
        for corner in &face.corners {
            for (axis, value) in corner.iter().enumerate() {
                min[axis] = min[axis].min(*value);
            }
        }
        let block = BlockKey {
            x: (min[0] - f64::from(normal[0]) * 0.5).floor() as i32,
            y: (min[1] - f64::from(normal[1]) * 0.5).floor() as i32,
            z: (min[2] - f64::from(normal[2]) * 0.5).floor() as i32,
        };
        let neighbor = BlockKey {
            x: block.x + normal[0],
            y: block.y + normal[1],
            z: block.z + normal[2],
        };
        self.opaque_neighbor_hides_transparent(neighbor)
    }

    pub(super) fn push_stream_fluid_faces(
        &self,
        blocks: &ChunkBlocks,
        faces: &mut Vec<StreamFace>,
        pos: ChunkPos,
        hidden: &BTreeSet<(ChunkPos, i32)>,
    ) {
        for (fluid_blocks, pass, water) in [
            (&blocks.water_blocks, StreamPass::Water, true),
            (&blocks.lava_blocks, StreamPass::Lava, false),
        ] {
            for block in fluid_blocks {
                if section_is_hidden_with_face_halo(hidden, pos, block.key.y) {
                    continue;
                }
                for face in FACE_DEFINITIONS {
                    let kind = if water {
                        BlockClass::Water
                    } else {
                        BlockClass::Lava
                    };
                    let surface = self.fluid_surface_height(block.key, block.height, kind);
                    let Some(bottom) = self.exposed_fluid_face_bottom(block, face, kind) else {
                        continue;
                    };
                    faces.push(stream_fluid_face_segment(
                        block.key,
                        face,
                        block.color,
                        fluid_material(block, face.normal),
                        pass,
                        bottom,
                        surface,
                    ));
                }
            }
        }
    }
}

/// A cube/fluid cell can emit a face into either vertically adjacent band.
/// Suppress it only if the cell AND both one-block halo bands are unreachable.
/// This deliberately preserves Section-boundary faces until all nearby bands
/// are certified hidden (including at negative world heights).
pub(super) fn section_is_hidden_with_face_halo(
    hidden: &BTreeSet<(ChunkPos, i32)>,
    pos: ChunkPos,
    block_y: i32,
) -> bool {
    [
        block_y.saturating_sub(1),
        block_y,
        block_y.saturating_add(1),
    ]
    .into_iter()
    .all(|y| hidden.contains(&(pos, y.div_euclid(64))))
}
pub(super) fn is_unshaded_detail(name: &str) -> bool {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    name.contains("grass")
        || name.contains("fern")
        || name.contains("flower")
        || name.contains("cane")
        || name.contains("reed")
        || name.contains("vine")
        || name.contains("seagrass")
        || name.contains("kelp")
        || name.contains("lily")
        || name.contains("sapling")
        || name.contains("wheat")
        || name.contains("crop")
        // Fire is emissive; directional face shading makes the flame texture look boxed.
        || name == "fire"
        || name == "soul_fire"
}

pub use super::gpu_cubes::GpuCubeBand;
