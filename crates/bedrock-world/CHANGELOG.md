# Changelog

All notable changes to `bedrock-world` are tracked here.

## 0.6.0 - 2026-08-17

### Breaking Changes

- `BlockEditOptions` now carries an explicit `WritePolicy`; struct literals must set it or use
  `..BlockEditOptions::default()`.
- The optional storage backend now requires `bedrock-leveldb` 0.6.0, whose Minecraft world-semantic
  helpers were intentionally removed. All chunk/key/terrain semantics are owned by `bedrock-world`.

### Added

- Added capability-based historical-format classification through `CompatibilityLevel`,
  `WorldCapabilities`, `ChunkCapabilities`, `SubChunkCodecKind`, and `ActorStorageModel`.
- Added a single-pass `scan_world_compatibility_blocking` report that detects mixed historical and
  modern chunk records, legacy inline entities, modern actor digest/prefix storage, unknown chunk
  tags and future subchunk versions from the actual database population.
- Added explicit `WritePolicy::{Preserve,Migrate,Refuse}` semantics.
- Added classification for legacy v0, paletted v1, legacy v2-v7, paletted v8/v9, and unknown future
  subchunk versions while retaining raw unsupported payloads.
- Added an explicit pre-LevelDB Pocket `chunks.dat` importer that migrates the container into
  `LegacyTerrain` records but deliberately reports that semantic chunk migration is still required.
- Added a cross-version local fixture matrix covering Pocket/early Bedrock, legacy LevelDB terrain,
  extended-height and actor-storage transitions, modern worlds, and synthetic future-format records.
- Added strict block-state upgrade validation hooks so a migration can be checked against an
  authoritative target palette before it is accepted for writing.

### Changed

- World compatibility is now derived from record/chunk capabilities instead of assuming one global
  `StorageVersion` describes every record in a partially upgraded world.
- Typed block edits are exact-format writers: historical chunks require an explicit migration first,
  while future/unknown formats are always protected from destructive structured rewrites.
- Ordinary block edits preserve an existing `FinalizedState` record instead of forcing a guessed
  value. Generation/finalization state is owned by world generation and migration flows.
- `bedrock-world` is explicitly the owner of Minecraft semantics above the raw `bedrock-leveldb`
  storage layer.

### Safety

- Unknown future records remain raw-preserved and are not considered safe for destructive writes.
- Legacy formats require an explicit migration path; `Preserve` mode only permits exact
  round-trippable chunk formats.
- Existing `level.dat` `RandomSeed` values remain map-owned metadata and are never overwritten by a
  caller-provided candidate seed.
- Future block-state storage versions are preserved and rejected by strict downgrade/write paths.

## 0.5.3 - 2026-08-17

### Added

- Added the initial historical compatibility/capability API and fixture planning that was consolidated
  into the 0.6.0 breaking boundary release.

## 0.5.2 - 2026-08-17

### Added

- Added a read-only whole-world integrity auditor covering `level.dat`, chunk/subchunk parseability, palette block-state versions, player/entity/block-entity NBT, and `digp` ↔ `actorprefix` ownership relationships.
- Added typed modern paletted-chunk block editing with chunk-grouped writes, primary/secondary block layers, block-entity replacement/removal, heightmap updates, and bounded transactional commit batches.
- Added a data-driven `BlockStateUpgrader` with identifier/state rename/remove/set/value-rewrite rules and strict unresolved-state handling.

### Changed

- Updated the optional `bedrock-leveldb` backend requirement to 0.5.1.
- Future block-state storage versions are now classified separately and rejected by strict upgrade paths instead of being treated as current data.
- Typed edits refuse unsupported/legacy subchunk encodings rather than guessing a destructive rewrite.

### Safety

- Existing `level.dat` `RandomSeed` values are map-owned metadata: helper APIs only initialize a seed when the field is absent and never overwrite an existing valid value.
- Unknown or future block-state schemas remain preserved/unresolved until an explicit migration rule exists.

## 0.5.1 - 2026-08-17

### Added

- Added canonical Bedrock `BlockState` NBT/byte helpers and semantic equality so world, editor, renderer, and server consumers share one order-stable identity definition.
- Added `read_level_dat_random_seed` and `initialize_level_dat_random_seed_if_missing` for safe map-owned seed handling.

### Changed

- Canonical block-state identity excludes storage-version metadata and emits state keys in deterministic lexical order.

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
  complete storage batch through one backend write.
