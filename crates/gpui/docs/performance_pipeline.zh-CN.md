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
- GPU mesh resource counts；
- 支持时的 allocator totals；
- retained resource trim activity。

## Retained Resources

renderer 会跨帧保留 pipelines、shader modules、atlases、backdrop blur targets 和
mesh buffers 等 resources。trimming 应释放 idle resources，而不改变 application
state。

## 可复现基准

`benches/` 下的 Criterion suites 是 CPU 侧微基准的事实源。使用显式 benchmark
feature 并以 release 模式运行：

```powershell
rtk cargo bench --manifest-path crates/gpui/Cargo.toml --features bench
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

## Guidelines

- 大范围 refactor 前，先用 metrics 确认 performance problems。
- measurement code 保持 application-neutral。
- 普通 UI 优先使用 event-driven rendering。
- 只有需要的窗口才使用 continuous rendering。
- 添加 renderer features 时文档化新 metrics。
- benchmark fixture 生成和文件 IO 必须放在计时迭代之外。
- 受控 benchmark 没有可测收益时，不合入仅用于“优化”的复杂实现。
