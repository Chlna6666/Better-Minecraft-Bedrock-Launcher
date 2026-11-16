//! Diagnostic: write standing banners with Banner block entities into a copied world.
//!
//! Usage: `place_banner <world-dir> <y> <x> <dye[:pattern:dye,...]> [<dye> ...]`
//!
//! Each banner is two blocks (`standing_banner` at `<y>` plus `banner` above it) laid two apart
//! along X. Dye and pattern colours accept indices (0..15) or dye names, and every position gets
//! the same Banner block-entity payload the game writes.

use std::collections::BTreeMap;
use std::path::Path;

use bedrock_world::block::BlockState;
use bedrock_world::editor::{
    BlockEdit, BlockEditOptions, BlockEntityEdit, apply_block_edits,
};
use bedrock_world::world::{OpenOptions, World};
use bedrock_world::{BlockPos, Dimension, NbtTag, WriteGuard};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(world_path), Some(y), Some(x)) = (args.next(), args.next(), args.next()) else {
        std::process::exit(2);
    };
    let y: i32 = y.parse().expect("y");
    let mut x: i32 = x.parse().expect("x");
    let specs = args.collect::<Vec<_>>();
    if specs.is_empty() {
        std::process::exit(2);
    }

    let mut options = OpenOptions::default();
    options.read_only = false;
    let world = World::open(&world_path, options).expect("open world");
    let guard = WriteGuard::confirmed(Path::new(&world_path), "place diagnostic banners");
    let mut edits = Vec::new();
    for spec in &specs {
        let Some((base, patterns)) = spec.split_once(':').or(Some((spec.as_str(), ""))) else {
            continue;
        };
        let base = dye_index(base);
        let mut states = BTreeMap::new();
        states.insert(
            "ground_sign_direction".to_owned(),
            NbtTag::Int(0),
        );
        let nbt = banner_nbt(x, y, base, patterns);
        for (offset, name) in [(1, "banner"), (0, "standing_banner")] {
            let mut edit = BlockEdit::new(
                Dimension::Overworld,
                BlockPos { x, y: y + offset, z: 0 },
                BlockState {
                    name: name.to_owned(),
                    states: states.clone(),
                    version: Some(1),
                },
            );
            edit.block_entity = BlockEntityEdit::Replace(nbt.clone());
            edits.push(edit);
        }
        println!("banner base={base} patterns={patterns:?} at ({x},{y},0)");
        x += 2;
    }
    let result = apply_block_edits(&world, &edits, &guard, BlockEditOptions::default())
        .expect("apply edits");
    println!("edited {} blocks", result.edited_blocks);
}

fn banner_nbt(x: i32, y: i32, base: u8, patterns: &str) -> NbtTag {
    let mut nbt = indexmap::IndexMap::new();
    nbt.insert("id".to_owned(), NbtTag::String("Banner".to_owned()));
    nbt.insert("x".to_owned(), NbtTag::Int(x));
    nbt.insert("y".to_owned(), NbtTag::Int(y));
    nbt.insert("z".to_owned(), NbtTag::Int(0));
    nbt.insert("Base".to_owned(), NbtTag::Int(i32::from(base)));
    let mut list = Vec::new();
    if !patterns.is_empty() {
        for layer in patterns.split(',') {
            let Some((pattern, color)) = layer.split_once(':') else {
                continue;
            };
            let mut entry = indexmap::IndexMap::new();
            entry.insert("Pattern".to_owned(), NbtTag::String(pattern.to_owned()));
            entry.insert(
                "Color".to_owned(),
                NbtTag::Int(i32::from(dye_index(color))),
            );
            list.push(NbtTag::Compound(entry));
        }
    }
    nbt.insert("Patterns".to_owned(), NbtTag::List(list));
    NbtTag::Compound(nbt)
}

fn dye_index(value: &str) -> u8 {
    const NAMES: [&str; 16] = [
        "white",
        "orange",
        "magenta",
        "light_blue",
        "yellow",
        "lime",
        "pink",
        "gray",
        "light_gray",
        "cyan",
        "purple",
        "blue",
        "brown",
        "green",
        "red",
        "black",
    ];
    let value = value.trim();
    value
        .parse::<u8>()
        .ok()
        .or_else(|| NAMES.iter().position(|name| *name == value).map(|index| index as u8))
        .unwrap_or(0)
}
