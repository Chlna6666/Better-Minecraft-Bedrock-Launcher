use super::*;

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
