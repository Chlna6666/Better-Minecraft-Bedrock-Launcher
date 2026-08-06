# API Guide

`bedrock-render` exposes five layers:

- `RenderPalette` for color lookup, overrides, and JSON imports.
- `MapRenderer` for tile, batch, region, bake, and web-map rendering.
- `MapRenderSession` for long-lived interactive renderers with cache reuse and
  streaming tile events.
- `bedrock_render::editor` for downstream map-viewer tooling that maps common
  `bedrock-world` typed writes to render/UI invalidation.
- Value types for layout, threading, memory budgets, diagnostics, and cached
  tile paths.

## Renderer Setup

Create a lazy `bedrock_world::BedrockWorld`, then pass it with a palette to
`MapRenderer::new`:

```rust
use std::sync::Arc;

let world = bedrock_world::BedrockWorld::open_blocking(
    "path/to/minecraftWorld",
    bedrock_world::OpenOptions::default(),
)?;
let renderer = bedrock_render::MapRenderer::new(
    Arc::new(world),
    bedrock_render::RenderPalette::default(),
);
```

`RenderPalette::default()` builds the palette from embedded auditable JSON
sources at startup. Applications that carry their own licensed color data can
merge additional JSON before constructing the renderer.

## Tile And Region Rendering

Use `RenderJob::new` for one tile and `MapRenderer::plan_region_tiles` for
deterministic web-map regions. `RenderLayout` controls world coverage and pixel
density. `RenderOptions` controls output format, backend, threading, memory
budget, CPU/GPU pipeline policy, priority, diagnostics, cancellation, and
surface shading.

`ImageFormat::Rgba` is the cheapest validation format. `ImageFormat::WebP` and
`ImageFormat::Png` populate `TileImage::encoded` when their features are enabled.

## Session Streaming

Use `MapRenderSession` for UI integrations. The session owns a `MapRenderer`,
shares a memory/disk tile cache, and can use render-index culling to avoid
loading chunks that have no renderable records.

```rust
let session = bedrock_render::MapRenderSession::new(
    renderer,
    bedrock_render::MapRenderSessionConfig {
        cache_root: "target/render-cache".into(),
        world_id: "world".into(),
        world_signature: "stable-content-signature".into(),
        cull_missing_chunks: true,
        ..Default::default()
    },
);
let cancel = bedrock_render::RenderCancelFlag::new();
session.render_web_tiles_streaming_blocking(
    &planned_tiles,
    bedrock_render::RenderOptions {
        cancel: Some(cancel),
        cache_policy: bedrock_render::RenderCachePolicy::Use,
        ..Default::default()
    },
    |event| {
        match event {
            bedrock_render::TileStreamEvent::Cached { .. } => {}
            bedrock_render::TileStreamEvent::Rendered { .. } => {}
            bedrock_render::TileStreamEvent::Failed { .. } => {}
            bedrock_render::TileStreamEvent::Progress(_) => {}
            bedrock_render::TileStreamEvent::Complete { .. } => {}
        }
        Ok(())
    },
)?;
```

`render_web_tiles_streaming` is available with the default `async` feature. It
wraps the same blocking pipeline in `tokio::task::spawn_blocking`. For UI code
that prefers an async channel, use `render_web_tiles_streaming_channel`; it
returns a `tokio::sync::mpsc::Receiver<TileStreamEvent>` immediately and lets
the render task continue in the background.

Use `render_web_tiles_streaming_blocking_v2`,
`render_web_tiles_streaming_v2`, or `render_web_tiles_streaming_channel_v2`
when a UI wants decoded pixels instead of encoded tile bytes. These APIs emit
`TileStreamEventV2::Ready` with `DecodedTileImage`; `RenderTileOutputOptions`
selects `TilePixelFormat::Rgba8` by default or `Bgra8` for consumers that
prefer BGRA byte order.

## Cache Identity

Use `world_cache_identity` when constructing cache keys for a world. It returns
`WorldCacheIdentity { world_id, world_signature }` from one filesystem metadata
snapshot, keeping the directory identity and content signature consistent for
the same request. `world_cache_id` and `world_cache_signature` remain as
compatibility accessors for callers that only need one component.

## Editor Facade

Rendering remains read-only by default. `bedrock_render::editor` is a downstream
adapter for explicit write-mode workflows; it does not change the
`bedrock-world` storage-layer API. The module re-exports common
`bedrock-world` v0.2 map/global/HSA/actor/block-entity/heightmap/biome/query
types and adds two render-facing helpers:

- `MapWorldEditor` opens or wraps a writable `BedrockWorld` and exposes common
  structured editor calls.
- `MapEditInvalidation` describes which render/UI metadata, overlays, chunks,
  and tile caches should be refreshed after a write.

```rust
use bedrock_render::editor::{MapWorldEditor, WorldScanOptions};

let editor = MapWorldEditor::open_writable("path/to/minecraftWorld")?;
let maps = editor.scan_map_records(WorldScanOptions::default())?;
let overlays = editor.scan_hsa_records(WorldScanOptions::default())?;

let invalidation = editor.put_heightmap(chunk_pos, chunk_version, &heightmap)?;
if invalidation.refresh_metadata() {
    // reload metadata panels or manifest-derived summaries
}
for chunk in invalidation.affected_chunks() {
    // remove cached tiles covering this chunk and schedule rerender
}
```

`MapWorldEditor` intentionally does not replace the full `bedrock-world` API.
Use the facade for common map viewer actions such as map/global records, HSA,
modern actors, block entities, heightmaps, and biome storage. Use
`editor.world()` or `bedrock-world` directly for uncommon Bedrock records or
tool-specific validation. Applications should still require a per-operation
confirmation before mutating methods and should increment UI generations before
refreshing overlays or tiles so stale background results are ignored.
`MapEditInvalidation` is not serialized to Bedrock data and is not part of
`bedrock-world`'s storage semantics.

### Replacing older entry points

- `render_web_tiles_blocking` remains suitable for static exports where one
  sink writes all tiles.
- Interactive renderers should use `MapRenderSession` and stream events into
  the UI.
- Full-world chunk scans should move to `bedrock-world` render-index APIs before
  planning visible tiles.
- `render_web_tiles_blocking` remains ordered for export. For first-screen
  interactivity, set `RenderOptions::priority` to
  `RenderTilePriority::DistanceFrom { tile_x, tile_z }`.

## CPU Pipeline

`RenderOptions::cpu` controls the local Rayon pipeline used for load, bake,
compose, and encode coordination:

- `queue_depth=0` picks a bounded queue sized to the worker count and work set.
- `chunk_batch_size=0` lets `bedrock-world` choose render chunk batch sizes.
- `encode_workers=0` keeps encode/write workers profile-aware.

The renderer does not use Rayon global pool state. Long-lived sessions reuse the
same world, renderer, cache, and diagnostics path while each render request
keeps cancellation and progress scoped to its generation.

## Render Modes

- `SurfaceBlocks` is the primary terrain map.
- `HeightMap` renders height gradients from the computed surface columns used
  by terrain rendering.
- `RawHeightMap` renders raw Bedrock Data2D/Data3D/Legacy heightmap records for
  diagnostics and migration checks.
- `Biome { y }` renders resolved biome colors at a sampled Y layer.
- `RawBiomeLayer { y }` renders diagnostic biome-id colors.
- `LayerBlocks { y }` renders a fixed X/Z block layer.
- `CaveSlice { y }` renders air, solid, water, and lava diagnostics for a fixed
  Y layer.

Missing chunks and empty terrain are transparent. Unknown block names are opaque
diagnostic pixels.

### Canonical Terrain Sampling

`SurfaceBlocks` and `HeightMap` use `bedrock-world` exact surface sampling:
actual block columns are scanned top-down and baked as a canonical
`TerrainColumnSample` contract with the visual surface block, relief support,
thin overlay, and water context. Saved heightmaps are still loaded, but they
are treated as hints/diagnostics and can disagree with the rendered surface.
Use `RawHeightMap` when you need the old raw-height diagnostic image.

### Legacy Terrain

Old Bedrock/Pocket Edition chunks may expose only `LegacyTerrain` tag `0x30`.
`bedrock-render` handles this through `bedrock-world` render exact-batch loading:

- `SurfaceBlocks` scans the legacy `0..=127` height range from actual block IDs.
- `HeightMap` colors the computed legacy surface height over `0..=127`.
- `RawHeightMap` colors the saved legacy heightmap over `0..=127`.
- `LayerBlocks` and `CaveSlice` sample legacy block ID + data arrays directly.
- Common 0.16 numeric block IDs are mapped to modern `minecraft:*` names before
  palette lookup; unknown IDs become `legacy:<id>` and are counted by
  diagnostics.
- Legacy biome samples are decoded as `[biome_id, red, green, blue]`.
  `Biome` renders saved RGB directly and prefers it over conflicting old
  Data2D/Data3D biome ids. `RawBiomeLayer` uses the saved biome ID when the
  palette knows it and falls back to saved RGB for unknown old IDs.
- `SurfaceBlocks` uses legacy RGB samples for grass and foliage tint. Water
  keeps the normal water-tint fallback so legacy grass colors are not applied
  to water.
- If a transition chunk has both `LegacyTerrain` and `SubChunkPrefix`, subchunk
  block data wins and legacy terrain is only a fallback for missing block data.

The renderer cache version is `RENDERER_CACHE_VERSION = 51`, so old
transparent, incorrectly sampled, raw-height-driven, misplaced region-compose,
renderer-rescanned, incorrectly prioritized legacy-biome, or pre-authority-cache
tiles are not reused. `RenderOptions::default()` bypasses tile cache
reads/writes; opt in with `RenderCachePolicy::Use` when a session or export
should use the cache.
`MapRenderSession::new` also lifts stale lower renderer versions to the current
version before creating cache keys.

Sessions write an authority cache for final tiles. The cache stores tile blob
entries, empty-tile sentinels, chunk dependency stamps, and reverse
chunk-to-tile references so later batches can trust or reject cached decoded
pixels without reloading every source chunk.

`RenderPipelineStats` exposes placement diagnostics for web/region pipelines:
`region_chunks_copied`, `region_chunks_out_of_bounds`, and
`tile_missing_region_samples`. Non-zero out-of-bounds or missing-region sample
counts indicate a planning/compose contract problem rather than a LevelDB read
failure.

## Error Handling

All public fallible APIs return `bedrock_render::Result<T>`.

Match `BedrockRenderError::kind()` for stable categories:

```rust
match error.kind() {
    bedrock_render::BedrockRenderErrorKind::Cancelled => {
        // The caller's cancel flag was observed.
    }
    bedrock_render::BedrockRenderErrorKind::UnsupportedMode => {
        // The requested output format or feature is not enabled.
    }
    _ => eprintln!("{error}"),
}
```

Display strings are for humans and may become more descriptive over time.

## GPU Backend

GPU backends are opt-in with `gpu`, `gpu-dx11`, `gpu-vulkan`, or `gpu-dx12`.
`RenderBackend::Auto` attempts GPU compose for tiles at or above
`RenderGpuOptions::min_pixels`, then falls back to CPU when the GPU feature,
adapter, tile shape, or shader path is unavailable.
`RenderBackend::Gpu` forces the attempt. `RenderGpuFallbackPolicy::Required`
turns these fallback cases into errors instead of CPU renders. GPU failures are
reported through `RenderPipelineStats::gpu_fallback_reason` and counted as CPU
fallback renders.

`RenderOptions::gpu` controls scheduling:

- `min_pixels` is the Auto-mode threshold before GPU compose is attempted.
- `backend` selects `auto` or the `wgpu` backend.
- `fallback_policy` selects CPU fallback or strict required GPU execution.
- `diagnostics` selects off, summary, or verbose GPU logs through the `log`
  facade.
- `max_in_flight=0` uses profile defaults: up to 4 interactive jobs or 8 export
  jobs.
- `batch_size=0`, `batch_pixels=0`, `submit_workers=0`, and
  `readback_workers=0` use profile-aware automatic defaults.
- `buffer_pool_bytes=0` and `staging_pool_bytes=0` enable automatic reusable
  storage/readback pool budgets. Explicit non-zero values cap each pool.

The GPU path uses a single `wgpu` device with a bounded in-flight queue. CPU
workers continue to load, bake, and prepare tiles while GPU jobs upload,
dispatch, and read back results. Web/region ready tiles are submitted through a
true batch path, so `batch_size` and `batch_pixels` affect submit/readback
counts. Stats expose selected backend, adapter name/vendor/device type,
`gpu_supported_tiles`, `gpu_skipped_tiles`, `gpu_skip_reason`, `gpu_batches`,
`gpu_batch_tiles`, `gpu_submit_batches`, `gpu_readback_batches`,
`gpu_uploaded_bytes`, `gpu_readback_bytes`, `gpu_prepare_ms`, `gpu_max_in_flight`,
`gpu_queue_wait_ms`, `gpu_submit_workers`, `gpu_worker_threads`, `gpu_buffer_reuses`,
`gpu_buffer_allocations`, `gpu_staging_reuses`, and
`gpu_staging_allocations` for tuning. CPU feeding diagnostics are split into
`world_load_ms`, `db_read_ms`, `decode_ms`, `region_copy_ms`, and
`world_worker_threads`, which helps distinguish a slow shader from an idle GPU
waiting for LevelDB/decode/bake work.

Environment overrides remain supported:

- `BEDROCK_RENDER_GPU=auto|on|off|required`
- `BEDROCK_RENDER_GPU_BACKEND=auto|wgpu`
- `BEDROCK_RENDER_GPU_LOG=off|summary|verbose`

`SurfaceRenderOptions::block_boundaries` controls the default top-down 2D block
outline and height-contact shadow. It is part of the render cache signature and
is disabled when `height_shading` is disabled.

Streaming complete events include `RenderPipelineStats`, so UI status bars can
show `resolved_backend`, `gpu_fallback_reason`, cache hits/misses, worker count,
world/decode worker count, GPU batch/readback counts, and CPU/GPU timing without
waiting for a separate export summary.

CPU-only consumers can disable default features and enable the formats they need.
