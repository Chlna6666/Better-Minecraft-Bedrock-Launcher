# bedrock-world

[English](README.md) | [简体中文](README.zh-CN.md)

`bedrock-world` is a Minecraft Bedrock world library built on top of
`bedrock-leveldb`. It provides fast `level.dat` access, little-endian NBT,
Bedrock DB key classification, player reads, chunk/subchunk parsing including
LevelDB-era legacy terrain records, entity and block-entity parsing, item
extraction, biome summaries, typed map/village/global record access, and
Bedrock `.mcstructure` file import/export helpers.

The performance model is benchmark-backed rather than "femtosecond" marketing:
hot paths avoid owned raw-value retention, use borrowed/event NBT views when a
DOM is not needed, reuse the default index/data caches for point reads, and let
one-shot scans bypass those caches when lock avoidance is more important.

This crate focuses on complete parsing behavior. The
`bedrock-dev/bedrock-level` project is referenced only for parsing behavior.

## Recommended API

- `read_level_dat(path)` and `write_level_dat_atomic(path, document)` are the
  launcher fast path. They do not open LevelDB.
- `BedrockWorld::open(path, OpenOptions)` creates a lazy world handle backed by
  `bedrock-leveldb` or the read-only legacy `chunks.dat` backend; it does not
  parse the full world.
- `BedrockWorld<S>` is generic over the storage handle. Compatibility
  constructors such as `open_blocking`, `open`, and `from_storage` return the
  dynamic `Arc<dyn WorldStorage>` form. Hot paths can use
  `BedrockWorld::from_typed_storage` or `BedrockWorld::open_typed_blocking` to
  keep `BedrockLevelDbStorage` or `MemoryStorage` as the concrete backend.
- `OpenOptions::format` defaults to `WorldFormatHint::Auto`. Auto opens
  `db/CURRENT` worlds as LevelDB, marks early `StorageVersion <= 4` worlds as
  `WorldFormat::LevelDbLegacyTerrain`, and opens pre-LevelDB `chunks.dat` worlds
  as `WorldFormat::PocketChunksDat`.
- `OpenOptions::default()` is read-only. Any world-record write must reopen the
  world with `OpenOptions { read_only: false, ..OpenOptions::default() }`.
  Read-only worlds return `BedrockWorldErrorKind::ReadOnly` from high-level
  writes before touching storage.
- Use category APIs for UI and tools:
  `classify_keys_blocking`, `list_players_blocking`,
  `list_chunk_positions_blocking`, `parse_chunk_blocking`,
  `parse_subchunk_blocking`, `scan_entities_blocking`,
  `scan_block_entities_blocking`, `scan_items_blocking`, `scan_maps_blocking`,
  `scan_villages_blocking`, and `scan_globals_blocking`.
- Use typed v0.2 BedrockLevelFormat write APIs on a writable world:
  `write_map_record_blocking`, `delete_map_record_blocking`,
  `write_global_record_blocking`, `delete_global_record_blocking`,
  `put_heightmap_blocking`, `put_biome_storage_blocking`,
  `put_hsa_for_chunk_blocking`, `delete_hsa_for_chunk_blocking`,
  `put_block_entities_blocking`, `edit_block_entity_at_blocking`,
  `delete_block_entity_at_blocking`, `put_actor_blocking`,
  `delete_actor_blocking`, `move_actor_blocking`, and
  `delete_chunk_positions_blocking`. Matching async wrappers are available
  behind the default `async` feature.
- High-level writes serialize and parse records back before commit. Actor writes
  update `digp -> actorprefix` records in one transaction. Block-entity writes
  validate coordinates against the target chunk. `PocketChunksDatStorage`
  remains read-only.
- LevelDB backend writes use synced WAL-backed write options. Bulk editors can
  call `compact_storage_blocking` after a write wave when they want an explicit
  backend compaction boundary.
- `bedrock-world` stops at Bedrock key/value semantics. Post-write refresh,
  invalidation, and presentation policy belong to downstream applications or
  adapter crates.
- Async wrappers use `tokio::task::spawn_blocking`, so disk and decode work does
  not block the foreground async runtime.
- `WorldScanOptions` controls threading, cancellation, and progress callbacks.
- `NbtReader::view().events()` provides a borrowed event stream for tools that
  need to inspect NBT without constructing an owned `NbtTag` DOM.
- `McStructureFile::read_from_path`, `McStructureFile::from_world_region_blocking`,
  and `McStructureFile::write_to_world_blocking` cover Bedrock
  `.mcstructure` import, export, and placement. Placement supports chunk
  targeting, Y offsets, horizontal rotation/mirroring, and block entities;
  touched heightmap columns are recomputed and writes commit in 16-chunk
  batches.
- `WorldPipelineOptions` refines the bounded pipeline with queue depth, chunk
  batch size, subchunk decode worker budget, and progress cadence. Zero values
  choose automatic defaults.
- Render-specific APIs now have their own fast path:
  `list_render_chunk_positions_blocking`,
  `list_render_chunk_positions_in_region_blocking`,
  `query_chunk_data_blocking`, `query_chunk_data_many_blocking`, and
  `query_chunk_region_blocking`. These only read records needed to render
  chunks and can run with bounded parallelism.
- Render chunk data now carries `legacy_terrain: Option<LegacyTerrain>`,
  structured `legacy_biomes`, and compatibility `legacy_biome_colors`.
  `LegacyTerrain` biome samples are decoded as `[biome_id, red, green, blue]`;
  the compatibility color is exposed as `0x00RRGGBB`. Exact surface sampling
  prefers those saved legacy RGB samples over conflicting old Data2D/Data3D
  biome ids and records `legacy_biome_preferred_columns` in load stats.
  `LegacyTerrain` records
  are requested through exact batch reads, so 0.16-era LevelDB worlds do not
  need `Data2D` or `SubChunkPrefix` to be considered renderable.
- `ChunkLoadOptions::request` selects one render load contract:
  `ExactSurface` computes canonical top-down surface columns, `RawHeightMap`
  loads only raw height records for diagnostics, and `Layer`/`Biome` load fixed
  slices. `ExactSurface` exposes `ChunkData::column_samples` with the
  real visual surface block, relief/support block, optional thin overlay, water
  context, biome sample, and source for every sampled X/Z column.
- Transition chunks that contain both `LegacyTerrain` and `SubChunkPrefix`
  keep both records; renderers should prefer subchunk block data and use legacy
  terrain/biome colors only as fallbacks.
- `parse_world_blocking(WorldParseOptions)` is an explicit advanced/offline API,
  not a launcher default path.
- Public fallible APIs return `bedrock_world::Result<T>`. Match
  `BedrockWorldError::kind()` for stable categories such as read-only handles,
  cancellation, malformed NBT, unsupported chunk formats, and backend errors.

More detailed API, testing, and benchmark notes are in
[`docs/API.md`](docs/API.md), [`docs/TESTING.md`](docs/TESTING.md), and
[`docs/BENCHMARKS.md`](docs/BENCHMARKS.md).

```rust
use bedrock_world::{
    read_level_dat, BedrockWorld, OpenOptions, WorldScanOptions, WorldThreadingOptions,
};

async fn inspect_world() -> bedrock_world::Result<()> {
    let level = read_level_dat("path/to/minecraftWorld/level.dat")?;
    println!("level.dat version={}", level.header.version);

    let world = BedrockWorld::open("path/to/minecraftWorld", OpenOptions::default()).await?;
    let players = world.list_players().await?;
    println!("players={}", players.len());

    let key_counts = world
        .classify_keys(WorldScanOptions {
            threading: WorldThreadingOptions::Auto,
            ..WorldScanOptions::default()
        })
        .await?;
    println!("key categories={}", key_counts.len());

    Ok(())
}
```

## Parse Strategies

| Strategy | Raw entries | Raw values | Subchunk indices | Actor resolution | Intended use |
| --- | ---: | ---: | ---: | --- | --- |
| `WorldParseOptions::summary()` | no | no | counts only | referenced actors | UI summaries, large scans |
| `WorldParseOptions::structured()` | selected parsed entries | no | counts only | referenced actors | inspection tools |
| `WorldParseOptions::full_raw()` / `full()` | yes | yes | full 4096 indices | all actors | debugging/offline analysis |

`WorldParseCategories` controls whether chunks, players, entities, block
entities, items, maps, villages, globals, and key counts are parsed. Summary mode
keeps counts and structured summaries while avoiding raw value retention.

## Performance Model

- Launcher operations that only need `level.dat` should use `read_level_dat` and
  `write_level_dat_atomic`; this path touches only `level.dat`.
- `BedrockWorld::open` is lazy and delegates DB access to `bedrock-leveldb`.
- Key classification uses key-only scans and does not retain values.
- Viewport rendering should use
  `list_render_chunk_positions_in_region_blocking` or its async wrapper before
  loading render chunks. This probes each visible chunk with key-only prefix
  scans and skips chunks that have no render records.
- For interactive tile rendering, `load_render_chunks_with_stats_blocking` uses
  exact `get_many` requests for `LegacyTerrain`, biome records, subchunks, and
  block entities. `ChunkLoadStats::prefix_scans` should remain `0` on this
  exact path; `legacy_terrain_records`, `legacy_biome_samples`,
  `legacy_biome_colors`,
  `terrain_source_legacy`, `terrain_source_subchunk`, `legacy_pocket_chunks`,
  and `detected_format` identify old-world and transition-world loads.
- Exact render chunk batches preserve the association between every requested
  `ChunkPos` and its records even when the input is shuffled, duplicated, or
  resorted by `ChunkLoadPriority`. If a renderer shows chunk-level visual
  scrambling, compare these exact-batch stats with renderer placement
  diagnostics before changing parser coordinate formulas.
- Chunk parsing uses prefix scans and the LevelDB native block cache; repeated
  sample chunk reads avoid full table scans.
- Default world scans use automatic bounded parallel table scanning. Use
  `WorldThreadingOptions::Single` for deterministic debugging.
- `ChunkLoadOptions::threading` and `WorldChunkQueryRegionLoadOptions::threading`
  control parallel render chunk loading. Use `Single` when an outer renderer
  already owns the worker pool to avoid nested oversubscription.
- `ChunkLoadOptions::priority` can use
  `ChunkLoadPriority::DistanceFrom { chunk_x, chunk_z }` so the current
  viewport center loads first. `RenderRegionData::stats` reports requested and
  loaded chunks, decoded subchunks, worker count, queue wait, and total load
  time.
- Long scans can be cancelled through `CancelFlag` and can report progress
  through `ProgressSink`.

### Current Large Fixture Baseline

Local run on Windows, Rust bench profile, `2026-05-03`, fixture
`C:\Users\Administrator\Desktop\BE-Community-Dev\bedrock-world\tests\fixtures\sample-bedrock-world`.
The fixture is local-only data and is not a CI contract.

Latest large-fixture numbers are tracked in
[`docs/BENCHMARKS.md`](docs/BENCHMARKS.md).

## Legacy World Formats

`bedrock-world` keeps LevelDB and pre-LevelDB worlds behind the same
`WorldStorage` abstraction:

- `WorldFormat::LevelDb` for current Bedrock LevelDB worlds.
- `WorldFormat::LevelDbLegacyTerrain` for old LevelDB worlds whose renderable
  chunk data is stored in `LegacyTerrain` tag `0x30`.
- `WorldFormat::PocketChunksDat` for old Pocket Edition worlds with
  `chunks.dat`. The `PocketChunksDatStorage` backend is read-only and exposes
  each terrain payload as a virtual `LegacyTerrain` record. Mutating methods
  return `UnsupportedChunkFormat`.

```rust
let world = bedrock_world::BedrockWorld::open_blocking(
    "path/to/minecraftWorld",
    bedrock_world::OpenOptions::default(),
)?;
println!("detected format: {:?}", world.format());
```

### Migration: full chunk scan to viewport render index

Old map viewers often waited for a full-world chunk scan before rendering:

```rust
let all_chunks = world
    .list_chunk_positions_blocking(WorldScanOptions::default())?;
let visible = all_chunks
    .into_iter()
    .filter(|pos| viewport.contains(*pos))
    .collect::<Vec<_>>();
```

Prefer a render-only region query for the current viewport:

```rust
let visible = world.list_render_chunk_positions_in_region_blocking(
    bedrock_world::WorldChunkQueryRegion {
        dimension,
        min_chunk_x,
        min_chunk_z,
        max_chunk_x,
        max_chunk_z,
    },
    WorldScanOptions {
        threading: WorldThreadingOptions::Auto,
        cancel: Some(cancel),
        progress: Some(progress),
        ..WorldScanOptions::default()
    },
)?;
```

Then load just those chunks for the tile or viewport batch:

```rust
let chunks = world.query_chunk_data_many_blocking(
    visible,
    bedrock_world::ChunkLoadOptions {
        threading: WorldThreadingOptions::Fixed(4),
        priority: bedrock_world::ChunkLoadPriority::DistanceFrom {
            chunk_x: viewport_center_x,
            chunk_z: viewport_center_z,
        },
        ..bedrock_world::ChunkLoadOptions::default()
    },
)?;
```

## Fixture Result

The optional fixture at `tests/fixtures/sample-bedrock-world` is a large local
Bedrock world with native `.ldb` tables and WAL data. It is intentionally
ignored by Git because real worlds are large and may contain player data. When
the folder is missing, the fixture test and large-world benches print a skip
message and continue successfully.

```text
cargo test -p bedrock-world -- --nocapture

unit tests: 36 passed; 0 failed; finished in 0.03s
fixture test: 1 passed; 0 failed; finished in 15.85s

db.entries.count=4571643
db.entries.key_bytes=63624175
db.entries.value_bytes=8398184492
db.chunk.positions.count=237534
db.unknown_keys.first=[]
parsed.sample_chunk.pos=ChunkPos { x: 451, z: -457, dimension: End }
parsed.sample_chunk.records=10
parsed.sample_chunk.subchunks=5
parsed.sample_chunk.subchunk_storages=4
parsed.sample_chunk.palette_states=10
parsed.sample_chunk.block_entities=0
parsed.sample_chunk.biomes.records=1 storages=25
parsed.sample_chunk.errors=[]
players.count=290
```

## Benchmark Results

Latest local Criterion and large-fixture results are tracked in
[`docs/BENCHMARKS.md`](docs/BENCHMARKS.md). The one-shot large fixture harness
is intentionally separate from Criterion because multi-million-entry scans
should not be repeated inside microbenchmarks.

## Features And docs.rs

docs.rs builds with all features enabled, so the hosted API reference includes
async wrappers and the optional `bedrock-leveldb` backend.

| Feature | Default | Meaning |
| --- | --- | --- |
| `async` | yes | Adds async wrappers that delegate blocking filesystem, LevelDB, and NBT work to `tokio::task::spawn_blocking` |
| `backend-bedrock-leveldb` | yes | Enables opening native Bedrock LevelDB worlds through `bedrock-leveldb` |
| `leveldb-mmap` | no | Enables the backend and forwards the `bedrock-leveldb/mmap` feature |

Disable default features when a tool only needs pure parsing, in-memory storage,
`level.dat`, or NBT helpers. The crates.io package includes the English and
Chinese READMEs, guide documents under `docs/`, the changelog, licenses,
source, tests, fixture documentation, and benchmarks.

## Completeness

| Area | Status |
| --- | --- |
| `level.dat` header, warning, atomic write | Implemented |
| Bedrock little-endian NBT and consecutive roots | Implemented |
| DB key classification | Implemented for chunk, player, actorprefix, digp, map, village, local player aliases, and common global keys |
| Legacy `LegacyTerrain` records | Implemented for 83,200-byte LevelDB-era terrain values |
| Legacy subchunk block arrays | Implemented for v0 and v2-v7 pre-paletted `SubChunkPrefix` values |
| Subchunk v1/v8/v9 palette parsing | Implemented, counts-only and full-indices modes |
| Data2D/Data3D biome and heightmap codecs | Implemented |
| HSA, map, global, actor, and block-entity writes | Implemented with roundtrip validation |
| `digp -> actorprefix` actor resolution | Implemented, configurable, with transactional modern actor writes |
| Players, entities, block entities, item stacks | Implemented common field extraction |
| Unknown version-specific data | Preserved or counted according to retention mode |
| Full structured editing for every chunk version | Not implemented |
| Map pixel record parsing | Implemented |

Historical Bedrock worlds that predate LevelDB and store chunk data in
`chunks.dat` / `entities.dat` are not database worlds; importing those files is
outside this crate's current scope.

## Operational Guidance

- Do not call `parse_world_blocking(WorldParseOptions::full_raw())` from UI code.
  It is an offline debugging path.
- Use `read_level_dat` for launcher metadata and `write_level_dat_atomic` for
  safe `level.dat` edits.
- Use category APIs when only one class of data is required. This avoids parsing
  entities, chunks, and global records unnecessarily.
- For renderers, build a viewport `WorldChunkQueryRegion` first and use the
  render-index APIs. Keep full `list_chunk_positions_blocking` for metadata,
  search, and offline export workflows.
- After large write waves, call `compact_storage_blocking` explicitly if the
  tool wants LevelDB compaction before handing the world back to another
  process.
- The optional `bedrock-leveldb` backend uses a versioned dependency for
  crates.io publishing and a local `../bedrock-leveldb` path for repository
  development.
