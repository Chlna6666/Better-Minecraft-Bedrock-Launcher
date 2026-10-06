use super::color::{Face, shade_face_color, shade_layer_edge_color};
use super::geometry::{
    ColorRun, CuboidSize, FaceGrid, QuadEdgeMask, SkinTextureScale, SkinVertex, color_runs,
    face_color_rect, face_grid, face_rect_corners, face_region, push_quad_with_edges,
};
use super::uv::{CuboidUv, TextureRegion};
use image::DynamicImage;

const SKIN_LAYER_ALPHA_THRESHOLD: f32 = 0.04;
const SKIN_LAYER_INNER_INFLATE: f32 = 0.0;

/// Builds one extruded overlay face into `vertices` and `indices`.
///
/// Visible texels are merged into same-color runs, and each run receives one front quad plus side
/// quads for its exposed edges. Merging keeps neighbouring texels sharing one edge instead of
/// overlapping, which removes the see-through seams the per-texel geometry produced.
pub(super) fn push_skin_layer_face(
    layer: &SkinLayerColors<'_>,
    size: CuboidSize,
    face: Face,
    inflate: f32,
    vertices: &mut Vec<SkinVertex>,
    indices: &mut Vec<u32>,
) {
    let SkinLayerColors { colors, grid } = *layer;

    for run in opaque_layer_runs(grid, colors) {
        let Some(color) = colors.get(run.color_index).copied() else {
            continue;
        };
        let outer = face_rect_corners(size, face, grid, run, inflate);
        let inner = face_rect_corners(size, face, grid, run, SKIN_LAYER_INNER_INFLATE);
        push_quad_with_edges(
            vertices,
            indices,
            outer,
            shade_face_color(color, face),
            QuadEdgeMask::NONE,
        );

        for edge in LayerPixelEdge::ALL {
            if layer_edge_is_visible(grid, colors, run, edge) {
                push_layer_edge(vertices, indices, inner, outer, edge, color);
            }
        }
    }
}

/// Authored texel colors for one skin layer face.
pub(super) struct SkinLayerColors<'a> {
    pub(super) colors: &'a [[f32; 4]],
    pub(super) grid: FaceGrid,
}

/// Reads one overlay face into same-color runs and keeps the run list ready for merging.
pub(super) fn layer_face_colors(
    image: &DynamicImage,
    texture_scale: SkinTextureScale,
    region: TextureRegion,
) -> (FaceGrid, Vec<[f32; 4]>) {
    let grid = face_grid(region, texture_scale.preview);
    let colors = face_color_rect(image, texture_scale, region, grid);
    (grid, colors)
}

/// Reports whether an overlay face contains at least one visible texel.
pub(super) fn layer_has_visible_texel(colors: &[[f32; 4]]) -> bool {
    colors
        .iter()
        .any(|color| color[3] > SKIN_LAYER_ALPHA_THRESHOLD)
}

/// Splits same-color runs so that no run mixes opaque and transparent texels.
///
/// A run boundary at a transparent texel is what the layer needs for its side edges, and keeping
/// opaque runs whole means adjacent texels share one quad edge instead of overlapping.
fn opaque_layer_runs(grid: FaceGrid, colors: &[[f32; 4]]) -> Vec<ColorRun> {
    let mut runs = Vec::new();
    for run in color_runs(grid, colors) {
        let mut start = run.x;
        for column in run.x..run.right() {
            let index = (run.y as usize).saturating_mul(grid.width as usize) + column as usize;
            let opaque = colors
                .get(index)
                .is_some_and(|color| color[3] > SKIN_LAYER_ALPHA_THRESHOLD);
            let last = column + 1 == run.right();
            if opaque && !last {
                continue;
            }
            let end = if opaque { column + 1 } else { column };
            if end > start {
                runs.push(ColorRun {
                    x: start,
                    y: run.y,
                    width: end - start,
                    height: 1,
                    color_index: (run.y as usize).saturating_mul(grid.width as usize)
                        + start as usize,
                });
            }
            start = column + 1;
        }
    }
    runs
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
    grid: FaceGrid,
    colors: &[[f32; 4]],
    run: ColorRun,
    edge: LayerPixelEdge,
) -> bool {
    match edge {
        LayerPixelEdge::Top => edge_row_is_transparent(grid, colors, run, run.y.checked_sub(1)),
        LayerPixelEdge::Bottom => edge_row_is_transparent(grid, colors, run, Some(run.bottom())),
        LayerPixelEdge::Left => {
            run.x == 0 || edge_column_is_transparent(grid, colors, run, run.x.checked_sub(1))
        }
        LayerPixelEdge::Right => {
            run.right() >= grid.width
                || edge_column_is_transparent(grid, colors, run, Some(run.right()))
        }
    }
}

fn edge_row_is_transparent(
    grid: FaceGrid,
    colors: &[[f32; 4]],
    run: ColorRun,
    row: Option<u32>,
) -> bool {
    let Some(row) = row.filter(|row| *row < grid.height) else {
        return true;
    };
    (run.x..run.right()).all(|column| !layer_texel_is_opaque(grid, colors, column, row))
}

fn edge_column_is_transparent(
    grid: FaceGrid,
    colors: &[[f32; 4]],
    run: ColorRun,
    column: Option<u32>,
) -> bool {
    let Some(column) = column.filter(|column| *column < grid.width) else {
        return true;
    };
    (run.y..run.bottom()).all(|row| !layer_texel_is_opaque(grid, colors, column, row))
}

fn layer_texel_is_opaque(grid: FaceGrid, colors: &[[f32; 4]], column: u32, row: u32) -> bool {
    let index = (row as usize).saturating_mul(grid.width as usize) + column as usize;
    colors
        .get(index)
        .is_some_and(|color| color[3] > SKIN_LAYER_ALPHA_THRESHOLD)
}

fn push_layer_edge(
    vertices: &mut Vec<SkinVertex>,
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
