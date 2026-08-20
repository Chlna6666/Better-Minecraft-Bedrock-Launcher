# Benchmark Notes

This file defines the reproducible 0.7.0 benchmark contract. Re-run the
commands below before comparing world-format changes because the result depends
on the host CPU, fixture shape, filesystem, cache state, and background load.

## Commands

```powershell
rustc --version
cargo --version
cargo bench -p bedrock-world --all-features --bench world_parse -- --noplot
cargo bench -p bedrock-world --all-features --bench large_fixture -- "C:\path\to\read-only-world"
```

The large-fixture runner opens LevelDB read-only and hashes every file, relative
path, and length with XXH3-128 before and after the run. A traversal failure,
changed hash, or changed byte count fails the run.

`logical_cold` means a new database handle per sample with the crate cache
bypassed. `logical_warm` means one primed handle with the crate cache enabled.
Neither condition clears the Windows filesystem cache, so these labels must not
be described as physical cold-disk and warm-disk measurements.

## Latest 0.7.0 fixture result

Local run:

```text
date: 2026-08-20
host: Windows x86_64 / NTFS
cpu: AMD Ryzen 7 7840H, 8 cores / 16 logical processors
disk: Fanxiang S500PRO 1TB NVMe SSD
features: --all-features
fixture_hash: 342010d8883d5b5399120b1ecded04fc (XXH3-128)
fixture_bytes: 1087899207
compatibility_records: 1764971
visible_leveldb_records: 1768601
chunks: 111827
parse_errors: 0
corrupt_chunks: 617
subchunk_versions: v8=125, v9=108402
```

The compatibility record count covers records classified by the world-format
scanner. The larger LevelDB count is the lower-layer visible-key scan and is
reported separately rather than conflating the two definitions. Likewise,
`parse_errors=0` does not erase the 617 chunks rejected by compatibility
validation.

### Read/decode query

Seven samples queried exact surface blocks and surface biomes for 256 real
chunks on one worker. Disk and CPU columns are averages reported by
`ChunkLoadStats`; percentile and throughput use wall time.

| Cache condition | p50 | p95 | Throughput | DB read avg | Decode avg |
| --- | ---: | ---: | ---: | ---: | ---: |
| `logical_cold` | 865.359 ms | 871.119 ms | 295.83 chunks/s | 503.429 ms | 300.000 ms |
| `logical_warm` | 662.490 ms | 692.064 ms | 386.42 chunks/s | 347.286 ms | 271.143 ms |

Additional one-shot evidence:

| Operation | Result |
| --- | --- |
| classify keys | 1,767,465 entries in 36,303 ms (48,686.13 entries/s) |
| visible key scan | 1,768,601 entries in 32,691 ms (54,100.37 entries/s) |
| player prefix scan | 110 records in 16,411 ms |
| player list | 111 players in 358 ms |
| fixed layer query | 256 chunks in 721 ms; DB 712 ms, decode 7 ms |

## Historical synthetic Criterion result

The following result is retained for trend context only; it used the repository
sample fixture on 2026-05-07 and is not interchangeable with the 0.7.0 snapshot
run above.

Run named bench targets instead of passing `--noplot` to the whole package; the
lib test harness does not accept Criterion's `--noplot` flag.

### Criterion

| Benchmark | Mean | Interval |
| --- | ---: | --- |
| `bedrock_world/level_dat/parse_synthetic` | 376.59 ns | 366.19..382.99 ns |
| `bedrock_world/level_dat/nbt_events_synthetic` | 141.71 ns | 136.72..153.28 ns |
| `bedrock_world/level_dat/read_fixture` | 53.970 us | 53.476..54.367 us |
| `bedrock_world/db/open_lazy` | 572.86 us | 546.81..632.79 us |
| `bedrock_world/world/list_players` | 53.286 ms | 51.765..55.314 ms |
| `bedrock_world/subchunk/decode_palette_full_indices` | 39.750 us | 37.665..40.933 us |
| `bedrock_world/subchunk/decode_palette_counts_only` | 38.311 us | 36.614..39.778 us |
| `bedrock_world/chunk/parse_fixture_chunk` | 70.046 ms | 56.201..76.518 ms |

Criterion reported local improvement for lazy DB open, no material change for
synthetic `level.dat`, fixture `level.dat`, and counts-only palette decode, and
regressions for list-player scanning, full-index palette decode, and fixture
chunk parsing versus the prior machine baseline. The fixture-backed numbers are
sensitive to disk cache and background load; use them as trend inputs, not CI
thresholds.

### Large Fixture

```text
large_fixture.level_dat elapsed_ms=0 version=10 payload_len=2889
large_fixture.db.open_lazy elapsed_ms=0 mmap_enabled=true
large_fixture.classify_keys.single elapsed_ms=15336 entries=4571643 entries_per_sec=298091.04
large_fixture.key_scan.generic elapsed_ms=14180 entries=4571643 entries_per_sec=322379.39 worker_threads=1 tables_scanned=509
large_fixture.prefix_ref_scan.players elapsed_ms=6019 entries=290 entries_per_sec=48.18 worker_threads=1 prefix_scans=1
large_fixture.players.dynamic elapsed_ms=74 count=290
large_fixture.players.generic elapsed_ms=84 count=290
large_fixture.sample_chunk elapsed_ms=72 records=17 subchunks=9 block_entities=0 parse_errors=0
large_fixture.render_exact_batch.generic elapsed_ms=37 chunks=4 worker_threads=1 prefix_scans=0 exact_get_batches=1 keys_requested=112 keys_found=39
large_fixture.nbt_events.level_dat elapsed_ms=0 events=147 payload_len=2889
```

### Chunk Query Fast Path

Local run on 2026-07-13 against the same fixture, after removing unnecessary
`LegacyTerrain` reads from fixed-layer/cave queries and enabling storage-block
cache reuse by default:

| Query | Chunks | Keys | DB | Decode | Elapsed |
| --- | ---: | ---: | ---: | ---: | ---: |
| Exact surface, cache reuse warm | 256 | 7168 | 44 ms | 272 ms | 319 ms |
| Fixed layer, cache reuse | 256 | 256 | 4 ms | 4 ms | 8 ms |

The fixed-layer result is cache-sensitive. Use `StorageCachePolicy::Bypass` for
cold-read measurements; the equivalent cold pass was 41 ms with 36 ms in DB
reads. Exact surface is decode-bound and should be optimized in the packed
surface-column parser rather than by adding DB concurrency.
