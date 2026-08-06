# API Guide

`bedrock-world` exposes two layers:

- File-level helpers for `level.dat` and Bedrock little-endian NBT.
- A lazy `BedrockWorld` handle backed by a `WorldStorage` implementation.

## Fast Metadata Path

Use `read_level_dat` when a launcher or management tool only needs world
metadata. This path does not open LevelDB.

```rust
let document = bedrock_world::read_level_dat("world/level.dat".as_ref())?;
println!("level.dat version={}", document.version());
```

Use `write_level_dat_atomic` for `level.dat` edits. It validates the serialized
bytes by parsing them back before replacing the file.

## Lazy World Path

`BedrockWorld::open(path, OpenOptions::default())` opens a world in read-only
mode and auto-detects `db/CURRENT` LevelDB worlds or old Pocket Edition
`chunks.dat` worlds. Open with `OpenOptions { read_only: false, ..Default::default() }`
only for explicit edit flows; high-level writes call `ensure_writable` and
return `BedrockWorldErrorKind::ReadOnly` on read-only handles before any storage
mutation. `BedrockWorld::open_blocking` exposes the same detection for CLI tools
and examples. Use targeted APIs instead of full-world parsing for UI flows:

- `list_players_blocking`
- `classify_keys_blocking`
- `list_chunk_positions_blocking`
- `parse_chunk_blocking`
- `scan_entities_blocking`
- `scan_block_entities_blocking`
- `scan_items_blocking`

Async methods are wrappers over the blocking implementation and use
`tokio::task::spawn_blocking`.

`OpenOptions::format` can force `WorldFormatHint::LevelDb` or
`WorldFormatHint::PocketChunksDat`; the default `Auto` reports the result through
`BedrockWorld::format()`.

```rust
let world = bedrock_world::BedrockWorld::open_blocking(
    "world",
    bedrock_world::OpenOptions::default(),
)?;
assert!(matches!(
    world.format(),
    bedrock_world::WorldFormat::LevelDb
        | bedrock_world::WorldFormat::LevelDbLegacyTerrain
        | bedrock_world::WorldFormat::PocketChunksDat
));
```

## Render Index Path

Interactive renderers should not wait for `list_chunk_positions_blocking` before
painting the first viewport. Use the render-index APIs instead:

- `list_render_chunk_positions_blocking` lists chunks that have render records.
- `list_chunk_positions_in_region_blocking` probes only a viewport or
  export region using key-only prefix scans.
- `query_chunk_data_blocking`, `query_chunk_data_many_blocking`, and
  `query_chunk_region_blocking` accept `ChunkLoadOptions` /
  `WorldChunkQueryRegionLoadOptions` with `threading`, `pipeline`, `cancel`,
  `progress`, and `priority` policies.

Async wrappers with the same names are available behind the default `async`
feature. They use `spawn_blocking` and preserve cancellation/progress options.
The blocking implementation uses bounded local parallelism, not Rayon global
pool state.

```rust
let region = bedrock_world::WorldChunkQueryRegion {
    dimension,
    min_chunk_x: -32,
    min_chunk_z: -32,
    max_chunk_x: 31,
    max_chunk_z: 31,
};
let positions = world.list_render_chunk_positions_in_region(
    region,
    bedrock_world::WorldScanOptions {
        pipeline: bedrock_world::WorldPipelineOptions {
            queue_depth: 64,
            ..Default::default()
        },
        ..Default::default()
    },
).await?;
let chunks = world.load_render_chunks(
    positions,
    bedrock_world::ChunkLoadOptions {
        priority: bedrock_world::ChunkLoadPriority::DistanceFrom {
            chunk_x: 0,
            chunk_z: 0,
        },
        ..Default::default()
    },
).await?;
```

`query_chunk_region_blocking` returns `WorldChunkQueryRegionData { region, chunks, stats }`.
Use `stats.worker_threads`, `stats.queue_wait_ms`, and
`stats.subchunks_decoded` to tune worker budgets without baking fixed time
thresholds into tests.

`ChunkLoadOptions::data_request` accepts a composable `ChunkDataRequest`.
Add only the payload that a consumer needs: `surface_columns(policy)`,
`layer(y)`, `cave_slice(y)`, `full_3d_indices()`, `height_map()`,
`biome(requirement)`, and `block_entities()`. The loader unions subchunk keys
for all requested representations, uses packed palette indices for a
surface-only request, and upgrades to full 3D indices only when a layer, cave,
or full-3D requirement needs random access. `ChunkDataRequest` remains a
legacy mutually-exclusive compatibility contract.

```rust
let request = bedrock_world::ChunkDataRequest::new()
    .surface_columns(bedrock_world::ExactSurfaceSubchunkPolicy::HintThenVerify)
    .height_map()
    .biome(bedrock_world::BiomeDataRequirement::SurfaceColumns);
let options = bedrock_world::ChunkLoadOptions::for_data_request(request);
```

Surface samples in `ChunkData::column_samples` are derived from actual
blocks, including relief support, thin overlays, water context, biome, and
source, rather than raw heightmap records.

Raw heightmaps remain available through `ChunkData::height_map`, but map
renderers should treat them as hints or diagnostics. Stats expose
`computed_surface_columns`, `raw_height_mismatch_columns`,
`missing_subchunk_columns`, `legacy_fallback_columns`,
`legacy_biome_preferred_columns`, and `modern_biome_fallback_columns`.

For old LevelDB worlds, render exact-batch loading always requests
`ChunkRecordTag::LegacyTerrain` (`0x30`). If that record exists,
`ChunkData::legacy_terrain` is populated, the chunk is considered loaded
even without `Data2D`/`SubChunkPrefix`, and `ChunkLoadStats::prefix_scans`
remains `0`.
Legacy biome samples are exposed through `ChunkData::legacy_biomes` as
`[biome_id, red, green, blue]`; `legacy_biome_colors` is retained only as a
compatibility `0x00RRGGBB` view. When both `LegacyTerrain` biome samples and
old Data2D/Data3D biome ids exist, exact surface sampling prefers the saved
legacy RGB sample and treats the numeric biome id as fallback only.

Exact render chunk batches preserve the input key/value association after
deduplication, priority sorting, and parallel decode. Regression fixtures should
shuffle and duplicate `ChunkPos` values, then assert each returned
`ChunkData.pos` still carries the matching block, height, and biome
sentinels.

`PocketChunksDatStorage` is a read-only `WorldStorage` backend. It maps each
old 82,176-byte `chunks.dat` terrain payload to an 83,200-byte virtual
`LegacyTerrain` record by appending default legacy biome samples. `put`,
`delete`, and `write_batch` return `UnsupportedChunkFormat`.

## Typed Bedrock Records

v0.2 adds typed APIs for BedrockLevelFormat records that map editors usually
need without forcing a full-world parse. The storage boundary remains raw
key/value: `bedrock-world` owns Bedrock key classification, NBT codecs,
coordinate validation, and write roundtrip checks. This crate does not decide
how callers refresh or invalidate their presentation state after a write;
downstream applications and adapter crates map these semantic writes to their
own update model.

Key helpers:

- `MapRecordId` validates and encodes `map_<id>` keys.
- `GlobalRecordKind` classifies `mobevents`, `Overworld`, `Nether`, `TheEnd`,
  `scoreboard`, `LocalPlayer`, `AutonomousEntities`, and preserved unknown
  global names.
- `ActorUid` and `ActorDigestKey` encode `actorprefix<uid>` and
  `digp<x><z>[dimension]`.

Map and global records:

- `read_map_record_blocking`, `scan_map_records_blocking`,
  `write_map_record_blocking`, and `delete_map_record_blocking`.
- `read_global_record_blocking`, `scan_global_records_blocking`,
  `write_global_record_blocking`, and `delete_global_record_blocking`.
- Async wrappers with the same names are available behind the default `async`
  feature.

`ParsedMapData` now carries the validated `record_id`, parsed NBT `roots`,
`known_fields`, optional `MapPixels`, and raw bytes for preservation. The core
crate exposes map pixel buffers only; PNG/image export belongs in an adapter or
feature crate.

Chunk payload helpers:

- `get_heightmap_blocking`, `put_heightmap_blocking`, and
  `put_biome_storage_blocking` cover Data2D/Data3D heightmap and biome storage.
- `scan_hsa_records_blocking`, `put_hsa_for_chunk_blocking`, and
  `delete_hsa_for_chunk_blocking` cover Hardcoded Spawn Areas.
- `block_entities_in_chunk_blocking`, `put_block_entities_blocking`,
  `edit_block_entity_at_blocking`, and `delete_block_entity_at_blocking` cover
  consecutive block-entity NBT compounds.
- `actors_in_chunk_blocking`, `put_actor_blocking`, `delete_actor_blocking`,
  and `move_actor_blocking` cover legacy inline `Entity` reads and modern
  `digp -> actorprefix` writes.
- `delete_chunk_positions_blocking` removes a deduplicated set of chunks and
  their modern actor records in one atomic storage batch. The lower-level
  `StorageBatch` API also exposes `delete_chunk`, `put_block_entities`, and
  `put_hsa_for_chunk` for composing a replacement commit.

All high-level writes validate the serialized value by parsing it back before
committing. Actor writes update `actorprefix` and `digp` in one transaction.
Block-entity writes reject coordinates outside the target chunk. `chunks.dat`
backends stay read-only. Chunk deletion also removes modern actor digest and
`actorprefix` records owned by the deleted chunks. Write examples should
therefore open a writable world:

```rust
let world = bedrock_world::BedrockWorld::open_blocking(
    "world",
    bedrock_world::OpenOptions {
        read_only: false,
        ..bedrock_world::OpenOptions::default()
    },
)?;
```

LevelDB backend writes use synced WAL-backed write options. Use
`BedrockWorld::compact_storage_blocking` after a large write wave when an editor
needs to force backend compaction before another process opens the world.

## Structure Files

`McStructureFile` reads and writes Bedrock `.mcstructure` files, which are
uncompressed little-endian NBT files containing a structure size, block index
arrays, a palette, and block entities. The helper can also export a world region
and place a structure into writable world storage:

```rust
let structure = bedrock_world::McStructureFile::read_from_path("house.mcstructure".as_ref())?;
let result = structure.write_to_world_blocking(
    &world,
    bedrock_world::McStructurePlacement {
        source_anchor: bedrock_world::ChunkPos { x: 0, z: 0, dimension },
        target_anchor: bedrock_world::ChunkPos { x: 8, z: -4, dimension },
        origin_y: 64,
        rotation: bedrock_world::McStructureRotation::Clockwise90,
        mirror_x: false,
        mirror_z: false,
    },
    Some(Box::new(|progress| eprintln!("{progress:?}"))),
)?;
println!("placed chunks={}", result.touched_chunks.len());
```

Placement recomputes the heightmap columns touched by the new blocks and
commits world changes in batches of 16 chunks. `WriteChunks` progress events
are emitted after each batch is committed, so callers can safely refresh data
after the reported count.

Placement updates subchunk records, preserves supported block entities, and
applies horizontal rotation or mirroring to common direction-like block states.
Entity placement is intentionally left to downstream tools because entity NBT
semantics are not block-grid local.

## Parsing Modes

`WorldParseOptions::summary()` is the default for large scans. It keeps counters
and summaries while avoiding raw value retention.

`WorldParseOptions::structured()` keeps structured parsed entries without raw
values.

`WorldParseOptions::full_raw()` keeps raw values and full subchunk indices. Use
it for offline debugging, not interactive UI.

## Error Handling

All public fallible APIs return `bedrock_world::Result<T>`.

Match `BedrockWorldError::kind()` for stable categories:

```rust
match error.kind() {
    bedrock_world::BedrockWorldErrorKind::ReadOnly => {
        // Ask the caller to reopen with OpenOptions { read_only: false }.
    }
    bedrock_world::BedrockWorldErrorKind::Cancelled => {
        // A scan observed the caller's cancellation flag.
    }
    _ => eprintln!("{error}"),
}
```

Avoid parsing display strings; they are meant for humans.
