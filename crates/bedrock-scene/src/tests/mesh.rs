use super::support::*;
use super::*;
use bedrock_block_model::model_family::canonical_block_name;

#[test]
fn file_export_does_not_truncate_faces_at_preview_gpu_budget() {
    let bounds = SlimeChunkBounds {
        dimension: bedrock_world::Dimension::Overworld,
        min_chunk_x: 0,
        max_chunk_x: 0,
        min_chunk_z: 0,
        max_chunk_z: 0,
    };
    let mut builder = SceneBuilder::new(bounds, 0);
    assert_eq!(builder.face_limit(1), 1);
    builder.export_geometry = true;
    assert_eq!(builder.face_limit(1), usize::MAX);
    let mut budget = FaceBudget::new(builder.face_limit(1));
    let mut faces = Vec::new();
    for _ in 0..128 {
        budget.push_or_omit(
            &mut faces,
            Face {
                corners: [[0.0; 3]; 4],
                color: [1.0; 4],
                shade: 1.0,
                normal: [0, 1, 0],
                material: Arc::<str>::from("minecraft_stone_up"),
                uv: None,
            },
        );
    }
    assert_eq!(faces.len(), 128);
    assert_eq!(budget.omitted_faces, 0);
}

#[test]
fn preview_face_lighting_preserves_linear_rgb_ratios_for_all_candidates() {
    let decode = |encoded: f32| {
        if encoded <= 0.04045 {
            encoded / 12.92
        } else {
            ((encoded + 0.055) / 1.055).powf(2.4)
        }
    };
    for candidate in bedrock_voxel::default_block_candidates() {
        for rgb in [candidate.top_color, candidate.side_color] {
            let source = rgb.map(|channel| f32::from(channel) / 255.0);
            for factor in [0.56, 0.72, 0.86, 1.0] {
                let result = shade_preview_color(source, factor);
                assert_eq!(source[3], result[3]);
                for channel in 0..3 {
                    assert!(
                        (decode(result[channel]) - decode(source[channel]) * factor).abs() < 0.009,
                        "{} channel {channel}, factor {factor}",
                        candidate.state.name
                    );
                }
            }
        }
    }
}

#[test]
fn legacy_variant_block_states_resolve_to_canonical_block_materials() {
    let red_flower = test_block_state_with_tag(
        "minecraft:red_flower",
        "flower_type",
        NbtTag::String("blue_orchid".to_string()),
    );
    let carpet = test_block_state_with_tag(
        "minecraft:carpet",
        "color",
        NbtTag::String("red".to_string()),
    );
    let numeric_carpet = test_block_state_with_tag("minecraft:carpet", "color", NbtTag::Byte(11));
    let spruce_fence = test_block_state_with_tag(
        "minecraft:fence",
        "wood_type",
        NbtTag::String("spruce".to_string()),
    );
    let brick_wall = test_block_state_with_tag(
        "minecraft:cobblestone_wall",
        "wall_block_type",
        NbtTag::String("brick".to_string()),
    );
    let end_brick_wall = test_block_state_with_tag(
        "minecraft:cobblestone_wall",
        "wall_block_type",
        NbtTag::String("end_brick".to_string()),
    );
    let blue_shulker =
        test_block_state_with_tag("minecraft:shulker_box", "color", NbtTag::Byte(11));

    assert_eq!(
        canonical_block_name(&block_state_query(&red_flower)),
        "minecraft:blue_orchid"
    );
    assert_eq!(
        canonical_block_name(&block_state_query(&carpet)),
        "minecraft:red_carpet"
    );
    assert_eq!(
        material_block_name(&carpet, BlockClass::DetailOpaque).as_ref(),
        "minecraft:red_wool"
    );
    assert_eq!(
        canonical_block_name(&block_state_query(&numeric_carpet)),
        "minecraft:blue_carpet"
    );
    assert_eq!(
        material_block_name(&numeric_carpet, BlockClass::DetailOpaque).as_ref(),
        "minecraft:blue_wool"
    );
    assert_eq!(
        canonical_block_name(&block_state_query(&spruce_fence)),
        "minecraft:spruce_fence"
    );
    assert_eq!(
        material_block_name(&spruce_fence, BlockClass::DetailOpaque).as_ref(),
        "minecraft:spruce_planks"
    );
    assert_eq!(
        canonical_block_name(&block_state_query(&brick_wall)),
        "minecraft:brick_wall"
    );
    assert_eq!(
        material_block_name(&brick_wall, BlockClass::DetailOpaque).as_ref(),
        "minecraft:bricks"
    );
    assert_eq!(
        canonical_block_name(&block_state_query(&end_brick_wall)),
        "minecraft:end_brick_wall"
    );
    assert_eq!(
        material_block_name(&end_brick_wall, BlockClass::DetailOpaque).as_ref(),
        "minecraft:end_bricks"
    );
    assert_eq!(
        canonical_block_name(&block_state_query(&blue_shulker)),
        "minecraft:blue_shulker_box"
    );
    assert_eq!(
        block_class("minecraft:double_plant"),
        BlockClass::DetailOpaque
    );
}

#[test]
fn map_viewer_preview_3d_uses_the_full_single_frame_gpu_face_budget() {
    assert_eq!(TOTAL_FACE_BUDGET, FACE_BUDGET);
}

#[test]
fn map_viewer_preview_3d_block_records_do_not_reject_large_chunks() {
    let mut budget = BlockBudget::unbounded();

    for _ in 0..(16 * 1024 + 1) {
        assert!(budget.try_take());
    }
}

#[test]
fn map_viewer_preview_3d_merges_exposed_faces_across_chunk_boundaries() {
    let bounds = SlimeChunkBounds {
        dimension: bedrock_world::Dimension::Overworld,
        min_chunk_x: 0,
        max_chunk_x: 1,
        min_chunk_z: 0,
        max_chunk_z: 0,
    };
    let mut merger = FaceMerger::new();
    for x in 0..32 {
        merger.push(
            BlockKey { x, y: 64, z: 0 },
            FACE_DEFINITIONS[0],
            [0.4, 0.5, 0.6, 1.0],
            test_material(),
            0.0,
        );
    }
    let mut faces = Vec::new();
    let mut budget = FaceBudget::new(FACE_BUDGET);
    merger.emit_into(&mut faces, &mut budget);
    let mesh = test_mesh_with_faces(faces, bounds);

    assert_eq!(mesh.face_count, 1);
    assert_eq!(test_mesh(&mesh).vertices().len(), 4);
}

#[test]
fn map_viewer_preview_3d_merges_opaque_faces_after_cross_chunk_culling() {
    let bounds = SlimeChunkBounds {
        dimension: bedrock_world::Dimension::Overworld,
        min_chunk_x: 0,
        max_chunk_x: 1,
        min_chunk_z: 0,
        max_chunk_z: 0,
    };
    let mut builder = SceneBuilder::new(bounds, 0);
    for chunk_x in 0..=1 {
        let blocks = (chunk_x * 16..chunk_x * 16 + 16)
            .map(|x| (BlockKey { x, y: 64, z: 0 }, [0.4, 0.5, 0.6, 1.0]))
            .map(|(block, color)| test_block_record(block, color, test_material()))
            .collect::<Vec<_>>();
        builder.block_chunks.insert(
            ChunkKey { x: chunk_x, z: 0 },
            ChunkBlocks {
                occupied: blocks.iter().map(|block| block.key).collect(),
                opaque_blocks: blocks,
                min_y: 64,
                max_y: 64,
                min_x: chunk_x * 16,
                max_x: chunk_x * 16 + 15,
                min_z: 0,
                max_z: 0,
                ..ChunkBlocks::default()
            },
        );
    }

    builder
        .rebuild_combined_mesh()
        .unwrap_or_else(|error| panic!("{error}"));
    let mesh = builder.build_mesh();

    assert_eq!(mesh.face_count, 6);
}

#[test]
fn map_viewer_preview_3d_single_water_cell_renders_all_six_faces() {
    let bounds = SlimeChunkBounds {
        dimension: bedrock_world::Dimension::Overworld,
        min_chunk_x: 0,
        max_chunk_x: 0,
        min_chunk_z: 0,
        max_chunk_z: 0,
    };
    let mut builder = SceneBuilder::new(bounds, 0);
    let block = BlockKey { x: 8, y: 64, z: 8 };
    builder.insert_prepared(PreparedChunk {
        chunk_key: ChunkKey { x: 0, z: 0 },
        subchunk_count: 1,
        internally_culled_blocks: 0,
        blocks: Some(ChunkBlocks {
            water: FxHashSet::from_iter([block]),
            water_blocks: vec![FluidBlock {
                key: block,
                color: test_water_color(),
                material: test_material(),
                height: 1.0,
            }],
            min_y: 64,
            max_y: 64,
            min_x: 8,
            max_x: 8,
            min_z: 8,
            max_z: 8,
            ..ChunkBlocks::default()
        }),
    });

    builder
        .rebuild_combined_mesh()
        .unwrap_or_else(|error| panic!("{error}"));
    let mesh = builder.build_mesh();

    assert_eq!(mesh.water_face_count, 6);
    assert_eq!(test_chunk_mesh(&mesh).ranges.water.count, 36);
}

#[test]
fn map_viewer_preview_3d_filters_fully_hidden_internal_blocks() {
    let mut opaque_blocks = Vec::new();
    let mut occupied = FxHashSet::default();
    for y in 0..3 {
        for z in 0..3 {
            for x in 0..3 {
                let block = BlockKey { x, y, z };
                occupied.insert(block);
                opaque_blocks.push(test_block_record(
                    block,
                    [0.4, 0.5, 0.6, 1.0],
                    test_material(),
                ));
            }
        }
    }
    let mut blocks = ChunkBlocks {
        occupied,
        opaque_blocks,
        min_y: 0,
        max_y: 2,
        min_x: 0,
        max_x: 2,
        min_z: 0,
        max_z: 2,
        ..ChunkBlocks::default()
    };

    let culled = filter_internal_block_records(&mut blocks);

    assert_eq!(culled, 1);
    assert_eq!(blocks.opaque_blocks.len(), 26);
    assert!(
        !blocks
            .opaque_blocks
            .iter()
            .any(|block| block.key == BlockKey { x: 1, y: 1, z: 1 })
    );
}

#[test]
fn map_viewer_preview_3d_keeps_full_grass_blocks_solid() {
    assert_eq!(block_class("minecraft:grass"), BlockClass::Opaque);
    assert_eq!(block_class("minecraft:grass_block"), BlockClass::Opaque);
    assert_eq!(
        block_class("minecraft:short_grass"),
        BlockClass::DetailOpaque
    );
}

#[test]
fn map_viewer_preview_3d_single_block_has_six_faces() {
    let block = BlockKey { x: 0, y: 0, z: 0 };
    let mut faces = Vec::new();
    for face in FACE_DEFINITIONS {
        faces.push(block_face(block, face, [0.4, 0.5, 0.6, 1.0]));
    }
    assert_eq!(faces.len(), 6);
}

#[test]
fn map_viewer_preview_3d_adjacent_blocks_cull_shared_face() {
    let bounds = SlimeChunkBounds {
        dimension: bedrock_world::Dimension::Overworld,
        min_chunk_x: 0,
        max_chunk_x: 0,
        min_chunk_z: 0,
        max_chunk_z: 0,
    };
    let occupied =
        FxHashSet::from_iter([BlockKey { x: 0, y: 0, z: 0 }, BlockKey { x: 1, y: 0, z: 0 }]);
    let mut faces = Vec::new();
    let mut culled = 0usize;
    for block in occupied.iter().copied() {
        for face in FACE_DEFINITIONS {
            let neighbor = BlockKey {
                x: block.x + face.normal[0],
                y: block.y + face.normal[1],
                z: block.z + face.normal[2],
            };
            if occupied.contains(&neighbor) {
                culled = culled.saturating_add(1);
            } else {
                faces.push(block_face(block, face, [0.5, 0.5, 0.5, 1.0]));
            }
        }
    }
    let mesh = test_mesh_with_faces(faces, bounds);

    assert_eq!(culled, 2);
    assert_eq!(mesh.face_count, 10);
}

#[test]
fn map_viewer_preview_3d_uses_full_subchunks_when_column_samples_exist() {
    let mut indices = vec![0_u16; 16 * 16 * 16];
    indices[bedrock_world::block_storage_index(0, 0, 0)] = 1;
    indices[bedrock_world::block_storage_index(0, 1, 0)] = 1;
    let mut subchunks = BTreeMap::new();
    subchunks.insert(
        0,
        bedrock_world::SubChunk {
            y: 0,
            format: bedrock_world::SubChunkFormat::Paletted {
                version: 8,
                storages: vec![::bedrock_world::BlockPalette::with_unpacked_indices(
                    vec![
                        test_block_state("minecraft:air"),
                        test_block_state("minecraft:sand"),
                    ],
                    indices,
                    Some(vec![4094, 2]),
                )],
            },
        },
    );
    let mut column_samples = bedrock_world::surface::TerrainColumnSamples::new();
    column_samples.set(
        0,
        0,
        bedrock_world::surface::TerrainColumnSample {
            surface_y: 1,
            surface_block_state: test_block_state("minecraft:sand"),
            relief_y: 1,
            relief_block_state: test_block_state("minecraft:sand"),
            overlay: None,
            water: None,
            biome: None,
            source: bedrock_world::surface::TerrainSampleSource::Subchunk,
        },
    );
    let chunk = ChunkData {
        pos: ChunkPos {
            x: 0,
            z: 0,
            dimension: bedrock_world::Dimension::Overworld,
        },
        is_loaded: true,
        height_map: None,
        legacy_biomes: None,
        legacy_biome_colors: None,
        biome_data: BTreeMap::new(),
        subchunks,
        block_entities: Vec::new(),
        legacy_terrain: None,
        column_samples: Some(column_samples),
        version: bedrock_world::ChunkVersion::New,
    };

    let blocks =
        collect::collect(&chunk, None, None, |_| true).unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(blocks.occupied.len(), 2);
    assert_eq!(blocks.opaque_blocks.len(), 2);
    assert!(blocks.occupied.contains(&BlockKey { x: 0, y: 0, z: 0 }));
    assert!(blocks.occupied.contains(&BlockKey { x: 0, y: 1, z: 0 }));
}

#[test]
fn map_viewer_preview_3d_uses_primary_water_when_visible_layer_is_detail() {
    let mut water_indices = vec![0_u16; 16 * 16 * 16];
    let mut detail_indices = vec![0_u16; 16 * 16 * 16];
    let block_index = bedrock_world::block_storage_index(0, 0, 0);
    water_indices[block_index] = 1;
    detail_indices[block_index] = 1;
    let mut subchunks = BTreeMap::new();
    subchunks.insert(
        0,
        bedrock_world::SubChunk {
            y: 0,
            format: bedrock_world::SubChunkFormat::Paletted {
                version: 9,
                storages: vec![
                    ::bedrock_world::BlockPalette::with_unpacked_indices(
                        vec![
                            test_block_state("minecraft:air"),
                            test_block_state("minecraft:water"),
                        ],
                        water_indices,
                        Some(vec![4095, 1]),
                    ),
                    ::bedrock_world::BlockPalette::with_unpacked_indices(
                        vec![
                            test_block_state("minecraft:air"),
                            test_block_state("minecraft:seagrass"),
                        ],
                        detail_indices,
                        Some(vec![4095, 1]),
                    ),
                ],
            },
        },
    );
    let chunk = test_render_chunk_with_subchunks(subchunks);

    let blocks =
        collect::collect(&chunk, None, None, |_| true).unwrap_or_else(|error| panic!("{error}"));

    assert_eq!(blocks.water_blocks.len(), 1);
    assert!(blocks.water.contains(&BlockKey { x: 0, y: 0, z: 0 }));
    assert!(blocks.opaque_blocks.is_empty());
}

#[test]
fn map_viewer_preview_3d_detail_blocks_generate_non_full_cube_faces() {
    let palette = RenderPalette::default();
    let block = BlockKey { x: 0, y: 64, z: 0 };
    let mut builder = SceneBuilder::new(
        SlimeChunkBounds {
            dimension: bedrock_world::Dimension::Overworld,
            min_chunk_x: 0,
            max_chunk_x: 0,
            min_chunk_z: 0,
            max_chunk_z: 0,
        },
        0,
    );
    let mut chunk_builder = StructureChunkBuilder::default();
    let state = test_block_state_with_tag("minecraft:stone_slab", "top_slot_bit", NbtTag::Byte(0));
    push_structure_block(block, Some(&state), None, &palette, &mut chunk_builder)
        .unwrap_or_else(|error| panic!("{error}"));
    builder.insert_prepared(PreparedChunk {
        chunk_key: ChunkKey { x: 0, z: 0 },
        subchunk_count: 1,
        internally_culled_blocks: 0,
        blocks: Some(chunk_builder.blocks),
    });

    builder
        .rebuild_combined_mesh()
        .unwrap_or_else(|error| panic!("{error}"));
    let mesh = builder.build_mesh();
    let gpu_mesh = test_mesh(&mesh);
    let world_origin_y = mesh
        .chunk_meshes
        .first()
        .map_or(0.0, |chunk_mesh| chunk_mesh.world_origin[1] as f32);
    let highest_y = gpu_mesh
        .vertices()
        .iter()
        .map(|vertex| vertex.position.y + world_origin_y)
        .fold(f32::NEG_INFINITY, f32::max);

    assert_eq!(mesh.face_count, 6);
    assert!((highest_y - 64.5).abs() < 0.001);
}

#[test]
fn map_viewer_preview_3d_glass_panes_use_detail_glass_layer() {
    let palette = RenderPalette::default();
    let block = BlockKey { x: 0, y: 64, z: 0 };
    let mut builder = SceneBuilder::new(
        SlimeChunkBounds {
            dimension: bedrock_world::Dimension::Overworld,
            min_chunk_x: 0,
            max_chunk_x: 0,
            min_chunk_z: 0,
            max_chunk_z: 0,
        },
        0,
    );
    let mut chunk_builder = StructureChunkBuilder::default();
    let state = test_block_state("minecraft:glass_pane");
    push_structure_block(block, Some(&state), None, &palette, &mut chunk_builder)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(chunk_builder.blocks.glass_detail_blocks.len(), 1);
    builder.insert_prepared(PreparedChunk {
        chunk_key: ChunkKey { x: 0, z: 0 },
        subchunk_count: 1,
        internally_culled_blocks: 0,
        blocks: Some(chunk_builder.blocks),
    });

    builder
        .rebuild_combined_mesh()
        .unwrap_or_else(|error| panic!("{error}"));
    let mesh = builder.build_mesh();
    let gpu_mesh = test_chunk_mesh(&mesh);

    assert_eq!(mesh.face_count, 0);
    assert!(mesh.glass_face_count > 6);
    assert_eq!(gpu_mesh.ranges.opaque.count, 0);
    assert!(gpu_mesh.ranges.glass.count > 6);
}

#[test]
fn map_viewer_preview_3d_water_blocks_generate_shell_faces() {
    let bounds = SlimeChunkBounds {
        dimension: bedrock_world::Dimension::Overworld,
        min_chunk_x: 0,
        max_chunk_x: 0,
        min_chunk_z: 0,
        max_chunk_z: 0,
    };
    let water_blocks =
        FxHashSet::from_iter([BlockKey { x: 0, y: 0, z: 0 }, BlockKey { x: 1, y: 0, z: 0 }]);
    let mut water_faces = Vec::new();
    let mut culled = 0usize;
    for block in water_blocks.iter().copied() {
        for face in FACE_DEFINITIONS {
            let neighbor = BlockKey {
                x: block.x + face.normal[0],
                y: block.y + face.normal[1],
                z: block.z + face.normal[2],
            };
            if water_blocks.contains(&neighbor) {
                culled = culled.saturating_add(1);
                continue;
            }
            water_faces.push(block_face(block, face, test_water_color()));
        }
    }
    let mesh = test_mesh_with_layers(Vec::new(), Vec::new(), water_faces, bounds);

    assert_eq!(culled, 2);
    assert_eq!(mesh.face_count, 0);
    assert_eq!(mesh.water_face_count, 10);
    assert_eq!(mesh.water_block_count, 1);
}

#[test]
fn map_viewer_preview_3d_lava_blocks_generate_shell_faces() {
    let lava_blocks =
        FxHashSet::from_iter([BlockKey { x: 0, y: 0, z: 0 }, BlockKey { x: 1, y: 0, z: 0 }]);
    let mut lava_faces = Vec::new();
    let mut culled = 0usize;
    for block in lava_blocks.iter().copied() {
        for face in FACE_DEFINITIONS {
            let neighbor = BlockKey {
                x: block.x + face.normal[0],
                y: block.y + face.normal[1],
                z: block.z + face.normal[2],
            };
            if lava_blocks.contains(&neighbor) {
                culled = culled.saturating_add(1);
                continue;
            }
            lava_faces.push(block_face(block, face, [0.98, 0.32, 0.05, LAVA_ALPHA]));
        }
    }
    let meshes = build_meshes(&[], &[], &[], &lava_faces, [0.0, 0.0, 0.0], 16.0, 16.0, 1)
        .unwrap_or_else(|error| panic!("{error}"));
    let gpu_mesh = meshes
        .first()
        .unwrap_or_else(|| panic!("lava mesh should be generated"));

    assert_eq!(culled, 2);
    assert_eq!(lava_faces.len(), 10);
    assert_eq!(gpu_mesh.ranges.water.count, 60);
    assert!((gpu_mesh.mesh.vertices()[0].color[3] - LAVA_ALPHA).abs() < 0.001);
}

#[test]
fn map_viewer_preview_3d_water_vertices_are_after_opaque_vertices() {
    let bounds = SlimeChunkBounds {
        dimension: bedrock_world::Dimension::Overworld,
        min_chunk_x: 0,
        max_chunk_x: 0,
        min_chunk_z: 0,
        max_chunk_z: 0,
    };
    let opaque = block_face(
        BlockKey { x: 7, y: 64, z: 8 },
        FACE_DEFINITIONS[0],
        [0.3, 0.7, 0.2, 1.0],
    );
    let water = block_face(
        BlockKey { x: 8, y: 64, z: 8 },
        FACE_DEFINITIONS[0],
        test_water_color(),
    );
    let mesh = test_mesh_with_layers(vec![opaque], Vec::new(), vec![water], bounds);
    let gpu_chunk_mesh = test_chunk_mesh(&mesh);
    let gpu_mesh = gpu_chunk_mesh.mesh.as_ref();
    let vertices = gpu_mesh.vertices();
    let indices = gpu_mesh.indices();
    let ranges = gpu_chunk_mesh.ranges;

    assert_eq!(vertices.len(), 8);
    assert_eq!(ranges.opaque.start, 0);
    assert_eq!(ranges.opaque.count, 6);
    assert_eq!(ranges.glass.start, 6);
    assert_eq!(ranges.glass.count, 0);
    assert_eq!(ranges.water.start, 6);
    assert_eq!(ranges.water.count, 6);
    let opaque_vertex = usize::try_from(indices[ranges.opaque.start as usize])
        .expect("opaque vertex index should fit usize");
    let water_vertex = usize::try_from(indices[ranges.water.start as usize])
        .expect("water vertex index should fit usize");
    assert_eq!(vertices[opaque_vertex].color[3], 1.0);
    let water_color = vertices[water_vertex].color;
    assert!((water_color[3] - WATER_ALPHA).abs() < 0.001);
    assert!(water_color[2] > water_color[1]);
    assert!(water_color[1] > water_color[0]);
}

#[test]
fn map_viewer_preview_3d_gpu_mesh_groups_material_ranges_globally() {
    let bounds = SlimeChunkBounds {
        dimension: bedrock_world::Dimension::Overworld,
        min_chunk_x: 0,
        max_chunk_x: 1,
        min_chunk_z: 0,
        max_chunk_z: 0,
    };
    let opaque = block_face(
        BlockKey { x: 7, y: 64, z: 8 },
        FACE_DEFINITIONS[0],
        [0.3, 0.7, 0.2, 1.0],
    );
    let glass = block_face(
        BlockKey { x: 8, y: 64, z: 8 },
        FACE_DEFINITIONS[0],
        [0.8, 0.9, 1.0, GLASS_ALPHA],
    );
    let water = block_face(
        BlockKey { x: 24, y: 64, z: 8 },
        FACE_DEFINITIONS[0],
        test_water_color(),
    );
    let mesh = test_mesh_with_layers(vec![opaque], vec![glass], vec![water], bounds);
    let gpu_mesh = test_chunk_mesh(&mesh);

    assert_eq!(gpu_mesh.mesh.vertices().len(), 12);
    assert_eq!(gpu_mesh.ranges.opaque.start, 0);
    assert_eq!(gpu_mesh.ranges.opaque.count, 6);
    assert_eq!(gpu_mesh.ranges.glass.start, 6);
    assert_eq!(gpu_mesh.ranges.glass.count, 6);
    assert_eq!(gpu_mesh.ranges.water.start, 12);
    assert_eq!(gpu_mesh.ranges.water.count, 6);
}

#[test]
fn map_viewer_preview_3d_gpu_meshes_split_vertex_counts_over_budget() {
    assert!(GPU_VERTEX_BUDGET * std::mem::size_of::<Vertex>() < 256 * 1024 * 1024);
    let faces = vec![
        block_face(
            BlockKey { x: 0, y: 0, z: 0 },
            FACE_DEFINITIONS[0],
            [0.3, 0.7, 0.2, 1.0],
        );
        GPU_VERTEX_BUDGET / 4 + 1
    ];

    let meshes = build_meshes(&faces, &[], &[], &[], [0.0, 0.0, 0.0], 16.0, 16.0, 0)
        .unwrap_or_else(|error| panic!("{error}"));
    let vertex_count = meshes
        .iter()
        .map(|mesh| mesh.mesh.vertices().len())
        .sum::<usize>();

    assert_eq!(meshes.len(), 2);
    assert_eq!(vertex_count, faces.len() * 4);
    assert!(
        meshes
            .iter()
            .all(|mesh| mesh.mesh.vertices().len() <= GPU_VERTEX_BUDGET)
    );
}

#[test]
fn map_viewer_preview_3d_rebases_large_world_coordinates_before_gpu_upload() {
    let world_origin = [54_240_342, 64, -54_240_342];
    let face = block_face(
        BlockKey {
            x: world_origin[0],
            y: world_origin[1],
            z: world_origin[2],
        },
        FACE_DEFINITIONS[0],
        [0.3, 0.7, 0.2, 1.0],
    );

    let meshes = build_meshes_at_world_origin(
        &[face],
        &[],
        &[],
        &[],
        world_origin,
        [0.5, 0.5, 0.5],
        16.0,
        16.0,
        0,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let mesh = meshes
        .first()
        .unwrap_or_else(|| panic!("mesh should exist"));
    let positions = mesh
        .mesh
        .vertices()
        .iter()
        .map(|vertex| position_array(vertex.position))
        .collect::<Vec<_>>();

    assert_eq!(mesh.world_origin, world_origin);
    assert!(positions.iter().any(|position| position[0] == 0.0));
    assert!(positions.iter().any(|position| position[0] == 1.0));
    assert!(positions.iter().any(|position| position[2] == 0.0));
    assert!(positions.iter().any(|position| position[2] == 1.0));
}
