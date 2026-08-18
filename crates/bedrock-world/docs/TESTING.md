# Testing And Benchmarks

## Required Checks

```powershell
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo doc --no-deps
```

These checks are expected to pass on a fresh checkout without private fixture
data. Synthetic unit/integration tests are the baseline contract; private real
worlds are additional compatibility evidence, not a prerequisite for a normal
checkout.

## Historical Compatibility Corpus

Cross-version compatibility is exercised by `tests/historical_compat.rs`. The
expected matrix is:

```text
bedrock-0.6.1
bedrock-0.14
bedrock-0.16
bedrock-1.0
bedrock-1.12
bedrock-1.13
bedrock-1.16
bedrock-1.17
bedrock-1.18.0
bedrock-1.18.30
bedrock-1.19
bedrock-1.20
bedrock-1.21
bedrock-1.26
future-unknown
```

By default the test looks under `tests/fixtures` and reports unavailable private
worlds as skipped. This optional mode is useful for ordinary source checkouts.
For a compatibility gate, skipping is not success: set
`BEDROCK_WORLD_REQUIRE_HISTORICAL_FIXTURES=1`. Every matrix entry must then be
present and complete or the test fails.

The corpus root can be moved outside the repository with:

```powershell
$env:BEDROCK_WORLD_FIXTURE_ROOT = 'D:\bedrock-world-corpus'
$env:BEDROCK_WORLD_REQUIRE_HISTORICAL_FIXTURES = '1'
cargo test -p bedrock-world --all-features --test historical_compat -- --nocapture
```

Each historical world directory must contain `level.dat` plus either
`db/CURRENT` or pre-LevelDB `chunks.dat`. `BedrockWorld::open_auto_blocking`
must open each fixture, `versions_blocking` must agree with the detected storage
family, and the storage must expose persisted records. `future-unknown` must
retain at least one future/unknown record rather than normalising it into a
known representation.

Do not commit private real-world corpora. They may be large and may contain
player data. CI can use redistributable/synthetic fixtures, while a private
compatibility environment can enforce the complete corpus with the environment
variables above.

## Pre-LevelDB Pocket Contract

Pocket `chunks.dat` data is opened through `BedrockWorld::open_auto_blocking`;
there is no public `PocketChunksDatStorage` compatibility API.

Tests must distinguish the two real terrain lengths:

```text
82,176 bytes  pre-LevelDB Pocket terrain core, no persisted biome/RGB tail
83,200 bytes  LevelDB LegacyTerrain, includes the 1,024-byte biome/RGB tail
```

The library must never append default biome bytes to an 82,176-byte source just
to make it look like the later 83,200-byte representation. Reads may expose the
known terrain core, while conversions that require real biome samples must fail
before staging writes.

Pocket `entities.dat` and `chunks.dat` imports must complete full preflight
before a single atomic destination batch. A failure in a later source record
must not leave a partially imported LevelDB world.

## Optional Large Performance Fixture

The older one-world performance/integration fixture remains independent from the
historical compatibility matrix. Place a copied LevelDB world at:

```text
tests/fixtures/sample-bedrock-world
```

The folder should contain `level.dat` and `db/CURRENT`.
`tests/fixture_world.rs` and the large fixture benchmark detect it automatically.
If it is missing, those *performance-fixture* checks print a skip message and
return successfully. Do not interpret this optional skip as historical
compatibility coverage.

## Render Correctness

Render exact-batch tests should include shuffled, duplicated, and
priority-sorted `ChunkPos` inputs. Use asymmetric block/height/biome sentinel
values and assert that every returned `ChunkData.pos` still owns the matching
decoded records with `ChunkLoadStats::prefix_scans == 0`.

Surface correctness tests should prefer
`ChunkDataRequest::new().surface_columns(ExactSurfaceSubchunkPolicy::Full)`.
Create fixtures where raw Data2D/Data3D or legacy heightmap values disagree
with actual SubChunk blocks, then assert `column_samples` reports the real top
block, overlay/water context when present, and
`ChunkLoadStats::raw_height_mismatch_columns` is non-zero. Raw heightmap
behavior belongs in `ChunkDataRequest::new().height_map()` tests.

For old Pocket terrain, render tests must not infer a legacy biome sample when
the source has no biome/RGB tail. Missing data is different from biome id zero.

## Typed Write Contracts

Typed write tests should cover persistence semantics rather than presentation
refresh policy:

- `OpenOptions::default()` remains read-only and high-level writes return
  `BedrockWorldErrorKind::ReadOnly` before mutating storage.
- Writable worlds are opened with
  `OpenOptions { read_only: false, ..OpenOptions::default() }`.
- map/global/HSA/heightmap/biome/block-entity writes serialize, parse back, and
  read back with semantic equivalence.
- actor writes update `digp -> actorprefix` records atomically.
- mixed `Entity` and `digp`/`actorprefix` actor representations may be converted
  only when actor order, ids, and NBT agree exactly; they must not be merged.
- raw non-UTF-8 `player_*` keys round-trip byte-for-byte through
  `BedrockDbKey`/`PlayerKeyRecord` APIs.
- a partial player SavedItem conversion is never treated as a complete world
  downgrade.
- `Data2D -> Data3D` tests select the destination height generation explicitly;
  no test should depend on an implicit Caves & Cliffs height default.

Do not add presentation-layer invalidation assertions here. Post-write refresh
and scheduling behavior is a downstream responsibility.

## LevelDB Compression Contract

`bedrock-leveldb` must read and write Mojang Bedrock compression id `4` as raw
DEFLATE. `CompressionPolicy::RawDeflate` is the Bedrock default; zlib framing
(id `2`) remains an explicit compatibility policy.

The public integration test `bedrock-leveldb/tests/raw_deflate.rs` must cover:

```text
default Options
  -> WAL write
  -> flush native table
  -> close
  -> reopen read-only
  -> exact value readback
```

This is intentionally separate from decoder-only tests so a future writer
regression cannot leave compression id `4` asymmetric again.

## Benchmarks

Run the benchmark set with:

```powershell
cargo bench --all-features --bench world_parse -- --noplot
cargo bench --all-features --bench large_fixture
```

`benches/world_parse.rs` always runs a synthetic `level.dat` parse benchmark and
adds LevelDB/chunk/SubChunk benchmarks when the optional large performance
fixture exists.

`benches/large_fixture.rs` is a one-shot harness for multi-million-entry scans.
It prints elapsed time and throughput once instead of asking Criterion to repeat
the scan many times.

Latest local numbers are recorded in [`BENCHMARKS.md`](BENCHMARKS.md).
