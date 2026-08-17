# Changelog

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
- Added `get_many_owned` regression coverage for early Bedrock
  `LegacyTerrain` (`0x30`) keys, preserving missing/duplicate/input ordering.
- Reaffirmed the storage-layer contract for renderer coordinate debugging:
  `get_many_owned` returns raw `LegacyTerrain`, legacy `SubChunkPrefix`, and
  modern `SubChunkPrefix` bytes unchanged; coordinate interpretation belongs to
  `bedrock-world` and `bedrock-render` tests.
- Clarified that legacy biome priority is also a world/render semantic; this
  crate only preserves the raw `LegacyTerrain` bytes and input ordering.
- Documented the old-world LevelDB boundary: native zlib tag `2`, raw deflate
  tag `4`, WAL + `.ldb`, and exact `LegacyTerrain` reads are supported here;
  pre-LevelDB `chunks.dat` files remain a `bedrock-world` backend concern.
- Corrected the `LegacyTerrain` helper's biome accessor so the final 1024-byte
  tail is exposed as `[biome_id, red, green, blue]` samples, with
  `biome_color_at` returning compatibility `0x00RRGGBB`.
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

### Migration Notes

- Render and world callers that previously used `for_each_prefix` only to collect
  keys should migrate to `for_each_prefix_key`.
- Async callers should wrap `Db` in `Arc` and use the owned async helpers instead
  of reopening the database per request.
- Tune `ScanPipelineOptions` only after looking at `ScanOutcome.queue_wait_ms`
  and `worker_threads`; the default zero values are automatic and usually best
  for interactive render indexing.

## 0.1.0 - 2026-05-01

### Added

- Initial public crate-ready implementation of a pure Rust LevelDB-style backend
  for Minecraft Bedrock world databases.
- Read-first native LevelDB support for manifest, WAL, table blocks, prefix
  scans, cache controls, cooperative scan cancellation, and progress reporting.
- Custom write, delete, batch, flush, and reopen support using this crate's
  documented `BWLDB...` table format.
- Bedrock LevelDB key helpers plus documented legacy `LegacyTerrain` and
  pre-paletted `SubChunkPrefix` payload helpers.
- `log` facade diagnostics, structured errors, CI, Criterion benchmarks, package
  metadata, and English/Simplified Chinese documentation.

### Notes

- Native LevelDB-compatible writes and compaction are intentionally not part of
  this release.
- Pre-LevelDB Bedrock files such as `chunks.dat` and `entities.dat` are outside
  this crate's storage scope.
