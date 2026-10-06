# Performance Pipeline

[English](performance_pipeline.md)

GPUI 会记录 renderer 和 UI metrics，使 frame pacing、resource growth、image caches
和 retained GPU resources 可以被诊断，而不需要向 framework internals 加入
application-specific instrumentation。

## Metrics Areas

Performance metrics 覆盖：

- selected renderer backend；
- frame timing 和 draw time；
- image cache items、bytes 和 evictions；
- queued animation bytes 与全进程 limit；
- sprite atlas 和 texture counts；
- backdrop blur primitive counts；
- 支持时的 allocator totals；
- retained resource trim activity。

## Retained Resources

renderer 会跨帧保留 pipelines、shader modules、atlases 和 backdrop blur targets 等
resources。GPUI 不再拥有 3D mesh 资源；extension 自行管理其 GPU 资源和生命周期。
trimming 应释放 GPUI 的 idle resources，而不改变 application state。

## 可复现基准

`benches/` 下的 Criterion suites 是 CPU 侧微基准的事实源。使用显式 benchmark
feature 并以 release 模式运行：

```powershell
rtk cargo bench --manifest-path crates/gpui/Cargo.toml --features bench-support
```

首份已记录的 Windows CPU 基线见
[`performance_baseline_2026-08-24.md`](performance_baseline_2026-08-24.md)。

当前 suites 覆盖 retained Nova frame-upload encoding、cold/retained layout、path tessellation、原尺寸 PNG/WebP render、
指定尺寸 PNG/WebP render、resident/有界 streaming WebP 动图处理以及损坏容器拒绝路径。fixture 是确定性的，并在
被测操作开始前生成；此外覆盖 uniform/mixed allocation size 下的稳态 bitmap-pool
复用以及 bucket 边界附近的 dense large-buffer 请求。结果同时报告工作量，使输入规模变化后仍可比较吞吐量。

任何性能结论必须记录：

- GPUI revision，以及工作区是否存在未提交变更；
- 操作系统、CPU、逻辑核心数、内存、GPU、renderer backend 与电源模式；
- Rust toolchain、Cargo profile、features 和 allocator；
- benchmark 名称、输入尺寸/数量、sample size 与 Criterion estimate；
- 同一台机器、同一次会话中的 baseline 和 candidate 结果；
- 内存敏感改动除耗时外，还要记录 peak resident memory。

比较分布和 confidence interval，不能用单次运行或复制自其他机器的绝对耗时作结论。
只有相关 benchmark 改善且相邻 case 没有实质性退化，才能认定为 CPU 优化。内存优化还
必须用有代表性的长时间负载证明 steady state 有界。交互式 examples 只用于视觉验证，
不能作为 benchmark 证据。

完整帧和 renderer 测量属于另一层基准。预热后至少采集 300 帧，报告 median/p95 frame
time、missed-frame count、uploaded bytes、atlas/cache residency 和实际 renderer。
baseline 与 candidate 必须使用相同窗口尺寸、scale factor、内容、动画状态以及前后台
状态。

### CPU 内存与指令缓存基准

在仓库根目录使用 PowerShell 7 运行：

```powershell
rtk proxy pwsh -NoProfile -File scripts/benchmark_gpui_cpu.ps1 -Suite All -Baseline before
# 修改后使用同一机器、features 和电源模式再次运行
rtk proxy pwsh -NoProfile -File scripts/benchmark_gpui_cpu.ps1 -Suite All -Baseline after
```

脚本默认将日志、退出码、运行命令和环境元数据写入
`target/diagnostics/gpui-cpu-bench/<Baseline>/`。元数据包含 revision、dirty 文件列表、
受测源码 SHA256、Rust toolchain、CPU、电源方案、features 和 allocator。失败会返回
非成功状态，原始日志不会被 RTK 摘要过滤。`-Suite Memory` 或 `-Suite PathMask`
可以独立运行一类测试；这些 CPU-only 命令不创建窗口或 GPU device。

Memory 使用 Criterion 的 30 个样本，默认预热 1 秒、测量 2 秒；这是快速本地比较，
稳定性结论应延长测量并重复运行。池是 process-global，只能在独立的 benchmark
进程内串行测量。该 GPUI benchmark binary 使用 Rust System allocator；BMCBL
主程序的 mimalloc 配置不能用于解释它的分配结果。

Memory 保留原有三个稳态工作负载，并补充各自的 `cold_after_trim`、
`trim_reclaim`，以及 `large_small_trim_small` 工作集回落。冷请求和整理使用
Criterion `PerIteration`：全局池准备紧邻被测操作，但不计入该操作耗时；工作集回落
则测量名称所列的完整四步序列。容量 JSON 在计时之外报告 requested/acquired、
idle retained/free buffers 和外层 staging Vec。热循环复用 staging Vec；旧 harness
每轮构造临时 Vec，因此新旧稳态耗时不能全部归因于生产 bitmap pool 的性能变化。

`working_set_switch` 另外覆盖不整理池时的 `large_to_tiny`、`large_to_half` 和
`large_tiny_alternating`，当前 Memory 共 13 个耗时工作负载。前两个场景输出首次小请求、
热复用和返回大请求的容量；交替场景测量预热后的完整大／小请求序列。记录小请求的
acquired capacity 与池的 retained capacity，可区分活跃图片容量放大和闲置复用预算。
比较修改前后策略时使用相同 benchmark 源码，并建议显式延长采样，例如
`-WarmupSeconds 3 -MeasurementSeconds 5`。

从快照恢复源码做 A/B 时，须确保修改时间触发重新编译；同时核对实际执行的
binary hash 和能区分两种策略的 fixture 输出。仅有恢复后源码的 hash，不能证明
Cargo 已重建缓存中的 binary。

Bitmap pool 的取得策略只考虑最小可用容量，要求与请求 bucket 同属一个复用分类，
且不超过该 **rounded bucket 的两倍**。分类随容量单调增加，最小候选不兼容时无需
扫描更大的分类。上限不等于原始请求大小的严格两倍：bucket 舍入本身也会放大容量。
跳过的大缓冲继续保留供大请求复用；既有 release 预算和事件驱动 trim 保持原语义。
未命中时的堆分配在释放池锁之后执行；复用比例和分类仅在存在候选时检查。并发时
其它线程可能在该次 miss 后归还缓冲，这是池的尽力复用语义，活动图片分配仍不由
闲置池预算硬限制。

PathMask 使用实际的私有步骤缓存 helper，以预建 descriptor 为输入，覆盖 32/1024
条指令、单槽命中、双槽轮换、三槽压力场景和逐次 scene revision 失效。当前 Nova
生产路径使用两个 frame resource slot。显式执行的 ignored release test 输出每个
case 的 30 批原始耗时（每批 10,000 次）、重建/命中数及 descriptor 容量字节。
批耗时除以迭代数得到批平均操作成本；其分位数不是逐帧延迟分位数。Fixture 不含
真实 shader、资源上传、GPU 提交或呈现，不能由它推出窗口 FPS。

指令 Vec 容量、bitmap-pool idle capacity 与 allocator 的实际保留是不同指标。
CPU heap 碎片及进程内存结论还需要 allocator 与 OS PrivateUsage/RSS 采样；GPU
碎片需要另外采集 backend allocator reserved/allocated。不要将清空池或收缩 Vec
直接描述为操作系统内存已经下降。

## Guidelines

- 大范围 refactor 前，先用 metrics 确认 performance problems。
- measurement code 保持 application-neutral。
- 普通 UI 优先使用 event-driven rendering。
- 只有需要的窗口才使用 continuous rendering。
- 添加 renderer features 时文档化新 metrics。
- benchmark fixture 生成和文件 IO 必须放在计时迭代之外。
- 受控 benchmark 没有可测收益时，不合入仅用于“优化”的复杂实现。
