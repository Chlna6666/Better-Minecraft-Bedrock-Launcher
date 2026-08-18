# API Guide

`bedrock-world` is a multi-version Minecraft Bedrock world library. The storage engine boundary is raw
key/value data; Minecraft record semantics live in this crate, while Mojang LevelDB table/WAL mechanics
live in `bedrock-leveldb`.

The main compatibility rule is **preserve the representation that is actually present**. Ordinary reads
and writes do not silently upgrade, downgrade, normalise, or invent fields. Operations that change a
storage generation or target a historical game version are explicit and preflighted.

## Opening worlds

For normal read-only tooling, use:

```rust
let world = bedrock_world::BedrockWorld::open_auto_blocking("world")?;
println!("format={:?}", world.format());
```

`open_auto_blocking` detects:

- modern/current Mojang LevelDB worlds;
- old LevelDB worlds that still contain `LegacyTerrain`;
- pre-LevelDB Pocket Edition worlds with `chunks.dat`;
- Pocket `entities.dat` alongside `chunks.dat`, exposed through the same read-only world handle as
  legacy chunk `Entity` and `BlockEntity` records.

The async `BedrockWorld::open_auto` wrapper is available with the `async` feature.

`BedrockWorld::open_blocking(path, OpenOptions)` remains available when a caller needs an explicit
format hint or writable LevelDB handle. A pre-LevelDB Pocket world is always read-only at the raw world
backend; converting it to another storage generation is a separate operation.

## Real version evidence

Do not infer a whole world's format from only `level.dat.lastOpenedWithVersion`. Worlds may be partially
upgraded and can contain multiple record generations at once.

```rust
let versions = world.versions_blocking()?;
println!("level={:?}", versions.level);
println!("format={:?}", versions.world_format);
println!("mixed={}", versions.has_mixed_version_storage());
println!("future={}", versions.has_future_storage());
```

`WorldVersions` records literal persisted evidence including:

- `Version`, `VersionOld`, and `LegacyVersion` chunk bytes;
- SubChunk version bytes;
- `LegacyTerrain`, `BlockExtraData`, `Data2DLegacy`, `Data2D`, and `Data3D` generations;
- legacy inline `Entity` and modern `digp`/`actorprefix` actor storage;
- `BlockEntity` and `ActorDigestVersion` records;
- unknown chunk tags, unknown database keys, and unknown SubChunk versions.

Unknown/future records are evidence to preserve, not permission to reinterpret them.

## Pocket Edition `chunks.dat`

Old Pocket terrain has a confirmed 82,176-byte core containing block IDs, metadata, sky light, block
light, and a 16x16 height map. Later LevelDB `LegacyTerrain` adds a 1,024-byte tail containing 256
`[biome_id, red, green, blue]` samples.

The library keeps these forms distinct:

```rust
let terrain: bedrock_world::chunk::legacy::LegacyTerrain = /* parsed terrain */;
println!("bytes={}", terrain.raw().len());
println!("has biome samples={}", terrain.has_biome_samples());
```

A real 82,176-byte Pocket source remains 82,176 bytes. `LegacyTerrain::biomes()` returns an empty slice
for that form and `biome_sample_at` returns `None`. No default biome id or RGB colour is appended.

`LegacyTerrainBuilder::from_terrain` preserves the source size. Block, metadata, light, and height edits
remain valid on the Pocket core. `set_biome_sample` returns an error when the source has no biome tail;
it does not extend the record implicitly.

Use `check_pocket_chunks_dat_leveldb_import_blocking` before attempting an exact later-LevelDB copy.
`import_pocket_chunks_dat_records_blocking` refuses the operation before target mutation if any source
record lacks the persisted biome/RGB tail. This is intentional: a game-compatible later record cannot be
constructed losslessly from bytes that never existed in the old source.

## Pocket Edition `entities.dat`

Confirmed MCPE `entities.dat` is handled as its real file type:

- four-byte `ENT\0` magic;
- little-endian file version;
- little-endian NBT byte length;
- one little-endian NBT root with `Entities` and `TileEntities` lists.

```rust
let mut document = bedrock_world::read_pocket_entities_dat("world")?;
let entities = document.entities()?;
let tile_entities = document.tile_entities()?;
```

Unmodified documents return the exact original bytes. Edited documents preserve unknown root fields and
source trailing bytes. `write_pocket_entities_dat_atomic` uses the historical sidecar replacement style.

`import_pocket_entities_dat_records_blocking` is explicit. It maps entities to old chunk `Entity`
records and tile entities to `BlockEntity`; it does **not** jump directly to `digp`/`actorprefix`.
Positions and collisions are completely preflighted and the target receives one atomic `StorageBatch`.
Unpositioned records are rejected unless `skip_unpositioned` is explicitly enabled.

## `level.dat` and NBT

Use the file-level API when a launcher or management tool only needs metadata:

```rust
let document = bedrock_world::read_level_dat_document("world/level.dat".as_ref())?;
println!("header version={}", document.version());
```

`LevelDatDocument` retains header information and non-fatal read warnings. NBT is little-endian Bedrock
NBT and supports owned, borrowed/event, and consecutive-root paths.

Normal write helpers validate serialized data before replacing the target file. Existing world seed and
unknown fields remain authoritative unless the caller explicitly edits them.

## Player data

Player data can live in several physical locations depending on the game/storage generation:

- historical `level.dat.Player`;
- `~local_player`;
- `player_<id>` records.

Normal player writes return to the same record family and do not move or rewrite the player to another
game version implicitly.

Historical saved-item conversion is exposed through concrete target families rather than one generic
"legacy" writer. Exact mapping checks run before mutation and reject missing/ambiguous item or BlockState
mappings.

For the confirmed MCPE 0.6.1 target, `write_mcpe_0_6_1_level_dat_player` requires the actual old field
shape, including numeric inventory items, old Armor list, exact NBT scalar widths, header version 3, and
`StorageVersion=3`. It does not manufacture missing historical fields.

## Chunk and SubChunk access

Use targeted APIs for interactive tools:

- `list_render_chunk_positions_blocking`;
- `list_chunk_positions_in_region_blocking`;
- `query_chunk_data_blocking`;
- `query_chunk_data_many_blocking`;
- `query_chunk_region_blocking`;
- `parse_chunk_blocking` for a complete structured chunk inspection.

`ChunkDataRequest` composes the representation a consumer actually needs: surface columns, a fixed
layer, cave slice, full 3D indices, height map, biome data, and block entities. Avoid full-world/raw
materialisation in render loops.

`SubChunkVersion` retains unknown future version bytes. Unsupported payloads must remain raw/preserved
unless the caller chooses a destructive operation whose target format is fully proven.

## Entities and BlockEntities

Legacy inline chunk entities and modern actors are separate storage generations. Reads support both;
ordinary writes do not silently move one to the other.

Modern actor writes update `actorprefix` and `digp` consistently. BlockEntity chunk payloads are
consecutive NBT roots. Unknown BlockEntity roots remain byte-for-byte unchanged when a concrete rewrite
only modifies a recognised sibling root.

Concrete block-entity rewrites use `BlockEntityRewriter` and `rewrite_block_entity_chunk_blocking` rather
than a generic world migration manager.

## Maps, villages, global records, and structures

Typed helpers exist for map records, village/global keys, hardcoded spawn areas, `.mcstructure`, player
records, block entities, actors, biomes, and chunk records. Raw storage remains available through
`WorldStorage` for tools that need exact key/value access.

Structure placement and other multi-record edits preflight their target data before committing. Use a
writable LevelDB world only for explicit edits:

```rust
let world = bedrock_world::BedrockWorld::open_blocking(
    "world",
    bedrock_world::OpenOptions {
        read_only: false,
        ..Default::default()
    },
)?;
```

Pre-LevelDB Pocket world handles remain read-only.

## Storage backends

Public storage abstractions include `WorldStorage`, `PartitionedWorldStorage`, `MemoryStorage`, scan
options/results, and `StorageBatch`. With the `backend-bedrock-leveldb` feature enabled,
`BedrockLevelDbStorage` provides Mojang LevelDB access.

The old synthetic `PocketChunksDatStorage` public backend has been removed. Pocket world opening belongs
to the world layer because the source representation is not equivalent to later LevelDB `LegacyTerrain`.

When `backend-bedrock-leveldb` is disabled, LevelDB open paths fail with an explicit unsupported-feature
error instead of disappearing from internal compilation.

## Compatibility fixture corpus

Historical compatibility tests are intentionally separate from ordinary synthetic unit tests. Local or
sanitised real-world corpora can be mounted with:

```text
BEDROCK_WORLD_FIXTURE_ROOT=/path/to/world-corpus
BEDROCK_WORLD_REQUIRE_HISTORICAL_FIXTURES=1
```

and raw Mojang LevelDB corpora with:

```text
BEDROCK_LEVELDB_FIXTURE_ROOT=/path/to/leveldb-corpus
BEDROCK_LEVELDB_REQUIRE_HISTORICAL_FIXTURES=1
```

When the `REQUIRE` flag is enabled, missing fixtures fail the suite. A skipped private fixture is not a
compatibility pass.

## Error handling

All public fallible APIs return `bedrock_world::Result<T>`. Match `BedrockWorldError::kind()` for stable
categories rather than parsing display text. Important categories include `ReadOnly`, `Validation`,
`UnsupportedChunkFormat`, `CorruptWorld`, `Cancelled`, and `LevelDb`.
