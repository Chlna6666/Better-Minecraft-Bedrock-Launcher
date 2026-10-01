use std::{
    collections::{BTreeMap, HashMap},
    env,
    path::Path,
};

use ::image::{ImageReader, Rgba, RgbaImage, imageops};
use bedrock_voxel::{Dithering, FlatImageOptions, default_block_candidates, flat_block_plan};

pub(super) fn compare_image(input_path: &Path, output_dir: &Path) -> Result<(), String> {
    let source = ImageReader::open(input_path)
        .map_err(|error| error.to_string())?
        .decode()
        .map_err(|error| error.to_string())?
        .to_rgba8();
    let candidates = default_block_candidates();
    let stem = input_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("image");

    println!(
        "image={} source={}x{} candidates={}",
        input_path.display(),
        source.width(),
        source.height(),
        candidates.len()
    );

    for side in [128, 256] {
        let resized = center_crop_resize(&source, side, side)?;
        save_png(
            &resized,
            &output_dir.join(format!("{stem}_{side}_source.png")),
        )?;
        let mut panels = vec![resized.clone()];
        for (name, dithering) in [
            ("none", Dithering::None),
            ("floyd_steinberg", Dithering::FloydSteinberg),
        ] {
            let plan = flat_block_plan(
                resized.as_raw(),
                side,
                side,
                candidates,
                FlatImageOptions {
                    dithering,
                    alpha_threshold: 1,
                    background: [255; 3],
                    transparent_fill: None,
                },
            )
            .map_err(|error| error.to_string())?;
            let (blocks, counts) = image_from_plan(&plan, side, side, candidates)?;
            let output_path = output_dir.join(format!("{stem}_{side}_{name}.png"));
            save_png(&blocks, &output_path)?;
            let structure = bedrock_world::structure::McStructureFile::from_placement_plan(&plan)
                .map_err(|error| error.to_string())?;
            std::fs::write(
                output_path.with_extension("mcstructure"),
                structure.to_bytes().map_err(|error| error.to_string())?,
            )
            .map_err(|error| error.to_string())?;
            print_color_metrics(&resized, &blocks, &plan.blocks().len(), side, name);
            print_block_distribution(&counts, 12);
            println!("output={}", output_path.display());
            panels.push(blocks);
        }
        let old_floyd_path = env::temp_dir()
            .join("bedrock-voxel-color-comparison")
            .join(format!("{stem}_{side}_floyd_steinberg.png"));
        if old_floyd_path.is_file() {
            let old_floyd = ImageReader::open(&old_floyd_path)
                .map_err(|error| error.to_string())?
                .decode()
                .map_err(|error| error.to_string())?
                .to_rgba8();
            if old_floyd.dimensions() == (side, side) {
                panels.push(old_floyd);
            }
        }
        let mosaic_path = output_dir.join(format!("{stem}_{side}_comparison.png"));
        let mut mosaic = RgbaImage::from_pixel(side * panels.len() as u32, side, Rgba([255; 4]));
        for (index, panel) in panels.iter().enumerate() {
            imageops::overlay(&mut mosaic, panel, (index as u32 * side) as i64, 0);
        }
        save_png(&mosaic, &mosaic_path)?;
        println!(
            "comparison_panels=source,none,floyd_after{}",
            if panels.len() == 4 {
                ",floyd_baseline"
            } else {
                ""
            }
        );
        println!("comparison={}", mosaic_path.display());
    }
    Ok(())
}

pub(super) fn compare_saved_images(source_path: &Path, output_path: &Path) -> Result<(), String> {
    let source = ImageReader::open(source_path)
        .map_err(|error| error.to_string())?
        .decode()
        .map_err(|error| error.to_string())?
        .to_rgba8();
    let output = ImageReader::open(output_path)
        .map_err(|error| error.to_string())?
        .decode()
        .map_err(|error| error.to_string())?
        .to_rgba8();
    if source.dimensions() != output.dimensions() || source.width() != source.height() {
        return Err("metrics require matching square source and converted PNGs".to_owned());
    }
    let block_count = output.pixels().filter(|pixel| pixel[3] > 0).count();
    let side = source.width();
    println!("saved_source={}", source_path.display());
    println!("saved_api_output={}", output_path.display());
    print_color_metrics(&source, &output, &block_count, side, "saved_api_output");
    Ok(())
}

fn center_crop_resize(source: &RgbaImage, width: u32, height: u32) -> Result<RgbaImage, String> {
    let source_width = source.width();
    let source_height = source.height();
    if source_width == 0 || source_height == 0 || width == 0 || height == 0 {
        return Err("image dimensions must be nonzero".to_owned());
    }

    let (crop_x, crop_y, crop_width, crop_height) = if u64::from(source_width) * u64::from(height)
        > u64::from(source_height) * u64::from(width)
    {
        let crop_width = source_height * width / height;
        (
            (source_width - crop_width) / 2,
            0,
            crop_width,
            source_height,
        )
    } else {
        let crop_height = source_width * height / width;
        (
            0,
            (source_height - crop_height) / 2,
            source_width,
            crop_height,
        )
    };
    let cropped = imageops::crop_imm(source, crop_x, crop_y, crop_width, crop_height).to_image();
    let mut premultiplied = cropped;
    for pixel in premultiplied.pixels_mut() {
        let alpha = u16::from(pixel[3]);
        for channel in &mut pixel.0[..3] {
            *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
        }
    }
    let mut resized = imageops::resize(
        &premultiplied,
        width,
        height,
        imageops::FilterType::Lanczos3,
    );
    for pixel in resized.pixels_mut() {
        let alpha = u32::from(pixel[3]);
        if alpha == 0 {
            pixel.0 = [0; 4];
            continue;
        }
        for channel in &mut pixel.0[..3] {
            *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
        }
    }
    Ok(resized)
}

fn image_from_plan(
    plan: &bedrock_world::editor::BlockPlacementPlan,
    width: u32,
    height: u32,
    candidates: &[bedrock_voxel::FlatBlockCandidate],
) -> Result<(RgbaImage, BTreeMap<String, usize>), String> {
    let colors = candidates
        .iter()
        .map(|candidate| (candidate.state.name.as_str(), candidate.top_color))
        .collect::<HashMap<_, _>>();
    let mut output = RgbaImage::new(width, height);
    let mut counts = BTreeMap::new();
    for block in plan.blocks() {
        let x = u32::try_from(block.offset.x).map_err(|error| error.to_string())?;
        let z = u32::try_from(block.offset.z).map_err(|error| error.to_string())?;
        let color = colors
            .get(block.state.name.as_str())
            .ok_or_else(|| format!("no preview color for {}", block.state.name))?;
        output.put_pixel(x, z, Rgba(*color));
        *counts.entry(block.state.name.clone()).or_insert(0) += 1;
    }
    Ok((output, counts))
}

fn print_color_metrics(
    source: &RgbaImage,
    output: &RgbaImage,
    block_count: &usize,
    side: u32,
    method: &str,
) {
    let mut absolute = [0_u64; 3];
    let mut squared = [0_u64; 3];
    let mut delta_e = 0.0;
    for (source, output) in source.pixels().zip(output.pixels()) {
        delta_e += oklab_delta_e(
            [source[0], source[1], source[2]],
            [output[0], output[1], output[2]],
        );
        for channel in 0..3 {
            let difference = i32::from(source[channel]) - i32::from(output[channel]);
            absolute[channel] += u64::from(difference.unsigned_abs());
            squared[channel] += u64::try_from(difference * difference).unwrap_or_default();
        }
    }
    let pixel_count = f64::from(side * side);
    let mae = absolute.map(|sum| sum as f64 / pixel_count);
    let rmse = squared.map(|sum| (sum as f64 / pixel_count).sqrt());
    println!(
        "size={side} method={method} blocks={block_count} RGB_MAE={:.2}/{:.2}/{:.2} RGB_RMSE={:.2}/{:.2}/{:.2} OKLab_Euclidean_deltaE_x100={:.2} (not CIEDE2000)",
        mae[0],
        mae[1],
        mae[2],
        rmse[0],
        rmse[1],
        rmse[2],
        delta_e / pixel_count
    );
    for region in [4, 8] {
        print_region_average_metrics(source, output, region);
    }
}

fn print_region_average_metrics(source: &RgbaImage, output: &RgbaImage, region: u32) {
    let mut absolute = [0.0; 3];
    let mut delta_e = 0.0;
    let mut regions = 0_u64;
    for top in (0..source.height()).step_by(region as usize) {
        for left in (0..source.width()).step_by(region as usize) {
            let mut source_sum = [0_u64; 3];
            let mut output_sum = [0_u64; 3];
            let mut pixels = 0_u64;
            for y in top..(top + region).min(source.height()) {
                for x in left..(left + region).min(source.width()) {
                    let source_pixel = source.get_pixel(x, y);
                    let output_pixel = output.get_pixel(x, y);
                    for channel in 0..3 {
                        source_sum[channel] += u64::from(source_pixel[channel]);
                        output_sum[channel] += u64::from(output_pixel[channel]);
                    }
                    pixels += 1;
                }
            }
            let source_mean = [0, 1, 2].map(|channel| source_sum[channel] as f64 / pixels as f64);
            let output_mean = [0, 1, 2].map(|channel| output_sum[channel] as f64 / pixels as f64);
            for channel in 0..3 {
                absolute[channel] += (source_mean[channel] - output_mean[channel]).abs();
            }
            delta_e += oklab_delta_e_f64(source_mean, output_mean);
            regions += 1;
        }
    }
    println!(
        "region={region}x{region} count={regions} mean_RGB_region_MAE={:.2}/{:.2}/{:.2} mean_OKLab_Euclidean_deltaE_x100={:.2}",
        absolute[0] / regions as f64,
        absolute[1] / regions as f64,
        absolute[2] / regions as f64,
        delta_e / regions as f64
    );
}

pub(super) fn oklab_delta_e(first: [u8; 3], second: [u8; 3]) -> f64 {
    oklab_delta_e_f64(first.map(f64::from), second.map(f64::from))
}

fn oklab_delta_e_f64(first: [f64; 3], second: [f64; 3]) -> f64 {
    let first = oklab_f64(first);
    let second = oklab_f64(second);
    100.0
        * ((first[0] - second[0]).powi(2)
            + (first[1] - second[1]).powi(2)
            + (first[2] - second[2]).powi(2))
        .sqrt()
}

fn oklab_f64(color: [f64; 3]) -> [f64; 3] {
    let linear = color.map(|channel| {
        let channel = channel / 255.0;
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    });
    let l = (0.4122214708 * linear[0] + 0.5363325363 * linear[1] + 0.0514459929 * linear[2]).cbrt();
    let m = (0.2119034982 * linear[0] + 0.6806995451 * linear[1] + 0.1073969566 * linear[2]).cbrt();
    let s = (0.0883024619 * linear[0] + 0.2817188376 * linear[1] + 0.6299787005 * linear[2]).cbrt();
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}

pub(super) fn print_block_distribution(counts: &BTreeMap<String, usize>, limit: usize) {
    let mut sorted = counts.iter().collect::<Vec<_>>();
    sorted.sort_by(|(name_a, count_a), (name_b, count_b)| {
        count_b.cmp(count_a).then_with(|| name_a.cmp(name_b))
    });
    println!(
        "used_palette_entries={} top_blocks={:?}",
        counts.len(),
        &sorted[..sorted.len().min(limit)]
    );
}

pub(super) fn save_png(image: &RgbaImage, path: &Path) -> Result<(), String> {
    image
        .save_with_format(path, ::image::ImageFormat::Png)
        .map_err(|error| error.to_string())
}

pub(super) fn srgb_linear(channel: u8) -> f32 {
    let channel = f32::from(channel) / 255.0;
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}

pub(super) fn linear_srgb(channel: f32) -> u8 {
    let channel = channel.clamp(0.0, 1.0);
    let encoded = if channel <= 0.0031308 {
        channel * 12.92
    } else {
        1.055 * channel.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round() as u8
}
