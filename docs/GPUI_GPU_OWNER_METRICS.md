# GPU Owner Queue And Service Metrics

Nova Windows, Linux and FreeBSD windows share `gpui-gpu-owner`. Diagnostics
observe this existing owner without changing its command ordering, latest-wins
replacement, damage merging, readiness callbacks or idle scheduling. Native
macOS Metal rendering does not use this Nova owner.

## API And Identity

`gpui::gpu_owner_metrics_snapshot()` returns cached global dispatch totals and
recent distributions for live owner proxies. Each proxy has an `owner_id`;
its `window_id` is `None` until the first `PresentationPacket` binds the actual
`WindowId::as_u64()`. These identifiers belong to different namespaces.

`gpui::gpu_owner_samples_since(owner_id, after_job_id)` copies retained completed
commands after that execution cursor. At most 1,024 records are retained per
owner. A gap in job IDs identifies history eviction. Window destruction removes
its totals and history; global totals continue to include destroyed windows.
An execution ID is not a UI FrameId: retained ticks can submit the same committed
scene, and commands can defer or fail without submitting any frame.
Raw execution start/completion timestamps share the global observation origin,
not each window's registration time, so service intervals can be aligned across windows.

## Measurement Boundaries

| Metric | Meaning |
| --- | --- |
| `queue_wait.p50_us/p95_us/p99_us` | Latest producer enqueue to command execution start, including the producer queue lock. |
| `pending_age.p50_us/p95_us/p99_us` | First replaced command's enqueue to execution start. Preserves backlog age under repeated replacement. |
| `owner_job_duration.p99_us` | Command service wall time, including backend calls, explicit waits, reports and thread preemption. |
| `owner_per_window_service_time_us` | Cumulative service wall time of this live proxy, including controls, failed and deferred commands. |
| `owner_busy_ratio` | Completed global dispatch wall time divided by elapsed observation time. Excludes channel idle waits; includes initialization, preparation and destruction. |
| `owner_cpu_busy_ratio` | OS thread CPU time divided by elapsed observation time. Windows uses `GetThreadTimes`; Linux/FreeBSD use `CLOCK_THREAD_CPUTIME_ID`. Unsupported platforms and query errors return `None`. Windows accounting can be coarse. |
| `owner_blocking_wait_time_us` | Explicitly instrumented Nova submission-wait call wall time. A subset of service time, not all driver-internal blocking. |
| `presentation_deadline_miss_count` | Currently `None`. Existing cadence deadlines are earliest eligible times, not actual display deadlines. |
| `sample_age_ms` | Time since the last completed observation. Ongoing commands are not counted until they finish. Idle windows remain stale without a sampling timer. |

Percentiles use nearest rank over retained completed commands, not lifetime
samples. No applicable samples means `sample_count == 0`; accompanying zero
percentiles must not be interpreted as measured zero latency. Raw `kind` and
`outcome` distinguish controls, Draw/Tick/Continue, submitted, deferred and failed
work. Successful submission does not establish GPU completion or physical scanout.

Autonomous Linux/FreeBSD continuation work has no producer enqueue; its queue wait
and pending age are `None` and excluded from those distributions. Its optional
`schedule_lateness_us` measures lateness relative to cadence eligibility only.
Native ticks and readiness continuations that enter the queue have normal queue
timestamps. A replaced request produces no separate execution sample; the final
sample carries `coalesced_count` and the earliest pending timestamp.

The global CPU value is captured after completed dispatches. The snapshot uses
the current elapsed time, so prolonged idle reduces lifetime busy ratios. A slow
unfinished job will appear in totals after completion; `sample_age_ms` exposes
that observation gap. These cumulative ratios are not instantaneous utilization.

## Interpreting Multi-Window Evidence

`gpui_perf_lab` JSON schema 5 exports `gpu_owner` and `gpu_owner_samples` alongside
the existing frame report. These are separate diagnostic reads; raw jobs are capped
at each snapshot's completed-job cursor, but history eviction or window teardown
between reads can leave fewer records. The lab currently opens one window, so its
report validates collection and does not establish multi-window contention.

Schema 5 also exports process-wide `text_metrics`; these have no WindowId and must
not be joined to the last owner job as if they were that window's measurements.

Collect snapshots and raw jobs for the main window, map viewer and auxiliary
window in one workload. Compare queue wait and pending age against other windows'
service intervals, and separate resource-heavy Draw jobs from controls and retained
ticks. Record backend, adapter, display cadence, workload and observation duration.

High queue latency with long owner service intervals establishes CPU-side owner
occupancy. Use the OS CPU ratio and explicit submission waits to distinguish CPU
work from known waits; remaining wall time can include driver calls or preemption.
Do not attribute it entirely to Scene packing without further profiling.

This change does not add GPU Timestamp Queries, actual display deadlines, extra
GPU owners or CPU scene workers. GPU pass timing and display timing remain separate
measurements. Thread affinity, resource registries, quality and damage semantics
remain governed by [GPUI_VENDOR_RENDERING.md](GPUI_VENDOR_RENDERING.md).
