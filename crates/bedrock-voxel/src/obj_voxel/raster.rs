use std::collections::HashMap;

use super::{ObjTriangle, ObjVoxelOptions, closest, cross, dot, subtract};
use crate::block_image::{LabPalette, linear_srgb, oklab, srgb_linear};
use crate::obj_texture::{SourceTexture, sample};

/// Deterministic in-cell offsets for texture multisampling.
const TEXTURE_SAMPLE_OFFSETS: [[f32; 3]; 4] = [
    [-0.3, -0.3, -0.3],
    [0.3, -0.3, 0.3],
    [-0.3, 0.3, 0.3],
    [0.3, 0.3, -0.3],
];

/// The closest eligible surface's palette index and distance from the cell center.
pub(super) struct SurfaceSample {
    /// Index into the caller's approved block palette.
    pub(super) candidate: usize,
    /// Squared distance in voxel coordinates, used to resolve overlapping surfaces.
    pub(super) distance: f32,
}

pub(super) fn rasterize_triangle(
    triangle: &ObjTriangle,
    minimum: [f32; 3],
    scale: f32,
    dimensions: [usize; 3],
    surface: &mut HashMap<usize, SurfaceSample>,
    top_all: &LabPalette,
    top_opaque: &LabPalette,
    side_all: &LabPalette,
    side_opaque: &LabPalette,
    diffuse: [u8; 3],
    opacity: f32,
    texture: Option<&SourceTexture>,
    options: ObjVoxelOptions,
) {
    if texture.is_none() && opacity * 255.0 < f32::from(options.alpha_threshold) {
        return;
    }
    let vertices = triangle
        .vertices
        .map(|vertex| [0, 1, 2].map(|axis| (vertex.position[axis] - minimum[axis]) * scale));
    let normal = cross(
        subtract(vertices[1], vertices[0]),
        subtract(vertices[2], vertices[0]),
    );
    let (all_palette, opaque_palette) = if normal[1].abs() >= normal[0].abs().max(normal[2].abs()) {
        (top_all, top_opaque)
    } else {
        (side_all, side_opaque)
    };
    let plain_block = if texture.is_none() && opacity * 255.0 >= f32::from(options.alpha_threshold)
    {
        let palette = if opacity < 1.0 {
            all_palette
        } else {
            opaque_palette
        };
        Some(palette.nearest(oklab(diffuse)))
    } else {
        None
    };
    let bounds = [0, 1, 2].map(|axis| {
        let low = vertices
            .iter()
            .map(|vertex| vertex[axis])
            .fold(f32::INFINITY, f32::min);
        let high = vertices
            .iter()
            .map(|vertex| vertex[axis])
            .fold(f32::NEG_INFINITY, f32::max);
        (
            (low.floor() as i32 - 1).max(0) as usize,
            (high.ceil() as usize).min(dimensions[axis] - 1),
        )
    });
    for z in bounds[2].0..=bounds[2].1 {
        for y in bounds[1].0..=bounds[1].1 {
            for x in bounds[0].0..=bounds[0].1 {
                if triangle_intersects_cell(vertices, [x as f32, y as f32, z as f32]) {
                    let index = (z * dimensions[1] + y) * dimensions[0] + x;
                    let (weights, distance) =
                        closest::sample(vertices, [x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5]);
                    if surface
                        .get(&index)
                        .is_some_and(|sample| distance > sample.distance)
                    {
                        continue;
                    }
                    let block = if let Some(texture) = texture {
                        // Average four deterministic in-cell samples. A single
                        // closest-point sample often lands on a specular or
                        // shadow texel; pale character textures then match pure
                        // white blocks and wash out against the preview
                        // background. This mirrors Colorify7's multisampling.
                        let mut color_sum = [0.0f32; 3];
                        let mut alpha_sum = 0.0f32;
                        for offset in TEXTURE_SAMPLE_OFFSETS {
                            let point = [
                                x as f32 + 0.5 + offset[0],
                                y as f32 + 0.5 + offset[1],
                                z as f32 + 0.5 + offset[2],
                            ];
                            let (sample_weights, _) = closest::sample(vertices, point);
                            let uv = sample_uv(triangle, sample_weights);
                            let color = sample(texture, uv);
                            for channel in 0..3 {
                                color_sum[channel] += f32::from(color[channel]);
                            }
                            alpha_sum += f32::from(color[3]);
                        }
                        let count = TEXTURE_SAMPLE_OFFSETS.len() as f32;
                        let pixel = [
                            (color_sum[0] / count).round() as u8,
                            (color_sum[1] / count).round() as u8,
                            (color_sum[2] / count).round() as u8,
                            (alpha_sum / count).round() as u8,
                        ];
                        let alpha = f32::from(pixel[3]) / 255.0 * opacity;
                        if alpha * 255.0 < f32::from(options.alpha_threshold) {
                            continue;
                        }
                        // Alpha only selects translucency. Keep the straight
                        // texture color: compositing a white backdrop here
                        // washed out every semi-transparent texel and made the
                        // converted model far paler than its source.
                        let mut rgb = [0, 1, 2].map(|channel| {
                            linear_srgb(srgb_linear(pixel[channel]) * srgb_linear(diffuse[channel]))
                        });
                        apply_ordered_dither(&mut rgb, [x, y, z]);
                        let palette = if alpha < 1.0 {
                            all_palette
                        } else {
                            opaque_palette
                        };
                        palette.nearest(oklab(rgb))
                    } else {
                        let Some(block) = plain_block else {
                            continue;
                        };
                        block
                    };
                    if surface.get(&index).is_some_and(|sample| {
                        distance.total_cmp(&sample.distance).is_eq() && block >= sample.candidate
                    }) {
                        continue;
                    }
                    surface.insert(
                        index,
                        SurfaceSample {
                            candidate: block,
                            distance,
                        },
                    );
                }
            }
        }
    }
}

/// 4x4x4 ordered dither applied before palette matching.
///
/// The same offset is added to all three channels, so hue is preserved while
/// neighboring cells alternate between the nearest light and dark candidates.
/// This is Colorify7's default (ordered, magnitude 32) and keeps large pale
/// regions from collapsing onto a single flat white block.
const DITHER_MATRIX: [u8; 64] = [
    0, 16, 2, 18, 48, 32, 50, 34, 6, 22, 4, 20, 54, 38, 52, 36, 24, 40, 26, 42, 8, 56, 10, 58, 30,
    46, 28, 44, 14, 62, 12, 60, 3, 19, 5, 21, 51, 35, 53, 37, 1, 17, 7, 23, 49, 33, 55, 39, 27, 43,
    29, 45, 11, 59, 13, 61, 25, 41, 31, 47, 9, 57, 15, 63,
];

const DITHER_MAGNITUDE: f32 = 32.0;

fn apply_ordered_dither(rgb: &mut [u8; 3], position: [usize; 3]) {
    let index = position[0] % 4 + 4 * (position[1] % 4) + 16 * (position[2] % 4);
    let offset = (f32::from(DITHER_MATRIX[index]) / 64.0 - 0.5) * DITHER_MAGNITUDE;
    for channel in rgb {
        *channel = (f32::from(*channel) + offset).clamp(0.0, 255.0).round() as u8;
    }
}

fn sample_uv(triangle: &ObjTriangle, weights: [f32; 3]) -> [f32; 2] {
    let uvs = triangle
        .vertices
        .map(|vertex| vertex.uv.unwrap_or([0.0; 2]));
    [0, 1].map(|axis| {
        (0..3)
            .map(|index| uvs[index][axis] * weights[index])
            .sum::<f32>()
    })
}

fn triangle_intersects_cell(vertices: [[f32; 3]; 3], cell: [f32; 3]) -> bool {
    let centered = vertices.map(|vertex| [0, 1, 2].map(|axis| vertex[axis] - cell[axis] - 0.5));
    let edges = [
        subtract(centered[1], centered[0]),
        subtract(centered[2], centered[1]),
        subtract(centered[0], centered[2]),
    ];
    let normal = cross(edges[0], edges[1]);
    let axes = [
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        normal,
        [0.0, edges[0][2], -edges[0][1]],
        [-edges[0][2], 0.0, edges[0][0]],
        [edges[0][1], -edges[0][0], 0.0],
        [0.0, edges[1][2], -edges[1][1]],
        [-edges[1][2], 0.0, edges[1][0]],
        [edges[1][1], -edges[1][0], 0.0],
        [0.0, edges[2][2], -edges[2][1]],
        [-edges[2][2], 0.0, edges[2][0]],
        [edges[2][1], -edges[2][0], 0.0],
    ];
    axes.into_iter().all(|axis| {
        if dot(axis, axis) <= 1e-12 {
            return true;
        }
        let projected = centered.map(|vertex| dot(vertex, axis));
        let radius = 0.5 * axis.iter().map(|component| component.abs()).sum::<f32>();
        let tolerance = 1e-5 * radius.max(1.0);
        projected.iter().copied().fold(f32::INFINITY, f32::min) <= radius + tolerance
            && projected.iter().copied().fold(f32::NEG_INFINITY, f32::max) >= -radius - tolerance
    })
}
