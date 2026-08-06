use bedrock_render::{
    BlockBoundaryRenderOptions, ChunkRegion, ChunkTileLayout, ImageFormat, MapRenderer,
    PlannedTile, RenderDiagnostics, RenderDiagnosticsSink, RenderMode, RenderOptions,
    RenderPalette, RenderThreadingOptions, RgbaColor, SurfaceRenderOptions, TerrainLightingOptions,
    TilePathScheme,
};
use bedrock_world::{BedrockWorld, Dimension, NbtTag, OpenOptions};
use image::{ImageFormat as OutputImageFormat, save_buffer_with_format};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

mod common;

const TILE_SIZE: u32 = 256;
const TILE_SIZE_I32: i32 = 256;
const DEFAULT_CENTER_TILE_X: i32 = 21;
const DEFAULT_CENTER_TILE_Z: i32 = 12;
const DEFAULT_VIEWPORT_TILES: i32 = 3;
const DEFAULT_LAYER_Y: i32 = 64;
const DEFAULT_CAVE_Y: i32 = 32;

fn main() -> bedrock_render::Result<()> {
    common::init_logger();
    let config = parse_args()?;
    std::fs::create_dir_all(&config.output_dir).map_err(|error| {
        bedrock_render::BedrockRenderError::io(
            format!(
                "failed to create output directory {}",
                config.output_dir.display()
            ),
            error,
        )
    })?;

    let world = Arc::new(
        BedrockWorld::open_blocking(config.world_path.clone(), OpenOptions::default())
            .map_err(bedrock_render::BedrockRenderError::World)?,
    );
    let palette =
        RenderPalette::default().with_unknown_biome_color(RgbaColor::new(96, 96, 96, 255));
    let renderer = MapRenderer::new(world, palette);

    render_viewport(
        &renderer,
        &config,
        RenderMode::Biome { y: config.layer_y },
        &config.output_dir.join("biome-viewport.png"),
    )?;
    render_viewport(
        &renderer,
        &config,
        RenderMode::RawBiomeLayer { y: config.layer_y },
        &config.output_dir.join("raw-biome-viewport.png"),
    )?;
    render_viewport(
        &renderer,
        &config,
        RenderMode::LayerBlocks { y: config.layer_y },
        &config
            .output_dir
            .join(format!("layer-y{}-viewport.png", config.layer_y)),
    )?;
    render_viewport(
        &renderer,
        &config,
        RenderMode::SurfaceBlocks,
        &config.output_dir.join("surface-viewport.png"),
    )?;
    render_viewport(
        &renderer,
        &config,
        RenderMode::HeightMap,
        &config.output_dir.join("heightmap-viewport.png"),
    )?;
    render_viewport(
        &renderer,
        &config,
        RenderMode::RawHeightMap,
        &config.output_dir.join("raw-heightmap-viewport.png"),
    )?;
    render_viewport(
        &renderer,
        &config,
        RenderMode::CaveSlice { y: config.cave_y },
        &config
            .output_dir
            .join(format!("cave-y{}-viewport.png", config.cave_y)),
    )?;

    println!("preview output: {}", config.output_dir.display());
    Ok(())
}

struct PreviewConfig {
    world_path: PathBuf,
    output_dir: PathBuf,
    center_tile_x: i32,
    center_tile_z: i32,
    viewport_tiles: i32,
    chunks_per_tile: u32,
    layer_y: i32,
    cave_y: i32,
}

fn parse_args() -> bedrock_render::Result<PreviewConfig> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    let default_world = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("bedrock-world")
        .join("tests")
        .join("fixtures")
        .join("sample-bedrock-world");
    let default_output = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("bedrock-render-preview");
    let world_path = args.first().map_or(default_world, PathBuf::from);
    let output_dir = args.get(1).map_or(default_output, PathBuf::from);
    let (default_center_tile_x, default_center_tile_z) =
        spawn_tile_center(&world_path).unwrap_or((DEFAULT_CENTER_TILE_X, DEFAULT_CENTER_TILE_Z));
    let center_tile_x = parse_optional_i32(args.get(2), default_center_tile_x, "center_tile_x")?;
    let center_tile_z = parse_optional_i32(args.get(3), default_center_tile_z, "center_tile_z")?;
    let viewport_tiles = parse_optional_i32(args.get(4), DEFAULT_VIEWPORT_TILES, "viewport_tiles")?;
    let layer_y = parse_optional_i32(args.get(5), DEFAULT_LAYER_Y, "layer_y")?;
    let cave_y = parse_optional_i32(args.get(6), DEFAULT_CAVE_Y, "cave_y")?;
    let chunks_per_tile = u32::try_from(parse_optional_i32(args.get(7), 16, "chunks_per_tile")?)
        .map_err(|_| {
            bedrock_render::BedrockRenderError::Validation(
                "chunks_per_tile must be positive".to_string(),
            )
        })?;
    if viewport_tiles <= 0 || viewport_tiles % 2 == 0 {
        return Err(bedrock_render::BedrockRenderError::Validation(
            "viewport_tiles must be a positive odd integer".to_string(),
        ));
    }
    if !world_path.join("db").join("CURRENT").exists() && !world_path.join("chunks.dat").exists() {
        return Err(bedrock_render::BedrockRenderError::Validation(format!(
            "world storage not found: expected {} or {}",
            world_path.join("db").join("CURRENT").display(),
            world_path.join("chunks.dat").display()
        )));
    }
    Ok(PreviewConfig {
        world_path,
        output_dir,
        center_tile_x,
        center_tile_z,
        viewport_tiles,
        chunks_per_tile,
        layer_y,
        cave_y,
    })
}

fn parse_optional_i32(
    value: Option<&std::ffi::OsString>,
    default: i32,
    name: &str,
) -> bedrock_render::Result<i32> {
    let Some(value) = value else {
        return Ok(default);
    };
    value.to_string_lossy().parse::<i32>().map_err(|error| {
        bedrock_render::BedrockRenderError::Validation(format!("invalid {name}: {error}"))
    })
}

fn spawn_tile_center(world_path: &Path) -> Option<(i32, i32)> {
    let document = bedrock_world::read_level_dat_document(&world_path.join("level.dat")).ok()?;
    let NbtTag::Compound(root) = &document.root else {
        return None;
    };
    let spawn_x = nbt_i32(root.get("SpawnX")?)?;
    let spawn_z = nbt_i32(root.get("SpawnZ")?)?;
    Some((
        spawn_x.div_euclid(TILE_SIZE_I32),
        spawn_z.div_euclid(TILE_SIZE_I32),
    ))
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

fn render_viewport(
    renderer: &MapRenderer,
    config: &PreviewConfig,
    mode: RenderMode,
    output_path: &Path,
) -> bedrock_render::Result<()> {
    let planned_tiles = viewport_tiles(config, mode)?;
    let jobs = planned_tiles
        .iter()
        .map(|tile| tile.job.clone())
        .collect::<Vec<_>>();
    let diagnostics = Arc::new(Mutex::new(RenderDiagnostics::default()));
    let diagnostics_sink = matches!(mode, RenderMode::SurfaceBlocks).then(|| {
        RenderDiagnosticsSink::new({
            let diagnostics = Arc::clone(&diagnostics);
            move |value| {
                if let Ok(mut diagnostics) = diagnostics.lock() {
                    diagnostics.add(value);
                }
            }
        })
    });
    let tiles = renderer.render_tiles_blocking(
        jobs,
        RenderOptions {
            format: ImageFormat::Rgba,
            threading: RenderThreadingOptions::Auto,
            diagnostics: diagnostics_sink,
            surface: preview_surface_options(),
            ..RenderOptions::default()
        },
    )?;
    save_web_tiles(config, mode, &planned_tiles, &tiles)?;
    let width = TILE_SIZE
        .checked_mul(u32::try_from(config.viewport_tiles).map_err(|_| {
            bedrock_render::BedrockRenderError::Validation("viewport width overflow".to_string())
        })?)
        .ok_or_else(|| {
            bedrock_render::BedrockRenderError::Validation("viewport width overflow".to_string())
        })?;
    let height = width;
    let mut atlas = vec![
        0_u8;
        usize::try_from(width)
            .ok()
            .and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| bedrock_render::BedrockRenderError::Validation(
                "atlas buffer size overflow".to_string()
            ))?
    ];

    for tile in tiles {
        let local_tile_x = tile.coord.x - config.center_tile_x + config.viewport_tiles / 2;
        let local_tile_z = tile.coord.z - config.center_tile_z + config.viewport_tiles / 2;
        blit_tile(
            &mut atlas,
            width,
            u32::try_from(local_tile_x).map_err(|_| {
                bedrock_render::BedrockRenderError::Validation(
                    "tile x outside viewport".to_string(),
                )
            })?,
            u32::try_from(local_tile_z).map_err(|_| {
                bedrock_render::BedrockRenderError::Validation(
                    "tile z outside viewport".to_string(),
                )
            })?,
            &tile.rgba,
        )?;
    }

    save_png(output_path, &atlas, width, height)?;
    if matches!(mode, RenderMode::SurfaceBlocks)
        && let Ok(diagnostics) = diagnostics.lock()
    {
        println!(
            "surface diagnostics: missing_chunks={} missing_heightmaps={} unknown_blocks={} fallback_pixels={}",
            diagnostics.missing_chunks,
            diagnostics.missing_heightmaps,
            diagnostics.unknown_blocks,
            diagnostics.fallback_pixels
        );
    }
    Ok(())
}

fn viewport_tiles(
    config: &PreviewConfig,
    mode: RenderMode,
) -> bedrock_render::Result<Vec<PlannedTile>> {
    let chunks_per_tile = i32::try_from(config.chunks_per_tile).map_err(|_| {
        bedrock_render::BedrockRenderError::Validation("chunks_per_tile overflow".to_string())
    })?;
    let tile_radius = config.viewport_tiles.checked_div(2).ok_or_else(|| {
        bedrock_render::BedrockRenderError::Validation("viewport tile range overflow".to_string())
    })?;
    let min_tile_x = config
        .center_tile_x
        .checked_sub(tile_radius)
        .ok_or_else(|| {
            bedrock_render::BedrockRenderError::Validation("min tile x overflow".to_string())
        })?;
    let max_tile_x = config
        .center_tile_x
        .checked_add(tile_radius)
        .ok_or_else(|| {
            bedrock_render::BedrockRenderError::Validation("max tile x overflow".to_string())
        })?;
    let min_tile_z = config
        .center_tile_z
        .checked_sub(tile_radius)
        .ok_or_else(|| {
            bedrock_render::BedrockRenderError::Validation("min tile z overflow".to_string())
        })?;
    let max_tile_z = config
        .center_tile_z
        .checked_add(tile_radius)
        .ok_or_else(|| {
            bedrock_render::BedrockRenderError::Validation("max tile z overflow".to_string())
        })?;
    let min_chunk_x = min_tile_x.checked_mul(chunks_per_tile).ok_or_else(|| {
        bedrock_render::BedrockRenderError::Validation("min chunk x overflow".to_string())
    })?;
    let min_chunk_z = min_tile_z.checked_mul(chunks_per_tile).ok_or_else(|| {
        bedrock_render::BedrockRenderError::Validation("min chunk z overflow".to_string())
    })?;
    let max_chunk_x = max_tile_x
        .checked_add(1)
        .and_then(|tile| tile.checked_mul(chunks_per_tile))
        .and_then(|chunk| chunk.checked_sub(1))
        .ok_or_else(|| {
            bedrock_render::BedrockRenderError::Validation("max chunk x overflow".to_string())
        })?;
    let max_chunk_z = max_tile_z
        .checked_add(1)
        .and_then(|tile| tile.checked_mul(chunks_per_tile))
        .and_then(|chunk| chunk.checked_sub(1))
        .ok_or_else(|| {
            bedrock_render::BedrockRenderError::Validation("max chunk z overflow".to_string())
        })?;
    let region = ChunkRegion::new(
        Dimension::Overworld,
        min_chunk_x,
        min_chunk_z,
        max_chunk_x,
        max_chunk_z,
    );
    MapRenderer::<std::sync::Arc<dyn bedrock_world::WorldStorage>>::plan_region_tiles(
        region,
        mode,
        ChunkTileLayout {
            chunks_per_tile: config.chunks_per_tile,
            blocks_per_pixel: 1,
            pixels_per_block: 1,
        },
    )
}

fn preview_surface_options() -> SurfaceRenderOptions {
    let mut lighting = TerrainLightingOptions::strong();
    lighting.normal_strength = 2.35;
    lighting.shadow_strength = 0.68;
    lighting.highlight_strength = 0.52;
    lighting.edge_relief_strength = 0.34;
    lighting.edge_relief_threshold = 2.0;
    lighting.edge_relief_max_shadow = 22.0;
    SurfaceRenderOptions {
        height_shading: true,
        lighting,
        block_boundaries: BlockBoundaryRenderOptions {
            strength: 0.42,
            flat_strength: 0.16,
            max_shadow: 18.0,
            ..BlockBoundaryRenderOptions::default()
        },
        ..SurfaceRenderOptions::default()
    }
}

fn save_web_tiles(
    config: &PreviewConfig,
    mode: RenderMode,
    planned_tiles: &[PlannedTile],
    tiles: &[bedrock_render::TileImage],
) -> bedrock_render::Result<()> {
    let tile_dir = config.output_dir.join("web-tiles");
    for (planned, tile) in planned_tiles.iter().zip(tiles) {
        let relative_path = planned.relative_path(TilePathScheme::WebMap, "png");
        let path = tile_dir.join(relative_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                bedrock_render::BedrockRenderError::Validation(error.to_string())
            })?;
        }
        save_png(&path, &tile.rgba, tile.width, tile.height)?;
    }
    let marker = tile_dir
        .join("latest")
        .join(format!("{mode:?}.txt").replace([' ', '{', '}', ':', ','], "_"));
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| bedrock_render::BedrockRenderError::Validation(error.to_string()))?;
    }
    std::fs::write(
        marker,
        format!(
            "chunks_per_tile={}\ncenter_tile_x={}\ncenter_tile_z={}\nviewport_tiles={}\n",
            config.chunks_per_tile,
            config.center_tile_x,
            config.center_tile_z,
            config.viewport_tiles
        ),
    )
    .map_err(|error| bedrock_render::BedrockRenderError::Validation(error.to_string()))
}

fn blit_tile(
    atlas: &mut [u8],
    atlas_width: u32,
    tile_x: u32,
    tile_z: u32,
    rgba: &[u8],
) -> bedrock_render::Result<()> {
    let row_bytes = usize::try_from(TILE_SIZE)
        .ok()
        .and_then(|size| size.checked_mul(4))
        .ok_or_else(|| {
            bedrock_render::BedrockRenderError::Validation("tile row size overflow".to_string())
        })?;
    for y in 0..TILE_SIZE {
        let source_start = usize::try_from(y)
            .ok()
            .and_then(|row| row.checked_mul(row_bytes))
            .ok_or_else(|| {
                bedrock_render::BedrockRenderError::Validation(
                    "tile source offset overflow".to_string(),
                )
            })?;
        let dest_x = tile_x.checked_mul(TILE_SIZE).ok_or_else(|| {
            bedrock_render::BedrockRenderError::Validation("atlas x overflow".to_string())
        })?;
        let dest_y = tile_z
            .checked_mul(TILE_SIZE)
            .and_then(|base| base.checked_add(y))
            .ok_or_else(|| {
                bedrock_render::BedrockRenderError::Validation("atlas y overflow".to_string())
            })?;
        let dest_start = usize::try_from(dest_y)
            .ok()
            .and_then(|row| {
                usize::try_from(atlas_width)
                    .ok()
                    .and_then(|width| row.checked_mul(width))
            })
            .and_then(|pixels| pixels.checked_add(usize::try_from(dest_x).ok()?))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| {
                bedrock_render::BedrockRenderError::Validation(
                    "atlas destination offset overflow".to_string(),
                )
            })?;
        let source_end = source_start.checked_add(row_bytes).ok_or_else(|| {
            bedrock_render::BedrockRenderError::Validation("tile source range overflow".to_string())
        })?;
        let dest_end = dest_start.checked_add(row_bytes).ok_or_else(|| {
            bedrock_render::BedrockRenderError::Validation(
                "atlas destination range overflow".to_string(),
            )
        })?;
        let source = rgba.get(source_start..source_end).ok_or_else(|| {
            bedrock_render::BedrockRenderError::Validation("tile source is truncated".to_string())
        })?;
        let dest = atlas.get_mut(dest_start..dest_end).ok_or_else(|| {
            bedrock_render::BedrockRenderError::Validation("atlas buffer is truncated".to_string())
        })?;
        dest.copy_from_slice(source);
    }
    Ok(())
}

fn save_png(
    output_path: &Path,
    rgba: &[u8],
    width: u32,
    height: u32,
) -> bedrock_render::Result<()> {
    save_buffer_with_format(
        output_path,
        rgba,
        width,
        height,
        image::ColorType::Rgba8,
        OutputImageFormat::Png,
    )
    .map_err(|error| {
        bedrock_render::BedrockRenderError::image(
            format!("failed to save PNG {}", output_path.display()),
            error,
        )
    })
}
