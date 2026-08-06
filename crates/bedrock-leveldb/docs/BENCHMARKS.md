# Benchmark Notes

This file records the latest local v0.2.0 benchmark run. Re-run the commands
below before comparing storage changes because Criterion output depends on the
host CPU, thermal state, filesystem, and background load.

## Commands

```powershell
rustc --version
cargo --version
cargo bench --all-features --bench db -- --noplot
```

## Latest Results

Local run:

```text
date: 2026-05-07
host: Windows / PowerShell
rustc: 1.93.1 (01f6ddf75 2026-02-11)
cargo: 1.93.1 (083ac5135 2025-12-15)
features: --all-features
criterion: sample_size=10, measurement_time=2s
plotting: gnuplot not installed; Criterion used Plotters
```

Run the named bench target instead of passing `--noplot` to the whole package;
the lib test harness does not accept Criterion's `--noplot` flag.

| Benchmark | Mean | Interval |
| --- | ---: | --- |
| `bedrock_leveldb/write/batch_1000_overlay` | 2.3576 ms | 2.3293..2.3915 ms |
| `bedrock_leveldb/get_point/overlay_hot` | 116.35 ns | 113.58..118.91 ns |
| `bedrock_leveldb/get_point/custom_table` | 5.0447 ms | 4.9506..5.1866 ms |
| `bedrock_leveldb/get_point/native_table` | 4.9751 ms | 4.9268..5.0552 ms |
| `bedrock_leveldb/get_point/native_table_ref_shared` | 5.0564 ms | 4.9894..5.1388 ms |
| `bedrock_leveldb/scan/custom_for_each_key` | 6.8257 ms | 6.6420..7.1484 ms |
| `bedrock_leveldb/scan/custom_for_each_entry` | 7.1233 ms | 6.9656..7.3259 ms |
| `bedrock_leveldb/scan/native_for_each_prefix` | 6.6402 ms | 6.4509..7.0278 ms |
| `bedrock_leveldb/scan/native_parallel_tables` | 3.7625 ms | 3.5858..4.1023 ms |
| `bedrock_leveldb/scan/native_prefix_ref_shared` | 6.8587 ms | 6.4358..7.3452 ms |
| `bedrock_leveldb/scan/native_prefix_ref_borrowed_mmap` | 7.2782 ms | 7.1565..7.4650 ms |
| `bedrock_leveldb/recover/wal_1000_overlay` | 2.1511 ms | 1.9346..2.4335 ms |

Criterion reported local improvements for native point reads, WAL recovery, and
overlay writes versus the prior machine baseline. It reported regressions for
legacy custom-table reads/scans and the mmap borrowed-prefix scan. Treat those
comparisons as local signals only; the absolute numbers above are the v0.2.0
reference for this machine.
