//! Build a compact capture world: one instance of every block state from a source world,
//! laid out on a grid so a single world load can render them all.
//!
//! Usage: cargo run -p bedrock-world --example make_capture_world -- <source-world> <dest-world>

use bedrock_world::block::BlockState;
use bedrock_world::editor::{BlockEdit, BlockEditOptions, apply_block_edits};
use bedrock_world::surface::{
    BiomeDataRequirement, ChunkDataRequest, ChunkLoadOptions, ChunkLoadPriority,
    WorldPipelineOptions, WorldScanOptions, WorldThreadingOptions,
};
use bedrock_world::world::{OpenOptions, World};
use bedrock_world::{BlockPos, Dimension, NbtTag, WriteGuard};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn load_options() -> ChunkLoadOptions {
    ChunkLoadOptions {
        data_request: ChunkDataRequest::new()
            .full_3d_indices()
            .biome(BiomeDataRequirement::None),
        subchunk_decode: bedrock_world::SubChunkDecodeMode::FullIndices,
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

fn copy_dir(source: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(source)? {
        let entry = entry?;
        let target = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn state_int(state: &BlockState, key: &str) -> Option<i32> {
    match state.states.get(key) {
        Some(NbtTag::Byte(value)) => Some(i32::from(*value)),
        Some(NbtTag::Short(value)) => Some(i32::from(*value)),
        Some(NbtTag::Int(value)) => Some(*value),
        _ => None,
    }
}

fn toggle_bit(state: &BlockState, key: &str) -> Option<BlockState> {
    let mut paired = state.clone();
    let value = state_int(state, key)?;
    let toggled = if value == 0 { 1 } else { 0 };
    paired.states.insert(key.to_string(), NbtTag::Byte(toggled));
    Some(paired)
}

/// Returns the offset of the companion block and the companion state.
fn companion(state: &BlockState) -> Option<([i32; 3], BlockState)> {
    if let Some(upper) = state_int(state, "upper_block_bit") {
        if upper == 0 {
            return Some(([0, 1, 0], toggle_bit(state, "upper_block_bit")?));
        }
        return Some(([0, -1, 0], toggle_bit(state, "upper_block_bit")?));
    }
    if let Some(head) = state_int(state, "head_piece_bit") {
        let direction = state_int(state, "direction")?;
        let vector = match direction {
            0 => [0, 0, 1],
            1 => [-1, 0, 0],
            2 => [0, 0, -1],
            3 => [1, 0, 0],
            _ => return None,
        };
        let offset = if head == 0 {
            vector
        } else {
            [-vector[0], -vector[1], -vector[2]]
        };
        return Some((offset, toggle_bit(state, "head_piece_bit")?));
    }
    None
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(source) = args.next() else {
        eprintln!("usage: make_capture_world <source-world> <dest-world>");
        std::process::exit(2);
    };
    let Some(dest) = args.next() else {
        eprintln!("usage: make_capture_world <source-world> <dest-world>");
        std::process::exit(2);
    };
    let mut options = OpenOptions::default();
    options.read_only = true;
    let world = World::open(&source, options).expect("open source world");
    let positions = world
        .chunk_positions(WorldScanOptions::default())
        .expect("list chunks");
    let mut states: BTreeMap<String, BlockState> = BTreeMap::new();
    for pos in &positions {
        let Ok(data) = world.query_chunk_data(*pos, load_options()) else {
            continue;
        };
        if !data.is_loaded {
            continue;
        }
        for subchunk in data.subchunks.values() {
            for y in 0..16u8 {
                for z in 0..16u8 {
                    for x in 0..16u8 {
                        if let Some(state) = subchunk.block_state_at(x, y, z) {
                            if state.name == "minecraft:air" || state.name.starts_with('<') {
                                continue;
                            }
                            let key = format!("{}\t{:?}", state.name, state.states);
                            states.entry(key).or_insert_with(|| state.clone());
                        }
                    }
                }
            }
        }
    }
    eprintln!("distinct states: {}", states.len());
    drop(world);

    let dest_path = PathBuf::from(&dest);
    if dest_path.exists() {
        std::fs::remove_dir_all(&dest_path).expect("remove stale dest");
    }
    copy_dir(Path::new(&source), &dest_path).expect("copy world");

    let mut options = OpenOptions::default();
    options.read_only = false;
    let world = World::open(&dest, options).expect("open dest world");
    let guard = WriteGuard::confirmed(&dest, "build compact state grid");

    const COLUMNS: usize = 104;
    const BASE_X: i32 = 100;
    const BASE_Z: i32 = 100;
    const BASE_Y: i32 = 100;
    let mut edits: Vec<BlockEdit> = Vec::new();
    // Blocks are left floating with air underneath so the renderer does not cull bottom faces
    // against a support platform.
    let mut mapping = Vec::new();
    for (index, state) in states.values().enumerate() {
        if index >= COLUMNS * COLUMNS {
            break;
        }
        let x = BASE_X + (index % COLUMNS) as i32 * 2;
        let z = BASE_Z + (index / COLUMNS) as i32 * 2;
        let mut state = state.clone();
        if state.version.is_none() {
            state.version = Some(1);
        }
        edits.push(BlockEdit::new(
            Dimension::Overworld,
            BlockPos { x, y: BASE_Y, z },
            state.clone(),
        ));
        mapping.push((
            state.name.clone(),
            format!("{:?}", state.states),
            x,
            BASE_Y,
            z,
        ));
    }
    let mut used: std::collections::BTreeSet<(i32, i32, i32)> = (0..COLUMNS * COLUMNS)
        .map(|index| {
            (
                BASE_X + (index % COLUMNS) as i32 * 2,
                BASE_Y,
                BASE_Z + (index / COLUMNS) as i32 * 2,
            )
        })
        .collect();
    for (index, state) in states.values().enumerate() {
        if index >= COLUMNS * COLUMNS {
            break;
        }
        let Some((offset, companion)) = companion(state) else {
            continue;
        };
        let position = BlockPos {
            x: BASE_X + (index % COLUMNS) as i32 * 2 + offset[0],
            y: BASE_Y + offset[1],
            z: BASE_Z + (index / COLUMNS) as i32 * 2 + offset[2],
        };
        let key = (position.x, position.y, position.z);
        if !used.insert(key) {
            continue;
        }
        edits.push(BlockEdit::new(
            Dimension::Overworld,
            position,
            companion.clone(),
        ));
        mapping.push((
            companion.name.clone(),
            format!("{:?}", companion.states),
            position.x,
            position.y,
            position.z,
        ));
    }
    eprintln!("edits: {}", edits.len());
    for batch in edits.chunks(4000) {
        apply_block_edits(&world, batch, &guard, BlockEditOptions::default()).expect("apply edits");
    }
    let json = serde_json::json!({
        "base": [BASE_X, BASE_Y, BASE_Z],
        "columns": COLUMNS,
        "entries": mapping.iter().map(|(name, states, x, y, z)| serde_json::json!({
            "name": name, "states": states, "pos": [x, y, z]
        })).collect::<Vec<_>>()
    });
    std::fs::write(
        "target/capture_world_map.json",
        serde_json::to_string(&json).unwrap(),
    )
    .expect("write mapping");
    eprintln!("mapping written: {} entries", mapping.len());
}
