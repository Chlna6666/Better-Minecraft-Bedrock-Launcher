use super::*;
#[test]
fn gpu_models_and_fluids_match_cpu_geometry_without_instance_face_expansion() {
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: bedrock_world::Dimension::Overworld,
    };
    let mut blocks = ChunkBlocks::test_cubes(&[[1, 63, 1], [2, 63, 1]]);
    let shape = Arc::new(DetailShape::from_cuboids(vec![Cuboid::new(
        [0.25, 0.0, 0.25],
        [0.75, 1.5, 0.75],
    )]));
    for x in 3..9 {
        blocks.detail_blocks.push(DetailBlock {
            key: BlockKey { x, y: 63, z: 1 },
            normalized_name: Arc::from("minecraft:oak_fence"),
            neighbor_model: NeighborModel::None,
            stair_state: None,
            shape: shape.clone(),
            color: [1.0; 4],
            material: Arc::from("minecraft:oak_fence"),
        });
    }
    for (x, water) in [(1, true), (2, true), (3, false), (4, true)] {
        let key = BlockKey { x, y: 63, z: 2 };
        let fluid = FluidBlock {
            key,
            color: [0.5, 0.5, 1.0, 0.7],
            material: Arc::from(if water {
                "minecraft:water"
            } else {
                "minecraft:lava"
            }),
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
    let mut builder = SceneBuilder::new_stream(SlimeChunkBounds {
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
        let source = gpui_3d::VoxelMeshSource::new(band.size, band.origin, band.cells, templates)
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
        dimension: bedrock_world::Dimension::Overworld,
    };
    let mut blocks = ChunkBlocks::default();
    for y in [90, 150] {
        let key = BlockKey { x: 8, y, z: 8 };
        blocks.water.insert(key);
        blocks.water_blocks.push(FluidBlock {
            key,
            color: [0.4, 0.6, 0.8, 0.5],
            material: Arc::from("minecraft:water"),
            height: 1.0,
        });
    }
    let mut builder = SceneBuilder::new_stream(SlimeChunkBounds {
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
        dimension: bedrock_world::Dimension::Overworld,
    };
    let neighbor = ChunkPos { x: 1, z: 0, ..pos };
    let mut blocks = ChunkBlocks::default();
    let key = BlockKey { x: 15, y: 88, z: 8 };
    blocks.water.insert(key);
    blocks.water_blocks.push(FluidBlock {
        key,
        color: [0.4, 0.6, 0.8, 0.5],
        material: Arc::from("minecraft:water"),
        height: 1.0,
    });
    let mut builder = SceneBuilder::new_stream(SlimeChunkBounds {
        dimension: pos.dimension,
        min_chunk_x: 0,
        max_chunk_x: 1,
        min_chunk_z: 0,
        max_chunk_z: 0,
    });
    builder.insert_stream_chunk(pos, blocks);
    let halo_class = |builder: &SceneBuilder| {
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
    builder.insert_stream_chunk(neighbor, ChunkBlocks::default());
    assert_eq!(halo_class(&builder) & 12, 0);
}

#[test]
fn completely_hidden_plain_cube_column_never_expands_model_templates() {
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: bedrock_world::Dimension::Overworld,
    };
    let mut builder = SceneBuilder::new_stream(SlimeChunkBounds {
        dimension: pos.dimension,
        min_chunk_x: 0,
        max_chunk_x: 0,
        min_chunk_z: 0,
        max_chunk_z: 0,
    });
    builder.insert_stream_chunk(pos, ChunkBlocks::test_cubes(&[[1, 64, 1], [2, 96, 2]]));
    assert!(!builder.gpu_cube_bands(pos, None).unwrap().is_empty());
    let hidden = std::collections::BTreeSet::from([0, 1, 2]);
    assert!(
        builder
            .gpu_cube_bands_skipping(pos, None, &hidden)
            .unwrap()
            .is_empty()
    );
    // A visible band with geometry must survive the preflight.
    let hidden_halo = std::collections::BTreeSet::from([0, 2]);
    let visible = builder
        .gpu_cube_bands_skipping(pos, None, &hidden_halo)
        .unwrap();
    assert_eq!(
        visible
            .iter()
            .map(|band| band.section_y)
            .collect::<Vec<_>>(),
        [1]
    );
}

#[test]
fn hidden_portal_bands_never_allocate_gpu_voxel_grids_or_instances() {
    let pos = ChunkPos {
        x: -1,
        z: 0,
        dimension: bedrock_world::Dimension::Overworld,
    };
    let mut builder = SceneBuilder::new_stream(SlimeChunkBounds {
        dimension: pos.dimension,
        min_chunk_x: -1,
        max_chunk_x: -1,
        min_chunk_z: 0,
        max_chunk_z: 0,
    });
    builder.insert_stream_chunk(
        pos,
        ChunkBlocks::test_cubes(&[[-15, -72, 3], [-14, 8, 3], [-13, 72, 3]]),
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
        dimension: bedrock_world::Dimension::Overworld,
    };
    let neighbor = ChunkPos { x: 0, ..pos };
    let mut builder = SceneBuilder::new_stream(SlimeChunkBounds {
        dimension: pos.dimension,
        min_chunk_x: -1,
        max_chunk_x: 0,
        min_chunk_z: 0,
        max_chunk_z: 0,
    });
    builder.insert_stream_chunk(
        pos,
        ChunkBlocks::test_cubes(&[
            [-16, 63, 5],
            [-15, 63, 5],
            [-1, 63, 5],
            [-1, 64, 5],
            [-8, 65, 6],
        ]),
    );
    builder.insert_stream_chunk(neighbor, ChunkBlocks::test_cubes(&[[0, 63, 5], [0, 64, 5]]));
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
                    let column = ChunkKey::from_block(key);
                    let expected = match builder.block_class_at(key) {
                        Some(BlockClass::Opaque) => 1,
                        Some(BlockClass::TransparentGlass) => 2,
                        Some(BlockClass::Water) => 4,
                        Some(BlockClass::Lava) => 8,
                        // Unknown columns suppress fluid walls until their boundary arrives.
                        None if column != ChunkKey::from_pos(pos)
                            && column != ChunkKey::from_pos(neighbor) =>
                        {
                            4 | 8
                        }
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
        dimension: bedrock_world::Dimension::Overworld,
    };
    let neighbor = ChunkPos { x: 1, ..pos };
    let mut builder = SceneBuilder::new_stream(SlimeChunkBounds {
        dimension: pos.dimension,
        min_chunk_x: 0,
        max_chunk_x: 1,
        min_chunk_z: 0,
        max_chunk_z: 0,
    });
    builder.insert_stream_chunk(
        pos,
        ChunkBlocks::test_cubes(&[
            [15, -1, 8],
            [15, 0, 8],
            [15, 63, 8],
            [15, 64, 8],
            [3, 128, 3],
        ]),
    );
    builder.insert_stream_chunk(neighbor, ChunkBlocks::test_cubes(&[[16, 63, 8]]));
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
        let source = gpui_3d::VoxelMeshSource::new(band.size, band.origin, band.cells, templates)
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
    let selection = BandSelection {
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
