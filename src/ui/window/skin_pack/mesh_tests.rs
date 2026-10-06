use super::super::color::Face;
use super::super::uv::head_uv;
use super::*;
use gpui_3d::{AlphaMode, Mesh, PreparedScene, Vec3, Vertex};
use image::{DynamicImage, ImageBuffer, Rgba};
use std::time::Duration;

/// A skin texture whose head-front region carries a recognizable per-texel pattern.
fn patterned_skin() -> DynamicImage {
    let mut image = ImageBuffer::from_pixel(64, 64, Rgba([0, 0, 0, 0]));
    // Head front: x 8..16, y 8..16. Left half red, right half blue.
    for y in 8..16u32 {
        for x in 8..16u32 {
            let color = if x < 12 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 255, 255])
            };
            image.put_pixel(x, y, color);
        }
    }
    // Body front: x 20..28, y 20..32. Solid green.
    for y in 20..32u32 {
        for x in 20..28u32 {
            image.put_pixel(x, y, Rgba([0, 255, 0, 255]));
        }
    }
    DynamicImage::ImageRgba8(image)
}

#[test]
fn default_pose_places_limbs_on_body_sides() {
    for (part, expected_pivot, expected_center) in [
        (
            SkinPreviewPart::RightArm { width: 4.0 },
            [-6.0, 8.0, 0.0],
            [-6.0, 2.0, 0.0],
        ),
        (
            SkinPreviewPart::LeftArm { width: 4.0 },
            [6.0, 8.0, 0.0],
            [6.0, 2.0, 0.0],
        ),
        (
            SkinPreviewPart::RightArm { width: 3.0 },
            [-5.5, 8.0, 0.0],
            [-5.5, 2.0, 0.0],
        ),
        (
            SkinPreviewPart::RightLeg,
            [-2.0, -4.0, 0.0],
            [-2.0, -10.0, 0.0],
        ),
        (
            SkinPreviewPart::LeftLeg,
            [2.0, -4.0, 0.0],
            [2.0, -10.0, 0.0],
        ),
    ] {
        let (pivot, mesh_offset, _) = skin_part_layout(part);
        assert_translation(pivot, expected_pivot);
        assert_translation(
            [
                pivot[0] + mesh_offset[0],
                pivot[1] + mesh_offset[1],
                pivot[2] + mesh_offset[2],
            ],
            expected_center,
        );
    }
}

#[test]
fn custom_geometry_limb_keeps_its_authored_pivot() {
    let pivot = [5.0, 6.0, 0.0];
    let (parent_translation, mesh_offset, swing_sign) =
        skin_part_layout(SkinPreviewPart::CustomGeometryBone {
            role: CustomGeometryBoneRole::LeftArm,
            pivot,
        });

    assert_point_near(parent_translation, pivot);
    assert_point_near(mesh_offset, [-pivot[0], -pivot[1], -pivot[2]]);
    assert_eq!(swing_sign, -1.0);
}

#[test]
fn cuboid_faces_use_outward_winding() {
    let size = CuboidSize {
        width: 8.0,
        height: 8.0,
        depth: 8.0,
    };
    let grid = FaceGrid {
        width: 1,
        height: 1,
    };
    let run = ColorRun {
        x: 0,
        y: 0,
        width: 1,
        height: 1,
        color_index: 0,
    };

    for (face, expected_axis, expected_sign) in [
        (Face::Front, 2, 1.0),
        (Face::Back, 2, -1.0),
        (Face::Right, 0, -1.0),
        (Face::Left, 0, 1.0),
        (Face::Top, 1, 1.0),
        (Face::Bottom, 1, -1.0),
    ] {
        let normal = quad_normal(face_rect_corners(size, face, grid, run, 0.0));
        assert!(
            normal[expected_axis] * expected_sign > 0.0,
            "face winding was not outward for axis {expected_axis}: {normal:?}"
        );
    }
}

#[test]
fn a_skin_part_is_one_textured_cuboid() {
    let mesh = build_cuboid_mesh(
        CuboidSize {
            width: 8.0,
            height: 8.0,
            depth: 8.0,
        },
        head_uv(false),
        0.0,
    )
    .expect("cuboid mesh should build");

    // Six faces as six independent quads: 36 indices over 24 shared corners.
    assert_eq!(mesh.indices().len(), 36);
    assert_eq!(mesh.vertices().len(), 24);
    assert!(
        mesh.uses_uv_regions(),
        "a cuboid without texture regions would render flat vertex colors",
    );
}

#[test]
fn cuboid_face_uvs_address_the_authored_atlas_region() {
    let mesh = build_cuboid_mesh(
        CuboidSize {
            width: 8.0,
            height: 8.0,
            depth: 8.0,
        },
        head_uv(false),
        0.0,
    )
    .expect("cuboid mesh should build");
    // The head front region is x 8..16, y 8..16 of a 64x64 atlas.
    let front = mesh
        .vertices()
        .iter()
        .filter(|vertex| vertex.normal.z > 0.5)
        .collect::<Vec<_>>();

    assert_eq!(front.len(), 4);
    let min_u = front.iter().map(|v| v.uv.x).fold(f32::MAX, f32::min);
    let max_u = front.iter().map(|v| v.uv.x).fold(f32::MIN, f32::max);
    let min_v = front.iter().map(|v| v.uv.y).fold(f32::MAX, f32::min);
    let max_v = front.iter().map(|v| v.uv.y).fold(f32::MIN, f32::max);
    assert!((min_u - 0.125).abs() < 1.0e-5, "unexpected front u min {min_u}");
    assert!((max_u - 0.25).abs() < 1.0e-5, "unexpected front u max {max_u}");
    assert!((min_v - 0.125).abs() < 1.0e-5, "unexpected front v min {min_v}");
    assert!((max_v - 0.25).abs() < 1.0e-5, "unexpected front v max {max_v}");
}

#[test]
fn prepared_skin_preview_binds_the_skin_texture_to_every_part() {
    let image = patterned_skin();
    let texture = skin_texture_asset(&image).expect("skin texture asset should build");
    let preview = build_skin_player_meshes(texture.clone(), false, SkinLayerMode::Extruded)
        .expect("extruded skin preview should build");
    let prepared = PreparedScene::new(
        preview.scene_view.scene(),
        preview.scene_view.camera(),
        1.0,
    )
    .expect("preview scene should prepare");

    assert_eq!(prepared.draws.len(), 12);
    let mut opaque = 0;
    let mut masked = 0;
    for draw in &prepared.draws {
        assert_eq!(
            draw.material.albedo_texture,
            Some(texture.id()),
            "every preview part must sample the skin atlas instead of flat vertex colors",
        );
        assert!(
            draw.mesh.uses_uv_regions(),
            "a draw without UV regions cannot sample the atlas",
        );
        match draw.material.alpha_mode {
            AlphaMode::Opaque => opaque += 1,
            AlphaMode::Mask => masked += 1,
            AlphaMode::Blend => panic!("preview parts must not need blend ordering"),
        }
    }
    assert_eq!(opaque, 6, "six base parts");
    assert_eq!(masked, 6, "six overlay parts");
}

#[test]
fn layered_and_flat_modes_use_the_authored_layer_alpha() {
    let image = patterned_skin();
    let texture = skin_texture_asset(&image).expect("skin texture asset should build");
    let flat = build_skin_player_meshes(texture.clone(), false, SkinLayerMode::Flat)
        .expect("flat skin preview should build");
    let extruded = build_skin_player_meshes(texture, false, SkinLayerMode::Extruded)
        .expect("extruded skin preview should build");

    let flat_draws = PreparedScene::new(
        flat.scene_view.scene(),
        flat.scene_view.camera(),
        1.0,
    )
    .expect("flat scene should prepare")
    .draws
    .len();
    let extruded_draws = PreparedScene::new(
        extruded.scene_view.scene(),
        extruded.scene_view.camera(),
        1.0,
    )
    .expect("extruded scene should prepare")
    .draws
    .len();

    // Six parts: one pivot node each. The overlay shares its part's pivot, so extruded mode adds
    // one mesh node per part instead of another pivot.
    assert_eq!(flat.scene_view.scene().len(), 12);
    assert_eq!(extruded.scene_view.scene().len(), 18);
    assert_eq!(flat_draws, 6);
    assert_eq!(extruded_draws, 12);
}

#[test]
fn prepared_skin_preview_geometry_stays_small() {
    let image = patterned_skin();
    let texture = skin_texture_asset(&image).expect("skin texture asset should build");
    let preview = build_skin_player_meshes(texture, false, SkinLayerMode::Extruded)
        .expect("extruded skin preview should build");
    let prepared = PreparedScene::new(
        preview.scene_view.scene(),
        preview.scene_view.camera(),
        1.0,
    )
    .expect("preview scene should prepare");

    let triangles = prepared
        .draws
        .iter()
        .map(|draw| draw.mesh.indices().len() / 3)
        .sum::<usize>();
    assert_eq!(
        triangles,
        12 * 6,
        "a textured preview must stay at one cuboid per part",
    );
}

#[test]
fn baked_skin_mesh_converts_srgb_vertex_colors_to_linear() {
    let color = [0.5, 0.25, 1.0, 0.4];
    let vertices = vec![
        SkinVertex {
            position: [0.0, 0.0, 0.0],
            color,
            edge_mask: 0,
        },
        SkinVertex {
            position: [1.0, 0.0, 0.0],
            color,
            edge_mask: 0,
        },
        SkinVertex {
            position: [0.0, 1.0, 0.0],
            color,
            edge_mask: 0,
        },
    ];
    let mesh = build_skin_mesh(vertices, vec![0, 1, 2]).expect("baked skin mesh should build");
    let converted = mesh.vertices()[0].color;

    assert!((converted[0] - 0.21404114).abs() < 1.0e-5);
    assert!((converted[1] - 0.05087609).abs() < 1.0e-5);
    assert!((converted[2] - 1.0).abs() < 1.0e-6);
    assert!((converted[3] - 0.4).abs() < 1.0e-6);
}

#[test]
fn skin_view_changes_reuse_scene_and_renderer_identity() -> Result<(), String> {
    let mesh = Arc::new(
        Mesh::new(
            [
                Vertex {
                    position: Vec3::new(-1.0, -1.0, 0.0),
                    normal: Vec3::Z,
                    uv: Vec2::ZERO,
                    color: [1.0, 0.0, 0.0, 1.0],
                },
                Vertex {
                    position: Vec3::new(1.0, -1.0, 0.0),
                    normal: Vec3::Z,
                    uv: Vec2::ZERO,
                    color: [1.0, 0.0, 0.0, 1.0],
                },
                Vertex {
                    position: Vec3::new(0.0, 1.0, 0.0),
                    normal: Vec3::Z,
                    uv: Vec2::ZERO,
                    color: [1.0, 0.0, 0.0, 1.0],
                },
            ],
            [0, 1, 2],
        )
        .map_err(|error| error.to_string())?,
    );
    let texture = skin_texture_asset(&patterned_skin())?;
    let mut scene = Scene::new();
    let material = baked_skin_material(AlphaMode::Opaque);
    insert_part_node(
        &mut scene,
        [0.0, 0.0, 0.0],
        [0.0, 2.0, 0.0],
        mesh,
        &material,
    )?;
    let meshes = finish_skin_preview(scene, texture, Vec::new())?;
    let base = skin_preview_scene_view(&meshes, 0.0, 0.0, 1.0, Duration::ZERO)?;
    let rotated = skin_preview_scene_view(&meshes, 0.4, -0.2, 1.0, Duration::ZERO)?;
    let zoomed = skin_preview_scene_view(&meshes, 0.0, 0.0, 1.2, Duration::ZERO)?;

    assert!(Arc::ptr_eq(meshes.scene_view.scene(), rotated.scene()));
    assert_eq!(base.scene_transform(), rotated.scene_transform());
    assert_eq!(base.scene_transform(), zoomed.scene_transform());
    assert_ne!(base.camera(), zoomed.camera());
    assert_ne!(base.camera(), rotated.camera());
    let camera = base.camera();
    assert!((camera.eye.x - camera.target.x).abs() < 1.0e-5);
    assert!((camera.eye.y - camera.target.y).abs() < 1.0e-5);
    assert!(camera.eye.z > camera.target.z);
    assert!(matches!(camera.projection, Projection::Orthographic { .. }));
    let Projection::Orthographic {
        height: base_height,
        ..
    } = camera.projection
    else {
        unreachable!("skin preview should use orthographic projection")
    };
    let Projection::Orthographic {
        height: rotated_height,
        ..
    } = rotated.camera().projection
    else {
        unreachable!("skin preview should use orthographic projection")
    };
    let Projection::Orthographic {
        height: zoomed_height,
        ..
    } = zoomed.camera().projection
    else {
        unreachable!("skin preview should use orthographic projection")
    };
    assert!(
        (rotated_height - base_height).abs() < 1.0e-6,
        "orbiting must not change skin preview scale",
    );
    assert_eq!(base.camera().target, rotated.camera().target);
    assert!(zoomed_height < base_height);
    assert!((base.camera().target.y - 2.0 * SKIN_PREVIEW_SCALE).abs() < 1.0e-4);
    let view_projection = base.camera().view_projection(1.0).unwrap();
    let front = view_projection.transform4([0.0, 0.5, 0.5, 1.0]);
    let back = view_projection.transform4([0.0, 0.5, -0.5, 1.0]);
    assert!((front[1] / front[3] - back[1] / back[3]).abs() < 1.0e-5);
    assert_eq!(meshes.scene_view.scene().len(), 2);
    Ok(())
}

#[test]
fn orbit_pitch_keeps_rotation_invariant_preview_scale() -> Result<(), String> {
    let texture = skin_texture_asset(&patterned_skin())?;
    let preview = build_skin_player_meshes(texture, false, SkinLayerMode::Extruded)?;
    let base = skin_preview_scene_view(&preview, 0.0, 0.0, 1.0, Duration::ZERO)?;
    let upper = skin_preview_scene_view(&preview, 0.0, 0.45, 1.0, Duration::ZERO)?;
    let lower = skin_preview_scene_view(&preview, 0.0, -0.75, 1.0, Duration::ZERO)?;
    let diagonal = skin_preview_scene_view(&preview, 1.25, -0.55, 1.0, Duration::ZERO)?;

    let height = |view: &Arc<SceneView>| match view.camera().projection {
        Projection::Orthographic { height, .. } => height,
        Projection::Perspective { .. } => panic!("skin preview must stay orthographic"),
    };
    let expected = height(&base);
    for actual in [height(&upper), height(&lower), height(&diagonal)] {
        assert!(
            (actual - expected).abs() < 1.0e-6,
            "yaw/pitch must not mutate orthographic preview height",
        );
    }
    Ok(())
}

#[test]
fn camera_moves_keep_the_retained_scene_snapshot() -> Result<(), String> {
    let texture = skin_texture_asset(&patterned_skin())?;
    let preview = build_skin_player_meshes(texture, false, SkinLayerMode::Extruded)?;
    let first = skin_preview_scene_view(&preview, 0.0, 0.0, 1.0, Duration::ZERO)?;
    let second = skin_preview_scene_view(&preview, 0.3, 0.1, 1.0, Duration::ZERO)?;

    assert!(Arc::ptr_eq(first.scene(), second.scene()));
    assert_eq!(first.scene().len(), second.scene().len());
    Ok(())
}

fn assert_translation(actual: [f32; 3], expected: [f32; 3]) {
    for (actual, expected) in actual.into_iter().zip(expected) {
        assert!(
            (actual - expected).abs() < 0.001,
            "expected translation {expected}, got {actual}",
        );
    }
}

fn assert_point_near(actual: [f32; 3], expected: [f32; 3]) {
    for (actual, expected) in actual.into_iter().zip(expected) {
        assert!(
            (actual - expected).abs() < 0.001,
            "expected point coordinate {expected}, got {actual}",
        );
    }
}

fn quad_normal(corners: [[f32; 3]; 4]) -> [f32; 3] {
    let a = [
        corners[1][0] - corners[0][0],
        corners[1][1] - corners[0][1],
        corners[1][2] - corners[0][2],
    ];
    let b = [
        corners[2][0] - corners[0][0],
        corners[2][1] - corners[0][1],
        corners[2][2] - corners[0][2],
    ];
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
