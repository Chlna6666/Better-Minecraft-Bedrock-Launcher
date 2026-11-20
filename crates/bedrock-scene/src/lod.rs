use super::*;

impl SceneBuilder {
    /// Builds a coarse, material-preserving far mesh from occupied cells. The source chunks and
    /// their neighbors stay available for culling while only the target columns emit faces.
    ///
    /// # Panics
    /// Panics if the scale is not a divisor of 16 greater than one.
    pub fn build_stream_lod_faces(&self, targets: &[ChunkPos], scale: i32) -> Vec<ChunkFaces> {
        assert!(
            scale > 1 && 16 % scale == 0,
            "LOD scale must divide 16 and exceed one"
        );
        let coarse_key = |key: BlockKey| BlockKey {
            x: key.x.div_euclid(scale),
            y: key.y.div_euclid(scale),
            z: key.z.div_euclid(scale),
        };
        let mut opaque = FxHashMap::<BlockKey, &BlockRecord>::default();
        let mut glass = FxHashMap::<BlockKey, &BlockRecord>::default();
        let mut water = FxHashMap::<BlockKey, &FluidBlock>::default();
        let mut lava = FxHashMap::<BlockKey, &FluidBlock>::default();
        let prefer = |candidate: BlockKey, current: BlockKey| {
            candidate.y > current.y
                || (candidate.y == current.y && (candidate.x, candidate.z) < (current.x, current.z))
        };
        for blocks in self.block_chunks.values() {
            for block in &blocks.opaque_blocks {
                opaque
                    .entry(coarse_key(block.key))
                    .and_modify(|current| {
                        if prefer(block.key, current.key) {
                            *current = block;
                        }
                    })
                    .or_insert(block);
            }
            for block in &blocks.glass_blocks {
                glass
                    .entry(coarse_key(block.key))
                    .and_modify(|current| {
                        if prefer(block.key, current.key) {
                            *current = block;
                        }
                    })
                    .or_insert(block);
            }
            for block in &blocks.water_blocks {
                water
                    .entry(coarse_key(block.key))
                    .and_modify(|current| {
                        if prefer(block.key, current.key) {
                            *current = block;
                        }
                    })
                    .or_insert(block);
            }
            for block in &blocks.lava_blocks {
                lava.entry(coarse_key(block.key))
                    .and_modify(|current| {
                        if prefer(block.key, current.key) {
                            *current = block;
                        }
                    })
                    .or_insert(block);
            }
        }
        assert!(
            self.boundary_chunks.is_empty() || scale == 4,
            "cached coarse neighbors require scale 4"
        );
        let mut boundary_opaque = FxHashSet::default();
        let mut boundary_glass = FxHashSet::default();
        let mut boundary_water = FxHashSet::default();
        let mut boundary_lava = FxHashSet::default();
        for boundary in self.boundary_chunks.values() {
            boundary_opaque.extend(boundary.coarse_opaque.iter().copied());
            boundary_glass.extend(boundary.coarse_glass.iter().copied());
            boundary_water.extend(boundary.coarse_water.iter().copied());
            boundary_water.extend(boundary.waterlogged_water.iter().copied());
            boundary_lava.extend(boundary.coarse_lava.iter().copied());
            boundary_lava.extend(boundary.waterlogged_lava.iter().copied());
        }
        let scaled_face = |key: BlockKey, face: FaceDefinition, color, material, pass| {
            let base = BlockKey {
                x: key.x * scale,
                y: key.y * scale,
                z: key.z * scale,
            };
            let mut output = stream_cube_face(base, face, color, material, pass);
            for corner in &mut output.corners {
                for axis in 0..3 {
                    let origin = f64::from([base.x, base.y, base.z][axis]);
                    corner[axis] = origin + (corner[axis] - origin) * f64::from(scale);
                }
            }
            output
        };
        let mut columns = FxHashMap::<(i32, i32), Vec<BlockKey>>::default();
        for key in opaque
            .keys()
            .chain(glass.keys())
            .chain(water.keys())
            .chain(lava.keys())
        {
            columns
                .entry((key.x.div_euclid(16 / scale), key.z.div_euclid(16 / scale)))
                .or_default()
                .push(*key);
        }
        for keys in columns.values_mut() {
            keys.sort_unstable_by_key(|key| (key.x, key.y, key.z));
            keys.dedup();
        }
        targets
            .par_iter()
            .filter_map(|pos| {
                let keys = columns.get(&(pos.x, pos.z))?;
                let mut faces = Vec::new();
                for &key in keys {
                    if let Some(block) = opaque.get(&key) {
                        for face in FACE_DEFINITIONS {
                            let neighbor = key.neighbor(face);
                            if opaque.contains_key(&neighbor) || boundary_opaque.contains(&neighbor)
                            {
                                continue;
                            }
                            faces.push(scaled_face(
                                key,
                                face,
                                block.colors.color_from_normal(face.normal),
                                face_material_name(block.material.clone(), face.normal),
                                StreamPass::Opaque,
                            ));
                        }
                        continue;
                    }
                    if let Some(block) = glass.get(&key) {
                        for face in FACE_DEFINITIONS {
                            let neighbor = key.neighbor(face);
                            if opaque.contains_key(&neighbor)
                                || boundary_opaque.contains(&neighbor)
                                || glass.contains_key(&neighbor)
                                || boundary_glass.contains(&neighbor)
                            {
                                continue;
                            }
                            faces.push(scaled_face(
                                key,
                                face,
                                block.colors.color_from_normal(face.normal),
                                face_material_name(block.material.clone(), face.normal),
                                StreamPass::Glass,
                            ));
                        }
                        continue;
                    }
                    for (cells, boundary_cells, pass) in [
                        (&water, &boundary_water, StreamPass::Water),
                        (&lava, &boundary_lava, StreamPass::Lava),
                    ] {
                        if let Some(block) = cells.get(&key) {
                            for face in FACE_DEFINITIONS {
                                let neighbor = key.neighbor(face);
                                if opaque.contains_key(&neighbor)
                                    || boundary_opaque.contains(&neighbor)
                                    || glass.contains_key(&neighbor)
                                    || boundary_glass.contains(&neighbor)
                                    || cells.contains_key(&neighbor)
                                    || boundary_cells.contains(&neighbor)
                                {
                                    continue;
                                }
                                faces.push(scaled_face(
                                    key,
                                    face,
                                    block.color,
                                    fluid_material(block, face.normal),
                                    pass,
                                ));
                            }
                            break;
                        }
                    }
                }
                (!faces.is_empty()).then_some(ChunkFaces { pos: *pos, faces })
            })
            .collect()
    }
}
