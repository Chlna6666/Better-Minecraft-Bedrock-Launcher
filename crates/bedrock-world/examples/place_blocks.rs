//! Diagnostic: write block states into a (copied) world so a headless render can inspect them.
//!
//! Usage: `place_blocks <world-dir> <y> <x> <name> [key=value ...] -- [<name> ...]`
//!
//! Blocks are laid out two apart along X at the given Y. Point the copy at the world chosen
//! with `BMCBL_WORLD3D_TEST_WORLD` and read the result with the ignored
//! `world3d::offscreen_check::render_real_world_snapshot` test.

use std::path::Path;

use bedrock_world::block::BlockState;
use bedrock_world::editor::{BlockEdit, BlockEditOptions, apply_block_edits};
use bedrock_world::world::{OpenOptions, World};
use bedrock_world::{BlockPos, Dimension, NbtTag, WriteGuard};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(world_path), Some(y), Some(x)) = (args.next(), args.next(), args.next()) else {
        std::process::exit(2);
    };
    let y: i32 = y.parse().expect("y");
    let mut x: i32 = x.parse().expect("x");
    let mut specs: Vec<Vec<String>> = Vec::new();
    let mut spec: Vec<String> = Vec::new();
    for arg in args {
        if arg == "--" {
            if !spec.is_empty() {
                specs.push(std::mem::take(&mut spec));
            }
        } else {
            spec.push(arg);
        }
    }
    if !spec.is_empty() {
        specs.push(spec);
    }
    let mut options = OpenOptions::default();
    options.read_only = false;
    let world = World::open(&world_path, options).expect("open world");
    let guard = WriteGuard::confirmed(Path::new(&world_path), "place diagnostic blocks");
    let mut edits = Vec::new();
    for spec in specs {
        let Some(name) = spec.first() else { continue };
        let mut states = std::collections::BTreeMap::new();
        for pair in &spec[1..] {
            let Some((key, raw)) = pair.split_once('=') else {
                continue;
            };
            let value = if let Ok(number) = raw.parse::<i32>() {
                NbtTag::Int(number)
            } else {
                NbtTag::String(raw.to_owned())
            };
            states.insert(key.to_owned(), value);
        }
        edits.push(BlockEdit::new(
            Dimension::Overworld,
            BlockPos { x, y, z: 0 },
            BlockState {
                name: name.clone(),
                states,
                version: Some(1),
            },
        ));
        println!("placed {name} at ({x},{y},0)");
        x += 2;
    }
    let result = apply_block_edits(&world, &edits, &guard, BlockEditOptions::default())
        .expect("apply edits");
    println!("edited {} blocks", result.edited_blocks);
}
