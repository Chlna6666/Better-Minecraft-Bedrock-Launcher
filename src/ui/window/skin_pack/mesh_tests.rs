use super::super::color::Face;
use super::super::custom_geometry_animation::CustomGeometryBoneRole;
use super::super::geometry::*;
use super::super::uv::TextureRegion;
use super::*;
use gpui::{GpuMesh3d, GpuMesh3dDrawRanges, GpuMesh3dRange, GpuMesh3dVertex};
use image::{DynamicImage, ImageBuffer, Rgba};
use std::sync::Arc;

#[test]
fn default_pose_places_limbs_on_body_sides() {
    for (part, expected) in [
        (SkinPreviewPart::RightArm { width: 4.0 }, [-6.0, 2.0, 0.0]),
        (SkinPreviewPart::LeftArm { width: 4.0 }, [6.0, 2.0, 0.0]),
        (SkinPreviewPart::RightArm { width: 3.0 }, [-5.5, 2.0, 0.0]),
        (SkinPreviewPart::RightLeg, [-2.0, -10.0, 0.0]),
        (SkinPreviewPart::LeftLeg, [2.0, -10.0, 0.0]),
    ] {
        assert_translation(skin_part_transform(part, 0.0), expected);
    }
}

#[test]
fn custom_geometry_limb_transform_keeps_geometry_pivot_fixed() {
    let pivot = [5.0, 6.0, 0.0];
    let transform = skin_part_transform(
        SkinPreviewPart::CustomGeometryBone {
            role: CustomGeometryBoneRole::LeftArm,
            pivot,
        },
        0.5,
    );

    assert_point_near(transform_point(transform, pivot), pivot);
}

#[test]
fn cuboid_faces_use_outward_winding() {
    let size = CuboidSize {
        width: 8.0,
        height: 8.0,
        depth: 8.0,
    };
    let grid = FaceGrid {
        width: 8,
        height: 8,
    };

    for (face, expected_axis, expected_sign) in [
        (Face::Front, 2, 1.0),
        (Face::Back, 2, -1.0),
        (Face::Right, 0, -1.0),
        (Face::Left, 0, 1.0),
        (Face::Top, 1, 1.0),
        (Face::Bottom, 1, -1.0),
    ] {
        let normal = quad_normal(face_pixel_corners(size, face, grid, 0, 0, 0.0));
        assert!(
            normal[expected_axis] * expected_sign > 0.0,
            "face winding was not outward for axis {expected_axis}: {normal:?}"
        );
    }
}

#[test]
fn cuboid_face_texture_axes_match_skin_layout() {
    let size = CuboidSize {
        width: 8.0,
        height: 8.0,
        depth: 8.0,
    };
    let grid = FaceGrid {
        width: 8,
        height: 8,
    };

    assert!(pixel_center(size, Face::Front, grid, 0, 4)[0] < 0.0);
    assert!(pixel_center(size, Face::Front, grid, 7, 4)[0] > 0.0);
    assert!(pixel_center(size, Face::Back, grid, 0, 4)[0] > 0.0);
    assert!(pixel_center(size, Face::Back, grid, 7, 4)[0] < 0.0);

    assert!(pixel_center(size, Face::Right, grid, 0, 4)[2] < 0.0);
    assert!(pixel_center(size, Face::Right, grid, 7, 4)[2] > 0.0);
    assert!(pixel_center(size, Face::Left, grid, 0, 4)[2] > 0.0);
    assert!(pixel_center(size, Face::Left, grid, 7, 4)[2] < 0.0);

    assert!(pixel_center(size, Face::Top, grid, 4, 0)[2] < 0.0);
    assert!(pixel_center(size, Face::Top, grid, 4, 7)[2] > 0.0);
}

#[test]
fn high_resolution_skin_uses_extra_preview_subdivision() {
    let region = TextureRegion {
        x: 8,
        y: 8,
        width: 8,
        height: 8,
    };
    let scale = SkinTextureScale::from_width(128);
    let grid = face_grid(region, scale.preview);

    assert_eq!(scale.source, 2);
    assert_eq!(scale.preview, 2);
    assert_eq!(grid.width, 16);
    assert_eq!(grid.height, 16);
    assert_eq!(source_pixel_offset(0, scale), 0);
    assert_eq!(source_pixel_offset(15, scale), 15);
}

#[test]
fn cuboid_face_does_not_encode_edge_alpha_masks() {
    let image = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(64, 64, Rgba([255, 0, 0, 255])));
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    push_face(
        &image,
        SkinTextureScale::from_width(64),
        CuboidSize {
            width: 8.0,
            height: 8.0,
            depth: 8.0,
        },
        Face::Front,
        TextureRegion {
            x: 8,
            y: 8,
            width: 8,
            height: 8,
        },
        0.0,
        false,
        &mut vertices,
        &mut indices,
    );

    assert!(!vertices.is_empty());
    assert!(!indices.is_empty());
    assert!(
        vertices
            .iter()
            .all(|vertex| encoded_edge_mask(vertex.color[3]) == 0)
    );
}

#[test]
fn extruded_skin_layer_adds_edges_for_isolated_overlay_pixel() {
    let mut source = ImageBuffer::from_pixel(64, 64, Rgba([0, 0, 0, 0]));
    source.put_pixel(40, 8, Rgba([255, 0, 0, 255]));
    let image = DynamicImage::ImageRgba8(source);
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    super::super::layer::push_skin_layer(
        &image,
        SkinTextureScale::from_width(64),
        CuboidSize {
            width: 8.0,
            height: 8.0,
            depth: 8.0,
        },
        head_uv(true),
        SKIN_OVERLAY_INFLATE,
        &mut vertices,
        &mut indices,
    );

    assert_eq!(vertices.len(), 30);
    assert_eq!(indices.len(), 30);
    assert!(
        vertices
            .iter()
            .all(|vertex| encoded_edge_mask(vertex.color[3]) == 0)
    );
}

#[test]
fn preview_paint_meshes_draw_original_then_antialias_underlay() -> Result<(), String> {
    let shader = skin_preview_shader()?;
    let mesh = Arc::new(GpuMesh3d::new(
        vec![
            GpuMesh3dVertex {
                position: [-1.0, -1.0, 0.0],
                color: [1.0, 0.0, 0.0, 1.0],
            },
            GpuMesh3dVertex {
                position: [1.0, -1.0, 0.0],
                color: [1.0, 0.0, 0.0, 1.0],
            },
            GpuMesh3dVertex {
                position: [0.0, 1.0, 0.0],
                color: [1.0, 0.0, 0.0, 1.0],
            },
        ],
        vec![0, 1, 2],
        GpuMesh3dDrawRanges {
            opaque: GpuMesh3dRange { start: 0, count: 3 },
            glass: GpuMesh3dRange::default(),
            water: GpuMesh3dRange::default(),
        },
        [0.0, 0.0, 0.0],
        1.0,
        1.0,
        shader,
    ));
    let meshes = SkinPreviewMeshes {
        parts: Arc::from(
            vec![SkinPreviewPartMesh {
                part: SkinPreviewPart::Body,
                mesh: mesh.clone(),
            }]
            .into_boxed_slice(),
        ),
    };
    let paint_meshes = skin_preview_paint_meshes(&meshes, 1.0, 0.0, 0.0, 1.0, 0.0, false);

    assert_eq!(paint_meshes.len(), 1 + SKIN_PREVIEW_ANTIALIAS_PASSES.len());
    assert!(Arc::ptr_eq(&paint_meshes[0].mesh, &mesh));
    assert!((paint_meshes[0].parameters.view_projection_model[0][3] - 1.0).abs() < f32::EPSILON);
    assert!(paint_meshes[0].parameters.view_projection_model[1][3].abs() < f32::EPSILON);
    assert!(paint_meshes[0].parameters.view_projection_model[2][3].abs() < f32::EPSILON);
    assert!((paint_meshes[0].parameters.view_projection_model[3][3] - 1.0).abs() < f32::EPSILON);

    for (paint_mesh, pass) in paint_meshes[1..].iter().zip(SKIN_PREVIEW_ANTIALIAS_PASSES) {
        assert!(Arc::ptr_eq(&paint_mesh.mesh, &mesh));
        assert!(
            (paint_mesh.parameters.view_projection_model[0][3] - SKIN_PREVIEW_ANTIALIAS_OPACITY)
                .abs()
                < f32::EPSILON
        );
        assert!(
            (paint_mesh.parameters.view_projection_model[1][3] - pass.pixel_offset[0]).abs()
                < f32::EPSILON
        );
        assert!(
            (paint_mesh.parameters.view_projection_model[2][3] - pass.pixel_offset[1]).abs()
                < f32::EPSILON
        );
        assert!(
            (paint_mesh.parameters.view_projection_model[3][3]
                - (1.0 + SKIN_PREVIEW_ANTIALIAS_DEPTH_BIAS))
                .abs()
                < f32::EPSILON
        );
    }

    Ok(())
}

#[test]
fn quad_edge_mask_encodes_only_requested_triangle_edges() {
    let corners = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
    ];
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    push_quad_with_edges(
        &mut vertices,
        &mut indices,
        corners,
        [1.0, 0.0, 0.0, 1.0],
        QuadEdgeMask::BOTTOM.union(QuadEdgeMask::RIGHT),
    );

    assert_eq!(vertices.len(), 6);
    assert_eq!(indices, [0, 1, 2, 3, 4, 5]);
    assert_eq!(
        encoded_edge_mask(vertices[0].color[3]),
        TRIANGLE_EDGE_0 | TRIANGLE_EDGE_2
    );
    assert_eq!(encoded_edge_mask(vertices[3].color[3]), 0);

    vertices.clear();
    indices.clear();
    push_quad_with_edges(
        &mut vertices,
        &mut indices,
        corners,
        [1.0, 0.0, 0.0, 1.0],
        QuadEdgeMask::TOP.union(QuadEdgeMask::LEFT),
    );

    assert_eq!(encoded_edge_mask(vertices[0].color[3]), 0);
    assert_eq!(
        encoded_edge_mask(vertices[3].color[3]),
        TRIANGLE_EDGE_0 | TRIANGLE_EDGE_1
    );
}

fn assert_translation(matrix: [[f32; 4]; 4], expected: [f32; 3]) {
    for (actual, expected) in [matrix[3][0], matrix[3][1], matrix[3][2]]
        .into_iter()
        .zip(expected)
    {
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

fn transform_point(matrix: [[f32; 4]; 4], point: [f32; 3]) -> [f32; 3] {
    [
        point[0] * matrix[0][0] + point[1] * matrix[1][0] + point[2] * matrix[2][0] + matrix[3][0],
        point[0] * matrix[0][1] + point[1] * matrix[1][1] + point[2] * matrix[2][1] + matrix[3][1],
        point[0] * matrix[0][2] + point[1] * matrix[1][2] + point[2] * matrix[2][2] + matrix[3][2],
    ]
}

fn encoded_edge_mask(alpha: f32) -> u8 {
    (alpha * 0.5).floor() as u8
}

fn pixel_center(size: CuboidSize, face: Face, grid: FaceGrid, px: u32, py: u32) -> [f32; 3] {
    quad_center(face_pixel_corners(size, face, grid, px, py, 0.0))
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
