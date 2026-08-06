# Testing And Benchmarks

## Required Checks

```powershell
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo doc --no-deps
```

These checks are expected to pass on a fresh checkout without private fixture
data.

## Optional Large Fixture

For end-to-end validation against a real Bedrock world, place a copied world at:

```text
tests/fixtures/sample-bedrock-world
```

The folder should contain `level.dat` and a `db/CURRENT` file. The integration
test and large fixture benchmark will detect it automatically. If the folder is
missing, those checks print a skip message and return successfully.

Do not commit this folder. Real worlds are large and may contain player data.

Legacy compatibility must use synthetic fixtures in CI. Cover both a memory
storage world containing only `LegacyTerrain` and a small generated
`chunks.dat` location table. Manual validation with a real 0.16 world is useful,
but the world itself must stay outside the repository.

Render exact-batch tests should include shuffled, duplicated, and
priority-sorted `ChunkPos` inputs. Use asymmetric block/height/biome sentinel
values and assert that every returned `ChunkData.pos` still owns the
matching decoded records with `ChunkLoadStats::prefix_scans == 0`.

Surface correctness tests should prefer `ChunkDataRequest::new().surface_columns(ExactSurfaceSubchunkPolicy::Full)`.
Create fixtures where raw Data2D/Data3D or legacy heightmap values disagree
with actual subchunk blocks, then assert `column_samples` reports the real top
block, overlay/water context when present, and
`ChunkLoadStats::raw_height_mismatch_columns` is non-zero. Raw heightmap
behavior belongs in `ChunkDataRequest::new().height_map()` tests.

Typed write tests should cover the storage-layer contract only:

- `OpenOptions::default()` remains read-only and every high-level write returns
  `BedrockWorldErrorKind::ReadOnly` before mutating storage.
- Writable worlds are opened with
  `OpenOptions { read_only: false, ..OpenOptions::default() }`.
- map/global/HSA/heightmap/biome/block-entity writes serialize, parse back, and
  read back with semantic equivalence.
- actor writes update `digp -> actorprefix` records in one transaction.
- `PocketChunksDatStorage` rejects `put`, `delete`, and batch writes.

Do not add presentation-layer invalidation assertions here. Post-write refresh
and scheduling behavior is a downstream responsibility.

## Benchmarks

Run the v0.2 benchmark set with:

```powershell
cargo bench --all-features --bench world_parse -- --noplot
cargo bench --all-features --bench large_fixture
```

`benches/world_parse.rs` always runs a synthetic `level.dat` parse benchmark and
adds LevelDB/chunk/subchunk benchmarks when the optional large fixture exists.

`benches/large_fixture.rs` is a one-shot harness for multi-million-entry scans.
It prints elapsed time and throughput once instead of asking Criterion to repeat
the scan many times.

The local fixture used for the current baseline is:

```text
C:\Users\Administrator\Desktop\BE-Community-Dev\bedrock-world\tests\fixtures\sample-bedrock-world
```

Latest local numbers are recorded in [`BENCHMARKS.md`](BENCHMARKS.md).
