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
| `logical_cold` | `single` | 1 | 20,903.117 ms | 23,050.600 ms | 84,435.78 | 130.01 |
| `logical_warm` | `single` | 1 | 22,379.099 ms | 22,517.430 ms | 78,866.94 | 121.43 |
| `logical_cold` | `parallel_auto` | 16 | 26,077.559 ms | 26,353.643 ms | 67,681.60 | 104.21 |
| `logical_warm` | `parallel_auto` | 16 | 26,166.812 ms | 26,926.023 ms | 67,450.75 | 103.85 |

With SIMD DEFLATE (zlib-ng) and borrowed value scanning, single-core throughput increased from **56.19 MiB/s to 130.01 MiB/s** (**2.31x speedup**), cutting scan time from 48.4s to 20.9s.

## Latest synthetic results

Local run:

```text
date: 2026-08-21
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
| `bedrock_leveldb/write/batch_1000_overlay` | 4.0317 ms | 3.9025..4.1856 ms | 248.03 Kelem/s |
| `bedrock_leveldb/get_point/overlay_hot` | 155.16 ns | 152.93..157.81 ns | 6.45 Melem/s |
| `bedrock_leveldb/get_point/custom_table` | 58.939 us | 53.881..62.240 us | 16.97 Kelem/s |
| `bedrock_leveldb/get_point/native_table` | 932.60 us | 915.06..954.14 us | 1.07 Kelem/s |
| `bedrock_leveldb/get_point/native_table_ref_shared` | 1.0016 ms | 950.52 us..1.0356 ms | 998.37 elem/s |
| `bedrock_leveldb/get_many/native_table_256_dense_bypass` | 694.96 us | 683.57..710.96 us | 368.37 Kelem/s |
| `bedrock_leveldb/get_many/native_table_256_dense_use` | 712.24 us | 683.39..760.04 us | 359.43 Kelem/s |
| `bedrock_leveldb/get_many/native_table_512_sparse_bypass` | 783.00 us | 759.59..832.70 us | 653.90 Kelem/s |
| `bedrock_leveldb/get_many/native_table_512_sparse_use` | 775.93 us | 760.76..805.10 us | 659.85 Kelem/s |
| `bedrock_leveldb/scan/custom_for_each_key` | 1.5163 ms | 1.5034..1.5318 ms | 2.70 Melem/s |
| `bedrock_leveldb/scan/custom_for_each_entry` | 1.8249 ms | 1.8079..1.8462 ms | 2.24 Melem/s |
| `bedrock_leveldb/scan/native_for_each_prefix` | 2.5390 ms | 2.4459..2.6265 ms | 1.61 Melem/s |
| `bedrock_leveldb/scan/native_for_each_prefix_key` | 2.1601 ms | 2.1014..2.2676 ms | 1.90 Melem/s |
| `bedrock_leveldb/scan/native_parallel_tables` | 2.3060 ms | 2.2664..2.3386 ms | 1.78 Melem/s |
| `bedrock_leveldb/scan/native_prefix_ref_shared` | 2.2389 ms | 2.0451..2.3786 ms | 1.83 Melem/s |
| `bedrock_leveldb/scan/native_prefix_ref_borrowed_mmap` | 1.9789 ms | 1.8988..2.0901 ms | 2.07 Melem/s |
| `bedrock_leveldb/recover/wal_1000_overlay` | 4.2906 ms | 3.7904..5.5232 ms | 233.07 Kelem/s |

Criterion reported massive performance improvements across reading paths: native table point lookups (932.6 µs vs prior 2.93 ms, **3.1x faster**), batch get lookups (695 µs vs 2.74 ms, **3.9x faster**), sequential key scans (1.52 ms vs 4.16 ms, **2.7x faster**), and prefix scans (1.98 ms vs 3.83 ms, **1.9x faster**).
