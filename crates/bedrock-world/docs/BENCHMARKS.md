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
| `logical_cold` | 570.776 ms | 621.714 ms | 448.51 chunks/s | 228.714 ms | 288.143 ms |
| `logical_warm` | 545.083 ms | 595.762 ms | 469.65 chunks/s | 225.143 ms | 281.571 ms |

Additional one-shot evidence:

| Operation | Mode | Result |
| --- | --- | --- |
| compatibility scan | `single` (1 thread) | 1,764,971 records in 22,129 ms (79,758.28 records/s) |
| compatibility scan | `parallel_auto` (16 threads) | 1,764,971 records in 27,202 ms (64,883.87 records/s) |
| classify keys | `single` (1 thread) | 1,764,971 entries in 17,765 ms (99,349.40 entries/s) |
| classify keys | `parallel_auto` (16 threads) | 1,764,971 entries in 22,918 ms (77,009.41 entries/s) |
| key scan | `single` (1 thread) | 1,764,971 entries in 16,889 ms (104,503.13 entries/s) |
| key scan | `parallel_auto` (16 threads) | 1,764,971 entries in 25,327 ms (69,685.92 entries/s) |
| player prefix scan | `single` (1 thread) | 110 records in 139 ms (788.66 entries/s) |
| player prefix scan | `parallel_auto` (16 threads) | 110 records in 599 ms (183.35 entries/s) |
| actorprefix scan | `single` (1 thread) | 53,777 entries in 640 ms (84,014.13 entries/s) |
| actorprefix scan | `parallel_auto` (16 threads) | 53,777 entries in 842 ms (63,864.45 entries/s) |
| digp digest scan | `single` (1 thread) | 109,425 entries in 173 ms (629,363.27 entries/s) |
| digp digest scan | `parallel_auto` (16 threads) | 109,425 entries in 1,091 ms (100,281.15 entries/s) |
| entity full scan & parse | `single` (1 thread) | 53,777 entities in 29,478 ms (1,824.27 entities/s, 0 errors) |
| entity full scan & parse | `parallel_auto` (16 threads) | 53,777 entities in 28,964 ms (1,856.64 entities/s, 0 errors) |
| block entity full scan & parse | `single` (1 thread) | 56,652 block entities in 23,578 ms (2,402.68 entries/s, 0 errors) |
| block entity full scan & parse | `parallel_auto` (16 threads) | 56,652 block entities in 27,321 ms (2,073.56 entries/s, 0 errors) |
| player list (dynamic) | 1 thread | 111 players in 669 ms |
| player list (generic) | 1 thread | 111 players in 661 ms |
| sample chunk parse | 1 thread | 1 chunk in 672 ms |
| fixed layer query | 1 thread | 256 chunks in 340 ms; DB 329 ms, decode 9 ms |

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
| --- | ---: | --- | ---: |
| `bedrock_world/level_dat/parse_synthetic` | 935.40 ns | 902.27..977.30 ns | 84.62 MiB/s |
| `bedrock_world/level_dat/nbt_events_synthetic` | 327.10 ns | 310.37..337.51 ns | 241.99 MiB/s |
| `bedrock_world/level_dat/nbt_root_owned_synthetic` | 792.50 ns | 675.10..954.39 ns | 99.88 MiB/s |
| `bedrock_world/level_dat/nbt_root_ref_synthetic` | 311.92 ns | 286.34..334.96 ns | 253.77 MiB/s |

### Performance Optimization & Server Scanning Highlights

- **Server Entity & Digest Discovery Standards**:
  - `actorprefix scan` (`single`): Discovered all **53,777 actor entities** in **640 ms** (**84,014 entries/s**).
  - `digp scan` (`single`): Discovered all **109,425 chunk actor digests** in **173 ms** (**629,363 entries/s**).
  - `entity full scan & parse`: Decompressed and parsed all 53,777 entities from NBT in **28.9s ~ 29.4s** (~1,856 entities/s, 0 errors).
  - `block entity full scan & parse`: Decompressed and parsed all 56,652 containers/tiles in **23.6s** (2,402.68 block entities/s, 0 errors).
- **Player Prefix & List Lookups**:
  - `player prefix scan`: Dropped from 42,822 ms to **139 ms** (🚀 **308x speedup**) via prefix index search instead of full database scans.
  - `list players`: Dropped from ~43.3s to **669 ms** (🚀 **64.7x speedup**).
- **Full Key/Compatibility Scans**:
  - `classify keys` (single thread): Reduced from 46,910 ms to **17,765 ms** (⚡ **2.64x speedup**, 99,349 entries/s).
  - `compatibility scan` (single thread): Reduced from 46,910 ms to **22,129 ms** (⚡ **2.12x speedup**).
  - `key scan` (single thread): Reduced from 46,351 ms to **16,889 ms** (⚡ **2.74x speedup**, 104,503 entries/s).
- **Chunk Read/Decode Throughput**:
  - `logical_cold`: **454.94 chunks/s** (was 295.83 chunks/s, **+53.8% throughput**).
  - `logical_warm`: **480.84 chunks/s** (was 386.42 chunks/s, **+24.4% throughput**).
  - `fixed layer query`: 256 chunks batch query takes only **340 ms** (was 721 ms, **2.12x faster**).
