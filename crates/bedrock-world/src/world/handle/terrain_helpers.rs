//! Internal Minecraft terrain coordinates, height, and legacy block mapping helpers.

use super::*;

pub(super) fn validate_local_column(local_x: u8, local_z: u8) -> Result<()> {
    if local_x >= 16 || local_z >= 16 {
        return Err(BedrockWorldError::Validation(format!(
            "local biome coordinates must be 0..15, got x={local_x}, z={local_z}"
        )));
    }
    Ok(())
}

pub(super) fn insert_needed_surface_subchunks(
    subchunk_ys: &mut BTreeSet<i8>,
    height_map: Option<&[[Option<i16>; 16]; 16]>,
    min_subchunk_y: i8,
    max_subchunk_y: i8,
) {
    const SURFACE_LOOKDOWN_SUBCHUNKS: i8 = 6;
    const SURFACE_LOOKUP_SUBCHUNKS: i8 = 4;
    let Some(height_map) = height_map else {
        return;
    };
    for row in height_map {
        for height in row.iter().flatten() {
            if let Ok(surface_y) = block_y_to_subchunk_y(i32::from(*height)) {
                let lower_y = surface_y
                    .saturating_sub(SURFACE_LOOKDOWN_SUBCHUNKS)
                    .max(min_subchunk_y);
                let upper_y = surface_y
                    .saturating_add(SURFACE_LOOKUP_SUBCHUNKS)
                    .clamp(min_subchunk_y, max_subchunk_y);
                for subchunk_y in lower_y..=upper_y {
                    subchunk_ys.insert(subchunk_y);
                }
            }
        }
    }
}

pub(super) fn block_y_to_subchunk_y(y: i32) -> Result<i8> {
    let subchunk_y = y.div_euclid(16);
    i8::try_from(subchunk_y).map_err(|_| {
        BedrockWorldError::Validation(format!(
            "block y={y} cannot be represented as a Bedrock subchunk index"
        ))
    })
}

pub(super) fn biome_storage_contains_y(storage: &ParsedBiomeStorage, y: i32) -> bool {
    storage
        .y
        .is_none_or(|start_y| (start_y..start_y + 16).contains(&y))
}

pub(super) fn biome_storage_bucket_y(y: i32) -> i32 {
    y.div_euclid(16) * 16
}

pub(super) fn biome_id_from_storage(
    storage: &ParsedBiomeStorage,
    local_x: u8,
    local_z: u8,
    y: i32,
) -> Option<u32> {
    let local_y = if let Some(start_y) = storage.y {
        u8::try_from(y - start_y).ok()?
    } else {
        0
    };
    storage.biome_id_at(local_x, local_y, local_z)
}

pub(super) fn height_map_index(local_x: u8, local_z: u8) -> usize {
    usize::from(local_z) * 16 + usize::from(local_x)
}

pub(super) fn column_index(local_x: u8, local_z: u8) -> Option<usize> {
    (local_x < 16 && local_z < 16).then_some(height_map_index(local_x, local_z))
}

pub(super) fn raw_height_at(
    height_map: Option<&[[Option<i16>; 16]; 16]>,
    local_x: u8,
    local_z: u8,
) -> Option<i16> {
    height_map?[usize::from(local_z)][usize::from(local_x)]
}

pub(super) fn raw_height_mismatch_columns(chunk: &ChunkData) -> usize {
    let Some(samples) = chunk.column_samples.as_ref() else {
        return 0;
    };
    let Some(height_map) = chunk.height_map.as_ref() else {
        return 0;
    };
    let mut mismatches = 0usize;
    for local_z in 0..16_u8 {
        for local_x in 0..16_u8 {
            if let Some(sample) = samples.get(local_x, local_z) {
                if height_map[usize::from(local_z)][usize::from(local_x)]
                    .is_some_and(|raw_height| raw_height != sample.surface_y)
                {
                    mismatches = mismatches.saturating_add(1);
                }
            }
        }
    }
    mismatches
}

pub(super) fn missing_surface_columns(chunk: &ChunkData) -> usize {
    chunk.column_samples.as_ref().map_or(0, |samples| {
        256usize.saturating_sub(samples.sampled_columns())
    })
}

pub(super) fn needed_exact_surface_chunk_requires_full_reload(chunk: &ChunkData) -> Result<bool> {
    let Some(samples) = chunk.column_samples.as_ref() else {
        return Ok(false);
    };
    if samples.sampled_columns() < 16 * 16 {
        return Ok(true);
    }
    if raw_height_mismatch_columns(chunk) > 0 {
        return Ok(true);
    }
    let Some(loaded_max_subchunk_y) = chunk.subchunks.keys().next_back().copied() else {
        return Ok(true);
    };
    let (_, world_max_subchunk_y) = chunk.pos.subchunk_index_range(chunk.version);
    if loaded_max_subchunk_y >= world_max_subchunk_y {
        return Ok(false);
    }
    for sample in samples.iter() {
        if block_y_to_subchunk_y(i32::from(sample.surface_y))? == loaded_max_subchunk_y {
            return Ok(true);
        }
        if let Some(overlay) = sample.overlay.as_ref() {
            if block_y_to_subchunk_y(i32::from(overlay.y))? == loaded_max_subchunk_y {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub(super) fn legacy_world_block_state(id: u8, data: u8) -> BlockState {
    let mut states = BTreeMap::new();
    states.insert("data".to_string(), NbtTag::Byte(data as i8));
    BlockState {
        name: legacy_world_block_name(id, data),
        states,
        version: None,
    }
}

#[allow(clippy::too_many_lines)]
pub(super) fn legacy_world_block_name(id: u8, data: u8) -> String {
    let name = match id {
        0 => "minecraft:air",
        1 => match data & 0x7 {
            1 => "minecraft:granite",
            2 => "minecraft:polished_granite",
            3 => "minecraft:diorite",
            4 => "minecraft:polished_diorite",
            5 => "minecraft:andesite",
            6 => "minecraft:polished_andesite",
            _ => "minecraft:stone",
        },
        2 => "minecraft:grass_block",
        3 => match data & 0x3 {
            1 => "minecraft:coarse_dirt",
            2 => "minecraft:podzol",
            _ => "minecraft:dirt",
        },
        4 => "minecraft:cobblestone",
        5 => legacy_world_wood_name(data, "planks"),
        6 => "minecraft:oak_sapling",
        7 => "minecraft:bedrock",
        8 | 9 => "minecraft:water",
        10 | 11 => "minecraft:lava",
        12 => match data & 0x1 {
            1 => "minecraft:red_sand",
            _ => "minecraft:sand",
        },
        13 => "minecraft:gravel",
        14 => "minecraft:gold_ore",
        15 => "minecraft:iron_ore",
        16 => "minecraft:coal_ore",
        17 => legacy_world_wood_name(data, "log"),
        18 => legacy_world_wood_name(data, "leaves"),
        19 => "minecraft:sponge",
        20 => "minecraft:glass",
        21 => "minecraft:lapis_ore",
        22 => "minecraft:lapis_block",
        24 => "minecraft:sandstone",
        26 => "minecraft:bed",
        30 => "minecraft:cobweb",
        31 => match data {
            1 => "minecraft:short_grass",
            2 => "minecraft:fern",
            _ => "minecraft:dead_bush",
        },
        32 => "minecraft:dead_bush",
        35 => legacy_world_wool_name(data),
        37 => "minecraft:dandelion",
        38 => "minecraft:poppy",
        39 => "minecraft:brown_mushroom",
        40 => "minecraft:red_mushroom",
        41 => "minecraft:gold_block",
        42 => "minecraft:iron_block",
        43 | 44 => "minecraft:stone_slab",
        45 => "minecraft:bricks",
        46 => "minecraft:tnt",
        47 => "minecraft:bookshelf",
        48 => "minecraft:mossy_cobblestone",
        49 => "minecraft:obsidian",
        50 => "minecraft:torch",
        51 => "minecraft:fire",
        52 => "minecraft:spawner",
        53 => "minecraft:oak_stairs",
        54 => "minecraft:chest",
        56 => "minecraft:diamond_ore",
        57 => "minecraft:diamond_block",
        58 => "minecraft:crafting_table",
        59 => "minecraft:wheat",
        60 => "minecraft:farmland",
        61 | 62 => "minecraft:furnace",
        63 | 68 => "minecraft:oak_sign",
        64 => "minecraft:oak_door",
        65 => "minecraft:ladder",
        66 => "minecraft:rail",
        67 => "minecraft:cobblestone_stairs",
        71 => "minecraft:iron_door",
        73 | 74 => "minecraft:redstone_ore",
        78 => "minecraft:snow",
        79 => "minecraft:ice",
        80 => "minecraft:snow_block",
        81 => "minecraft:cactus",
        82 => "minecraft:clay",
        83 => "minecraft:sugar_cane",
        85 => "minecraft:oak_fence",
        86 => "minecraft:pumpkin",
        87 => "minecraft:netherrack",
        88 => "minecraft:soul_sand",
        89 => "minecraft:glowstone",
        91 => "minecraft:jack_o_lantern",
        95 => "minecraft:invisible_bedrock",
        98 => "minecraft:stone_bricks",
        99 | 100 => "minecraft:mushroom_stem",
        103 => "minecraft:melon",
        106 => "minecraft:vine",
        107 => "minecraft:oak_fence_gate",
        108 => "minecraft:brick_stairs",
        109 => "minecraft:stone_brick_stairs",
        110 => "minecraft:mycelium",
        111 => "minecraft:lily_pad",
        112 => "minecraft:nether_bricks",
        121 => "minecraft:end_stone",
        129 => "minecraft:emerald_ore",
        133 => "minecraft:emerald_block",
        155 => "minecraft:quartz_block",
        159 | 172 => "minecraft:terracotta",
        161 => legacy_world_wood_name(data.saturating_add(4), "leaves"),
        162 => legacy_world_wood_name(data.saturating_add(4), "log"),
        169 => "minecraft:sea_lantern",
        170 => "minecraft:hay_block",
        171 => "minecraft:white_carpet",
        173 => "minecraft:coal_block",
        174 => "minecraft:packed_ice",
        175 => "minecraft:sunflower",
        _ => return format!("legacy:{id}"),
    };
    name.to_string()
}

pub(super) fn legacy_world_wood_name(data: u8, suffix: &'static str) -> &'static str {
    match (data & 0x7, suffix) {
        (1, "planks") => "minecraft:spruce_planks",
        (2, "planks") => "minecraft:birch_planks",
        (3, "planks") => "minecraft:jungle_planks",
        (4, "planks") => "minecraft:acacia_planks",
        (5, "planks") => "minecraft:dark_oak_planks",
        (_, "planks") => "minecraft:oak_planks",
        (1, "log") => "minecraft:spruce_log",
        (2, "log") => "minecraft:birch_log",
        (3, "log") => "minecraft:jungle_log",
        (4, "log") => "minecraft:acacia_log",
        (5, "log") => "minecraft:dark_oak_log",
        (_, "log") => "minecraft:oak_log",
        (1, "leaves") => "minecraft:spruce_leaves",
        (2, "leaves") => "minecraft:birch_leaves",
        (3, "leaves") => "minecraft:jungle_leaves",
        (4, "leaves") => "minecraft:acacia_leaves",
        (5, "leaves") => "minecraft:dark_oak_leaves",
        _ => "minecraft:oak_leaves",
    }
}

pub(super) fn legacy_world_wool_name(data: u8) -> &'static str {
    match data & 0x0f {
        1 => "minecraft:orange_wool",
        2 => "minecraft:magenta_wool",
        3 => "minecraft:light_blue_wool",
        4 => "minecraft:yellow_wool",
        5 => "minecraft:lime_wool",
        6 => "minecraft:pink_wool",
        7 => "minecraft:gray_wool",
        8 => "minecraft:light_gray_wool",
        9 => "minecraft:cyan_wool",
        10 => "minecraft:purple_wool",
        11 => "minecraft:blue_wool",
        12 => "minecraft:brown_wool",
        13 => "minecraft:green_wool",
        14 => "minecraft:red_wool",
        15 => "minecraft:black_wool",
        _ => "minecraft:white_wool",
    }
}
