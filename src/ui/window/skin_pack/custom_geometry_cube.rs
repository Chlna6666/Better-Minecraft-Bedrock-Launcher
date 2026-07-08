use image::DynamicImage;
use serde_json::Value;
use std::collections::HashMap;

use super::super::color::{Face, shade_layer_edge_color};
use super::super::custom_geometry_json::array3;
use super::super::custom_geometry_math::{
    add3, bedrock_to_preview, normal_from_corners, rotate_point_around,
};
use super::super::custom_geometry_uv::{
    GeometryTextureRegion, cube_uv_regions, texture_grid_count,
};
use super::super::geometry::{
    CuboidSize, FaceGrid, QuadEdgeMask, face_pixel_corners, push_quad_with_edges,
};
use super::{
    BonePose, CustomGeometryPartBuilder, TextureSpace, ensure_capacity, sample_uv_color,
    transform_point_for_bone,
};

const CUSTOM_GEOMETRY_MAX_CUBE_TEXELS: usize = 240_000;

pub(super) fn push_cube(
    image: &DynamicImage,
    texture_space: TextureSpace,
    bone_poses: &HashMap<String, BonePose>,
    bone_name: Option<&str>,
    bone: &Value,
    cube: &Value,
    builder: &mut CustomGeometryPartBuilder,
) -> Result<(), String> {
    let Some(origin) = array3(cube.get("origin")) else {
        return Ok(());
    };
    let Some(size) = array3(cube.get("size")) else {
        return Ok(());
    };
    if zero_dimension_count(size) >= 2 {
        return Ok(());
    }
    let Some(uv) = cube.get("uv") else {
        return Ok(());
    };
    let Some(regions) = cube_uv_regions(uv, size) else {
        return Ok(());
    };

    let cuboid_size = CuboidSize {
        width: size[0].abs(),
        height: size[1].abs(),
        depth: size[2].abs(),
    };
    let center = bedrock_to_preview([
        origin[0] + size[0] * 0.5,
        origin[1] + size[1] * 0.5,
        origin[2] + size[2] * 0.5,
    ]);
    let pivot = cube
        .get("pivot")
        .or_else(|| bone.get("pivot"))
        .and_then(|value| array3(Some(value)))
        .map(bedrock_to_preview)
        .unwrap_or(center);
    let cube_rotation = array3(cube.get("rotation")).unwrap_or([0.0, 0.0, 0.0]);
    let inflate = cube
        .get("inflate")
        .and_then(Value::as_f64)
        .map(|value| value as f32)
        .unwrap_or(0.0);

    for face in cube_faces(size) {
        let Some(region) = regions.region(*face) else {
            continue;
        };
        push_cube_face(
            image,
            texture_space,
            cuboid_size,
            *face,
            region,
            inflate,
            |point| {
                let point = add3(center, point);
                let point = rotate_point_around(point, pivot, cube_rotation);
                transform_point_for_bone(point, bone_name, bone_poses)
            },
            builder,
        )?;
    }

    Ok(())
}

fn push_cube_face(
    image: &DynamicImage,
    texture_space: TextureSpace,
    size: CuboidSize,
    face: Face,
    region: GeometryTextureRegion,
    inflate: f32,
    transform: impl Fn([f32; 3]) -> [f32; 3],
    builder: &mut CustomGeometryPartBuilder,
) -> Result<(), String> {
    let grid = FaceGrid {
        width: texture_grid_count(region.width, texture_space.preview_scale),
        height: texture_grid_count(region.height, texture_space.preview_scale),
    };
    let texel_count = (grid.width as usize).saturating_mul(grid.height as usize);
    builder.cube_texel_count = builder.cube_texel_count.saturating_add(texel_count);
    if builder.cube_texel_count > CUSTOM_GEOMETRY_MAX_CUBE_TEXELS {
        return Err("自定义皮肤 geometry.json cube 贴图面过多".to_string());
    }

    for pixel_y in 0..grid.height {
        for pixel_x in 0..grid.width {
            let uv = [
                region.u + (pixel_x as f32 + 0.5) * region.width / grid.width as f32,
                region.v + (pixel_y as f32 + 0.5) * region.height / grid.height as f32,
            ];
            let color = sample_uv_color(image, texture_space, uv, false);
            if color[3] <= 0.04 {
                continue;
            }

            let corners =
                face_pixel_corners(size, face, grid, pixel_x, pixel_y, inflate).map(&transform);
            let normal = normal_from_corners(corners);
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

fn cube_faces(size: [f32; 3]) -> &'static [Face] {
    match zero_dimension_axis(size) {
        Some(0) => &[Face::Right, Face::Left],
        Some(1) => &[Face::Top, Face::Bottom],
        Some(2) => &[Face::Front, Face::Back],
        _ => &[
            Face::Top,
            Face::Bottom,
            Face::Right,
            Face::Front,
            Face::Left,
            Face::Back,
        ],
    }
}

fn zero_dimension_count(size: [f32; 3]) -> usize {
    size.iter()
        .filter(|value| value.abs() <= f32::EPSILON)
        .count()
}

fn zero_dimension_axis(size: [f32; 3]) -> Option<usize> {
    (zero_dimension_count(size) == 1)
        .then(|| size.iter().position(|value| value.abs() <= f32::EPSILON))
        .flatten()
}
