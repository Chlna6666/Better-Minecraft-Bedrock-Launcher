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

// Alpha-mask coverage is not glass-like transmission. Character hair and
// clothing textures can have opaque pixels next to fully transparent pixels.
const OPAQUE_ALPHA_CUTOFF: f32 = 250.0 / 255.0;

fn uses_translucent_palette(alpha: f32, material_opacity: f32, opaque_samples: usize) -> bool {
    alpha < OPAQUE_ALPHA_CUTOFF
        && (material_opacity < OPAQUE_ALPHA_CUTOFF
            || opaque_samples < TEXTURE_SAMPLE_OFFSETS.len() / 2)
}

fn straight_texture_color(
    premultiplied_rgb_sum: [f32; 3],
    alpha_sum: f32,
    sample_count: usize,
) -> [u8; 4] {
    // Normalize RGB by visible alpha, not by the total number of samples:
    // transparent pixels may contain black padding unrelated to the texture color.
    let visible_weight = (alpha_sum / 255.0).max(f32::EPSILON);
    [
        (premultiplied_rgb_sum[0] / visible_weight).clamp(0.0, 255.0).round() as u8,
        (premultiplied_rgb_sum[1] / visible_weight).clamp(0.0, 255.0).round() as u8,
        (premultiplied_rgb_sum[2] / visible_weight).clamp(0.0, 255.0).round() as u8,
        (alpha_sum / sample_count as f32).clamp(0.0, 255.0).round() as u8,
    ]
}

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
        let palette = if uses_translucent_palette(opacity, opacity, 0) {
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
                    let (_, distance) =
                        closest::sample(vertices, [x as f32 + 0.5, y as f32 + 0.5, z as f32 + 0.5]);
                    if surface
                        .get(&index)
                        .is_some_and(|sample| distance > sample.distance)
                    {
                        continue;
                    }
                    let block = if let Some(texture) = texture {
                        // Resolve four in-cell texture samples. Weight RGB by coverage
                        // so the transparent padding in hair/clothing masks cannot
                        // introduce dark fringe colors or falsely select glass.
                        let mut color_sum = [0.0f32; 3];
                        let mut alpha_sum = 0.0f32;
                        let mut opaque_samples = 0usize;
                        for offset in TEXTURE_SAMPLE_OFFSETS {
                            let point = [
                                x as f32 + 0.5 + offset[0],
                                y as f32 + 0.5 + offset[1],
                                z as f32 + 0.5 + offset[2],
                            ];
                            let (sample_weights, _) = closest::sample(vertices, point);
                            let uv = sample_uv(triangle, sample_weights);
                            let color = sample(texture, uv);
                            let sample_alpha = f32::from(color[3]) / 255.0;
                            for channel in 0..3 {
                                color_sum[channel] += f32::from(color[channel]) * sample_alpha;
                            }
                            alpha_sum += f32::from(color[3]);
                            opaque_samples += usize::from(color[3] >= 250);
                        }
                        let pixel =
                            straight_texture_color(color_sum, alpha_sum, TEXTURE_SAMPLE_OFFSETS.len());
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
                        let palette = if uses_translucent_palette(alpha, opacity, opaque_samples) {
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

#[cfg(test)]
mod raster_alpha_tests {
    use super::{straight_texture_color, uses_translucent_palette};

    #[test]
    fn transparent_padding_does_not_darken_opaque_texture_colors() {
        // Three opaque white texels and one transparent black texel are
        // still white; the fourth texel only reduces coverage.
        let pixel = straight_texture_color([765.0; 3], 765.0, 4);
        assert_eq!(pixel, [255, 255, 255, 191]);
    }

    #[test]
    fn alpha_mask_coverage_is_not_translucent_glass() {
        assert!(!uses_translucent_palette(191.0 / 255.0, 1.0, 3));
        assert!(!uses_translucent_palette(254.0 / 255.0, 1.0, 0));
        assert!(uses_translucent_palette(0.5, 1.0, 0));
        assert!(uses_translucent_palette(0.5, 0.5, 4));
    }
}
