use super::support::*;
use super::*;
use bedrock_block_model::model_family::canonical_block_name;

#[test]
fn banner_art_reads_the_block_entity_dye_and_pattern_layers() {
    let mut nbt = indexmap::IndexMap::new();
    nbt.insert("id".to_owned(), NbtTag::String("Banner".to_owned()));
    nbt.insert("Base".to_owned(), NbtTag::Int(14));
    let mut layers = Vec::new();
    for (pattern, color) in [
        ("bs", NbtTag::Int(4)),
        ("cs", NbtTag::String("black".to_owned())),
        ("unknown_pattern", NbtTag::Int(1)),
    ] {
        let mut layer = indexmap::IndexMap::new();
        layer.insert("Pattern".to_owned(), NbtTag::String(pattern.to_owned()));
        layer.insert("Color".to_owned(), color);
        layers.push(NbtTag::Compound(layer));
    }
    nbt.insert("Patterns".to_owned(), NbtTag::List(layers));

    let art = banner_art_from_nbt(&nbt).expect("banner art");
    assert_eq!(art.base, 14);
    // `bs` is the bottom stripe and `cs` the centre stripe; unknown ids are dropped.
    assert_eq!(art.layers.len(), 2);
    assert_eq!(
        art.layers[0].pattern,
        BannerArt::pattern_index("bs").expect("bs")
    );
    assert_eq!(art.layers[0].color, 4);
    assert_eq!(
        art.layers[1].pattern,
        BannerArt::pattern_index("cs").expect("cs")
    );
    assert_eq!(art.layers[1].color, 15);
    // The material name the face pipeline carries round-trips the same art.
    assert_eq!(
        BannerArt::from_material_name(&art.material_name()),
        Some(art)
    );
    assert!(is_banner_block("minecraft:standing_banner"));
    assert!(!is_banner_block("minecraft:stone"));
}

#[test]
fn banner_art_without_patterns_still_carries_the_dye() {
    let mut nbt = indexmap::IndexMap::new();
    nbt.insert("Base".to_owned(), NbtTag::String("orange".to_owned()));
    let art = banner_art_from_nbt(&nbt).expect("banner art");
    assert_eq!(art.base, 1);
    assert!(art.layers.is_empty());
    assert_eq!(art.material_name(), "bannerart-01");
}

#[test]
fn procedural_cuboids_without_uvs_get_the_full_texture_tile() {
    // Bedrock ships one 16x16 tile per face for partial blocks. A cuboid that omits UVs
    // must sample that tile whole; block-space projection cropped thin elements (torch,
    // button, door) down to a strip of the texture.
    let cuboid = ModelCuboid::new([0.4375, 0.0, 0.4375], [0.5625, 0.625, 0.5625]);
    let preview = cuboid_from_model(cuboid);
    let expected = full_texture_uv();
    for normal in [
        [0, 1, 0],
        [0, -1, 0],
        [0, 0, -1],
        [0, 0, 1],
        [1, 0, 0],
        [-1, 0, 0],
    ] {
        assert_eq!(preview.face_uv(normal), Some(expected), "{normal:?}");
    }
}

#[test]
fn resolver_geometry_turns_resource_pack_block_into_non_full_shape() {
    let mut repository = BlockModelRepository::new();
    repository.merge_blocks_value(&serde_json::json!({
        "minecraft:sunflower": {
            "geometry": "geometry.test.sunflower"
        }
    }));
    repository.geometries.merge_value(&serde_json::json!({
        "minecraft:geometry": [{
            "description": { "identifier": "geometry.test.sunflower" },
            "bones": [{
                "name": "root",
                "cubes": [{
                    "origin": [-8, 0, -0.5],
                    "size": [16, 16, 1]
                }]
            }]
        }]
    }));
    let state = test_block_state_with_tag(
        "minecraft:double_plant",
        "double_plant_type",
        NbtTag::String("sunflower".to_string()),
    );

    let shape = resolved_detail_shape(&repository, &state, BlockClass::DetailOpaque)
        .unwrap_or_else(|| panic!("sunflower geometry should resolve"));

    assert_eq!(
        canonical_block_name(&block_state_query(&state)),
        "minecraft:sunflower"
    );
    assert_eq!(shape.cuboids.len(), 1);
    assert!(shape.cuboids[0].max[2] - shape.cuboids[0].min[2] <= 0.07);
    assert!(!shape_is_full_cube(&shape));
}

#[test]
fn resolver_uses_shulker_geometry_for_shulker_box_blocks() {
    let mut repository = BlockModelRepository::new();
    repository.geometries.merge_value(&serde_json::json!({
        "geometry.shulker.v1.8": {
            "texturewidth": 64,
            "textureheight": 64,
            "bones": [{
                "name": "base",
                "cubes": [{
                    "origin": [-8.0, 0.0, -8.0],
                    "size": [16.0, 8.0, 16.0],
                    "uv": [0, 28]
                }]
            }, {
                "name": "lid",
                "cubes": [{
                    "origin": [-8.0, 4.0, -8.0],
                    "size": [16.0, 12.0, 16.0],
                    "uv": [0, 0]
                }]
            }, {
                "name": "head",
                "cubes": [{
                    "origin": [-3.0, 6.0, -3.0],
                    "size": [6.0, 6.0, 6.0],
                    "uv": [0, 52]
                }]
            }]
        }
    }));

    let shape = resolved_detail_shape(
        &repository,
        &test_block_state("minecraft:blue_shulker_box"),
        BlockClass::DetailOpaque,
    )
    .unwrap_or_else(|| panic!("shulker geometry should resolve"));

    assert_eq!(shape.cuboids.len(), 2);
    assert!(
        shape
            .cuboids
            .iter()
            .any(|cuboid| (cuboid.max[1] - cuboid.min[1] - 0.75).abs() < 0.001)
    );
    assert!(
        shape
            .cuboids
            .iter()
            .all(|cuboid| (cuboid.max[0] - cuboid.min[0]) > 0.4)
    );
    assert!(
        shape
            .cuboids
            .iter()
            .all(|cuboid| cuboid.face_material_slots.values().all(|slot| {
                let slot = slot.as_ref();
                slot == "up" || slot == "down" || slot == "side"
            }))
    );
}

#[test]
fn model_collection_borrows_only_selected_subchunks() {
    let subchunks = [-1, 0]
        .into_iter()
        .map(|y| {
            let mut indices = vec![0; 4096];
            indices[0] = 1;
            let storage = ::bedrock_world::block::BlockPalette::with_unpacked_indices(
                vec![
                    test_block_state("minecraft:air"),
                    test_block_state("minecraft:stone"),
                ],
                indices,
                None,
            );
            (
                y,
                bedrock_world::SubChunk {
                    y,
                    format: bedrock_world::SubChunkFormat::Paletted {
                        version: 9,
                        storages: vec![storage],
                    },
                },
            )
        })
        .collect();
    let chunk = test_render_chunk_with_subchunks(subchunks);
    let blocks = collect::collect(&chunk, None, None, |y| y < 0)
        .expect("selected subchunk should be collected");
    assert_eq!(
        blocks.occupied,
        FxHashSet::from_iter([BlockKey { x: 0, y: -16, z: 0 }]),
    );
    assert_eq!(chunk.subchunks.len(), 2);
}

#[test]
fn map_viewer_preview_3d_reassigns_unused_transparent_capacity_to_terrain() {
    let glass_faces = 683;
    let water_faces = 8_136;
    let lava_faces = 1_614;

    let opaque_budget = opaque_face_budget(glass_faces, water_faces, lava_faces);

    assert_eq!(
        opaque_budget,
        TOTAL_FACE_BUDGET - glass_faces - water_faces - lava_faces
    );
    assert!(opaque_budget > TOTAL_FACE_BUDGET * 3 / 5);
}

#[test]
fn map_viewer_preview_3d_transparent_blocks_are_classified() {
    for name in ["minecraft:air", "minecraft:water", "minecraft:glass"] {
        assert!(!is_solid_block(name), "{name}");
    }
    assert!(is_solid_block("minecraft:stone"));
    assert!(is_solid_block("minecraft:oak_leaves"));
    assert_eq!(
        block_class("minecraft:short_grass"),
        BlockClass::DetailOpaque
    );
    assert_eq!(block_class("minecraft:vine"), BlockClass::DetailOpaque);
    assert_eq!(block_class("minecraft:rail"), BlockClass::DetailOpaque);
    assert_eq!(
        block_class("minecraft:standing_sign"),
        BlockClass::DetailOpaque
    );
    assert_eq!(block_class("minecraft:water"), BlockClass::Water);
    assert_eq!(block_class("minecraft:flowing_water"), BlockClass::Water);
    assert_eq!(block_class("minecraft:lava"), BlockClass::Lava);
    assert_eq!(block_class("minecraft:flowing_lava"), BlockClass::Lava);
    for name in ["minecraft:glass", "minecraft:white_stained_glass"] {
        assert_eq!(block_class(name), BlockClass::TransparentGlass, "{name}");
    }
    for name in ["minecraft:glass_pane", "minecraft:blue_stained_glass_pane"] {
        assert_eq!(block_class(name), BlockClass::DetailGlass, "{name}");
    }
}

#[test]
fn map_viewer_preview_3d_collects_common_detail_blocks_as_geometry() {
    let palette = RenderPalette::default();
    let block = BlockKey { x: 0, y: 64, z: 0 };
    let mut builder = StructureChunkBuilder::default();

    for (offset, state) in [
        (
            0,
            test_block_state_with_tag("minecraft:stone_slab", "top_slot_bit", NbtTag::Byte(0)),
        ),
        (
            1,
            test_block_state_with_tag(
                "minecraft:ladder",
                "minecraft:cardinal_direction",
                NbtTag::String("north".to_string()),
            ),
        ),
        (2, test_block_state("minecraft:oak_fence")),
        (3, test_block_state("minecraft:short_grass")),
    ]
    .iter()
    {
        push_structure_block(
            BlockKey {
                x: block.x + *offset,
                y: block.y,
                z: block.z,
            },
            Some(state),
            None,
            &palette,
            &mut builder,
        )
        .unwrap_or_else(|error| panic!("{error}"));
    }

    assert_eq!(builder.blocks.detail_blocks.len(), 4);
    assert!(builder.blocks.occupied.is_empty());
}

#[test]
fn map_viewer_preview_3d_collects_variant_detail_blocks_as_non_full_geometry() {
    let palette = RenderPalette::default();
    let mut builder = StructureChunkBuilder::default();

    for (offset, state) in [
        (0, test_block_state("minecraft:iron_bars")),
        (1, test_block_state("minecraft:stonecutter")),
        (2, test_block_state("minecraft:anvil")),
        (3, test_block_state("minecraft:decorated_pot")),
        (4, test_block_state("minecraft:blue_shulker_box")),
        (5, test_block_state("minecraft:chain")),
        (6, test_block_state("minecraft:lantern")),
        (7, test_block_state("minecraft:chest")),
        (8, test_block_state("minecraft:hopper")),
    ] {
        push_structure_block(
            BlockKey {
                x: offset,
                y: 64,
                z: 0,
            },
            Some(&state),
            None,
            &palette,
            &mut builder,
        )
        .unwrap_or_else(|error| panic!("{error}"));
    }

    assert_eq!(builder.blocks.detail_blocks.len(), 9);
    assert!(builder.blocks.occupied.is_empty());
}

#[test]
fn map_viewer_preview_3d_fence_and_wall_shapes_follow_connection_state() {
    let fence = test_block_state_with_tag("minecraft:oak_fence", "north", NbtTag::Byte(1));
    let fence_shape =
        detail_shape(&fence, None).unwrap_or_else(|| panic!("fence should have detail shape"));
    assert_eq!(fence_shape.cuboids.len(), 3);
    assert!(
        fence_shape
            .cuboids
            .iter()
            .any(|cuboid| cuboid.min[2] == 0.0 && cuboid.max[2] == 0.5)
    );

    let mut wall_states = BTreeMap::new();
    wall_states.insert(
        "wall_connection_type_north".to_string(),
        NbtTag::String("tall".to_string()),
    );
    wall_states.insert(
        "wall_connection_type_south".to_string(),
        NbtTag::String("none".to_string()),
    );
    wall_states.insert("wall_post_bit".to_string(), NbtTag::Byte(1));
    let wall = BlockState {
        name: "minecraft:brick_wall".to_string(),
        states: wall_states,
        version: None,
    };
    let wall_shape =
        detail_shape(&wall, None).unwrap_or_else(|| panic!("wall should have detail shape"));
    assert_eq!(wall_shape.cuboids.len(), 2);
    assert!(
        wall_shape
            .cuboids
            .iter()
            .any(|cuboid| cuboid.min[2] == 0.0 && (cuboid.max[1] - 1.0).abs() < 0.001)
    );
}

#[test]
fn map_viewer_preview_3d_stairs_use_inner_and_outer_shape_states() {
    let mut inner_states = BTreeMap::new();
    inner_states.insert(
        "minecraft:cardinal_direction".to_string(),
        NbtTag::String("north".to_string()),
    );
    inner_states.insert(
        "shape".to_string(),
        NbtTag::String("inner_left".to_string()),
    );
    let inner = BlockState {
        name: "minecraft:oak_stairs".to_string(),
        states: inner_states,
        version: None,
    };
    let inner_shape = detail_shape(&inner, None)
        .unwrap_or_else(|| panic!("inner stairs should have detail shape"));

    let mut outer_states = BTreeMap::new();
    outer_states.insert(
        "minecraft:cardinal_direction".to_string(),
        NbtTag::String("north".to_string()),
    );
    outer_states.insert(
        "shape".to_string(),
        NbtTag::String("outer_right".to_string()),
    );
    let outer = BlockState {
        name: "minecraft:oak_stairs".to_string(),
        states: outer_states,
        version: None,
    };
    let outer_shape = detail_shape(&outer, None)
        .unwrap_or_else(|| panic!("outer stairs should have detail shape"));

    assert_eq!(inner_shape.cuboids.len(), 3);
    assert_eq!(outer_shape.cuboids.len(), 2);
    assert!(
        outer_shape
            .cuboids
            .iter()
            .any(|cuboid| cuboid.min[0] >= 0.5 && cuboid.max[2] <= 0.5)
    );
}

#[test]
fn map_viewer_preview_3d_shulker_box_shape_is_closed_shell_without_inner_head() {
    let shape = detail_shape(&test_block_state("minecraft:blue_shulker_box"), None)
        .unwrap_or_else(|| panic!("shulker box should have detail shape"));

    assert_eq!(shape.cuboids.len(), 2);
    assert!(
        shape
            .cuboids
            .iter()
            .all(|cuboid| cuboid.max[0] - cuboid.min[0] >= 1.0
                || cuboid.max[1] - cuboid.min[1] >= 0.5)
    );
}

#[test]
fn map_viewer_preview_3d_panes_iron_bars_and_plants_use_cutout_detail_shapes() {
    let iron_bars = test_block_state_with_tag("minecraft:iron_bars", "north", NbtTag::Byte(1));
    let bars_shape = detail_shape(&iron_bars, None)
        .unwrap_or_else(|| panic!("iron bars should have detail shape"));
    // The post is caps-only planes; the connected arm is the only cuboid and stays 2 px wide.
    assert_eq!(bars_shape.cuboids.len(), 1);
    assert!(
        bars_shape
            .cuboids
            .iter()
            .all(|cuboid| cuboid.max[0] - cuboid.min[0] <= 0.25)
    );

    let isolated_bars_shape = detail_shape(&test_block_state("minecraft:iron_bars"), None)
        .unwrap_or_else(|| panic!("isolated iron bars should have detail shape"));
    // The post is two cap planes; every disconnected side draws its `noside` card as a
    // plane (a zero-thickness cuboid is skipped by the face emitter and renders as nothing).
    assert_eq!(isolated_bars_shape.cuboids.len(), 0);
    assert_eq!(isolated_bars_shape.planes.len(), 6);

    let plant_shape = detail_shape(&test_block_state("minecraft:poppy"), None)
        .unwrap_or_else(|| panic!("plant should have detail shape"));
    assert!(plant_shape.cuboids.is_empty());
    assert_eq!(plant_shape.planes.len(), 2);

    let web_shape = detail_shape(&test_block_state("minecraft:web"), None)
        .unwrap_or_else(|| panic!("web should have detail shape"));
    assert!(web_shape.cuboids.is_empty());
    assert_eq!(web_shape.planes.len(), 2);
}

#[test]
fn map_viewer_preview_3d_redstone_wire_uses_flat_cutout_planes() {
    let mut states = BTreeMap::new();
    states.insert("power".to_string(), NbtTag::Int(15));
    states.insert("north".to_string(), NbtTag::String("side".to_string()));
    states.insert("south".to_string(), NbtTag::String("side".to_string()));
    states.insert("east".to_string(), NbtTag::String("none".to_string()));
    states.insert("west".to_string(), NbtTag::String("none".to_string()));
    let redstone = BlockState {
        name: "minecraft:redstone_wire".to_string(),
        states,
        version: None,
    };
    let shape = detail_shape(&redstone, None)
        .unwrap_or_else(|| panic!("redstone wire should have detail shape"));

    assert!(shape.cuboids.is_empty());
    assert_eq!(shape.planes.len(), 1);
    assert_eq!(shape.planes[0].material_slot.as_deref(), Some("down"));
    assert!(
        shape.planes[0]
            .corners
            .iter()
            .all(|corner| (corner[1] - 0.01).abs() < 0.001)
    );
    let color = redstone_wire_color(&redstone);
    assert!((color[0] - 1.0).abs() < 0.001);
    assert!((color[1] - 0.2).abs() < 0.001);
    assert!((color[2] - 0.0).abs() < 0.001);
    assert!((color[3] - 1.0).abs() < 0.001);
}

#[test]
fn map_viewer_preview_3d_chest_faces_sample_their_own_16px_tiles() {
    // Bedrock ships chest_side/chest_front/chest_top as independent 16x16 tiles with the
    // metal rim and latch baked in — each face reads its whole tile (no 64x64 unwrap).
    let shape = detail_shape(&test_block_state("minecraft:chest"), None)
        .unwrap_or_else(|| panic!("chest should have detail shape"));
    let body = shape
        .cuboids
        .first()
        .unwrap_or_else(|| panic!("chest body should exist"));
    for face in [BlockFace::North, BlockFace::South, BlockFace::East, BlockFace::West,
                 BlockFace::Up, BlockFace::Down] {
        assert_eq!(body.face_uvs.get(&face).copied(), Some(full_texture_uv()),
            "chest face {face:?} must sample the whole 16x16 tile");
    }
}

#[test]
fn map_viewer_preview_3d_double_slabs_are_full_blocks_with_slab_material() {
    assert_eq!(
        block_class("minecraft:double_stone_slab"),
        BlockClass::Opaque
    );
    let state = test_block_state_with_tag(
        "minecraft:double_stone_slab",
        "stone_slab_type",
        NbtTag::String("brick".to_string()),
    );
    assert_eq!(
        material_block_name(&state, BlockClass::Opaque).as_ref(),
        "minecraft:bricks"
    );
}

#[test]
fn map_viewer_preview_3d_trapdoor_torch_and_portal_use_state_geometry() {
    let mut trapdoor_states = BTreeMap::new();
    trapdoor_states.insert("open".to_string(), NbtTag::Byte(1));
    trapdoor_states.insert("facing_direction".to_string(), NbtTag::Byte(4));
    let trapdoor = BlockState {
        name: "minecraft:oak_trapdoor".to_string(),
        states: trapdoor_states,
        version: None,
    };
    let trapdoor_shape = detail_shape(&trapdoor, None)
        .unwrap_or_else(|| panic!("trapdoor should have detail shape"));
    // `facing_direction = 4` is west, and the trapdoor's direction names the side it hangs
    // on, so the open panel is the full-height slab on that same west boundary.
    assert!(
        trapdoor_shape
            .cuboids
            .iter()
            .any(|cuboid| cuboid.max[1] - cuboid.min[1] > 0.9
                && cuboid.min[0] < 0.05
                && cuboid.max[0] <= 0.2)
    );

    let torch = test_block_state_with_tag(
        "minecraft:torch",
        "minecraft:block_face",
        NbtTag::String("floor".to_string()),
    );
    let torch_shape =
        detail_shape(&torch, None).unwrap_or_else(|| panic!("torch should have detail shape"));
    assert!(torch_shape.cuboids[0].max[0] - torch_shape.cuboids[0].min[0] < 0.25);
    assert!(torch_shape.cuboids[0].max[1] <= 0.625);

    let portal = test_block_state_with_tag(
        "minecraft:portal",
        "portal_axis",
        NbtTag::String("x".to_string()),
    );
    let portal_shape =
        detail_shape(&portal, None).unwrap_or_else(|| panic!("portal should have detail shape"));
    assert!(portal_shape.cuboids[0].max[2] - portal_shape.cuboids[0].min[2] < 0.08);
}

#[test]
fn map_viewer_preview_3d_glass_uses_transparent_layer() {
    let bounds = SlimeChunkBounds {
        dimension: bedrock_world::Dimension::Overworld,
        min_chunk_x: 0,
        max_chunk_x: 0,
        min_chunk_z: 0,
        max_chunk_z: 0,
    };
    let glass = block_face(
        BlockKey { x: 8, y: 64, z: 8 },
        FACE_DEFINITIONS[0],
        [0.50, 0.72, 0.95, GLASS_ALPHA],
    );
    let mesh = test_mesh_with_layers(Vec::new(), vec![glass], Vec::new(), bounds);

    assert_eq!(mesh.face_count, 0);
    assert_eq!(mesh.glass_face_count, 1);
    assert_eq!(mesh.water_face_count, 0);
    let gpu_mesh = test_chunk_mesh(&mesh);
    assert_eq!(gpu_mesh.ranges.opaque.count, 0);
    assert_eq!(gpu_mesh.ranges.glass.start, 0);
    assert_eq!(gpu_mesh.ranges.glass.count, 6);
    assert_eq!(gpu_mesh.ranges.water.start, 6);
    assert_eq!(gpu_mesh.mesh.vertices()[0].color[3], GLASS_ALPHA);
}
