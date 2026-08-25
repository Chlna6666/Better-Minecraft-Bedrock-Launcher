# Performance Pipeline

[Chinese](performance_pipeline.zh-CN.md)

GPUI records renderer and UI metrics so frame pacing, resource growth, image
caches, and retained GPU resources can be diagnosed without adding
application-specific instrumentation to framework internals.

## Metrics Areas

Performance metrics cover:

- selected renderer backend;
- frame timing and draw time;
- image cache items, bytes, and evictions;
- queued animation bytes and their process-wide limit;
- sprite atlas and texture counts;
- backdrop blur primitive counts;
- GPU mesh resource counts;
- allocator totals where supported;
- retained resource trim activity.

## Retained Resources

The renderer keeps resources such as pipelines, shader modules, atlases,
backdrop blur targets, and mesh buffers across frames. Trimming should release
idle resources without changing application state.

## Reproducible Benchmarks

The Criterion suites under `benches/` are the source of truth for CPU-side
microbenchmarks. Run them in release mode with the explicit benchmark feature:

```powershell
rtk cargo bench --manifest-path crates/gpui/Cargo.toml --features bench
```

The first recorded Windows CPU baseline is
[`performance_baseline_2026-08-24.md`](performance_baseline_2026-08-24.md).

The current suites cover retained Nova frame-upload encoding, cold and retained layout, path tessellation, full-size
PNG/WebP rendering, size-constrained PNG/WebP rendering, resident and bounded-streaming animated
WebP processing, malformed-container rejection, plus steady-state bitmap-pool reuse under uniform and mixed
allocation sizes, including dense large-buffer requests around bucket boundaries. Benchmark fixtures are deterministic and are created before
the measured operation. Each result reports work units so throughput remains
comparable when input sizes change.

A performance claim must record:

- the GPUI revision and whether the working tree was dirty;
- OS, CPU, logical core count, memory, GPU, renderer backend, and power mode;
- Rust toolchain, Cargo profile, enabled features, and allocator;
- benchmark name, input dimensions/counts, sample size, and Criterion estimate;
- both the baseline and candidate results from the same machine and session;
- peak resident memory for memory-sensitive work, in addition to elapsed time.

Compare distributions and confidence intervals, not one invocation or an
absolute duration copied from another machine. A change is accepted as a CPU
optimization only when the relevant benchmark improves without a material
regression in adjacent cases. Memory work must also demonstrate a bounded
steady state with a representative long-running workload. Interactive examples
are useful for visual validation, but are not benchmark evidence.

Full-frame and renderer measurements are a separate layer. Capture at least
300 post-warmup frames, report median and p95 frame time, missed-frame count,
uploaded bytes, atlas/cache residency, and the selected renderer. Use the same
window size, scale factor, content, animation state, and foreground/background
state for baseline and candidate runs.

## Guidelines

- Use metrics to confirm performance problems before broad refactors.
- Keep measurement code application-neutral.
- Prefer event-driven rendering for ordinary UI.
- Use continuous rendering only for windows that need it.
- Document new metrics when adding renderer features.
- Keep benchmark fixture generation and file I/O outside measured iterations.
- Do not merge optimization-only complexity when the controlled benchmark has
  no measurable benefit.
