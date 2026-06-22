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
- sprite atlas 和 texture counts；
- backdrop blur primitive counts；
- GPU mesh resource counts；
- 支持时的 allocator totals；
- retained resource trim activity。

## Retained Resources

renderer 会跨帧保留 pipelines、shader modules、atlases、backdrop blur targets 和
mesh buffers 等 resources。trimming 应释放 idle resources，而不改变 application
state。

## Guidelines

- 大范围 refactor 前，先用 metrics 确认 performance problems。
- measurement code 保持 application-neutral。
- 普通 UI 优先使用 event-driven rendering。
- 只有需要的窗口才使用 continuous rendering。
- 添加 renderer features 时文档化新 metrics。
