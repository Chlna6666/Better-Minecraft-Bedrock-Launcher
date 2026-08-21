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
date: 2026-08-22
host: Windows x86_64 / NTFS
cpu: AMD Ryzen 7 7840H, 8 cores / 16 logical processors
disk: Fanxiang S500PRO 1TB NVMe SSD
features: --all-features
fixture_hash: 342010d8883d5b5399120b1ecded04fc (XXH3-128)
fixture_bytes: 1087899207
compatibility_records: 1764971
visible_leveldb_records: 1764971
chunks: 111827
parse_errors: 0
corrupt_chunks: 617
subchunk_versions: v8=125, v9=108402
```

The compatibility record count covers records classified by the world-format
scanner. Likewise, `parse_errors=0` does not erase the 617 chunks rejected by compatibility
validation.

### Read/decode query

Seven samples queried exact surface blocks and surface biomes for 256 real
chunks on one worker. Disk and CPU columns are averages reported by
`ChunkLoadStats`; percentile and throughput use wall time.

| Cache condition | p50 | p95 | Throughput | DB read avg | Decode avg |
| --- | ---: | ---: | ---: | ---: | ---: |
| `logical_cold` | 362.791 ms | 412.139 ms | 705.64 chunks/s | 156.286 ms | 187.143 ms |
| `logical_warm` | 349.427 ms | 351.502 ms | 732.63 chunks/s | 146.571 ms | 170.286 ms |

Additional one-shot evidence:

| Operation | Mode | Result |
| --- | --- | --- |
| compatibility scan | `single` (1 thread) | 1,764,971 records in 14,908 ms (118,390.86 records/s) |
| compatibility scan | `parallel_auto` (16 threads) | 1,764,971 records in 3,372 ms (523,419.63 records/s, 4.42x speedup) |
| classify keys | `single` (1 thread) | 1,764,971 entries in 14,370 ms (122,822.13 entries/s) |
| classify keys | `parallel_auto` (16 threads) | 1,764,971 entries in 3,177 ms (555,513.55 entries/s, 4.52x speedup) |
| key scan | `single` (1 thread) | 1,764,971 entries in 14,154 ms (124,696.99 entries/s) |
| key scan | `parallel_auto` (16 threads) | 1,764,971 entries in 3,059 ms (576,926.58 entries/s, 4.63x speedup) |
| player prefix scan | `single` (1 thread) | 110 records in 95 ms (1,153.18 entries/s) |
| player prefix scan | `parallel_auto` (16 threads) | 110 records in 19 ms (5,712.24 entries/s) |
| actorprefix scan | `single` (1 thread) | 53,777 entries in 401 ms (133,894.97 entries/s) |
| actorprefix scan | `parallel_auto` (16 threads) | 53,777 entries in 82 ms (652,717.46 entries/s) |
| digp digest scan | `single` (1 thread) | 109,425 entries in 123 ms (885,523.30 entries/s) |
| digp digest scan | `parallel_auto` (16 threads) | 109,425 entries in 37 ms (2,941,034.18 entries/s) |
| entity full scan & parse | `single` (1 thread) | 53,777 entities in 15,747 ms (3,415.06 entities/s, 0 errors) |
| entity full scan & parse | `parallel_auto` (16 threads) | 53,777 entities in 4,178 ms (12,870.03 entities/s, 0 errors) |
| block entity full scan & parse | `single` (1 thread) | 56,652 block entities in 15,071 ms (3,758.83 entries/s, 0 errors) |
| block entity full scan & parse | `parallel_auto` (16 threads) | 56,652 block entities in 3,488 ms (16,240.82 entries/s, 0 errors) |
| player list (dynamic) | 1 thread | 111 players in 19 ms |
| player list (generic) | 1 thread | 111 players in 19 ms |
| sample chunk parse | 1 thread | 1 chunk in 22 ms |
| fixed layer query | 1 thread | 256 chunks in 184 ms; DB 177 ms, decode 6 ms |

## Latest synthetic Criterion result

Local run:

```text
date: 2026-08-22
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
| --- | ---: | --- | ---: |
| `bedrock_world/level_dat/parse_synthetic` | 452.41 ns | 442.08..459.65 ns | 174.96 MiB/s |
| `bedrock_world/level_dat/nbt_events_synthetic` | 188.92 ns | 186.83..190.77 ns | 418.98 MiB/s |
| `bedrock_world/level_dat/nbt_root_owned_synthetic` | 680.46 ns | 677.88..685.39 ns | 116.33 MiB/s |
| `bedrock_world/level_dat/nbt_root_ref_synthetic` | 186.01 ns | 185.55..186.72 ns | 425.55 MiB/s |

### Performance Optimization & Server Scanning Highlights

- **Parallel Table & World Scan Breakthrough**:
  - `compatibility scan` (parallel 16T): Dropped to **3.37 seconds** (🚀 **8.07x speedup**, 523,419 records/s).
  - `classify keys` (parallel 16T): Dropped to **3.18 seconds** (🚀 **7.21x speedup**, 555,513 entries/s).
  - `key scan` (parallel 16T): Dropped to **3.06 seconds** (🚀 **8.28x speedup**, 576,926 entries/s).
- **Server Entity & Digest Discovery Standards**:
  - `actorprefix scan` (parallel 16T): Discovered all **53,777 actor entities** in **82 ms** (🚀 **652,717 entries/s**).
  - `digp scan` (parallel 16T): Discovered all **109,425 chunk actor digests** in **37 ms** (🚀 **2,941,034 entries/s**).
  - `entity full scan & parse` (parallel 16T): Decompressed and parsed all 53,777 entities from NBT in **4.18 seconds** (🚀 **12,870.03 entities/s**, 0 errors).
  - `block entity full scan & parse` (parallel 16T): Decompressed and parsed all 56,652 containers/tiles in **3.49 seconds** (🚀 **16,240.82 block entities/s**, 0 errors).
- **Player Prefix & Point Query Latency**:
  - `player prefix scan` (parallel 16T): Finished in **19 ms** (was 599 ms).
  - `list players`: Finished in **19 ms** (was ~43.3s initially, 🚀 **2,279x faster**).
  - `sample chunk parse`: Finished in **22 ms** (was ~43.8s initially, 🚀 **1,990x faster**).
- **Chunk Read/Decode Throughput**:
  - `logical_cold`: **705.64 chunks/s** (was 295.83 chunks/s initially, **+138.5% throughput**, p50: 362.8 ms).
  - `logical_warm`: **732.63 chunks/s** (was 386.42 chunks/s initially, **+89.6% throughput**, p50: 349.4 ms).
  - `fixed layer query`: 256 chunks batch query takes only **184 ms** (was 721 ms, **3.92x faster**).
