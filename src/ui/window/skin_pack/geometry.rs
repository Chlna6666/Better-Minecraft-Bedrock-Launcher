use image::DynamicImage;

use super::color::{Face, sample_image_color, shade_face_color};
use super::uv::{CuboidUv, TextureRegion};

pub(super) const SKIN_MIN_SIZE: u32 = 64;
pub(super) const TRIANGLE_EDGE_0: u8 = 1;
pub(super) const TRIANGLE_EDGE_1: u8 = 1 << 1;
pub(super) const TRIANGLE_EDGE_2: u8 = 1 << 2;
const SKIN_PREVIEW_MAX_TEXTURE_SCALE: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SkinVertex {
    pub(super) position: [f32; 3],
    pub(super) color: [f32; 4],
    pub(super) edge_mask: u8,
}

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

/// Reads a rectangle of the face grid into flat `[f32; 4]` colors.
///
/// One run is one quad, so a face with uniform texels collapses from one quad per texel to one
/// quad per horizontal run. Adjacent runs share their edge exactly, which removes the overlap that
/// per-texel quads created along the texture's vertical axis.
pub(super) fn face_color_rect(
    image: &DynamicImage,
    texture_scale: SkinTextureScale,
    region: TextureRegion,
    grid: FaceGrid,
) -> Vec<[f32; 4]> {
    let mut colors = Vec::with_capacity((grid.width as usize).saturating_mul(grid.height as usize));
    for pixel_y in 0..grid.height {
        for pixel_x in 0..grid.width {
            colors.push(face_source_color(
                image,
                texture_scale,
                region,
                pixel_x,
                pixel_y,
            ));
        }
    }
    colors
}

pub(super) fn face_source_color(
    image: &DynamicImage,
    texture_scale: SkinTextureScale,
    region: TextureRegion,
    pixel_x: u32,
    pixel_y: u32,
) -> [f32; 4] {
    let image_origin_x = region.x.saturating_mul(texture_scale.source);
    let image_origin_y = region.y.saturating_mul(texture_scale.source);
    let image_x = image_origin_x.saturating_add(source_pixel_offset(pixel_x, texture_scale));
    let image_y = image_origin_y.saturating_add(source_pixel_offset(pixel_y, texture_scale));
    sample_image_color(image, image_x, image_y)
}

pub(super) fn push_face(
    image: &DynamicImage,
    texture_scale: SkinTextureScale,
    size: CuboidSize,
    face: Face,
    region: TextureRegion,
    inflate: f32,
    transparent: bool,
    vertices: &mut Vec<SkinVertex>,
    indices: &mut Vec<u32>,
) {
    let grid = face_grid(region, texture_scale.preview);
    let colors = face_color_rect(image, texture_scale, region, grid);
    for_each_color_run(grid, &colors, |run| {
        let Some(run_color) = colors.get(run.color_index).copied() else {
            return;
        };
        let mut color = run_color;
        if color[3] <= 0.04 && transparent {
            return;
        }
        if !transparent {
            color[3] = color[3].max(1.0);
        }
        let corners = face_rect_corners(size, face, grid, run, inflate);
        push_quad_with_edges(
            vertices,
            indices,
            corners,
            shade_face_color(color, face),
            QuadEdgeMask::NONE,
        );
    });
}

/// One horizontal run of same-color face texels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ColorRun {
    /// Left edge in preview-grid columns.
    pub(super) x: u32,
    /// Top edge in preview-grid rows.
    pub(super) y: u32,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) color_index: usize,
}

impl ColorRun {
    pub(super) fn right(self) -> u32 {
        self.x.saturating_add(self.width)
    }

    pub(super) fn bottom(self) -> u32 {
        self.y.saturating_add(self.height)
    }
}

/// Groups a row-major color grid into horizontal same-color runs.
///
/// `colors` is indexed `y * grid.width + x` and must contain one entry per cell.
pub(super) fn color_runs(grid: FaceGrid, colors: &[[f32; 4]]) -> Vec<ColorRun> {
    let mut runs = Vec::new();
    for_each_color_run(grid, colors, |run| runs.push(run));
    runs
}

fn for_each_color_run(grid: FaceGrid, colors: &[[f32; 4]], mut visit: impl FnMut(ColorRun)) {
    if grid.width == 0 || grid.height == 0 {
        return;
    }
    for pixel_y in 0..grid.height {
        let row_start = (pixel_y as usize).saturating_mul(grid.width as usize);
        let mut pixel_x = 0;
        while pixel_x < grid.width {
            let index = row_start.saturating_add(pixel_x as usize);
            let Some(color) = colors.get(index) else {
                break;
            };
            let mut run_width = 1;
            while pixel_x + run_width < grid.width {
                let next = row_start.saturating_add((pixel_x + run_width) as usize);
                if colors.get(next) != Some(color) {
                    break;
                }
                run_width += 1;
            }
            visit(ColorRun {
                x: pixel_x,
                y: pixel_y,
                width: run_width,
                height: 1,
                color_index: index,
            });
            pixel_x += run_width;
        }
    }
}

/// Corners of one preview-grid rectangle on a face, in authored cuboid units.
pub(super) fn face_rect_corners(
    size: CuboidSize,
    face: Face,
    grid: FaceGrid,
    run: ColorRun,
    inflate: f32,
) -> [[f32; 3]; 4] {
    let u0 = run.x as f32 / grid.width as f32;
    let u1 = run.right() as f32 / grid.width as f32;
    let v0 = run.y as f32 / grid.height as f32;
    let v1 = run.bottom() as f32 / grid.height as f32;

    face_uv_corners(size, face, u0, u1, v0, v1, inflate)
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

/// Average authored color of one cuboid face region.
///
/// The preview bakes one color per cuboid face, so a face needs the mean of the texels it covers
/// rather than one sampled texel. Transparent texels are excluded when the region has any opaque
/// coverage, so an overlay region with holes still reports its visible color.
pub(super) fn face_average_color(
    image: &DynamicImage,
    texture_scale: SkinTextureScale,
    region: TextureRegion,
    grid: FaceGrid,
) -> [f32; 4] {
    let colors = face_color_rect(image, texture_scale, region, grid);
    let mut total = [0.0f32; 4];
    let mut opaque_count = 0.0f32;
    let mut count = 0.0f32;
    for color in &colors {
        if color[3] > 0.04 {
            for channel in 0..4 {
                total[channel] += color[channel];
            }
            opaque_count += 1.0;
        }
        count += 1.0;
    }

    if opaque_count > 0.0 {
        return [
            total[0] / opaque_count,
            total[1] / opaque_count,
            total[2] / opaque_count,
            (total[3] / opaque_count).min(1.0),
        ];
    }

    if count > 0.0 {
        for channel in 0..4 {
            total[channel] /= count;
        }
        return total;
    }
    [1.0, 1.0, 1.0, 1.0]
}

pub(super) fn face_pixel_corners(
    size: CuboidSize,
    face: Face,
    grid: FaceGrid,
    pixel_x: u32,
    pixel_y: u32,
    inflate: f32,
) -> [[f32; 3]; 4] {
    let u0 = pixel_x as f32 / grid.width as f32;
    let u1 = (pixel_x + 1) as f32 / grid.width as f32;
    let v0 = pixel_y as f32 / grid.height as f32;
    let v1 = (pixel_y + 1) as f32 / grid.height as f32;

    face_uv_corners(size, face, u0, u1, v0, v1, inflate)
}

fn face_uv_corners(
    size: CuboidSize,
    face: Face,
    u0: f32,
    u1: f32,
    v0: f32,
    v1: f32,
    inflate: f32,
) -> [[f32; 3]; 4] {
    let half_width = size.width * 0.5 + inflate;
    let half_height = size.height * 0.5 + inflate;
    let half_depth = size.depth * 0.5 + inflate;

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
    vertices: &mut Vec<SkinVertex>,
    indices: &mut Vec<u32>,
    corners: [[f32; 3]; 4],
    color: [f32; 4],
    edge_mask: QuadEdgeMask,
) {
    let Ok(base) = u32::try_from(vertices.len()) else {
        return;
    };
    vertices.extend([
        SkinVertex {
            position: corners[0],
            color,
            edge_mask: first_triangle_edge_mask(edge_mask),
        },
        SkinVertex {
            position: corners[1],
            color,
            edge_mask: first_triangle_edge_mask(edge_mask),
        },
        SkinVertex {
            position: corners[2],
            color,
            edge_mask: first_triangle_edge_mask(edge_mask),
        },
        SkinVertex {
            position: corners[0],
            color,
            edge_mask: second_triangle_edge_mask(edge_mask),
        },
        SkinVertex {
            position: corners[2],
            color,
            edge_mask: second_triangle_edge_mask(edge_mask),
        },
        SkinVertex {
            position: corners[3],
            color,
            edge_mask: second_triangle_edge_mask(edge_mask),
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
