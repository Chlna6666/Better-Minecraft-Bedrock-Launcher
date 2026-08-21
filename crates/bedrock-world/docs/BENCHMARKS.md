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
date: 2026-08-21
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
| `logical_cold` | 573.741 ms | 630.523 ms | 446.19 chunks/s | 317.286 ms | 217.857 ms |
| `logical_warm` | 536.753 ms | 551.229 ms | 476.94 chunks/s | 289.857 ms | 198.714 ms |

Additional one-shot evidence:

| Operation | Result |
| --- | --- |
| classify keys | 1,764,971 entries in 46,910 ms (37,624.13 entries/s) |
| visible key scan | 1,768,601 entries in 46,351 ms (38,156.16 entries/s) |
| player prefix scan | 1,767,364 entries in 42,822 ms (41,271.84 entries/s) |
| player list | 111 players in 43,303 ms (dynamic) / 46,544 ms (generic) |
| fixed layer query | 256 chunks in 271 ms; DB 265 ms, decode 5 ms |

## Latest synthetic Criterion result

Local run:

```text
date: 2026-08-21
host: Windows / PowerShell
rustc: 1.95.0 (59807616e 2026-04-14)
cargo: 1.95.0 (f2d3ce0bd 2026-03-21)
features: --all-features
criterion: sample_size=10, measurement_time=4s
plotting: gnuplot not installed; Criterion used Plotters
```

Run named bench targets instead of passing `--noplot` to the whole package; the
lib test harness does not accept Criterion's `--noplot` flag.

### Criterion

| Benchmark | Mean | Interval | Throughput |
| --- | ---: | --- | --- |
| `bedrock_world/level_dat/parse_synthetic` | 789.29 ns | 727.20..816.61 ns | 100.29 MiB/s |
| `bedrock_world/level_dat/nbt_events_synthetic` | 239.78 ns | 233.76..246.34 ns | 330.12 MiB/s |
| `bedrock_world/level_dat/nbt_root_owned_synthetic` | 593.74 ns | 575.30..633.54 ns | 133.32 MiB/s |
| `bedrock_world/level_dat/nbt_root_ref_synthetic` | 215.45 ns | 202.81..222.00 ns | 367.40 MiB/s |

### Performance Optimization Highlights vs Prior Baseline

- **Chunk Read/Decode Throughput**:
  - `logical_cold`: Increased by **+50.8%** (from 295.83 chunks/s to **446.19 chunks/s**). DB read average dropped from 503.4 ms to 317.3 ms (-37.0%), decode average dropped from 300.0 ms to 217.9 ms (-27.4%).
  - `logical_warm`: Increased by **+23.4%** (from 386.42 chunks/s to **476.94 chunks/s**). DB read average dropped from 347.3 ms to 289.9 ms (-16.5%), decode average dropped from 271.1 ms to 198.7 ms (-26.7%).
- **Fixed Layer Query**:
  - 256 chunks batch query improved from 721 ms down to **271 ms** (**2.66x faster**), with decode overhead reduced to only 5 ms.

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
