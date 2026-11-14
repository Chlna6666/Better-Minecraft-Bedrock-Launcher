//! Pure OBJ geometry conversion into local Bedrock block placements.

use std::{
    collections::{HashMap, VecDeque, hash_map::Entry},
    sync::Arc,
};

use rayon::prelude::*;

use bedrock_world::editor::{BlockOffset, BlockPlacementPlan, PlacementBlock};
use bedrock_world::surface::CancelFlag;

use crate::{
    FlatBlockCandidate, Result,
    block_image::{is_glass_candidate, oklab},
    obj::{ObjModel, ObjTriangle},
    obj_texture::{SourceTexture, TextureAlphaKind, classify_texture_alpha, load_obj_textures},
    validation,
};

const MAX_PLACEMENT_BLOCKS: usize = 2_000_000;
const MAX_SOLID_FLOOD_CELLS: usize = 9_000_000;

mod closest;
mod raster;
use raster::{NormalPalettes, SurfaceSample, rasterize_triangle};

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
            background: [0; 3],
        }
    }
}

/// Voxelize a validated OBJ into local X/Y/Z blocks.
///
/// Source Y remains up, X east, and Z south. The minimum model coordinate maps
/// to local zero. This function reads validated source texture files but does
/// not read or write a world; collision and world-height checks are performed
/// when the plan is placed.
/// Textured surfaces use the bilinear map_Kd sample without the MTL diffuse tint; untextured
/// surfaces use their material diffuse RGB. Both are matched in OKLab to the explicitly approved
/// block candidates. Horizontal source faces use
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
/// invalid material or texture data, excessive actual block count, an
/// oversized dense flood fill in solid mode, or an empty placement.
pub fn voxelize_obj(
    model: &ObjModel,
    candidates: &[FlatBlockCandidate],
    options: ObjVoxelOptions,
) -> Result<BlockPlacementPlan> {
    let textures = load_obj_textures(model)?;
    voxelize_textured(
        VoxelizeTexturesRequest {
            model,
            textures: &textures,
            candidates,
            options,
            cancel: &CancelFlag::new(),
        },
        |_, _| {},
    )
}

/// Inputs for voxelizing an OBJ with pre-decoded textures. JS/TS-style parameter
/// object: one `voxelize_textured` entry instead of a `_with_*` variant chain.
pub struct VoxelizeTexturesRequest<'a> {
    pub model: &'a ObjModel,
    pub textures: &'a [Option<Arc<SourceTexture>>],
    pub candidates: &'a [FlatBlockCandidate],
    pub options: ObjVoxelOptions,
    pub cancel: &'a CancelFlag,
}

/// Uses previously decoded MTL images shared with the GPUI-3D source preview.
/// Call this when generating both views from the same source TaskManager job.
///
/// # Errors
/// Returns the same geometry, palette and cancellation errors as the ordinary voxelizer.
pub fn voxelize_textured(
    request: VoxelizeTexturesRequest<'_>,
    mut progress: impl FnMut(usize, usize),
) -> Result<BlockPlacementPlan> {
    let VoxelizeTexturesRequest {
        model,
        textures,
        candidates,
        options,
        cancel,
    } = request;

    if textures.len() != model.materials.len() {
        return Err(validation("OBJ texture table has incorrect material count"));
    }
    if !(1..=384).contains(&options.longest_side_blocks) {
        return Err(validation("OBJ longest side must be 1..=384 blocks"));
    }
    if model.triangles.is_empty() || candidates.is_empty() {
        return Err(validation(
            "OBJ geometry and block candidates must be nonempty",
        ));
    }
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
    // Surface mode rasterizes only triangle-intersecting cells into a sparse
    // map. Empty bounding-box space never allocates any voxel records.
    // Solid fill does allocate a dense padded flood-fill map: preflight that
    // allocation separately so oversized solid requests fail immediately.
    if options.fill == ObjFill::Solid {
        let flood_volume = dimensions.iter().try_fold(1_usize, |volume, &length| {
            volume.checked_mul(length + 2)
        }).ok_or_else(|| validation("solid OBJ flood-fill volume overflows"))?;
        if flood_volume > MAX_SOLID_FLOOD_CELLS {
            return Err(validation(
                "实心 OBJ 体积超过洪水填充上限（900 万格）；请降低目标尺寸或选择“空心·仅表面”",
            ));
        }
    }
    let candidate_labs = candidates
        .iter()
        .map(|candidate| candidate_lab(candidate.top_color))
        .collect::<Vec<_>>();
    let candidate_side_labs = candidates
        .iter()
        .map(|candidate| candidate_lab(candidate.side_color))
        .collect::<Vec<_>>();
    let opaque_candidate_indices = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| (!is_glass_candidate(candidate)).then_some(index))
        .collect::<Vec<_>>();
    let allowed_all = vec![true; candidates.len()];
    let allowed_opaque = candidates
        .iter()
        .map(|candidate| !is_glass_candidate(candidate))
        .collect::<Vec<_>>();
    // Cache a small set of exact weighted top/side palettes once per
    // conversion. Each voxel still performs one SIMD nearest-color lookup.
    let normal_palettes = NormalPalettes::new(
        &candidate_labs,
        &candidate_side_labs,
        &allowed_all,
        &allowed_opaque,
    );
    if opaque_candidate_indices.is_empty() {
        return Err(validation("no opaque block colors were supplied"));
    }
    let opaque_candidate_mask = candidates
        .iter()
        .map(|candidate| !is_glass_candidate(candidate))
        .collect::<Vec<_>>();
    // Classify once per material, not for every triangle or voxel sample.
    // A cutout atlas has binary coverage and must not generate translucent
    // glass blocks from antialiased edge pixels.
    let texture_alpha_kinds = textures
        .iter()
        .map(|texture| {
            texture.as_ref().map_or(TextureAlphaKind::Opaque, |texture| {
                classify_texture_alpha(&texture.pixels)
            })
        })
        .collect::<Vec<_>>();
    // Batch triangles across all cores. Each worker owns a private cell map;
    // reducing keeps the closest surface per cell, with the lower candidate
    // index breaking distance ties exactly like the serial scan.
    let mut surface = model
        .triangles
        .par_chunks(256)
        .map(|triangles| -> Result<HashMap<usize, SurfaceSample>> {
            let mut surface = HashMap::<usize, SurfaceSample>::new();
            for triangle in triangles {
                if cancel.is_cancelled() {
                    return Err(validation("OBJ conversion cancelled"));
                }
                let rgb = material_rgb(model, triangle)?;
                let texture = triangle
                    .material
                    .and_then(|id| textures.get(id))
                    .and_then(Option::as_deref);
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
                    &normal_palettes,
                    rgb,
                    opacity,
                    texture,
                    triangle
                        .material
                        .and_then(|id| texture_alpha_kinds.get(id))
                        .copied()
                        .unwrap_or(TextureAlphaKind::Opaque),
                    options,
                );
                if surface.len() > MAX_PLACEMENT_BLOCKS {
                    return Err(validation("OBJ placement exceeds two million blocks"));
                }
            }
            Ok(surface)
        })
        .try_reduce(HashMap::new, |mut merged, local| {
            for (index, sample) in local {
                match merged.entry(index) {
                    Entry::Occupied(mut entry) => {
                        let current = entry.get();
                        let order = sample.distance.total_cmp(&current.distance);
                        if order.is_lt() || (order.is_eq() && sample.candidate < current.candidate)
                        {
                            entry.insert(sample);
                        }
                    }
                    Entry::Vacant(entry) => {
                        entry.insert(sample);
                    }
                }
            }
            Ok(merged)
        })?;
    // Each parallel worker is bounded, but their merged output can exceed
    // that bound. Enforce the same cap before allocating the sorted plan.
    if surface.len() > MAX_PLACEMENT_BLOCKS {
        return Err(validation("OBJ placement exceeds two million blocks"));
    }
    progress(model.triangles.len(), model.triangles.len());
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
    // MTL Kd is a diffuse tint for untextured materials. PMX/MMD exporters
    // commonly emit a non-white default Kd even when map_Kd already carries the
    // full albedo, and multiplying it there darkens the whole model. Textured
    // surfaces therefore use map_Kd alone, matching Colorify7's OBJ path.
    let diffuse = if material.diffuse_texture.is_some() {
        [1.0; 3]
    } else {
        material.diffuse.unwrap_or([0.75; 3])
    };
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

fn candidate_lab(color: [u8; 4]) -> [f32; 3] {
    // Alpha only decides whether a glass candidate is eligible. Matching uses
    // the straight RGB so translucent source texels and glass share one space.
    oklab([color[0], color[1], color[2]])
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
    if volume > MAX_SOLID_FLOOD_CELLS {
        return Err(validation(
            "实心 OBJ 体积超过洪水填充上限（900 万格）；请降低目标尺寸或选择“空心·仅表面”",
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
    if surface.len() > MAX_PLACEMENT_BLOCKS {
        return Err(validation("solid OBJ placement exceeds two million blocks"));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
