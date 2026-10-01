use std::collections::BTreeMap;

use bedrock_world::BlockState;

use super::*;
use crate::{ObjMaterial, ObjVertex};

#[test]
fn overlapping_surfaces_choose_closest_material_independently_of_triangle_order() {
    let mut red = candidate();
    red.state.name = "minecraft:red_concrete".to_owned();
    red.top_color = [255, 0, 0, 255];
    red.side_color = red.top_color;
    let mut blue = candidate();
    blue.state.name = "minecraft:blue_concrete".to_owned();
    blue.top_color = [0, 0, 255, 255];
    blue.side_color = blue.top_color;
    let candidates = [red, blue];
    let mut model = ObjModel {
        triangles: [0.1, 0.4]
            .into_iter()
            .enumerate()
            .map(|(material, z)| ObjTriangle {
                vertices: [[0.0, 0.0, z], [1.0, 0.0, z], [0.0, 1.0, z]]
                    .map(|position| ObjVertex { position, uv: None }),
                material: Some(material),
            })
            .collect(),
        materials: [[1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]
            .into_iter()
            .map(|diffuse| ObjMaterial {
                diffuse: Some(diffuse),
                opacity: 1.0,
                diffuse_texture: None,
            })
            .collect(),
    };
    let options = ObjVoxelOptions {
        longest_side_blocks: 1,
        ..ObjVoxelOptions::default()
    };
    let forward = voxelize_obj(&model, &candidates, options).unwrap();
    model.triangles.reverse();
    let reversed = voxelize_obj(&model, &candidates, options).unwrap();
    assert_eq!(forward.blocks(), reversed.blocks());
    assert!(
        forward
            .blocks()
            .iter()
            .all(|block| block.state.name == "minecraft:blue_concrete")
    );
}

fn candidate() -> FlatBlockCandidate {
    FlatBlockCandidate {
        state: BlockState {
            name: "minecraft:stone".to_owned(),
            states: BTreeMap::new(),
            version: None,
        },
        top_color: [192, 192, 192, 255],
        side_color: [192, 192, 192, 255],
    }
}

fn cube() -> ObjModel {
    let corners = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [1.0, 0.0, 1.0],
        [1.0, 1.0, 1.0],
        [0.0, 1.0, 1.0],
    ];
    let quads = [
        [0, 1, 2, 3],
        [4, 5, 6, 7],
        [0, 1, 5, 4],
        [2, 3, 7, 6],
        [1, 2, 6, 5],
        [0, 3, 7, 4],
    ];
    let triangles = quads
        .into_iter()
        .flat_map(|quad| [[quad[0], quad[1], quad[2]], [quad[0], quad[2], quad[3]]])
        .map(|indices| ObjTriangle {
            vertices: indices.map(|index| ObjVertex {
                position: corners[index],
                uv: None,
            }),
            material: None,
        })
        .collect();
    ObjModel {
        triangles,
        materials: vec![ObjMaterial {
            diffuse: None,
            opacity: 1.0,
            diffuse_texture: None,
        }],
    }
}

#[test]
fn surface_is_smaller_than_solid_and_scale_is_uniform() {
    let model = cube();
    let surface = voxelize_obj(
        &model,
        &[candidate()],
        ObjVoxelOptions {
            longest_side_blocks: 5,
            fill: ObjFill::Surface,
            ..ObjVoxelOptions::default()
        },
    )
    .unwrap();
    let solid = voxelize_obj(
        &model,
        &[candidate()],
        ObjVoxelOptions {
            longest_side_blocks: 5,
            fill: ObjFill::Solid,
            ..ObjVoxelOptions::default()
        },
    )
    .unwrap();
    assert!(surface.blocks().len() < solid.blocks().len());
    assert!(
        solid
            .blocks()
            .iter()
            .any(|block| block.offset == BlockOffset { x: 2, y: 2, z: 2 })
    );
    assert!(solid.blocks().iter().all(|block| {
        [block.offset.x, block.offset.y, block.offset.z]
            .into_iter()
            .all(|axis| (0..5).contains(&axis))
    }));
}

#[test]
fn missing_texture_is_rejected() {
    let mut model = cube();
    model.materials[0].diffuse_texture = Some("texture.png".into());
    assert!(
        voxelize_obj(
            &model,
            &[candidate()],
            ObjVoxelOptions {
                longest_side_blocks: 5,
                ..ObjVoxelOptions::default()
            }
        )
        .is_err()
    );
}

#[test]
fn tiny_model_uses_the_same_target_block_size() {
    let ordinary = voxelize_obj(
        &cube(),
        &[candidate()],
        ObjVoxelOptions {
            longest_side_blocks: 5,
            ..ObjVoxelOptions::default()
        },
    )
    .unwrap();
    let mut tiny = cube();
    for triangle in &mut tiny.triangles {
        for vertex in &mut triangle.vertices {
            vertex.position = vertex.position.map(|value| value * 0.0001);
        }
    }
    let scaled = voxelize_obj(
        &tiny,
        &[candidate()],
        ObjVoxelOptions {
            longest_side_blocks: 5,
            ..ObjVoxelOptions::default()
        },
    )
    .unwrap();
    assert_eq!(ordinary.blocks().len(), scaled.blocks().len());
}

#[test]
fn texture_uvs_select_different_blocks() {
    let texture_path = std::env::temp_dir().join(format!(
        "bmcbl-obj-voxel-texture-{}.png",
        std::process::id()
    ));
    image::RgbaImage::from_raw(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255])
        .unwrap()
        .save(&texture_path)
        .unwrap();
    let model = ObjModel {
        triangles: vec![ObjTriangle {
            vertices: [
                ObjVertex {
                    position: [0.0, 0.0, 0.0],
                    uv: Some([0.0, 0.0]),
                },
                ObjVertex {
                    position: [1.0, 0.0, 0.0],
                    uv: Some([1.0, 0.0]),
                },
                ObjVertex {
                    position: [0.0, 0.0, 1.0],
                    uv: Some([0.0, 1.0]),
                },
            ],
            material: Some(0),
        }],
        materials: vec![ObjMaterial {
            diffuse: None,
            opacity: 1.0,
            diffuse_texture: Some(texture_path.clone()),
        }],
    };
    let red = FlatBlockCandidate {
        state: BlockState {
            name: "minecraft:red_wool".into(),
            states: BTreeMap::new(),
            version: None,
        },
        top_color: [255, 0, 0, 255],
        side_color: [0, 0, 255, 255],
    };
    let blue = FlatBlockCandidate {
        state: BlockState {
            name: "minecraft:blue_wool".into(),
            states: BTreeMap::new(),
            version: None,
        },
        top_color: [0, 0, 255, 255],
        side_color: [0, 0, 255, 255],
    };
    let plan = voxelize_obj(
        &model,
        &[red, blue],
        ObjVoxelOptions {
            longest_side_blocks: 8,
            ..ObjVoxelOptions::default()
        },
    )
    .unwrap();
    assert!(
        plan.blocks()
            .iter()
            .any(|block| block.state.name == "minecraft:red_wool")
    );
    assert!(
        plan.blocks()
            .iter()
            .any(|block| block.state.name == "minecraft:blue_wool")
    );
    std::fs::remove_file(texture_path).unwrap();
}

#[test]
fn vertical_obj_faces_match_block_side_colors_and_horizontal_faces_match_top_colors() {
    let candidates = [
        FlatBlockCandidate {
            state: BlockState {
                name: "minecraft:red_wool".into(),
                states: BTreeMap::new(),
                version: None,
            },
            top_color: [255, 0, 0, 255],
            side_color: [0, 0, 255, 255],
        },
        FlatBlockCandidate {
            state: BlockState {
                name: "minecraft:blue_wool".into(),
                states: BTreeMap::new(),
                version: None,
            },
            top_color: [0, 0, 255, 255],
            side_color: [255, 0, 0, 255],
        },
    ];
    let model = |vertices: [[f32; 3]; 3]| ObjModel {
        triangles: vec![ObjTriangle {
            vertices: vertices.map(|position| ObjVertex { position, uv: None }),
            material: Some(0),
        }],
        materials: vec![ObjMaterial {
            diffuse: Some([1.0, 0.0, 0.0]),
            opacity: 1.0,
            diffuse_texture: None,
        }],
    };
    let horizontal = voxelize_obj(
        &model([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]]),
        &candidates,
        ObjVoxelOptions {
            longest_side_blocks: 4,
            ..ObjVoxelOptions::default()
        },
    )
    .expect("horizontal OBJ face");
    let vertical = voxelize_obj(
        &model([[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]),
        &candidates,
        ObjVoxelOptions {
            longest_side_blocks: 4,
            ..ObjVoxelOptions::default()
        },
    )
    .expect("vertical OBJ face");

    assert!(
        horizontal
            .blocks()
            .iter()
            .all(|block| block.state.name == "minecraft:red_wool")
    );
    assert!(
        vertical
            .blocks()
            .iter()
            .all(|block| block.state.name == "minecraft:blue_wool")
    );
}

#[test]
fn material_opacity_allows_glass_only_for_translucent_surfaces() {
    let mut model = cube();
    model.materials[0].diffuse = Some([1.0; 3]);
    model.materials[0].opacity = 0.5;
    for triangle in &mut model.triangles {
        triangle.material = Some(0);
    }
    let glass = FlatBlockCandidate {
        state: BlockState {
            name: "minecraft:glass".to_owned(),
            states: BTreeMap::new(),
            version: None,
        },
        top_color: [255, 255, 255, 128],
        side_color: [255, 255, 255, 128],
    };
    let white = FlatBlockCandidate {
        state: BlockState {
            name: "minecraft:white_wool".to_owned(),
            states: BTreeMap::new(),
            version: None,
        },
        top_color: [255, 255, 255, 255],
        side_color: [255, 255, 255, 255],
    };
    let translucent = voxelize_obj(
        &model,
        &[glass.clone(), white.clone()],
        ObjVoxelOptions {
            longest_side_blocks: 5,
            ..ObjVoxelOptions::default()
        },
    )
    .unwrap();
    assert!(
        translucent
            .blocks()
            .iter()
            .any(|block| block.state.name == "minecraft:glass")
    );

    model.materials[0].opacity = 1.0;
    let opaque = voxelize_obj(
        &model,
        &[glass, white],
        ObjVoxelOptions {
            longest_side_blocks: 5,
            ..ObjVoxelOptions::default()
        },
    )
    .unwrap();
    assert!(
        opaque
            .blocks()
            .iter()
            .all(|block| block.state.name != "minecraft:glass")
    );
}
