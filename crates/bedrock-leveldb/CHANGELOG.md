# Changelog

## 0.6.0 - 2026-08-17

### Breaking Changes

- Removed the public Minecraft-world semantic helpers (`BedrockKey`, `ChunkKey`, `Dimension`,
  `LegacyTerrain`, `LegacySubChunk`, `SubChunkPayload` and related constants). These concepts belong
  to `bedrock-world`; `bedrock-leveldb` now exposes only Mojang LevelDB storage-engine primitives and
  raw byte key/value APIs.
- Consumers that previously used these helpers should move chunk/key/terrain interpretation to
  `bedrock-world` 0.6 or newer.

### Compatibility

- The native table reader continues to accept compression tags `0` (none), `1` (Snappy), `2`
  (zlib-wrapped DEFLATE), and `4` (Bedrock raw DEFLATE) while preserving raw user key/value bytes.
- Added an optional local historical database fixture matrix for uncompressed, Snappy, zlib,
  Bedrock-raw-deflate, WAL replay and multi-table databases.
- Historical compatibility tests open fixtures read-only and scan raw entries without invoking
  Minecraft world-format logic or implicit repair.

### Scope

- `bedrock-leveldb` is now explicitly a high-performance pure Rust implementation of the storage
  boundary used by Mojang's modified LevelDB: WAL, SSTable, manifest, compression, checksum, cache,
  snapshot, scan, batch, flush, compaction and repair mechanics only.
- NBT, chunk keys, BlockState, actors, biomes, maps, players and world migrations are exclusively
  handled by `bedrock-world`.

## 0.5.1 - 2026-08-17

### Added

- Added `WriteBatch::encoded_len_hint` so callers can reserve WAL buffers and make backpressure decisions without first encoding the batch.
- Added `WriteBatch::compact_last_write_wins` for storage-engine-level duplicate-key elimination inside one atomic batch.

### Changed

- `WriteBatch::encode` now reserves a conservative target capacity up front, reducing reallocations on large map-editor transactions.
- Batch compaction preserves LevelDB last-write-wins semantics and the relative order of retained final operations, reducing redundant WAL and memtable traffic for repeated writes to the same chunk record.

### Scope

- This crate remains a Mojang Bedrock LevelDB storage engine only. Chunk, SubChunk, BlockState, NBT game semantics, entities, heightmaps, and world migration remain the responsibility of `bedrock-world`.

## 0.5.0

- Replace the aggregate `OpenOptions::cache_size` setting with independent sharded native cache capacities.
- Remove `NativeCacheOptions::from_total` and `Db::open_with_cache_options`; no legacy cache configuration bridge remains.
- Add compact table identities, bounded file handles, cache statistics, incremental WAL recovery accounting, and allocation-reduced exact batch reads.

All notable changes to `bedrock-leveldb` are tracked here.

## 0.4.0 - 2026-08-03

### Changed

- Native SSTables are emitted through a bounded streaming writer instead of
  building the complete table and a nested copy of every data block in memory.
- Native data and index blocks now use restart interval 16 with shared-key
  prefix compression.
- Database reads hold a shared database guard instead of cloning an
  `Arc<BTreeMap>` snapshot and forcing later writers through
  `Arc::make_mut` full-map copies.
- Default reads use the native cache. Large one-shot scans can still opt out
  with `CachePolicy::Bypass`.
- Native table files are cached as shared handles and read with positional I/O,
  avoiding repeated opens and shared seek cursors.
- Exact batches keep unresolved keys sorted and group them by table range and
  native data block.
- Parallel scans reuse bounded Rayon pools and default to a `workers × 4`
  queue instead of a per-record `workers × 256` backlog.

### Fixed

- Full compaction now rewrites the complete visible state, commits the new
  manifest and WAL before removing obsolete files, and reclaims old tables,
  tombstones, and overlay state.
- Empty explicit flushes no longer create empty or redundant tables.
- Native table scans use one previous-key buffer for duplicate-version
  suppression instead of allocating a `BTreeSet<Vec<u8>>` per table scan.
- Overlay shadow checks borrow the read-guarded memtable directly instead of
  cloning every hidden key, and parallel entry byte counts are no longer
  accumulated twice.

### Compatibility

- Long reads now block writes for the duration of the read guard. This trades
  write concurrency for bounded memory and removes full-overlay copy-on-write
  amplification. A generation-based memtable can replace this boundary in a
  later release without changing the public API.

## 0.3.1 - 2026-07-14

### Fixed

- Fixed native-table reader lint failures that blocked the `0.3.0` publish
  workflow.

## 0.3.0 - 2026-07-14

### Changed

- Split native SSTable payloads into bounded data blocks and added a bounded
  native index cache, improving point reads and range scans on larger tables.
- Hardened native table reads for concurrent access by avoiding shared seek
  cursors on cached file handles.

## 0.2.2 - 2026-06-28

### Changed

- Disabled Tokio default features for the optional `async` feature. The crate
  now enables only the Tokio runtime pieces it needs for `spawn_blocking`
  wrappers.

### Fixed

- Made `OpenOptions::write_buffer_size = 0` disable automatic native table
  flushes instead of flushing after every write. WAL-backed writes now remain in
  the overlay until an explicit flush, compaction, or recovery path consumes
  them.

## 0.2.1 - 2026-05-07

### Documentation

- Added docs.rs all-features metadata and expanded the crate-level feature
  overview for the hosted API reference.
- Clarified crates.io package contents and feature behavior in the English and
  Simplified Chinese READMEs.

### Fixed

- Replaced an unstable `if` let-chain in native table multi-get code with
  stable Rust syntax, preserving compatibility with the crate's declared
  minimum supported Rust version.

## 0.2.0 - 2026-05-07

### Added

- Added native LevelDB write APIs: `Db::write_batch_native`,
  `Db::flush_memtable`, `Db::compact_range_native`, and `Db::recover_native`.
- Added standard LevelDB WAL batch append, native `.ldb` flush, manifest
  version edit persistence, sequence-number visibility, and deletion tombstone
  replay for the v0.2 write path.
- Added key-only prefix scans with `Db::for_each_prefix_key` so render indexes
  can discover chunk records without materializing unrelated values.
- Added owned async read helpers for shared handles:
  `Arc<Db>::get_async`, `Arc<Db>::get_with_async`,
  `Arc<Db>::collect_keys_owned_async`,
  `Arc<Db>::collect_prefix_keys_owned_async`, and
  `Arc<Db>::collect_prefix_owned_async`.
- Added owned sync collectors: `collect_keys_owned`,
  `collect_prefix_keys_owned`, and `collect_prefix_owned`.
- Added `ReadOptions::pipeline` / `ScanPipelineOptions` for bounded queue depth,
  table batch sizing, and progress cadence in parallel scans.
- Added `ScanOutcome` diagnostics for `tables_scanned`, `worker_threads`,
  `queue_wait_ms`, and `cancel_checks`.
- Added `get_many_owned` regression coverage for early Bedrock raw chunk-record keys, preserving
  missing/duplicate/input ordering without interpreting those keys in the storage layer.
- Clarified that legacy biome priority and terrain interpretation are world semantics; this crate only
  preserves raw key/value bytes and input ordering.
- Documented the old-world LevelDB boundary: native zlib tag `2`, raw deflate tag `4`, WAL + `.ldb`,
  and exact raw reads are supported here; pre-LevelDB `chunks.dat` files remain a `bedrock-world`
  concern.
- Added clearer Rayon worker logging around scan start/finish, prefix scans,
  progress, queue backpressure, and cancellation-sensitive paths through the
  `log` facade.

### Breaking Changes

- Visitor callbacks used with table-parallel APIs must be `Send` because scans
  now run on a local Rayon thread pool.
- Struct literals for `ReadOptions` must set `pipeline` or use
  `..ReadOptions::default()`.
- New writes now use native LevelDB-compatible files. The old `BWLDB...` format
  remains readable for migration/backward compatibility, but is no longer the
  default flush output.
