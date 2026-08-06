# Changelog

All notable changes to `bedrock-world` are tracked here.

## 0.4.0 - 2026-08-04

### Added

- Added stack-backed `EncodedChunkKey` encoding and allocation-free
  `BedrockDbKeyKind` classification for storage hot paths.
- Added callback-based `visit_nbt_events` / `NbtView::visit_events` parsing
  with immediate early termination and no intermediate event vector.
- Forwarded exact-get and native cache counters through `StorageScanOutcome`.

### Changed

- Updated the optional `bedrock-leveldb` backend to 0.4.0.
- Exact surface projection decodes packed palette words once per storage,
  precomputes terrain roles, and stores results in a fixed 16×16 column grid.
- Writable LevelDB worlds use a bounded 4 MiB write buffer.

### Fixed

- `BedrockLevelDbStorage::flush` and `compact` now call the underlying
  database instead of returning success without doing any work.

## 0.3.5 - 2026-07-18

### Added

- Added atomic `delete_chunk_positions_blocking` support for deduplicated
  multi-chunk deletion.
- Added transaction helpers for replacing a chunk and staging validated block
  entities or hardcoded spawn areas in one commit.

### Changed

- Made `WriteGuard::validate` public for callers that coordinate their own
  storage transactions.
- Structure placement now recalculates touched heightmap columns and commits
  chunk writes in batches of 16, with progress reported after each commit.

### Fixed

- Preserved inherited biome storage entries during Data3D encoding.

## 0.3.4 - 2026-07-17

### Added

- Added borrowed chunk-record parsing APIs so callers can retain raw records
  and build structured data from a single storage scan.

## 0.3.3 - 2026-07-14

### Fixed

- Fixed the publish workflow to wait for the declared `bedrock-leveldb`
  dependency version instead of the `bedrock-world` release version.

## 0.3.2 - 2026-07-14

### Fixed

- Fixed Clippy findings in chunk-record fingerprinting and biome request
  handling that blocked the `0.3.1` publish workflow.

## 0.3.1 - 2026-07-14

### Changed

- Updated the optional `bedrock-leveldb` backend dependency to `0.3.1` after
  the `0.3.0` release workflow was blocked before publication.

## 0.3.0 - 2026-07-14

### Added

- Added composable `ChunkDataRequest` loading, `ChunkData` results, chunk-record
  batch queries, pending-tick overlays, storage cache policies, and shared
  terrain-surface helpers.

### Changed

- Replaced the render-specific chunk-loading API with general chunk-query types
  such as `ChunkLoadOptions`, `ChunkLoadPriority`, and
  `WorldChunkQueryRegion`, so renderers and other consumers share one data
  loading contract.
- Updated the README, API guide, benchmarks, and testing guidance for the new
  chunk-query and surface-data APIs.
- Updated the optional `bedrock-leveldb` backend dependency to `0.3.0`.

## 0.2.2 - 2026-06-29

### Added

- Added `.mcstructure` read/write and world placement helpers through the
  `mcstructure` module, including structure palette handling, chunk targeting,
  rotation/mirroring, block-entity preservation, and placement progress.
- Added `BedrockWorld::compact_storage_blocking` and `WorldStorage::compact`
  so bulk-write tools can request explicit backend compaction after committing
  changes.

### Changed

- Updated the optional `bedrock-leveldb` backend to `0.2.2`, kept the local
  repository path for development, disabled default backend features, and
  forwards zlib/snappy plus the async feature explicitly.
- Changed LevelDB writes to use synced write options with WAL-backed writes and
  made flush a cheap backend boundary. Transaction commits now persist the
  batch and leave optional compaction to explicit callers.
- Increased the NBT container limit for large Bedrock structure arrays.

### Fixed

- `delete_chunks_blocking` now removes modern actor digest and `actorprefix`
  records for deleted chunks, not only the terrain records.

## 0.2.1 - 2026-05-07

### Changed

- Added docs.rs all-features metadata, crate-level feature guidance, and
  README sections describing hosted docs, feature behavior, and package
  contents.
- Added complete public API rustdoc coverage and enabled the release workflow's
  missing-docs gate for all-feature rustdoc builds.
- Prepared the crate for publishing by removing `publish = false` and declaring
  the optional `bedrock-leveldb` backend as a versioned `0.2.1` dependency while
  retaining the local `../bedrock-leveldb` path for repository development.
- Clarified documentation scope: `bedrock-world` focuses on complete parsing and
  references `bedrock-dev/bedrock-level` only for parsing behavior.
- Reframed map pixel support documentation as implemented parser coverage
  instead of an unimplemented feature.

## 0.2.0 - 2026-05-07

### Added

- Added typed BedrockLevelFormat key APIs for map records, global records,
  `actorprefix`, `digp`, dimension records, local-player aliases, and known
  global keys.
- Added high-level map/global record read, scan, write, and delete APIs with
  parse -> serialize -> parse roundtrip validation before writes.
- Added chunk payload helpers for Data2D/Data3D heightmaps and biome storage,
  hardcoded spawn areas, block entities, and modern actor digest/prefix records.
- Added actor APIs that read both legacy inline `Entity` chunk records and
  modern `digp -> actorprefix` records, while writes update actorprefix and
  digest records in one transaction.
- Added render-only chunk position indexes:
  `list_render_chunk_positions_blocking` and
  `list_render_chunk_positions_in_region_blocking`, plus async wrappers.
- Added async wrappers for render chunk, render chunk batch, render region, and
  render chunk position loading.
- Added `threading` to `RenderChunkLoadOptions` and
  `RenderRegionLoadOptions`, allowing bounded parallel render chunk loading.
- Added `WorldPipelineOptions` for queue depth, chunk batch sizing, subchunk
  decode worker budgeting, and scan progress cadence.
- Added `RenderChunkPriority::{RowMajor, DistanceFrom { .. }}` so viewport
  loaders can deliver chunks closest to the current view first.
- Added `cancel`, `progress`, and `priority` to render chunk and render region
  load options.
- Added `RenderRegionData::stats` / `RenderLoadStats` with requested/loaded
  chunk counts, subchunk decode counts, worker counts, queue wait, and load time.
- Added `WorldFormatHint` and `WorldFormat` with automatic detection for modern
  LevelDB, old LevelDB `LegacyTerrain`, and read-only Pocket Edition
  `chunks.dat` worlds.
- Added `PocketChunksDatStorage`, a read-only backend that exposes old
  `chunks.dat` payloads as virtual `LegacyTerrain` (`0x30`) records.
- Added exact-batch render loading of `LegacyTerrain`, plus
  `RenderChunkData::legacy_terrain` and `RenderLoadStats` fields for
  `legacy_terrain_records`, `legacy_pocket_chunks`, and `detected_format`.
- Added structured legacy biome exposure for `LegacyTerrain` render loads
  through `RenderChunkData::legacy_biomes`. Legacy biome samples are decoded as
  `[biome_id, red, green, blue]`; `legacy_biome_colors` remains as
  compatibility `0x00RRGGBB` output. Stats now include
  `legacy_biome_samples`, `legacy_biome_preferred_columns`,
  `modern_biome_fallback_columns`, and legacy terrain/subchunk source counts.
- Legacy biome samples now take priority over conflicting old Data2D/Data3D
  biome ids for exact surface render loads, preventing saved 0.16 RGB biome
  colors from being overwritten by stale numeric biome records.
- Mixed transition chunks that contain both `LegacyTerrain` and
  `SubChunkPrefix` now keep subchunk records and prefer subchunk block data;
  `LegacyTerrain` is only the terrain fallback for missing subchunks.
- Added exact-batch coordinate regression coverage for shuffled and
  priority-sorted render chunk loads, ensuring records stay bound to their
  original `ChunkPos` across modern, legacy, and transition render paths.
- Added canonical terrain column sampling through `RenderChunkRequest::ExactSurface`,
  `TerrainColumnSample`, and `RenderChunkData::column_samples`. Exact surface
  render loads now scan actual block columns top-down across modern paletted
  subchunks, legacy subchunks, and `LegacyTerrain`, recording computed surface
  block/height, relief support block/height, optional thin overlay, water
  context, biome, and terrain source.
- Added render load diagnostics for computed surface columns, raw height
  mismatches, missing surface columns, and legacy terrain fallback columns.
- Added `log` facade diagnostics for render region indexing and render chunk
  worker execution.

### Breaking Changes

- `WorldScanOptions`, `RenderChunkLoadOptions`, and `RenderRegionLoadOptions`
  now include pipeline/cancel/progress/priority fields. Use
  `..Default::default()` or set `WorldThreadingOptions::Single` when an outer
  renderer already schedules work.
- `OpenOptions` now includes `format: WorldFormatHint`; struct literals must set
  it or use `..OpenOptions::default()`.
- `RenderChunkData` now includes `legacy_terrain`, `legacy_biomes`, and
  `legacy_biome_colors`.
- `RenderChunkLoadOptions` and `RenderRegionLoadOptions` now use
  `RenderChunkRequest` instead of freely combinable surface/raw-height/layer
  fields. Callers that only need raw height records should explicitly request
  `RenderChunkRequest::RawHeightMap`.
- `ExactSurface` defaults to full surface subchunk loading so computed terrain
  columns are not bounded by stale raw heightmap hints.
- Two-dimensional height/biome column order is treated as `z * 16 + x`.
  Existing callers that relied on the old transposed interpretation must update
  their fixtures and coordinate expectations.

### Migration Notes

- Replace full-world chunk scans used only for visible map rendering with
  `list_render_chunk_positions_in_region_blocking` or its async wrapper.
- Use `load_render_chunks_blocking`/`load_render_chunks` for viewport batches
  instead of repeatedly loading single chunks on the UI thread.
- For interactive maps, set `RenderChunkPriority::DistanceFrom` to the center
  chunk of the visible viewport and inspect `RenderRegionData::stats` before
  increasing worker counts.
- Old 0.16-era LevelDB worlds no longer need `Data2D`/subchunk records to be
  considered renderable; use the render exact-batch APIs and verify
  `RenderLoadStats::prefix_scans == 0`.
- For old worlds, prefer `RenderChunkData::legacy_biomes` so callers receive
  both the numeric biome ID and the saved RGB. Use `legacy_biome_colors` only
  as a compatibility `0x00RRGGBB` view.
- For map rendering, use `RenderChunkRequest::ExactSurface` and consume
  `column_samples` instead of trusting raw Data2D/Data3D/Legacy heightmap
  values. Raw heightmaps are now hints/diagnostics, not the source of truth for
  terrain surface rendering.
- Pre-LevelDB `chunks.dat` worlds are read-only. Open them with
  `BedrockWorld::open_blocking`/`open` and `WorldFormatHint::Auto` or
  `WorldFormatHint::PocketChunksDat`.

## 0.1.0 - 2026-05-01

### Added

- Initial public crate-ready world API for `level.dat`, little-endian NBT,
  Bedrock DB key classification, player reads, chunk/subchunk parsing, entity
  and block-entity parsing, item extraction, biome summaries, maps, villages,
  globals, and scan progress/cancellation.
- Legacy LevelDB-era chunk parsing for `LegacyTerrain` records and
  pre-paletted `SubChunkPrefix` payloads, including block ID, metadata, and
  optional light arrays.
- Paletted subchunk parsing for old single-storage v1 payloads and modern
  v8/v9 storage-count payloads.
- Lazy `BedrockWorld` APIs backed by `bedrock-leveldb`, async wrappers,
  benches, fixture tests, and English/Simplified Chinese documentation.
- Stable `BedrockWorldErrorKind` categories for application error handling.
- Read-only enforcement for batched world transactions and read-only LevelDB
  opens through `BedrockWorld::open`.
- Optional large-world fixture policy, contribution notes, API/testing guides,
  and dual MIT/Apache-2.0 license files.

### Notes

- Full semantic migration of historical gameplay data remains intentionally
  scoped to structured chunk/world parsers, not the lower-level storage crate.
- Complete structured editing for every chunk version is not implemented in this
  release.
