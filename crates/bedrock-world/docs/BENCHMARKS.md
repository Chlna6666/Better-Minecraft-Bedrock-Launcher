# Benchmark Notes

This file records the latest local v0.2.0 benchmark run. Re-run the commands
below before comparing world-format changes because Criterion output depends on
the host CPU, fixture shape, filesystem, and background load.

## Commands

```powershell
rustc --version
cargo --version
cargo bench --all-features --bench world_parse -- --noplot
cargo bench --all-features --bench large_fixture
```

## Latest Results

Local run:

```text
date: 2026-05-07
host: Windows / PowerShell
rustc: 1.93.1 (01f6ddf75 2026-02-11)
cargo: 1.93.1 (083ac5135 2025-12-15)
features: --all-features
criterion: sample_size=10, measurement_time=4s
plotting: gnuplot not installed; Criterion used Plotters
fixture: tests/fixtures/sample-bedrock-world
```

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
