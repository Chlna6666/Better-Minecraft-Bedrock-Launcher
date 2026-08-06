use bedrock_render::{
    ImageFormat, MapRenderer, RenderBackend, RenderJob, RenderLayout, RenderMode, RenderOptions,
    RenderPalette, RenderThreadingOptions, TileCoord,
};
use bedrock_world::{
    BedrockWorld, ChunkData, ChunkLoadOptions, ChunkPos, Dimension, ExactSurfaceBiomeLoad,
    ExactSurfaceSubchunkPolicy, NbtTag, OpenOptions, SubChunkFormat, WorldThreadingOptions,
    read_level_dat_document,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[test]
#[ignore = "requires BEDROCK_RENDER_REAL_WORLD to point at the failing local world"]
fn real_world_exact_surface_finds_secondary_storage_surface()
-> Result<(), Box<dyn std::error::Error>> {
    let Some(world_path) = std::env::var_os("BEDROCK_RENDER_REAL_WORLD").map(PathBuf::from) else {
        eprintln!("BEDROCK_RENDER_REAL_WORLD is not set; skipping real-world smoke test");
        return Ok(());
    };
    let world = Arc::new(BedrockWorld::open_blocking(
        &world_path,
        OpenOptions::default(),
    )?);
    let (spawn_chunk_x, spawn_chunk_z) = spawn_chunk(&world_path).unwrap_or((0, 0));

    let target_pos = ChunkPos {
        x: spawn_chunk_x + 1,
        z: spawn_chunk_z - 3,
        dimension: Dimension::Overworld,
    };
    let chunk = world.query_chunk_data_blocking(target_pos, exact_surface_options())?;
    let surface = find_layered_surface_column(&chunk).ok_or_else(|| {
        format!("no secondary-storage exact surface column found in chunk {target_pos:?}")
    })?;
    eprintln!(
        "real-world layered surface chunk=({}, {}) local=({}, {}) surface={} block={}",
        surface.chunk_pos.x,
        surface.chunk_pos.z,
        surface.local_x,
        surface.local_z,
        surface.surface_y,
        surface.block_name
    );

    let renderer = MapRenderer::new(world, RenderPalette::default());
    let layout = RenderLayout {
        chunks_per_tile: 1,
        blocks_per_pixel: 1,
        pixels_per_block: 1,
    };
    let tile_x = surface.chunk_pos.x;
    let tile_z = surface.chunk_pos.z;
    for mode in [
        RenderMode::SurfaceBlocks,
        RenderMode::HeightMap,
        RenderMode::RawHeightMap,
    ] {
        let tile = renderer.render_tile_with_options_blocking(
            RenderJob::chunk_tile(
                TileCoord {
                    x: tile_x,
                    z: tile_z,
                    dimension: Dimension::Overworld,
                },
                mode,
                layout,
            )?,
            &RenderOptions {
                format: ImageFormat::Rgba,
                backend: RenderBackend::Cpu,
                threading: RenderThreadingOptions::Single,
                ..RenderOptions::default()
            },
        )?;
        assert_eq!(
            tile.rgba.len(),
            usize::try_from(tile.width * tile.height * 4)?
        );
    }

    Ok(())
}

#[test]
#[ignore = "requires BEDROCK_RENDER_REAL_WORLD to point at the failing local world"]
fn real_world_zero_bit_top_subchunks_parse_and_sample_above_stone()
-> Result<(), Box<dyn std::error::Error>> {
    let Some(world_path) = std::env::var_os("BEDROCK_RENDER_REAL_WORLD").map(PathBuf::from) else {
        eprintln!("BEDROCK_RENDER_REAL_WORLD is not set; skipping real-world regression test");
        return Ok(());
    };
    let world = BedrockWorld::open_blocking(&world_path, OpenOptions::default())?;

    for (block_x, block_z) in [(340_i32, 36_i32), (360_i32, 67_i32)] {
        let chunk_pos = ChunkPos {
            x: block_x.div_euclid(16),
            z: block_z.div_euclid(16),
            dimension: Dimension::Overworld,
        };
        let local_x = u8::try_from(block_x.rem_euclid(16))?;
        let local_z = u8::try_from(block_z.rem_euclid(16))?;
        let chunk = world.query_chunk_data_blocking(chunk_pos, exact_surface_options())?;
        let subchunk = chunk
            .subchunks
            .get(&4)
            .ok_or_else(|| format!("missing sy=4 at block ({block_x}, {block_z})"))?;
        assert!(
            matches!(subchunk.format, SubChunkFormat::Paletted { .. }),
            "sy=4 decoded as non-paletted at block ({block_x}, {block_z}): {:?}",
            subchunk.format
        );

        let raw_height = world
            .get_height_at_blocking(chunk_pos, local_x, local_z)?
            .ok_or_else(|| format!("missing raw height at block ({block_x}, {block_z})"))?;
        assert!(
            raw_height > 63,
            "normalized raw height should point into sy=4 near ({block_x}, {block_z}), got {raw_height}"
        );

        let sample = chunk
            .column_sample_at(local_x, local_z)
            .ok_or_else(|| format!("missing exact surface at block ({block_x}, {block_z})"))?;
        eprintln!(
            "real-world top column block=({block_x}, {block_z}) chunk=({}, {}) local=({}, {}) raw_height={} surface={} block={}",
            chunk_pos.x,
            chunk_pos.z,
            local_x,
            local_z,
            raw_height,
            sample.surface_y,
            sample.surface_block_state.name
        );
        assert!(
            sample.surface_y > 63,
            "surface should come from sy=4 or above at block ({block_x}, {block_z}), got {}",
            sample.surface_y
        );
        assert_ne!(
            sample.surface_block_state.name, "minecraft:stone",
            "surface still fell back to stone at block ({block_x}, {block_z})"
        );
    }

    Ok(())
}

#[test]
#[ignore = "requires BEDROCK_RENDER_REAL_WORLD to point at the failing local world"]
fn real_world_cobblestone_slab_aliases_render_consistently()
-> Result<(), Box<dyn std::error::Error>> {
    let Some(world_path) = std::env::var_os("BEDROCK_RENDER_REAL_WORLD").map(PathBuf::from) else {
        eprintln!("BEDROCK_RENDER_REAL_WORLD is not set; skipping slab alias regression test");
        return Ok(());
    };
    let world = Arc::new(BedrockWorld::open_blocking(
        &world_path,
        OpenOptions::default(),
    )?);

    for (block_x, block_z, expected_name) in [
        (178_i32, -15_i32, "minecraft:cobblestone_slab"),
        (172_i32, 20_i32, "minecraft:cobblestone_slab"),
        (170_i32, -15_i32, "minecraft:stone_slab"),
        (210_i32, -15_i32, "minecraft:stone_slab"),
    ] {
        let chunk_pos = ChunkPos {
            x: block_x.div_euclid(16),
            z: block_z.div_euclid(16),
            dimension: Dimension::Overworld,
        };
        let local_x = u8::try_from(block_x.rem_euclid(16))?;
        let local_z = u8::try_from(block_z.rem_euclid(16))?;
        let chunk = world.query_chunk_data_blocking(chunk_pos, exact_surface_options())?;
        let sample = chunk
            .column_sample_at(local_x, local_z)
            .ok_or_else(|| format!("missing exact surface at block ({block_x}, {block_z})"))?;
        eprintln!(
            "real-world slab alias block=({block_x}, {block_z}) surface={} block={} states={:?}",
            sample.surface_y, sample.surface_block_state.name, sample.surface_block_state.states
        );
        assert_eq!(sample.surface_y, 181);
        assert_eq!(sample.surface_block_state.name, expected_name);
    }

    let renderer = MapRenderer::new(Arc::clone(&world), RenderPalette::default());
    let modern_a = surface_pixel_for_block(&renderer, 178, -15)?;
    let modern_b = surface_pixel_for_block(&renderer, 172, 20)?;
    let old_a = surface_pixel_for_block(&renderer, 170, -15)?;
    let old_b = surface_pixel_for_block(&renderer, 210, -15)?;
    let expected = RenderPalette::default()
        .block_color("minecraft:cobblestone")
        .to_array();

    assert_eq!(modern_a, expected);
    assert_eq!(modern_b, expected);
    assert_eq!(old_a, expected);
    assert_eq!(old_b, expected);
    assert_eq!(modern_a, old_a);
    assert_eq!(modern_b, old_b);

    Ok(())
}

fn exact_surface_options() -> ChunkLoadOptions {
    ChunkLoadOptions {
        threading: WorldThreadingOptions::Auto,
        ..ChunkLoadOptions::exact_surface_columns(
            ExactSurfaceSubchunkPolicy::Full,
            ExactSurfaceBiomeLoad::None,
            false,
        )
    }
}

fn surface_pixel_for_block(
    renderer: &MapRenderer,
    block_x: i32,
    block_z: i32,
) -> Result<[u8; 4], Box<dyn std::error::Error>> {
    let chunk_x = block_x.div_euclid(16);
    let chunk_z = block_z.div_euclid(16);
    let local_x = u32::try_from(block_x.rem_euclid(16))?;
    let local_z = u32::try_from(block_z.rem_euclid(16))?;
    let tile = renderer.render_tile_with_options_blocking(
        RenderJob::chunk_tile(
            TileCoord {
                x: chunk_x,
                z: chunk_z,
                dimension: Dimension::Overworld,
            },
            RenderMode::SurfaceBlocks,
            RenderLayout {
                chunks_per_tile: 1,
                blocks_per_pixel: 1,
                pixels_per_block: 1,
            },
        )?,
        &RenderOptions {
            format: ImageFormat::Rgba,
            backend: RenderBackend::Cpu,
            threading: RenderThreadingOptions::Single,
            surface: bedrock_render::SurfaceRenderOptions {
                biome_tint: false,
                height_shading: false,
                block_boundaries: bedrock_render::BlockBoundaryRenderOptions::off(),
                ..bedrock_render::SurfaceRenderOptions::default()
            },
            ..RenderOptions::default()
        },
    )?;
    let index = usize::try_from((local_z * tile.width + local_x) * 4)?;
    Ok([
        tile.rgba[index],
        tile.rgba[index + 1],
        tile.rgba[index + 2],
        tile.rgba[index + 3],
    ])
}

fn find_layered_surface_column(chunk: &ChunkData) -> Option<RealWorldLayeredSurface> {
    for local_z in 0..16_u8 {
        for local_x in 0..16_u8 {
            let Some(sample) = chunk.column_sample_at(local_x, local_z) else {
                continue;
            };
            let Some(subchunk_y) = i8::try_from(i32::from(sample.surface_y).div_euclid(16)).ok()
            else {
                continue;
            };
            let Some(local_y) = u8::try_from(i32::from(sample.surface_y).rem_euclid(16)).ok()
            else {
                continue;
            };
            let Some(subchunk) = chunk.subchunks.get(&subchunk_y) else {
                continue;
            };
            let storage_zero = subchunk.block_state_at(local_x, local_y, local_z);
            if storage_zero.is_none_or(|state| state.name != sample.surface_block_state.name)
                && subchunk
                    .visible_block_state_at(local_x, local_y, local_z)
                    .is_some_and(|state| state.name == sample.surface_block_state.name)
            {
                return Some(RealWorldLayeredSurface {
                    chunk_pos: chunk.pos,
                    local_x,
                    local_z,
                    surface_y: sample.surface_y,
                    block_name: sample.surface_block_state.name.clone(),
                });
            }
        }
    }
    None
}

struct RealWorldLayeredSurface {
    chunk_pos: ChunkPos,
    local_x: u8,
    local_z: u8,
    surface_y: i16,
    block_name: String,
}

fn spawn_chunk(world_path: &Path) -> Option<(i32, i32)> {
    let document = read_level_dat_document(&world_path.join("level.dat")).ok()?;
    let NbtTag::Compound(root) = document.root else {
        return None;
    };
    let spawn_x = nbt_i32(root.get("SpawnX")?)?;
    let spawn_z = nbt_i32(root.get("SpawnZ")?)?;
    Some((spawn_x.div_euclid(16), spawn_z.div_euclid(16)))
}

fn nbt_i32(tag: &NbtTag) -> Option<i32> {
    match tag {
        NbtTag::Byte(value) => Some(i32::from(*value)),
        NbtTag::Short(value) => Some(i32::from(*value)),
        NbtTag::Int(value) => Some(*value),
        NbtTag::Long(value) => i32::try_from(*value).ok(),
        _ => None,
    }
}
