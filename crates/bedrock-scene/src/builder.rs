use super::*;

/// Builds Bedrock geometry while retaining the required chunk and neighbor data.
pub struct SceneBuilder {
    pub(super) bounds: SlimeChunkBounds,
    pub(super) mask_unprocessed_neighbors: bool,
    pub(super) ambient_occlusion: bool,
    pub(super) neutral_colors: bool,
    pub(super) export_geometry: bool,
    pub(super) combined_meshes: Vec<ChunkMesh>,
    pub(super) mesh_generation: u64,
    pub(super) min_y: i16,
    pub(super) max_y: i16,
    pub(super) min_x: i32,
    pub(super) max_x: i32,
    pub(super) min_z: i32,
    pub(super) max_z: i32,
    pub(super) missing_chunks: usize,
    pub(super) processed_chunk_count: usize,
    pub(super) subchunk_count: usize,
    pub(super) solid_block_count: usize,
    pub(super) glass_block_count: usize,
    pub(super) water_block_count: usize,
    pub(super) lava_block_count: usize,
    pub(super) face_count: usize,
    pub(super) glass_face_count: usize,
    pub(super) water_face_count: usize,
    pub(super) lava_face_count: usize,
    pub(super) culled_face_count: usize,
    pub(super) omitted_face_count: usize,
    pub(super) truncated_chunk_count: usize,
    pub(super) block_chunks: FxHashMap<ChunkKey, ChunkBlocks>,
    pub(super) boundary_chunks: FxHashMap<ChunkKey, Arc<ChunkBoundary>>,
    pub(super) processed_chunk_keys: FxHashSet<ChunkKey>,
}

/// Decoded chunk records prepared for a selection snapshot.
pub struct PreparedChunk {
    pub(super) chunk_key: ChunkKey,
    pub(super) subchunk_count: usize,
    pub(super) internally_culled_blocks: usize,
    pub(super) blocks: Option<ChunkBlocks>,
}

impl SceneBuilder {
    pub(super) fn new(bounds: SlimeChunkBounds, truncated_chunk_count: usize) -> Self {
        Self {
            bounds,
            mask_unprocessed_neighbors: true,
            ambient_occlusion: true,
            neutral_colors: false,
            export_geometry: false,
            combined_meshes: Vec::new(),
            mesh_generation: 0,
            min_y: i16::MAX,
            max_y: i16::MIN,
            min_x: i32::MAX,
            max_x: i32::MIN,
            min_z: i32::MAX,
            max_z: i32::MIN,
            missing_chunks: 0,
            processed_chunk_count: 0,
            subchunk_count: 0,
            solid_block_count: 0,
            glass_block_count: 0,
            water_block_count: 0,
            lava_block_count: 0,
            face_count: 0,
            glass_face_count: 0,
            water_face_count: 0,
            lava_face_count: 0,
            culled_face_count: 0,
            omitted_face_count: 0,
            truncated_chunk_count,
            block_chunks: FxHashMap::default(),
            boundary_chunks: FxHashMap::default(),
            processed_chunk_keys: FxHashSet::default(),
        }
    }

    /// Starts an incremental selection build.
    ///
    /// Export mode emits all faces, including those over the interactive preview budget.
    pub fn new_selection(
        bounds: SlimeChunkBounds,
        truncated_chunk_count: usize,
        for_export: bool,
    ) -> Self {
        let mut builder = Self::new(bounds, truncated_chunk_count);
        builder.export_geometry = for_export;
        builder
    }

    /// Rebuilds the selection geometry from the chunks inserted so far.
    ///
    /// # Errors
    /// Returns a mesh construction error if the geometry is invalid.
    pub fn build_selection(&mut self) -> Result<SceneMesh, String> {
        self.rebuild_combined_mesh()?;
        Ok(self.build_mesh())
    }

    pub(super) fn with_neutral_colors(mut self) -> Self {
        self.neutral_colors = true;
        self.ambient_occlusion = false;
        self
    }

    /// Adds a prepared chunk to the current selection.
    pub fn insert_prepared(&mut self, processed_chunk: PreparedChunk) {
        self.processed_chunk_count = self.processed_chunk_count.saturating_add(1);
        self.processed_chunk_keys.insert(processed_chunk.chunk_key);
        self.culled_face_count = self
            .culled_face_count
            .saturating_add(processed_chunk.internally_culled_blocks.saturating_mul(6));
        let Some(mut blocks) = processed_chunk.blocks else {
            self.missing_chunks = self.missing_chunks.saturating_add(1);
            return;
        };
        blocks.rebuild_neighbor_indexes();
        self.subchunk_count = self
            .subchunk_count
            .saturating_add(processed_chunk.subchunk_count);
        self.block_chunks.insert(processed_chunk.chunk_key, blocks);
    }

    pub(super) fn build_mesh(&self) -> SceneMesh {
        SceneMesh {
            chunk_meshes: self.combined_meshes.clone(),
            min_y: if self.min_y == i16::MAX {
                0
            } else {
                self.min_y
            },
            max_y: if self.max_y == i16::MIN {
                0
            } else {
                self.max_y
            },
            min_x: if self.min_x == i32::MAX {
                0
            } else {
                self.min_x
            },
            max_x: if self.max_x == i32::MIN {
                0
            } else {
                self.max_x
            },
            min_z: if self.min_z == i32::MAX {
                0
            } else {
                self.min_z
            },
            max_z: if self.max_z == i32::MIN {
                0
            } else {
                self.max_z
            },
            missing_chunks: self.missing_chunks,
            chunk_count: bounds_chunk_count(self.bounds),
            processed_chunk_count: self.processed_chunk_count,
            subchunk_count: self.subchunk_count,
            solid_block_count: self.solid_block_count,
            glass_block_count: self.glass_block_count,
            water_block_count: self.water_block_count,
            lava_block_count: self.lava_block_count,
            face_count: self.face_count,
            glass_face_count: self.glass_face_count,
            water_face_count: self.water_face_count,
            lava_face_count: self.lava_face_count,
            culled_face_count: self.culled_face_count,
            omitted_face_count: self.omitted_face_count,
            truncated_chunk_count: self.truncated_chunk_count,
            vertex_budget: TOTAL_VERTEX_BUDGET,
        }
    }

    pub(super) fn rebuild_combined_mesh(&mut self) -> Result<(), String> {
        self.recalculate_block_stats();
        self.mesh_generation = self.mesh_generation.saturating_add(1);
        let (build, omitted_face_count, water_face_count, lava_face_count) =
            self.build_combined_meshes(self.mesh_generation)?;
        self.combined_meshes = build;
        self.omitted_face_count = omitted_face_count;
        self.recalculate_mesh_stats();
        self.water_face_count = water_face_count;
        self.lava_face_count = lava_face_count;
        Ok(())
    }
    pub(super) fn recalculate_block_stats(&mut self) {
        self.min_y = i16::MAX;
        self.max_y = i16::MIN;
        self.min_x = i32::MAX;
        self.max_x = i32::MIN;
        self.min_z = i32::MAX;
        self.max_z = i32::MIN;
        self.solid_block_count = 0;
        self.glass_block_count = 0;
        self.water_block_count = 0;
        self.lava_block_count = 0;
        for blocks in self.block_chunks.values() {
            self.solid_block_count = self
                .solid_block_count
                .saturating_add(blocks.opaque_blocks.len())
                .saturating_add(blocks.detail_blocks.len());
            self.glass_block_count = self
                .glass_block_count
                .saturating_add(blocks.glass_blocks.len())
                .saturating_add(blocks.glass_detail_blocks.len());
            self.water_block_count = self
                .water_block_count
                .saturating_add(blocks.water_blocks.len());
            self.lava_block_count = self
                .lava_block_count
                .saturating_add(blocks.lava_blocks.len());
            if blocks.min_y != i16::MAX {
                self.min_y = self.min_y.min(blocks.min_y);
                self.max_y = self.max_y.max(blocks.max_y);
                self.min_x = self.min_x.min(blocks.min_x);
                self.max_x = self.max_x.max(blocks.max_x);
                self.min_z = self.min_z.min(blocks.min_z);
                self.max_z = self.max_z.max(blocks.max_z);
            }
        }
    }

    pub(super) fn recalculate_mesh_stats(&mut self) {
        self.face_count = 0;
        self.glass_face_count = 0;
        self.water_face_count = 0;
        self.lava_face_count = 0;
        for mesh in &self.combined_meshes {
            self.face_count = self
                .face_count
                .saturating_add(mesh.ranges.opaque.count as usize / 6);
            self.glass_face_count = self
                .glass_face_count
                .saturating_add(mesh.ranges.glass.count as usize / 6);
        }
    }
}

pub(super) fn selection_frame(bounds: SlimeChunkBounds) -> ([f32; 3], f32, f32) {
    let min_x = (bounds.min_chunk_x.saturating_mul(16)) as f32;
    let max_x = (bounds.max_chunk_x.saturating_add(1).saturating_mul(16)) as f32;
    let min_z = (bounds.min_chunk_z.saturating_mul(16)) as f32;
    let max_z = (bounds.max_chunk_z.saturating_add(1).saturating_mul(16)) as f32;
    let horizontal_span = (max_x - min_x).max(max_z - min_z).max(1.0);
    (
        [(min_x + max_x) * 0.5, 0.5, (min_z + max_z) * 0.5],
        horizontal_span,
        1.0,
    )
}

/// Prepares a decoded chunk for selection meshing. Missing chunks are tracked as empty records.
///
/// # Errors
/// Returns a scene collection error if the chunk cannot be prepared.
pub fn prepare(
    chunk: &ChunkData,
    block_models: Option<&BlockModelRepository>,
) -> Result<PreparedChunk, String> {
    let chunk_key = ChunkKey::from_pos(chunk.pos);
    if !chunk.is_loaded {
        return Ok(PreparedChunk {
            chunk_key,
            subchunk_count: 0,
            internally_culled_blocks: 0,
            blocks: None,
        });
    }
    let blocks = crate::chunk::collect(chunk, block_models, None, |_| true)?;
    let internally_culled_blocks = blocks.internally_culled_blocks;
    Ok(PreparedChunk {
        chunk_key,
        subchunk_count: chunk.subchunks.len(),
        internally_culled_blocks,
        blocks: Some(blocks),
    })
}
pub(super) const fn bounds_width(bounds: SlimeChunkBounds) -> i32 {
    bounds.max_chunk_x.saturating_sub(bounds.min_chunk_x) + 1
}

pub(super) const fn bounds_depth(bounds: SlimeChunkBounds) -> i32 {
    bounds.max_chunk_z.saturating_sub(bounds.min_chunk_z) + 1
}
pub(super) fn bounds_chunk_count(bounds: SlimeChunkBounds) -> usize {
    usize::try_from(bounds_width(bounds))
        .unwrap_or(0)
        .saturating_mul(usize::try_from(bounds_depth(bounds)).unwrap_or(0))
}
