#![allow(clippy::too_many_lines)]

#[cfg(feature = "webp")]
use bedrock_render::{
    ChunkRegion, MapRenderSession, MapRenderSessionConfig, RenderCachePolicy,
    RenderExecutionProfile, RenderLayout, RenderThreadingOptions, RenderTileOutputOptions,
    TilePixelFormat, TileReadySource, TileStreamEvent, TileStreamEventV2,
};
use bedrock_render::{
    ImageFormat, MapRenderer, RenderJob, RenderMode, RenderOptions, RenderPalette, TileCoord,
};
use bedrock_world::{BedrockLevelDbStorage, BedrockWorld, Dimension, OpenOptions};
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(feature = "webp")]
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(feature = "webp")]
use std::time::{SystemTime, UNIX_EPOCH};

fn fixture_world_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("bedrock-world")
        .join("tests")
        .join("fixtures")
        .join("sample-bedrock-world")
}

#[test]
fn renders_fixture_biome_tile_as_rgba() {
    let world_path = fixture_world_path();
    if !world_path.join("db").join("CURRENT").exists() {
        return;
    }
    let storage = Arc::new(BedrockLevelDbStorage::open(world_path.join("db")).expect("open db"));
    let world = Arc::new(BedrockWorld::from_storage(
        world_path,
        storage,
        OpenOptions::default(),
    ));
    let renderer = MapRenderer::new(world, RenderPalette::default());
    let tile = renderer
        .render_tile_with_options_blocking(
            RenderJob {
                tile_size: 16,
                ..RenderJob::new(
                    TileCoord {
                        x: 0,
                        z: 0,
                        dimension: Dimension::Overworld,
                    },
                    RenderMode::Biome { y: 64 },
                )
            },
            &RenderOptions {
                format: ImageFormat::Rgba,
                ..RenderOptions::default()
            },
        )
        .expect("render fixture biome tile");
    assert_eq!(tile.rgba.len(), 16 * 16 * 4);
    assert!(tile.rgba.chunks_exact(4).any(|pixel| pixel[3] != 0));
}

#[cfg(feature = "webp")]
#[test]
fn streaming_session_emits_rendered_then_cached_events() {
    let world_path = fixture_world_path();
    if !world_path.join("db").join("CURRENT").exists() {
        return;
    }
    let storage =
        Arc::new(BedrockLevelDbStorage::open_read_only(world_path.join("db")).expect("open db"));
    let world = Arc::new(BedrockWorld::from_storage(
        world_path,
        storage,
        OpenOptions::default(),
    ));
    let renderer = MapRenderer::new(world, RenderPalette::default());
    let cache_root = unique_cache_root();
    let session = MapRenderSession::new(
        renderer,
        MapRenderSessionConfig {
            cache_root: cache_root.clone(),
            world_id: "fixture".to_string(),
            world_signature: "streaming-test".to_string(),
            cull_missing_chunks: true,
            ..MapRenderSessionConfig::default()
        },
    );
    let layout = RenderLayout {
        chunks_per_tile: 1,
        blocks_per_pixel: 16,
        pixels_per_block: 1,
    };
    let planned_tiles =
        MapRenderer::<std::sync::Arc<dyn bedrock_world::WorldStorage>>::plan_region_tiles(
            ChunkRegion::new(Dimension::Overworld, 0, 0, 0, 0),
            RenderMode::Biome { y: 64 },
            layout,
        )
        .expect("plan tile");
    assert_eq!(planned_tiles.len(), 1);

    let rendered_tiles = Arc::new(AtomicUsize::new(0));
    let cached = Arc::new(AtomicUsize::new(0));
    let complete = Arc::new(AtomicUsize::new(0));
    session
        .render_web_tiles_streaming_blocking(
            &planned_tiles,
            RenderOptions {
                format: ImageFormat::FastRgbaZstd,
                threading: RenderThreadingOptions::Single,
                execution_profile: RenderExecutionProfile::Interactive,
                cache_policy: RenderCachePolicy::Use,
                ..RenderOptions::default()
            },
            {
                let rendered_tiles = Arc::clone(&rendered_tiles);
                let complete = Arc::clone(&complete);
                move |event| {
                    match event {
                        TileStreamEvent::Ready {
                            source: TileReadySource::Render,
                            ..
                        } => {
                            rendered_tiles.fetch_add(1, Ordering::Relaxed);
                        }
                        TileStreamEvent::Complete { .. } => {
                            complete.fetch_add(1, Ordering::Relaxed);
                        }
                        _ => {}
                    }
                    Ok(())
                }
            },
        )
        .expect("first streaming render");
    assert_eq!(rendered_tiles.load(Ordering::Relaxed), 1);
    assert_eq!(complete.load(Ordering::Relaxed), 1);

    session
        .render_web_tiles_streaming_blocking(
            &planned_tiles,
            RenderOptions {
                format: ImageFormat::FastRgbaZstd,
                threading: RenderThreadingOptions::Single,
                execution_profile: RenderExecutionProfile::Interactive,
                cache_policy: RenderCachePolicy::Use,
                ..RenderOptions::default()
            },
            {
                let cached = Arc::clone(&cached);
                let complete = Arc::clone(&complete);
                move |event| {
                    match event {
                        TileStreamEvent::Ready {
                            tile,
                            source: TileReadySource::MemoryCache | TileReadySource::DiskCacheFresh,
                            ..
                        } => {
                            assert!(!tile.rgba.is_empty());
                            cached.fetch_add(1, Ordering::Relaxed);
                        }
                        TileStreamEvent::Complete { .. } => {
                            complete.fetch_add(1, Ordering::Relaxed);
                        }
                        _ => {}
                    }
                    Ok(())
                }
            },
        )
        .expect("cached streaming render");
    assert_eq!(cached.load(Ordering::Relaxed), 1);
    assert_eq!(complete.load(Ordering::Relaxed), 2);

    let _ = std::fs::remove_dir_all(cache_root);
}

#[cfg(feature = "webp")]
#[test]
fn streaming_session_v2_emits_rgba_tiles_from_render_by_default() {
    let world_path = fixture_world_path();
    if !world_path.join("db").join("CURRENT").exists() {
        return;
    }
    let storage =
        Arc::new(BedrockLevelDbStorage::open_read_only(world_path.join("db")).expect("open db"));
    let world = Arc::new(BedrockWorld::from_storage(
        world_path,
        storage,
        OpenOptions::default(),
    ));
    let renderer = MapRenderer::new(Arc::clone(&world), RenderPalette::default());
    let cache_root = unique_cache_root();
    let session = MapRenderSession::new(
        renderer,
        MapRenderSessionConfig {
            cache_root: cache_root.clone(),
            world_id: "fixture".to_string(),
            world_signature: "streaming-v2-test".to_string(),
            cull_missing_chunks: true,
            ..MapRenderSessionConfig::default()
        },
    );
    let layout = RenderLayout {
        chunks_per_tile: 1,
        blocks_per_pixel: 16,
        pixels_per_block: 1,
    };
    let mut planned_tiles =
        MapRenderer::<std::sync::Arc<dyn bedrock_world::WorldStorage>>::plan_region_tiles(
            ChunkRegion::new(Dimension::Overworld, 0, 0, 0, 0),
            RenderMode::Biome { y: 64 },
            layout,
        )
        .expect("plan tile");
    assert_eq!(planned_tiles.len(), 1);
    planned_tiles[0].chunk_positions = Some(
        vec![bedrock_world::ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        }]
        .into(),
    );

    let output = RenderTileOutputOptions::default();
    let rendered_tiles = Arc::new(AtomicUsize::new(0));
    let complete = Arc::new(AtomicUsize::new(0));
    session
        .render_web_tiles_streaming_blocking_v2(
            &planned_tiles,
            RenderOptions {
                threading: RenderThreadingOptions::Single,
                execution_profile: RenderExecutionProfile::Interactive,
                cache_policy: RenderCachePolicy::Use,
                tile_cache_validation_seed: 42,
                ..RenderOptions::default()
            },
            output,
            {
                let rendered_tiles = Arc::clone(&rendered_tiles);
                let complete = Arc::clone(&complete);
                move |event| {
                    match event {
                        TileStreamEventV2::Ready {
                            tile,
                            source: TileReadySource::Render,
                            ..
                        } => {
                            assert_eq!(tile.pixel_format, TilePixelFormat::Rgba8);
                            assert!(!tile.pixels.is_empty());
                            rendered_tiles.fetch_add(1, Ordering::Relaxed);
                        }
                        TileStreamEventV2::Complete { .. } => {
                            complete.fetch_add(1, Ordering::Relaxed);
                        }
                        _ => {}
                    }
                    Ok(())
                }
            },
        )
        .expect("first v2 streaming render");
    assert_eq!(rendered_tiles.load(Ordering::Relaxed), 1);
    assert_eq!(complete.load(Ordering::Relaxed), 1);

    let _ = std::fs::remove_dir_all(cache_root);
}

#[cfg(feature = "webp")]
fn unique_cache_root() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("bedrock-render-session-test-{stamp}"))
}
