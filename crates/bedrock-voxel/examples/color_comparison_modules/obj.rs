use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
};

use super::{
    image::{linear_srgb, oklab_delta_e, print_block_distribution, save_png, srgb_linear},
    obj_source::{Projection, blank_projection, obj_xy_source_projection},
};
use ::image::{Rgba, RgbaImage, imageops};
use bedrock_voxel::{
    ObjFill, ObjVoxelOptions, default_block_candidates, load_obj_model, voxelize_obj,
};

pub(super) fn compare_obj(
    input_path: &Path,
    output_dir: &Path,
    longest_side: u16,
    exclude_snow: bool,
    compare_order: bool,
) -> Result<(), String> {
    let obj_path = resolve_obj_path(input_path)?;
    let model = load_obj_model(&obj_path)?;
    let all_candidates = default_block_candidates();
    let candidates = if exclude_snow {
        all_candidates
            .iter()
            .cloned()
            .filter(|candidate| candidate.state.name != "minecraft:snow")
            .collect::<Vec<_>>()
    } else {
        all_candidates.to_vec()
    };
    let palette_name = if exclude_snow {
        "without_snow"
    } else {
        "full_palette"
    };
    println!("palette={palette_name} candidates={}", candidates.len());
    print_obj_materials(&model);
    let front_plan = voxelize_obj(&model, &candidates, voxel_options(longest_side))
        .map_err(|error| error.to_string())?;
    let reverse_plan = if compare_order {
        let mut reversed_model = model.clone();
        reversed_model.triangles.reverse();
        Some(
            voxelize_obj(&reversed_model, &candidates, voxel_options(longest_side))
                .map_err(|error| error.to_string())?,
        )
    } else {
        None
    };
    if let Some(reverse_plan) = &reverse_plan {
        let (changed_cells, compared_cells) = changed_placement_cells(&front_plan, reverse_plan);
        println!(
            "triangle_order forward_blocks={} reversed_blocks={} changed_cells={changed_cells}/{compared_cells} changed_percent={:.2}",
            front_plan.blocks().len(),
            reverse_plan.blocks().len(),
            changed_cells as f64 * 100.0 / compared_cells.max(1) as f64
        );
    }
    for view_positive_z in [false, true] {
        let view_name = if view_positive_z { "+Z" } else { "-Z" };
        let direction = if view_positive_z {
            "positive_z"
        } else {
            "negative_z"
        };
        let source_projection = obj_xy_source_projection(&model, longest_side, view_positive_z)?;
        let nearest_projection = nearest_palette_projection(&source_projection, &candidates);
        let (voxel_projection, counts) = obj_xy_voxel_projection(
            &front_plan,
            &candidates,
            source_projection.pixels.width(),
            source_projection.pixels.height(),
            view_positive_z,
        )?;
        let source_path = output_dir.join(format!(
            "obj_{longest_side}_{direction}_xy_source_material.png"
        ));
        let nearest_path = output_dir.join(format!(
            "obj_{longest_side}_{direction}_xy_nearest_palette.png"
        ));
        let voxel_path = output_dir.join(format!(
            "obj_{longest_side}_{direction}_xy_voxel_output.png"
        ));
        save_png(&source_projection.pixels, &source_path)?;
        save_png(&nearest_projection.pixels, &nearest_path)?;
        save_png(&voxel_projection.pixels, &voxel_path)?;
        let mut panels = vec![
            source_projection.pixels.clone(),
            nearest_projection.pixels.clone(),
            voxel_projection.pixels.clone(),
        ];
        if let Some(reverse_plan) = &reverse_plan {
            let (reverse_projection, _) = obj_xy_voxel_projection(
                reverse_plan,
                &candidates,
                source_projection.pixels.width(),
                source_projection.pixels.height(),
                view_positive_z,
            )?;
            let reverse_path = output_dir.join(format!(
                "obj_{longest_side}_{direction}_xy_voxel_reversed.png"
            ));
            save_png(&reverse_projection.pixels, &reverse_path)?;
            let order_overlap = voxel_projection
                .mask
                .iter()
                .zip(&reverse_projection.mask)
                .map(|(first, second)| *first && *second)
                .collect::<Vec<_>>();
            print_projection_color_error(
                "forward_vs_reversed_order",
                &voxel_projection,
                &reverse_projection,
                &order_overlap,
            );
            print_obj_projection_metrics(
                view_name,
                "reversed_order",
                &source_projection,
                &nearest_projection,
                &reverse_projection,
            );
            println!("reversed_projection={}", reverse_path.display());
            panels.push(reverse_projection.pixels.clone());
        }
        println!(
            "obj={} triangles={} materials={} view={view_name} blocks={} XY_projection={}x{}",
            obj_path.display(),
            model.triangles.len(),
            model.materials.len(),
            front_plan.blocks().len(),
            source_projection.pixels.width(),
            source_projection.pixels.height()
        );
        print_block_distribution(&counts, 24);
        print_obj_projection_metrics(
            view_name,
            "forward_order",
            &source_projection,
            &nearest_projection,
            &voxel_projection,
        );
        println!("source_projection={}", source_path.display());
        println!("nearest_projection={}", nearest_path.display());
        println!("voxel_projection={}", voxel_path.display());
        let comparison_path =
            output_dir.join(format!("obj_{longest_side}_{direction}_xy_comparison.png"));
        let panel_width = source_projection.pixels.width();
        let mut mosaic = RgbaImage::new(
            panel_width * panels.len() as u32,
            source_projection.pixels.height(),
        );
        for (index, panel) in panels.iter().enumerate() {
            imageops::overlay(&mut mosaic, panel, (index as u32 * panel_width) as i64, 0);
        }
        save_png(&mosaic, &comparison_path)?;
        println!(
            "comparison_panels=source,nearest,forward{}",
            if panels.len() == 4 { ",reversed" } else { "" }
        );
        println!("comparison={}", comparison_path.display());
    }
    Ok(())
}

fn voxel_options(longest_side: u16) -> ObjVoxelOptions {
    ObjVoxelOptions {
        longest_side_blocks: longest_side,
        fill: ObjFill::Surface,
        alpha_threshold: 1,
        background: [255; 3],
    }
}

fn resolve_obj_path(input_path: &Path) -> Result<PathBuf, String> {
    if !input_path.is_dir() {
        return Ok(input_path.to_path_buf());
    }
    let mut obj_paths = fs::read_dir(input_path)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("obj"))
        })
        .collect::<Vec<_>>();
    obj_paths.sort();
    match obj_paths.as_slice() {
        [path] => Ok(path.clone()),
        [] => Err("input directory contains no OBJ file".to_owned()),
        _ => Err("input directory contains more than one OBJ file; pass one OBJ path".to_owned()),
    }
}

fn print_obj_materials(model: &bedrock_voxel::ObjModel) {
    let mut diffuse_colors = BTreeMap::<String, usize>::new();
    let mut textures = BTreeMap::<String, usize>::new();
    for material in &model.materials {
        let effective_diffuse = material
            .diffuse
            .unwrap_or(if material.diffuse_texture.is_some() {
                [1.0; 3]
            } else {
                [0.75; 3]
            });
        let color = effective_diffuse
            .map(|channel| format!("{channel:.3}"))
            .join(",");
        *diffuse_colors.entry(color).or_insert(0) += 1;
        let texture = material
            .diffuse_texture
            .as_deref()
            .map_or_else(|| "<none>".to_owned(), |path| path.display().to_string());
        *textures.entry(texture).or_insert(0) += 1;
    }
    println!("effective MTL Kd values (including voxelizer defaults): {diffuse_colors:?}");
    println!("diffuse texture references loaded through OBJ API: {textures:?}");
}

fn obj_xy_voxel_projection(
    plan: &bedrock_world::editor::BlockPlacementPlan,
    candidates: &[bedrock_voxel::FlatBlockCandidate],
    width: u32,
    height: u32,
    view_positive_z: bool,
) -> Result<(Projection, BTreeMap<String, usize>), String> {
    let colors = candidates
        .iter()
        .map(|candidate| (candidate.state.name.as_str(), candidate.side_color))
        .collect::<HashMap<_, _>>();
    let mut columns = HashMap::<(u32, u32), (i32, [u8; 4])>::new();
    let mut counts = BTreeMap::new();
    for block in plan.blocks() {
        let color = *colors
            .get(block.state.name.as_str())
            .ok_or_else(|| format!("no preview color for {}", block.state.name))?;
        let x = u32::try_from(block.offset.x).map_err(|error| error.to_string())?;
        let y = height
            .checked_sub(1 + u32::try_from(block.offset.y).map_err(|error| error.to_string())?)
            .ok_or_else(|| "voxel Y lies outside projected dimensions".to_owned())?;
        if x >= width || y >= height {
            return Err("voxel lies outside projected dimensions".to_owned());
        }
        let column = (x, y);
        let is_nearer = columns.get(&column).is_none_or(|(existing_z, _)| {
            if view_positive_z {
                block.offset.z > *existing_z
            } else {
                block.offset.z < *existing_z
            }
        });
        if is_nearer {
            columns.insert(column, (block.offset.z, color));
        }
        *counts.entry(block.state.name.clone()).or_insert(0) += 1;
    }
    let mut projection = blank_projection(width, height);
    for ((x, y), (_, color)) in columns {
        let pixel_index = (y * width + x) as usize;
        projection
            .pixels
            .put_pixel(x, y, Rgba(composite_over_white_linear(color)));
        projection.mask[pixel_index] = true;
    }
    Ok((projection, counts))
}

fn nearest_palette_projection(
    source: &Projection,
    candidates: &[bedrock_voxel::FlatBlockCandidate],
) -> Projection {
    let mut nearest = blank_projection(source.pixels.width(), source.pixels.height());
    nearest.alpha.clone_from(&source.alpha);
    nearest.top_face.clone_from(&source.top_face);
    for (index, &present) in source.mask.iter().enumerate() {
        if !present {
            continue;
        }
        let width = source.pixels.width() as usize;
        let source_pixel = source
            .pixels
            .get_pixel((index % width) as u32, (index / width) as u32);
        let allow_glass = source.alpha[index] < 1.0;
        let target = [source_pixel[0], source_pixel[1], source_pixel[2]];
        let palette_color = candidates
            .iter()
            .enumerate()
            .filter(|(_, candidate)| allow_glass || !is_glass_candidate(candidate))
            .map(|(palette_index, candidate)| {
                // Both reference and voxel projection view cubes along Z, so the
                // visible cube surface uses side colors regardless of mesh normal.
                let color = candidate.side_color;
                let color = composite_over_white_linear(color);
                (
                    palette_index,
                    color,
                    oklab_delta_e(target, [color[0], color[1], color[2]]),
                )
            })
            .min_by(|first, second| {
                first
                    .2
                    .total_cmp(&second.2)
                    .then_with(|| first.0.cmp(&second.0))
            })
            .map(|(_, color, _)| color)
            .unwrap_or([255; 4]);
        let x = (index % width) as u32;
        let y = (index / width) as u32;
        nearest.pixels.put_pixel(x, y, Rgba(palette_color));
        nearest.mask[index] = true;
    }
    nearest
}

fn is_glass_candidate(candidate: &bedrock_voxel::FlatBlockCandidate) -> bool {
    candidate.state.name == "minecraft:glass" || candidate.state.name.ends_with("_glass")
}

fn changed_placement_cells(
    first: &bedrock_world::editor::BlockPlacementPlan,
    second: &bedrock_world::editor::BlockPlacementPlan,
) -> (usize, usize) {
    let placements = |plan: &bedrock_world::editor::BlockPlacementPlan| {
        plan.blocks()
            .iter()
            .map(|block| {
                (
                    (block.offset.x, block.offset.y, block.offset.z),
                    block.state.name.clone(),
                )
            })
            .collect::<HashMap<_, _>>()
    };
    let first = placements(first);
    let second = placements(second);
    let cells = first
        .keys()
        .chain(second.keys())
        .copied()
        .collect::<std::collections::HashSet<_>>();
    let changed = cells
        .iter()
        .filter(|position| first.get(position) != second.get(position))
        .count();
    (changed, cells.len())
}

fn composite_over_white_linear(color: [u8; 4]) -> [u8; 4] {
    let alpha = f32::from(color[3]) / 255.0;
    let rgb =
        [0, 1, 2].map(|channel| linear_srgb(srgb_linear(color[channel]) * alpha + 1.0 - alpha));
    [rgb[0], rgb[1], rgb[2], 255]
}

fn print_obj_projection_metrics(
    view: &str,
    order: &str,
    source: &Projection,
    nearest: &Projection,
    voxel: &Projection,
) {
    let mut intersection = 0_u64;
    let mut union = 0_u64;
    let mut source_count = 0_u64;
    let mut voxel_count = 0_u64;
    let eroded = erode_mask(&source.mask, source.pixels.width(), source.pixels.height());
    let mut overlap_mask = vec![false; source.mask.len()];
    let mut interior_mask = vec![false; source.mask.len()];
    let mut boundary_mask = vec![false; source.mask.len()];
    for index in 0..source.mask.len() {
        let source_present = source.mask[index];
        let voxel_present = voxel.mask[index];
        source_count += u64::from(source_present);
        voxel_count += u64::from(voxel_present);
        union += u64::from(source_present || voxel_present);
        if source_present && voxel_present {
            intersection += 1;
            overlap_mask[index] = true;
            if eroded[index] {
                interior_mask[index] = true;
            } else {
                boundary_mask[index] = true;
            }
        }
    }
    println!(
        "XY view={view} order={order} masks source={source_count} voxel={voxel_count} intersection={intersection} IoU={:.4} source_coverage={:.4} voxel_precision={:.4}",
        intersection as f64 / union.max(1) as f64,
        intersection as f64 / source_count.max(1) as f64,
        intersection as f64 / voxel_count.max(1) as f64
    );
    let actual_delta =
        print_projection_color_error("source_vs_voxel", source, voxel, &overlap_mask);
    let nearest_delta = print_projection_color_error(
        "source_vs_nearest_palette_overlap",
        source,
        nearest,
        &overlap_mask,
    );
    print_projection_color_error(
        "source_vs_nearest_palette_all",
        source,
        nearest,
        &source.mask,
    );
    println!(
        "palette_quantization_deltaE_x100={nearest_delta:.2} voxel_deltaE_x100={actual_delta:.2} excess_over_nearest={:.2}",
        actual_delta - nearest_delta
    );
    print_projection_color_error("interior_eroded_1px", source, voxel, &interior_mask);
    print_projection_color_error(
        "boundary_source_minus_interior",
        source,
        voxel,
        &boundary_mask,
    );
}

fn print_projection_color_error(
    label: &str,
    source: &Projection,
    output: &Projection,
    mask: &[bool],
) -> f64 {
    let mut count = 0_u64;
    let mut absolute = [0_u64; 3];
    let mut delta_e = 0.0;
    let width = source.pixels.width() as usize;
    for (index, &include) in mask.iter().enumerate() {
        if !include {
            continue;
        }
        count += 1;
        let x = (index % width) as u32;
        let y = (index / width) as u32;
        let source_pixel = source.pixels.get_pixel(x, y);
        let output_pixel = output.pixels.get_pixel(x, y);
        delta_e += oklab_delta_e(
            [source_pixel[0], source_pixel[1], source_pixel[2]],
            [output_pixel[0], output_pixel[1], output_pixel[2]],
        );
        for channel in 0..3 {
            absolute[channel] += u64::from(source_pixel[channel].abs_diff(output_pixel[channel]));
        }
    }
    let denom = count.max(1) as f64;
    let mean_delta = delta_e / denom;
    println!(
        "{label} pixels={count} RGB_MAE={:.2}/{:.2}/{:.2} OKLab_Euclidean_deltaE_x100={mean_delta:.2} (not CIEDE2000)",
        absolute[0] as f64 / denom,
        absolute[1] as f64 / denom,
        absolute[2] as f64 / denom
    );
    mean_delta
}

fn erode_mask(mask: &[bool], width: u32, height: u32) -> Vec<bool> {
    let mut eroded = vec![false; mask.len()];
    for y in 1..height.saturating_sub(1) {
        for x in 1..width.saturating_sub(1) {
            let index = (y * width + x) as usize;
            eroded[index] = (-1_i32..=1).all(|dy| {
                (-1_i32..=1).all(|dx| {
                    let neighbor = ((y as i32 + dy) * width as i32 + x as i32 + dx) as usize;
                    mask[neighbor]
                })
            });
        }
    }
    eroded
}
