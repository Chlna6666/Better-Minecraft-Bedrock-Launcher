//! Cell/halo extraction for GPU cube meshing; no per-block faces are materialized.
use super::*;

pub(in crate::ui::window::map_viewer) struct GpuCubeBand {
    pub size: [u32; 3],
    pub origin: gpui_3d::Vec3,
    pub cells: Vec<u32>,
    pub faces: Vec<Preview3dStreamFace>,
    pub model_faces: usize,
    pub template_faces: usize,
    pub classes: Vec<u32>,
    pub neighbours: Vec<[i32; 3]>,
    pub instances: Vec<gpui_3d::VoxelMeshInstance>,
    pub section_y: i32,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gpu_models_and_fluids_match_cpu_geometry_without_instance_face_expansion() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: bedrock_render::Dimension::Overworld,
        };
        let mut blocks = Preview3dChunkBlocks::test_cubes(&[[1, 63, 1], [2, 63, 1]]);
        let shape = Arc::new(Preview3dDetailShape::from_cuboids(vec![
            Preview3dCuboid::new([0.25, 0.0, 0.25], [0.75, 1.5, 0.75]),
        ]));
        for x in 3..9 {
            blocks.detail_blocks.push(Preview3dDetailBlock {
                key: BlockKey { x, y: 63, z: 1 },
                normalized_name: Arc::from("minecraft:oak_fence"),
                neighbor_model: Preview3dNeighborModel::None,
                stair_state: None,
                shape: shape.clone(),
                color: [1.0; 4],
                material: Arc::from("minecraft:oak_fence"),
            });
        }
        for (x, water, surface_only) in [
            (1, true, false),
            (2, true, false),
            (3, false, false),
            (4, true, true),
        ] {
            let key = BlockKey { x, y: 63, z: 2 };
            let fluid = Preview3dFluidBlock {
                key,
                color: [0.5, 0.5, 1.0, 0.7],
                material: Arc::from(if water {
                    "minecraft:water"
                } else {
                    "minecraft:lava"
                }),
                face_mode: if surface_only {
                    FluidFaceMode::TopOnly
                } else {
                    FluidFaceMode::AllFaces
                },
                height: 0.75,
            };
            if water {
                blocks.water.insert(key);
                blocks.water_blocks.push(fluid);
            } else {
                blocks.lava.insert(key);
                blocks.lava_blocks.push(fluid);
            }
        }
        let mut builder = Preview3dMeshBuilder::new_stream(SlimeChunkBounds {
            dimension: pos.dimension,
            min_chunk_x: 0,
            max_chunk_x: 0,
            min_chunk_z: 0,
            max_chunk_z: 0,
        });
        builder.insert_stream_chunk(pos, blocks);
        let cpu = builder
            .build_stream_faces(&[pos], StreamFaceOptions::default())
            .remove(0)
            .faces;
        let mut expected = cpu
            .iter()
            .map(|face| face.corners.map(|p| p.map(|v| (v as f32).to_bits())))
            .collect::<Vec<_>>();
        let mut actual = Vec::new();
        for band in builder.gpu_cube_bands(pos, None).unwrap() {
            assert!(
                band.faces.len() < cpu.len(),
                "shared models must not expand faces for each instance"
            );
            let templates = band
                .faces
                .iter()
                .enumerate()
                .map(|(index, face)| gpui_3d::VoxelFaceTemplate {
                    vertices: face.corners.map(|p| Vertex {
                        position: Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32),
                        normal: Vec3::Y,
                        uv: gpui_3d::Vec2::ZERO,
                        color: [1.0; 4],
                    }),
                    material_slot: 0,
                    neighbour: band.neighbours[index],
                })
                .collect();
            let mut templates: Vec<gpui_3d::VoxelFaceTemplate> = templates;
            let instance_templates = templates.split_off(band.model_faces);
            let source =
                gpui_3d::VoxelMeshSource::new(band.size, band.origin, band.cells, templates)
                    .unwrap()
                    .with_instances(band.classes, instance_templates, band.instances)
                    .unwrap()
                    .with_y_range(
                        (band.section_y * 64) as f32,
                        ((band.section_y + 1) * 64) as f32,
                    )
                    .unwrap();
            let mesh = Mesh::from_gpu_voxels(source);
            actual.extend(
                mesh.vertices()
                    .chunks_exact(4)
                    .filter(|q| q[0].color[3] > 0.0)
                    .map(|q| {
                        std::array::from_fn(|i| {
                            [
                                q[i].position.x.to_bits(),
                                q[i].position.y.to_bits(),
                                q[i].position.z.to_bits(),
                            ]
                        })
                    }),
            );
        }
        expected.sort();
        actual.sort();
        assert_eq!(actual, expected);
    }
    #[test]
    fn gpu_model_plan_filters_hidden_instances_before_face_templates() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: bedrock_render::Dimension::Overworld,
        };
        let mut blocks = Preview3dChunkBlocks::default();
        for y in [90, 150] {
            let key = BlockKey { x: 8, y, z: 8 };
            blocks.water.insert(key);
            blocks.water_blocks.push(Preview3dFluidBlock {
                key,
                color: [0.4, 0.6, 0.8, 0.5],
                material: Arc::from("minecraft:water"),
                face_mode: FluidFaceMode::AllFaces,
                height: 1.0,
            });
        }
        let mut builder = Preview3dMeshBuilder::new_stream(SlimeChunkBounds {
            dimension: pos.dimension,
            min_chunk_x: 0,
            max_chunk_x: 0,
            min_chunk_z: 0,
            max_chunk_z: 0,
        });
        builder.insert_stream_chunk(pos, blocks);
        let stored = builder.block_chunks.get(&ChunkKey::from_pos(pos)).unwrap();
        let full = super::gpu_models::ModelPlan::new(&builder, stored, |_, _| false);
        let reduced = super::gpu_models::ModelPlan::new(&builder, stored, |y, _| y == 90);
        assert_eq!(full.records.len(), 2);
        assert_eq!(reduced.records.len(), 1);
        assert_eq!(reduced.records[0].key.y, 150);
        assert!(reduced.faces.len() <= full.faces.len());
        let bands = builder
            .gpu_cube_bands_skipping(pos, None, &std::collections::BTreeSet::from([1]))
            .unwrap();
        assert!(!bands.iter().any(|band| band.section_y == 1));
        assert!(bands.iter().any(|band| band.section_y == 2));
    }

    #[test]
    fn gpu_fluid_halo_defers_unknown_seams_but_retains_known_air() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: bedrock_render::Dimension::Overworld,
        };
        let neighbor = ChunkPos { x: 1, z: 0, ..pos };
        let mut blocks = Preview3dChunkBlocks::default();
        let key = BlockKey { x: 15, y: 88, z: 8 };
        blocks.water.insert(key);
        blocks.water_blocks.push(Preview3dFluidBlock {
            key,
            color: [0.4, 0.6, 0.8, 0.5],
            material: Arc::from("minecraft:water"),
            face_mode: FluidFaceMode::AllFaces,
            height: 1.0,
        });
        let mut builder = Preview3dMeshBuilder::new_stream(SlimeChunkBounds {
            dimension: pos.dimension,
            min_chunk_x: 0,
            max_chunk_x: 1,
            min_chunk_z: 0,
            max_chunk_z: 0,
        });
        builder.insert_stream_chunk(pos, blocks);
        let halo_class = |builder: &Preview3dMeshBuilder| {
            let band = builder
                .gpu_cube_bands(pos, None)
                .unwrap()
                .into_iter()
                .find(|band| band.section_y == 1)
                .unwrap();
            let x = (16 - band.origin.x as i32) as usize;
            let y = (88 - band.origin.y as i32) as usize;
            let z = (8 - band.origin.z as i32) as usize;
            band.classes[(z * band.size[1] as usize + y) * band.size[0] as usize + x]
        };
        assert_eq!(halo_class(&builder) & 12, 12);
        builder.insert_stream_chunk(neighbor, Preview3dChunkBlocks::default());
        assert_eq!(halo_class(&builder) & 12, 0);
    }

    #[test]
    fn completely_hidden_plain_cube_column_never_expands_model_templates() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: bedrock_render::Dimension::Overworld,
        };
        let mut builder = Preview3dMeshBuilder::new_stream(SlimeChunkBounds {
            dimension: pos.dimension,
            min_chunk_x: 0,
            max_chunk_x: 0,
            min_chunk_z: 0,
            max_chunk_z: 0,
        });
        builder.insert_stream_chunk(
            pos,
            Preview3dChunkBlocks::test_cubes(&[[1, 64, 1], [2, 96, 2]]),
        );
        assert!(!builder.gpu_cube_bands(pos, None).unwrap().is_empty());
        let hidden = std::collections::BTreeSet::from([0, 1, 2]);
        assert!(
            builder
                .gpu_cube_bands_skipping(pos, None, &hidden)
                .unwrap()
                .is_empty()
        );
        // A hole in the one-block halo forces regular model planning so no
        // visible boundary face is ever skipped by this shortcut.
        let visible_halo = std::collections::BTreeSet::from([1]);
        assert!(
            !builder
                .gpu_cube_bands_skipping(pos, None, &visible_halo)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn hidden_portal_bands_never_allocate_gpu_voxel_grids_or_instances() {
        let pos = ChunkPos {
            x: -1,
            z: 0,
            dimension: bedrock_render::Dimension::Overworld,
        };
        let mut builder = Preview3dMeshBuilder::new_stream(SlimeChunkBounds {
            dimension: pos.dimension,
            min_chunk_x: -1,
            max_chunk_x: -1,
            min_chunk_z: 0,
            max_chunk_z: 0,
        });
        builder.insert_stream_chunk(
            pos,
            Preview3dChunkBlocks::test_cubes(&[[-15, -72, 3], [-14, 8, 3], [-13, 72, 3]]),
        );
        let original = builder
            .gpu_cube_bands(pos, None)
            .expect("full GPU band plan");
        let kept = builder
            .gpu_cube_bands_skipping(pos, None, &std::collections::BTreeSet::from([0]))
            .expect("reduced GPU band plan");
        assert!(original.iter().any(|band| band.section_y == 0));
        assert!(!kept.iter().any(|band| band.section_y == 0));
        let original_other = original
            .iter()
            .filter(|band| band.section_y != 0)
            .map(|band| band.section_y)
            .collect::<Vec<_>>();
        let kept_bands = kept.iter().map(|band| band.section_y).collect::<Vec<_>>();
        assert_eq!(kept_bands, original_other);
        assert!(kept.len() < original.len());
    }

    #[test]
    fn padded_gpu_classes_match_full_lookups_inside_and_across_columns() {
        let pos = ChunkPos {
            x: -1,
            z: 0,
            dimension: bedrock_render::Dimension::Overworld,
        };
        let neighbor = ChunkPos { x: 0, ..pos };
        let mut builder = Preview3dMeshBuilder::new_stream(SlimeChunkBounds {
            dimension: pos.dimension,
            min_chunk_x: -1,
            max_chunk_x: 0,
            min_chunk_z: 0,
            max_chunk_z: 0,
        });
        builder.insert_stream_chunk(
            pos,
            Preview3dChunkBlocks::test_cubes(&[
                [-16, 63, 5],
                [-15, 63, 5],
                [-1, 63, 5],
                [-1, 64, 5],
                [-8, 65, 6],
            ]),
        );
        builder.insert_stream_chunk(
            neighbor,
            Preview3dChunkBlocks::test_cubes(&[[0, 63, 5], [0, 64, 5]]),
        );
        for band in builder.gpu_cube_bands(pos, None).unwrap() {
            assert!(
                band.model_faces > 0,
                "opaque cubes must use native GPU cells"
            );
            assert!(
                band.cells.iter().any(|&cell| cell != 0 && cell != u32::MAX),
                "cube geometry must not be sent through six-face instance jobs"
            );
            for z in 0..band.size[2] {
                for y in 0..band.size[1] {
                    for x in 0..band.size[0] {
                        let key = BlockKey {
                            x: pos.x * 16 + band.origin.x as i32 + x as i32,
                            y: band.origin.y as i32 + y as i32,
                            z: pos.z * 16 + band.origin.z as i32 + z as i32,
                        };
                        let expected = match builder.block_class_at(key) {
                            Some(Preview3dBlockClass::Opaque) => 1,
                            Some(Preview3dBlockClass::TransparentGlass) => 2,
                            Some(Preview3dBlockClass::Water) => 4,
                            Some(Preview3dBlockClass::Lava) => 8,
                            _ => 0,
                        } | if builder.ambient_occludes_at(key) {
                            16
                        } else {
                            0
                        };
                        let index = ((z * band.size[1] + y) * band.size[0] + x) as usize;
                        assert_eq!(band.classes[index], expected, "cell {key:?}");
                        // Opaque cells either contain a model index or the
                        // non-emitting occluder sentinel; other classes do not
                        // create cube geometry.
                        assert_eq!(band.cells[index] != 0, expected & 1 != 0);
                    }
                }
            }
        }
    }

    #[test]
    fn gpu_cube_cells_match_cpu_faces_at_chunk_and_height_boundaries() {
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: bedrock_render::Dimension::Overworld,
        };
        let neighbor = ChunkPos { x: 1, ..pos };
        let mut builder = Preview3dMeshBuilder::new_stream(SlimeChunkBounds {
            dimension: pos.dimension,
            min_chunk_x: 0,
            max_chunk_x: 1,
            min_chunk_z: 0,
            max_chunk_z: 0,
        });
        builder.insert_stream_chunk(
            pos,
            Preview3dChunkBlocks::test_cubes(&[
                [15, -1, 8],
                [15, 0, 8],
                [15, 63, 8],
                [15, 64, 8],
                [3, 128, 3],
            ]),
        );
        builder.insert_stream_chunk(neighbor, Preview3dChunkBlocks::test_cubes(&[[16, 63, 8]]));
        let mut expected = builder
            .build_stream_faces(&[pos], StreamFaceOptions::default())
            .remove(0)
            .faces
            .into_iter()
            .map(|face| face.corners.map(|p| p.map(|v| v as i32)))
            .collect::<Vec<_>>();
        let mut actual = Vec::new();
        for band in builder.gpu_cube_bands(pos, None).unwrap() {
            let templates = band
                .faces
                .iter()
                .enumerate()
                .map(|(index, face)| gpui_3d::VoxelFaceTemplate {
                    vertices: face.corners.map(|p| Vertex {
                        position: Vec3::new(p[0] as f32, p[1] as f32, p[2] as f32),
                        normal: Vec3::Y,
                        uv: gpui_3d::Vec2::ZERO,
                        color: [1.0; 4],
                    }),
                    material_slot: 0,
                    neighbour: band.neighbours[index],
                })
                .collect();
            let mut templates: Vec<gpui_3d::VoxelFaceTemplate> = templates;
            let instance_templates = templates.split_off(band.model_faces);
            let source =
                gpui_3d::VoxelMeshSource::new(band.size, band.origin, band.cells, templates)
                    .unwrap()
                    .with_instances(band.classes, instance_templates, band.instances)
                    .unwrap()
                    .with_y_range(
                        (band.section_y * 64) as f32,
                        ((band.section_y + 1) * 64) as f32,
                    )
                    .unwrap();
            let mesh = Mesh::from_gpu_voxels(source);
            actual.extend(
                mesh.vertices()
                    .chunks_exact(4)
                    .filter(|quad| quad[0].color[3] > 0.0)
                    .map(|quad| {
                        std::array::from_fn(|i| {
                            [
                                quad[i].position.x as i32,
                                quad[i].position.y as i32,
                                quad[i].position.z as i32,
                            ]
                        })
                    }),
            );
        }
        expected.sort();
        actual.sort();
        assert_eq!(actual, expected);
        let cpu = builder.build_stream_faces(
            &[pos],
            StreamFaceOptions {
                selections: Some(&FxHashMap::default()),
                gpu_cubes: Some(&FxHashSet::from_iter([pos])),
                ..Default::default()
            },
        );
        assert!(cpu.iter().all(|chunk| chunk.faces.is_empty()));
        let selection = Preview3dBandSelection {
            retained: std::collections::HashSet::from([-1, 0, 1, 2]),
            affected: vec![(1, 1)],
            band_height: 64,
        };
        assert_eq!(
            builder
                .gpu_cube_bands(pos, Some(&selection))
                .unwrap()
                .iter()
                .map(|band| band.section_y)
                .collect::<Vec<_>>(),
            [1]
        );
    }
}

impl Preview3dMeshBuilder {
    pub(in crate::ui::window::map_viewer) fn gpu_cube_bands(
        &self,
        pos: ChunkPos,
        selection: Option<&Preview3dBandSelection>,
    ) -> Option<Vec<GpuCubeBand>> {
        self.gpu_cube_bands_skipping(pos, selection, &std::collections::BTreeSet::new())
    }

    /// Skip unreachable 64-block bands before allocating GPU voxel grids,
    /// face templates or instance records. Visibility is conservative and
    /// rechecked when a deferred band is promoted by the streaming worker.
    pub(in crate::ui::window::map_viewer) fn gpu_cube_bands_skipping(
        &self,
        pos: ChunkPos,
        selection: Option<&Preview3dBandSelection>,
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
                        let class = if unknown_column {
                            4 | 8
                        } else {
                            match block_class {
                                Some(Preview3dBlockClass::Opaque) => 1,
                                Some(Preview3dBlockClass::TransparentGlass) => 2,
                                Some(Preview3dBlockClass::Water) => 4,
                                Some(Preview3dBlockClass::Lava) => 8,
                                _ => 0,
                            }
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
