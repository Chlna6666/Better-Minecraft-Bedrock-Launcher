//! Query authoritative block states at specific world positions.
//!
//! Usage:
//!   cargo run -p bedrock-world --example query_block -- <world-dir> [x y z]...

use bedrock_world::chunk::{BlockPos, Dimension};
use bedrock_world::world::{OpenOptions, World};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(world_path) = args.next() else {
        eprintln!("usage: query_block <world-dir> [x y z]...");
        std::process::exit(2);
    };
    let mut positions = Vec::new();
    let mut file_args = None;
    let mut rest = Vec::new();
    while let Some(arg) = args.next() {
        if arg == "--file" {
            file_args = args.next();
        } else {
            rest.push(arg);
        }
    }
    if let Some(path) = file_args {
        let text = std::fs::read_to_string(&path).expect("read positions file");
        let nums: Vec<i64> = text
            .split_whitespace()
            .filter_map(|token| token.parse().ok())
            .collect();
        for chunk in nums.chunks_exact(3) {
            positions.push(BlockPos {
                x: chunk[0] as i32,
                y: chunk[1] as i32,
                z: chunk[2] as i32,
            });
        }
        rest.into_iter();
    } else {
        let mut it = rest.into_iter();
        while let (Some(x), Some(y), Some(z)) = (it.next(), it.next(), it.next()) {
            positions.push(BlockPos {
                x: x.parse().expect("x"),
                y: y.parse().expect("y"),
                z: z.parse().expect("z"),
            });
        }
    }
    if positions.is_empty() {
        positions.push(BlockPos { x: 0, y: 64, z: 0 });
    }
    let mut options = OpenOptions::default();
    options.read_only = true;
    let world = World::open(&world_path, options).expect("open world");
    for dimension in [Dimension::Overworld, Dimension::Nether, Dimension::End] {
        match world.block_states(dimension, positions.iter().copied()) {
            Ok(results) => {
                for result in results {
                    match result.state {
                        Some(state) => {
                            println!(
                                "{dimension:?} ({}, {}, {}) = {} states={:?}",
                                result.pos.x, result.pos.y, result.pos.z, state.name, state.states
                            );
                        }
                        None => println!(
                            "{dimension:?} ({}, {}, {}) = <empty>",
                            result.pos.x, result.pos.y, result.pos.z
                        ),
                    }
                }
            }
            Err(error) => eprintln!("query failed for {dimension:?}: {error}"),
        }
    }
}
