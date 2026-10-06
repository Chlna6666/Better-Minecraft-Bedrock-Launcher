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
- allocator totals where supported;
- retained resource trim activity.

## Retained Resources

The renderer keeps resources such as pipelines, shader modules, atlases, and
backdrop blur targets across frames. GPUI no longer owns 3D mesh resources;
extensions own their GPU resources and lifetime. Trimming should release idle
GPUI resources without changing application state.

## Reproducible Benchmarks

The Criterion suites under `benches/` are the source of truth for CPU-side
microbenchmarks. Run them in release mode with the explicit benchmark feature:

```powershell
rtk cargo bench --manifest-path crates/gpui/Cargo.toml --features bench-support
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

## Bitmap Pool Working-Set Benchmarks

The local runner preserves raw logs, exit codes, commands, environment metadata,
and source hashes under `target/diagnostics/gpui-cpu-bench/<Baseline>/`:

```powershell
rtk proxy pwsh -NoProfile -File scripts/benchmark_gpui_cpu.ps1 -Suite Memory -Baseline before -WarmupSeconds 3 -MeasurementSeconds 5
```

Use the same benchmark source, machine, features, and power mode for the
candidate run. When restoring source snapshots for A/B runs, ensure their
modification times trigger recompilation. Verify the executed binary's hash
and a distinguishing fixture result as well as source hashes; a restored
source file alone does not prove that Cargo rebuilt its cached binary.
The Memory suite has 13 timing workloads: three steady-state
cases, their cold-after-trim and trim-reclaim variants, a complete
large/small/trim/small sequence, and three switches without trimming
(`large_to_tiny`, `large_to_half`, `large_tiny_alternating`). The first two
no-trim cases also report capacities for the first small request, warm reuse,
and return to large requests outside timing. Cold and trim cases use
Criterion `PerIteration` setup outside the measured operation. The benchmark
binary uses Rust's System allocator and serializes its process-global pool;
it does not use BMCBL's mimalloc configuration or create a window/GPU device.

The pool considers the smallest available capacity in the same reuse class,
limited to twice the **rounded request bucket**. Bucket rounding means this is
not a strict twofold limit relative to the raw request. Classes increase with
capacity, so an incompatible smallest candidate makes scanning larger entries
unnecessary. Skipped buffers remain idle for larger requests. Existing release
budgets and event-driven trimming retain their semantics. Allocation on a miss
occurs after releasing the pool lock; another thread may return a buffer after
that miss, consistent with best-effort reuse. The idle budget does not cap
active image allocations.

Acquired Vec capacity, idle retained capacity, and allocator retention are
distinct measurements. CPU heap-fragmentation or process-memory claims require
allocator statistics and OS PrivateUsage/RSS samples; GPU-fragmentation claims
require backend reserved/allocated measurements. Clearing a pool or shrinking
a Vec does not prove lower OS memory usage. CPU-only benchmark results do not
prove window FPS or presentation latency.

## Guidelines

- Use metrics to confirm performance problems before broad refactors.
- Keep measurement code application-neutral.
- Prefer event-driven rendering for ordinary UI.
- Use continuous rendering only for windows that need it.
- Document new metrics when adding renderer features.
- Keep benchmark fixture generation and file I/O outside measured iterations.
- Do not merge optimization-only complexity when the controlled benchmark has
  no measurable benefit.
