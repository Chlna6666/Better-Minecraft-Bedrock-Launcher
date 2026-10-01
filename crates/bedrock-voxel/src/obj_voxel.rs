//! Pure OBJ geometry conversion into local Bedrock block placements.

use std::collections::{HashMap, VecDeque};

use bedrock_world::editor::{BlockOffset, BlockPlacementPlan, PlacementBlock};
use bedrock_world::surface::CancelFlag;

use crate::{
    FlatBlockCandidate, Result,
    block_image::{is_glass_candidate, linear_srgb, oklab, srgb_linear},
    obj::{ObjModel, ObjTriangle},
    obj_texture::load_textures,
    validation,
};

mod closest;
mod raster;
use raster::{SurfaceSample, rasterize_triangle};

/// How an OBJ surface is turned into occupied block cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjFill {
    /// Place blocks only where a triangle intersects a cell.
    Surface,
    /// Fill cells enclosed by the triangle surface after a 3D exterior flood fill.
    Solid,
}

/// Size and fill controls for a local OBJ placement plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjVoxelOptions {
    /// Number of blocks across the longest source-model dimension, from 1 to 384.
    /// Other dimensions use the same scale without stretching the model.
    pub longest_side_blocks: u16,
    /// Whether to keep the triangle surface or fill its enclosed volume.
    pub fill: ObjFill,
    /// Effective MTL and texture alpha samples below this value are skipped.
    pub alpha_threshold: u8,
    /// RGB background behind semitransparent texture samples.
    pub background: [u8; 3],
}

impl Default for ObjVoxelOptions {
    fn default() -> Self {
        Self {
            longest_side_blocks: 32,
            fill: ObjFill::Surface,
            alpha_threshold: 1,
            background: [255; 3],
        }
    }
}

/// Voxelize a validated OBJ into local X/Y/Z blocks.
///
/// Source Y remains up, X east, and Z south. The minimum model coordinate maps
/// to local zero. This function reads validated source texture files but does
/// not read or write a world; collision and world-height checks are performed
/// when the plan is placed.
/// Material diffuse RGB and bilinear diffuse-texture samples are matched in
/// OKLab to the explicitly approved block candidates. Horizontal source faces use
/// candidate top colors; vertical faces use candidate side colors. Textures are
/// sampled at the closest triangle point to the cell center. When surfaces share a cell,
/// the closest sample determines its color; equal distances use palette order,
/// so reordering source triangles does not change the placement.
/// Texture alpha is applied before matching. Untextured surfaces and fully opaque pixels only match
/// non-glass blocks. Partially transparent MTL or texture alpha may match glass,
/// with both palette and source colors composited over the configured background.
/// Solid-fill interior cells always use an opaque block.
/// A `Solid` result is meaningful for closed shells; open surfaces may have no
/// enclosed cells.
///
/// # Errors
/// Returns a validation error for invalid size, empty model or candidates,
/// invalid material or texture data, excess output volume, or an empty placement.
pub fn voxelize_obj(
    model: &ObjModel,
    candidates: &[FlatBlockCandidate],
    options: ObjVoxelOptions,
) -> Result<BlockPlacementPlan> {
    voxelize_obj_with_progress(model, candidates, options, &CancelFlag::new(), |_, _| {})
}

/// Voxelize with a cooperative cancellation check and triangle-count progress.
///
/// Progress counts parsed source triangles and is emitted after each 256
/// triangles and at completion. A cancellation request stops before another
/// triangle or flood-fill step; no world data has been modified.
///
/// # Errors
/// Returns the same validation errors as [`voxelize_obj`], or a cancellation
/// error when `cancel` has been requested.
pub fn voxelize_obj_with_progress(
    model: &ObjModel,
    candidates: &[FlatBlockCandidate],
    options: ObjVoxelOptions,
    cancel: &CancelFlag,
    mut progress: impl FnMut(usize, usize),
) -> Result<BlockPlacementPlan> {
    if !(1..=384).contains(&options.longest_side_blocks) {
        return Err(validation("OBJ longest side must be 1..=384 blocks"));
    }
    if model.triangles.is_empty() || candidates.is_empty() {
        return Err(validation(
            "OBJ geometry and block candidates must be nonempty",
        ));
    }
    let textures = load_textures(model)?;
    if cancel.is_cancelled() {
        return Err(validation("OBJ conversion cancelled"));
    }
    let mut minimum = [f32::INFINITY; 3];
    let mut maximum = [f32::NEG_INFINITY; 3];
    for triangle in &model.triangles {
        if cancel.is_cancelled() {
            return Err(validation("OBJ conversion cancelled"));
        }
        for vertex in triangle.vertices {
            if !vertex.position.iter().all(|value| value.is_finite()) {
                return Err(validation("OBJ position is not finite"));
            }
            if vertex
                .uv
                .is_some_and(|uv| !uv.iter().all(|value| value.is_finite()))
            {
                return Err(validation("OBJ UV is not finite"));
            }
            for axis in 0..3 {
                minimum[axis] = minimum[axis].min(vertex.position[axis]);
                maximum[axis] = maximum[axis].max(vertex.position[axis]);
            }
        }
    }
    let extent = (0..3)
        .map(|axis| maximum[axis] - minimum[axis])
        .collect::<Vec<_>>();
    let longest = extent.iter().copied().fold(0.0_f32, f32::max);
    if !longest.is_finite() || longest <= 0.0 {
        return Err(validation("OBJ has no usable spatial extent"));
    }
    let scale = f32::from(options.longest_side_blocks) / longest;
    if !scale.is_finite() {
        return Err(validation("OBJ scale exceeds numeric range"));
    }
    let dimensions = [0, 1, 2].map(|axis| (extent[axis] * scale).ceil().max(1.0) as usize);
    let volume = dimensions
        .iter()
        .try_fold(1_usize, |volume, &length| volume.checked_mul(length))
        .ok_or_else(|| validation("OBJ voxel volume overflows"))?;
    if volume > 8_000_000 {
        return Err(validation("OBJ voxel volume exceeds eight million cells"));
    }
    let candidate_labs = candidates
        .iter()
        .map(|candidate| candidate_lab(candidate.top_color, options.background))
        .collect::<Vec<_>>();
    let candidate_side_labs = candidates
        .iter()
        .map(|candidate| candidate_lab(candidate.side_color, options.background))
        .collect::<Vec<_>>();
    let all_candidate_indices = (0..candidates.len()).collect::<Vec<_>>();
    let opaque_candidate_indices = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| (!is_glass_candidate(candidate)).then_some(index))
        .collect::<Vec<_>>();
    if opaque_candidate_indices.is_empty() {
        return Err(validation("no opaque block colors were supplied"));
    }
    let opaque_candidate_mask = candidates
        .iter()
        .map(|candidate| !is_glass_candidate(candidate))
        .collect::<Vec<_>>();
    let mut surface = HashMap::<usize, SurfaceSample>::new();
    for (triangle_index, triangle) in model.triangles.iter().enumerate() {
        if cancel.is_cancelled() {
            return Err(validation("OBJ conversion cancelled"));
        }
        let rgb = material_rgb(model, triangle)?;
        let texture = triangle.material.and_then(|id| textures[id].as_deref());
        if texture.is_some() && triangle.vertices.iter().any(|vertex| vertex.uv.is_none()) {
            return Err(validation("textured OBJ face is missing UV coordinates"));
        }
        let opacity = material_opacity(model, triangle)?;
        let edge_a = subtract(triangle.vertices[1].position, triangle.vertices[0].position);
        let edge_b = subtract(triangle.vertices[2].position, triangle.vertices[0].position);
        let normal = cross(edge_a, edge_b);
        if dot(normal, normal) <= dot(edge_a, edge_a) * dot(edge_b, edge_b) * 1e-12 {
            return Err(validation("OBJ contains a degenerate triangle"));
        }
        rasterize_triangle(
            triangle,
            minimum,
            scale,
            dimensions,
            &mut surface,
            &candidate_labs,
            &candidate_side_labs,
            &all_candidate_indices,
            &opaque_candidate_indices,
            rgb,
            opacity,
            texture,
            options,
        );
        if surface.len() > 2_000_000 {
            return Err(validation("OBJ placement exceeds two million blocks"));
        }
        if (triangle_index + 1) % 256 == 0 || triangle_index + 1 == model.triangles.len() {
            progress(triangle_index + 1, model.triangles.len());
        }
    }
    if surface.is_empty() {
        return Err(validation("OBJ produced no occupied cells"));
    }
    if options.fill == ObjFill::Solid {
        fill_enclosed(
            &mut surface,
            dimensions,
            &opaque_candidate_indices,
            &opaque_candidate_mask,
            cancel,
        )?;
    }
    let mut cells = surface.into_iter().collect::<Vec<_>>();
    cells.sort_unstable_by_key(|(index, _)| *index);
    let blocks = cells
        .into_iter()
        .map(|(index, sample)| {
            let x = index % dimensions[0];
            let y = index / dimensions[0] % dimensions[1];
            let z = index / dimensions[0] / dimensions[1];
            PlacementBlock {
                offset: BlockOffset {
                    x: x as i32,
                    y: y as i32,
                    z: z as i32,
                },
                state: candidates[sample.candidate].state.clone(),
            }
        })
        .collect();
    BlockPlacementPlan::new(blocks).map_err(|error| error.to_string())
}

fn material_rgb(model: &ObjModel, triangle: &ObjTriangle) -> Result<[u8; 3]> {
    let Some(material_id) = triangle.material else {
        return Ok([192; 3]);
    };
    let material = model
        .materials
        .get(material_id)
        .ok_or_else(|| validation("OBJ triangle references a missing material"))?;
    let diffuse = material
        .diffuse
        .unwrap_or(if material.diffuse_texture.is_some() {
            [1.0; 3]
        } else {
            [0.75; 3]
        });
    if !diffuse.iter().all(|value| value.is_finite()) {
        return Err(validation("OBJ material has nonfinite diffuse color"));
    }
    Ok(diffuse.map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8))
}

fn material_opacity(model: &ObjModel, triangle: &ObjTriangle) -> Result<f32> {
    let Some(material_id) = triangle.material else {
        return Ok(1.0);
    };
    let material = model
        .materials
        .get(material_id)
        .ok_or_else(|| validation("OBJ triangle references a missing material"))?;
    if !material.opacity.is_finite() || !(0.0..=1.0).contains(&material.opacity) {
        return Err(validation("OBJ material opacity must be between 0 and 1"));
    }
    Ok(material.opacity)
}

fn candidate_lab(color: [u8; 4], background: [u8; 3]) -> [f32; 3] {
    let alpha = f32::from(color[3]) / 255.0;
    oklab([0, 1, 2].map(|channel| {
        let foreground = srgb_linear(color[channel]);
        let background = srgb_linear(background[channel]);
        linear_srgb(foreground * alpha + background * (1.0 - alpha))
    }))
}

fn subtract(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|axis| a[axis] - b[axis])
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|axis| a[axis] * b[axis]).sum()
}

fn fill_enclosed(
    surface: &mut HashMap<usize, SurfaceSample>,
    dimensions: [usize; 3],
    opaque_candidate_indices: &[usize],
    opaque_candidate_mask: &[bool],
    cancel: &CancelFlag,
) -> Result<()> {
    let padded = dimensions.map(|length| length + 2);
    let volume = padded.iter().product::<usize>();
    if volume > 9_000_000 {
        return Err(validation(
            "solid OBJ flood-fill volume exceeds nine million cells",
        ));
    }
    let mut marks = vec![0_u8; volume];
    let encode = |x: usize, y: usize, z: usize| (z * padded[1] + y) * padded[0] + x;
    for &index in surface.keys() {
        let x = index % dimensions[0];
        let y = index / dimensions[0] % dimensions[1];
        let z = index / dimensions[0] / dimensions[1];
        marks[encode(x + 1, y + 1, z + 1)] = 1;
    }
    let mut queue = VecDeque::from([0_usize]);
    marks[0] = 2;
    let mut scanned = 0_usize;
    while let Some(index) = queue.pop_front() {
        scanned += 1;
        if scanned % 4096 == 0 && cancel.is_cancelled() {
            return Err(validation("OBJ conversion cancelled"));
        }
        let x = index % padded[0];
        let y = index / padded[0] % padded[1];
        let z = index / padded[0] / padded[1];
        for (next, valid) in [
            (index.wrapping_sub(1), x > 0),
            (index + 1, x + 1 < padded[0]),
            (index.wrapping_sub(padded[0]), y > 0),
            (index + padded[0], y + 1 < padded[1]),
            (index.wrapping_sub(padded[0] * padded[1]), z > 0),
            (index + padded[0] * padded[1], z + 1 < padded[2]),
        ] {
            if valid && marks[next] == 0 {
                marks[next] = 2;
                queue.push_back(next);
            }
        }
    }
    let mut counts = HashMap::<usize, usize>::new();
    for sample in surface.values() {
        let candidate = sample.candidate;
        if opaque_candidate_mask[candidate] {
            *counts.entry(candidate).or_default() += 1;
        }
    }
    let interior_block = counts
        .into_iter()
        .max_by_key(|&(candidate, count)| (count, usize::MAX - candidate))
        .map(|(candidate, _)| candidate)
        .or_else(|| opaque_candidate_indices.first().copied())
        .ok_or_else(|| validation("OBJ surface is empty"))?;
    for z in 0..dimensions[2] {
        for y in 0..dimensions[1] {
            for x in 0..dimensions[0] {
                if marks[encode(x + 1, y + 1, z + 1)] == 0 {
                    surface.insert(
                        (z * dimensions[1] + y) * dimensions[0] + x,
                        SurfaceSample {
                            candidate: interior_block,
                            distance: 0.0,
                        },
                    );
                }
            }
        }
    }
    if surface.len() > 2_000_000 {
        return Err(validation("solid OBJ placement exceeds two million blocks"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
