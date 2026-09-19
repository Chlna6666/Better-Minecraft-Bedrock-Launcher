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

## CPU SIMD 执行矩阵

Criterion 快速套件测量的是完整 render pipeline，不能把整张地图耗时直接标成 SIMD
收益。当前生产 compose 直接写出请求的 `Rgba8` 或 `Bgra8`，缓存也按像素格式保存
原生字节；无邻域光照的连续已解析颜色会进入 `fearless_simd` pack kernel，复杂
邻域光照的邻域读取和浮点计算仍保持 scalar；形成连续最终颜色后仍可使用 SIMD
完成打包。

| 阶段 | 当前状态 | SIMD 判断 |
| --- | --- | --- |
| RGBA/BGRA 连续打包 | `pixels.rs::pack_colors` | `Auto` 使用 SIMD；`Scalar` 是完整 pipeline 基线 |
| 有邻域光照的 RGBA/BGRA 写入 | `compose_region_tile_from_prepared` 或 lighting-only compose | 邻域计算 scalar；最终连续颜色在 `Auto` 下统一进入 pack kernel |
| RGBA/BGRA 缓存 | `FastRgbaZstd` / `FastBgraZstd` | 原生格式命中；格式不匹配就是 miss |
| RGB brightness/shade、tint、blend | semantic sample 阶段 | 只有形成连续 buffer 后再做 A/B |
| heightmap、boundary/shadow mask | lookup/邻域读取仍在前面 | 暂不强行 SIMD |
| block-state、palette、HashMap、LevelDB | 控制流和随机访问 | 不适合 SIMD |

### SIMD A/B 规则

`<64 B` 使用 scalar；`64–256 B` 必须 benchmark 后决定；`>256 B` 才是候选。任何
候选都必须在相同输入、相同输出格式和 release 构建下比较 scalar、可用指令集和自动
选择，并记录 CPU、编译参数、tile 尺寸和中位数。`--no-default-features` 不是“关闭
SIMD”的开关；完整 pipeline 的无 SIMD 基线需要单独的受控编译配置。

2026-09-20 release pack A/B 记录：RGBA scalar/SSE2/SSE4.2/AVX2/auto 约为
40.2/44.2/49.4/53.8/40.8 µs，BGRA 约为 101.4/56.1/288.0/290.4/44.3 µs。
pack kernel 已接入所有 CPU compose 分支的最终连续颜色打包；邻域读取和光照算术仍保持
scalar，`Scalar` 继续保留直接写出基线。

```powershell
cargo build --release -p bedrock-render
cargo test --release -p bedrock-render --lib benchmark_packed_color_instruction_sets -- --ignored --nocapture
```

完整 pipeline 的 scalar/Auto 对照使用 `RenderOptions::simd`，当前覆盖 biome
RGBA、biome BGRA、lighting-only surface RGBA 和 heightmap：

```powershell
cargo bench -p bedrock-render --bench render --all-features -- "biome_tile_256"
cargo bench -p bedrock-render --bench render --all-features -- "surface_tile_256_rgba"
cargo bench -p bedrock-render --bench render --all-features -- "surface_tile_256_rgba_flat"
cargo bench -p bedrock-render --bench render --all-features -- "surface_tile_256_rgba_lighting_only"
cargo bench -p bedrock-render --bench render --all-features -- "heightmap_tile_256_rgba"
```

输出中的 `scalar` 是“无显式 SIMD kernel”的对照，`sse2`、`sse4.2`、`avx2` 是强制
指令集 kernel，`auto` 是 `fearless_simd` 的运行时选择。它们只比较连续 color pack
候选；完整 pipeline 的结果必须查看对应的 `*_scalar` 与 `*_auto` case。没有全局进程级
“关闭 CPU 指令集”开关，单次渲染使用 `RenderOptions::simd = Scalar` 做基线；也不要把
`--no-default-features` 当成 SIMD off。

这里的 Scalar 表示不调用显式 `fearless_simd` kernel；Rust/LLVM 仍可能对普通标量循环
做编译器自动向量化，因此它是工程 A/B 基线，不是禁用 CPU ISA 的硬件隔离实验。

`auto` 会在当前平台选择可用的 NEON、WASM SIMD 或 x86 指令集；Windows x86 的
显式 A/B 目前列出 SSE2、SSE4.2 和 AVX2。新增其它 kernel 前必须先有同一输入的
scalar 对照和 release 中位数。

不要新增 `rgba_to_bgra`、`bgra_to_rgba` 或等价隐式转换 helper。若缓存的
`pixel_format` 与请求不同，必须记录 cache miss 并按请求格式重新 compose。

多线程调度先批量 probe cache，再只把 miss 交给渲染队列；worker 数量会按实际 tile
数量裁剪，命中项不会创建无效的 compose 任务。loader、bake、compose 阶段分别受
`RenderCpuPipelineOptions` 上限和 pipeline queue depth 约束。比较
`tile_batch_auto_threads` 与 `tile_batch_single_thread` 时，同时记录 cache hit/miss、
`worker_threads`、`world_worker_threads` 和各阶段耗时，不能只看总耗时。

### 调度与内存审计（2026-09-20）

已经确认的调度开销：每个 region wave 会创建一个本地 Rayon pool；wave 内有可用
tile 时，`render_web_tile_indexes` 还会创建 compose pool。前一个 pool 的 region
worker 仍在运行，因此两个 pool 以及 region 内的 Bedrock world worker 可能同时存在。
本次已把 compose pool 限制为 `worker_count - region_worker_count`（至少 1），避免
两个 render pool 都按完整 worker budget 建线程；两个 pool 仍会重叠，所以仍需测量
线程唤醒、上下文切换和 cache 竞争。`RenderPipelineStats::peak_worker_threads` 当前
是配置上限，不是系统实际活跃线程数，不能把它当成 oversubscription 已被解决的证据。

已经确认的内存放大点：

- `render_tiles_from_shared_bakes` 把 `ChunkBake` 深拷贝进每个 `TileComposeTask`；
  原始 bake map 与排队任务会同时存活。
- `RegionPlan` 的 chunk position 列表会同时出现在全局 region map 和每个 tile 的
  readiness plan 中。
- prepared compose 会同时保留颜色、邻域高度、水深和最终 RGBA buffer；pack 阶段
  还可能有一份 `u32` 颜色 staging。
- tile cache writer 仍按至少 64 个 work item 初始化，单 tile 请求可能无谓启动多个
  编码线程和一个提交线程；由于 writer 在 session 内复用，不能只把这个下限删除，
  下一步应先加入按 session 最大请求量安全扩容/缩容的测量后再改。
- 显式增大 `pipeline_depth`/`queue_depth` 会按 item 数量放大这些对象，当前内存预算
  只约束 region wave 选择，不覆盖所有队列和临时 buffer。
- region bake memory LRU 现在会在命中时刷新顺序，并在重复 key 插入前去重，避免
  `memory_order` 因重复 key 无界增长。

下一步应先用同一 world、相同 cache 状态测量 pool 创建时间、channel wait、实际活动
线程和 RSS/allocator peak，再决定是否把 pool 提升为 session 级复用、把 `ChunkBake`
改成共享所有权，或为每个 worker 引入 scratch buffer。没有这些数据前，不应直接把
`worker_count` 调大或把 queue depth 设成 128；那只会增加调度和内存压力。

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

### 真实世界只读对照

`examples/compare_backends.rs` 从指定世界的主世界选择区块最多的 16×16 区域，
只读打开 LevelDB，并绕过 tile cache。它关闭邻域光照来隔离连续 resolved-color
pack kernel，输出 `cpu-scalar`、`cpu-auto`、DX11、Vulkan 的总耗时、
RGBA 哈希、实际 GPU backend 和 upload/dispatch/readback 耗时。GPU 不可用时报告错误，
不会把 CPU fallback 当成 GPU 结果。

```powershell
cargo run --release -p bedrock-render --example compare_backends --features "gpu-dx11 gpu-vulkan" -- "<world-path>"
```

DX11/Vulkan shader 当前只复制 CPU 已生成的 RGBA，因此整图 GPU 数据只衡量上传、
复制与读回的开销；它不代表 tint、shade、blend 在 GPU 上执行。当前 batch 参数
只影响 GPU 进入条件和调度，实际仍按 tile 提交 copy kernel，并不是一次多 tile dispatch。
当前 GPU compose 只接受原生 `Rgba8`；请求 `Bgra8` 时保持 CPU 原生 BGRA 路径，
不会为了进入 GPU 额外做通道转换。

### Windows CPU/GPU 混合渲染边界

当前 DX11 和 Vulkan kernel 仍是 RGBA copy kernel：调用顺序是
`CPU semantic/bake/lighting -> GPU upload/copy -> CPU readback`。它可以验证
设备、队列、上传和读回统计，但不能加速 terrain lighting、biome tint、boundary
或 shadow。因此 `gpu_tiles > 0` 不能单独证明 GPU 做了有效渲染工作。

Windows 上可实施的混合路径应保持 LevelDB、block-state、palette、chunk traversal
和 cache 在 CPU；CPU 先生成紧凑的 `PreparedTileCompose`，GPU 再处理连续数值阶段：

```text
CPU semantic/bake
    -> colors + 9-neighbor heights + water mask
    -> DX11/Vulkan terrain-lighting compute
    -> optional shadow/boundary mask
    -> one batch readback
    -> CPU encode/cache or GPUI upload
```

首个 kernel 应限制为 `SurfaceBlocks`/`HeightMap` 的纯 height lighting，暂时排除
atlas material、block-volume 和动态材质 cast shadow。`terrain_lit_color` 的 Sobel、
法线、sqrt、光照因子和 RGB shade 是连续算术，适合 GPU；palette lookup、材质分支和
动态 ray/max shadow 仍由 CPU 保留。`PreparedTileCompose` 当前每个 256×256 tile
约有 256 KiB colors、2.25 MiB 的 9-height `i32` 数据和 256 KiB water mask，应该
先改成紧凑 SoA/`i16` 输入，再评估上传成本。

现有 GPU backend 每个 tile 都创建 upload/output/readback resource，并同步等待
readback；真正的收益需要多 tile 合批、复用 staging/storage buffer，并让 CPU 准备
下一批时 GPU 处理上一批。`max_in_flight`、`pipeline_level` 和 staging pool 当前
还没有形成这条执行路径。DX11 compute 和 Vulkan WGSL 应先共享同一 golden tile，
逐像素比较 CPU Scalar 与 GPU 输出后再开启默认路径。当前 `Bgra8` 继续走 CPU 原生
输出，直到 GPU shader 明确支持 BGRA storage，否则不做通道转换。

Windows GPU 对比必须增加 `cpu-scalar`、`cpu-auto`、`gpu-copy`、`gpu-terrain` 和
`cpu-gpu-mixed` 五组，并同时记录 upload、dispatch、readback、实际 GPU tile 数、
像素 hash/误差和总耗时。单 tile 或 decode/bake 占主导的整图不能作为 GPU 算法收益
证明。macOS 不在这条路径的支持范围内。

2026-09-20 在 Radeon RX 7600M XT 和指定的 Bedrock level2 世界上，所选区域有
256/256 个 chunk，四条路径的 RGBA hash 都是 `467b5130af7dda59`：

| 路径 | 总耗时 | compose | GPU 实际后端 |
| --- | ---: | ---: | --- |
| `cpu-scalar` | 478 ms | 4 ms | — |
| `cpu-auto` | 498 ms | 4 ms | — |
| `dx11` | 443 ms | 8 ms | `Dx11` / Radeon RX 7600M XT |
| `vulkan` | 476 ms | 15 ms | `Vulkan` / Radeon RX 7600M XT |

这次运行表明同一输入下 Auto 与 Scalar 结果一致；该单次整图样本中 Auto 比 Scalar
慢 14 ms，不能宣称 SIMD 已让整图变快，因为 decode/bake 占主要时间且计时噪声大。
应以 release Criterion 的多次中位数决定是否扩大 kernel。缓存格式和 GPU 输出格式
应在同一请求中保持一致，不能通过额外的 RGBA/BGRA 转换修补结果。

GPU 对比是 opt-in 的，因为它依赖本机驱动、适配器和后台负载。运行前把
`benches/render.rs` 中的 `RUN_GPU_COMPARISON_REPORTS` 改为 `true`；Windows 专业对比
应该至少覆盖 `CPU`、`Auto`、`DX11` 和 `Vulkan` 四个路径：

> 该历史 Criterion 报告直接使用 `MapRenderer::new`，没有初始化 GPU context；
> `backend=dx11/vulkan` 标签可能仍对应 `gpu_tiles=0`。真实 GPU 对比请使用上面的
> `compare_backends` 示例，并确认 `gpu_tiles > 0` 和 `gpu_actual`。

```powershell
cargo bench -p bedrock-render --bench render --features "gpu-dx11 gpu-vulkan" -- --noplot __machine_report_only__
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
Criterion case 使用 typed `World<BedrockLevelDbStorage>` 构造；baked surface
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

## CPU SIMD Execution Matrix

The Criterion render suite measures the complete pipeline; its tile time must
not be reported as a SIMD speedup without a matching scalar case. Production
compose writes the requested `Rgba8` or `Bgra8` order directly. Every CPU
compose branch now sends its contiguous final colors through the safe
`fearless_simd` pack kernel under `Auto`; semantic lookup, neighborhood reads,
and floating-point shading stay scalar.
The cache stores native order in separate `FastRgbaZstd` or `FastBgraZstd` entries.
A pixel-format mismatch is a cache miss, not a conversion pass.

Keep small writes scalar (`<64 B`). Benchmark the `64–256 B` range before
choosing a kernel; larger contiguous buffers are SIMD candidates. Compare
scalar, each available instruction set, and runtime auto-dispatch with the
same release build, input, output order, tile size, and CPU. The pack kernel has
production `Auto` and controlled `Scalar` paths. The isolated instruction-set
benchmark remains useful for deciding whether each target should keep runtime
dispatch:

```powershell
cargo build --release -p bedrock-render
cargo test --release -p bedrock-render --lib benchmark_packed_color_instruction_sets -- --ignored --nocapture
```

The 2026-09-20 release pack A/B measured RGBA scalar/SSE2/SSE4.2/AVX2/auto at
40.2/44.2/49.4/53.8/40.8 µs and BGRA at 101.4/56.1/288.0/290.4/44.3 µs.

The full-pipeline A/B cases use `RenderOptions::simd` and currently cover biome
RGBA, biome BGRA, lighting-only surface RGBA, and heightmap:

```powershell
cargo bench -p bedrock-render --bench render --all-features -- "biome_tile_256"
cargo bench -p bedrock-render --bench render --all-features -- "surface_tile_256_rgba"
cargo bench -p bedrock-render --bench render --all-features -- "surface_tile_256_rgba_flat"
cargo bench -p bedrock-render --bench render --all-features -- "surface_tile_256_rgba_lighting_only"
cargo bench -p bedrock-render --bench render --all-features -- "heightmap_tile_256_rgba"
```

The benchmark labels are `scalar`, `sse2`, `sse4.2`, `avx2`, and `auto`.
`--no-default-features` does not disable CPU instruction sets, and there is no
process-wide SIMD-off switch. Use `RenderOptions::simd = Scalar` for a per-render
baseline. Scalar means no explicit `fearless_simd` kernel; LLVM may still
auto-vectorize ordinary scalar loops, so this is an engineering A/B control,
not a hardware ISA isolation experiment. Full-pipeline claims must use the matching
`*_scalar` and Auto cases. Do not add RGBA/BGRA conversion helpers; cache
entries with the wrong `pixel_format` must be recomposed in the requested order.

`auto` selects available NEON, WASM SIMD, or x86 instructions on the current target;
the explicit Windows x86 A/B cases currently cover SSE2, SSE4.2, and AVX2. Add another
kernel only with a matching scalar control and release-mode median.

For multi-threaded scheduling, cache probing happens before the render queue and
only misses enter compose. Worker counts are clamped to the number of tiles, so
cache hits do not create empty compose work. Loader, bake, and compose stages
also honor their `RenderCpuPipelineOptions` limits and pipeline queue depth.
Compare `tile_batch_auto_threads` with `tile_batch_single_thread` together with
cache hit/miss counts, `worker_threads`, `world_worker_threads`, and per-stage
timings rather than total elapsed time alone.

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
- `v02_map_scan` records Bedrock map item scan time and count.
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

For the same real world and the full CPU scalar/SIMD comparison, use the
read-only `compare_backends` example. It disables neighborhood lighting so the
contiguous resolved-color pack kernel is exercised, then emits `cpu-scalar`,
`cpu-auto`, `dx11`, and `vulkan`; the GPU rows require an actual GPU backend and
never silently convert a CPU fallback into a GPU result.
The current GPU compose contract accepts native `Rgba8` only; `Bgra8` stays on
the native CPU path until its shader/layout contract is verified, with no channel
conversion helper added.

### Windows CPU/GPU Hybrid Rendering Boundary

The current DX11 and Vulkan kernels are still RGBA copy kernels. The pipeline is
`CPU semantic/bake/lighting -> GPU upload/copy -> CPU readback`, so it measures
device, transfer, and synchronization costs but does not accelerate terrain
lighting, biome tint, boundary, or shadow. `gpu_tiles > 0` alone is not evidence
that useful rendering work ran on the GPU.

The first useful Windows hybrid kernel should keep LevelDB, block-state, palette,
chunk traversal, and cache work on the CPU. CPU prepares compact
`PreparedTileCompose` colors, nine-neighbor heights, and water masks; DX11/Vulkan
then computes height lighting and later shadow/boundary masks in a batched dispatch.
Atlas material branches, block-volume logic, and dynamic ray/max shadows should
remain on CPU until a matching GPU data layout and golden-tile comparison exist.

The current per-tile resource creation and synchronous readback must be replaced by
batched dispatch plus reusable staging/storage buffers before expecting a speedup.
`max_in_flight`, `pipeline_level`, and staging pool settings do not yet implement
that execution path. Native `Bgra8` remains CPU output until the GPU shader supports
BGRA storage directly. Windows validation should compare `cpu-scalar`, `cpu-auto`,
`gpu-copy`, `gpu-terrain`, and `cpu-gpu-mixed`, including transfer timings and
pixel equality/error; macOS is outside this path.

```powershell
cargo run --release -p bedrock-render --example compare_backends --features "gpu-dx11 gpu-vulkan" -- "<world-path>"
```

The 2026-09-20 run on the supplied `Bedrock level2` world selected 256/256
Overworld chunks on a Radeon RX 7600M XT. All four rows produced the same RGBA
hash, `467b5130af7dda59`:

| Path | Total | Compose | Actual GPU |
| --- | ---: | ---: | --- |
| `cpu-scalar` | 478 ms | 4 ms | — |
| `cpu-auto` | 498 ms | 4 ms | — |
| `dx11` | 443 ms | 8 ms | `Dx11` / Radeon RX 7600M XT |
| `vulkan` | 476 ms | 15 ms | `Vulkan` / Radeon RX 7600M XT |

The example uses lighting off to exercise the contiguous resolved-color pack
kernel. This single full-world sample did not show an Auto win: Auto was 20 ms
slower than Scalar while decode and bake dominated. Use release Criterion
medians before widening the SIMD kernel.

```powershell
cargo bench -p bedrock-render --bench render --features "gpu-dx11 gpu-vulkan" -- --noplot __machine_report_only__
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
This baseline uses typed `World<BedrockLevelDbStorage>` construction for
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
