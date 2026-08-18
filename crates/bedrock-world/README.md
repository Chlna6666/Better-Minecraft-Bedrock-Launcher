# bedrock-world

[English](README.md) | [简体中文](README.zh-CN.md)

`bedrock-world` is a multi-version Minecraft Bedrock world library built on top of
`bedrock-leveldb`. It owns Bedrock file, key, NBT, chunk, player, actor, biome,
structure, and compatibility semantics while the lower crate owns Mojang LevelDB
WAL/SST/MANIFEST mechanics.

The core rule is **preserve the representation that is actually persisted**.
Normal reads and writes do not silently upgrade, downgrade, normalize, or invent
historical fields. Generation-changing operations are explicit and preflighted.

## Quick start

For normal read-only inspection, let the world layer detect the persisted format:

```rust
use bedrock_world::{BedrockWorld, WorldScanOptions};

fn inspect() -> bedrock_world::Result<()> {
    let world = BedrockWorld::open_auto_blocking("path/to/minecraftWorld")?;
    println!("format={:?}", world.format());

    let versions = world.versions_blocking()?;
    println!("mixed={}", versions.has_mixed_version_storage());
    println!("future={}", versions.has_future_storage());

    let chunks = world.list_chunk_positions_blocking(WorldScanOptions::default())?;
    println!("chunks={}", chunks.len());
    Ok(())
}
```

With the `async` feature, `BedrockWorld::open_auto` and async wrappers are
available. Use `BedrockWorld::open_blocking(path, OpenOptions)` when an explicit
format hint or writable LevelDB world is required.

## Supported persisted generations

World opening distinguishes the physical storage that is present:

- current Mojang LevelDB worlds;
- early LevelDB worlds containing `LegacyTerrain`;
- pre-LevelDB Pocket Edition worlds containing `chunks.dat`;
- Pocket `entities.dat` alongside `chunks.dat`;
- mixed/partially upgraded LevelDB worlds containing records from more than one
  generation;
- unknown/future chunk tags, database keys, and SubChunk versions, which are
  preserved as unknown evidence rather than reinterpreted.

`WorldVersions` records literal persisted evidence. Do not infer the entire world
format from only `level.dat.lastOpenedWithVersion`.

## Pocket `chunks.dat` is not later `LegacyTerrain`

Old Pocket terrain has an 82,176-byte core:

- block ids;
- block metadata;
- sky light;
- block light;
- 16x16 height map.

Later LevelDB `LegacyTerrain` has an additional 1,024-byte biome/RGB tail, making
an 83,200-byte record. `bedrock-world` keeps these two representations distinct.
An 82,176-byte source remains 82,176 bytes and does not receive fabricated biome
ids or colors.

`LegacyTerrain::has_biome_samples()` exposes this distinction. Editing blocks,
metadata, light, or height remains possible on the Pocket core; editing a biome
sample is rejected when the persisted tail does not exist.

Use `check_pocket_chunks_dat_leveldb_import_blocking` before an exact later-LevelDB
copy. `import_pocket_chunks_dat_records_blocking` refuses lossy conversion before
mutating the target when required persisted data is absent.

## Pocket `entities.dat`

Historical Pocket `entities.dat` is parsed as its real file format rather than
being treated as LevelDB actor storage. The parser preserves the file header,
unknown root fields, trailing bytes, entities, and tile entities.

Explicit import maps entities to old inline chunk `Entity` records and tile
entities to `BlockEntity`. It does not jump directly to modern
`digp`/`actorprefix`. Position and collision checks run before the target batch is
committed.

## LevelDB semantics

With `backend-bedrock-leveldb`, `BedrockLevelDbStorage` is the concrete public
backend. Public storage abstractions also include `WorldStorage`,
`PartitionedWorldStorage`, `MemoryStorage`, scan controls/results, and
`StorageBatch`.

Mojang/Bedrock native table writing uses raw DEFLATE compression id `0x04` by
default. Standard zlib id `0x02`, Snappy, and uncompressed blocks remain readable
and explicitly selectable at the `bedrock-leveldb` layer.

The synthetic public `PocketChunksDatStorage` abstraction has been removed.
Pocket opening belongs to the world layer because the pre-LevelDB source is not
byte-equivalent to later LevelDB `LegacyTerrain`.

## Players

Player data can be stored in:

- `level.dat.Player`;
- `~local_player`;
- `player_<id>` LevelDB records.

`PlayerId` is the textual convenience API. Arbitrary non-UTF-8 `player_*` keys
are retained byte-for-byte through the raw player-key APIs rather than passed
through lossy UTF-8 conversion.

Historical `level.dat.Player` handling is explicit. The library does not treat an
entire `level.dat` root as player NBT merely because a caller selected the legacy
player id.

Historical saved-item writes use concrete target families and exact mapping
checks. Missing or ambiguous historical item/BlockState mappings are rejected
before mutation.

## Chunks, SubChunks, biomes, and rendering

Interactive tools should request only the data representation they need:

- `list_render_chunk_positions_blocking`;
- `list_chunk_positions_in_region_blocking`;
- `query_chunk_data_blocking`;
- `query_chunk_data_many_blocking`;
- `query_chunk_region_blocking`;
- `parse_chunk_blocking` for complete structured inspection.

`ChunkDataRequest` composes surface columns, fixed layers, cave slices, full 3D
indices, height maps, biome data, and block entities. Render paths use exact
batch reads and can avoid full-world scans or unnecessary 4096-index
materialization.

Legacy terrain and SubChunk records may coexist in transition worlds. Both are
retained; renderers prefer actual SubChunk block data and use legacy terrain only
where it is genuinely the available/fallback representation.

Unknown SubChunk version bytes remain explicit compatibility evidence. They are
not silently parsed as a known version.

## Actors and BlockEntities

Legacy inline chunk `Entity` payloads and modern `digp -> actorprefix` storage are
separate generations. Reads support both. Ordinary writes do not silently move
one representation into the other.

Compatibility/integrity scans report structural actor problems including:

- dangling `digp` references;
- orphan `actorprefix` records;
- actors owned by multiple chunk digests;
- malformed actor digest payloads.

Structural failures can mark the owning chunk corrupt instead of merely logging
a parse warning. Modern actor writes update digest and actor records together.

BlockEntity payloads use consecutive little-endian NBT roots. Concrete rewrites
validate chunk coordinates and preserve unrelated data.

## `level.dat`, NBT, maps, globals, and structures

For launcher metadata, prefer the file-level `level.dat` API; it avoids opening
the world database. `LevelDatDocument` retains header metadata and read warnings.
Bedrock little-endian NBT supports owned parsing, borrowed/event traversal, and
consecutive roots.

Typed helpers cover maps, villages, common global records, hardcoded spawn areas,
biomes, actors, block entities, player records, chunk records, and
`.mcstructure` import/export/placement. Multi-record edits preflight their target
data before commit.

## Writing

`OpenOptions::default()` is read-only. Open a LevelDB world explicitly writable
for edits:

```rust
let world = bedrock_world::BedrockWorld::open_blocking(
    "path/to/minecraftWorld",
    bedrock_world::OpenOptions {
        read_only: false,
        ..Default::default()
    },
)?;
```

Pre-LevelDB Pocket world handles remain read-only. Converting a Pocket world to
another generation is a separate explicit import/conversion operation.

High-level writes validate their encoded representation before committing.
Unknown/future records are preserved unless the caller explicitly performs a
destructive operation with a proven target format.

## Historical compatibility corpus

Synthetic unit tests are not treated as proof of compatibility with historical
worlds. A real/sanitized world corpus can be mounted with:

```text
BEDROCK_WORLD_FIXTURE_ROOT=/path/to/world-corpus
BEDROCK_WORLD_REQUIRE_HISTORICAL_FIXTURES=1
```

The current world matrix covers named fixtures from `bedrock-0.6.1` through
`bedrock-1.26` plus `future-unknown`. When the `REQUIRE` flag is enabled, missing
or incomplete fixtures fail the suite.

Raw Mojang LevelDB historical fixtures use the corresponding
`BEDROCK_LEVELDB_FIXTURE_ROOT` and
`BEDROCK_LEVELDB_REQUIRE_HISTORICAL_FIXTURES` variables.

`tests/fixtures/sample-bedrock-world` remains an optional large local performance
fixture. A skipped private performance fixture is not a historical compatibility
pass.

See [`docs/TESTING.md`](docs/TESTING.md) for the exact test contract.

## Completeness model

| Area | Current behavior |
| --- | --- |
| `level.dat` header + little-endian NBT | Implemented with unknown-field preservation |
| Mojang LevelDB raw key/value access | Implemented through `bedrock-leveldb` |
| Native Bedrock raw-DEFLATE table writes | Implemented as compression id `0x04` |
| Chunk key classification | Known generations classified; unknown keys/tags retained |
| Pocket 82,176-byte terrain | Implemented without fabricated biome tail |
| LevelDB 83,200-byte `LegacyTerrain` | Implemented with biome/RGB samples |
| Legacy and paletted SubChunks | Implemented for known persisted versions; unknown versions retained |
| Data2D/Data3D biome + height data | Implemented |
| Player record families | Implemented; arbitrary raw `player_*` keys preserved separately |
| Legacy inline and modern actor storage | Implemented with integrity diagnostics |
| Map/global/HSA/block-entity/actor writes | Typed writes with validation |
| Explicit historical conversion | Only where source and target representations are proven |
| Unknown/future formats | Preserve/report; do not guess |

“Readable” does not mean “losslessly writable to every historical target”. The
compatibility report and explicit preflight APIs are authoritative for that
distinction.

## Performance model

- use file-level `level.dat` access when no database data is needed;
- use exact `get_many` render reads instead of scanning unrelated records;
- use key-only scans for classification and bounds discovery;
- use bounded table-parallel scans for large offline operations;
- avoid nested worker pools when a caller already owns render workers;
- preserve raw bytes and borrow data when a structured owned form is not needed.

Benchmarks and large-fixture notes live in [`docs/BENCHMARKS.md`](docs/BENCHMARKS.md).

## Error handling

Public fallible APIs return `bedrock_world::Result<T>`. Match
`BedrockWorldError::kind()` rather than parsing display text. Important classes
include read-only, validation, unsupported-format, corrupt-world, cancellation,
and LevelDB failures.

More detail is available in [`docs/API.md`](docs/API.md),
[`docs/TESTING.md`](docs/TESTING.md), and [`ARCHITECTURE.md`](ARCHITECTURE.md).
