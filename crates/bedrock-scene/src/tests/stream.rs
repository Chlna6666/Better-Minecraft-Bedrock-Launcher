use super::support::*;
use super::*;

#[test]
fn full_cube_detail_states_emit_faces_in_cpu_and_gpu_streams() {
    for name in [
        "minecraft:furnace",
        "minecraft:observer",
        "minecraft:crafter",
        "minecraft:blue_glazed_terracotta",
        "minecraft:exposed_double_cut_copper_slab",
    ] {
        let mut indices = vec![0u16; 4096];
        indices[::bedrock_world::block::block_storage_index(1, 1, 1)] = 1;
        let data = test_render_chunk_with_subchunks(BTreeMap::from([(
            0,
            ::bedrock_world::SubChunk {
                y: 0,
                format: ::bedrock_world::SubChunkFormat::Paletted {
                    version: 9,
                    storages: vec![::bedrock_world::BlockPalette::with_unpacked_indices(
                        vec![test_block_state("minecraft:air"), test_block_state(name)],
                        indices,
                        None,
                    )],
                },
            },
        )]));
        let blocks = collect::collect(&data, None, None, |_| true).unwrap();
        let mut builder = SceneBuilder::new_stream(SlimeChunkBounds {
            dimension: data.pos.dimension,
            min_chunk_x: 0,
            max_chunk_x: 0,
            min_chunk_z: 0,
            max_chunk_z: 0,
        });
        builder.insert_stream_chunk(data.pos, blocks);
        let cpu = builder.build_stream_faces(&[data.pos], StreamFaceOptions::default());
        assert_eq!(
            cpu.iter().map(|chunk| chunk.faces.len()).sum::<usize>(),
            6,
            "{name}"
        );
        let gpu = builder.gpu_cube_bands(data.pos, None).unwrap();
        assert_eq!(
            gpu.iter().map(|band| band.faces.len()).sum::<usize>(),
            6,
            "{name}"
        );
    }
}

#[test]
fn streaming_face_generation_skips_only_retained_unaffected_bands() {
    let dimension = bedrock_world::Dimension::Overworld;
    let target = ChunkPos {
        x: 0,
        z: 0,
        dimension,
    };
    let neighbor = ChunkPos { x: 1, ..target };
    let mut builder = SceneBuilder::new_stream(SlimeChunkBounds {
        dimension,
        min_chunk_x: 0,
        max_chunk_x: 1,
        min_chunk_z: 0,
        max_chunk_z: 0,
    });
    builder.insert_stream_chunk(target, ChunkBlocks::test_cubes(&[[15, 8, 8], [15, 136, 8]]));
    builder.insert_stream_chunk(neighbor, ChunkBlocks::test_cubes(&[[16, 136, 8]]));
    assert_eq!(
        builder.build_stream_faces(&[target], StreamFaceOptions::default(),)[0]
            .faces
            .len(),
        11
    );
    let selections = FxHashMap::from_iter([(
        target,
        BandSelection {
            affected: vec![(2, 2)],
            retained: std::collections::HashSet::from([0]),
            band_height: 64,
        },
    )]);
    let faces = builder.build_stream_faces(
        &[target],
        StreamFaceOptions {
            selections: Some(&selections),
            ..Default::default()
        },
    );
    assert_eq!(faces[0].faces.len(), 5);
    assert!(
        faces[0]
            .faces
            .iter()
            .flat_map(|face| face.corners)
            .all(|corner| corner[1] >= 136.0)
    );
    let edge = BandSelection {
        affected: vec![(1, 1)],
        retained: std::collections::HashSet::from([0]),
        band_height: 64,
    };
    assert!(
        edge.includes_cube(63),
        "the top face crosses into the next band"
    );
    assert!(!edge.includes_cube(8));
    let missing = BandSelection {
        affected: vec![(2, 2)],
        retained: std::collections::HashSet::new(),
        band_height: 64,
    };
    assert!(
        missing.includes_cube(8),
        "a missing old band must be regenerated"
    );
    let negative = BandSelection {
        affected: vec![(0, 0)],
        retained: std::collections::HashSet::from([-1]),
        band_height: 64,
    };
    assert!(
        negative.includes_cube(-1),
        "the top face crosses the zero band"
    );
}

#[test]
fn stream_faces_remain_visible_until_a_solid_neighbor_arrives() {
    let dimension = bedrock_world::Dimension::Overworld;
    let target = ChunkPos {
        x: 0,
        z: 0,
        dimension,
    };
    let neighbor = ChunkPos { x: 1, ..target };
    let mut builder = SceneBuilder::new_stream(SlimeChunkBounds {
        dimension,
        min_chunk_x: -1,
        max_chunk_x: 1,
        min_chunk_z: -1,
        max_chunk_z: 1,
    });
    let blocks = |x| {
        let block = BlockKey { x, y: 64, z: 8 };
        ChunkBlocks {
            occupied: FxHashSet::from_iter([block]),
            opaque_blocks: vec![test_block_record(block, [1.0; 4], test_material())],
            min_x: x,
            max_x: x,
            min_z: 8,
            max_z: 8,
            min_y: 64,
            max_y: 64,
            ..ChunkBlocks::default()
        }
    };
    builder.insert_stream_chunk(target, blocks(15));
    assert_eq!(
        builder.build_stream_faces(&[target], StreamFaceOptions::default(),)[0]
            .faces
            .len(),
        6
    );
    builder.insert_stream_chunk(neighbor, ChunkBlocks::default());
    assert_eq!(
        builder.build_stream_faces(&[target], StreamFaceOptions::default(),)[0]
            .faces
            .len(),
        6
    );
    builder.insert_stream_chunk(neighbor, blocks(16));
    assert_eq!(
        builder.build_stream_faces(&[target], StreamFaceOptions::default(),)[0]
            .faces
            .len(),
        5
    );
}

#[test]
fn collected_bounds_limit_new_column_neighbor_updates() {
    let pos = ChunkPos {
        x: -1,
        z: -1,
        dimension: bedrock_world::Dimension::Overworld,
    };
    let west = ChunkPos { x: -2, ..pos };
    let east = ChunkPos { x: 0, ..pos };
    let north = ChunkPos { z: -2, ..pos };
    let south = ChunkPos { z: 0, ..pos };
    let mut blocks = ChunkBlocks::default();
    for neighbor in [west, east, north, south] {
        assert!(!blocks.touches_neighbor(pos, neighbor));
    }
    blocks.min_x = -15;
    blocks.max_x = -2;
    blocks.min_z = -15;
    blocks.max_z = -2;
    for neighbor in [west, east, north, south] {
        assert!(!blocks.touches_neighbor(pos, neighbor));
    }
    blocks.min_x = -16;
    blocks.max_z = -1;
    assert!(blocks.touches_neighbor(pos, west));
    assert!(blocks.touches_neighbor(pos, south));
    assert!(!blocks.touches_neighbor(pos, east));
    assert!(!blocks.touches_neighbor(pos, north));
}

#[test]
fn streamed_faces_stay_in_their_chunk_after_cross_chunk_culling() {
    let dimension = bedrock_world::Dimension::Overworld;
    let mut builder = SceneBuilder::new_stream(SlimeChunkBounds {
        dimension,
        min_chunk_x: -1,
        max_chunk_x: 0,
        min_chunk_z: 0,
        max_chunk_z: 0,
    });
    let positions = [-1, 0].map(|x| ChunkPos { x, z: 0, dimension });
    for (pos, x) in positions.iter().copied().zip([-1, 0]) {
        let block = BlockKey { x, y: 64, z: 0 };
        builder.insert_stream_chunk(
            pos,
            ChunkBlocks {
                occupied: FxHashSet::from_iter([block]),
                opaque_blocks: vec![test_block_record(
                    block,
                    [0.4, 0.5, 0.6, 1.0],
                    test_material(),
                )],
                min_y: 64,
                max_y: 64,
                min_x: x,
                max_x: x,
                min_z: 0,
                max_z: 0,
                ..ChunkBlocks::default()
            },
        );
    }
    let faces = builder.build_stream_faces(&positions, StreamFaceOptions::default());
    assert_eq!(faces.len(), 2);
    for (chunk, pos) in faces.iter().zip(positions) {
        assert_eq!(chunk.pos, pos);
        assert_eq!(chunk.faces.len(), 5);
    }
    let coarse = builder.build_stream_lod_faces(&positions, 4);
    assert_eq!(coarse.len(), 2);
    for (chunk, pos) in coarse.iter().zip(positions) {
        assert_eq!(chunk.pos, pos);
        assert_eq!(chunk.faces.len(), 5);
        assert!(
            chunk
                .faces
                .iter()
                .flat_map(|face| face.corners)
                .all(|corner| {
                    corner[0] >= f64::from(pos.x * 16) && corner[0] <= f64::from(pos.x * 16 + 16)
                })
        );
    }
    let mut chunks = builder.take_stream_chunks();
    let neighbor_index = chunks
        .iter()
        .position(|(pos, _)| *pos == positions[1])
        .unwrap();
    let (_, mut neighbor) = chunks.swap_remove(neighbor_index);
    let boundary = Arc::new(neighbor.boundary_snapshot());
    let (target, blocks) = chunks.pop().unwrap();
    builder.insert_stream_chunk(target, blocks);
    builder.insert_stream_boundary(positions[1], boundary);
    assert_eq!(
        builder.build_stream_faces(&[target], StreamFaceOptions::default(),)[0]
            .faces
            .len(),
        5
    );
    assert_eq!(
        builder.build_stream_lod_faces(&[target], 4)[0].faces.len(),
        5
    );
}

#[test]
fn coarse_streaming_merges_blocks_inside_one_voxel() {
    let dimension = bedrock_world::Dimension::Overworld;
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension,
    };
    let mut builder = SceneBuilder::new_stream(SlimeChunkBounds {
        dimension,
        min_chunk_x: 0,
        max_chunk_x: 0,
        min_chunk_z: 0,
        max_chunk_z: 0,
    });
    let records = [0, 1].map(|x| {
        test_block_record(
            BlockKey { x, y: 64, z: 0 },
            [0.4, 0.5, 0.6, 1.0],
            test_material(),
        )
    });
    builder.insert_stream_chunk(
        pos,
        ChunkBlocks {
            opaque_blocks: records.into(),
            ..ChunkBlocks::default()
        },
    );
    let coarse = builder.build_stream_lod_faces(&[pos], 4);
    assert_eq!(coarse[0].faces.len(), 6);
    assert!(
        coarse[0]
            .faces
            .iter()
            .flat_map(|face| face.corners)
            .all(|corner| corner[0] >= 0.0 && corner[0] <= 4.0)
    );
}

#[test]
fn compact_chunk_storage_preserves_blocks_and_releases_excess_capacity() {
    let block = BlockKey { x: 1, y: 2, z: 3 };
    let mut blocks = ChunkBlocks::default();
    blocks.occupied.reserve(4096);
    blocks.occupied.insert(block);
    blocks.opaque_blocks.reserve(4096);
    let before = blocks.estimated_cpu_bytes();
    blocks.compact_storage();
    assert!(blocks.occupied.contains(&block));
    assert!(blocks.occupied.capacity() < 256);
    assert!(blocks.opaque_blocks.capacity() < 256);
    assert!(blocks.estimated_cpu_bytes() < before);
}

#[test]
fn stream_cube_face_winding_is_consistent_on_all_sides() {
    for face in FACE_DEFINITIONS {
        let corners = stream_face_corners(face.normal);
        let sub = |a: [f32; 3], b: [f32; 3]| [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
        for (a, b, c) in [(0, 1, 2), (0, 2, 3)] {
            let u = sub(corners[b], corners[a]);
            let v = sub(corners[c], corners[a]);
            let cross = [
                u[1] * v[2] - u[2] * v[1],
                u[2] * v[0] - u[0] * v[2],
                u[0] * v[1] - u[1] * v[0],
            ];
            let dot = cross[0] * face.normal[0] as f32
                + cross[1] * face.normal[1] as f32
                + cross[2] * face.normal[2] as f32;
            assert!(dot < 0.0, "inconsistent winding for {:?}", face.normal);
        }
    }
}

#[test]
fn boundary_snapshot_preserves_neighbor_classes_and_connections() {
    let solid = BlockKey { x: -1, y: 63, z: 5 };
    let glass = BlockKey { x: -1, y: 64, z: 6 };
    let water = BlockKey { x: -1, y: 65, z: 7 };
    let lava = BlockKey { x: -1, y: 66, z: 8 };
    let stair = BlockKey { x: -1, y: 67, z: 9 };
    let interior = BlockKey { x: -8, y: 64, z: 8 };
    let below_zero = BlockKey { x: -1, y: -1, z: 5 };
    let adjacent_solid = BlockKey { x: -2, y: 64, z: 6 };
    let state = StairState {
        facing: CardinalDirection::North,
        top: true,
    };
    let mut blocks = ChunkBlocks {
        occupied: FxHashSet::from_iter([solid, interior, below_zero, adjacent_solid]),
        // Overlap keeps the existing class precedence: opaque is written last.
        glass: FxHashSet::from_iter([glass, adjacent_solid]),
        water: FxHashSet::from_iter([water]),
        lava: FxHashSet::from_iter([lava]),
        connection_kinds: FxHashMap::from_iter([(stair, ConnectionKind::Fence)]),
        detail_blocks: vec![DetailBlock {
            key: stair,
            normalized_name: Arc::from("minecraft:stone_stairs"),
            neighbor_model: NeighborModel::Stairs,
            stair_state: Some(state),
            shape: Arc::new(DetailShape::default()),
            color: [1.0; 4],
            material: test_material(),
        }],
        ..ChunkBlocks::default()
    };
    let boundary = blocks.boundary_snapshot();
    assert_eq!(boundary.class_at(solid), Some(BlockClass::Opaque));
    assert_eq!(boundary.class_at(below_zero), Some(BlockClass::Opaque));
    assert_eq!(boundary.class_at(glass), Some(BlockClass::TransparentGlass));
    assert_eq!(boundary.class_at(water), Some(BlockClass::Water));
    assert_eq!(boundary.class_at(lava), Some(BlockClass::Lava));
    assert_eq!(boundary.class_at(adjacent_solid), Some(BlockClass::Opaque));
    assert_eq!(boundary.class_at(interior), Some(BlockClass::Opaque));
    assert_eq!(
        boundary.connection_kinds.get(&stair),
        Some(&ConnectionKind::Fence)
    );
    assert_eq!(boundary.stair_states.get(&stair), Some(&state));
}

#[test]
fn map_viewer_preview_3d_hides_faces_against_unprocessed_selected_neighbors() {
    let bounds = SlimeChunkBounds {
        dimension: bedrock_world::Dimension::Overworld,
        min_chunk_x: 0,
        max_chunk_x: 1,
        min_chunk_z: 0,
        max_chunk_z: 0,
    };
    let mut builder = SceneBuilder::new(bounds, 0);
    let block = BlockKey { x: 15, y: 64, z: 8 };
    builder.insert_prepared(PreparedChunk {
        chunk_key: ChunkKey { x: 0, z: 0 },
        subchunk_count: 1,
        internally_culled_blocks: 0,
        blocks: Some(ChunkBlocks {
            occupied: FxHashSet::from_iter([block]),
            opaque_blocks: vec![test_block_record(
                block,
                [0.4, 0.5, 0.6, 1.0],
                test_material(),
            )],
            min_y: 64,
            max_y: 64,
            min_x: 15,
            max_x: 15,
            min_z: 8,
            max_z: 8,
            ..ChunkBlocks::default()
        }),
    });

    builder
        .rebuild_combined_mesh()
        .unwrap_or_else(|error| panic!("{error}"));

    let mesh = builder.build_mesh();
    assert_eq!(mesh.face_count, 5);
}

#[test]
fn map_viewer_preview_3d_restores_boundary_faces_after_selection_finishes() {
    let bounds = SlimeChunkBounds {
        dimension: bedrock_world::Dimension::Overworld,
        min_chunk_x: 0,
        max_chunk_x: 1,
        min_chunk_z: 0,
        max_chunk_z: 0,
    };
    let mut builder = SceneBuilder::new(bounds, 0);
    let block = BlockKey { x: 15, y: 64, z: 8 };
    builder.insert_prepared(PreparedChunk {
        chunk_key: ChunkKey { x: 0, z: 0 },
        subchunk_count: 1,
        internally_culled_blocks: 0,
        blocks: Some(ChunkBlocks {
            occupied: FxHashSet::from_iter([block]),
            opaque_blocks: vec![test_block_record(
                block,
                [0.4, 0.5, 0.6, 1.0],
                test_material(),
            )],
            min_y: 64,
            max_y: 64,
            min_x: 15,
            max_x: 15,
            min_z: 8,
            max_z: 8,
            ..ChunkBlocks::default()
        }),
    });
    builder.insert_prepared(PreparedChunk {
        chunk_key: ChunkKey { x: 1, z: 0 },
        subchunk_count: 0,
        internally_culled_blocks: 0,
        blocks: None,
    });

    builder
        .rebuild_combined_mesh()
        .unwrap_or_else(|error| panic!("{error}"));
    let mesh = builder.build_mesh();

    assert_eq!(mesh.face_count, 6);
}

#[test]
fn map_viewer_preview_3d_panes_infer_missing_connections_from_neighbors() {
    let palette = RenderPalette::default();
    let bounds = SlimeChunkBounds {
        dimension: bedrock_world::Dimension::Overworld,
        min_chunk_x: 0,
        max_chunk_x: 0,
        min_chunk_z: 0,
        max_chunk_z: 0,
    };
    let mut chunk_builder = StructureChunkBuilder::default();
    push_structure_block(
        BlockKey { x: 0, y: 64, z: 0 },
        Some(&test_block_state("minecraft:iron_bars")),
        None,
        &palette,
        &mut chunk_builder,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    push_structure_block(
        BlockKey { x: 1, y: 64, z: 0 },
        Some(&test_block_state("minecraft:stone")),
        None,
        &palette,
        &mut chunk_builder,
    )
    .unwrap_or_else(|error| panic!("{error}"));

    let mut builder = SceneBuilder::new(bounds, 0);
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
    let world_origin = mesh.chunk_meshes.first().map_or([0.0; 3], |chunk_mesh| {
        chunk_mesh.world_origin.map(|coordinate| coordinate as f32)
    });

    assert!(
        gpu_mesh.vertices().iter().any(|vertex| {
            (vertex.position.x + world_origin[0] - 1.0).abs() < 0.001
                && vertex.position.y + world_origin[1] >= 64.0
                && vertex.position.y + world_origin[1] <= 65.0
                && vertex.position.z + world_origin[2] >= 0.4375
                && vertex.position.z + world_origin[2] <= 0.5625
        }),
        "iron bars without explicit state should connect to the east neighbor"
    );
}
