# Text Execution Metrics

`gpui::text_metrics_snapshot()` returns process-wide observations grouped by native
backend and operation. `gpui_perf_lab` schema 5 exports these as `text_metrics`,
separately from window/frame samples. The text platform interface has no WindowId;
these rows must not be attributed to whichever window submitted last.
macOS text backends are not instrumented by this change.

## Measurement Contract

| Field | Meaning |
| --- | --- |
| `backend` | `cosmic` on Linux/FreeBSD or `direct-write` on Windows. Only observed rows are returned. |
| `operation` | `shape`, `raster-bounds`, or `rasterize`. Bounds preparation can also perform raster/cache work. |
| `calls` | Lifetime completed calls, including calls returning errors. |
| `lock_wait_total_us` | Lifetime wall time spent acquiring the instrumented state lock. |
| `method_total_us` | Lifetime wall time inside the native method after acquisition. |
| `lock_wait`, `method` | Nearest-rank p50/p95/p99/max and sample count over at most 256 recent calls per row. |

Times use the profiling monotonic clock, not the visual animation clock. They include
thread preemption and are not isolated CPU execution time. Values are truncated to
microseconds; a sub-microsecond measurement can be zero. Missing rows and a zero
sample count mean unobserved work, not a free backend. Lifetime totals saturate rather
than wrap; history eviction does not reset them.

Cosmic Text measures its existing state write lock for all three operations.
DirectWrite measures the state write lock for shaping and its state read lock for
bounds/rasterization. Its separate pending-glyph-analysis Mutex is outside these
measurements. Neither font-selection locks nor application resource locks are covered.

Recorders release the font state guard before acquiring the diagnostics Mutex.
Recording keeps a bounded ring; percentile sorting happens only during snapshot
collection, outside the recorder lock. This change does not split font/cache locks
or change font loading, fallback, glyph generation, or singleflight behavior.

## Interpreting Results

Use the existing text-layout cache hit/reuse/miss fields in
`performance_metrics_snapshot()` alongside these observations. A cache miss is not
necessarily a distinct native shape call because singleflight can merge requests.
Conversely, bounds preparation and rasterization are separate measured operations;
do not sum their call counts as unique generated glyphs.

Repeated high lock-wait percentiles are evidence to investigate concurrency. High
method time with low wait instead points toward native work, fallback, cache misses
or scheduling/preemption. Font-fallback counts, atlas-glyph misses and per-window
attribution are not provided by this API. The lock-release test verifies operation
completion and sample collection while a test-held lock is released. It does not
prove that the worker was already blocked, impose a timing threshold, or diagnose
real workload contention. Synthetic-duration tests verify the distributions.

Windows DirectWrite validation cannot establish Linux Cosmic Text runtime behavior.
Compare release-mode workloads and retain platform/backend identity when reporting
results; do not replace the state lock without workload evidence and consistency
tests for font loading and raster caches.
