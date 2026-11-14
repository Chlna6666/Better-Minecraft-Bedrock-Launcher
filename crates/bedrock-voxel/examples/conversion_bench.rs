//! Temporary CPU baseline for the three conversion paths.
use std::time::Instant;

use bedrock_voxel::{
    Dithering, FlatImageOptions, ObjVoxelOptions, VoxelizeTexturesRequest,
    default_block_candidates, load_obj_model, load_obj_textures, plan_flat_image,
    voxelize_textured,
};
use bedrock_world::surface::CancelFlag;

fn main() {
    let mut args = std::env::args().skip(1);
    let obj_path = args.next().expect("usage: conversion_bench <obj> <image>");
    let image_path = args.next().expect("usage: conversion_bench <obj> <image>");

    let model = load_obj_model(std::path::Path::new(&obj_path)).unwrap();
    let decode_started = Instant::now();
    let textures = load_obj_textures(&model).unwrap();
    println!(
        "texture decode files={} elapsed={:?}",
        textures.iter().filter(|t| t.is_some()).count(),
        decode_started.elapsed()
    );
    let candidates = default_block_candidates();
    for side in [128_u16, 256] {
        let started = Instant::now();
        let plan = voxelize_textured(
            VoxelizeTexturesRequest {
                model: &model,
                textures: &textures,
                candidates,
                options: ObjVoxelOptions {
                    longest_side_blocks: side,
                    ..Default::default()
                },
                cancel: &CancelFlag::new(),
            },
            |_, _| {},
        )
        .unwrap();
        println!(
            "obj side={side} blocks={} elapsed={:?}",
            plan.blocks().len(),
            started.elapsed()
        );
    }

    let source = image::open(&image_path).unwrap().to_rgba8();
    let (src_w, src_h) = source.dimensions();
    for side in [512_u32, 1024] {
        let height = ((u64::from(side) * u64::from(src_h)) / u64::from(src_w)).max(1) as u32;
        let resized =
            image::imageops::resize(&source, side, height, image::imageops::FilterType::Lanczos3);
        let started = Instant::now();
        let plan = plan_flat_image(
            resized.as_raw(),
            resized.width(),
            resized.height(),
            candidates,
            FlatImageOptions {
                dithering: Dithering::None,
                alpha_threshold: 1,
                background: [255; 3],
                transparent_fill: None,
            },
        )
        .unwrap();
        println!(
            "image side={side} blocks={} elapsed={:?}",
            plan.blocks().len(),
            started.elapsed()
        );
    }
}
