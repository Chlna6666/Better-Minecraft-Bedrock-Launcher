use gpui::GpuMesh3dVertex;
use image::DynamicImage;

use super::color::{Face, sample_image_color, shade_face_color, shade_layer_edge_color};
use super::geometry::{
    CuboidSize, FaceGrid, QuadEdgeMask, SkinTextureScale, face_grid, face_pixel_corners,
    face_region, push_quad_with_edges, source_pixel_offset,
};
use super::uv::{CuboidUv, TextureRegion};

const SKIN_LAYER_ALPHA_THRESHOLD: f32 = 0.04;
const SKIN_LAYER_INNER_INFLATE: f32 = 0.0;

pub(super) fn push_skin_layer(
    image: &DynamicImage,
    texture_scale: SkinTextureScale,
    size: CuboidSize,
    uv: CuboidUv,
    inflate: f32,
    vertices: &mut Vec<GpuMesh3dVertex>,
    indices: &mut Vec<u32>,
) {
    for face in skin_layer_faces() {
        push_skin_layer_face(
            image,
            texture_scale,
            size,
            *face,
            face_region(uv, *face),
            inflate,
            vertices,
            indices,
        );
    }
}

fn skin_layer_faces() -> &'static [Face; 6] {
    &[
        Face::Top,
        Face::Bottom,
        Face::Right,
        Face::Front,
        Face::Left,
        Face::Back,
    ]
}

fn push_skin_layer_face(
    image: &DynamicImage,
    texture_scale: SkinTextureScale,
    size: CuboidSize,
    face: Face,
    region: TextureRegion,
    inflate: f32,
    vertices: &mut Vec<GpuMesh3dVertex>,
    indices: &mut Vec<u32>,
) {
    let grid = face_grid(region, texture_scale.preview);

    for pixel_y in 0..grid.height {
        for pixel_x in 0..grid.width {
            let Some(color) = layer_pixel_color(image, texture_scale, region, pixel_x, pixel_y)
            else {
                continue;
            };

            let outer = face_pixel_corners(size, face, grid, pixel_x, pixel_y, inflate);
            let inner =
                face_pixel_corners(size, face, grid, pixel_x, pixel_y, SKIN_LAYER_INNER_INFLATE);
            push_quad_with_edges(
                vertices,
                indices,
                outer,
                shade_face_color(color, face),
                QuadEdgeMask::NONE,
            );

            for edge in LayerPixelEdge::ALL {
                if layer_edge_is_visible(image, texture_scale, region, grid, pixel_x, pixel_y, edge)
                {
                    push_layer_edge(vertices, indices, inner, outer, edge, color);
                }
            }
        }
    }
}

fn layer_pixel_color(
    image: &DynamicImage,
    texture_scale: SkinTextureScale,
    region: TextureRegion,
    pixel_x: u32,
    pixel_y: u32,
) -> Option<[f32; 4]> {
    let image_origin_x = region.x.saturating_mul(texture_scale.source);
    let image_origin_y = region.y.saturating_mul(texture_scale.source);
    let image_x = image_origin_x.saturating_add(source_pixel_offset(pixel_x, texture_scale));
    let image_y = image_origin_y.saturating_add(source_pixel_offset(pixel_y, texture_scale));
    let color = sample_image_color(image, image_x, image_y);
    (color[3] > SKIN_LAYER_ALPHA_THRESHOLD).then_some(color)
}

#[derive(Clone, Copy)]
enum LayerPixelEdge {
    Top,
    Right,
    Bottom,
    Left,
}

impl LayerPixelEdge {
    const ALL: [Self; 4] = [Self::Top, Self::Right, Self::Bottom, Self::Left];
}

fn layer_edge_is_visible(
    image: &DynamicImage,
    texture_scale: SkinTextureScale,
    region: TextureRegion,
    grid: FaceGrid,
    pixel_x: u32,
    pixel_y: u32,
    edge: LayerPixelEdge,
) -> bool {
    let Some((neighbor_x, neighbor_y)) = layer_edge_neighbor(grid, pixel_x, pixel_y, edge) else {
        return true;
    };

    layer_pixel_color(image, texture_scale, region, neighbor_x, neighbor_y).is_none()
}

fn layer_edge_neighbor(
    grid: FaceGrid,
    pixel_x: u32,
    pixel_y: u32,
    edge: LayerPixelEdge,
) -> Option<(u32, u32)> {
    match edge {
        LayerPixelEdge::Top => pixel_y
            .checked_sub(1)
            .map(|neighbor_y| (pixel_x, neighbor_y)),
        LayerPixelEdge::Right if pixel_x + 1 < grid.width => Some((pixel_x + 1, pixel_y)),
        LayerPixelEdge::Bottom if pixel_y + 1 < grid.height => Some((pixel_x, pixel_y + 1)),
        LayerPixelEdge::Left => pixel_x
            .checked_sub(1)
            .map(|neighbor_x| (neighbor_x, pixel_y)),
        LayerPixelEdge::Right | LayerPixelEdge::Bottom => None,
    }
}

fn push_layer_edge(
    vertices: &mut Vec<GpuMesh3dVertex>,
    indices: &mut Vec<u32>,
    inner: [[f32; 3]; 4],
    outer: [[f32; 3]; 4],
    edge: LayerPixelEdge,
    color: [f32; 4],
) {
    let (inner_a, inner_b) = edge_points(inner, edge);
    let (outer_a, outer_b) = edge_points(outer, edge);
    let desired_normal = vec3_sub(edge_midpoint(outer_a, outer_b), quad_center(outer));
    let mut corners = [inner_a, inner_b, outer_b, outer_a];
    let mut normal = quad_normal(corners);

    if vec3_dot(normal, desired_normal) < 0.0 {
        corners = [inner_b, inner_a, outer_a, outer_b];
        normal = quad_normal(corners);
    }

    push_quad_with_edges(
        vertices,
        indices,
        corners,
        shade_layer_edge_color(color, normal),
        QuadEdgeMask::NONE,
    );
}

fn edge_points(corners: [[f32; 3]; 4], edge: LayerPixelEdge) -> ([f32; 3], [f32; 3]) {
    match edge {
        LayerPixelEdge::Bottom => (corners[0], corners[1]),
        LayerPixelEdge::Right => (corners[1], corners[2]),
        LayerPixelEdge::Top => (corners[3], corners[2]),
        LayerPixelEdge::Left => (corners[0], corners[3]),
    }
}

fn edge_midpoint(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        (a[0] + b[0]) * 0.5,
        (a[1] + b[1]) * 0.5,
        (a[2] + b[2]) * 0.5,
    ]
}

fn quad_center(corners: [[f32; 3]; 4]) -> [f32; 3] {
    [
        (corners[0][0] + corners[1][0] + corners[2][0] + corners[3][0]) * 0.25,
        (corners[0][1] + corners[1][1] + corners[2][1] + corners[3][1]) * 0.25,
        (corners[0][2] + corners[1][2] + corners[2][2] + corners[3][2]) * 0.25,
    ]
}

fn vec3_sub(left: [f32; 3], right: [f32; 3]) -> [f32; 3] {
    [left[0] - right[0], left[1] - right[1], left[2] - right[2]]
}

fn vec3_dot(left: [f32; 3], right: [f32; 3]) -> f32 {
    left[0] * right[0] + left[1] * right[1] + left[2] * right[2]
}

fn quad_normal(corners: [[f32; 3]; 4]) -> [f32; 3] {
    let a = vec3_sub(corners[1], corners[0]);
    let b = vec3_sub(corners[2], corners[0]);
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
