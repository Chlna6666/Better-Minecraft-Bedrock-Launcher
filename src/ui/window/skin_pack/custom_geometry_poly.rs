use image::DynamicImage;
use serde_json::Value;
use std::collections::HashMap;

#[path = "custom_geometry_poly_triangle.rs"]
mod triangle;

use self::triangle::push_poly_mesh_triangle;
use super::super::color::shade_layer_edge_color;
use super::super::custom_geometry_json::{index_triplet, point2_array, point3_array};
use super::super::custom_geometry_math::{bedrock_to_preview, normalize};
use super::super::custom_geometry_uv::texture_grid_count;
use super::super::geometry::{FaceGrid, QuadEdgeMask, push_quad_with_edges};
use super::{
    BonePose, CustomGeometryPartBuilder, TextureSpace, ensure_capacity, sample_uv_color,
    transform_normal_for_bone, transform_point_for_bone,
};

const CUSTOM_GEOMETRY_MAX_POLYGONS: usize = 100_000;
const POLY_MESH_UV_EPSILON: f32 = 0.0001;
const POLY_MESH_POSITION_EPSILON: f32 = 0.0001;
const POLY_MESH_NORMAL_DOT_MIN: f32 = 0.995;

#[derive(Clone, Copy)]
struct PolyMeshVertex {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
}

pub(super) fn push_poly_mesh(
    image: &DynamicImage,
    texture_space: TextureSpace,
    bone_poses: &HashMap<String, BonePose>,
    bone_name: Option<&str>,
    poly_mesh: &Value,
    builder: &mut CustomGeometryPartBuilder,
) -> Result<(), String> {
    let Some(polys) = poly_mesh.get("polys").and_then(Value::as_array) else {
        return Ok(());
    };
    let positions = point3_array(poly_mesh.get("positions"));
    let normals = point3_array(poly_mesh.get("normals"));
    let uvs = point2_array(poly_mesh.get("uvs"));
    let normalized_uvs = poly_mesh
        .get("normalized_uvs")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let mut index = 0;
    while index < polys.len() {
        builder.polygon_count = builder.polygon_count.saturating_add(1);
        if builder.polygon_count > CUSTOM_GEOMETRY_MAX_POLYGONS {
            return Err("自定义皮肤 geometry.json 面片过多".to_string());
        }

        let Some(polygon_vertices) = poly_mesh_polygon_vertices(
            &polys[index],
            &positions,
            &normals,
            &uvs,
            bone_name,
            bone_poses,
        ) else {
            index += 1;
            continue;
        };
        if polygon_vertices.len() < 3 {
            index += 1;
            continue;
        }

        if let Some(quad) = poly_mesh_quad_from_polygon(&polygon_vertices) {
            push_poly_mesh_quad(image, texture_space, normalized_uvs, quad, builder)?;
            index += 1;
            continue;
        }

        if let Some(next_poly) = polys.get(index + 1)
            && let Some(next_vertices) = poly_mesh_polygon_vertices(
                next_poly, &positions, &normals, &uvs, bone_name, bone_poses,
            )
            && let Some(quad) = poly_mesh_quad_from_triangle_pair(&polygon_vertices, &next_vertices)
        {
            builder.polygon_count = builder.polygon_count.saturating_add(1);
            if builder.polygon_count > CUSTOM_GEOMETRY_MAX_POLYGONS {
                return Err("自定义皮肤 geometry.json 面片过多".to_string());
            }
            push_poly_mesh_quad(image, texture_space, normalized_uvs, quad, builder)?;
            index += 2;
            continue;
        }

        push_poly_mesh_polygon(
            image,
            texture_space,
            normalized_uvs,
            &polygon_vertices,
            builder,
        )?;
        index += 1;
    }

    Ok(())
}

fn poly_mesh_polygon_vertices(
    poly: &Value,
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
    uvs: &[[f32; 2]],
    bone_name: Option<&str>,
    bone_poses: &HashMap<String, BonePose>,
) -> Option<Vec<PolyMeshVertex>> {
    let vertex_refs = poly.as_array()?;
    let mut polygon_vertices = Vec::with_capacity(vertex_refs.len().min(8));

    for vertex_ref in vertex_refs.iter().take(8) {
        let Some([position_index, normal_index, uv_index]) = index_triplet(vertex_ref) else {
            continue;
        };
        let Some(position) = positions.get(position_index).copied() else {
            continue;
        };
        let Some(uv) = uvs.get(uv_index).copied() else {
            continue;
        };

        let position =
            transform_point_for_bone(bedrock_to_preview(position), bone_name, bone_poses);
        let normal = normals
            .get(normal_index)
            .copied()
            .map(|normal| transform_normal_for_bone(normal, bone_name, bone_poses))
            .unwrap_or([0.0, 1.0, 0.0]);
        polygon_vertices.push(PolyMeshVertex {
            position,
            normal,
            uv,
        });
    }

    let polygon_vertices = deduplicate_poly_mesh_polygon_vertices(polygon_vertices);
    (!polygon_vertices.is_empty()).then_some(polygon_vertices)
}

fn deduplicate_poly_mesh_polygon_vertices(
    mut polygon_vertices: Vec<PolyMeshVertex>,
) -> Vec<PolyMeshVertex> {
    polygon_vertices.dedup_by(|current, previous| poly_mesh_vertices_match(current, previous));
    if polygon_vertices
        .first()
        .zip(polygon_vertices.last())
        .is_some_and(|(first, last)| poly_mesh_vertices_match(first, last))
    {
        polygon_vertices.pop();
    }
    polygon_vertices
}

fn push_poly_mesh_polygon(
    image: &DynamicImage,
    texture_space: TextureSpace,
    normalized_uvs: bool,
    polygon_vertices: &[PolyMeshVertex],
    builder: &mut CustomGeometryPartBuilder,
) -> Result<(), String> {
    for index in 1..polygon_vertices.len().saturating_sub(1) {
        push_poly_mesh_triangle(
            image,
            texture_space,
            normalized_uvs,
            [
                polygon_vertices[0],
                polygon_vertices[index],
                polygon_vertices[index + 1],
            ],
            builder,
        )?;
    }

    Ok(())
}

fn push_poly_mesh_quad(
    image: &DynamicImage,
    texture_space: TextureSpace,
    normalized_uvs: bool,
    quad: [PolyMeshVertex; 4],
    builder: &mut CustomGeometryPartBuilder,
) -> Result<(), String> {
    let Some((uv_min, uv_max)) = poly_mesh_uv_bounds(&quad) else {
        return Ok(());
    };
    let grid = poly_mesh_quad_grid(texture_space, normalized_uvs, uv_min, uv_max);
    let normal = poly_mesh_average_normal(&quad);

    for pixel_y in 0..grid.height {
        for pixel_x in 0..grid.width {
            let u0 = pixel_x as f32 / grid.width as f32;
            let u1 = (pixel_x + 1) as f32 / grid.width as f32;
            let v0 = pixel_y as f32 / grid.height as f32;
            let v1 = (pixel_y + 1) as f32 / grid.height as f32;
            let uv = [
                interpolate_scalar(uv_min[0], uv_max[0], (u0 + u1) * 0.5),
                interpolate_scalar(uv_min[1], uv_max[1], (v0 + v1) * 0.5),
            ];
            let color = sample_uv_color(image, texture_space, uv, normalized_uvs);
            if color[3] <= 0.04 {
                continue;
            }

            let corners = [
                interpolate_poly_mesh_quad_position(quad, u0, v0),
                interpolate_poly_mesh_quad_position(quad, u1, v0),
                interpolate_poly_mesh_quad_position(quad, u1, v1),
                interpolate_poly_mesh_quad_position(quad, u0, v1),
            ];
            ensure_capacity(&builder.vertices, &builder.indices, 6, 6)?;
            push_quad_with_edges(
                &mut builder.vertices,
                &mut builder.indices,
                corners,
                shade_layer_edge_color(color, normal),
                QuadEdgeMask::NONE,
            );
        }
    }

    Ok(())
}

fn poly_mesh_quad_grid(
    texture_space: TextureSpace,
    normalized_uvs: bool,
    uv_min: [f32; 2],
    uv_max: [f32; 2],
) -> FaceGrid {
    let uv_width = (uv_max[0] - uv_min[0]).abs();
    let uv_height = (uv_max[1] - uv_min[1]).abs();
    let texture_width = if normalized_uvs {
        uv_width * texture_space.width
    } else {
        uv_width
    };
    let texture_height = if normalized_uvs {
        uv_height * texture_space.height
    } else {
        uv_height
    };

    FaceGrid {
        width: texture_grid_count(texture_width, texture_space.preview_scale),
        height: texture_grid_count(texture_height, texture_space.preview_scale),
    }
}

fn interpolate_poly_mesh_quad_position(quad: [PolyMeshVertex; 4], u: f32, v: f32) -> [f32; 3] {
    let top = interpolate3(quad[0].position, quad[1].position, u);
    let bottom = interpolate3(quad[3].position, quad[2].position, u);
    interpolate3(top, bottom, v)
}

fn interpolate3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        interpolate_scalar(a[0], b[0], t),
        interpolate_scalar(a[1], b[1], t),
        interpolate_scalar(a[2], b[2], t),
    ]
}

fn interpolate_scalar(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn poly_mesh_average_normal(vertices: &[PolyMeshVertex; 4]) -> [f32; 3] {
    normalize([
        vertices.iter().map(|vertex| vertex.normal[0]).sum::<f32>() / 4.0,
        vertices.iter().map(|vertex| vertex.normal[1]).sum::<f32>() / 4.0,
        vertices.iter().map(|vertex| vertex.normal[2]).sum::<f32>() / 4.0,
    ])
}

fn poly_mesh_quad_from_polygon(vertices: &[PolyMeshVertex]) -> Option<[PolyMeshVertex; 4]> {
    let unique_vertices = unique_poly_mesh_vertices(vertices);
    poly_mesh_quad_from_unique_vertices(&unique_vertices)
}

fn poly_mesh_quad_from_triangle_pair(
    first: &[PolyMeshVertex],
    second: &[PolyMeshVertex],
) -> Option<[PolyMeshVertex; 4]> {
    if first.len() != 3 || second.len() != 3 {
        return None;
    }

    let mut vertices = Vec::with_capacity(6);
    vertices.extend_from_slice(first);
    vertices.extend_from_slice(second);
    let unique_vertices = unique_poly_mesh_vertices(&vertices);
    poly_mesh_quad_from_unique_vertices(&unique_vertices)
}

fn poly_mesh_quad_from_unique_vertices(vertices: &[PolyMeshVertex]) -> Option<[PolyMeshVertex; 4]> {
    if vertices.len() != 4 || !poly_mesh_normals_are_compatible(vertices) {
        return None;
    }

    let (uv_min, uv_max) = poly_mesh_uv_bounds(vertices)?;
    Some([
        poly_mesh_vertex_for_uv(vertices, [uv_min[0], uv_min[1]])?,
        poly_mesh_vertex_for_uv(vertices, [uv_max[0], uv_min[1]])?,
        poly_mesh_vertex_for_uv(vertices, [uv_max[0], uv_max[1]])?,
        poly_mesh_vertex_for_uv(vertices, [uv_min[0], uv_max[1]])?,
    ])
}

fn unique_poly_mesh_vertices(vertices: &[PolyMeshVertex]) -> Vec<PolyMeshVertex> {
    let mut unique_vertices = Vec::with_capacity(vertices.len());
    for vertex in vertices {
        if !unique_vertices
            .iter()
            .any(|candidate| poly_mesh_vertices_match(candidate, vertex))
        {
            unique_vertices.push(*vertex);
        }
    }
    unique_vertices
}

fn poly_mesh_vertex_for_uv(vertices: &[PolyMeshVertex], uv: [f32; 2]) -> Option<PolyMeshVertex> {
    vertices
        .iter()
        .find(|vertex| vec2_close(vertex.uv, uv, POLY_MESH_UV_EPSILON))
        .copied()
}

fn poly_mesh_uv_bounds(vertices: &[PolyMeshVertex]) -> Option<([f32; 2], [f32; 2])> {
    let first = vertices.first()?;
    if !first.uv[0].is_finite() || !first.uv[1].is_finite() {
        return None;
    }

    let mut uv_min = first.uv;
    let mut uv_max = first.uv;
    for vertex in vertices.iter().skip(1) {
        if !vertex.uv[0].is_finite() || !vertex.uv[1].is_finite() {
            return None;
        }
        uv_min[0] = uv_min[0].min(vertex.uv[0]);
        uv_min[1] = uv_min[1].min(vertex.uv[1]);
        uv_max[0] = uv_max[0].max(vertex.uv[0]);
        uv_max[1] = uv_max[1].max(vertex.uv[1]);
    }

    if uv_max[0] - uv_min[0] <= POLY_MESH_UV_EPSILON
        || uv_max[1] - uv_min[1] <= POLY_MESH_UV_EPSILON
    {
        return None;
    }
    Some((uv_min, uv_max))
}

fn poly_mesh_normals_are_compatible(vertices: &[PolyMeshVertex]) -> bool {
    let Some(first) = vertices.first() else {
        return false;
    };
    let normal = normalize(first.normal);
    vertices
        .iter()
        .all(|vertex| dot3(normal, normalize(vertex.normal)) >= POLY_MESH_NORMAL_DOT_MIN)
}

fn poly_mesh_vertices_match(first: &PolyMeshVertex, second: &PolyMeshVertex) -> bool {
    vec3_close(first.position, second.position, POLY_MESH_POSITION_EPSILON)
        && vec2_close(first.uv, second.uv, POLY_MESH_UV_EPSILON)
}

fn vec2_close(first: [f32; 2], second: [f32; 2], epsilon: f32) -> bool {
    (first[0] - second[0]).abs() <= epsilon && (first[1] - second[1]).abs() <= epsilon
}

fn vec3_close(first: [f32; 3], second: [f32; 3], epsilon: f32) -> bool {
    (first[0] - second[0]).abs() <= epsilon
        && (first[1] - second[1]).abs() <= epsilon
        && (first[2] - second[2]).abs() <= epsilon
}

fn dot3(first: [f32; 3], second: [f32; 3]) -> f32 {
    first[0] * second[0] + first[1] * second[1] + first[2] * second[2]
}
