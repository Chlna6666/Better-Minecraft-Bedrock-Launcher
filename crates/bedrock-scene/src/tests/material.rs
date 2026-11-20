use super::support::*;
use super::*;

#[test]
fn dry_foliage_blocks_tint_without_biome_data() {
    // A block outside any biome (or in a chunk without biome storage) must still get the
    // palette's built-in dry-foliage default instead of rendering as a raw grey mask.
    let palette = RenderPalette::default();
    let state = test_block_state("minecraft:leaf_litter");
    let tint = texture_tint(&palette, &state, None)
        .expect("leaf_litter should tint even with no biome sample");
    let expected = palette
        .surface_block_tint("minecraft:leaf_litter", None)
        .expect("palette dry foliage default");
    assert_eq!(tint.to_array(), expected.to_array());
    // The dry channel is warm, not the green grass/foliage default.
    let color = tint.to_array();
    assert!(
        color[0] > color[1] && color[1] > color[2],
        "dry foliage tint {color:?} should be a warm brown"
    );

    // An explicit biome still wins.
    let biome = BiomeSample {
        biome_id: Some(2),
        dry_foliage_tint: Some(RgbaColor::new(123, 83, 52, 255)),
        ..BiomeSample::empty()
    };
    let biome_tint = texture_tint(&palette, &state, Some(biome)).expect("biome tint");
    assert_eq!(biome_tint.to_array(), [123, 83, 52, 255]);
}

#[test]
fn map_viewer_preview_3d_grass_faces_use_tint_and_dirt_side_color() {
    let palette = RenderPalette::default()
        .with_block_color(
            "minecraft:dirt",
            bedrock_map_render::RgbaColor::new(120, 80, 45, 255),
        )
        .with_block_color(
            "minecraft:water",
            bedrock_map_render::RgbaColor::new(10, 90, 220, 255),
        );
    let grass_state = test_block_state("minecraft:grass_block");
    let water_state = test_block_state("minecraft:water");

    let grass_colors = block_face_colors(&palette, &grass_state, None, true);
    let water_color = water_color(&palette, &water_state, None);
    let grass_tint = color_from_rgba(
        palette
            .surface_block_tint("minecraft:grass_block", None)
            .expect("grass block should be biome-tinted"),
        1.0,
    );
    let dirt_color = color_from_rgba(palette.block_color("minecraft:dirt"), 1.0);

    assert_eq!(grass_colors.up, grass_tint);
    assert_eq!(grass_colors.side, shade_face(grass_tint, FACE_SHADE_SIDE));
    assert_eq!(grass_colors.down, shade_face(dirt_color, FACE_SHADE_DOWN));
    assert!(water_color[2] > water_color[1]);
    assert!((water_color[3] - WATER_ALPHA).abs() < 0.001);
}

#[test]
fn map_viewer_preview_3d_water_color_falls_back_from_washed_out_palette_color() {
    let palette = RenderPalette::default().with_block_color(
        "minecraft:water",
        bedrock_map_render::RgbaColor::new(240, 240, 245, 255),
    );
    let water_state = test_block_state("minecraft:water");

    let water_color = water_color(&palette, &water_state, None);

    assert!(water_color[2] > water_color[1]);
    assert!(water_color[1] > water_color[0]);
    assert!(luminance(water_color) < 0.66);
    assert!((water_color[3] - WATER_ALPHA).abs() < 0.001);
}

#[test]
fn map_viewer_preview_3d_washed_out_water_color_detection_rejects_white() {
    assert!(water_color_is_washed_out([0.94, 0.94, 0.96, WATER_ALPHA,]));
    assert!(!water_color_is_washed_out([
        DEFAULT_WATER_RGB[0],
        DEFAULT_WATER_RGB[1],
        DEFAULT_WATER_RGB[2],
        WATER_ALPHA,
    ]));
}

#[test]
fn map_viewer_preview_3d_grass_and_leaves_use_biome_tints() {
    let palette = RenderPalette::default();
    let grass_block_state = BlockState {
        name: "minecraft:grass_block".to_string(),
        states: Default::default(),
        version: None,
    };
    let legacy_grass_state = BlockState {
        name: "minecraft:grass".to_string(),
        states: Default::default(),
        version: None,
    };
    let leaf_state = BlockState {
        name: "minecraft:oak_leaves".to_string(),
        states: Default::default(),
        version: None,
    };

    let grass_color = block_color(&palette, &grass_block_state, None);
    let legacy_grass_color = block_color(&palette, &legacy_grass_state, None);
    let leaf_color = block_color(&palette, &leaf_state, None);

    assert_eq!(
        grass_color,
        color_from_rgba(
            palette
                .surface_block_tint("minecraft:grass_block", None)
                .expect("grass block should be biome-tinted"),
            1.0,
        )
    );
    assert_eq!(legacy_grass_color, grass_color);
    assert_eq!(
        leaf_color,
        color_from_rgba(
            palette
                .surface_block_tint("minecraft:oak_leaves", None)
                .expect("oak leaves should be biome-tinted"),
            1.0,
        )
    );
    assert!((leaf_color[3] - 1.0).abs() < 0.001);
}

#[test]
fn dry_foliage_blocks_keep_a_default_colour_without_any_biome_storage() {
    // A chunk with no biome storage at all produces no blend column, so the block falls back
    // to the palette's built-in default dry-foliage colour instead of a raw grey mask.
    let chunk = test_render_chunk_with_subchunks(BTreeMap::new());
    let field = BiomeBlendField::from_chunks(&[chunk]);
    assert!(
        field.tints(0, 0).is_none(),
        "no biome storage means no blended tint"
    );

    let palette = RenderPalette::default();
    let state = test_block_state("minecraft:leaf_litter");
    let tint = texture_tint(&palette, &state, None).expect("default dry foliage tint");
    let expected = palette
        .surface_block_tint("minecraft:leaf_litter", None)
        .expect("palette default");
    assert_eq!(tint.to_array(), expected.to_array());
    // Warm brown, not the green grass/foliage default.
    let [red, green, blue, _] = expected.to_array();
    assert!(
        red > green && green > blue,
        "default {expected:?} should be warm"
    );
}

#[test]
fn map_viewer_preview_3d_biome_blend_averages_the_eight_neighbors() {
    let field = BiomeBlendField::from_chunks(&[test_biome_chunk(0, 1), test_biome_chunk(1, 21)]);
    let palette = RenderPalette::default();
    let average = |left: RgbaColor, right: RgbaColor| {
        let left = left.to_array();
        let right = right.to_array();
        let blend = |channel: usize| {
            let sum = u32::from(left[channel]) * 6 + u32::from(right[channel]) * 3;
            u8::try_from((sum + 4) / 9).unwrap_or(u8::MAX)
        };
        RgbaColor::new(blend(0), blend(1), blend(2), 255)
    };

    // Column 15 sits on the border: 6 samples come from chunk 0, 3 from chunk 1.
    let border = field.tints(15, 8).expect("border column");
    assert_eq!(
        border.grass,
        average(
            palette.biome_grass_color(Some(1)),
            palette.biome_grass_color(Some(21))
        )
    );
    assert_eq!(
        border.foliage,
        average(
            palette.biome_foliage_color(Some(1)),
            palette.biome_foliage_color(Some(21))
        )
    );
    // The dry-foliage channel blends across the border too.
    let dry = |id| {
        palette
            .surface_block_tint("minecraft:leaf_litter", Some(id))
            .expect("dry foliage tint")
    };
    assert_eq!(border.dry_foliage, average(dry(1), dry(21)));
    assert_eq!(
        field.tints(4, 8).expect("interior column").grass,
        palette.biome_grass_color(Some(1))
    );
    assert_eq!(
        field.tints(20, 8).expect("interior column").grass,
        palette.biome_grass_color(Some(21))
    );
}

#[test]
fn map_viewer_preview_3d_legacy_tallgrass_uses_grass_tint() {
    let palette = RenderPalette::default();
    let legacy_state = test_block_state("minecraft:tallgrass");
    let short_grass_state = test_block_state("minecraft:short_grass");

    let legacy_color = block_color(&palette, &legacy_state, None);
    let short_grass_color = block_color(&palette, &short_grass_state, None);

    assert_eq!(legacy_color, short_grass_color);
    assert!(
        legacy_color[1] > legacy_color[0] && legacy_color[1] > legacy_color[2],
        "legacy tallgrass should use the biome grass tint, got {legacy_color:?}"
    );
}

#[test]
fn map_viewer_preview_3d_double_plant_grass_uses_biome_grass_tint() {
    let palette = RenderPalette::default();
    let double_plant = test_block_state_with_tag(
        "minecraft:double_plant",
        "double_plant_type",
        NbtTag::String("grass".to_string()),
    );
    let tall_grass = test_block_state("minecraft:tall_grass");

    let color = block_color(&palette, &double_plant, None);
    assert_eq!(color, block_color(&palette, &tall_grass, None));
    assert!(
        color[1] > color[0] && color[1] > color[2],
        "double plant grass should use the biome grass tint, got {color:?}"
    );
}

#[test]
fn map_viewer_preview_3d_glass_color_uses_palette_with_alpha() {
    let palette = RenderPalette::default().with_block_color(
        "minecraft:white_stained_glass",
        bedrock_map_render::RgbaColor::new(32, 96, 224, 255),
    );
    let state = BlockState {
        name: "minecraft:white_stained_glass".to_string(),
        states: Default::default(),
        version: None,
    };
    let color = transparent_color(&palette, &state, None);
    let expected = palette
        .block_color("minecraft:white_stained_glass")
        .to_array();
    let glass = block_face(BlockKey { x: 8, y: 64, z: 8 }, FACE_DEFINITIONS[0], color);
    let bounds = SlimeChunkBounds {
        dimension: bedrock_world::Dimension::Overworld,
        min_chunk_x: 0,
        max_chunk_x: 0,
        min_chunk_z: 0,
        max_chunk_z: 0,
    };
    let mesh = test_mesh_with_layers(Vec::new(), vec![glass], Vec::new(), bounds);
    let gpu_chunk_mesh = test_chunk_mesh(&mesh);
    let gpu_mesh = gpu_chunk_mesh.mesh.as_ref();
    let glass_vertex =
        usize::try_from(gpu_mesh.indices()[gpu_chunk_mesh.ranges.glass.start as usize])
            .expect("glass vertex index should fit usize");
    let vertex_color = gpu_mesh.vertices()[glass_vertex].color;

    assert_eq!(gpu_chunk_mesh.ranges.glass.count, 6);
    assert!((vertex_color[0] - f32::from(expected[0]) / 255.0).abs() < 0.001);
    assert!((vertex_color[1] - f32::from(expected[1]) / 255.0).abs() < 0.001);
    assert!((vertex_color[2] - f32::from(expected[2]) / 255.0).abs() < 0.001);
    assert!((vertex_color[3] - GLASS_ALPHA).abs() < 0.001);
}
