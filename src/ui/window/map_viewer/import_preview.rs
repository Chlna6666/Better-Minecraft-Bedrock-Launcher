use super::model::{CopiedChunkData, CopiedChunkPreviewImage};
use super::prelude::*;

pub(super) fn copied_chunk_preview_images_for_import(
    copied_chunk: &CopiedChunkData,
) -> Result<BTreeMap<ChunkPos, CopiedChunkPreviewImage>, String> {
    copied_chunk_preview_images_from_records(copied_chunk)
}

pub(super) fn copied_chunk_preview_images_from_records(
    copied_chunk: &CopiedChunkData,
) -> Result<BTreeMap<ChunkPos, CopiedChunkPreviewImage>, String> {
    let mut images = BTreeMap::new();
    for snapshot in &copied_chunk.chunks {
        if let Some(image) = copied_chunk_preview_image_from_snapshot(snapshot)? {
            images.insert(snapshot.chunk, image);
        }
    }
    Ok(images)
}

pub(super) fn mcstructure_preview_images(
    structure: &bedrock_world::McStructureFile,
    anchor_chunk: ChunkPos,
) -> Result<BTreeMap<ChunkPos, CopiedChunkPreviewImage>, String> {
    let mut columns: BTreeMap<ChunkPos, Vec<Option<ImportPreviewColumn>>> = BTreeMap::new();
    let origin_x = anchor_chunk.x.saturating_mul(16);
    let origin_z = anchor_chunk.z.saturating_mul(16);
    for block in structure
        .blocks()
        .map_err(|error| format!("结构预览索引无效：{error}"))?
    {
        let primary = structure_palette_preview_state(structure, block.primary);
        let secondary = structure_palette_preview_state(structure, block.secondary);
        let Some(state) = import_preview_visible_state(primary, secondary) else {
            continue;
        };
        let world_x = origin_x.saturating_add(block.x);
        let world_z = origin_z.saturating_add(block.z);
        let chunk = ChunkPos {
            x: world_x.div_euclid(16),
            z: world_z.div_euclid(16),
            dimension: anchor_chunk.dimension,
        };
        let local_x = usize::try_from(world_x.rem_euclid(16))
            .map_err(|_| format!("结构预览 X 坐标越界：{world_x}"))?;
        let local_z = usize::try_from(world_z.rem_euclid(16))
            .map_err(|_| format!("结构预览 Z 坐标越界：{world_z}"))?;
        update_preview_column(&mut columns, chunk, local_x, local_z, block.y, state);
    }
    preview_images_from_columns(columns)
}

fn copied_chunk_preview_image_from_snapshot(
    snapshot: &super::model::CopiedChunkSnapshot,
) -> Result<Option<CopiedChunkPreviewImage>, String> {
    let parsed = bedrock_world::parsed::parse_chunk_records_ref_with_options(
        snapshot.chunk,
        &snapshot.records,
        import_preview_parse_options(),
    );
    let mut columns = vec![None; 16 * 16];
    for record in parsed.records {
        match record.value {
            bedrock_world::ParsedChunkRecordValue::SubChunk(subchunk) => {
                sample_subchunk_columns(&subchunk, &mut columns);
            }
            bedrock_world::ParsedChunkRecordValue::LegacyTerrain(terrain) => {
                sample_legacy_terrain_columns(&terrain, &mut columns);
            }
            _ => {}
        }
    }
    let Some(pixels) = preview_pixels_from_chunk_columns(&columns) else {
        return Ok(None);
    };
    Ok(Some(CopiedChunkPreviewImage {
        chunk: snapshot.chunk,
        image: render_image_from_preview_pixels(pixels)?,
        width: 16,
        height: 16,
    }))
}

fn sample_subchunk_columns(
    subchunk: &bedrock_world::SubChunk,
    columns: &mut [Option<ImportPreviewColumn>],
) {
    let base_y = i32::from(subchunk.y) * 16;
    for local_y in 0u8..16 {
        let y = base_y.saturating_add(i32::from(local_y));
        for local_z in 0u8..16 {
            for local_x in 0u8..16 {
                let primary = subchunk
                    .block_state_at(local_x, local_y, local_z)
                    .map(block_state_preview_state);
                let secondary = subchunk
                    .visible_block_states_at(local_x, local_y, local_z)
                    .find(|state| {
                        matches!(
                            import_preview_block_class(&state.name),
                            ImportPreviewBlockClass::Water | ImportPreviewBlockClass::Lava
                        )
                    })
                    .map(block_state_preview_state);
                if let Some(state) = import_preview_visible_state(primary, secondary) {
                    update_column_slice(
                        columns,
                        usize::from(local_x),
                        usize::from(local_z),
                        y,
                        state,
                    );
                } else if let Some(id) = subchunk.legacy_block_id_at(local_x, local_y, local_z)
                    && let Some(name) = legacy_preview_block_name(id)
                {
                    let state = ImportPreviewState {
                        name,
                        block_class: import_preview_block_class(name),
                    };
                    update_column_slice(
                        columns,
                        usize::from(local_x),
                        usize::from(local_z),
                        y,
                        state,
                    );
                }
            }
        }
    }
}

fn sample_legacy_terrain_columns(
    terrain: &bedrock_world::LegacyTerrain,
    columns: &mut [Option<ImportPreviewColumn>],
) {
    for y in 0u8..128 {
        for local_z in 0u8..16 {
            for local_x in 0u8..16 {
                let Some(id) = terrain.block_id_at(local_x, y, local_z) else {
                    continue;
                };
                let Some(name) = legacy_preview_block_name(id) else {
                    continue;
                };
                let state = ImportPreviewState {
                    name,
                    block_class: import_preview_block_class(name),
                };
                update_column_slice(
                    columns,
                    usize::from(local_x),
                    usize::from(local_z),
                    i32::from(y),
                    state,
                );
            }
        }
    }
}

fn update_preview_column(
    columns: &mut BTreeMap<ChunkPos, Vec<Option<ImportPreviewColumn>>>,
    chunk: ChunkPos,
    local_x: usize,
    local_z: usize,
    y: i32,
    state: ImportPreviewState<'_>,
) {
    let chunk_columns = columns.entry(chunk).or_insert_with(|| vec![None; 16 * 16]);
    update_column_slice(chunk_columns, local_x, local_z, y, state);
}

fn update_column_slice(
    columns: &mut [Option<ImportPreviewColumn>],
    local_x: usize,
    local_z: usize,
    y: i32,
    state: ImportPreviewState<'_>,
) {
    let Some(index) = local_z
        .checked_mul(16)
        .and_then(|row| row.checked_add(local_x))
    else {
        return;
    };
    let Some(column) = columns.get_mut(index) else {
        return;
    };
    if column.as_ref().is_none_or(|existing| y >= existing.y) {
        *column = Some(ImportPreviewColumn {
            y,
            color: import_preview_color_for_state(state),
        });
    }
}

fn preview_images_from_columns(
    columns: BTreeMap<ChunkPos, Vec<Option<ImportPreviewColumn>>>,
) -> Result<BTreeMap<ChunkPos, CopiedChunkPreviewImage>, String> {
    let mut preview_images = BTreeMap::new();
    for (chunk, chunk_columns) in columns {
        let Some(pixels) = preview_pixels_from_chunk_columns(&chunk_columns) else {
            continue;
        };
        preview_images.insert(
            chunk,
            CopiedChunkPreviewImage {
                chunk,
                image: render_image_from_preview_pixels(pixels)?,
                width: 16,
                height: 16,
            },
        );
    }
    Ok(preview_images)
}

fn preview_pixels_from_chunk_columns(columns: &[Option<ImportPreviewColumn>]) -> Option<Vec<u8>> {
    let mut pixels = vec![0_u8; 16 * 16 * 4];
    let mut has_pixels = false;
    for (index, column) in columns.iter().enumerate() {
        let Some(column) = column else {
            continue;
        };
        let pixel_index = index.checked_mul(4)?;
        pixels[pixel_index..pixel_index + 4].copy_from_slice(&column.color);
        has_pixels = true;
    }
    has_pixels.then_some(pixels)
}

fn render_image_from_preview_pixels(pixels: Vec<u8>) -> Result<Arc<RenderImage>, String> {
    RenderImage::from_raw_pixels(16, 16, RenderImagePixelFormat::Rgba8, pixels)
        .map(Arc::new)
        .map_err(|error| format!("导入预览图片无效：{error}"))
}

fn import_preview_parse_options() -> bedrock_world::WorldParseOptions {
    bedrock_world::WorldParseOptions {
        categories: bedrock_world::WorldParseCategories {
            chunks: true,
            players: false,
            actors: false,
            maps: false,
            villages: false,
            globals: false,
        },
        retention: bedrock_world::RetentionMode::Structured,
        subchunk_decode_mode: bedrock_world::SubChunkDecodeMode::FullIndices,
        actor_resolution: bedrock_world::ActorResolution::None,
    }
}

fn structure_palette_preview_state(
    structure: &bedrock_world::McStructureFile,
    index: i32,
) -> Option<ImportPreviewState<'_>> {
    let index = usize::try_from(index).ok()?;
    let entry = structure.palette.get(index)?;
    let block_class = import_preview_block_class(&entry.name);
    Some(ImportPreviewState {
        name: &entry.name,
        block_class,
    })
}

fn block_state_preview_state(state: &bedrock_world::BlockState) -> ImportPreviewState<'_> {
    ImportPreviewState {
        name: &state.name,
        block_class: import_preview_block_class(&state.name),
    }
}

fn import_preview_visible_state<'state>(
    primary: Option<ImportPreviewState<'state>>,
    secondary: Option<ImportPreviewState<'state>>,
) -> Option<ImportPreviewState<'state>> {
    let primary_class = primary.map(|state| state.block_class);
    if let Some(state) = primary
        && import_preview_block_class_is_renderable(state.block_class)
    {
        return Some(state);
    }
    if let Some(state) = secondary
        && matches!(
            state.block_class,
            ImportPreviewBlockClass::Water | ImportPreviewBlockClass::Lava
        )
        && primary_class != Some(state.block_class)
    {
        return Some(state);
    }
    secondary.filter(|state| import_preview_block_class_is_renderable(state.block_class))
}

fn import_preview_color_for_state(state: ImportPreviewState<'_>) -> [u8; 4] {
    let palette = import_preview_palette();
    match state.block_class {
        ImportPreviewBlockClass::Water => [45, 118, 190, 232],
        ImportPreviewBlockClass::Lava => {
            let mut color = palette
                .surface_block_color(state.name, None, true)
                .to_array();
            color[3] = 255;
            color
        }
        ImportPreviewBlockClass::TransparentGlass => {
            let mut color = palette
                .surface_block_color(state.name, None, true)
                .to_array();
            color[3] = 204;
            color
        }
        ImportPreviewBlockClass::Opaque => {
            let mut color = palette
                .surface_block_color(state.name, None, true)
                .to_array();
            color[3] = 255;
            color
        }
        ImportPreviewBlockClass::Air | ImportPreviewBlockClass::SkipTransparent => [0, 0, 0, 0],
    }
}

fn import_preview_palette() -> &'static RenderPalette {
    static PALETTE: OnceLock<RenderPalette> = OnceLock::new();
    PALETTE.get_or_init(RenderPalette::default)
}

#[derive(Clone, Copy)]
struct ImportPreviewColumn {
    y: i32,
    color: [u8; 4],
}

#[derive(Clone, Copy)]
struct ImportPreviewState<'state> {
    name: &'state str,
    block_class: ImportPreviewBlockClass,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ImportPreviewBlockClass {
    Air,
    Opaque,
    TransparentGlass,
    Water,
    Lava,
    SkipTransparent,
}

fn import_preview_block_class(name: &str) -> ImportPreviewBlockClass {
    let normalized = name.strip_prefix("minecraft:").unwrap_or(name);
    if matches!(normalized, "air" | "cave_air" | "void_air") {
        return ImportPreviewBlockClass::Air;
    }
    if matches!(normalized, "water" | "flowing_water") {
        return ImportPreviewBlockClass::Water;
    }
    if matches!(normalized, "lava" | "flowing_lava") {
        return ImportPreviewBlockClass::Lava;
    }
    if import_preview_is_glass_block(normalized) {
        return ImportPreviewBlockClass::TransparentGlass;
    }
    if import_preview_is_transparent_detail_block(normalized) {
        return ImportPreviewBlockClass::SkipTransparent;
    }
    ImportPreviewBlockClass::Opaque
}

fn import_preview_block_class_is_renderable(block_class: ImportPreviewBlockClass) -> bool {
    !matches!(
        block_class,
        ImportPreviewBlockClass::Air | ImportPreviewBlockClass::SkipTransparent
    )
}

fn import_preview_is_glass_block(normalized: &str) -> bool {
    normalized == "glass"
        || normalized == "glass_pane"
        || normalized.ends_with("_glass")
        || normalized.ends_with("_glass_pane")
        || normalized.contains("stained_glass")
}

fn import_preview_is_transparent_detail_block(normalized: &str) -> bool {
    let transparent_exact = [
        "short_grass",
        "tall_grass",
        "fern",
        "large_fern",
        "deadbush",
        "vine",
        "twisting_vines",
        "weeping_vines",
        "kelp",
        "kelp_plant",
        "seagrass",
        "tall_seagrass",
        "snow_layer",
        "tripwire",
        "chain",
    ];
    if transparent_exact.contains(&normalized) {
        return true;
    }

    let transparent_suffixes = [
        "sapling",
        "flower",
        "mushroom",
        "torch",
        "rail",
        "button",
        "pressure_plate",
        "carpet",
        "sign",
        "door",
        "trapdoor",
        "fence",
        "fence_gate",
        "wall",
        "pane",
        "bars",
        "ladder",
        "lever",
        "skull",
        "head",
        "coral",
        "fan",
        "plate",
        "banner",
        "candle",
        "lantern",
        "rod",
        "wire",
    ];
    transparent_suffixes
        .iter()
        .any(|suffix| normalized.ends_with(suffix))
}

fn legacy_preview_block_name(id: u8) -> Option<&'static str> {
    match id {
        0 => None,
        1 => Some("minecraft:stone"),
        2 => Some("minecraft:grass_block"),
        3 => Some("minecraft:dirt"),
        4 => Some("minecraft:cobblestone"),
        5 => Some("minecraft:oak_planks"),
        7 => Some("minecraft:bedrock"),
        8 | 9 => Some("minecraft:water"),
        10 | 11 => Some("minecraft:lava"),
        12 => Some("minecraft:sand"),
        13 => Some("minecraft:gravel"),
        14 => Some("minecraft:gold_ore"),
        15 => Some("minecraft:iron_ore"),
        16 => Some("minecraft:coal_ore"),
        17 => Some("minecraft:oak_log"),
        18 => Some("minecraft:oak_leaves"),
        20 => Some("minecraft:glass"),
        24 => Some("minecraft:sandstone"),
        31 | 32 | 37 | 38 | 39 | 40 => None,
        41 => Some("minecraft:gold_block"),
        42 => Some("minecraft:iron_block"),
        43 | 44 => Some("minecraft:stone_slab"),
        45 => Some("minecraft:bricks"),
        48 => Some("minecraft:mossy_cobblestone"),
        49 => Some("minecraft:obsidian"),
        56 => Some("minecraft:diamond_ore"),
        57 => Some("minecraft:diamond_block"),
        79 => Some("minecraft:ice"),
        80 => Some("minecraft:snow"),
        82 => Some("minecraft:clay"),
        87 => Some("minecraft:netherrack"),
        88 => Some("minecraft:soul_sand"),
        89 => Some("minecraft:glowstone"),
        98 => Some("minecraft:stone_bricks"),
        103 => Some("minecraft:melon"),
        110 => Some("minecraft:mycelium"),
        112 => Some("minecraft:nether_bricks"),
        121 => Some("minecraft:end_stone"),
        129 => Some("minecraft:emerald_ore"),
        133 => Some("minecraft:emerald_block"),
        152 => Some("minecraft:redstone_block"),
        159 => Some("minecraft:white_terracotta"),
        161 => Some("minecraft:acacia_leaves"),
        162 => Some("minecraft:acacia_log"),
        172 => Some("minecraft:terracotta"),
        173 => Some("minecraft:coal_block"),
        174 => Some("minecraft:packed_ice"),
        179 => Some("minecraft:red_sandstone"),
        _ => Some("minecraft:stone"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn import_preview_uses_opaque_alpha_for_solid_blocks() {
        let color = import_preview_color_for_state(ImportPreviewState {
            name: "minecraft:stone",
            block_class: ImportPreviewBlockClass::Opaque,
        });

        assert_eq!(color[3], 255);
    }

    #[::core::prelude::v1::test]
    fn import_preview_preserves_material_specific_transparency() {
        let water = import_preview_color_for_state(ImportPreviewState {
            name: "minecraft:water",
            block_class: ImportPreviewBlockClass::Water,
        });
        let glass = import_preview_color_for_state(ImportPreviewState {
            name: "minecraft:glass",
            block_class: ImportPreviewBlockClass::TransparentGlass,
        });

        assert_eq!(water[3], 232);
        assert_eq!(glass[3], 204);
    }
}
