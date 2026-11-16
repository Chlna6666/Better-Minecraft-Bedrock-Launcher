//! Diagnostic: dump the BlockEntity NBT stored around one block position.
//!
//! Usage: `dump_block_entity <world-dir> <x> <y> <z> [radius-chunks]`

use std::path::Path;

use bedrock_world::chunk::{ChunkPos, Dimension};
use bedrock_world::world::{OpenOptions, World};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let Some(world_path) = args.next() else {
        std::process::exit(2);
    };
    let x: i32 = args.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let y: i32 = args.next().and_then(|v| v.parse().ok()).unwrap_or(64);
    let z: i32 = args.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let radius: i32 = args.next().and_then(|v| v.parse().ok()).unwrap_or(1);

    let mut options = OpenOptions::default();
    options.read_only = true;
    let world = World::open(&world_path, options)?;
    let (center_x, center_z) = (x.div_euclid(16), z.div_euclid(16));
    let mut positions = Vec::new();
    for dx in -radius..=radius {
        for dz in -radius..=radius {
            positions.push(ChunkPos {
                x: center_x + dx,
                z: center_z + dz,
                dimension: Dimension::Overworld,
            });
        }
    }
    let chunks = world.chunk_block_entities(positions, Default::default())?;
    let mut printed = 0;
    for chunk in &chunks {
        for entity in &chunk.entities {
            let Some(position) = entity.position else {
                continue;
            };
            if (position[0] - x).abs() > radius * 16 || (position[2] - z).abs() > radius * 16 {
                continue;
            }
            println!(
                "{} at {:?} id={:?}\n  {}",
                entity.id.as_deref().unwrap_or("<no id>"),
                position,
                entity.id,
                format_nbt(&entity.nbt, 1)
            );
            printed += 1;
        }
    }
    println!("{printed} block entities within {radius} chunk(s) of ({x}, {y}, {z})");
    let _ = Path::new(&world_path);
    Ok(())
}

fn format_nbt(tag: &bedrock_world::NbtTag, indent: usize) -> String {
    use bedrock_world::NbtTag;
    let pad = "  ".repeat(indent);
    match tag {
        NbtTag::Compound(map) => {
            let mut out = String::from("{\n");
            for (key, value) in map {
                out.push_str(&format!("{pad}  {key}: {}\n", format_nbt(value, indent + 1)));
            }
            out.push_str(&format!("{pad}}}"));
            out
        }
        NbtTag::List(values) => {
            let items = values
                .iter()
                .map(|value| format_nbt(value, indent + 1))
                .collect::<Vec<_>>()
                .join(", ");
            format!("[{items}]")
        }
        NbtTag::String(value) => format!("{value:?}"),
        other => format!("{other:?}"),
    }
}
