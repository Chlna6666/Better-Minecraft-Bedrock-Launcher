use std::collections::HashMap;

use super::{ObjTriangle, ObjVoxelOptions, closest, cross, dot, subtract};
use crate::block_image::{distance_squared, linear_srgb, oklab, srgb_linear};
use crate::obj_texture::sample;
use image::RgbaImage;

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
    top_candidate_labs: &[[f32; 3]],
    side_candidate_labs: &[[f32; 3]],
    all_candidate_indices: &[usize],
    opaque_candidate_indices: &[usize],
    diffuse: [u8; 3],
    opacity: f32,
    texture: Option<&RgbaImage>,
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
    let candidate_labs = if normal[1].abs() >= normal[0].abs().max(normal[2].abs()) {
        top_candidate_labs
    } else {
        side_candidate_labs
    };
    let plain_block = if texture.is_none() && opacity * 255.0 >= f32::from(options.alpha_threshold)
    {
        let indices = if opacity < 1.0 {
            all_candidate_indices
        } else {
            opaque_candidate_indices
        };
        let rgb = composite_rgb(diffuse, opacity, options.background);
        Some(nearest_block(rgb, candidate_labs, indices))
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
                        let uv = sample_uv(triangle, weights);
                        let pixel = sample(texture, uv);
                        let alpha = f32::from(pixel[3]) / 255.0 * opacity;
                        if alpha * 255.0 < f32::from(options.alpha_threshold) {
                            continue;
                        }
                        let rgb = [0, 1, 2].map(|channel| {
                            linear_srgb(
                                srgb_linear(pixel[channel]) * srgb_linear(diffuse[channel]) * alpha
                                    + srgb_linear(options.background[channel]) * (1.0 - alpha),
                            )
                        });
                        let match_indices = if alpha < 1.0 {
                            all_candidate_indices
                        } else {
                            opaque_candidate_indices
                        };
                        nearest_block(rgb, candidate_labs, match_indices)
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

fn composite_rgb(foreground: [u8; 3], alpha: f32, background: [u8; 3]) -> [u8; 3] {
    [0, 1, 2].map(|channel| {
        linear_srgb(
            srgb_linear(foreground[channel]) * alpha
                + srgb_linear(background[channel]) * (1.0 - alpha),
        )
    })
}

fn nearest_block(rgb: [u8; 3], candidates: &[[f32; 3]], indices: &[usize]) -> usize {
    let target = oklab(rgb);
    indices
        .iter()
        .min_by(|a_index, b_index| {
            distance_squared(target, candidates[**a_index])
                .total_cmp(&distance_squared(target, candidates[**b_index]))
                .then(a_index.cmp(b_index))
        })
        .copied()
        .unwrap_or(0)
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
