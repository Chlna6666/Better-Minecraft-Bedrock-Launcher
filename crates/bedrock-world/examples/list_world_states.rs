//! Enumerate every distinct block state stored in a world save.
//!
//! Usage: cargo run -p bedrock-world --example list_world_states -- <world-dir> [overworld|nether|end]

use bedrock_world::surface::{
    BiomeDataRequirement, ChunkDataRequest, ChunkLoadOptions, ChunkLoadPriority,
    WorldPipelineOptions, WorldScanOptions, WorldThreadingOptions,
};
use bedrock_world::world::{OpenOptions, World};
use bedrock_world::{Dimension, SubChunkDecodeMode};
use std::collections::BTreeMap;

fn load_options() -> ChunkLoadOptions {
    ChunkLoadOptions {
        data_request: ChunkDataRequest::new()
            .full_3d_indices()
            .biome(BiomeDataRequirement::None),
        subchunk_decode: SubChunkDecodeMode::FullIndices,
        threading: WorldThreadingOptions::Single,
        pipeline: WorldPipelineOptions::default(),
        cancel: None,
        progress: None,
        priority: ChunkLoadPriority::DistanceFrom {
            chunk_x: 0,
            chunk_z: 0,
        },
        storage_cache_policy: Default::default(),
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(world_path) = args.next() else {
        eprintln!("usage: list_world_states <world-dir> [overworld|nether|end]");
        std::process::exit(2);
    };
    let dimension = match args.next().as_deref() {
        Some("nether") => Dimension::Nether,
        Some("end") => Dimension::End,
        _ => Dimension::Overworld,
    };
    let mut options = OpenOptions::default();
    options.read_only = true;
    let world = World::open(&world_path, options).expect("open world");
    let positions = world
        .chunk_positions(WorldScanOptions::default())
        .expect("list chunks");
    eprintln!("chunks: {}", positions.len());
    let mut states: BTreeMap<(String, String), (usize, [i32; 3])> = BTreeMap::new();
    let mut loaded = 0usize;
    for pos in positions {
        let Ok(data) = world.query_chunk_data(pos, load_options()) else {
            continue;
        };
        if !data.is_loaded {
            continue;
        }
        loaded += 1;
        for (sub_y, subchunk) in &data.subchunks {
            for y in 0..16i32 {
                for z in 0..16i32 {
                    for x in 0..16i32 {
                        if let Some(state) = subchunk.block_state_at(x as u8, y as u8, z as u8) {
                            let pos = [pos.x * 16 + x, i32::from(*sub_y) * 16 + y, pos.z * 16 + z];
                            let entry = states
                                .entry((state.name.clone(), format!("{:?}", state.states)))
                                .or_insert((0, pos));
                            entry.0 += 1;
                        }
                    }
                }
            }
        }
    }
    eprintln!("loaded chunks: {loaded}");
    for ((name, state), (count, pos)) in &states {
        println!(
            "{count}\t{0}\t{1}\t{2}\t{name}\t{state}",
            pos[0], pos[1], pos[2]
        );
    }
    eprintln!("distinct states: {}", states.len());
}
