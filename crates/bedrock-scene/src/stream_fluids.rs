use super::*;

fn liquid(x: i32, height: f32) -> FluidBlock {
    FluidBlock {
        key: BlockKey { x, y: 64, z: 8 },
        color: [0.3, 0.5, 0.8, 0.46],
        material: Arc::from("minecraft_water"),
        height,
    }
}

#[test]
fn fluid_side_uv_is_pinned_to_the_surface_not_stretched() {
    // A liquid side is not a full cell: its texture is anchored to the surface, so a
    // 0.5-tall column samples only the top half of the tile (v 0.5..1), not the whole 0..1
    // squashed into half a cell. A neighbor hiding the bottom third shifts the sampled band.
    let full = fluid_face_uv([0, 0, 1], 0.0, 1.0);
    assert!((full[0][1] - 0.0).abs() < 1e-6 && (full[2][1] - 1.0).abs() < 1e-6);
    let half = fluid_face_uv([0, 0, 1], 0.0, 0.5);
    assert!((half[0][1] - 0.5).abs() < 1e-6 && (half[2][1] - 1.0).abs() < 1e-6);
    let shifted = fluid_face_uv([0, 0, 1], 1.0 / 3.0, 1.0);
    assert!((shifted[0][1] - 1.0 / 3.0).abs() < 1e-6);
    // Top/bottom faces keep the whole tile.
    assert_eq!(fluid_face_uv([0, 1, 0], 0.0, 0.5), cube_face_uv([0, 1, 0]));
}

#[test]
fn unloaded_water_seam_waits_for_verified_neighbor_air() {
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: bedrock_world::Dimension::Overworld,
    };
    let neighbor = ChunkPos { x: 1, z: 0, ..pos };
    let mut water = liquid(15, 1.0);
    water.key.y = 80;
    let mut blocks = ChunkBlocks::default();
    blocks.water.insert(water.key);
    blocks.water_blocks.push(water.clone());
    let mut builder = SceneBuilder::new_stream(SlimeChunkBounds {
        dimension: pos.dimension,
        min_chunk_x: 0,
        max_chunk_x: 1,
        min_chunk_z: 0,
        max_chunk_z: 0,
    });
    builder.insert_stream_chunk(pos, blocks);
    let east = FACE_DEFINITIONS
        .into_iter()
        .find(|face| face.normal == [1, 0, 0])
        .unwrap();
    // Unknown is neither air nor solid: don't allocate a fake water wall.
    assert_eq!(
        builder.exposed_fluid_face_bottom(&water, east, BlockClass::Water),
        None,
    );
    builder.insert_stream_chunk(neighbor, ChunkBlocks::default());
    // Once the empty neighbor column is decoded, the real exposed wall is restored.
    assert_eq!(
        builder.exposed_fluid_face_bottom(&water, east, BlockClass::Water),
        Some(0.0),
    );
}

#[test]
fn hidden_detail_sections_skip_tessellation_before_cpu_faces_exist() {
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: bedrock_world::Dimension::Overworld,
    };
    let mut blocks = ChunkBlocks::default();
    blocks.detail_blocks.push(DetailBlock {
        key: BlockKey { x: 8, y: 90, z: 8 },
        normalized_name: Arc::from("minecraft:oak_fence"),
        neighbor_model: NeighborModel::None,
        stair_state: None,
        shape: Arc::new(DetailShape::from_cuboids(vec![Cuboid::new(
            [0.25, 0.0, 0.25],
            [0.75, 1.0, 0.75],
        )])),
        color: [1.0; 4],
        material: Arc::from("minecraft:oak_fence"),
    });
    let mut builder = SceneBuilder::new_stream(SlimeChunkBounds {
        dimension: pos.dimension,
        min_chunk_x: 0,
        max_chunk_x: 0,
        min_chunk_z: 0,
        max_chunk_z: 0,
    });
    builder.insert_stream_chunk(pos, blocks);
    assert!(
        !builder
            .build_stream_faces(&[pos], StreamFaceOptions::default(),)
            .is_empty()
    );
    let hidden = BTreeSet::from([(pos, 1)]);
    assert!(
        builder
            .build_stream_faces(
                &[pos],
                StreamFaceOptions {
                    selections: Some(&FxHashMap::default()),
                    gpu_cubes: Some(&FxHashSet::default()),
                    hidden_bands: Some(&hidden),
                    ..Default::default()
                },
            )
            .is_empty(),
        "hidden detailed models must not generate CPU quad vertices",
    );
}

#[test]
fn adjacent_partial_water_keeps_only_the_exposed_upper_strip() {
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: bedrock_world::Dimension::Overworld,
    };
    let mut blocks = ChunkBlocks::default();
    for fluid in [liquid(7, 1.0), liquid(8, 0.5)] {
        blocks.water.insert(fluid.key);
        blocks.water_blocks.push(fluid);
    }
    let mut builder = SceneBuilder::new_stream(SlimeChunkBounds {
        dimension: pos.dimension,
        min_chunk_x: 0,
        max_chunk_x: 0,
        min_chunk_z: 0,
        max_chunk_z: 0,
    });
    builder.insert_stream_chunk(pos, blocks);
    let face = FACE_DEFINITIONS
        .into_iter()
        .find(|face| face.normal == [1, 0, 0])
        .unwrap();
    let upper = liquid(7, 1.0);
    let lower = liquid(8, 0.5);
    assert_eq!(
        builder.exposed_fluid_face_bottom(&upper, face, BlockClass::Water,),
        Some(0.5)
    );
    let reverse = FACE_DEFINITIONS
        .into_iter()
        .find(|face| face.normal == [-1, 0, 0])
        .unwrap();
    assert_eq!(
        builder.exposed_fluid_face_bottom(&lower, reverse, BlockClass::Water,),
        None
    );
    let exposed = fluid_face_segment(
        upper.key,
        face,
        upper.color,
        upper.material.clone(),
        0.5,
        upper.height,
    );
    let heights = exposed.corners.map(|corner| corner[1]);
    assert!(heights.contains(&64.5));
    assert!(heights.contains(&65.0));
    assert!(!heights.contains(&64.0));
}

#[test]
fn waterlogged_water_renders_as_a_full_square_at_the_source_height() {
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: bedrock_world::Dimension::Overworld,
    };
    // Official waterlogging (minecraft:liquid_detection): the liquid layer is
    // plain full-square quads at level/9, never clipped to the co-located
    // block's shape. A source cell alone in air shows all six faces with the
    // surface one ninth below the cell top.
    let mut blocks = ChunkBlocks::default();
    let block = liquid(8, SOURCE_FLUID_HEIGHT);
    blocks.water.insert(block.key);
    blocks.water_blocks.push(block);
    let mut builder = SceneBuilder::new_stream(SlimeChunkBounds {
        dimension: pos.dimension,
        min_chunk_x: 0,
        max_chunk_x: 0,
        min_chunk_z: 0,
        max_chunk_z: 0,
    });
    builder.insert_stream_chunk(pos, blocks);
    let faces: Vec<_> = builder
        .build_stream_faces(&[pos], StreamFaceOptions::default())
        .remove(0)
        .faces
        .into_iter()
        .filter(|face| face.pass == StreamPass::Water)
        .collect();
    assert_eq!(faces.len(), 6, "an exposed water cell draws all six faces");
    let surface = f64::from(64) + f64::from(SOURCE_FLUID_HEIGHT);
    let top = faces
        .iter()
        .find(|face| face.normal == [0, 1, 0])
        .expect("full-square surface");
    let top_ys = top.corners.map(|corner| corner[1]);
    assert!(top_ys.iter().all(|&y| (y - surface).abs() < 1e-4));
    let top_xs = top.corners.map(|corner| corner[0]);
    assert!(top_xs.contains(&8.0) && top_xs.contains(&9.0), "full square");
    let east = faces
        .iter()
        .find(|face| face.normal == [1, 0, 0])
        .expect("side wall");
    let east_ys = east.corners.map(|corner| corner[1]);
    assert!(east_ys.contains(&64.0) && east_ys.contains(&surface));
    let east_xs = east.corners.map(|corner| corner[0]);
    assert!(east_xs.iter().all(|&x| (x - 8.999).abs() < 1e-6));
}

#[test]
fn water_under_water_walls_reach_the_full_cell_height() {
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: bedrock_world::Dimension::Overworld,
    };
    // The height table raises a cell with the same fluid above to a full
    // block, so stacked source cells wall seamlessly from floor to surface.
    let mut blocks = ChunkBlocks::default();
    for y in [64, 65] {
        let mut block = liquid(8, SOURCE_FLUID_HEIGHT);
        block.key.y = y;
        blocks.water.insert(block.key);
        blocks.water_blocks.push(block);
    }
    let mut builder = SceneBuilder::new_stream(SlimeChunkBounds {
        dimension: pos.dimension,
        min_chunk_x: 0,
        max_chunk_x: 0,
        min_chunk_z: 0,
        max_chunk_z: 0,
    });
    builder.insert_stream_chunk(pos, blocks);
    let faces: Vec<_> = builder
        .build_stream_faces(&[pos], StreamFaceOptions::default())
        .remove(0)
        .faces
        .into_iter()
        .filter(|face| face.pass == StreamPass::Water)
        .collect();
    let surface = f64::from(65) + f64::from(SOURCE_FLUID_HEIGHT);
    // No faces at the internal boundary: the column is one water body.
    assert!(
        !faces
            .iter()
            .any(|face| face.corners.iter().any(|corner| (corner[1] - 65.0).abs() < 1e-9
                && face.normal != [1, 0, 0]
                && face.normal != [-1, 0, 0]
                && face.normal != [0, 0, 1]
                && face.normal != [0, 0, -1])),
        "no horizontal face at the internal boundary"
    );
    // The lower cell walls to the full cell; the upper cell tops at 8/9.
    let lower_east = faces
        .iter()
        .find(|face| {
            face.normal == [1, 0, 0]
                && face.corners.iter().any(|corner| (corner[1] - 64.0).abs() < 1e-9)
        })
        .expect("lower wall");
    let lower_ys = lower_east.corners.map(|corner| corner[1]);
    assert!(lower_ys.contains(&64.0) && lower_ys.contains(&65.0));
    let top = faces
        .iter()
        .find(|face| face.normal == [0, 1, 0])
        .expect("single surface");
    let top_ys = top.corners.map(|corner| corner[1]);
    assert!(top_ys.iter().all(|&y| (y - surface).abs() < 1e-4));
}

#[test]
fn adjacent_waterlogged_cells_share_one_water_body() {
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: bedrock_world::Dimension::Overworld,
    };
    // Two neighbouring waterlogged cells (their primary block is not water):
    // the wall between them must not render, otherwise two coplanar faces
    // blend into every neighbouring block face — the "water through solid
    // faces" artifact. Exposed outer walls keep their official full-square
    // quads at the source height.
    let mut blocks = ChunkBlocks::default();
    for x in [7, 8] {
        let mut block = liquid(x, SOURCE_FLUID_HEIGHT);
        blocks.waterlogged_water.insert(block.key);
        blocks.water_blocks.push(block);
    }
    let mut builder = SceneBuilder::new_stream(SlimeChunkBounds {
        dimension: pos.dimension,
        min_chunk_x: 0,
        max_chunk_x: 0,
        min_chunk_z: 0,
        max_chunk_z: 0,
    });
    builder.insert_stream_chunk(pos, blocks);
    let faces: Vec<_> = builder
        .build_stream_faces(&[pos], StreamFaceOptions::default())
        .remove(0)
        .faces
        .into_iter()
        .filter(|face| face.pass == StreamPass::Water)
        .collect();
    // The shared boundary sits at x = 8: no water face may lie there.
    assert!(
        !faces.iter().any(|face| face
            .corners
            .iter()
            .all(|corner| (corner[0] - 8.0).abs() < 0.01)),
        "no water wall between two waterlogged cells"
    );
    // Outer walls survive: the west face of the first cell (x = 7, inset).
    assert!(
        faces
            .iter()
            .any(|face| face.corners.iter().all(|corner| (corner[0] - 6.999).abs() < 0.01)),
        "exposed outer wall stays"
    );
}

#[test]
fn internal_fluid_filter_keeps_exposed_strips_and_retains_culled_neighbor_level() {
    let mut blocks = ChunkBlocks::default();
    let high = liquid(7, 1.0);
    let low = liquid(8, 0.5);
    // Six same-fluid neighbours normally make a cell an internal record.
    // The high cell must nevertheless remain renderable above the low
    // neighbour's surface; the low cell may be culled but its level is
    // still needed as neighbor metadata.
    for (dx, dy, dz) in [
        (0, 0, 0),
        (1, 0, 0),
        (-1, 0, 0),
        (0, 1, 0),
        (0, -1, 0),
        (0, 0, 1),
        (0, 0, -1),
    ] {
        let block = if dx == 1 && dy == 0 && dz == 0 {
            low.clone()
        } else {
            let mut value = high.clone();
            value.key.x += dx;
            value.key.y += dy;
            value.key.z += dz;
            value
        };
        blocks.water.insert(block.key);
        blocks.water_blocks.push(block);
    }
    blocks.rebuild_neighbor_indexes();
    filter_internal_block_records(&mut blocks);
    assert!(
        blocks
            .water_blocks
            .iter()
            .any(|block| block.key == high.key)
    );
    assert_eq!(blocks.partial_fluid_heights.get(&low.key), Some(&0.5));
    blocks.rebuild_neighbor_indexes();
    assert_eq!(blocks.partial_fluid_heights.get(&low.key), Some(&0.5));
}

#[test]
fn boundary_halo_retains_partial_fluid_level_without_interior_duplicates() {
    let pos = ChunkPos {
        x: 0,
        z: 0,
        dimension: bedrock_world::Dimension::Overworld,
    };
    let mut blocks = ChunkBlocks::default();
    for fluid in [liquid(0, 0.4), liquid(7, 0.7)] {
        blocks.water.insert(fluid.key);
        blocks.water_blocks.push(fluid);
    }
    let halo = blocks.boundary_snapshot();
    assert_eq!(halo.partial_fluid_heights.len(), 1);
    assert_eq!(
        halo.partial_fluid_heights.get(&liquid(0, 0.4).key),
        Some(&0.4)
    );
    let mut builder = SceneBuilder::new_stream(SlimeChunkBounds {
        dimension: pos.dimension,
        min_chunk_x: -1,
        max_chunk_x: 0,
        min_chunk_z: 0,
        max_chunk_z: 0,
    });
    builder.insert_stream_boundary(pos, Arc::new(halo));
    let higher = liquid(-1, 1.0);
    let face = FACE_DEFINITIONS
        .into_iter()
        .find(|face| face.normal == [1, 0, 0])
        .unwrap();
    assert_eq!(
        builder.exposed_fluid_face_bottom(&higher, face, BlockClass::Water,),
        Some(0.4)
    );
}
