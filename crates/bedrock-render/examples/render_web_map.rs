use bedrock_render::{
    BlockBoundaryRenderOptions, ChunkRegion, DEFAULT_PALETTE_VERSION, ImageFormat, MapRenderer,
    PlannedTile, RENDERER_CACHE_VERSION, RegionLayout, RenderBackend, RenderDiagnostics,
    RenderDiagnosticsSink, RenderExecutionProfile, RenderJob, RenderLayout, RenderMemoryBudget,
    RenderMode, RenderOptions, RenderPalette, RenderPipelineStats, RenderThreadingOptions,
    SurfaceRenderOptions, TerrainLightingOptions, TerrainLightingPreset, TileCoord,
};
use bedrock_world::{
    BedrockWorld, ChunkBounds, ChunkPos, Dimension, OpenOptions, WorldScanOptions,
    WorldThreadingOptions,
};
use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::Read as _;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc,
};
use std::thread;

mod common;

// This example keeps setup, layout discovery, and rendering orchestration visible in one entrypoint.
#[allow(clippy::too_many_lines)]
fn main() -> bedrock_render::Result<()> {
    common::init_logger();
    let mut config = WebMapConfig::parse()?;
    if config.output_dir.exists() && config.force {
        fs::remove_dir_all(&config.output_dir).map_err(|error| {
            bedrock_render::BedrockRenderError::io(
                format!(
                    "failed to remove output directory {}",
                    config.output_dir.display()
                ),
                error,
            )
        })?;
    }
    fs::create_dir_all(&config.output_dir).map_err(|error| {
        bedrock_render::BedrockRenderError::io(
            format!(
                "failed to create output directory {}",
                config.output_dir.display()
            ),
            error,
        )
    })?;

    let mut palette = RenderPalette::default();
    for json_path in &config.palette_json_paths {
        let report = palette.merge_json_file(json_path)?;
        println!(
            "loaded palette JSON {}: block_colors={} biome_colors={} skipped={}",
            json_path.display(),
            report.block_colors,
            report.biome_colors,
            report.skipped_entries
        );
    }
    config.cache_signature = cache_signature(
        &config.world_path,
        config.layout,
        config.region_layout,
        config.surface_options,
        config.backend,
        &config.palette_json_paths,
        None,
    )?;
    let cache_valid =
        read_cache_manifest(&config)?.is_some_and(|manifest| manifest == config.cache_signature);
    if config.render_cache_validation {
        write_cache_manifest(&config)?;
    }
    let mut dimensions = initial_layout_dimensions(&config);
    write_map_layout(&config, &dimensions)?;
    write_tile_index(&config, &dimensions)?;
    write_viewer_html(&config)?;
    println!(
        "web map viewer initialized: {}",
        config.output_dir.join("viewer.html").display()
    );

    let world = Arc::new(
        BedrockWorld::open_blocking(config.world_path.clone(), OpenOptions::default())
            .map_err(bedrock_render::BedrockRenderError::World)?,
    );
    let renderer = MapRenderer::new(Arc::clone(&world), palette);
    let discovered_dimensions = discover_dimension_tiles(world.as_ref(), &config)?;

    for (dimension_index, dimension) in config.dimensions.iter().enumerate() {
        let discovered = discovered_dimensions
            .get(dimension)
            .cloned()
            .unwrap_or_default();
        dimensions[dimension_index] = plan_dimension_layout(&config, *dimension, discovered);
        write_map_layout(&config, &dimensions)?;
        write_tile_index(&config, &dimensions)?;
    }

    if config.parallel_dimensions {
        render_dimensions_parallel(&renderer, &config, cache_valid, &mut dimensions)?;
    } else {
        for dimension_index in 0..dimensions.len() {
            render_dimension_modes(
                &renderer,
                &config,
                cache_valid,
                dimension_index,
                &mut dimensions,
            )?;
        }
    }

    println!("web map output: {}", config.output_dir.display());
    Ok(())
}

#[derive(Debug, Clone)]
// These flags map directly to CLI switches and keep the generated manifest explicit.
#[allow(clippy::struct_excessive_bools)]
struct WebMapConfig {
    world_path: PathBuf,
    output_dir: PathBuf,
    dimensions: Vec<Dimension>,
    modes: Vec<WebRenderMode>,
    y_layer: i32,
    cave_y: i32,
    layout: RenderLayout,
    region_layout: RegionLayout,
    surface_options: SurfaceRenderOptions,
    region: Option<(i32, i32, i32, i32)>,
    threading: RenderThreadingOptions,
    backend: RenderBackend,
    profile: RenderExecutionProfile,
    memory_budget: RenderMemoryBudget,
    pipeline_depth: usize,
    print_stats: bool,
    palette_json_paths: Vec<PathBuf>,
    plugin_data_paths: Vec<PathBuf>,
    viewer_prefetch_radius: usize,
    viewer_retain_radius: usize,
    viewer_max_image_loads: usize,
    viewer_refresh_ms: u64,
    max_render_threads: Option<usize>,
    reserve_threads: usize,
    tile_batch_size: Option<usize>,
    writer_threads: usize,
    write_queue_capacity: usize,
    parallel_dimensions: bool,
    render_cache_validation: bool,
    cache_signature: String,
    force: bool,
}

impl WebMapConfig {
    // The example avoids a CLI parser dependency so downstream users can copy it into tools easily.
    #[allow(clippy::too_many_lines)]
    fn parse() -> bedrock_render::Result<Self> {
        let mut world_path = default_world_path();
        let mut output_dir = default_output_dir();
        let mut dimensions = vec![Dimension::Overworld, Dimension::Nether, Dimension::End];
        let mut modes = vec![
            WebRenderMode::Surface,
            WebRenderMode::HeightMap,
            WebRenderMode::Biome,
        ];
        let mut y_layer = 64;
        let mut cave_y = 32;
        let mut chunks_per_tile = 16;
        let mut chunks_per_region = 32;
        let mut blocks_per_pixel = 1;
        let mut pixels_per_block = None;
        let mut tile_size_pixels = None;
        let mut terrain_lighting = TerrainLightingOptions::default();
        let mut block_boundaries = BlockBoundaryRenderOptions::default();
        let mut region = None;
        let mut threading = RenderThreadingOptions::Auto;
        let backend = RenderBackend::Cpu;
        let mut profile = RenderExecutionProfile::Export;
        let mut memory_budget = RenderMemoryBudget::Auto;
        let mut pipeline_depth = 0_usize;
        let mut print_stats = false;
        let mut palette_json_paths = Vec::new();
        let mut plugin_data_paths = Vec::new();
        let mut viewer_prefetch_radius = 1_usize;
        let mut viewer_retain_radius = 2_usize;
        let mut viewer_max_image_loads = 8_usize;
        let mut viewer_refresh_ms = 2000_u64;
        let mut max_render_threads = None;
        let mut reserve_threads = 0_usize;
        let mut tile_batch_size = None;
        let mut writer_threads = 1_usize;
        let mut write_queue_capacity = 256_usize;
        let mut parallel_dimensions = true;
        let mut render_cache_validation = true;
        let mut force = false;

        let args = std::env::args().skip(1).collect::<Vec<_>>();
        let mut index = 0;
        while index < args.len() {
            match args[index].as_str() {
                "--world" => {
                    world_path = PathBuf::from(next_arg(&args, &mut index, "--world")?);
                }
                "--out" => {
                    output_dir = PathBuf::from(next_arg(&args, &mut index, "--out")?);
                }
                "--dimensions" => {
                    dimensions = parse_dimensions(&next_arg(&args, &mut index, "--dimensions")?)?;
                }
                "--mode" | "--modes" => {
                    modes = parse_modes(&next_arg(&args, &mut index, "--mode")?)?;
                }
                "--y" => {
                    y_layer = parse_i32(&next_arg(&args, &mut index, "--y")?, "--y")?;
                }
                "--cave-y" => {
                    cave_y = parse_i32(&next_arg(&args, &mut index, "--cave-y")?, "--cave-y")?;
                }
                "--chunks-per-tile" => {
                    chunks_per_tile = parse_u32(
                        &next_arg(&args, &mut index, "--chunks-per-tile")?,
                        "--chunks-per-tile",
                    )?;
                }
                "--chunks-per-region" => {
                    chunks_per_region = parse_u32(
                        &next_arg(&args, &mut index, "--chunks-per-region")?,
                        "--chunks-per-region",
                    )?;
                }
                "--blocks-per-pixel" => {
                    blocks_per_pixel = parse_u32(
                        &next_arg(&args, &mut index, "--blocks-per-pixel")?,
                        "--blocks-per-pixel",
                    )?;
                }
                "--pixels-per-block" => {
                    pixels_per_block = Some(parse_u32(
                        &next_arg(&args, &mut index, "--pixels-per-block")?,
                        "--pixels-per-block",
                    )?);
                }
                "--tile-size-pixels" => {
                    tile_size_pixels = Some(parse_u32(
                        &next_arg(&args, &mut index, "--tile-size-pixels")?,
                        "--tile-size-pixels",
                    )?);
                }
                "--terrain-lighting" => {
                    terrain_lighting = parse_terrain_lighting(&next_arg(
                        &args,
                        &mut index,
                        "--terrain-lighting",
                    )?)?;
                }
                "--light-azimuth" => {
                    terrain_lighting.light_azimuth_degrees = parse_f32(
                        &next_arg(&args, &mut index, "--light-azimuth")?,
                        "--light-azimuth",
                    )?;
                }
                "--light-elevation" => {
                    terrain_lighting.light_elevation_degrees = parse_f32(
                        &next_arg(&args, &mut index, "--light-elevation")?,
                        "--light-elevation",
                    )?;
                }
                "--normal-strength" | "--land-normal-strength" => {
                    terrain_lighting.normal_strength = parse_f32(
                        &next_arg(&args, &mut index, "--normal-strength")?,
                        "--normal-strength",
                    )?;
                }
                "--land-shadow-strength" => {
                    terrain_lighting.shadow_strength = parse_f32(
                        &next_arg(&args, &mut index, "--land-shadow-strength")?,
                        "--land-shadow-strength",
                    )?;
                }
                "--land-highlight-strength" => {
                    terrain_lighting.highlight_strength = parse_f32(
                        &next_arg(&args, &mut index, "--land-highlight-strength")?,
                        "--land-highlight-strength",
                    )?;
                }
                "--land-ambient-occlusion" => {
                    terrain_lighting.ambient_occlusion = parse_f32(
                        &next_arg(&args, &mut index, "--land-ambient-occlusion")?,
                        "--land-ambient-occlusion",
                    )?;
                }
                "--land-max-shadow" => {
                    terrain_lighting.max_shadow = parse_f32(
                        &next_arg(&args, &mut index, "--land-max-shadow")?,
                        "--land-max-shadow",
                    )?;
                }
                "--land-slope-softness" => {
                    terrain_lighting.land_slope_softness = parse_f32(
                        &next_arg(&args, &mut index, "--land-slope-softness")?,
                        "--land-slope-softness",
                    )?;
                }
                "--edge-relief-strength" => {
                    terrain_lighting.edge_relief_strength = parse_f32(
                        &next_arg(&args, &mut index, "--edge-relief-strength")?,
                        "--edge-relief-strength",
                    )?;
                }
                "--edge-relief-threshold" => {
                    terrain_lighting.edge_relief_threshold = parse_f32(
                        &next_arg(&args, &mut index, "--edge-relief-threshold")?,
                        "--edge-relief-threshold",
                    )?;
                }
                "--edge-relief-max-shadow" => {
                    terrain_lighting.edge_relief_max_shadow = parse_f32(
                        &next_arg(&args, &mut index, "--edge-relief-max-shadow")?,
                        "--edge-relief-max-shadow",
                    )?;
                }
                "--edge-relief-highlight" => {
                    terrain_lighting.edge_relief_highlight = parse_f32(
                        &next_arg(&args, &mut index, "--edge-relief-highlight")?,
                        "--edge-relief-highlight",
                    )?;
                }
                "--underwater-relief" => {
                    apply_underwater_relief_preset(
                        &mut terrain_lighting,
                        parse_terrain_lighting_preset(&next_arg(
                            &args,
                            &mut index,
                            "--underwater-relief",
                        )?)?,
                    );
                }
                "--underwater-relief-strength" => {
                    terrain_lighting.underwater_relief_strength = parse_f32(
                        &next_arg(&args, &mut index, "--underwater-relief-strength")?,
                        "--underwater-relief-strength",
                    )?;
                    terrain_lighting.underwater_relief_enabled =
                        terrain_lighting.underwater_relief_strength > 0.0;
                }
                "--underwater-depth-fade" => {
                    terrain_lighting.underwater_depth_fade = parse_f32(
                        &next_arg(&args, &mut index, "--underwater-depth-fade")?,
                        "--underwater-depth-fade",
                    )?;
                }
                "--underwater-min-light" => {
                    terrain_lighting.underwater_min_light = parse_f32(
                        &next_arg(&args, &mut index, "--underwater-min-light")?,
                        "--underwater-min-light",
                    )?;
                }
                "--block-boundaries" => {
                    block_boundaries.enabled =
                        parse_bool(&next_arg(&args, &mut index, "--block-boundaries")?)?;
                }
                "--block-boundary-strength" => {
                    block_boundaries.strength = parse_f32(
                        &next_arg(&args, &mut index, "--block-boundary-strength")?,
                        "--block-boundary-strength",
                    )?;
                }
                "--block-boundary-flat-strength" => {
                    block_boundaries.flat_strength = parse_f32(
                        &next_arg(&args, &mut index, "--block-boundary-flat-strength")?,
                        "--block-boundary-flat-strength",
                    )?;
                }
                "--block-boundary-threshold" => {
                    block_boundaries.height_threshold = parse_f32(
                        &next_arg(&args, &mut index, "--block-boundary-threshold")?,
                        "--block-boundary-threshold",
                    )?;
                }
                "--block-boundary-max-shadow" => {
                    block_boundaries.max_shadow = parse_f32(
                        &next_arg(&args, &mut index, "--block-boundary-max-shadow")?,
                        "--block-boundary-max-shadow",
                    )?;
                }
                "--block-boundary-line-width" => {
                    block_boundaries.line_width_pixels = parse_f32(
                        &next_arg(&args, &mut index, "--block-boundary-line-width")?,
                        "--block-boundary-line-width",
                    )?;
                }
                "--region" => {
                    region = Some(parse_region(&next_arg(&args, &mut index, "--region")?)?);
                }
                "--threads" => {
                    threading = parse_threads(&next_arg(&args, &mut index, "--threads")?)?;
                }
                "--profile" => {
                    profile = parse_profile(&next_arg(&args, &mut index, "--profile")?)?;
                }
                "--memory-budget" => {
                    memory_budget =
                        parse_memory_budget(&next_arg(&args, &mut index, "--memory-budget")?)?;
                }
                "--pipeline-depth" => {
                    pipeline_depth = parse_positive_usize(
                        &next_arg(&args, &mut index, "--pipeline-depth")?,
                        "--pipeline-depth",
                    )?;
                }
                "--stats" => {
                    print_stats = true;
                }
                "--palette-json" => {
                    palette_json_paths.push(PathBuf::from(next_arg(
                        &args,
                        &mut index,
                        "--palette-json",
                    )?));
                }
                "--plugin-data" => {
                    plugin_data_paths.push(PathBuf::from(next_arg(
                        &args,
                        &mut index,
                        "--plugin-data",
                    )?));
                }
                "--viewer-prefetch-radius" => {
                    viewer_prefetch_radius = parse_nonnegative_usize(
                        &next_arg(&args, &mut index, "--viewer-prefetch-radius")?,
                        "--viewer-prefetch-radius",
                    )?;
                }
                "--viewer-retain-radius" => {
                    viewer_retain_radius = parse_nonnegative_usize(
                        &next_arg(&args, &mut index, "--viewer-retain-radius")?,
                        "--viewer-retain-radius",
                    )?;
                }
                "--viewer-max-image-loads" => {
                    viewer_max_image_loads = parse_positive_usize(
                        &next_arg(&args, &mut index, "--viewer-max-image-loads")?,
                        "--viewer-max-image-loads",
                    )?;
                }
                "--viewer-refresh-ms" => {
                    viewer_refresh_ms = parse_positive_u64(
                        &next_arg(&args, &mut index, "--viewer-refresh-ms")?,
                        "--viewer-refresh-ms",
                    )?;
                }
                "--max-render-threads" => {
                    max_render_threads = Some(parse_positive_usize(
                        &next_arg(&args, &mut index, "--max-render-threads")?,
                        "--max-render-threads",
                    )?);
                }
                "--reserve-threads" => {
                    reserve_threads = parse_nonnegative_usize(
                        &next_arg(&args, &mut index, "--reserve-threads")?,
                        "--reserve-threads",
                    )?;
                }
                "--tile-batch-size" => {
                    tile_batch_size = parse_optional_usize(
                        &next_arg(&args, &mut index, "--tile-batch-size")?,
                        "--tile-batch-size",
                    )?;
                }
                "--writer-threads" => {
                    writer_threads = parse_positive_usize(
                        &next_arg(&args, &mut index, "--writer-threads")?,
                        "--writer-threads",
                    )?;
                    if writer_threads > bedrock_render::MAX_RENDER_THREADS {
                        return Err(bedrock_render::BedrockRenderError::Validation(
                            "--writer-threads must be in 1..=512".to_string(),
                        ));
                    }
                }
                "--write-queue-capacity" => {
                    write_queue_capacity = parse_positive_usize(
                        &next_arg(&args, &mut index, "--write-queue-capacity")?,
                        "--write-queue-capacity",
                    )?;
                }
                "--parallel-dimensions" => {
                    parallel_dimensions = true;
                }
                "--no-parallel-dimensions" => {
                    parallel_dimensions = false;
                }
                "--cache-validation" => {
                    render_cache_validation = true;
                }
                "--no-cache-validation" => {
                    render_cache_validation = false;
                }
                "--force" => {
                    force = true;
                }
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                other => {
                    return Err(bedrock_render::BedrockRenderError::Validation(format!(
                        "unknown argument: {other}"
                    )));
                }
            }
            index += 1;
        }

        if !world_path.join("db").join("CURRENT").exists()
            && !world_path.join("chunks.dat").exists()
        {
            return Err(bedrock_render::BedrockRenderError::Validation(format!(
                "world storage not found: expected {} or {}",
                world_path.join("db").join("CURRENT").display(),
                world_path.join("chunks.dat").display()
            )));
        }
        let pixels_per_block = resolve_pixels_per_block(
            chunks_per_tile,
            blocks_per_pixel,
            pixels_per_block,
            tile_size_pixels,
        )?;
        let layout = RenderLayout {
            chunks_per_tile,
            blocks_per_pixel,
            pixels_per_block,
        };
        if layout.tile_size().is_none() {
            return Err(bedrock_render::BedrockRenderError::Validation(format!(
                "chunks_per_tile * 16 * pixels_per_block must be divisible by blocks_per_pixel and produce a tile <= {}px",
                bedrock_render::MAX_TILE_SIZE_PIXELS
            )));
        }
        if viewer_retain_radius < viewer_prefetch_radius {
            return Err(bedrock_render::BedrockRenderError::Validation(
                "--viewer-retain-radius must be >= --viewer-prefetch-radius".to_string(),
            ));
        }
        if viewer_max_image_loads > bedrock_render::MAX_RENDER_THREADS {
            return Err(bedrock_render::BedrockRenderError::Validation(
                "--viewer-max-image-loads must be in 1..=512".to_string(),
            ));
        }
        if let Some(max_render_threads) = max_render_threads
            && max_render_threads > bedrock_render::MAX_RENDER_THREADS
        {
            return Err(bedrock_render::BedrockRenderError::Validation(
                "--max-render-threads must be in 1..=512".to_string(),
            ));
        }
        let region_layout = RegionLayout { chunks_per_region };
        region_layout.validate()?;
        let surface_options = SurfaceRenderOptions {
            height_shading: terrain_lighting.enabled,
            lighting: terrain_lighting,
            block_boundaries,
            ..SurfaceRenderOptions::default()
        };
        let cache_signature = cache_signature(
            &world_path,
            layout,
            region_layout,
            surface_options,
            backend,
            &palette_json_paths,
            None,
        )?;
        Ok(Self {
            world_path,
            output_dir,
            dimensions,
            modes,
            y_layer,
            cave_y,
            layout,
            region_layout,
            surface_options,
            region,
            threading,
            backend,
            profile,
            memory_budget,
            pipeline_depth,
            print_stats,
            palette_json_paths,
            plugin_data_paths,
            viewer_prefetch_radius,
            viewer_retain_radius,
            viewer_max_image_loads,
            viewer_refresh_ms,
            max_render_threads,
            reserve_threads,
            tile_batch_size,
            writer_threads,
            write_queue_capacity,
            parallel_dimensions,
            render_cache_validation,
            cache_signature,
            force,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WebRenderMode {
    Surface,
    HeightMap,
    RawHeightMap,
    Biome,
    Layer,
    Cave,
}

impl WebRenderMode {
    fn to_render_mode(self, y_layer: i32, cave_y: i32) -> RenderMode {
        match self {
            Self::Surface => RenderMode::SurfaceBlocks,
            Self::HeightMap => RenderMode::HeightMap,
            Self::RawHeightMap => RenderMode::RawHeightMap,
            Self::Biome => RenderMode::Biome { y: y_layer },
            Self::Layer => RenderMode::LayerBlocks { y: y_layer },
            Self::Cave => RenderMode::CaveSlice { y: cave_y },
        }
    }
}

#[derive(Debug, Clone)]
struct RenderedDimension {
    dimension: Dimension,
    bounds: Option<ChunkBounds>,
    tile_coords: Vec<(i32, i32)>,
    tile_chunks: BTreeMap<(i32, i32), Vec<ChunkPos>>,
    modes: Vec<RenderedMode>,
}

#[derive(Debug, Clone)]
struct RenderedMode {
    mode: RenderMode,
    tile_count: usize,
    rendered: bool,
    diagnostics: RenderDiagnostics,
    stats: RenderPipelineStats,
}

#[derive(Debug, Clone)]
struct RenderModeTask {
    dimension_index: usize,
    mode_index: usize,
    dimension: Dimension,
    mode: RenderMode,
    bounds: Option<ChunkBounds>,
    tile_coords: Vec<(i32, i32)>,
    tile_chunks: BTreeMap<(i32, i32), Vec<ChunkPos>>,
    tile_count: usize,
}

struct TileWrite {
    path: PathBuf,
    encoded: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
struct DiscoveredDimension {
    bounds: Option<ChunkBounds>,
    tile_coords: Vec<(i32, i32)>,
    tile_chunks: BTreeMap<(i32, i32), Vec<ChunkPos>>,
}

fn initial_layout_dimensions(config: &WebMapConfig) -> Vec<RenderedDimension> {
    config
        .dimensions
        .iter()
        .map(|dimension| plan_dimension_layout(config, *dimension, DiscoveredDimension::default()))
        .collect()
}

fn plan_dimension_layout(
    config: &WebMapConfig,
    dimension: Dimension,
    discovered: DiscoveredDimension,
) -> RenderedDimension {
    let mut modes = Vec::with_capacity(config.modes.len());
    for mode in &config.modes {
        let mode = mode.to_render_mode(config.y_layer, config.cave_y);
        let tile_count = planned_tile_count(&discovered.tile_coords);
        modes.push(RenderedMode {
            mode,
            tile_count,
            rendered: false,
            diagnostics: RenderDiagnostics::default(),
            stats: RenderPipelineStats {
                planned_tiles: tile_count,
                ..RenderPipelineStats::default()
            },
        });
    }
    RenderedDimension {
        dimension,
        bounds: discovered.bounds,
        tile_coords: discovered.tile_coords,
        tile_chunks: discovered.tile_chunks,
        modes,
    }
}

fn planned_tile_count(tile_coords: &[(i32, i32)]) -> usize {
    tile_coords.len()
}

// Parallel export coordination is intentionally kept together for benchmark readability.
#[allow(clippy::too_many_lines)]
fn render_dimensions_parallel(
    renderer: &MapRenderer,
    config: &WebMapConfig,
    cache_valid: bool,
    dimensions: &mut [RenderedDimension],
) -> bedrock_render::Result<()> {
    let mut tasks = Vec::new();
    for (dimension_index, dimension) in dimensions.iter().enumerate() {
        for (mode_index, mode) in config.modes.iter().enumerate() {
            let mode = mode.to_render_mode(config.y_layer, config.cave_y);
            tasks.push(RenderModeTask {
                dimension_index,
                mode_index,
                dimension: dimension.dimension,
                mode,
                bounds: dimension.bounds,
                tile_coords: dimension.tile_coords.clone(),
                tile_chunks: dimension.tile_chunks.clone(),
                tile_count: dimension.tile_coords.len(),
            });
        }
    }
    tasks.sort_by(|left, right| {
        right
            .tile_count
            .cmp(&left.tile_count)
            .then_with(|| left.dimension_index.cmp(&right.dimension_index))
            .then_with(|| left.mode_index.cmp(&right.mode_index))
    });
    let total_work_items = tasks
        .iter()
        .fold(0usize, |total, task| total.saturating_add(task.tile_count))
        .max(tasks.len())
        .max(1);
    let total_threads = resolve_render_threads(config, total_work_items)?;
    let active_tasks = tasks
        .len()
        .min(config.dimensions.len().max(1))
        .min(total_threads)
        .max(1);
    let task_threads = total_threads.saturating_div(active_tasks).max(1);
    if config.print_stats {
        println!(
            "scheduler global_mode_queue tasks={} thread_budget={} active_tasks={} task_threads={} policy=largest-first",
            tasks.len(),
            total_threads,
            active_tasks,
            task_threads
        );
    }
    let started = std::time::Instant::now();
    let task_count = tasks.len();
    let queue = Arc::new(Mutex::new(VecDeque::from(tasks)));
    let (sender, receiver) =
        mpsc::channel::<bedrock_render::Result<(usize, usize, RenderedMode)>>();
    thread::scope(|scope| -> bedrock_render::Result<()> {
        for worker_index in 0..active_tasks {
            let queue = Arc::clone(&queue);
            let sender = sender.clone();
            let renderer = renderer.clone();
            let config = config.clone();
            scope.spawn(move || {
                loop {
                    let task = if let Ok(mut queue) = queue.lock() {
                        queue.pop_front()
                    } else {
                        let send_result =
                            sender.send(Err(bedrock_render::BedrockRenderError::Validation(
                                "render task queue lock was poisoned".to_string(),
                            )));
                        drop(send_result);
                        return;
                    };
                    let Some(task) = task else {
                        return;
                    };
                    if config.print_stats {
                        println!(
                            "scheduler worker={} task_start {} {} tiles={} task_threads={}",
                            worker_index + 1,
                            dimension_slug(task.dimension),
                            mode_slug(task.mode),
                            task.tile_count,
                            task_threads
                        );
                    }
                    let mut task_config = config.clone();
                    task_config.threading = threading_for_threads(task_threads);
                    let result = render_dimension_mode(
                        &renderer,
                        &task_config,
                        cache_valid,
                        task.dimension,
                        task.mode,
                        task.bounds,
                        &task.tile_coords,
                        &task.tile_chunks,
                    )
                    .map(|mode| (task.dimension_index, task.mode_index, mode));
                    if sender.send(result).is_err() {
                        return;
                    }
                }
            });
        }
        drop(sender);
        for _ in 0..task_count {
            let (dimension_index, mode_index, mode) = receiver.recv().map_err(|_| {
                bedrock_render::BedrockRenderError::Join(
                    "render task worker stopped unexpectedly".to_string(),
                )
            })??;
            if let Some(dimension) = dimensions.get_mut(dimension_index)
                && let Some(mode_slot) = dimension.modes.get_mut(mode_index)
            {
                *mode_slot = mode;
            }
            write_map_layout(config, dimensions)?;
        }
        Ok(())
    })?;
    if config.print_stats {
        println!(
            "scheduler global_mode_queue complete tasks={} elapsed_ms={} peak_worker_threads={} active_tasks_peak={}",
            config.dimensions.len().saturating_mul(config.modes.len()),
            started.elapsed().as_millis(),
            total_threads,
            active_tasks
        );
    }
    Ok(())
}

fn render_dimension_modes(
    renderer: &MapRenderer,
    config: &WebMapConfig,
    cache_valid: bool,
    dimension_index: usize,
    dimensions: &mut [RenderedDimension],
) -> bedrock_render::Result<()> {
    let dimension = dimensions[dimension_index].dimension;
    let bounds = dimensions[dimension_index].bounds;
    let tile_coords = dimensions[dimension_index].tile_coords.clone();
    let tile_chunks = dimensions[dimension_index].tile_chunks.clone();
    dimensions[dimension_index].modes = render_dimension_mode_list(
        renderer,
        config,
        cache_valid,
        dimension,
        bounds,
        &tile_coords,
        &tile_chunks,
    )?;
    write_map_layout(config, dimensions)
}

fn render_dimension_mode_list(
    renderer: &MapRenderer,
    config: &WebMapConfig,
    cache_valid: bool,
    dimension: Dimension,
    bounds: Option<ChunkBounds>,
    tile_coords: &[(i32, i32)],
    tile_chunks: &BTreeMap<(i32, i32), Vec<ChunkPos>>,
) -> bedrock_render::Result<Vec<RenderedMode>> {
    let mut rendered_modes = Vec::with_capacity(config.modes.len());
    for mode in &config.modes {
        let mode = mode.to_render_mode(config.y_layer, config.cave_y);
        rendered_modes.push(render_dimension_mode(
            renderer,
            config,
            cache_valid,
            dimension,
            mode,
            bounds,
            tile_coords,
            tile_chunks,
        )?);
    }
    Ok(rendered_modes)
}

// The arguments mirror independent render dimensions, avoiding another config object for the example.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn render_dimension_mode(
    renderer: &MapRenderer,
    config: &WebMapConfig,
    cache_valid: bool,
    dimension: Dimension,
    mode: RenderMode,
    bounds: Option<ChunkBounds>,
    tile_coords: &[(i32, i32)],
    tile_chunks: &BTreeMap<(i32, i32), Vec<ChunkPos>>,
) -> bedrock_render::Result<RenderedMode> {
    let Some(_bounds) = bounds else {
        return Ok(RenderedMode {
            mode,
            tile_count: 0,
            rendered: true,
            diagnostics: RenderDiagnostics::default(),
            stats: RenderPipelineStats::default(),
        });
    };
    if tile_coords.is_empty() {
        return Ok(RenderedMode {
            mode,
            tile_count: 0,
            rendered: true,
            diagnostics: RenderDiagnostics::default(),
            stats: RenderPipelineStats::default(),
        });
    }
    let planned_tiles =
        planned_tiles_for_coords(dimension, mode, config.layout, tile_coords, tile_chunks)?;
    if config.render_cache_validation
        && cache_valid
        && all_tiles_present(config, dimension, mode, &planned_tiles)
    {
        let stats = RenderPipelineStats {
            planned_tiles: planned_tiles.len(),
            cache_hits: planned_tiles.len(),
            ..RenderPipelineStats::default()
        };
        println!(
            "{} {} tiles={} cache=hit",
            dimension_slug(dimension),
            mode_slug(mode),
            planned_tiles.len()
        );
        return Ok(RenderedMode {
            mode,
            tile_count: planned_tiles.len(),
            rendered: true,
            diagnostics: RenderDiagnostics {
                cache_hits: planned_tiles.len(),
                ..RenderDiagnostics::default()
            },
            stats,
        });
    }
    let diagnostics = Arc::new(Mutex::new(RenderDiagnostics::default()));
    let total_tiles = planned_tiles.len();
    let resolved_threads = resolve_render_threads(config, total_tiles)?;
    let _batch_size = config
        .tile_batch_size
        .unwrap_or_else(|| resolved_threads.saturating_mul(4).max(32))
        .min(total_tiles.max(1));
    let write_cancelled = Arc::new(AtomicBool::new(false));
    let write_ms = Arc::new(AtomicU64::new(0));
    let queue_capacity = if config.pipeline_depth > 0 {
        config.pipeline_depth
    } else {
        config.write_queue_capacity
    };
    let (write_sender, write_receiver) = crossbeam_channel::bounded::<TileWrite>(queue_capacity);
    let (error_sender, error_receiver) =
        crossbeam_channel::bounded::<bedrock_render::BedrockRenderError>(config.writer_threads);
    let render_result = thread::scope(|scope| -> bedrock_render::Result<_> {
        for _ in 0..config.writer_threads {
            let write_receiver = write_receiver.clone();
            let error_sender = error_sender.clone();
            let write_cancelled = Arc::clone(&write_cancelled);
            let write_ms = Arc::clone(&write_ms);
            scope.spawn(move || {
                loop {
                    let Ok(message) = write_receiver.recv() else {
                        return;
                    };
                    let started = std::time::Instant::now();
                    if let Err(error) = write_tile(message) {
                        write_cancelled.store(true, Ordering::Relaxed);
                        let _ = error_sender.send(error);
                        return;
                    }
                    let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
                    write_ms.fetch_add(elapsed, Ordering::Relaxed);
                }
            });
        }
        drop(error_sender);
        let sink = RenderDiagnosticsSink::new({
            let diagnostics = Arc::clone(&diagnostics);
            move |value| {
                if let Ok(mut diagnostics) = diagnostics.lock() {
                    diagnostics.add(value);
                }
            }
        });
        let render_result = renderer.render_web_tiles_blocking(
            &planned_tiles,
            RenderOptions {
                format: ImageFormat::WebP,
                backend: config.backend,
                threading: threading_for_threads(resolved_threads),
                execution_profile: config.profile,
                memory_budget: config.memory_budget,
                pipeline_depth: config.pipeline_depth,
                diagnostics: Some(sink),
                surface: config.surface_options,
                region_layout: config.region_layout,
                ..RenderOptions::default()
            },
            |planned, tile| {
                if write_cancelled.load(Ordering::Relaxed) {
                    return Err(next_writer_error(&error_receiver));
                }
                let Some(encoded) = tile.encoded else {
                    return Err(bedrock_render::BedrockRenderError::Validation(
                        "webp encoder did not return encoded bytes".to_string(),
                    ));
                };
                let path = tile_path(
                    config,
                    dimension,
                    mode,
                    planned.job.coord.x,
                    planned.job.coord.z,
                );
                write_sender
                    .send(TileWrite { path, encoded })
                    .map_err(|_| next_writer_error(&error_receiver))?;
                Ok(())
            },
        )?;
        drop(write_sender);
        Ok(render_result)
    });
    let mut render_result = render_result?;
    render_result.stats.write_ms = u128::from(write_ms.load(Ordering::Relaxed));
    if let Ok(error) = error_receiver.try_recv() {
        return Err(error);
    }
    let mut diagnostics = diagnostics
        .lock()
        .map_err(|_| {
            bedrock_render::BedrockRenderError::Validation(
                "render diagnostics lock was poisoned".to_string(),
            )
        })?
        .clone();
    diagnostics.add(render_result.diagnostics.clone());
    println!(
        "{} {} tiles={} missing={} transparent={} unknown={}",
        dimension_slug(dimension),
        mode_slug(mode),
        planned_tiles.len(),
        diagnostics.missing_chunks,
        diagnostics.transparent_pixels,
        diagnostics.unknown_blocks
    );
    let unknown_summary = format_unknown_blocks(&diagnostics, 8);
    if !unknown_summary.is_empty() {
        println!(
            "{} {} unknown_blocks_by_name={}",
            dimension_slug(dimension),
            mode_slug(mode),
            unknown_summary
        );
    }
    if config.print_stats {
        print_pipeline_stats(dimension, mode, &render_result.stats);
    }
    Ok(RenderedMode {
        mode,
        tile_count: planned_tiles.len(),
        rendered: true,
        diagnostics,
        stats: render_result.stats,
    })
}

fn all_tiles_present(
    config: &WebMapConfig,
    dimension: Dimension,
    mode: RenderMode,
    planned_tiles: &[bedrock_render::PlannedTile],
) -> bool {
    planned_tiles.iter().all(|planned| {
        let path = tile_path(
            config,
            dimension,
            mode,
            planned.job.coord.x,
            planned.job.coord.z,
        );
        fs::metadata(path).is_ok_and(|metadata| metadata.len() > 0)
    })
}

fn write_tile(message: TileWrite) -> bedrock_render::Result<()> {
    if let Some(parent) = message.path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            bedrock_render::BedrockRenderError::io(
                format!("failed to create tile directory {}", parent.display()),
                error,
            )
        })?;
    }
    fs::write(&message.path, message.encoded).map_err(|error| {
        bedrock_render::BedrockRenderError::io(
            format!("failed to write tile {}", message.path.display()),
            error,
        )
    })
}

fn next_writer_error(
    receiver: &crossbeam_channel::Receiver<bedrock_render::BedrockRenderError>,
) -> bedrock_render::BedrockRenderError {
    receiver.try_recv().unwrap_or_else(|_| {
        bedrock_render::BedrockRenderError::Join("tile writer stopped unexpectedly".to_string())
    })
}

fn format_unknown_blocks(diagnostics: &RenderDiagnostics, limit: usize) -> String {
    let mut entries = diagnostics
        .unknown_blocks_by_name
        .iter()
        .map(|(name, count)| (name.as_str(), *count))
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(right.0)));
    entries
        .into_iter()
        .take(limit)
        .map(|(name, count)| format!("{name}:{count}"))
        .collect::<Vec<_>>()
        .join(",")
}

fn print_pipeline_stats(dimension: Dimension, mode: RenderMode, stats: &RenderPipelineStats) {
    println!(
        "{} {} stats planned_tiles={} planned_regions={} unique_chunks={} baked_chunks={} baked_regions={} region_chunks_copied={} region_chunks_out_of_bounds={} tile_missing_region_samples={} cache_hits={} cache_misses={} region_hits={} region_misses={} bake_ms={} world_load_ms={} region_bake_ms={} region_copy_ms={} tile_compose_ms={} encode_ms={} write_ms={} idle_ms={} queue_wait_ms={} cpu_queue_wait_ms={} db_read_ms={} decode_ms={} peak_cache_bytes={} active_tasks_peak={} peak_worker_threads={} world_worker_threads={} backend={} cpu_tiles={}",
        dimension_slug(dimension),
        mode_slug(mode),
        stats.planned_tiles,
        stats.planned_regions,
        stats.unique_chunks,
        stats.baked_chunks,
        stats.baked_regions,
        stats.region_chunks_copied,
        stats.region_chunks_out_of_bounds,
        stats.tile_missing_region_samples,
        stats.cache_hits,
        stats.cache_misses,
        stats.region_cache_hits,
        stats.region_cache_misses,
        stats.bake_ms,
        stats.world_load_ms,
        stats.region_bake_ms,
        stats.region_copy_ms,
        stats.tile_compose_ms,
        stats.encode_ms,
        stats.write_ms,
        stats.worker_idle_ms,
        stats.queue_wait_ms,
        stats.cpu_queue_wait_ms,
        stats.db_read_ms,
        stats.decode_ms,
        stats.peak_cache_bytes,
        stats.active_tasks_peak,
        stats.peak_worker_threads,
        stats.world_worker_threads,
        stats.resolved_backend.label(),
        stats.cpu_tiles
    );
}

fn resolve_render_threads(
    config: &WebMapConfig,
    work_items: usize,
) -> bedrock_render::Result<usize> {
    config.threading.resolve_for_profile_with_limits(
        config.profile,
        work_items.max(1),
        config.max_render_threads,
        config.reserve_threads,
    )
}

fn render_thread_limit(config: &WebMapConfig) -> usize {
    let mut allowed = bedrock_render::MAX_RENDER_THREADS;
    if let Some(max_render_threads) = config.max_render_threads {
        allowed = allowed.min(max_render_threads);
    }
    if config.reserve_threads > 0 {
        let available = std::thread::available_parallelism().map_or(1, usize::from);
        allowed = allowed.min(available.saturating_sub(config.reserve_threads).max(1));
    }
    allowed.max(1)
}

fn threading_for_threads(threads: usize) -> RenderThreadingOptions {
    if threads <= 1 {
        RenderThreadingOptions::Single
    } else {
        RenderThreadingOptions::Fixed(threads)
    }
}

fn world_scan_threading(config: &WebMapConfig) -> WorldThreadingOptions {
    let available = std::thread::available_parallelism().map_or(1, usize::from);
    let threads = available.min(render_thread_limit(config)).max(1);
    if threads <= 1 {
        WorldThreadingOptions::Single
    } else {
        WorldThreadingOptions::Fixed(threads)
    }
}

fn discover_dimension_tiles(
    world: &BedrockWorld,
    config: &WebMapConfig,
) -> bedrock_render::Result<BTreeMap<Dimension, DiscoveredDimension>> {
    let mut dimensions = config
        .dimensions
        .iter()
        .copied()
        .map(|dimension| (dimension, DiscoveredDimension::default()))
        .collect::<BTreeMap<_, _>>();
    let mut tile_chunks = config
        .dimensions
        .iter()
        .copied()
        .map(|dimension| (dimension, BTreeMap::new()))
        .collect::<BTreeMap<_, _>>();
    let chunks_per_tile = i32::try_from(config.layout.chunks_per_tile).map_err(|_| {
        bedrock_render::BedrockRenderError::Validation(
            "chunks_per_tile is outside i32 range".to_string(),
        )
    })?;
    let positions = world
        .list_render_chunk_positions_blocking(WorldScanOptions {
            threading: world_scan_threading(config),
            ..WorldScanOptions::default()
        })
        .map_err(bedrock_render::BedrockRenderError::World)?;
    for pos in positions {
        let Some(discovered) = dimensions.get_mut(&pos.dimension) else {
            continue;
        };
        if !chunk_in_region(pos, config.region) {
            continue;
        }
        include_chunk_bounds(&mut discovered.bounds, pos);
        if let Some(tile_chunks) = tile_chunks.get_mut(&pos.dimension) {
            let tile_coord = (
                pos.x.div_euclid(chunks_per_tile),
                pos.z.div_euclid(chunks_per_tile),
            );
            tile_chunks
                .entry(tile_coord)
                .or_insert_with(Vec::new)
                .push(pos);
        }
    }
    for (dimension, discovered) in &mut dimensions {
        discovered.tile_chunks = tile_chunks.remove(dimension).unwrap_or_default();
        for chunks in discovered.tile_chunks.values_mut() {
            chunks.sort();
            chunks.dedup();
        }
        discovered.tile_coords = discovered.tile_chunks.keys().copied().collect::<Vec<_>>();
        sort_tile_coords_for_rendering(discovered);
    }
    Ok(dimensions)
}

fn sort_tile_coords_for_rendering(discovered: &mut DiscoveredDimension) {
    let Some(bounds) = discovered.bounds else {
        return;
    };
    let center_x = (i64::from(bounds.min_chunk_x) + i64::from(bounds.max_chunk_x)) / 32;
    let center_z = (i64::from(bounds.min_chunk_z) + i64::from(bounds.max_chunk_z)) / 32;
    discovered.tile_coords.sort_by_key(|(tile_x, tile_z)| {
        let dx = i64::from(*tile_x) - center_x;
        let dz = i64::from(*tile_z) - center_z;
        (
            dx.saturating_mul(dx).saturating_add(dz.saturating_mul(dz)),
            *tile_z,
            *tile_x,
        )
    });
}

fn chunk_in_region(pos: ChunkPos, region: Option<(i32, i32, i32, i32)>) -> bool {
    let Some((min_chunk_x, min_chunk_z, max_chunk_x, max_chunk_z)) = region else {
        return true;
    };
    pos.x >= min_chunk_x && pos.x <= max_chunk_x && pos.z >= min_chunk_z && pos.z <= max_chunk_z
}

fn include_chunk_bounds(bounds: &mut Option<ChunkBounds>, pos: ChunkPos) {
    match bounds {
        Some(bounds) => {
            bounds.min_chunk_x = bounds.min_chunk_x.min(pos.x);
            bounds.min_chunk_z = bounds.min_chunk_z.min(pos.z);
            bounds.max_chunk_x = bounds.max_chunk_x.max(pos.x);
            bounds.max_chunk_z = bounds.max_chunk_z.max(pos.z);
            bounds.chunk_count = bounds.chunk_count.saturating_add(1);
        }
        None => {
            *bounds = Some(ChunkBounds {
                dimension: pos.dimension,
                min_chunk_x: pos.x,
                min_chunk_z: pos.z,
                max_chunk_x: pos.x,
                max_chunk_z: pos.z,
                chunk_count: 1,
            });
        }
    }
}

fn planned_tiles_for_coords(
    dimension: Dimension,
    mode: RenderMode,
    layout: RenderLayout,
    tile_coords: &[(i32, i32)],
    tile_chunks: &BTreeMap<(i32, i32), Vec<ChunkPos>>,
) -> bedrock_render::Result<Vec<PlannedTile>> {
    tile_coords
        .iter()
        .copied()
        .map(|(tile_x, tile_z)| {
            let chunks = tile_chunks
                .get(&(tile_x, tile_z))
                .cloned()
                .unwrap_or_default();
            planned_tile_for_coord(dimension, mode, layout, tile_x, tile_z, chunks)
        })
        .collect()
}

fn planned_tile_for_coord(
    dimension: Dimension,
    mode: RenderMode,
    layout: RenderLayout,
    tile_x: i32,
    tile_z: i32,
    chunk_positions: Vec<ChunkPos>,
) -> bedrock_render::Result<PlannedTile> {
    let job = RenderJob::chunk_tile(
        TileCoord {
            x: tile_x,
            z: tile_z,
            dimension,
        },
        mode,
        layout,
    )?;
    let chunks_per_tile = i32::try_from(layout.chunks_per_tile).map_err(|_| {
        bedrock_render::BedrockRenderError::Validation(
            "chunks_per_tile is outside i32 range".to_string(),
        )
    })?;
    let min_chunk_x = tile_x.checked_mul(chunks_per_tile).ok_or_else(|| {
        bedrock_render::BedrockRenderError::Validation("tile x span overflow".to_string())
    })?;
    let min_chunk_z = tile_z.checked_mul(chunks_per_tile).ok_or_else(|| {
        bedrock_render::BedrockRenderError::Validation("tile z span overflow".to_string())
    })?;
    let max_chunk_x = min_chunk_x
        .checked_add(chunks_per_tile.saturating_sub(1))
        .ok_or_else(|| {
            bedrock_render::BedrockRenderError::Validation("tile x extent overflow".to_string())
        })?;
    let max_chunk_z = min_chunk_z
        .checked_add(chunks_per_tile.saturating_sub(1))
        .ok_or_else(|| {
            bedrock_render::BedrockRenderError::Validation("tile z extent overflow".to_string())
        })?;
    Ok(PlannedTile {
        job,
        region: ChunkRegion::new(
            dimension,
            min_chunk_x,
            min_chunk_z,
            max_chunk_x,
            max_chunk_z,
        ),
        layout,
        chunk_positions: Some(chunk_positions.into()),
    })
}

// The static viewer payload is written in one pass so the generated file stays easy to audit.
#[allow(clippy::too_many_lines)]
fn write_map_layout(
    config: &WebMapConfig,
    dimensions: &[RenderedDimension],
) -> bedrock_render::Result<()> {
    let mut json = String::new();
    json.push_str("{\n");
    write_json_field(&mut json, 1, "tileFormat", "webp", true);
    write_json_field(&mut json, 1, "layout", &layout_slug(config.layout), true);
    write_json_number(
        &mut json,
        1,
        "chunksPerTile",
        config.layout.chunks_per_tile,
        true,
    );
    write_json_number(
        &mut json,
        1,
        "blocksPerPixel",
        config.layout.blocks_per_pixel,
        true,
    );
    write_json_number(
        &mut json,
        1,
        "pixelsPerBlock",
        config.layout.pixels_per_block,
        true,
    );
    write_json_number(
        &mut json,
        1,
        "chunksPerRegion",
        config.region_layout.chunks_per_region,
        true,
    );
    write_json_number(
        &mut json,
        1,
        "tileSize",
        config.layout.tile_size().unwrap_or(256),
        true,
    );
    json.push_str("  \"viewer\": {");
    let _ = write!(
        json,
        "\"prefetchRadius\":{},\"retainRadius\":{},\"maxImageLoads\":{},\"refreshMs\":{}",
        config.viewer_prefetch_radius,
        config.viewer_retain_radius,
        config.viewer_max_image_loads,
        config.viewer_refresh_ms
    );
    json.push_str("},\n");
    json.push_str("  \"dimensions\": [\n");
    for (dimension_index, dimension) in dimensions.iter().enumerate() {
        json.push_str("    {\n");
        write_json_field(
            &mut json,
            3,
            "id",
            &dimension_slug(dimension.dimension),
            true,
        );
        write_json_field(
            &mut json,
            3,
            "label",
            &dimension_label(dimension.dimension),
            true,
        );
        if let Some(bounds) = dimension.bounds {
            json.push_str("      \"bounds\": {");
            let _ = write!(
                json,
                "\"minChunkX\":{},\"minChunkZ\":{},\"maxChunkX\":{},\"maxChunkZ\":{}",
                bounds.min_chunk_x, bounds.min_chunk_z, bounds.max_chunk_x, bounds.max_chunk_z
            );
            json.push_str("},\n");
        } else {
            json.push_str("      \"bounds\": null,\n");
        }
        json.push_str("      \"modes\": [\n");
        for (mode_index, mode) in dimension.modes.iter().enumerate() {
            json.push_str("        {");
            let _ = write!(
                json,
                "\"id\":\"{}\",\"label\":\"{}\",\"tiles\":{},\"rendered\":{},\"missingChunks\":{},\"transparentPixels\":{},\"unknownBlocks\":{},\"plannedRegions\":{},\"uniqueChunks\":{},\"bakedChunks\":{},\"bakedRegions\":{},\"regionChunksCopied\":{},\"regionChunksOutOfBounds\":{},\"tileMissingRegionSamples\":{},\"worldLoadMs\":{},\"regionBakeMs\":{},\"regionCopyMs\":{},\"tileComposeMs\":{},\"bakeMs\":{},\"encodeMs\":{},\"dbReadMs\":{},\"decodeMs\":{},\"peakCacheBytes\":{},\"activeTasksPeak\":{},\"peakWorkerThreads\":{},\"worldWorkerThreads\":{},\"backend\":\"{}\",\"cpuTiles\":{}",
                mode_slug(mode.mode),
                mode_label(mode.mode),
                mode.tile_count,
                mode.rendered,
                mode.diagnostics.missing_chunks,
                mode.diagnostics.transparent_pixels,
                mode.diagnostics.unknown_blocks,
                mode.stats.planned_regions,
                mode.stats.unique_chunks,
                mode.stats.baked_chunks,
                mode.stats.baked_regions,
                mode.stats.region_chunks_copied,
                mode.stats.region_chunks_out_of_bounds,
                mode.stats.tile_missing_region_samples,
                mode.stats.world_load_ms,
                mode.stats.region_bake_ms,
                mode.stats.region_copy_ms,
                mode.stats.tile_compose_ms,
                mode.stats.bake_ms,
                mode.stats.encode_ms,
                mode.stats.db_read_ms,
                mode.stats.decode_ms,
                mode.stats.peak_cache_bytes,
                mode.stats.active_tasks_peak,
                mode.stats.peak_worker_threads,
                mode.stats.world_worker_threads,
                mode.stats.resolved_backend.label(),
                mode.stats.cpu_tiles
            );
            json.push('}');
            if mode_index + 1 != dimension.modes.len() {
                json.push(',');
            }
            json.push('\n');
        }
        json.push_str("      ]\n");
        json.push_str("    }");
        if dimension_index + 1 != dimensions.len() {
            json.push(',');
        }
        json.push('\n');
    }
    json.push_str("  ]\n");
    json.push_str("}\n");
    fs::write(config.output_dir.join("map-layout.json"), &json).map_err(|error| {
        bedrock_render::BedrockRenderError::io("failed to write map-layout.json", error)
    })?;
    let mut map_data_js = String::new();
    map_data_js.push_str("(() => {\n");
    map_data_js.push_str("  const layout = ");
    map_data_js.push_str(json.trim_end());
    map_data_js.push_str(";\n");
    map_data_js.push_str(
        r"  function tileBounds(bounds) {
    if (!bounds) return null;
    return {
      minX: Math.floor(bounds.minChunkX / layout.chunksPerTile),
      maxX: Math.floor(bounds.maxChunkX / layout.chunksPerTile),
      minZ: Math.floor(bounds.minChunkZ / layout.chunksPerTile),
      maxZ: Math.floor(bounds.maxChunkZ / layout.chunksPerTile),
    };
  }

  function tileId(dimensionId, modeId, tileX, tileZ) {
    return `${dimensionId}/${modeId}/${layout.layout}/${tileZ}/${tileX}`;
  }

  function tilePath(dimensionId, modeId, tileX, tileZ) {
    return `tiles/${tileId(dimensionId, modeId, tileX, tileZ)}.${layout.tileFormat}`;
  }

  window.BEDROCK_WEB_MAP = {
    ...layout,
    tileBounds,
    tileId,
    tilePath,
  };
})();
",
    );
    fs::write(config.output_dir.join("map-data.js"), map_data_js).map_err(|error| {
        bedrock_render::BedrockRenderError::io("failed to write map-data.js", error)
    })?;
    Ok(())
}

fn write_tile_index(
    config: &WebMapConfig,
    dimensions: &[RenderedDimension],
) -> bedrock_render::Result<()> {
    let mut js = String::new();
    js.push_str("(() => {\n");
    js.push_str("  window.BEDROCK_WEB_TILE_INDEX = {\n");
    write_json_field(&mut js, 2, "layout", &layout_slug(config.layout), true);
    js.push_str("    \"dimensions\": [\n");
    for (dimension_index, dimension) in dimensions.iter().enumerate() {
        js.push_str("      {\n");
        write_json_field(&mut js, 4, "id", &dimension_slug(dimension.dimension), true);
        js.push_str("        \"tiles\": [");
        for (tile_index, (tile_x, tile_z)) in dimension.tile_coords.iter().enumerate() {
            let _ = write!(js, "[{tile_x},{tile_z}]");
            if tile_index + 1 != dimension.tile_coords.len() {
                js.push(',');
            }
        }
        js.push_str("]\n");
        js.push_str("      }");
        if dimension_index + 1 != dimensions.len() {
            js.push(',');
        }
        js.push('\n');
    }
    js.push_str("    ]\n");
    js.push_str("  };\n");
    js.push_str("})();\n");
    fs::write(config.output_dir.join("tile-index.js"), js).map_err(|error| {
        bedrock_render::BedrockRenderError::io("failed to write tile-index.js", error)
    })
}

fn write_viewer_html(config: &WebMapConfig) -> bedrock_render::Result<()> {
    write_viewer_assets(config)?;
    write_plugin_index(config)
}

fn write_viewer_assets(config: &WebMapConfig) -> bedrock_render::Result<()> {
    const VIEWER_HTML: &str = include_str!("web_map_assets/viewer.html");
    const VIEWER_CSS: &str = include_str!("web_map_assets/viewer.css");
    const VIEWER_JS: &str = include_str!("web_map_assets/viewer.js");
    const PLUGIN_RUNTIME_JS: &str = include_str!("web_map_assets/plugin-runtime.js");

    let assets_dir = config.output_dir.join("assets");
    fs::create_dir_all(&assets_dir).map_err(|error| {
        bedrock_render::BedrockRenderError::io(
            format!("failed to create asset directory {}", assets_dir.display()),
            error,
        )
    })?;
    fs::write(config.output_dir.join("viewer.html"), VIEWER_HTML).map_err(|error| {
        bedrock_render::BedrockRenderError::io("failed to write viewer.html", error)
    })?;
    fs::write(assets_dir.join("viewer.css"), VIEWER_CSS).map_err(|error| {
        bedrock_render::BedrockRenderError::io("failed to write viewer.css", error)
    })?;
    fs::write(assets_dir.join("viewer.js"), VIEWER_JS).map_err(|error| {
        bedrock_render::BedrockRenderError::io("failed to write viewer.js", error)
    })?;
    fs::write(assets_dir.join("plugin-runtime.js"), PLUGIN_RUNTIME_JS).map_err(|error| {
        bedrock_render::BedrockRenderError::io("failed to write plugin-runtime.js", error)
    })?;
    Ok(())
}

fn write_plugin_index(config: &WebMapConfig) -> bedrock_render::Result<()> {
    let plugins = load_plugin_data(&config.plugin_data_paths)?;
    let json = serde_json::to_string(&plugins).map_err(|error| {
        bedrock_render::BedrockRenderError::Validation(format!(
            "failed to encode plugin index: {error}"
        ))
    })?;
    let js = format!("window.BEDROCK_WEB_PLUGINS = {{\"plugins\":{json}}};\n");
    fs::write(config.output_dir.join("plugin-index.js"), js).map_err(|error| {
        bedrock_render::BedrockRenderError::io("failed to write plugin-index.js", error)
    })
}

fn load_plugin_data(paths: &[PathBuf]) -> bedrock_render::Result<Vec<serde_json::Value>> {
    let mut plugins = Vec::with_capacity(paths.len());
    for path in paths {
        let content = fs::read_to_string(path).map_err(|error| {
            bedrock_render::BedrockRenderError::io(
                format!("failed to read plugin data {}", path.display()),
                error,
            )
        })?;
        let value = serde_json::from_str::<serde_json::Value>(&content).map_err(|error| {
            bedrock_render::BedrockRenderError::Validation(format!(
                "invalid plugin JSON {}: {error}",
                path.display()
            ))
        })?;
        validate_plugin_data(&value, path)?;
        plugins.push(value);
    }
    Ok(plugins)
}

fn validate_plugin_data(
    value: &serde_json::Value,
    path: &std::path::Path,
) -> bedrock_render::Result<()> {
    let Some(object) = value.as_object() else {
        return Err(bedrock_render::BedrockRenderError::Validation(format!(
            "plugin data {} must be a JSON object",
            path.display()
        )));
    };
    for key in ["id", "label", "version"] {
        if !object.get(key).is_some_and(serde_json::Value::is_string) {
            return Err(bedrock_render::BedrockRenderError::Validation(format!(
                "plugin data {} requires string field `{key}`",
                path.display()
            )));
        }
    }
    let Some(items) = object.get("items").and_then(serde_json::Value::as_array) else {
        return Err(bedrock_render::BedrockRenderError::Validation(format!(
            "plugin data {} requires array field `items`",
            path.display()
        )));
    };
    for (item_index, item) in items.iter().enumerate() {
        validate_plugin_item(item, item_index, path)?;
    }
    Ok(())
}

fn validate_plugin_item(
    item: &serde_json::Value,
    item_index: usize,
    path: &std::path::Path,
) -> bedrock_render::Result<()> {
    let Some(object) = item.as_object() else {
        return Err(bedrock_render::BedrockRenderError::Validation(format!(
            "plugin item #{item_index} in {} must be a JSON object",
            path.display()
        )));
    };
    let Some(item_type) = object.get("type").and_then(serde_json::Value::as_str) else {
        return Err(bedrock_render::BedrockRenderError::Validation(format!(
            "plugin item #{item_index} in {} requires string field `type`",
            path.display()
        )));
    };
    if !matches!(item_type, "markers" | "chat" | "points" | "areas" | "panel") {
        return Err(bedrock_render::BedrockRenderError::Validation(format!(
            "plugin item #{item_index} in {} has unsupported type `{item_type}`",
            path.display()
        )));
    }
    if object
        .get("dimension")
        .and_then(serde_json::Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err(bedrock_render::BedrockRenderError::Validation(format!(
            "plugin item #{item_index} in {} requires string field `dimension`",
            path.display()
        )));
    }
    match item_type {
        "markers" | "points" => validate_point_payload(object, item_index, path),
        "areas" => validate_area_payload(object, item_index, path),
        "chat" => validate_chat_payload(object, item_index, path),
        "panel" => validate_panel_payload(object, item_index, path),
        _ => Ok(()),
    }
}

fn validate_point_payload(
    object: &serde_json::Map<String, serde_json::Value>,
    item_index: usize,
    path: &std::path::Path,
) -> bedrock_render::Result<()> {
    if object
        .get("entries")
        .is_some_and(serde_json::Value::is_array)
        || (object.get("x").is_some_and(serde_json::Value::is_number)
            && object.get("z").is_some_and(serde_json::Value::is_number))
    {
        return Ok(());
    }
    Err(bedrock_render::BedrockRenderError::Validation(format!(
        "plugin point item #{item_index} in {} requires numeric x/z or array entries",
        path.display()
    )))
}

fn validate_area_payload(
    object: &serde_json::Map<String, serde_json::Value>,
    item_index: usize,
    path: &std::path::Path,
) -> bedrock_render::Result<()> {
    if object
        .get("entries")
        .is_some_and(serde_json::Value::is_array)
        || object
            .get("bounds")
            .is_some_and(serde_json::Value::is_object)
    {
        return Ok(());
    }
    Err(bedrock_render::BedrockRenderError::Validation(format!(
        "plugin area item #{item_index} in {} requires object bounds or array entries",
        path.display()
    )))
}

fn validate_chat_payload(
    object: &serde_json::Map<String, serde_json::Value>,
    item_index: usize,
    path: &std::path::Path,
) -> bedrock_render::Result<()> {
    if object
        .get("messages")
        .is_some_and(serde_json::Value::is_array)
        || object
            .get("entries")
            .is_some_and(serde_json::Value::is_array)
        || object.get("text").is_some_and(serde_json::Value::is_string)
    {
        return Ok(());
    }
    Err(bedrock_render::BedrockRenderError::Validation(format!(
        "plugin chat item #{item_index} in {} requires messages, entries, or text",
        path.display()
    )))
}

fn validate_panel_payload(
    object: &serde_json::Map<String, serde_json::Value>,
    item_index: usize,
    path: &std::path::Path,
) -> bedrock_render::Result<()> {
    if object.get("text").is_some_and(serde_json::Value::is_string)
        || object
            .get("content")
            .is_some_and(serde_json::Value::is_string)
    {
        return Ok(());
    }
    Err(bedrock_render::BedrockRenderError::Validation(format!(
        "plugin panel item #{item_index} in {} requires text or content",
        path.display()
    )))
}

fn tile_path(
    config: &WebMapConfig,
    dimension: Dimension,
    mode: RenderMode,
    tile_x: i32,
    tile_z: i32,
) -> PathBuf {
    config
        .output_dir
        .join("tiles")
        .join(dimension_slug(dimension))
        .join(mode_slug(mode))
        .join(layout_slug(config.layout))
        .join(tile_z.to_string())
        .join(format!("{tile_x}.webp"))
}

fn next_arg(args: &[String], index: &mut usize, name: &str) -> bedrock_render::Result<String> {
    *index = index.saturating_add(1);
    args.get(*index).cloned().ok_or_else(|| {
        bedrock_render::BedrockRenderError::Validation(format!("{name} requires a value"))
    })
}

fn parse_dimensions(value: &str) -> bedrock_render::Result<Vec<Dimension>> {
    let mut dimensions = Vec::new();
    for item in value.split(',').filter(|item| !item.trim().is_empty()) {
        dimensions.push(match item.trim().to_ascii_lowercase().as_str() {
            "overworld" | "主世界" => Dimension::Overworld,
            "nether" | "下界" => Dimension::Nether,
            "end" | "the_end" | "末地" => Dimension::End,
            other => {
                let id = other.parse::<i32>().map_err(|error| {
                    bedrock_render::BedrockRenderError::Validation(format!(
                        "invalid dimension {other}: {error}"
                    ))
                })?;
                Dimension::Unknown(id)
            }
        });
    }
    Ok(dimensions)
}

fn parse_modes(value: &str) -> bedrock_render::Result<Vec<WebRenderMode>> {
    let mut modes = Vec::new();
    for item in value.split(',').filter(|item| !item.trim().is_empty()) {
        modes.push(match item.trim().to_ascii_lowercase().as_str() {
            "surface" => WebRenderMode::Surface,
            "heightmap" | "height" => WebRenderMode::HeightMap,
            "raw-heightmap" | "raw_heightmap" | "rawheightmap" | "raw-height" => {
                WebRenderMode::RawHeightMap
            }
            "biome" => WebRenderMode::Biome,
            "layer" => WebRenderMode::Layer,
            "cave" => WebRenderMode::Cave,
            other => {
                return Err(bedrock_render::BedrockRenderError::Validation(format!(
                    "invalid mode: {other}"
                )));
            }
        });
    }
    Ok(modes)
}

fn parse_i32(value: &str, name: &str) -> bedrock_render::Result<i32> {
    value.parse::<i32>().map_err(|error| {
        bedrock_render::BedrockRenderError::Validation(format!("invalid {name}: {error}"))
    })
}

fn parse_u32(value: &str, name: &str) -> bedrock_render::Result<u32> {
    let value = value.parse::<u32>().map_err(|error| {
        bedrock_render::BedrockRenderError::Validation(format!("invalid {name}: {error}"))
    })?;
    if value == 0 {
        return Err(bedrock_render::BedrockRenderError::Validation(format!(
            "{name} must be greater than zero"
        )));
    }
    Ok(value)
}

fn parse_bool(value: &str) -> bedrock_render::Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" | "enabled" => Ok(true),
        "0" | "false" | "no" | "off" | "disabled" => Ok(false),
        _ => Err(bedrock_render::BedrockRenderError::Validation(
            "boolean value must be on/off or true/false".to_string(),
        )),
    }
}

fn parse_f32(value: &str, name: &str) -> bedrock_render::Result<f32> {
    let parsed = value.parse::<f32>().map_err(|error| {
        bedrock_render::BedrockRenderError::Validation(format!("invalid {name}: {error}"))
    })?;
    if !parsed.is_finite() {
        return Err(bedrock_render::BedrockRenderError::Validation(format!(
            "{name} must be finite"
        )));
    }
    Ok(parsed)
}

fn parse_terrain_lighting(value: &str) -> bedrock_render::Result<TerrainLightingOptions> {
    Ok(TerrainLightingOptions::preset(
        parse_terrain_lighting_preset(value)?,
    ))
}

fn parse_terrain_lighting_preset(value: &str) -> bedrock_render::Result<TerrainLightingPreset> {
    let preset = match value.trim().to_ascii_lowercase().as_str() {
        "off" | "none" | "false" => TerrainLightingPreset::Off,
        "soft" | "on" | "true" => TerrainLightingPreset::Soft,
        "strong" => TerrainLightingPreset::Strong,
        other => {
            return Err(bedrock_render::BedrockRenderError::Validation(format!(
                "invalid --terrain-lighting: {other}"
            )));
        }
    };
    Ok(preset)
}

fn apply_underwater_relief_preset(
    lighting: &mut TerrainLightingOptions,
    preset: TerrainLightingPreset,
) {
    let preset_lighting = TerrainLightingOptions::preset(preset);
    lighting.underwater_relief_enabled = preset_lighting.underwater_relief_enabled;
    lighting.underwater_relief_strength = preset_lighting.underwater_relief_strength;
    lighting.underwater_depth_fade = preset_lighting.underwater_depth_fade;
    lighting.underwater_min_light = preset_lighting.underwater_min_light;
}

fn resolve_pixels_per_block(
    chunks_per_tile: u32,
    blocks_per_pixel: u32,
    pixels_per_block: Option<u32>,
    tile_size_pixels: Option<u32>,
) -> bedrock_render::Result<u32> {
    let Some(tile_size_pixels) = tile_size_pixels else {
        return Ok(pixels_per_block.unwrap_or(1));
    };
    let tile_blocks = u64::from(chunks_per_tile).checked_mul(16).ok_or_else(|| {
        bedrock_render::BedrockRenderError::Validation("tile block span overflow".to_string())
    })?;
    let requested_output_pixels = u64::from(tile_size_pixels)
        .checked_mul(u64::from(blocks_per_pixel))
        .ok_or_else(|| {
            bedrock_render::BedrockRenderError::Validation(
                "--tile-size-pixels is too large".to_string(),
            )
        })?;
    if requested_output_pixels % tile_blocks != 0 {
        return Err(bedrock_render::BedrockRenderError::Validation(format!(
            "--tile-size-pixels {tile_size_pixels} cannot be represented as an integer pixels_per_block for chunks_per_tile={chunks_per_tile} and blocks_per_pixel={blocks_per_pixel}"
        )));
    }
    let resolved = u32::try_from(requested_output_pixels / tile_blocks).map_err(|_| {
        bedrock_render::BedrockRenderError::Validation(
            "--tile-size-pixels produces a pixels_per_block value outside u32 range".to_string(),
        )
    })?;
    if resolved == 0 {
        return Err(bedrock_render::BedrockRenderError::Validation(
            "--tile-size-pixels is too small".to_string(),
        ));
    }
    if let Some(pixels_per_block) = pixels_per_block
        && pixels_per_block != resolved
    {
        return Err(bedrock_render::BedrockRenderError::Validation(format!(
            "--pixels-per-block {pixels_per_block} conflicts with --tile-size-pixels {tile_size_pixels}, which implies {resolved}"
        )));
    }
    Ok(resolved)
}

fn parse_threads(value: &str) -> bedrock_render::Result<RenderThreadingOptions> {
    if value.eq_ignore_ascii_case("auto") {
        return Ok(RenderThreadingOptions::Auto);
    }
    let threads = parse_positive_usize(value, "--threads")?;
    if threads > bedrock_render::MAX_RENDER_THREADS {
        return Err(bedrock_render::BedrockRenderError::Validation(
            "--threads must be in 1..=512 or auto".to_string(),
        ));
    }
    Ok(if threads == 1 {
        RenderThreadingOptions::Single
    } else {
        RenderThreadingOptions::Fixed(threads)
    })
}

fn parse_profile(value: &str) -> bedrock_render::Result<RenderExecutionProfile> {
    match value.to_ascii_lowercase().as_str() {
        "export" => Ok(RenderExecutionProfile::Export),
        "interactive" => Ok(RenderExecutionProfile::Interactive),
        _ => Err(bedrock_render::BedrockRenderError::Validation(
            "--profile must be export or interactive".to_string(),
        )),
    }
}

fn parse_memory_budget(value: &str) -> bedrock_render::Result<RenderMemoryBudget> {
    if value.eq_ignore_ascii_case("auto") {
        return Ok(RenderMemoryBudget::Auto);
    }
    if value.eq_ignore_ascii_case("disabled") {
        return Ok(RenderMemoryBudget::Disabled);
    }
    let mib = parse_positive_usize(value, "--memory-budget")?;
    let bytes = u64::try_from(mib)
        .ok()
        .and_then(|value| value.checked_mul(1024 * 1024))
        .ok_or_else(|| {
            bedrock_render::BedrockRenderError::Validation(
                "--memory-budget is too large".to_string(),
            )
        })?;
    Ok(RenderMemoryBudget::FixedBytes(bytes))
}

fn parse_optional_usize(value: &str, name: &str) -> bedrock_render::Result<Option<usize>> {
    if value.eq_ignore_ascii_case("auto") {
        return Ok(None);
    }
    parse_positive_usize(value, name).map(Some)
}

fn parse_nonnegative_usize(value: &str, name: &str) -> bedrock_render::Result<usize> {
    value.parse::<usize>().map_err(|error| {
        bedrock_render::BedrockRenderError::Validation(format!("invalid {name}: {error}"))
    })
}

fn parse_positive_usize(value: &str, name: &str) -> bedrock_render::Result<usize> {
    let value = value.parse::<usize>().map_err(|error| {
        bedrock_render::BedrockRenderError::Validation(format!("invalid {name}: {error}"))
    })?;
    if value == 0 {
        return Err(bedrock_render::BedrockRenderError::Validation(format!(
            "{name} must be greater than zero"
        )));
    }
    Ok(value)
}

fn parse_positive_u64(value: &str, name: &str) -> bedrock_render::Result<u64> {
    let value = value.parse::<u64>().map_err(|error| {
        bedrock_render::BedrockRenderError::Validation(format!("invalid {name}: {error}"))
    })?;
    if value == 0 {
        return Err(bedrock_render::BedrockRenderError::Validation(format!(
            "{name} must be greater than zero"
        )));
    }
    Ok(value)
}

fn parse_region(value: &str) -> bedrock_render::Result<(i32, i32, i32, i32)> {
    let parts = value
        .split(',')
        .map(str::trim)
        .map(|part| parse_i32(part, "--region"))
        .collect::<bedrock_render::Result<Vec<_>>>()?;
    let [min_x, min_z, max_x, max_z]: [i32; 4] = parts.try_into().map_err(|_| {
        bedrock_render::BedrockRenderError::Validation(
            "--region must be min_x,min_z,max_x,max_z".to_string(),
        )
    })?;
    Ok((min_x, min_z, max_x, max_z))
}

fn cache_manifest_path(config: &WebMapConfig) -> PathBuf {
    config.output_dir.join(".bedrock-render-cache.json")
}

fn read_cache_manifest(config: &WebMapConfig) -> bedrock_render::Result<Option<String>> {
    if !config.render_cache_validation {
        return Ok(None);
    }
    let path = cache_manifest_path(config);
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(path).map_err(|error| {
        bedrock_render::BedrockRenderError::io("failed to read cache manifest", error)
    })?;
    let value = serde_json::from_str::<serde_json::Value>(&text)
        .map_err(|error| bedrock_render::BedrockRenderError::Validation(error.to_string()))?;
    Ok(value
        .get("signature")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned))
}

fn write_cache_manifest(config: &WebMapConfig) -> bedrock_render::Result<()> {
    let mut json = String::new();
    json.push_str("{\n");
    write_json_field(&mut json, 1, "signature", &config.cache_signature, true);
    write_json_number(
        &mut json,
        1,
        "rendererVersion",
        RENDERER_CACHE_VERSION,
        true,
    );
    write_json_number(
        &mut json,
        1,
        "paletteVersion",
        DEFAULT_PALETTE_VERSION,
        true,
    );
    write_json_field(&mut json, 1, "layout", &layout_slug(config.layout), false);
    json.push_str("}\n");
    fs::write(cache_manifest_path(config), json).map_err(|error| {
        bedrock_render::BedrockRenderError::io("failed to write cache manifest", error)
    })
}

#[allow(clippy::too_many_lines)]
fn cache_signature(
    world_path: &std::path::Path,
    layout: RenderLayout,
    region_layout: RegionLayout,
    surface_options: SurfaceRenderOptions,
    backend: RenderBackend,
    palette_json_paths: &[PathBuf],
    extra_signature_salt: Option<&str>,
) -> bedrock_render::Result<String> {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    RENDERER_CACHE_VERSION.hash(&mut hasher);
    DEFAULT_PALETTE_VERSION.hash(&mut hasher);
    layout.chunks_per_tile.hash(&mut hasher);
    layout.blocks_per_pixel.hash(&mut hasher);
    layout.pixels_per_block.hash(&mut hasher);
    region_layout.chunks_per_region.hash(&mut hasher);
    surface_options.height_shading.hash(&mut hasher);
    surface_options.lighting.enabled.hash(&mut hasher);
    surface_options
        .lighting
        .light_azimuth_degrees
        .to_bits()
        .hash(&mut hasher);
    surface_options
        .lighting
        .light_elevation_degrees
        .to_bits()
        .hash(&mut hasher);
    surface_options
        .lighting
        .normal_strength
        .to_bits()
        .hash(&mut hasher);
    surface_options
        .lighting
        .shadow_strength
        .to_bits()
        .hash(&mut hasher);
    surface_options
        .lighting
        .highlight_strength
        .to_bits()
        .hash(&mut hasher);
    surface_options
        .lighting
        .ambient_occlusion
        .to_bits()
        .hash(&mut hasher);
    surface_options
        .lighting
        .max_shadow
        .to_bits()
        .hash(&mut hasher);
    surface_options
        .lighting
        .land_slope_softness
        .to_bits()
        .hash(&mut hasher);
    surface_options
        .lighting
        .edge_relief_strength
        .to_bits()
        .hash(&mut hasher);
    surface_options
        .lighting
        .edge_relief_threshold
        .to_bits()
        .hash(&mut hasher);
    surface_options
        .lighting
        .edge_relief_max_shadow
        .to_bits()
        .hash(&mut hasher);
    surface_options
        .lighting
        .edge_relief_highlight
        .to_bits()
        .hash(&mut hasher);
    surface_options
        .lighting
        .underwater_relief_enabled
        .hash(&mut hasher);
    surface_options
        .lighting
        .underwater_relief_strength
        .to_bits()
        .hash(&mut hasher);
    surface_options
        .lighting
        .underwater_depth_fade
        .to_bits()
        .hash(&mut hasher);
    surface_options
        .lighting
        .underwater_min_light
        .to_bits()
        .hash(&mut hasher);
    surface_options.block_boundaries.enabled.hash(&mut hasher);
    surface_options
        .block_boundaries
        .strength
        .to_bits()
        .hash(&mut hasher);
    surface_options
        .block_boundaries
        .flat_strength
        .to_bits()
        .hash(&mut hasher);
    surface_options
        .block_boundaries
        .height_threshold
        .to_bits()
        .hash(&mut hasher);
    surface_options
        .block_boundaries
        .max_shadow
        .to_bits()
        .hash(&mut hasher);
    surface_options
        .block_boundaries
        .highlight_strength
        .to_bits()
        .hash(&mut hasher);
    surface_options
        .block_boundaries
        .softness
        .to_bits()
        .hash(&mut hasher);
    surface_options
        .block_boundaries
        .line_width_pixels
        .to_bits()
        .hash(&mut hasher);
    backend.cache_slug().hash(&mut hasher);
    hash_file_signature(&mut hasher, &world_path.join("level.dat"))?;
    hash_file_signature(&mut hasher, &world_path.join("db").join("CURRENT"))?;
    hash_file_signature(&mut hasher, &world_path.join("db").join("MANIFEST-000001"))?;
    hash_file_signature(&mut hasher, &world_path.join("chunks.dat"))?;
    for path in palette_json_paths {
        hash_file_signature(&mut hasher, path)?;
    }
    if let Some(salt) = extra_signature_salt {
        salt.hash(&mut hasher);
    }
    Ok(format!("{:016x}", hasher.finish()))
}

fn hash_file_signature(
    hasher: &mut std::collections::hash_map::DefaultHasher,
    path: &std::path::Path,
) -> bedrock_render::Result<()> {
    path.to_string_lossy().hash(hasher);
    let Ok(metadata) = fs::metadata(path) else {
        0_u64.hash(hasher);
        return Ok(());
    };
    metadata.len().hash(hasher);
    if let Ok(modified) = metadata.modified()
        && let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH)
    {
        duration.as_secs().hash(hasher);
        duration.subsec_nanos().hash(hasher);
    }
    if metadata.len() <= 1024 * 1024 {
        let mut file = fs::File::open(path).map_err(|error| {
            bedrock_render::BedrockRenderError::io(
                format!("failed to open cache input {}", path.display()),
                error,
            )
        })?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(|error| {
            bedrock_render::BedrockRenderError::io(
                format!("failed to read cache input {}", path.display()),
                error,
            )
        })?;
        bytes.hash(hasher);
    }
    Ok(())
}

fn default_world_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("bedrock-world")
        .join("tests")
        .join("fixtures")
        .join("sample-bedrock-world")
}

fn default_output_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("bedrock-web-map")
}

fn print_help() {
    println!("render_web_map --world <path> --out <dir> [options]");
    println!("  --dimensions overworld,nether,end");
    println!("  --mode surface,heightmap,raw-heightmap,biome,layer,cave");
    println!(
        "  --y 64 --chunks-per-tile 16 --chunks-per-region 32 --blocks-per-pixel 1 --pixels-per-block 2"
    );
    println!("  --tile-size-pixels 512");
    println!("  --terrain-lighting off|soft|strong");
    println!("  --light-azimuth degrees --light-elevation degrees --normal-strength value");
    println!("  --land-normal-strength value --land-shadow-strength value");
    println!(
        "  --land-highlight-strength value --land-ambient-occlusion value --land-max-shadow value"
    );
    println!("  --underwater-relief off|soft|strong");
    println!("  --underwater-relief-strength value --underwater-depth-fade value");
    println!("  --underwater-min-light value");
    println!("  --block-boundaries on|off --block-boundary-strength value");
    println!("  --block-boundary-flat-strength value --block-boundary-threshold value");
    println!("  --block-boundary-max-shadow value --block-boundary-line-width value");
    println!("  --region min_x,min_z,max_x,max_z --threads auto|1..512");
    println!("  --max-render-threads N --reserve-threads N");
    println!("  --profile export|interactive");
    println!("  --memory-budget auto|disabled|MiB --pipeline-depth N");
    println!("  --tile-batch-size auto|N --writer-threads N --write-queue-capacity N");
    println!("  --viewer-prefetch-radius N --viewer-retain-radius N --viewer-max-image-loads N");
    println!("  --viewer-refresh-ms N --plugin-data <plugin.json>");
    println!("  --parallel-dimensions --no-parallel-dimensions");
    println!("  --cache-validation --no-cache-validation");
    println!("  --stats");
    println!("  --palette-json <optional-owned-palette.json>");
    println!("  --force");
    println!("If --region is omitted, the loaded chunk bounds are rendered.");
    println!("External palette JSON is optional; the renderer has a built-in palette.");
}

fn write_json_field(json: &mut String, indent: usize, key: &str, value: &str, comma: bool) {
    let suffix = if comma { "," } else { "" };
    let _ = writeln!(
        json,
        "{}\"{}\":\"{}\"{}",
        "  ".repeat(indent),
        key,
        value.replace('\\', "\\\\").replace('"', "\\\""),
        suffix
    );
}

fn write_json_number(
    json: &mut String,
    indent: usize,
    key: &str,
    value: impl std::fmt::Display,
    comma: bool,
) {
    let suffix = if comma { "," } else { "" };
    let _ = writeln!(
        json,
        "{}\"{}\":{}{}",
        "  ".repeat(indent),
        key,
        value,
        suffix
    );
}

fn dimension_slug(dimension: Dimension) -> String {
    match dimension {
        Dimension::Overworld => "overworld".to_string(),
        Dimension::Nether => "nether".to_string(),
        Dimension::End => "end".to_string(),
        Dimension::Unknown(id) => format!("dimension-{id}"),
    }
}

fn dimension_label(dimension: Dimension) -> String {
    match dimension {
        Dimension::Overworld => "Overworld".to_string(),
        Dimension::Nether => "Nether".to_string(),
        Dimension::End => "The End".to_string(),
        Dimension::Unknown(id) => format!("Dimension {id}"),
    }
}

fn mode_slug(mode: RenderMode) -> String {
    match mode {
        RenderMode::SurfaceBlocks => "surface".to_string(),
        RenderMode::HeightMap => "heightmap".to_string(),
        RenderMode::RawHeightMap => "raw-heightmap".to_string(),
        RenderMode::Biome { y } => format!("biome-y{y}"),
        RenderMode::RawBiomeLayer { y } => format!("raw-biome-y{y}"),
        RenderMode::LayerBlocks { y } => format!("layer-y{y}"),
        RenderMode::CaveSlice { y } => format!("cave-y{y}"),
    }
}

fn mode_label(mode: RenderMode) -> String {
    match mode {
        RenderMode::SurfaceBlocks => "Surface".to_string(),
        RenderMode::HeightMap => "Height Map".to_string(),
        RenderMode::RawHeightMap => "Raw Height Map".to_string(),
        RenderMode::Biome { y } => format!("Biome Y {y}"),
        RenderMode::RawBiomeLayer { y } => format!("Raw Biome Y {y}"),
        RenderMode::LayerBlocks { y } => format!("Layer Y {y}"),
        RenderMode::CaveSlice { y } => format!("Cave Y {y}"),
    }
}

fn layout_slug(layout: RenderLayout) -> String {
    format!(
        "{}c-{}bpp-{}ppb",
        layout.chunks_per_tile, layout.blocks_per_pixel, layout.pixels_per_block
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_size_pixels_infers_pixels_per_block() {
        let pixels_per_block =
            resolve_pixels_per_block(16, 1, None, Some(512)).expect("pixels per block");
        assert_eq!(pixels_per_block, 2);
    }

    #[test]
    fn tile_size_pixels_rejects_conflicting_pixels_per_block() {
        let result = resolve_pixels_per_block(16, 1, Some(1), Some(512));
        assert!(result.is_err());
    }

    #[test]
    fn layout_slug_includes_pixels_per_block() {
        assert_eq!(
            layout_slug(RenderLayout {
                chunks_per_tile: 16,
                blocks_per_pixel: 1,
                pixels_per_block: 2,
            }),
            "16c-1bpp-2ppb"
        );
    }

    #[test]
    fn terrain_lighting_parser_supports_presets() {
        assert!(!parse_terrain_lighting("off").expect("off").enabled);
        assert!(parse_terrain_lighting("soft").expect("soft").enabled);
        let strong = parse_terrain_lighting("strong").expect("strong");
        let soft = TerrainLightingOptions::soft();
        assert!(strong.normal_strength > soft.normal_strength);
    }

    #[test]
    fn underwater_relief_preset_updates_only_underwater_fields() {
        let mut lighting = TerrainLightingOptions::soft();
        let normal_strength = lighting.normal_strength;
        apply_underwater_relief_preset(&mut lighting, TerrainLightingPreset::Strong);
        assert_eq!(lighting.normal_strength, normal_strength);
        assert!(lighting.underwater_relief_enabled);
        assert!(
            lighting.underwater_relief_strength
                > TerrainLightingOptions::soft().underwater_relief_strength
        );
        apply_underwater_relief_preset(&mut lighting, TerrainLightingPreset::Off);
        assert!(!lighting.underwater_relief_enabled);
    }

    #[test]
    fn cache_signature_changes_with_terrain_lighting() {
        let layout = RenderLayout::default();
        let region_layout = RegionLayout::default();
        let soft = SurfaceRenderOptions::default();
        let strong = SurfaceRenderOptions {
            lighting: TerrainLightingOptions::strong(),
            ..SurfaceRenderOptions::default()
        };
        let world_path = default_world_path();
        let soft_signature = cache_signature(
            &world_path,
            layout,
            region_layout,
            soft,
            RenderBackend::Cpu,
            &[],
            None,
        )
        .expect("soft");
        let strong_signature = cache_signature(
            &world_path,
            layout,
            region_layout,
            strong,
            RenderBackend::Cpu,
            &[],
            None,
        )
        .expect("strong");
        assert_ne!(soft_signature, strong_signature);
    }

    #[test]
    fn cache_signature_changes_with_underwater_relief() {
        let layout = RenderLayout::default();
        let region_layout = RegionLayout::default();
        let soft = SurfaceRenderOptions::default();
        let mut underwater = SurfaceRenderOptions::default();
        underwater.lighting.underwater_relief_strength = 1.75;
        let world_path = default_world_path();
        let soft_signature = cache_signature(
            &world_path,
            layout,
            region_layout,
            soft,
            RenderBackend::Cpu,
            &[],
            None,
        )
        .expect("soft");
        let underwater_signature = cache_signature(
            &world_path,
            layout,
            region_layout,
            underwater,
            RenderBackend::Cpu,
            &[],
            None,
        )
        .expect("underwater");
        assert_ne!(soft_signature, underwater_signature);
    }

    #[test]
    fn cache_signature_changes_with_land_lighting() {
        let layout = RenderLayout::default();
        let region_layout = RegionLayout::default();
        let soft = SurfaceRenderOptions::default();
        let mut land = SurfaceRenderOptions::default();
        land.lighting.max_shadow = 18.0;
        let world_path = default_world_path();
        let soft_signature = cache_signature(
            &world_path,
            layout,
            region_layout,
            soft,
            RenderBackend::Cpu,
            &[],
            None,
        )
        .expect("soft");
        let land_signature = cache_signature(
            &world_path,
            layout,
            region_layout,
            land,
            RenderBackend::Cpu,
            &[],
            None,
        )
        .expect("land");
        assert_ne!(soft_signature, land_signature);
    }

    #[test]
    fn cache_signature_changes_with_land_slope_softness() {
        let layout = RenderLayout::default();
        let region_layout = RegionLayout::default();
        let soft = SurfaceRenderOptions::default();
        let mut slope_softness = SurfaceRenderOptions::default();
        slope_softness.lighting.land_slope_softness = 12.0;
        let world_path = default_world_path();
        let soft_signature = cache_signature(
            &world_path,
            layout,
            region_layout,
            soft,
            RenderBackend::Cpu,
            &[],
            None,
        )
        .expect("soft");
        let slope_signature = cache_signature(
            &world_path,
            layout,
            region_layout,
            slope_softness,
            RenderBackend::Cpu,
            &[],
            None,
        )
        .expect("slope");
        assert_ne!(soft_signature, slope_signature);
    }

    #[test]
    fn cache_signature_changes_with_edge_relief() {
        let layout = RenderLayout::default();
        let region_layout = RegionLayout::default();
        let soft = SurfaceRenderOptions::default();
        let mut edge = SurfaceRenderOptions::default();
        edge.lighting.edge_relief_strength = 0.75;
        edge.lighting.edge_relief_threshold = 2.0;
        edge.lighting.edge_relief_max_shadow = 24.0;
        edge.lighting.edge_relief_highlight = 0.18;
        let world_path = default_world_path();
        let soft_signature = cache_signature(
            &world_path,
            layout,
            region_layout,
            soft,
            RenderBackend::Cpu,
            &[],
            None,
        )
        .expect("soft");
        let edge_signature = cache_signature(
            &world_path,
            layout,
            region_layout,
            edge,
            RenderBackend::Cpu,
            &[],
            None,
        )
        .expect("edge");
        assert_ne!(soft_signature, edge_signature);
    }

    #[test]
    fn cache_signature_changes_with_block_boundaries() {
        let layout = RenderLayout::default();
        let region_layout = RegionLayout::default();
        let soft = SurfaceRenderOptions::default();
        let mut outlined = SurfaceRenderOptions::default();
        outlined.block_boundaries.strength = 0.85;
        outlined.block_boundaries.flat_strength = 0.25;
        let world_path = default_world_path();
        let soft_signature = cache_signature(
            &world_path,
            layout,
            region_layout,
            soft,
            RenderBackend::Cpu,
            &[],
            None,
        )
        .expect("soft");
        let outlined_signature = cache_signature(
            &world_path,
            layout,
            region_layout,
            outlined,
            RenderBackend::Cpu,
            &[],
            None,
        )
        .expect("outlined");
        assert_ne!(soft_signature, outlined_signature);
    }

    #[test]
    fn viewer_assets_are_external() {
        let html = include_str!("web_map_assets/viewer.html");
        assert!(html.contains("assets/viewer.css"));
        assert!(html.contains("assets/viewer.js"));
        assert!(html.contains("assets/plugin-runtime.js"));
        assert!(!html.contains("<style>"));
        assert!(!html.contains("function render()"));
    }

    #[test]
    fn plugin_data_accepts_chat_and_markers() {
        let value = serde_json::json!({
            "id": "sample",
            "label": "Sample",
            "version": "1",
            "items": [
                {
                    "type": "markers",
                    "dimension": "overworld",
                    "entries": [{"x": 10, "z": 20, "label": "Spawn"}]
                },
                {
                    "type": "chat",
                    "dimension": "overworld",
                    "messages": [{"player": "Alex", "message": "hello"}]
                }
            ]
        });
        validate_plugin_data(&value, std::path::Path::new("plugin.json"))
            .expect("valid plugin data");
    }

    #[test]
    fn plugin_data_rejects_unknown_item_type() {
        let value = serde_json::json!({
            "id": "sample",
            "label": "Sample",
            "version": "1",
            "items": [{"type": "script", "dimension": "overworld"}]
        });
        assert!(validate_plugin_data(&value, std::path::Path::new("plugin.json")).is_err());
    }

    #[test]
    fn viewer_retain_radius_must_cover_prefetch_radius() {
        assert_eq!(
            parse_nonnegative_usize("0", "--viewer-prefetch-radius").unwrap(),
            0
        );
        assert!(parse_positive_usize("0", "--viewer-max-image-loads").is_err());
    }

    #[test]
    fn render_thread_limits_reserve_cpu_capacity() {
        let config = WebMapConfig {
            world_path: default_world_path(),
            output_dir: default_output_dir(),
            dimensions: vec![Dimension::Overworld],
            modes: vec![WebRenderMode::Surface],
            y_layer: 64,
            cave_y: 32,
            layout: RenderLayout::default(),
            region_layout: RegionLayout::default(),
            surface_options: SurfaceRenderOptions::default(),
            region: None,
            threading: RenderThreadingOptions::Auto,
            backend: RenderBackend::Cpu,
            profile: RenderExecutionProfile::Export,
            memory_budget: RenderMemoryBudget::Auto,
            pipeline_depth: 0,
            print_stats: false,
            palette_json_paths: Vec::new(),
            plugin_data_paths: Vec::new(),
            viewer_prefetch_radius: 1,
            viewer_retain_radius: 2,
            viewer_max_image_loads: 8,
            viewer_refresh_ms: 2000,
            max_render_threads: Some(2),
            reserve_threads: 0,
            tile_batch_size: None,
            writer_threads: 1,
            write_queue_capacity: 256,
            parallel_dimensions: true,
            render_cache_validation: true,
            cache_signature: String::new(),
            force: false,
        };
        assert!(resolve_render_threads(&config, 128).unwrap() <= 2);
    }
}
