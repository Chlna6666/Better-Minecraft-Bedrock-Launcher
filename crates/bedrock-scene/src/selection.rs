use super::*;

impl SceneBuilder {
    pub(super) fn build_combined_meshes(
        &self,
        generation: u64,
    ) -> Result<(Vec<ChunkMesh>, usize, usize, usize), String> {
        let (glass_result, (water_result, lava_result)) = rayon::join(
            || self.collect_glass_faces(),
            || rayon::join(|| self.collect_water_faces(), || self.collect_lava_faces()),
        );
        let (mut glass_faces, glass_omitted) = glass_result;
        let (mut water_faces, water_omitted) = water_result;
        let (mut lava_faces, lava_omitted) = lava_result;
        let preview_opaque_budget =
            opaque_face_budget(glass_faces.len(), water_faces.len(), lava_faces.len());
        let (mut opaque_faces, opaque_omitted) =
            self.collect_opaque_faces(self.face_limit(preview_opaque_budget));
        // Source-model voxels are a neutral material inspection, not terrain
        // lighting. Keep the palette's authored RGB on every visible face.
        if self.neutral_colors {
            for faces in [
                &mut opaque_faces,
                &mut glass_faces,
                &mut water_faces,
                &mut lava_faces,
            ] {
                for face in faces {
                    face.shade = 1.0;
                }
            }
        }
        let omitted_face_count = opaque_omitted
            .saturating_add(glass_omitted)
            .saturating_add(water_omitted)
            .saturating_add(lava_omitted);

        if opaque_faces.is_empty()
            && glass_faces.is_empty()
            && water_faces.is_empty()
            && lava_faces.is_empty()
        {
            return Ok((Vec::new(), omitted_face_count, 0, 0));
        }
        let (world_origin, center, horizontal_span, vertical_span) = self.mesh_frame();
        let meshes = build_meshes_at_world_origin(
            &opaque_faces,
            &glass_faces,
            &water_faces,
            &lava_faces,
            world_origin,
            center,
            horizontal_span,
            vertical_span,
            generation,
        )?;
        Ok((
            meshes,
            omitted_face_count,
            water_faces.len(),
            lava_faces.len(),
        ))
    }

    pub(super) fn face_limit(&self, preview_limit: usize) -> usize {
        if self.export_geometry {
            usize::MAX
        } else {
            preview_limit
        }
    }

    pub(super) fn collect_opaque_faces(&self, max_faces: usize) -> (Vec<Face>, usize) {
        let mut budget = FaceBudget::new(max_faces);
        let mut faces = Vec::with_capacity(max_faces.min(64 * 1024));
        self.push_opaque_faces(&mut faces, &mut budget);
        (faces, budget.omitted_faces)
    }

    pub(super) fn collect_glass_faces(&self) -> (Vec<Face>, usize) {
        let mut budget = FaceBudget::new(self.face_limit(GLASS_FACE_BUDGET));
        let mut faces = Vec::with_capacity(GLASS_FACE_BUDGET.min(16 * 1024));
        self.push_glass_faces(&mut faces, &mut budget);
        (faces, budget.omitted_faces)
    }

    pub(super) fn collect_water_faces(&self) -> (Vec<Face>, usize) {
        let mut budget = FaceBudget::new(self.face_limit(WATER_FACE_BUDGET / 2));
        let mut faces = Vec::with_capacity(WATER_FACE_BUDGET.min(16 * 1024));
        self.push_water_faces(&mut faces, &mut budget);
        (faces, budget.omitted_faces)
    }

    pub(super) fn collect_lava_faces(&self) -> (Vec<Face>, usize) {
        let mut budget = FaceBudget::new(self.face_limit(WATER_FACE_BUDGET / 2));
        let mut faces = Vec::with_capacity((WATER_FACE_BUDGET / 2).min(8 * 1024));
        self.push_lava_faces(&mut faces, &mut budget);
        (faces, budget.omitted_faces)
    }

    pub(super) fn push_opaque_faces(&self, faces: &mut Vec<Face>, budget: &mut FaceBudget) {
        let merger = self
            .block_chunks
            .par_iter()
            .map(|blocks| {
                let blocks = blocks.1;
                let mut merger = FaceMerger::new();
                for block in &blocks.opaque_blocks {
                    for face in FACE_DEFINITIONS {
                        if self.is_opaque_neighbor(block.key, face) {
                            continue;
                        }
                        merger.push(
                            block.key,
                            face,
                            block.colors.color_from_normal(face.normal),
                            block.material.clone(),
                            0.0,
                        );
                    }
                }
                merger
            })
            .reduce(FaceMerger::new, FaceMerger::merge);
        merger.emit_into(faces, budget);
        self.push_detail_faces(faces, budget, |blocks| &blocks.detail_blocks);
    }

    pub(super) fn mesh_frame(&self) -> ([i32; 3], [f32; 3], f32, f32) {
        if self.min_y == i16::MAX || self.max_y == i16::MIN {
            let (center, horizontal_span, vertical_span) = selection_frame(self.bounds);
            return ([0, 0, 0], center, horizontal_span, vertical_span);
        }
        let world_origin = [self.min_x, i32::from(self.min_y), self.min_z];
        let min_x = f64::from(self.min_x);
        let max_x = f64::from(self.max_x) + 1.0;
        let min_z = f64::from(self.min_z);
        let max_z = f64::from(self.max_z) + 1.0;
        let min_y = f64::from(self.min_y);
        let max_y = (f64::from(self.max_y)).max(min_y + 1.0) + 1.0;
        let horizontal_span = (max_x - min_x).max(max_z - min_z).max(1.0) as f32;
        let vertical_span = (max_y - min_y).max(1.0) as f32;
        (
            world_origin,
            [
                ((min_x + max_x) * 0.5 - f64::from(world_origin[0])) as f32,
                ((min_y + max_y) * 0.5 - f64::from(world_origin[1])) as f32,
                ((min_z + max_z) * 0.5 - f64::from(world_origin[2])) as f32,
            ],
            horizontal_span,
            vertical_span,
        )
    }

    pub(super) fn push_glass_faces(&self, faces: &mut Vec<Face>, budget: &mut FaceBudget) {
        let merger = self
            .block_chunks
            .par_iter()
            .map(|blocks| {
                let blocks = blocks.1;
                let mut merger = FaceMerger::new();
                for block in &blocks.glass_blocks {
                    for face in FACE_DEFINITIONS {
                        if self.is_glass_neighbor(block.key, face) {
                            continue;
                        }
                        merger.push(
                            block.key,
                            face,
                            block.colors.color_from_normal(face.normal),
                            block.material.clone(),
                            0.0,
                        );
                    }
                }
                merger
            })
            .reduce(FaceMerger::new, FaceMerger::merge);
        merger.emit_into(faces, budget);
        self.push_detail_faces(faces, budget, |blocks| &blocks.glass_detail_blocks);
    }

    pub(super) fn push_detail_faces<'a>(
        &'a self,
        faces: &mut Vec<Face>,
        budget: &mut FaceBudget,
        blocks_for_chunk: impl Fn(&'a ChunkBlocks) -> &'a [DetailBlock] + Copy + Sync,
    ) {
        let detail_faces = self
            .block_chunks
            .par_iter()
            .map(|blocks| {
                let mut faces = Vec::new();
                for block in blocks_for_chunk(blocks.1) {
                    let inferred_shape = self.inferred_detail_shape(block);
                    let shape = inferred_shape.as_ref().unwrap_or(&block.shape);
                    push_detail_block_faces(block, block.key, shape, &mut faces);
                }
                faces
            })
            .reduce(Vec::new, |mut left, mut right| {
                left.append(&mut right);
                left
            });
        for face in detail_faces {
            budget.push_or_omit(faces, face);
        }
    }
    pub(super) fn push_water_faces(&self, faces: &mut Vec<Face>, budget: &mut FaceBudget) {
        let merger = self
            .block_chunks
            .par_iter()
            .map(|blocks| {
                let blocks = blocks.1;
                let mut merger = FaceMerger::new();
                for block in &blocks.water_blocks {
                    let surface =
                        self.fluid_surface_height(block.key, block.height, BlockClass::Water);
                    for face in FACE_DEFINITIONS {
                        // Side walls below the full cell height cannot join the
                        // integer-cell merge; the exact pass emits them.
                        if face.normal[1] == 0 && surface < 0.999 {
                            continue;
                        }
                        // A partial-height neighbour exposes only the upper
                        // strip. Keep ordinary full cube faces in the greedy
                        // merger, but emit this strip in the exact pass below.
                        if self.exposed_fluid_face_bottom(block, face, BlockClass::Water)
                            != Some(0.0)
                        {
                            continue;
                        }
                        // Horizontal fluid faces sit at the official surface
                        // height (level/9), not the cell boundary; carry the
                        // drop in the merge key so ocean surfaces still merge.
                        let plane_drop = if face.normal[1] > 0 {
                            1.0 - surface
                        } else {
                            0.0
                        };
                        merger.push(
                            block.key,
                            face,
                            block.color,
                            fluid_material(block, face.normal),
                            plane_drop,
                        );
                    }
                }
                merger
            })
            .reduce(FaceMerger::new, FaceMerger::merge);
        merger.emit_into(faces, budget);
        for blocks in self.block_chunks.values() {
            for block in &blocks.water_blocks {
                let surface =
                    self.fluid_surface_height(block.key, block.height, BlockClass::Water);
                for face in FACE_DEFINITIONS {
                    // Horizontal faces are fully covered by the greedy pass
                    // (each is either culled or emitted at the surface plane);
                    // full-height side walls merged there as well.
                    if face.normal[1] != 0 || surface >= 0.999 {
                        continue;
                    }
                    let Some(bottom) =
                        self.exposed_fluid_face_bottom(block, face, BlockClass::Water)
                    else {
                        continue;
                    };
                    budget.push_or_omit(
                        faces,
                        fluid_face_segment(
                            block.key,
                            face,
                            block.color,
                            fluid_material(block, face.normal),
                            bottom,
                            surface,
                        ),
                    );
                }
            }
        }
    }

    pub(super) fn push_lava_faces(&self, faces: &mut Vec<Face>, budget: &mut FaceBudget) {
        let merger = self
            .block_chunks
            .par_iter()
            .map(|blocks| {
                let blocks = blocks.1;
                let mut merger = FaceMerger::new();
                for block in &blocks.lava_blocks {
                    let surface =
                        self.fluid_surface_height(block.key, block.height, BlockClass::Lava);
                    for face in FACE_DEFINITIONS {
                        // Side walls below the full cell height cannot join the
                        // integer-cell merge; the exact pass emits them.
                        if face.normal[1] == 0 && surface < 0.999 {
                            continue;
                        }
                        // A partial-height neighbour exposes only the upper
                        // strip. Keep ordinary full cube faces in the greedy
                        // merger, but emit this strip in the exact pass below.
                        if self.exposed_fluid_face_bottom(block, face, BlockClass::Lava)
                            != Some(0.0)
                        {
                            continue;
                        }
                        let plane_drop = if face.normal[1] > 0 {
                            1.0 - surface
                        } else {
                            0.0
                        };
                        merger.push(
                            block.key,
                            face,
                            block.color,
                            fluid_material(block, face.normal),
                            plane_drop,
                        );
                    }
                }
                merger
            })
            .reduce(FaceMerger::new, FaceMerger::merge);
        merger.emit_into(faces, budget);
        for blocks in self.block_chunks.values() {
            for block in &blocks.lava_blocks {
                let surface =
                    self.fluid_surface_height(block.key, block.height, BlockClass::Lava);
                for face in FACE_DEFINITIONS {
                    if face.normal[1] != 0 || surface >= 0.999 {
                        continue;
                    }
                    let Some(bottom) =
                        self.exposed_fluid_face_bottom(block, face, BlockClass::Lava)
                    else {
                        continue;
                    };
                    budget.push_or_omit(
                        faces,
                        fluid_face_segment(
                            block.key,
                            face,
                            block.color,
                            fluid_material(block, face.normal),
                            bottom,
                            surface,
                        ),
                    );
                }
            }
        }
    }
}
