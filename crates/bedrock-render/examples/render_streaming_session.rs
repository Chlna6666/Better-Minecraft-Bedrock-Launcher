use bedrock_render::{
    ChunkRegion, ImageFormat, MapRenderSession, MapRenderSessionConfig, MapRenderer,
    RenderCachePolicy, RenderCancelFlag, RenderExecutionProfile, RenderLayout, RenderMode,
    RenderOptions, RenderPalette, RenderThreadingOptions, RenderTilePriority, TileReadySource,
    TileStreamEvent,
};
use bedrock_world::{BedrockWorld, Dimension, OpenOptions};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

mod common;

#[allow(clippy::too_many_lines)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    common::init_logger();
    let world_path = std::env::args_os().nth(1).map_or_else(
        || {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("bedrock-world")
                .join("tests")
                .join("fixtures")
                .join("sample-bedrock-world")
        },
        PathBuf::from,
    );

    if !world_path.join("db").join("CURRENT").exists() && !world_path.join("chunks.dat").exists() {
        println!(
            "missing fixture world; pass a Bedrock world path as the first argument: {}",
            world_path.display()
        );
        return Ok(());
    }

    let world = Arc::new(BedrockWorld::open_blocking(
        &world_path,
        OpenOptions::default(),
    )?);
    let renderer = MapRenderer::new(world, RenderPalette::default());
    let session = MapRenderSession::new(
        renderer,
        MapRenderSessionConfig {
            cache_root: unique_cache_root(),
            world_id: "streaming-example".to_string(),
            world_signature: "local-fixture".to_string(),
            cull_missing_chunks: true,
            ..MapRenderSessionConfig::default()
        },
    );

    let layout = RenderLayout {
        chunks_per_tile: 16,
        blocks_per_pixel: 4,
        pixels_per_block: 1,
    };
    let planned_tiles =
        MapRenderer::<std::sync::Arc<dyn bedrock_world::WorldStorage>>::plan_region_tiles(
            ChunkRegion::new(Dimension::Overworld, 0, 0, 31, 31),
            RenderMode::SurfaceBlocks,
            layout,
        )?;
    let rendered_tiles = Arc::new(AtomicUsize::new(0));
    let cached = Arc::new(AtomicUsize::new(0));
    let failed = Arc::new(AtomicUsize::new(0));

    let output_format = default_output_format();
    let cancel = RenderCancelFlag::new();
    session.render_web_tiles_streaming_blocking(
        &planned_tiles,
        RenderOptions {
            format: output_format,
            threading: RenderThreadingOptions::Auto,
            execution_profile: RenderExecutionProfile::Interactive,
            cache_policy: RenderCachePolicy::Use,
            cancel: Some(cancel),
            priority: RenderTilePriority::DistanceFrom { tile_x: 0, tile_z: 0 },
            ..RenderOptions::default()
        },
        {
            let rendered_tiles = Arc::clone(&rendered_tiles);
            let cached = Arc::clone(&cached);
            let failed = Arc::clone(&failed);
            move |event| {
                match event {
                    TileStreamEvent::Ready {
                        planned,
                        tile,
                        source,
                    } => {
                        match source {
                            TileReadySource::MemoryCache
                            | TileReadySource::DiskCacheFresh
                            | TileReadySource::DiskCacheStale => {
                                cached.fetch_add(1, Ordering::Relaxed);
                            }
                            TileReadySource::Render => {
                                rendered_tiles.fetch_add(1, Ordering::Relaxed);
                            }
                            TileReadySource::Preview => {}
                        }
                        println!(
                            "{:?} tile ({}, {}) pixels={}",
                            source,
                            planned.job.coord.x,
                            planned.job.coord.z,
                            tile.rgba.len() / 4
                        );
                    }
                    TileStreamEvent::Failed { planned, error } => {
                        failed.fetch_add(1, Ordering::Relaxed);
                        println!(
                            "failed tile ({}, {}): {error}",
                            planned.job.coord.x, planned.job.coord.z
                        );
                    }
                    TileStreamEvent::Empty { planned } => {
                        println!("empty tile ({}, {})", planned.job.coord.x, planned.job.coord.z);
                    }
                    TileStreamEvent::Progress(progress) => {
                        println!(
                            "progress {}/{}",
                            progress.completed_tiles, progress.total_tiles
                        );
                    }
                    TileStreamEvent::Complete { diagnostics, stats } => {
                        println!(
                            "complete planned={} cache={}/{} backend={} cpu_tiles={} missing_chunks={}",
                            stats.planned_tiles,
                            stats.cache_hits,
                            stats.cache_misses,
                            stats.resolved_backend.label(),
                            stats.cpu_tiles,
                            diagnostics.missing_chunks
                        );
                    }
                }
                Ok(())
            }
        },
    )?;

    println!(
        "summary rendered={} cached={} failed={}",
        rendered_tiles.load(Ordering::Relaxed),
        cached.load(Ordering::Relaxed),
        failed.load(Ordering::Relaxed)
    );
    Ok(())
}

fn unique_cache_root() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("bedrock-render-session-{stamp}"))
}

fn default_output_format() -> ImageFormat {
    #[cfg(feature = "webp")]
    {
        ImageFormat::WebP
    }
    #[cfg(not(feature = "webp"))]
    {
        ImageFormat::Rgba
    }
}
