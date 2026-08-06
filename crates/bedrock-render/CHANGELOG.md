# Changelog

All notable changes to `bedrock-render` are tracked here.

## 0.3.4 - 2026-07-18

### Added

- Added `WorldCacheIdentity` and `world_cache_identity` to compute the world
  cache directory ID and content signature from one metadata snapshot.

### Changed

- Updated the `bedrock-world` dependency to `0.3.4`.

## 0.3.3 - 2026-07-14

### Changed

- Updated the `bedrock-world` dependency to `0.3.3` after fixing its blocked
  publish workflow.

## 0.3.2 - 2026-07-14

### Changed

- Updated the `bedrock-world` dependency to `0.3.2` after the `0.3.1` release
  workflow was blocked before publication.

## 0.3.1 - 2026-07-14

### Changed

- Updated `bedrock-leveldb` and `bedrock-world` dependencies to `0.3.1` after
  the `0.3.0` release workflow was blocked before publication.

## 0.3.0 - 2026-07-14

### Added

- Added cache and validation benchmarks covering authority-index lookup,
  payload verification, refresh writes, and parallel chunk fingerprint checks.

### Changed

- Extended the authoritative tile cache with reusable payload readers,
  XXH3-128 payload fingerprints, durable delta updates, and parallel dependency
  validation for interactive refreshes.
- Updated interactive scheduling benchmarks and cache-performance guidance.
- Updated `bedrock-leveldb` and `bedrock-world` dependencies to `0.3.0`.

## 0.2.1 - 2026-06-29

### Added

- Added an authoritative final-tile cache index/blob alongside the manifest
  probe cache. Session rendering can now validate tile dependencies by world
  signature, chunk state, and tile-to-chunk references before trusting cached
  decoded output.
- Added decoded v2 streaming APIs through `TileStreamEventV2`,
  `DecodedTileImage`, `RenderTileOutputOptions`, and `TilePixelFormat`, with
  RGBA8 output by default and BGRA8 available for UI toolkits that prefer it.
- Added tile index cache diagnostics to `RenderPipelineStats` for trusted hits,
  validated hits, misses, empty hits, and index read time.

### Changed

- Prepared the crate for crates.io publishing by removing `publish = false` and
  declaring versioned `bedrock-leveldb 0.2.2` and `bedrock-world 0.2.2`
  dependencies while retaining local repository paths for development.
- Bumped `RENDERER_CACHE_VERSION` to `51`; cached tiles from older cache
  layouts or pre-authority validation are invalidated.
- Clarified the `editor` module as a downstream adapter over the core
  `bedrock-world` write APIs.
- Kept GPU backends opt-in. The default crate features are `async` and `webp`;
  `gpu`, `gpu-dx11`, `gpu-vulkan`, and `gpu-dx12` remain explicit feature
  choices.

### Breaking Changes

- `TileImage::rgba` is now shared as `Arc<[u8]>` so streaming sessions and cache
  readers can pass decoded pixels without extra copies.
- `PlannedTile::chunk_positions` is optional, allowing render-index planning to
  defer or reuse chunk dependency sets.
- Existing renderer tile caches must be rebuilt because renderer cache version
  `51` uses the authority index/blob layout.

## 0.2.0 - 2026-05-07

### Added

- Added `bedrock_render::editor`, a v0.2.0 writable facade over
  `bedrock-world` map/global/HSA/actor/block-entity/heightmap/biome APIs.
  `MapWorldEditor` keeps normal render sources read-only and returns
  `MapEditInvalidation` so UI integrations can refresh metadata, overlays,
  affected chunks, and tile caches after writes.
- Added `MapRenderSession` and `MapRenderSessionConfig` for long-lived
  interactive renderers that reuse the world handle, renderer, tile cache, and
  diagnostics state.
- Added cancellable streaming tile rendering through
  `render_web_tiles_streaming_blocking` and the async
  `render_web_tiles_streaming` wrapper.
- Added `TileStreamEvent` with cached, rendered, failed, progress, and complete
  events so frontends can paint visible tiles before a full batch finishes.
- Added render-index culling for missing chunks before baking, backed by
  `bedrock-world` key-only region scans.
- Added `log` facade diagnostics around session streaming, cache hits/misses,
  GPU backend selection, and CPU fallback.
- Added `BlockBoundaryRenderOptions` for default 2D per-block surface outlines
  and height-contact shadows in `SurfaceBlocks`.
- Added `RenderGpuOptions` and GPU scheduling stats for bounded in-flight GPU
  compose, queue wait timing, batch counts, and readback worker visibility.
- Added `RenderCpuPipelineOptions` and `RenderTilePriority` on `RenderOptions`
  for bounded Rayon load/bake/compose scheduling and viewport-center-first tile
  ordering.
- Added `render_web_tiles_streaming_channel`, an async channel API returning
  `tokio::sync::mpsc::Receiver<TileStreamEvent>`.
- Extended GPU scheduling with `batch_pixels`, `submit_workers`,
  `buffer_pool_bytes`, and `staging_pool_bytes`, plus buffer/staging pool reuse
  stats.
- Changed interactive streaming to render multiple cache misses per group by
  default and split the worker budget between region workers and bedrock-world
  decode workers, so GPU batches are fed by more than one tile/region when
  resources allow.
- Extended `RenderPipelineStats` and logs with world load, DB read, decode,
  region copy, GPU prepare, upload, dispatch, and readback timings plus peak
  world worker counts.
- Added rendering support for `bedrock-world` legacy terrain chunks:
  `SurfaceBlocks`, `HeightMap`, `LayerBlocks`, and `CaveSlice` now sample
  `LegacyTerrain` (`0x30`) data and map common 0.16 numeric block IDs to
  modern `minecraft:*` names.
- Added structured legacy biome rendering for old worlds. Real `LegacyTerrain`
  biome samples are decoded as `[biome_id, red, green, blue]`; `Biome` uses the
  saved RGB and prefers it over conflicting old Data2D/Data3D biome ids,
  `RawBiomeLayer` uses known saved IDs and falls back to saved RGB, and
  `SurfaceBlocks` uses the RGB for grass/foliage tint while keeping water on
  the existing water-tint path.
- Fixed mixed old/new chunks so subchunk block data wins over fallback
  `LegacyTerrain`, preventing grass surfaces from rendering as lower stone.
- Added coordinate-signature regression coverage for direct tile, shared bake,
  region/web tile, and streaming-session paths, including negative chunk
  coordinates, one-region-per-chunk composition, and top-layer surface blocks
  above misleading raw heightmaps.
- Added region/tile placement diagnostics to `RenderPipelineStats`:
  `region_chunks_copied`, `region_chunks_out_of_bounds`, and
  `tile_missing_region_samples`.
- Added `RenderMode::RawHeightMap` as the raw Data2D/Data3D/Legacy heightmap
  diagnostic mode.
- `SurfaceBlocks` and default `HeightMap` now use the same canonical visual
  terrain column samples from `bedrock-world`. The renderer no longer performs
  its own top-down surface scan, so roofs/leaves above stale raw heightmaps,
  thin overlays, water relief, and block-entity surface colors share one source
  of truth.
- Updated preview/static web-map/streaming examples to use
  `BedrockWorld::open_blocking`, enabling automatic old LevelDB and
  `chunks.dat` detection.

### Breaking Changes

- Interactive integrations should migrate from one-shot
  `render_web_tiles_blocking` calls to a reusable `MapRenderSession`. The old
  blocking batch API remains for export tools, but it no longer represents the
  recommended UI path.
- `SurfaceRenderOptions` now includes `block_boundaries`, and `RenderOptions`
  now includes `gpu`, `cpu`, and `priority`. Struct literals must add these
  fields or use `..Default::default()`.
- Renderer and GPU shader cache versions were bumped because default
  `SurfaceBlocks` output now includes block-boundary shadowing, legacy
  terrain/biome sampling changed, and renderer cache version `48` invalidates
  tiles produced before the region/chunk coordinate and computed-height
  contracts were hardened. Existing cached tiles from older legacy sampling,
  misplaced region compose, or raw-height-driven surface rendering are invalid.
- `RenderMode::HeightMap` now means computed surface height. Use
  `RenderMode::RawHeightMap` to preserve the old raw heightmap diagnostic
  behavior.
- Renderer cache version is now `48`; cached tiles from raw-height-driven
  surface rendering, older legacy biome priority, earlier coordinate/height
  fixes, or renderer-side surface rescanning are invalid.
- `RenderOptions::default().cache_policy` is now `Bypass`. Set
  `RenderCachePolicy::Use` explicitly for sessions or exports that should
  read/write tile cache entries.

### Migration Notes

- Replace per-batch world/cache construction with one session per opened world.
- Stream `TileStreamEvent` values into the UI and discard stale events by
  generation when the world, dimension, mode, layout, or cache policy changes.
- Use `RenderCancelFlag` on every viewport batch and cancel the previous flag
  before scheduling a new visible-tile request.
- Use `RenderOptions::gpu` to tune GPU compose. The default zero values select
  profile-aware in-flight, batch, readback, submit-worker, and buffer-pool
  settings.
- Use `RenderOptions::priority = RenderTilePriority::DistanceFrom { .. }` for
  interactive first-screen rendering; keep `RowMajor` for deterministic export
  order.
- Use `bedrock_render::editor::MapWorldEditor` for common write-mode map
  tooling. After a successful edit, apply `MapEditInvalidation` before
  rescheduling render work.
- For 0.16-era worlds, open with `bedrock_world::BedrockWorld::open` or
  `open_blocking` so format detection can identify `LevelDbLegacyTerrain` or
  read-only `PocketChunksDat`.
- If an integration compared `HeightMap` output against Data2D/Data3D bytes,
  migrate that check to `RawHeightMap`. Use `HeightMap` for “what the rendered
  terrain surface height is,” not “what the world saved as a height hint.”

## 0.1.0 - 2026-05-01

### Added

- Initial public crate-ready tile renderer for Minecraft Bedrock worlds.
- Surface, height map, biome, raw biome, fixed layer, and cave slice render modes.
- Embedded Bedrock block and biome color palette JSON sources, including grass,
  foliage, and water tint tables.
- Palette source schema metadata, source-policy documentation, JSON-only palette
  rebuild support, and a `palette_tool` example for audit/normalize checks.
- Clean-room block and biome palette generation, tainted-source audit checks, and
  optional local resource-pack palette derivation under `target/`.
- Semantic palette guardrails for bamboo, dirt/grass paths, tint masks, biome
  grass separation, and real surface-render biome tint output.
- Region planning, deterministic tile paths, render diagnostics, cancellation,
  progress callbacks, bounded threading, memory-budgeted region baking, and
  optional GPU terrain-light compose.
- PNG/WebP/RGBA output support, preview and static web-map examples, fixture tests,
  Criterion benches, and English/Simplified Chinese documentation.

### Notes

- The crate depends on `bedrock-world` by pinned Git revision until the Bedrock
  crate family is ready for crates.io publishing.
- Real Bedrock worlds and generated render outputs are intentionally excluded
  from version control.
