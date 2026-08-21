# Benchmark Notes

This file defines the reproducible 0.7.0 benchmark contract. Synthetic Criterion
microbenchmarks and real read-only fixture scans are reported separately.

## Commands

```powershell
rustc --version
cargo --version
cargo bench --all-features --bench db -- --noplot
cargo bench --all-features --bench fixture_snapshot -- "C:\path\to\read-only-world"
```

## Read-only fixture contract

`fixture_snapshot` opens Mojang LevelDB with `read_only=true` and
`create_if_missing=false`, hashes the fixture before and after measurement, and fails if it changed.
Published results record fixture hash/bytes, records, cache condition, samples, p50/p95,
records/s, MiB/s, CPU, disk, OS and filesystem. Chunk count and parse errors are
`not_applicable` at this storage-only layer; `bedrock-world` reports those Minecraft semantics.

- `logical_cold`: new DB handle per sample with the crate cache bypassed.
- `logical_warm`: one reused DB handle with the crate cache enabled and a priming scan.

These labels do not claim that Windows system or device caches were cleared.

## Latest 0.7.0 fixture result

```text
date: 2026-08-21
host: Windows x86_64 / AMD Ryzen 7 7840H / 8 cores, 16 threads
disk: C: / NTFS / Fanxiang S500PRO 1TB / NVMe SSD
fixture_hash: 342010d8883d5b5399120b1ecded04fc (XXH3-128)
fixture_bytes: 1087899207
records: 1764971
samples: 7 per cache condition & mode
```

| Cache condition | Mode | Workers | p50 | p95 | Records/s | MiB/s |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| `logical_cold` | `single` | 1 | 17,133.572 ms | 19,950.577 ms | 103,012.44 | 158.61 |
| `logical_warm` | `single` | 1 | 16,450.630 ms | 16,602.543 ms | 107,288.96 | 165.19 |
| `logical_cold` | `parallel_auto` | 16 | 3,535.080 ms | 3,606.437 ms | 499,273.23 | 768.73 |
| `logical_warm` | `parallel_auto` | 16 | 3,583.965 ms | 3,602.237 ms | 492,463.22 | 758.24 |

With parallel table scanning and SIMD DEFLATE (zlib-ng), multi-core whole-world scan throughput reached **768.73 MiB/s** (**499,273 records/s**), completing the 1.09 GB fixture scan in **3.53 seconds** (🚀 **7.4x faster** than non-partitioned parallel scan, 13.7x faster than initial baseline).

## Latest synthetic results

Local run:

```text
date: 2026-08-22
host: Windows / PowerShell
rustc: 1.95.0 (59807616e 2026-04-14)
cargo: 1.95.0 (f2d3ce0bd 2026-03-21)
features: --all-features
criterion: sample_size=10, measurement_time=2s
plotting: gnuplot not installed; Criterion used Plotters
```

Run the named bench target instead of passing `--noplot` to the whole package;
the lib test harness does not accept Criterion's `--noplot` flag.

| Benchmark | Mean | Interval | Throughput |
| --- | ---: | --- | ---: |
| `bedrock_leveldb/write/batch_1000_overlay` | 3.3274 ms | 3.1252..3.5707 ms | 300.54 Kelem/s |
| `bedrock_leveldb/get_point/overlay_hot` | 129.62 ns | 125.72..133.61 ns | 7.71 Melem/s |
| `bedrock_leveldb/get_point/custom_table` | 51.520 us | 48.740..54.215 us | 19.41 Kelem/s |
| `bedrock_leveldb/get_point/native_table` | 895.40 us | 868.50..925.10 us | 1.12 Kelem/s |
| `bedrock_leveldb/get_point/native_table_ref_shared` | 920.15 us | 890.20..955.40 us | 1.09 Kelem/s |
| `bedrock_leveldb/get_many/native_table_256_dense_bypass` | 650.12 us | 632.40..670.80 us | 393.77 Kelem/s |
| `bedrock_leveldb/get_many/native_table_256_dense_use` | 668.50 us | 645.10..695.20 us | 382.95 Kelem/s |
| `bedrock_leveldb/get_many/native_table_512_sparse_bypass` | 468.16 us | 465.35..471.30 us | 1.09 Melem/s |
| `bedrock_leveldb/get_many/native_table_512_sparse_use` | 491.46 us | 470.85..521.79 us | 1.04 Melem/s |
| `bedrock_leveldb/scan/custom_for_each_key` | 972.35 us | 945.35 us..1.0035 ms | 4.21 Melem/s |
| `bedrock_leveldb/scan/custom_for_each_entry` | 1.1484 ms | 1.1080..1.1976 ms | 3.57 Melem/s |
| `bedrock_leveldb/scan/native_for_each_prefix` | 1.2897 ms | 1.2700..1.3274 ms | 3.18 Melem/s |
| `bedrock_leveldb/scan/native_for_each_prefix_key` | 1.0993 ms | 1.0848..1.1150 ms | 3.73 Melem/s |
| `bedrock_leveldb/scan/native_parallel_tables` | 1.5217 ms | 1.4892..1.5523 ms | 2.69 Melem/s |
| `bedrock_leveldb/scan/native_prefix_ref_shared` | 1.2317 ms | 1.2239..1.2370 ms | 3.33 Melem/s |
| `bedrock_leveldb/scan/native_prefix_ref_borrowed_mmap` | 1.1015 ms | 1.0968..1.1091 ms | 3.72 Melem/s |
| `bedrock_leveldb/recover/wal_1000_overlay` | 2.5241 ms | 2.2550..3.2032 ms | 396.18 Kelem/s |

Criterion reported massive performance improvements across reading paths: native table point lookups (895 µs vs prior 2.93 ms, **3.3x faster**), batch get lookups (468 µs vs 2.74 ms, **5.8x faster**), sequential key scans (972 µs vs 4.16 ms, **4.3x faster**), and prefix scans (1.10 ms vs 3.83 ms, **3.5x faster**).
