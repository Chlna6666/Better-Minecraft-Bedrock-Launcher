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
records: 1768601
samples: 7 per cache condition
```

| Cache condition | p50 | p95 | Records/s | MiB/s |
| --- | ---: | ---: | ---: | ---: |
| `logical_cold` | 48,366.635 ms | 49,635.690 ms | 36,566.55 | 56.19 |
| `logical_warm` | 52,138.520 ms | 53,721.049 ms | 33,921.20 | 52.12 |

The cache-enabled full scan was slower on this fixture. This is a measured result, not an assumed
benefit: the 1.09 GB physical snapshot expands to 2.85 GB of visited key/value bytes, so cache
bookkeeping and churn can outweigh reuse for a sequential whole-database scan.

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
| `bedrock_leveldb/write/batch_1000_overlay` | 2.6076 ms | 2.5085..2.7201 ms | 383.49 Kelem/s |
| `bedrock_leveldb/get_point/overlay_hot` | 112.56 ns | 111.03..114.16 ns | 8.88 Melem/s |
| `bedrock_leveldb/get_point/custom_table` | 34.496 us | 32.502..37.583 us | 28.99 Kelem/s |
| `bedrock_leveldb/get_point/native_table` | 2.9256 ms | 2.8165..2.9867 ms | 341.81 elem/s |
| `bedrock_leveldb/get_point/native_table_ref_shared` | 2.9799 ms | 2.9175..3.0201 ms | 335.58 elem/s |
| `bedrock_leveldb/get_many/native_table_256_dense_bypass` | 2.7376 ms | 2.6952..2.7960 ms | 93.51 Kelem/s |
| `bedrock_leveldb/get_many/native_table_256_dense_use` | 2.9677 ms | 2.9323..3.0097 ms | 86.26 Kelem/s |
| `bedrock_leveldb/get_many/native_table_512_sparse_bypass` | 2.7479 ms | 2.7308..2.7713 ms | 186.33 Kelem/s |
| `bedrock_leveldb/get_many/native_table_512_sparse_use` | 2.9911 ms | 2.8652..3.0911 ms | 171.17 Kelem/s |
| `bedrock_leveldb/scan/custom_for_each_key` | 4.1619 ms | 4.0830..4.2982 ms | 984.16 Kelem/s |
| `bedrock_leveldb/scan/custom_for_each_entry` | 4.2543 ms | 4.1331..4.4003 ms | 962.80 Kelem/s |
| `bedrock_leveldb/scan/native_for_each_prefix` | 3.9668 ms | 3.9038..4.0358 ms | 1.03 Melem/s |
| `bedrock_leveldb/scan/native_for_each_prefix_key` | 3.9382 ms | 3.9010..3.9855 ms | 1.04 Melem/s |
| `bedrock_leveldb/scan/native_parallel_tables` | 4.0201 ms | 3.8907..4.1834 ms | 1.02 Melem/s |
| `bedrock_leveldb/scan/native_prefix_ref_shared` | 4.1783 ms | 4.0452..4.3135 ms | 980.29 Kelem/s |
| `bedrock_leveldb/scan/native_prefix_ref_borrowed_mmap` | 3.8263 ms | 3.7802..3.8860 ms | 1.07 Melem/s |
| `bedrock_leveldb/recover/wal_1000_overlay` | 3.6836 ms | 3.2686..4.6478 ms | 271.47 Kelem/s |

Criterion reported significant improvements for custom-table point lookups (34.5 µs vs prior 5.04 ms, ~146x faster), native table point lookups (2.93 ms vs 4.98 ms, 1.7x faster), table scans (4.16 ms vs 6.83 ms), and prefix scans (3.83 ms vs 7.28 ms, ~1.9x faster). Treat comparisons as local machine indicators.
