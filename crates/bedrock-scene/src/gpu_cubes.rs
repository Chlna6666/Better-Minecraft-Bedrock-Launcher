//! Cell/halo extraction for GPU cube meshing; no per-block faces are materialized.
use super::*;

/// CPU-side cells, face templates, and model instances for one 64-block band.
pub struct GpuCubeBand {
    pub size: [u32; 3],
    pub origin: gpui_3d::Vec3,
    pub cells: Vec<u32>,
    pub faces: Vec<StreamFace>,
    pub model_faces: usize,
    pub template_faces: usize,
    pub classes: Vec<u32>,
    pub neighbours: Vec<[i32; 3]>,
    pub instances: Vec<gpui_3d::VoxelMeshInstance>,
    pub section_y: i32,
}

#[cfg(test)]
mod tests;

impl SceneBuilder {
    /// Builds voxel descriptions for an inserted chunk, optionally retaining selected bands.
    pub fn gpu_cube_bands(
        &self,
        pos: ChunkPos,
        selection: Option<&BandSelection>,
    ) -> Option<Vec<GpuCubeBand>> {
        self.gpu_cube_bands_skipping(pos, selection, &std::collections::BTreeSet::new())
    }

    /// Skip unreachable 64-block bands before allocating GPU voxel grids,
    /// face templates or instance records. Visibility is conservative and
    /// rechecked when a deferred band is promoted by the streaming worker.
    pub fn gpu_cube_bands_skipping(
        &self,
        pos: ChunkPos,
        selection: Option<&BandSelection>,
        hidden: &std::collections::BTreeSet<i32>,
    ) -> Option<Vec<GpuCubeBand>> {
        let blocks = self.block_chunks.get(&ChunkKey::from_pos(pos))?;
        // Sodium/vanilla skip compiling known-empty render sections before
        // block-model expansion. Apply the same fast exit to fully hidden
        // vanilla cube/fluid columns: every possible face band (including
        // +/-1 height-boundary halo) is already certified unreachable.
        // Custom/detail geometry can overhang by arbitrary distances, so it
        // always uses the full ModelPlan range calculation below.
        let has_detail_models =
            !blocks.detail_blocks.is_empty() || !blocks.glass_detail_blocks.is_empty();
        if !has_detail_models && blocks.min_y <= blocks.max_y {
            let first = (i32::from(blocks.min_y) - 1).div_euclid(64);
            let last = (i32::from(blocks.max_y) + 1).div_euclid(64);
            if (first..=last).all(|band| hidden.contains(&band)) {
                return Some(Vec::new());
            }
        }
        if blocks.opaque_blocks.is_empty()
            && blocks.glass_blocks.is_empty()
            && blocks.detail_blocks.is_empty()
            && blocks.glass_detail_blocks.is_empty()
            && blocks.water_blocks.is_empty()
            && blocks.lava_blocks.is_empty()
        {
            return Some(Vec::new());
        }
        // Filter cells before ModelPlan resolves captured/state-dependent
        // geometry. The old path built every face template and per-cell model
        // record first, then dropped the hidden bands afterward.
        let plan = super::gpu_models::ModelPlan::new(self, blocks, |y, detail| {
            let halo = if detail { 2_i32 } else { 1_i32 };
            (-halo..=halo).all(|dy| hidden.contains(&y.saturating_add(dy).div_euclid(64)))
        });
        let mut bands = std::collections::BTreeMap::<i32, Vec<_>>::new();
        for record in &plan.records {
            let (min, max) = plan.band_range(record);
            if i64::from(max) - i64::from(min) > 1024 {
                return None;
            }
            for band in min..=max {
                if hidden.contains(&band) {
                    continue;
                }
                if selection.is_none_or(|selection| {
                    !selection.retained.contains(&band)
                        || selection
                            .affected
                            .iter()
                            .any(|&(min, max)| (min..=max).contains(&band))
                }) {
                    bands.entry(band).or_default().push(record);
                }
            }
        }
        let mut output = Vec::new();
        for (band, records) in bands {
            let mut min = [i32::MAX; 3];
            let mut max = [i32::MIN; 3];
            for record in &records {
                for (axis, value) in [record.key.x, record.key.y, record.key.z]
                    .into_iter()
                    .enumerate()
                {
                    min[axis] = min[axis].min(value.checked_add(plan.min_offset[axis])?);
                    max[axis] = max[axis].max(value.checked_add(plan.max_offset[axis])?);
                }
            }
            let mut size = [0; 3];
            for axis in 0..3 {
                size[axis] = u32::try_from(i64::from(max[axis]) - i64::from(min[axis]) + 1)
                    .ok()?
                    .max(3);
            }
            let count = size
                .into_iter()
                .try_fold(1usize, |total, n| total.checked_mul(n as usize))?;
            // A resource-pack model can extend arbitrarily far. Keep such exceptional grids
            // on the existing CPU path rather than allocate an unbounded halo.
            if count > 2_097_152 {
                return None;
            }
            let mut classes = vec![0; count];
            let mut cells = vec![0; count];
            for z in 0..size[2] {
                for y in 0..size[1] {
                    for x in 0..size[0] {
                        let key = BlockKey {
                            x: min[0] + x as i32,
                            y: min[1] + y as i32,
                            z: min[2] + z as i32,
                        };
                        // Most padded-grid cells belong to the target column.
                        // Its full block data is already borrowed above: avoid
                        // two global ChunkKey -> FxHashMap probes per cell (one
                        // for material class, another for ambient occlusion).
                        // Only the one-cell halo needs cross-column lookups.
                        let (block_class, ao) =
                            if ChunkKey::from_block(key) == ChunkKey::from_pos(pos) {
                                (
                                    blocks.class_at(key),
                                    blocks.occupied.contains(&key)
                                        && !blocks.ao_exclusions.contains(&key),
                                )
                            } else {
                                (self.block_class_at(key), self.ambient_occludes_at(key))
                            };
                        // An absent neighbor column is *unknown*, not
                        // certified air. Mark its halo with only the fluid
                        // bits so water/lava side instances cannot emit
                        // full-height walls into unloaded Chunk seams.
                        // Opaque cubes still see the cell as non-occluding.
                        // When the neighbor is later decoded, ordinary seam
                        // invalidation re-creates the correct exposed faces.
                        let neighbour_column = ChunkKey::from_block(key);
                        let unknown_column = neighbour_column != ChunkKey::from_pos(pos)
                            && !self.block_chunks.contains_key(&neighbour_column)
                            && !self.boundary_chunks.contains_key(&neighbour_column);
                        // Waterlogged cells carry a liquid layer: fluid-side
                        // instances must see them as same-fluid neighbours even
                        // though their primary block is a detail/solid block,
                        // otherwise adjacent waterlogged pairs paint coplanar
                        // walls through each other's faces.
                        let fluid_bits = match (blocks.water.contains(&key)
                            || blocks.waterlogged_water.contains(&key), blocks.lava.contains(&key)
                            || blocks.waterlogged_lava.contains(&key))
                        {
                            (true, _) => 4,
                            (_, true) => 8,
                            _ => 0,
                        };
                        let class = if unknown_column {
                            4 | 8
                        } else {
                            let base = match block_class {
                                Some(BlockClass::Opaque) => 1,
                                Some(BlockClass::TransparentGlass) => 2,
                                Some(BlockClass::Water) => 4,
                                Some(BlockClass::Lava) => 8,
                                _ => 0,
                            };
                            base | fluid_bits
                        };
                        let index = ((z * size[1] + y) * size[0] + x) as usize;
                        classes[index] = class | if ao { 16 } else { 0 };
                        // Populate cube occupancy alongside the class halo.
                        // The old second volume-sized pass allocated and
                        // scanned the same padded grid again per Section.
                        cells[index] = if class & 1 != 0 { u32::MAX } else { 0 };
                    }
                }
            }
            // Full opaque cubes are *cells*, not six-face model instances.
            // The padded halo is computed above before selecting any face.
            // Unknown/opaque neighbours suppress a face through the non-emitting
            // sentinel; transparent and detailed blocks never falsely occlude
            // the opaque cell mesh. GPU cs_mesh checks these cells before it
            // writes a GeneratedFace, skipping the instance count/scan/emit
            // stages entirely for ordinary terrain.
            let mut cube_faces = Vec::new();
            let mut cube_neighbours = Vec::new();
            let mut instance_faces = Vec::new();
            let mut instance_neighbours = Vec::new();
            let mut cube_models = FxHashMap::<(u32, u32), u32>::default();
            let mut instance_ranges = FxHashMap::<(u32, u32), std::ops::Range<u32>>::default();
            let mut instances = Vec::new();
            for record in records {
                let position = [
                    (record.key.x - min[0]) as u32,
                    (record.key.y - min[1]) as u32,
                    (record.key.z - min[2]) as u32,
                ];
                if record.opaque_cube {
                    let kind = *cube_models
                        .entry((record.faces.start, record.faces.end))
                        .or_insert_with(|| {
                            let kind = (cube_faces.len() / 6 + 1) as u32;
                            cube_faces.extend_from_slice(
                                &plan.faces[record.faces.start as usize..record.faces.end as usize],
                            );
                            cube_neighbours.extend_from_slice(
                                &plan.neighbours
                                    [record.faces.start as usize..record.faces.end as usize],
                            );
                            kind
                        });
                    let index =
                        ((position[2] * size[1] + position[1]) * size[0] + position[0]) as usize;
                    cells[index] = kind;
                } else {
                    let range = instance_ranges
                        .entry((record.faces.start, record.faces.end))
                        .or_insert_with(|| {
                            let start = instance_faces.len() as u32;
                            instance_faces.extend_from_slice(
                                &plan.faces[record.faces.start as usize..record.faces.end as usize],
                            );
                            instance_neighbours.extend_from_slice(
                                &plan.neighbours
                                    [record.faces.start as usize..record.faces.end as usize],
                            );
                            start..instance_faces.len() as u32
                        })
                        .clone();
                    instances.push(gpui_3d::VoxelMeshInstance {
                        position,
                        faces: range,
                        height: record.height,
                        occlusion_mask: record.mask,
                    });
                }
            }
            let model_faces = cube_faces.len();
            cube_faces.extend(instance_faces);
            cube_neighbours.extend(instance_neighbours);
            output.push(GpuCubeBand {
                size,
                origin: gpui_3d::Vec3::new(
                    (min[0] - pos.x * 16) as f32,
                    min[1] as f32,
                    (min[2] - pos.z * 16) as f32,
                ),
                cells,
                classes,
                template_faces: cube_faces.len(),
                faces: cube_faces,
                neighbours: cube_neighbours,
                model_faces,
                instances,
                section_y: band,
            });
        }
        Some(output)
    }
}
