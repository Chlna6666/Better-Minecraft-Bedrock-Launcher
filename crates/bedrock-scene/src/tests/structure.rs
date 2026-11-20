use super::*;
use bedrock_world::editor::{BlockOffset, BlockPlacementPlan, PlacementBlock};
use std::sync::OnceLock;

fn voxel_palette() -> &'static RenderPalette {
    static PALETTE: OnceLock<RenderPalette> = OnceLock::new();
    PALETTE.get_or_init(|| {
        let mut palette = RenderPalette::default();
        // Converted models are colored by the voxelizer's 26.60 palette.
        // Override the builtin table so the 3D preview resolves exactly the
        // same appearance colors for every voxel candidate, including block
        // families the builtin table predates.
        let color =
            |channels: [u8; 4]| RgbaColor::new(channels[0], channels[1], channels[2], channels[3]);
        for candidate in bedrock_voxel::default_block_candidates() {
            palette.set_block_color(candidate.state.name.clone(), color(candidate.top_color));
            palette.set_block_side_color(candidate.state.name.clone(), color(candidate.side_color));
        }
        palette
    })
}

#[test]
fn imported_structure_faces_are_not_darkened_by_world_lighting() {
    let palette = voxel_palette();
    let candidates = bedrock_voxel::default_block_candidates();
    for name in ["minecraft:white_concrete", "minecraft:red_concrete"] {
        let candidate = candidates
            .iter()
            .find(|candidate| candidate.state.name == name)
            .expect("opaque preview reference block");
        let colors = block_face_colors(palette, &candidate.state, None, false);
        for (actual, expected) in [
            (colors.up, candidate.top_color),
            (colors.down, candidate.top_color),
            (colors.side, candidate.side_color),
        ] {
            let expected = expected.map(|value| f32::from(value) / 255.0);
            for channel in 0..3 {
                assert!(
                    (actual[channel] - expected[channel]).abs() <= 0.07,
                    "{} channel={channel} actual={} expected={}",
                    candidate.state.name,
                    actual[channel],
                    expected[channel]
                );
            }
        }
    }
}

#[test]
fn imported_structure_preview_uses_voxel_candidate_colors() {
    // The preview palette is derived from the voxel candidates. Keep a
    // small tolerance so a future renderer-side color normalization does
    // not turn this into a bit-exact snapshot test.
    let close = |actual: [u8; 4], expected: [u8; 4]| {
        actual
            .iter()
            .zip(expected)
            .all(|(actual, expected)| actual.abs_diff(expected) <= 16)
    };
    for candidate in bedrock_voxel::default_block_candidates() {
        let palette = voxel_palette();
        assert!(
            close(
                palette.block_color(&candidate.state.name).to_array(),
                candidate.top_color
            ),
            "top of {}: {:?} vs {:?}",
            candidate.state.name,
            palette.block_color(&candidate.state.name).to_array(),
            candidate.top_color
        );
        assert!(
            close(
                palette.block_side_color(&candidate.state.name).to_array(),
                candidate.side_color
            ),
            "side of {}: {:?} vs {:?}",
            candidate.state.name,
            palette.block_side_color(&candidate.state.name).to_array(),
            candidate.side_color
        );
        if candidate.top_color[3] == 255 && candidate.side_color[3] == 255 {
            let faces = block_face_colors(palette, &candidate.state, None, true);
            let close_face = |actual: [f32; 4], expected: [u8; 4]| {
                // Face shading scales RGB only; alpha keeps the palette value.
                actual
                    .iter()
                    .zip(expected)
                    .take(3)
                    .all(|(actual, expected)| {
                        (actual - f32::from(expected) / 255.0).abs() <= 16.0 / 255.0
                    })
            };
            let expected_up = candidate
                .top_color
                .map(|channel| f32::from(channel) / 255.0);
            let expected_side = candidate
                .side_color
                .map(|channel| (f32::from(channel) / 255.0) * 0.8);
            assert!(
                close_face(faces.up, expected_up.map(|v| (v * 255.0).round() as u8)),
                "face up of {}: {:?} vs {:?}",
                candidate.state.name,
                faces.up,
                candidate.top_color
            );
            assert!(
                close_face(faces.side, expected_side.map(|v| (v * 255.0).round() as u8)),
                "face side of {}: {:?} vs {:?}",
                candidate.state.name,
                faces.side,
                candidate.side_color
            );
        }
    }
}

#[test]
fn imported_structure_builds_visible_block_mesh_at_selected_height() {
    let blocks = [
        (0, "minecraft:red_concrete"),
        (1, "minecraft:blue_concrete"),
    ]
    .into_iter()
    .map(|(x, name)| PlacementBlock {
        offset: BlockOffset { x, y: 0, z: 0 },
        state: BlockState {
            name: name.to_owned(),
            states: BTreeMap::new(),
            version: None,
        },
    })
    .collect();
    let plan = BlockPlacementPlan::new(blocks).expect("plan");
    let structure =
        ::bedrock_world::structure::McStructureFile::from_placement_plan(&plan).expect("structure");
    let mesh = crate::structure::build(
        &structure,
        ChunkPos {
            x: 0,
            z: 0,
            dimension: bedrock_world::Dimension::Overworld,
        },
        70,
        voxel_palette(),
    )
    .expect("mesh");
    assert_eq!(mesh.solid_block_count, 2);
    assert!(mesh.face_count > 0);
    assert!(mesh.vertex_count() > 0);
    assert_eq!(mesh.min_y, 70);
}
