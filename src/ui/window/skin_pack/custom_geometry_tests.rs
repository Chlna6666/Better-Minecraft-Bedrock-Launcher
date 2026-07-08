use super::*;
use image::{ImageBuffer, Rgba};
use serde_json::json;

#[test]
fn legacy_poly_mesh_quad_generates_textured_mesh() {
    let image = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(4, 4, Rgba([255, 0, 0, 255])));
    let geometry = json!({
        "format_version": "1.8.0",
        "geometry.test": {
            "bones": [{
                "name": "body",
                "poly_mesh": {
                    "normalized_uvs": true,
                    "positions": [
                        [0.0, 16.0, 0.0],
                        [1.0, 16.0, 0.0],
                        [1.0, 17.0, 0.0],
                        [0.0, 17.0, 0.0]
                    ],
                    "normals": [[0.0, 0.0, 1.0]],
                    "uvs": [
                        [0.0, 0.0],
                        [0.5, 0.0],
                        [0.5, 0.5],
                        [0.0, 0.5]
                    ],
                    "polys": [
                        [[0, 0, 0], [1, 0, 1], [2, 0, 2], [3, 0, 3]]
                    ]
                }
            }]
        }
    });

    let mesh = build_custom_geometry_from_value(&image, &geometry, "geometry.test")
        .expect("custom geometry should parse")
        .expect("custom geometry should produce mesh");

    assert!(mesh_vertex_count(&mesh) > 0);
    assert!(mesh_index_count(&mesh) > 0);
    assert!(
        mesh.parts
            .iter()
            .all(|part| part.vertices.iter().all(|vertex| vertex.color[3] <= 1.0))
    );
}

#[test]
fn adjacent_poly_mesh_triangles_render_as_texel_quads() {
    let image = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(4, 4, Rgba([255, 0, 0, 255])));
    let geometry = json!({
        "format_version": "1.8.0",
        "geometry.test": {
            "bones": [{
                "name": "body",
                "poly_mesh": {
                    "normalized_uvs": true,
                    "positions": [
                        [0.0, 16.0, 0.0],
                        [2.0, 16.0, 0.0],
                        [0.0, 18.0, 0.0],
                        [2.0, 18.0, 0.0]
                    ],
                    "normals": [[0.0, 0.0, 1.0]],
                    "uvs": [
                        [0.0, 0.0],
                        [0.5, 0.0],
                        [0.0, 0.5],
                        [0.5, 0.5]
                    ],
                    "polys": [
                        [[0, 0, 0], [1, 0, 1], [2, 0, 2], [2, 0, 2]],
                        [[1, 0, 1], [3, 0, 3], [2, 0, 2], [2, 0, 2]]
                    ]
                }
            }]
        }
    });

    let mesh = build_custom_geometry_from_value(&image, &geometry, "geometry.test")
        .expect("custom geometry should parse")
        .expect("custom geometry should produce mesh");

    assert_eq!(mesh_vertex_count(&mesh), 24);
    assert_eq!(mesh_index_count(&mesh), 24);
}

#[test]
fn normalized_poly_mesh_uvs_use_bottom_origin_v_axis() {
    let mut image = ImageBuffer::from_pixel(4, 4, Rgba([0, 0, 0, 0]));
    for x in 0..4 {
        image.put_pixel(x, 3, Rgba([255, 0, 0, 255]));
    }
    let image = DynamicImage::ImageRgba8(image);
    let geometry = json!({
        "format_version": "1.8.0",
        "geometry.test": {
            "bones": [{
                "name": "body",
                "poly_mesh": {
                    "normalized_uvs": true,
                    "positions": [
                        [0.0, 16.0, 0.0],
                        [1.0, 16.0, 0.0],
                        [1.0, 17.0, 0.0],
                        [0.0, 17.0, 0.0]
                    ],
                    "normals": [[0.0, 0.0, 1.0]],
                    "uvs": [
                        [0.0, 0.1],
                        [0.5, 0.1],
                        [0.5, 0.2],
                        [0.0, 0.2]
                    ],
                    "polys": [
                        [[0, 0, 0], [1, 0, 1], [2, 0, 2], [3, 0, 3]]
                    ]
                }
            }]
        }
    });

    let mesh = build_custom_geometry_from_value(&image, &geometry, "geometry.test")
        .expect("custom geometry should parse")
        .expect("bottom-origin normalized UVs should sample nontransparent pixels");

    assert!(mesh_vertex_count(&mesh) > 0);
    assert!(mesh_index_count(&mesh) > 0);
    assert!(
        mesh.parts
            .iter()
            .all(|part| part.vertices.iter().all(|vertex| vertex.color[3] <= 1.0))
    );
}

#[test]
fn poly_mesh_samples_texture_inside_triangle() {
    let mut image = ImageBuffer::from_pixel(4, 4, Rgba([0, 0, 0, 0]));
    image.put_pixel(1, 2, Rgba([255, 0, 0, 255]));
    let image = DynamicImage::ImageRgba8(image);
    let geometry = json!({
        "format_version": "1.8.0",
        "geometry.test": {
            "bones": [{
                "name": "body",
                "poly_mesh": {
                    "normalized_uvs": true,
                    "positions": [
                        [0.0, 16.0, 0.0],
                        [4.0, 16.0, 0.0],
                        [0.0, 20.0, 0.0]
                    ],
                    "normals": [[0.0, 0.0, 1.0]],
                    "uvs": [
                        [0.0, 0.0],
                        [1.0, 0.0],
                        [0.0, 1.0]
                    ],
                    "polys": [
                        [[0, 0, 0], [1, 0, 1], [2, 0, 2]]
                    ]
                }
            }]
        }
    });

    let mesh = build_custom_geometry_from_value(&image, &geometry, "geometry.test")
        .expect("custom geometry should parse")
        .expect("interior texture pixels should generate visible mesh");

    assert!(mesh_vertex_count(&mesh) > 0);
    assert!(mesh_index_count(&mesh) > 0);
}

#[test]
fn modern_cube_geometry_generates_textured_faces() {
    let image = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(64, 64, Rgba([0, 255, 0, 255])));
    let geometry = json!({
        "format_version": "1.12.0",
        "minecraft:geometry": [{
            "description": {
                "identifier": "geometry.test",
                "texture_width": 64,
                "texture_height": 64
            },
            "bones": [{
                "name": "body",
                "pivot": [0.0, 12.0, 0.0],
                "cubes": [{
                    "origin": [-4.0, 12.0, -2.0],
                    "size": [8.0, 12.0, 4.0],
                    "uv": [16.0, 16.0]
                }]
            }]
        }]
    });

    let mesh = build_custom_geometry_from_value(&image, &geometry, "geometry.test")
        .expect("custom cube geometry should parse")
        .expect("custom cube geometry should produce mesh");

    assert!(mesh_vertex_count(&mesh) > 0);
    assert!(mesh_index_count(&mesh) > 0);
}

#[test]
fn zero_depth_cube_geometry_generates_plane_from_unshifted_uv() {
    let mut image = ImageBuffer::from_pixel(4, 4, Rgba([0, 0, 0, 0]));
    image.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
    let image = DynamicImage::ImageRgba8(image);
    let geometry = json!({
        "format_version": "1.8.0",
        "geometry.test": {
            "texturewidth": 4,
            "textureheight": 4,
            "bones": [{
                "name": "body",
                "cubes": [{
                    "origin": [0.0, 16.0, 0.0],
                    "size": [1.0, 1.0, 0.0],
                    "uv": [0.0, 0.0]
                }]
            }]
        }
    });

    let mesh = build_custom_geometry_from_value(&image, &geometry, "geometry.test")
        .expect("custom cube geometry should parse")
        .expect("zero-depth cube should produce a plane mesh");

    assert_eq!(mesh_vertex_count(&mesh), 12);
    assert_eq!(mesh_index_count(&mesh), 12);
}

#[test]
fn player_limb_geometry_uses_limb_animation_binding() {
    let image = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(4, 4, Rgba([255, 0, 0, 255])));
    let geometry = json!({
        "format_version": "1.8.0",
        "geometry.test": {
            "bones": [{
                "name": "body",
                "pivot": [0.0, 24.0, 0.0]
            }, {
                "name": "leftArm",
                "parent": "body",
                "pivot": [5.0, 22.0, 0.0],
                "poly_mesh": {
                    "normalized_uvs": true,
                    "positions": [
                        [4.0, 16.0, 0.0],
                        [6.0, 16.0, 0.0],
                        [6.0, 18.0, 0.0],
                        [4.0, 18.0, 0.0]
                    ],
                    "normals": [[0.0, 0.0, 1.0]],
                    "uvs": [
                        [0.0, 0.0],
                        [0.5, 0.0],
                        [0.5, 0.5],
                        [0.0, 0.5]
                    ],
                    "polys": [
                        [[0, 0, 0], [1, 0, 1], [2, 0, 2], [3, 0, 3]]
                    ]
                }
            }]
        }
    });

    let mesh = build_custom_geometry_from_value(&image, &geometry, "geometry.test")
        .expect("custom geometry should parse")
        .expect("custom geometry should produce mesh");

    let part = mesh
        .parts
        .iter()
        .find(|part| part.role == CustomGeometryBoneRole::LeftArm)
        .expect("left arm geometry should be grouped as a left arm");

    assert_eq!(part.pivot, [5.0, 6.0, 0.0]);
    assert!(!part.vertices.is_empty());
    assert!(!part.indices.is_empty());
}

fn mesh_vertex_count(mesh: &CustomGeometryMesh) -> usize {
    mesh.parts.iter().map(|part| part.vertices.len()).sum()
}

fn mesh_index_count(mesh: &CustomGeometryMesh) -> usize {
    mesh.parts.iter().map(|part| part.indices.len()).sum()
}
