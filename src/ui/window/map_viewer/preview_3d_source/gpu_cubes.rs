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
                surface_only,
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
        let cpu = builder.build_stream_faces(&[pos]).remove(0).faces;
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
            let source = gpui_3d::VoxelMeshSource::new(band.size, band.origin, band.cells, vec![])
                .unwrap()
                .with_instances(band.classes, templates, band.instances)
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
                [-16, 63, 5], [-15, 63, 5], [-1, 63, 5],
                [-1, 64, 5], [-8, 65, 6],
            ]),
        );
        builder.insert_stream_chunk(
            neighbor,
            Preview3dChunkBlocks::test_cubes(&[[0, 63, 5], [0, 64, 5]]),
        );
        for band in builder.gpu_cube_bands(pos, None).unwrap() {
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
                        } | if builder.ambient_occludes_at(key) { 16 } else { 0 };
                        let index = ((z * band.size[1] + y) * band.size[0] + x) as usize;
                        assert_eq!(band.classes[index], expected, "cell {key:?}");
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
            .build_stream_faces(&[pos])
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
            let source = gpui_3d::VoxelMeshSource::new(band.size, band.origin, band.cells, vec![])
                .unwrap()
                .with_instances(band.classes, templates, band.instances)
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
        let cpu = builder.build_stream_faces_with_gpu_cubes(
            &[pos],
            &std::collections::HashMap::new(),
            &std::collections::HashSet::from([pos]),
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
        let plan = super::gpu_models::ModelPlan::new(self, blocks);
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
                        // two global ChunkKey -> HashMap probes per cell (one
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
                        let class = match block_class {
                            Some(Preview3dBlockClass::Opaque) => 1,
                            Some(Preview3dBlockClass::TransparentGlass) => 2,
                            Some(Preview3dBlockClass::Water) => 4,
                            Some(Preview3dBlockClass::Lava) => 8,
                            _ => 0,
                        };
                        classes[((z * size[1] + y) * size[0] + x) as usize] =
                            class | if ao { 16 } else { 0 };
                    }
                }
            }
            let mut faces = Vec::new();
            let mut neighbours = Vec::new();
            let mut ranges = HashMap::default();
            let instances = records
                .into_iter()
                .map(|record| gpui_3d::VoxelMeshInstance {
                    position: [
                        (record.key.x - min[0]) as u32,
                        (record.key.y - min[1]) as u32,
                        (record.key.z - min[2]) as u32,
                    ],
                    faces: ranges
                        .entry((record.faces.start, record.faces.end))
                        .or_insert_with(|| {
                            let start = faces.len() as u32;
                            faces.extend_from_slice(
                                &plan.faces[record.faces.start as usize..record.faces.end as usize],
                            );
                            neighbours.extend_from_slice(
                                &plan.neighbours
                                    [record.faces.start as usize..record.faces.end as usize],
                            );
                            start..faces.len() as u32
                        })
                        .clone(),
                    height: record.height,
                    occlusion_mask: record.mask,
                })
                .collect();
            output.push(GpuCubeBand {
                size,
                origin: gpui_3d::Vec3::new(
                    (min[0] - pos.x * 16) as f32,
                    min[1] as f32,
                    (min[2] - pos.z * 16) as f32,
                ),
                cells: vec![0; count],
                classes,
                template_faces: faces.len(),
                faces,
                neighbours,
                model_faces: 0,
                instances,
                section_y: band,
            });
        }
        Some(output)
    }
}
