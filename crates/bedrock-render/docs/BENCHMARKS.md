# Benchmarks

## 中文说明

`benches/render.rs` 使用 Criterion 和相邻 `bedrock-world` checkout 中的可选
sample world。如果 fixture 不存在，benchmark 会直接跳过 world-backed case。

默认快速套件：

```powershell
cargo bench --bench render --all-features
```

快速套件覆盖代表性的 tile 渲染、chunk bake 和小批量渲染。完整 web-map 导出比较慢，
默认由 `RUN_FULL_BENCHMARKS = false` 关闭；运行前把 `benches/render.rs` 中该常量改为 `true`：

```powershell
cargo bench --bench render --all-features
```

## 缓存与验证对比

在同一机器上将渲染和缓存套件成对运行：

```powershell
cargo bench --bench render --bench cache --all-features
```

`cache` 不依赖 world fixture，独立测量内存 tile 索引查找、复用文件句柄后的 64 KiB
authority payload 读取与 XXH3-128 校验、变更 payload 的 extent 刷新提交，以及 128 个
16 KiB chunk 的 Rayon 并行指纹验证。`render` 中的 `surface_tile_256_rgba` 是冷渲染
基线；缓存命中与编辑刷新必须分别与对应的 `cache` case 比较，不能把不同机器或 feature
set 的中位数相加。

格式或缓存策略变更前后使用同名 Criterion baseline 对比：

```powershell
cargo bench --bench render --bench cache --all-features -- --save-baseline before
# 修改后：
cargo bench --bench render --bench cache --all-features -- --baseline before
```

记录 CPU 型号、核心数、存储设备、电源模式、Rust 版本、feature set、fixture revision，
以及 Windows Defender 是否索引临时 benchmark 目录。缓存套件不测 world 打开、renderer
启动、GPU upload 或 texture cache。

`authority_refresh_new_payload_64k` 在 WAL 小于 512 KiB 时测量 blob 写入加 WAL delta
追加；它不重写完整 checkpoint 索引。超过阈值的下一批会在后台 checkpoint，因此应另行
记录该低频写放大，而不要把它混入交互刷新中位数。

如果只需要固定 `key=value` 字段的机器可读报告，可以使用不匹配任何 Criterion case
的过滤器：

```powershell
cargo bench --bench render --all-features -- --noplot __machine_report_only__
```

报告字段包括 `storage`、`backend`、`elapsed_ms`、`tiles`、`worker_threads`、
`world_worker_threads`、`prefix_scans`、`exact_get_batches`、
`exact_keys_requested`、`exact_keys_found`、`db_read_ms`、`decode_ms`、
`gpu_tiles`、`cpu_tiles`、`gpu_requested`、`gpu_actual`、`gpu_adapter`、
`gpu_device`、`gpu_fallback`、`gpu_upload_ms`、`gpu_dispatch_ms`、
`gpu_readback_ms`、`gpu_uploaded_bytes`、`gpu_readback_bytes`、
`gpu_peak_in_flight` 和 `gpu_buffer_reuses`。

v0.2.0 editor 门面还会输出：

- `v02_overlay_query`：区域 overlay 查询，包含 entity、block entity、HSA 和 village 数量。
- `v02_map_scan`：地图记录扫描数量。
- `v02_global_scan`：全局记录扫描数量、scoreboard 是否存在，以及解析错误字段。
- `v02_hsa_scan`：HSA chunk 数和区域数。
- `v02_edit_invalidation`：`MapEditInvalidation` 合并辅助逻辑耗时和刷新标志。

## GPU 对比套件

GPU 对比是 opt-in 的，因为它依赖本机驱动、适配器和后台负载。运行前把
`benches/render.rs` 中的 `RUN_GPU_COMPARISON_REPORTS` 改为 `true`；Windows 专业对比
应该至少覆盖 `CPU`、`Auto`、`DX11` 和 `Vulkan` 四个路径：

```powershell
cargo bench --bench render --features "gpu-dx11 gpu-vulkan" -- --noplot __machine_report_only__
```

如果需要同时验证 DX12 crate 编译，可单独在 `bedrock-render` 仓库运行
`cargo check --features gpu-dx12`。BMCBL 默认 Windows 构建不启用 DX12，避免与
宿主 UI 依赖的 Windows crate 版本产生冲突。

GPU 对比报告会额外输出：

```text
bedrock_render_report case=surface_region_rgba_gpu_compare storage=generic backend=cpu ...
bedrock_render_report case=surface_region_rgba_gpu_compare storage=generic backend=auto ...
bedrock_render_report case=surface_region_rgba_gpu_compare storage=generic backend=dx11 ...
bedrock_render_report case=surface_region_rgba_gpu_compare storage=generic backend=vulkan ...
```

解读规则：

- `backend=cpu` 是基线，不应出现 `gpu_tiles > 0`。
- `backend=auto` 在 Windows 上应优先尝试 DX11；如果 DX11 不可用，允许 Vulkan 或
  CPU fallback，但必须记录 `gpu_fallback`。
- `backend=dx11` 期望 `gpu_actual=Dx11` 且 `gpu_tiles > 0`；没有 D3D11 compute
  设备时允许 fallback，但这不是性能基线。
- `backend=vulkan` 期望 `gpu_actual=Vulkan` 且 `gpu_tiles > 0`；没有 Vulkan 驱动时
  允许 fallback。
- GPU 性能只和同一机器、同一驱动、同一 world fixture、同一 feature set 的结果比较。
- `elapsed_ms` 必须和 `db_read_ms`、`decode_ms`、`gpu_upload_ms`、`gpu_dispatch_ms`、
  `gpu_readback_ms` 一起看；如果 DB/decode 占主导，GPU 后端差异不会代表真实瓶颈。
- 当前 DX11/Vulkan 路径仍通过 readback 返回 RGBA 给现有 image/cache 管线，因此
  `gpu_readback_ms` 是关键指标；未来零拷贝 texture 显示需要单独建新基线。

当前本地基线在 2026-05-07 采集，系统为 Windows，默认 features，fixture 路径为
`C:\Users\Administrator\Desktop\BE-Community-Dev\bedrock-world\tests\fixtures\sample-bedrock-world`。
Criterion case 使用 typed `BedrockWorld<BedrockLevelDbStorage>` 构造；baked surface
tile 使用 exact-batch world load；editor 场景通过 `MapWorldEditor` 只读打开同一 fixture。

机器可读报告：

```text
bedrock_render_report case=surface_region_rgba storage=generic backend=default elapsed_ms=582 tiles=1 worker_threads=1 world_worker_threads=1 prefix_scans=0 exact_get_batches=1 exact_keys_requested=7424 exact_keys_found=2558 db_read_ms=256 decode_ms=234 gpu_tiles=0 cpu_tiles=1 gpu_requested=Auto gpu_actual=Auto gpu_adapter=none gpu_device=none gpu_fallback=none gpu_upload_ms=0 gpu_dispatch_ms=0 gpu_readback_ms=0 gpu_uploaded_bytes=0 gpu_readback_bytes=0 gpu_peak_in_flight=0 gpu_buffer_reuses=0
bedrock_render_report case=v02_overlay_query storage=editor elapsed_ms=14957 chunks=256 entities=0 block_entities=64 hsa=0 villages=182
bedrock_render_report case=v02_map_scan storage=editor elapsed_ms=188 records=1760
bedrock_render_report case=v02_global_scan storage=editor elapsed_ms=1149 records=0 scoreboard_found=false error=Bedrock_world_error:_NBT_error:_unknown_NBT_tag_type:_52
bedrock_render_report case=v02_hsa_scan storage=editor elapsed_ms=1797 chunks=842 areas=1703
bedrock_render_report case=v02_edit_invalidation storage=memory elapsed_ns=14100 affected_chunks=2 refresh_metadata=true refresh_overlays=true clear_tile_cache=true
bedrock_render_report case=surface_region_rgba storage=dynamic backend=default elapsed_ms=576 tiles=1 worker_threads=1 world_worker_threads=1 prefix_scans=0 exact_get_batches=1 exact_keys_requested=7424 exact_keys_found=2558 db_read_ms=264 decode_ms=235 gpu_tiles=0 cpu_tiles=1 gpu_requested=Auto gpu_actual=Auto gpu_adapter=none gpu_device=none gpu_fallback=none gpu_upload_ms=0 gpu_dispatch_ms=0 gpu_readback_ms=0 gpu_uploaded_bytes=0 gpu_readback_bytes=0 gpu_peak_in_flight=0 gpu_buffer_reuses=0
```

Criterion 快速套件：

| Benchmark | 典型耗时 |
| --- | ---: |
| `biome_tile_256_rgba` | 273.6 ms |
| `biome_tile_256_webp` | 257.5 ms |
| `fixed_y_tile_256_rgba` | 281.0 ms |
| `raw_biome_tile_256_rgba` | 264.8 ms |
| `surface_tile_256_rgba` | 406.7 ms |
| `interactive_ordered_surface_batch_2x2_rgba` | 1.0457-1.0833 s (quick smoke) |
| `bake_chunk_surface` | 31.3 ms |
| `render_tile_surface_from_bake` | 418.9 ms |
| `heightmap_tile_256_rgba` | 373.2 ms |
| `cave_slice_tile_256_rgba` | 270.9 ms |
| `tile_batch_auto_threads` | 2.32 s |
| `tile_batch_single_thread` | 1.25 s |

解读时不要只看总耗时。`prefix_scans=0` 且 `exact_get_batches=1` 表示 sampled
surface region 已经走 exact batch 读取，没有在渲染循环内做 prefix scan。后续比较
需要同时记录 GPU adapter/driver、`RenderCpuPipelineOptions`、`RenderTilePriority`、
`RenderGpuOptions`、是否启用 `block_boundaries`，以及使用 `HeightMap` 还是
`RawHeightMap`。

---

`benches/render.rs` uses Criterion against the optional sample world from the
adjacent `bedrock-world` checkout. If the fixture is missing, the benchmark
returns without measuring world-backed cases.

## Quick Suite

Run the default suite with:

```powershell
cargo bench --bench render --all-features
```

The quick suite measures representative tile renders, chunk baking, and small
batch behavior. It avoids repeating full web-map exports by default.

## Cache And Validation Comparison

Run the render and cache suites as one comparison on the same machine:

```powershell
cargo bench --bench render --bench cache --all-features
```

`cache` is fixture-independent and measures four isolated paths:

| Benchmark | What it measures | Use it to compare |
| --- | --- | --- |
| `authority_index_lookup` | In-memory `(tile_x, tile_z)` lookup | Index overhead before any I/O |
| `authority_payload_pread_64k` | Reused file handle plus one 64 KiB authority payload read and XXH3-128 verification | Disk-cache hit latency |
| `authority_refresh_new_payload_64k` | A changed 64 KiB payload, extent allocation, and an atomic WAL delta append | Interactive cache-refresh write cost |
| `xxh3_128_validate_128x16k` | Rayon validation of 128 independent 16 KiB chunk records | CPU change-detection throughput |

Interpret the two suites together. `surface_tile_256_rgba` from `render` is the
cold render baseline; a cache hit should be compared with
`authority_payload_pread_64k`, while an edited tile should be compared with
`authority_refresh_new_payload_64k` plus its render cost. Do not add those
medians from different machines or feature sets.

For a regression check, save Criterion's baseline before a storage-format
change and compare it after the change:

```powershell
cargo bench --bench render --bench cache --all-features -- --save-baseline before
# Apply the change.
cargo bench --bench render --bench cache --all-features -- --baseline before
```

Record CPU model, core count, storage device, power mode, Rust version, enabled
features, fixture revision, and whether Windows Defender indexed the temporary
benchmark directory. Cache benchmarks use a reusable reader handle; they do
not measure renderer startup, world opening, GPU upload, or texture caching.

Below the 512 KiB WAL checkpoint threshold,
`authority_refresh_new_payload_64k` measures the blob write plus one durable
WAL delta, not a complete generation-index rewrite. Record the infrequent
checkpoint separately so its write amplification is not folded into the
interactive refresh median.

The harness also prints one-shot machine-readable report lines before Criterion
samples. Use a filter that matches no Criterion case when you only need those
lines:

```powershell
cargo bench --bench render --all-features -- --noplot __machine_report_only__
```

Each report line uses fixed `key=value` fields, including `storage`, `backend`,
`elapsed_ms`, `tiles`, `worker_threads`, `world_worker_threads`,
`prefix_scans`, `exact_get_batches`, `exact_keys_requested`,
`exact_keys_found`, `db_read_ms`, `decode_ms`, `gpu_tiles`, `cpu_tiles`,
`gpu_requested`, `gpu_actual`, `gpu_adapter`, `gpu_device`, `gpu_fallback`,
`gpu_upload_ms`, `gpu_dispatch_ms`, `gpu_readback_ms`, `gpu_uploaded_bytes`,
`gpu_readback_bytes`, `gpu_peak_in_flight`, and `gpu_buffer_reuses`.

The v0.2.0 editor facade reports are emitted alongside the render reports:

- `v02_overlay_query` records region overlay scan time and overlay counts.
- `v02_map_scan` records typed map record scan time and count.
- `v02_global_scan` records typed global scan time, scoreboard presence, and an
  `error` field when fixture data contains a record the parser rejects.
- `v02_hsa_scan` records HSA chunk and area counts.
- `v02_edit_invalidation` records `MapEditInvalidation` merge helper timing and
  refresh flags.

## GPU Comparison Suite

GPU comparison is opt-in because it depends on local drivers, adapters, and
background load. Set `RUN_GPU_COMPARISON_REPORTS` to `true` in
`benches/render.rs` first. On Windows, professional comparisons should cover
`CPU`, `Auto`, `DX11`, and `Vulkan`:

```powershell
cargo bench --bench render --features "gpu-dx11 gpu-vulkan" -- --noplot __machine_report_only__
```

If DX12 crate compilation needs to be verified, run
`cargo check --features gpu-dx12` in `bedrock-render` separately. BMCBL's
default Windows build does not enable DX12 to avoid Windows crate version
conflicts with the host UI stack.

The GPU report emits an explicit backend matrix:

```text
bedrock_render_report case=surface_region_rgba_gpu_compare storage=generic backend=cpu ...
bedrock_render_report case=surface_region_rgba_gpu_compare storage=generic backend=auto ...
bedrock_render_report case=surface_region_rgba_gpu_compare storage=generic backend=dx11 ...
bedrock_render_report case=surface_region_rgba_gpu_compare storage=generic backend=vulkan ...
```

Interpretation rules:

- `backend=cpu` is the baseline and should not report `gpu_tiles > 0`.
- `backend=auto` should try DX11 first on Windows; if DX11 is unavailable,
  Vulkan or CPU fallback is acceptable but must be recorded in `gpu_fallback`.
- `backend=dx11` should report `gpu_actual=Dx11` and `gpu_tiles > 0`; fallback
  means the host is not a valid DX11 performance baseline.
- `backend=vulkan` should report `gpu_actual=Vulkan` and `gpu_tiles > 0`;
  fallback means the host is not a valid Vulkan performance baseline.
- Compare GPU numbers only on the same machine, driver, world fixture, and
  feature set.
- Read `elapsed_ms` together with `db_read_ms`, `decode_ms`, `gpu_upload_ms`,
  `gpu_dispatch_ms`, and `gpu_readback_ms`. When DB/decode dominates, backend
  differences do not identify the true bottleneck.
- The current DX11/Vulkan path still reads RGBA back into the existing
  image/cache pipeline, so `gpu_readback_ms` is a first-class metric. Future
  zero-copy texture presentation needs a separate baseline.

For interactive work, compare both one-shot batch rendering and session reuse.
The session path should avoid repeated world open/cache construction and should
produce the first `Ready` event before the full batch completes.
When collecting local numbers, record:

- planned tiles and visible render chunks
- cache hits and misses
- render worker count, world decode worker count, and CPU utilization
- `resolved_backend`, selected GPU backend/adapter/device type, skip/fallback
  reasons, and supported/skipped/fallback tile counts
- `gpu_requested_backend`, `gpu_actual_backend`, `gpu_adapter_name`,
  `gpu_device_name`, `gpu_fallback_reason`, `gpu_uploaded_bytes`,
  `gpu_readback_bytes`, `gpu_peak_in_flight`, and `gpu_queue_wait_ms`
- `world_load_ms`, `db_read_ms`, `decode_ms`, `region_copy_ms`,
  `gpu_prepare_ms`, `gpu_upload_ms`, `gpu_dispatch_ms`, `gpu_readback_ms`,
  `cpu_queue_wait_ms`, and `gpu_buffer_reuses`
- placement diagnostics: `region_chunks_copied`,
  `region_chunks_out_of_bounds`, and `tile_missing_region_samples`
- old-world counters from `bedrock-world` when applicable:
  `legacy_terrain_records`, `legacy_pocket_chunks`, `detected_format`, and
  `render_prefix_scans`/`RenderLoadStats::prefix_scans`
- canonical-height counters from `bedrock-world`:
  `computed_surface_columns`, `raw_height_mismatch_columns`,
  `missing_subchunk_columns`, and `legacy_fallback_columns`
- time to first tile and time to complete

## Full Export Suite

Full web-map export benchmarks are intentionally opt-in because each sample can
take multiple seconds and can be dominated by disk throughput. Set
`RUN_FULL_BENCHMARKS` to `true` in `benches/render.rs` first:

```powershell
cargo bench --bench render --all-features
```

## Latest Local Baseline

Measured on 2026-05-07 on Windows in release Criterion mode with default
features and fixture
`C:\Users\Administrator\Desktop\BE-Community-Dev\bedrock-world\tests\fixtures\sample-bedrock-world`.
This baseline uses typed `BedrockWorld<BedrockLevelDbStorage>` construction for
the Criterion cases, the exact-batch world load path for baked surface tiles,
and `MapWorldEditor` for v0.2.0 editor facade reports.

Machine-readable one-shot report:

```text
bedrock_render_report case=surface_region_rgba storage=generic backend=default elapsed_ms=582 tiles=1 worker_threads=1 world_worker_threads=1 prefix_scans=0 exact_get_batches=1 exact_keys_requested=7424 exact_keys_found=2558 db_read_ms=256 decode_ms=234 gpu_tiles=0 cpu_tiles=1 gpu_requested=Auto gpu_actual=Auto gpu_adapter=none gpu_device=none gpu_fallback=none gpu_upload_ms=0 gpu_dispatch_ms=0 gpu_readback_ms=0 gpu_uploaded_bytes=0 gpu_readback_bytes=0 gpu_peak_in_flight=0 gpu_buffer_reuses=0
bedrock_render_report case=v02_overlay_query storage=editor elapsed_ms=14957 chunks=256 entities=0 block_entities=64 hsa=0 villages=182
bedrock_render_report case=v02_map_scan storage=editor elapsed_ms=188 records=1760
bedrock_render_report case=v02_global_scan storage=editor elapsed_ms=1149 records=0 scoreboard_found=false error=Bedrock_world_error:_NBT_error:_unknown_NBT_tag_type:_52
bedrock_render_report case=v02_hsa_scan storage=editor elapsed_ms=1797 chunks=842 areas=1703
bedrock_render_report case=v02_edit_invalidation storage=memory elapsed_ns=14100 affected_chunks=2 refresh_metadata=true refresh_overlays=true clear_tile_cache=true
bedrock_render_report case=surface_region_rgba storage=dynamic backend=default elapsed_ms=576 tiles=1 worker_threads=1 world_worker_threads=1 prefix_scans=0 exact_get_batches=1 exact_keys_requested=7424 exact_keys_found=2558 db_read_ms=264 decode_ms=235 gpu_tiles=0 cpu_tiles=1 gpu_requested=Auto gpu_actual=Auto gpu_adapter=none gpu_device=none gpu_fallback=none gpu_upload_ms=0 gpu_dispatch_ms=0 gpu_readback_ms=0 gpu_uploaded_bytes=0 gpu_readback_bytes=0 gpu_peak_in_flight=0 gpu_buffer_reuses=0
```

Criterion quick suite:

| Benchmark | Typical time |
| --- | ---: |
| `biome_tile_256_rgba` | 273.6 ms |
| `biome_tile_256_webp` | 257.5 ms |
| `fixed_y_tile_256_rgba` | 281.0 ms |
| `raw_biome_tile_256_rgba` | 264.8 ms |
| `surface_tile_256_rgba` | 406.7 ms |
| `interactive_ordered_surface_batch_2x2_rgba` | 1.0457-1.0833 s (quick smoke) |
| `bake_chunk_surface` | 31.3 ms |
| `render_tile_surface_from_bake` | 418.9 ms |
| `heightmap_tile_256_rgba` | 373.2 ms |
| `cave_slice_tile_256_rgba` | 270.9 ms |
| `tile_batch_auto_threads` | 2.32 s |
| `tile_batch_single_thread` | 1.25 s |

Previous local baseline, measured on 2026-05-01 before the computed-surface and
exact-batch changes:

| Benchmark | Typical time |
| --- | ---: |
| `biome_tile_256_rgba` | 22.8 ms |
| `biome_tile_256_webp` | 23.0 ms |
| `fixed_y_tile_256_rgba` | 29.9 ms |
| `raw_biome_tile_256_rgba` | 22.8 ms |
| `surface_tile_256_rgba` | 208.1 ms |
| `bake_chunk_surface` | 6.5 ms |
| `render_tile_surface_from_bake` | 212.3 ms |
| `heightmap_tile_256_rgba` | 38.9 ms |
| `cave_slice_tile_256_rgba` | 31.6 ms |
| `tile_batch_auto_threads` | 210.7 ms |
| `tile_batch_single_thread` | 716.5 ms |

The 2026-05-01 numbers predate the block-boundary shadow, bounded GPU in-flight
compose queue, and computed-surface `HeightMap` semantics. New baselines should
record whether `block_boundaries` is enabled, whether the workload uses
`HeightMap` or `RawHeightMap`, and which `RenderCpuPipelineOptions`,
`RenderTilePriority`, and `RenderGpuOptions` values were used.

The ordered interactive 2x2 smoke case was measured on 2026-07-13 with
`RenderExecutionProfile::Interactive`, `RenderThreadingOptions::Fixed(4)`, RGBA
output, disabled tile cache, and Criterion `--quick`. Its `1.0457-1.0833 s`
interval is a scheduling smoke result, not a historical baseline or a direct
comparison with the single-tile cases above.

Full export benchmarks from this audit were around 5 seconds per sample for the
sample web region and are kept outside the default suite.

After the session upgrade, regressions should be triaged by pipeline stage:
render-index scan, world load/DB read/decode, region copy, GPU prepare/upload/
dispatch/readback, encode, and cache write. GPU
numbers are only comparable when the same adapter and driver are used; always
include the fallback reason when GPU work falls back to CPU. If
`world_load_ms` or `decode_ms` dominates, raise render/world worker budgets
before blaming the shader. If `gpu_queue_wait_ms` dominates, lower
`max_in_flight` or increase CPU bake parallelism only after checking GPU
utilization. If `gpu_readback_ms` dominates, the current readback-based RGBA
path is the bottleneck and zero-copy texture presentation should be measured as
a separate follow-up.
