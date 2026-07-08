use gpui::GpuMesh3dVertex;
use image::DynamicImage;

use super::super::super::color::shade_layer_edge_color;
use super::super::super::custom_geometry_math::{
    average2, average3, barycentric2, barycentric3, normalize, texture_edge_length,
};
use super::super::{CustomGeometryPartBuilder, TextureSpace, ensure_capacity, sample_uv_color};
use super::PolyMeshVertex;

const CUSTOM_POLY_MAX_SUBDIVISIONS: u32 = 12;

pub(super) fn push_poly_mesh_triangle(
    image: &DynamicImage,
    texture_space: TextureSpace,
    normalized_uvs: bool,
    triangle: [PolyMeshVertex; 3],
    builder: &mut CustomGeometryPartBuilder,
) -> Result<(), String> {
    let subdivisions = poly_mesh_subdivisions(texture_space, normalized_uvs, triangle);
    for row in 0..subdivisions {
        for column in 0..(subdivisions - row) {
            let a = interpolate_poly_mesh_vertex(triangle, column, row, subdivisions);
            let b = interpolate_poly_mesh_vertex(triangle, column + 1, row, subdivisions);
            let c = interpolate_poly_mesh_vertex(triangle, column, row + 1, subdivisions);
            push_sampled_poly_triangle(image, texture_space, normalized_uvs, [a, b, c], builder)?;

            if column + row + 1 < subdivisions {
                let d = interpolate_poly_mesh_vertex(triangle, column + 1, row + 1, subdivisions);
                push_sampled_poly_triangle(
                    image,
                    texture_space,
                    normalized_uvs,
                    [b, d, c],
                    builder,
                )?;
            }
        }
    }

    Ok(())
}

fn push_sampled_poly_triangle(
    image: &DynamicImage,
    texture_space: TextureSpace,
    normalized_uvs: bool,
    triangle: [PolyMeshVertex; 3],
    builder: &mut CustomGeometryPartBuilder,
) -> Result<(), String> {
    let uv = average2([triangle[0].uv, triangle[1].uv, triangle[2].uv]);
    let normal = normalize(average3([
        triangle[0].normal,
        triangle[1].normal,
        triangle[2].normal,
    ]));
    let color = shade_layer_edge_color(
        sample_uv_color(image, texture_space, uv, normalized_uvs),
        normal,
    );
    if color[3] <= 0.04 {
        return Ok(());
    }

    push_triangle_vertices(
        &mut builder.vertices,
        &mut builder.indices,
        triangle.map(|vertex| GpuMesh3dVertex {
            position: vertex.position,
            color,
        }),
    )
}

fn push_triangle_vertices(
    vertices: &mut Vec<GpuMesh3dVertex>,
    indices: &mut Vec<u32>,
    triangle: [GpuMesh3dVertex; 3],
) -> Result<(), String> {
    ensure_capacity(vertices, indices, 3, 3)?;
    let base = u32::try_from(vertices.len()).map_err(|_| "3D 网格顶点过多".to_string())?;
    vertices.extend(triangle);
    indices.extend([base, base + 1, base + 2]);
    Ok(())
}

fn poly_mesh_subdivisions(
    texture_space: TextureSpace,
    normalized_uvs: bool,
    triangle: [PolyMeshVertex; 3],
) -> u32 {
    let texture_points =
        triangle.map(|vertex| texture_point(texture_space, vertex.uv, normalized_uvs));
    let max_edge = texture_edge_length(texture_points[0], texture_points[1])
        .max(texture_edge_length(texture_points[1], texture_points[2]))
        .max(texture_edge_length(texture_points[2], texture_points[0]));
    max_edge
        .ceil()
        .clamp(1.0, CUSTOM_POLY_MAX_SUBDIVISIONS as f32) as u32
}

fn interpolate_poly_mesh_vertex(
    triangle: [PolyMeshVertex; 3],
    column: u32,
    row: u32,
    subdivisions: u32,
) -> PolyMeshVertex {
    let u = column as f32 / subdivisions as f32;
    let v = row as f32 / subdivisions as f32;
    let w = 1.0 - u - v;
    PolyMeshVertex {
        position: barycentric3(
            triangle[0].position,
            triangle[1].position,
            triangle[2].position,
            [w, u, v],
        ),
        normal: normalize(barycentric3(
            triangle[0].normal,
            triangle[1].normal,
            triangle[2].normal,
            [w, u, v],
        )),
        uv: barycentric2(triangle[0].uv, triangle[1].uv, triangle[2].uv, [w, u, v]),
    }
}

fn texture_point(texture_space: TextureSpace, uv: [f32; 2], normalized: bool) -> [f32; 2] {
    if normalized {
        [
            uv[0] * texture_space.width,
            (1.0 - uv[1]) * texture_space.height,
        ]
    } else {
        [uv[0], uv[1]]
    }
}
