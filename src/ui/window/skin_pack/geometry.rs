use gpui::GpuMesh3dVertex;
use image::DynamicImage;

use super::color::{Face, sample_image_color, shade_face_color};
use super::uv::{CuboidUv, TextureRegion};

pub(super) const SKIN_MIN_SIZE: u32 = 64;
pub(super) const TRIANGLE_EDGE_0: u8 = 1;
pub(super) const TRIANGLE_EDGE_1: u8 = 1 << 1;
pub(super) const TRIANGLE_EDGE_2: u8 = 1 << 2;
const SKIN_PREVIEW_MAX_TEXTURE_SCALE: u32 = 2;
const EDGE_MASK_ALPHA_STRIDE: f32 = 2.0;

#[derive(Clone, Copy)]
pub(super) struct CuboidSize {
    pub(super) width: f32,
    pub(super) height: f32,
    pub(super) depth: f32,
}

#[derive(Clone, Copy)]
pub(super) struct SkinTextureScale {
    pub(super) source: u32,
    pub(super) preview: u32,
}

#[derive(Clone, Copy)]
pub(super) struct FaceGrid {
    pub(super) width: u32,
    pub(super) height: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct QuadEdgeMask(u8);

impl QuadEdgeMask {
    pub(super) const NONE: Self = Self(0);
    pub(super) const BOTTOM: Self = Self(1);
    pub(super) const RIGHT: Self = Self(1 << 1);
    pub(super) const TOP: Self = Self(1 << 2);
    pub(super) const LEFT: Self = Self(1 << 3);

    #[cfg(test)]
    pub(super) const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    const fn contains(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }
}

impl SkinTextureScale {
    pub(super) fn from_width(width: u32) -> Self {
        let source = (width / SKIN_MIN_SIZE).max(1);
        Self {
            source,
            preview: source.min(SKIN_PREVIEW_MAX_TEXTURE_SCALE).max(1),
        }
    }
}

pub(super) fn skin_preview_faces() -> &'static [Face; 6] {
    &[
        Face::Top,
        Face::Bottom,
        Face::Right,
        Face::Front,
        Face::Left,
        Face::Back,
    ]
}

pub(super) fn face_region(uv: CuboidUv, face: Face) -> TextureRegion {
    match face {
        Face::Top => uv.top,
        Face::Bottom => uv.bottom,
        Face::Right => uv.right,
        Face::Front => uv.front,
        Face::Left => uv.left,
        Face::Back => uv.back,
    }
}

pub(super) fn cuboid_uv_pixel_count(uv: CuboidUv, preview_scale: u32) -> usize {
    skin_preview_faces()
        .iter()
        .map(|face| {
            let region = face_region(uv, *face);
            let grid = face_grid(region, preview_scale);
            (grid.width as usize).saturating_mul(grid.height as usize)
        })
        .sum()
}

pub(super) fn push_face(
    image: &DynamicImage,
    texture_scale: SkinTextureScale,
    size: CuboidSize,
    face: Face,
    region: TextureRegion,
    inflate: f32,
    transparent: bool,
    vertices: &mut Vec<GpuMesh3dVertex>,
    indices: &mut Vec<u32>,
) {
    let grid = face_grid(region, texture_scale.preview);
    let image_origin_x = region.x.saturating_mul(texture_scale.source);
    let image_origin_y = region.y.saturating_mul(texture_scale.source);
    for pixel_y in 0..grid.height {
        for pixel_x in 0..grid.width {
            let image_x =
                image_origin_x.saturating_add(source_pixel_offset(pixel_x, texture_scale));
            let image_y =
                image_origin_y.saturating_add(source_pixel_offset(pixel_y, texture_scale));
            let mut color = sample_image_color(image, image_x, image_y);
            if color[3] <= 0.04 && transparent {
                continue;
            }
            if !transparent {
                color[3] = color[3].max(1.0);
            }
            let corners = face_pixel_corners(size, face, grid, pixel_x, pixel_y, inflate);
            push_quad_with_edges(
                vertices,
                indices,
                corners,
                shade_face_color(color, face),
                QuadEdgeMask::NONE,
            );
        }
    }
}

pub(super) fn face_grid(region: TextureRegion, preview_scale: u32) -> FaceGrid {
    FaceGrid {
        width: region.width.saturating_mul(preview_scale).max(1),
        height: region.height.saturating_mul(preview_scale).max(1),
    }
}

pub(super) fn source_pixel_offset(preview_pixel: u32, texture_scale: SkinTextureScale) -> u32 {
    let numerator = preview_pixel
        .saturating_mul(texture_scale.source)
        .saturating_mul(2)
        .saturating_add(texture_scale.source);
    let denominator = texture_scale.preview.saturating_mul(2).max(1);
    numerator / denominator
}

#[cfg(test)]
pub(super) fn quad_center(corners: [[f32; 3]; 4]) -> [f32; 3] {
    [
        (corners[0][0] + corners[1][0] + corners[2][0] + corners[3][0]) * 0.25,
        (corners[0][1] + corners[1][1] + corners[2][1] + corners[3][1]) * 0.25,
        (corners[0][2] + corners[1][2] + corners[2][2] + corners[3][2]) * 0.25,
    ]
}

pub(super) fn face_pixel_corners(
    size: CuboidSize,
    face: Face,
    grid: FaceGrid,
    pixel_x: u32,
    pixel_y: u32,
    inflate: f32,
) -> [[f32; 3]; 4] {
    let half_width = size.width * 0.5 + inflate;
    let half_height = size.height * 0.5 + inflate;
    let half_depth = size.depth * 0.5 + inflate;
    let u0 = pixel_x as f32 / grid.width as f32;
    let u1 = (pixel_x + 1) as f32 / grid.width as f32;
    let v0 = pixel_y as f32 / grid.height as f32;
    let v1 = (pixel_y + 1) as f32 / grid.height as f32;

    match face {
        Face::Front => front_face(half_width, half_height, half_depth, u0, u1, v0, v1),
        Face::Back => back_face(half_width, half_height, half_depth, u0, u1, v0, v1),
        Face::Right => right_face(half_width, half_height, half_depth, u0, u1, v0, v1),
        Face::Left => left_face(half_width, half_height, half_depth, u0, u1, v0, v1),
        Face::Top => top_face(half_width, half_depth, half_height, u0, u1, v0, v1),
        Face::Bottom => cap_face(
            half_width,
            half_depth,
            -half_height,
            u0,
            u1,
            1.0 - v0,
            1.0 - v1,
        ),
    }
}

fn front_face(w: f32, h: f32, z: f32, u0: f32, u1: f32, v0: f32, v1: f32) -> [[f32; 3]; 4] {
    let x0 = -w + u0 * w * 2.0;
    let x1 = -w + u1 * w * 2.0;
    let y1 = h - v0 * h * 2.0;
    let y0 = h - v1 * h * 2.0;
    [[x0, y0, z], [x1, y0, z], [x1, y1, z], [x0, y1, z]]
}

fn back_face(w: f32, h: f32, d: f32, u0: f32, u1: f32, v0: f32, v1: f32) -> [[f32; 3]; 4] {
    let x0 = w - u0 * w * 2.0;
    let x1 = w - u1 * w * 2.0;
    let y1 = h - v0 * h * 2.0;
    let y0 = h - v1 * h * 2.0;
    [[x0, y0, -d], [x1, y0, -d], [x1, y1, -d], [x0, y1, -d]]
}

fn right_face(w: f32, h: f32, d: f32, u0: f32, u1: f32, v0: f32, v1: f32) -> [[f32; 3]; 4] {
    let z0 = -d + u0 * d * 2.0;
    let z1 = -d + u1 * d * 2.0;
    let y1 = h - v0 * h * 2.0;
    let y0 = h - v1 * h * 2.0;
    [[-w, y0, z0], [-w, y0, z1], [-w, y1, z1], [-w, y1, z0]]
}

fn left_face(w: f32, h: f32, d: f32, u0: f32, u1: f32, v0: f32, v1: f32) -> [[f32; 3]; 4] {
    let z0 = d - u0 * d * 2.0;
    let z1 = d - u1 * d * 2.0;
    let y1 = h - v0 * h * 2.0;
    let y0 = h - v1 * h * 2.0;
    [[w, y0, z0], [w, y0, z1], [w, y1, z1], [w, y1, z0]]
}

fn top_face(w: f32, d: f32, y: f32, u0: f32, u1: f32, v0: f32, v1: f32) -> [[f32; 3]; 4] {
    let x0 = -w + u0 * w * 2.0;
    let x1 = -w + u1 * w * 2.0;
    let z0 = -d + v0 * d * 2.0;
    let z1 = -d + v1 * d * 2.0;
    [[x0, y, z1], [x1, y, z1], [x1, y, z0], [x0, y, z0]]
}

fn cap_face(w: f32, d: f32, y: f32, u0: f32, u1: f32, v0: f32, v1: f32) -> [[f32; 3]; 4] {
    let x0 = -w + u0 * w * 2.0;
    let x1 = -w + u1 * w * 2.0;
    let z0 = d - v0 * d * 2.0;
    let z1 = d - v1 * d * 2.0;
    [[x1, y, z1], [x0, y, z1], [x0, y, z0], [x1, y, z0]]
}

pub(super) fn push_quad_with_edges(
    vertices: &mut Vec<GpuMesh3dVertex>,
    indices: &mut Vec<u32>,
    corners: [[f32; 3]; 4],
    color: [f32; 4],
    edge_mask: QuadEdgeMask,
) {
    let Ok(base) = u32::try_from(vertices.len()) else {
        return;
    };
    vertices.extend([
        GpuMesh3dVertex {
            position: corners[0],
            color: color_with_triangle_edge_mask(color, first_triangle_edge_mask(edge_mask)),
        },
        GpuMesh3dVertex {
            position: corners[1],
            color: color_with_triangle_edge_mask(color, first_triangle_edge_mask(edge_mask)),
        },
        GpuMesh3dVertex {
            position: corners[2],
            color: color_with_triangle_edge_mask(color, first_triangle_edge_mask(edge_mask)),
        },
        GpuMesh3dVertex {
            position: corners[0],
            color: color_with_triangle_edge_mask(color, second_triangle_edge_mask(edge_mask)),
        },
        GpuMesh3dVertex {
            position: corners[2],
            color: color_with_triangle_edge_mask(color, second_triangle_edge_mask(edge_mask)),
        },
        GpuMesh3dVertex {
            position: corners[3],
            color: color_with_triangle_edge_mask(color, second_triangle_edge_mask(edge_mask)),
        },
    ]);
    indices.extend([
        base,
        base.saturating_add(1),
        base.saturating_add(2),
        base.saturating_add(3),
        base.saturating_add(4),
        base.saturating_add(5),
    ]);
}

pub(super) fn color_with_triangle_edge_mask(mut color: [f32; 4], edge_mask: u8) -> [f32; 4] {
    color[3] = color[3].clamp(0.0, 1.0) + f32::from(edge_mask) * EDGE_MASK_ALPHA_STRIDE;
    color
}

fn first_triangle_edge_mask(edge_mask: QuadEdgeMask) -> u8 {
    let mut triangle_edge_mask = 0;
    if edge_mask.contains(QuadEdgeMask::RIGHT) {
        triangle_edge_mask |= TRIANGLE_EDGE_0;
    }
    if edge_mask.contains(QuadEdgeMask::BOTTOM) {
        triangle_edge_mask |= TRIANGLE_EDGE_2;
    }
    triangle_edge_mask
}

fn second_triangle_edge_mask(edge_mask: QuadEdgeMask) -> u8 {
    let mut triangle_edge_mask = 0;
    if edge_mask.contains(QuadEdgeMask::TOP) {
        triangle_edge_mask |= TRIANGLE_EDGE_0;
    }
    if edge_mask.contains(QuadEdgeMask::LEFT) {
        triangle_edge_mask |= TRIANGLE_EDGE_1;
    }
    triangle_edge_mask
}
