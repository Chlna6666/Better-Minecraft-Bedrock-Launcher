//! Read-only scalar/SIMD CPU and GPU comparison for one populated region of a Bedrock world.
use bedrock_render::{
    ChunkRegion, ImageFormat, MapRenderSession, MapRenderSessionConfig, MapRenderer, RenderBackend,
    RenderGpuBackend, RenderGpuFallbackPolicy, RenderGpuOptions, RenderLayout, RenderMode,
    RenderOptions, RenderPalette, RenderSimdPolicy, SurfaceRenderOptions, TerrainLightingOptions,
};
use bedrock_world::surface::WorldScanOptions;
use bedrock_world::{BedrockLevelDbStorage, Dimension, OpenOptions, World};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let world_path = PathBuf::from(std::env::args_os().nth(1).ok_or("pass a world directory")?);
    let storage = BedrockLevelDbStorage::open_read_only(world_path.join("db"))?;
    let world = Arc::new(World::from_typed_storage(
        &world_path,
        storage,
        OpenOptions::default(),
    ));
    let positions = world.render_chunk_positions(WorldScanOptions::default())?;
    let mut region_counts = HashMap::new();
    for position in positions
        .iter()
        .filter(|position| position.dimension == Dimension::Overworld)
    {
        *region_counts
            .entry((position.x.div_euclid(16), position.z.div_euclid(16)))
            .or_insert(0usize) += 1;
    }
    let (&(region_x, region_z), &selected_chunks) = region_counts
        .iter()
        .max_by_key(|(region, count)| (*count, *region))
        .ok_or("world has no Overworld render chunks")?;
    let min_x = region_x * 16;
    let min_z = region_z * 16;
    let region = ChunkRegion::new(Dimension::Overworld, min_x, min_z, min_x + 15, min_z + 15);
    let tiles = MapRenderer::<BedrockLevelDbStorage>::plan_region_tiles(
        region,
        RenderMode::SurfaceBlocks,
        RenderLayout::default(),
    )?;
    println!(
        "world={} chunks={} selected_chunks={} region={min_x},{min_z}..{},{} tiles={}",
        world_path.display(),
        positions.len(),
        selected_chunks,
        min_x + 15,
        min_z + 15,
        tiles.len()
    );
    let simd_surface = SurfaceRenderOptions {
        lighting: TerrainLightingOptions::off(),
        ..SurfaceRenderOptions::default()
    };
    println!("comparison_mode=resolved_color_pack lighting=off");

    for (label, backend, gpu_backend, simd) in [
        (
            "cpu-scalar",
            RenderBackend::Cpu,
            RenderGpuBackend::Auto,
            RenderSimdPolicy::Scalar,
        ),
        (
            "cpu-auto",
            RenderBackend::Cpu,
            RenderGpuBackend::Auto,
            RenderSimdPolicy::Auto,
        ),
        (
            "dx11",
            RenderBackend::Wgpu,
            RenderGpuBackend::Dx11,
            RenderSimdPolicy::Auto,
        ),
        (
            "vulkan",
            RenderBackend::Wgpu,
            RenderGpuBackend::Vulkan,
            RenderSimdPolicy::Auto,
        ),
    ] {
        let session = MapRenderSession::new(
            MapRenderer::new(Arc::clone(&world), RenderPalette::default()),
            MapRenderSessionConfig {
                gpu_backend,
                ..MapRenderSessionConfig::default()
            },
        );
        if backend == RenderBackend::Wgpu && !session.gpu_available() {
            println!("backend={label} error=GPU context unavailable");
            continue;
        }
        let hash = AtomicU64::new(0);
        let start = Instant::now();
        let result = session.renderer().render_web_tiles(
            &tiles,
            RenderOptions {
                format: ImageFormat::Rgba,
                backend,
                simd,
                surface: simd_surface,
                gpu: RenderGpuOptions {
                    backend: gpu_backend,
                    fallback_policy: RenderGpuFallbackPolicy::Required,
                    batch_pixels: 1,
                    ..RenderGpuOptions::default()
                },
                ..RenderOptions::default()
            },
            |_, tile| {
                let mut tile_hash = 0xcbf2_9ce4_8422_2325_u64;
                for byte in tile.rgba.iter() {
                    tile_hash = (tile_hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3);
                }
                hash.fetch_xor(tile_hash, Ordering::Relaxed);
                Ok(())
            },
        );
        match result {
            Ok(result) => println!(
                "backend={label} elapsed_ms={} hash={:016x} cpu_tiles={} gpu_tiles={} gpu_actual={:?} adapter={} db_read_ms={} decode_ms={} bake_ms={} compose_ms={} upload_ms={} dispatch_ms={} readback_ms={}",
                start.elapsed().as_millis(),
                hash.load(Ordering::Relaxed),
                result.stats.cpu_tiles,
                result.stats.gpu_tiles,
                result.stats.gpu_actual_backend,
                result.stats.gpu_adapter_name.as_deref().unwrap_or("none"),
                result.stats.db_read_ms,
                result.stats.decode_ms,
                result.stats.region_bake_ms,
                result.stats.tile_compose_ms,
                result.stats.gpu_upload_ms,
                result.stats.gpu_dispatch_ms,
                result.stats.gpu_readback_ms,
            ),
            Err(error) => println!("backend={label} error={error}"),
        }
    }
    Ok(())
}
