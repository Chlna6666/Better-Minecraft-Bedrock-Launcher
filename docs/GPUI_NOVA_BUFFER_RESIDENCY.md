# Nova static Buffer residency baseline (P1-A)

P1-A establishes a device-specific baseline for static storage Buffer uploads and
repeated shader reads. It does not change GPUI's production `CpuToGpu` policy.
GPU-only defaults require the corresponding P0-C asynchronous upload path to be
validated first. Current DX12 and Vulkan GPU-only writes still synchronize.

## Native paths under comparison

| Backend | CPU-visible request | GPU-only request | Interpretation |
| --- | --- | --- | --- |
| DX11 | DEFAULT + UpdateSubresource | DEFAULT + UpdateSubresource | Equivalent native paths; no placement winner. |
| DX12 | UPLOAD heap + mapped write | DEFAULT heap + temporary staging copy + fence wait | Different heap policies; measure upload cost separately from reuse. |
| Vulkan | CpuToGpu allocator request + mapped write | GpuOnly allocator request + upload page/copy/queue_wait_idle | Different requests; actual memory types may overlap on UMA. |
| OpenGL | DYNAMIC_DRAW + glBufferSubData | STATIC_DRAW + glBufferSubData | Driver hints; neither establishes physical placement. |
| Metal | Shared Buffer | GPU-only upload unsupported | Native draw also remains incomplete; report unsupported, no fabricated timings. |

`DiagnosticsDevice::memory_architecture()` returns `Unified`, `Discrete` or
`Unknown`. DX12 queries node zero's `D3D12_FEATURE_ARCHITECTURE.UMA`; Metal uses
`MTLDevice.hasUnifiedMemory`. DX11, Vulkan and OpenGL currently return Unknown.
Adapter names, integrated device type, dedicated-memory sizes and local driver
budgets are not substitutes for an authoritative physical architecture query.
Architecture does not establish a preferred resource policy.

## Reproducible standalone workload

`buffer_residency_lab` is a GPUI example only to reuse existing Nova dependencies.
It directly owns a native device and does not instantiate `NovaRenderer`, change
application settings or override resource creation in production. Its experimental
SM5 shaders use the existing Windows FXC API through a dev-dependency feature;
production shaders remain precompiled.

```powershell
cargo build -p gpui --example buffer_residency_lab --no-default-features --features windows-manifest,mimalloc-collect,nova-gfx-dx11,nova-gfx-dx12,nova-gfx-vulkan,nova-gfx-opengl
target/debug/examples/buffer_residency_lab.exe --backend=nova-dx12 --memory=cpu-visible --bytes=8388608 --samples=80 --warmup=8 --draws=8 --update-every=0
target/debug/examples/buffer_residency_lab.exe --backend=nova-dx12 --memory=gpu-only --bytes=8388608 --samples=80 --warmup=8 --draws=8 --update-every=0
```

Run each policy in a fresh process so retained allocator blocks from another
policy cannot contaminate the baseline. Repeat with `nova-dx11`, `nova-vulkan`
and `nova-opengl`. `--adapter=NAME` selects the native adapter on backends that
support name selection; always inspect the reported selected adapter.

For the complete Windows matrix (80 measured batches, 16 warm-up batches), run:

```powershell
cargo build --release -p gpui --example buffer_residency_lab --no-default-features --features windows-manifest,mimalloc-collect,nova-gfx-dx11,nova-gfx-dx12,nova-gfx-vulkan,nova-gfx-opengl
pwsh -NoProfile -File scripts/benchmark_buffer_residency.ps1 -DiscreteAdapter 'AMD Radeon RX 7600M XT'
```

Omit `-DiscreteAdapter` on machines without that additional adapter, or supply
the exact name of the local discrete adapter. The script runs A/B/B/A at each
size/reuse/update scenario, rejects failed pixel checks and debug builds, and
writes raw JSON plus `index.csv` under `target/p1a-memory/release`. With an
additional adapter it runs 120 fresh processes; otherwise it runs 80. It never
invokes Metal. Driver identity should be recorded beside the results.

The buffer contains deterministic words. A 256×256 fragment shader reads every
word for buffers of at least 256 KiB and folds contiguous words into each pixel's
32-bit RGBA checksum. Smaller buffers repeat their words across pixels. Pixel
readback is checked against the CPU payload after the initial upload, every dirty
write and the final batch. Logical read bytes include cache hits and are not a
measurement of physical DRAM/PCIe bandwidth.

`--update-every=0` uploads once and then reuses the static stream. A nonzero
interval updates a rotating dirty range of max(64 KiB, buffer_size/16) while
preserving the remainder. Test sizes 64 KiB, 1 MiB and 8 MiB, plus `--draws=1`
and `--draws=8` to distinguish API overhead from repeated-read pressure.

The single destination Buffer is updated only after the previous batch's fence
completes. This deliberately controlled throughput baseline does not test an
upload ring or prove stall-free production uploads with two in-flight slots.

## Measurements and decision gates

JSON schema 1 records the selected adapter, architecture, requested policy,
audited native path, workload parameters, raw samples, p50/p95/p99/max/mean,
pixel-check count, and allocator/resource-size observations with available local
and non-local driver budgets. Initial allocation and upload are timed separately.

Each batch uses the existing compatible offscreen helper. DX12 and Vulkan then
use `wait_texture_transfers`, whose current implementations drain all pending
graphics work (DX12 queue fence, Vulkan device idle). DX11 and OpenGL place an
empty-encoder completion fence after that draw. The report separates the helper,
completion signal and wait wall times; signal time is zero when bundled with the
backend wait hook. Their sum is draw-through-completion wall time. It includes driver overhead and
host scheduling; it is not a GPU timestamp. Readback/checksum computation is
outside those samples. Dirty upload distributions exclude clean zero-write frames.
Budget snapshots are not peak transient staging or isolated process residency.

Use an interleaved A/B/B/A order with repeated fresh processes. Compare initial
upload, dirty upload, clean reuse and dirty+completion separately. Record build
profile, driver/adapter identity, sizes, draw counts and update interval. A debug
run is a correctness/smoke baseline, not a production throughput claim. Optimized
builds and native GPU timestamps/captures are needed before a bandwidth claim.

Maintain separate Unified, Discrete and Unknown result groups. This Windows
machine alone cannot establish every hardware tier. DX11 and OpenGL results are
path/hint controls and cannot select a physical placement winner. Vulkan results
compare requests and upload routes without asserting distinct physical pools.

Enable a GPU-only default only after pixel validation, async upload/lifetime
validation, repeated representative workload gains and tests on the affected
hardware tier. Retain CpuToGpu for per-frame globals and animation values. No
such default is enabled by this baseline, and no Metal testing is performed in
this task.

## Measured Windows baseline (2026-10-09)

The release matrix completed 120 fresh-process runs: six backend/adapter pairs,
five scenarios and four A/B/B/A runs per scenario. All runs passed pixel checks
with 80 measured and 16 warm-up batches. Both AMD adapters used driver
`32.0.31007.1017`. OpenGL selected RX 7600M XT by default; the other default
backends selected Radeon 780M. Only DX12's native architecture query classified
these as Unified and Discrete; the remaining backend reports are Unknown.

The following values are the mean of two run-level p50 values for the 8 MiB
static stream with eight draws per batch, in microseconds. Completion includes
CPU recording, driver submission and the completion wait, not GPU timestamps.

| Backend / actual adapter | Initial upload CPU-visible / GPU-only | Completion CPU-visible / GPU-only |
| --- | ---: | ---: |
| DX11 / Radeon 780M | 4974.9 / 4862.6 | 1013.7 / 1006.6 |
| DX12 / Radeon 780M | 307.3 / 3747.9 | 1367.4 / 1614.8 |
| DX12 / RX 7600M XT | 308.7 / 5383.8 | 5392.9 / 1237.8 |
| Vulkan / Radeon 780M | 706.9 / 4411.1 | 8627.1 / 8483.3 |
| OpenGL / RX 7600M XT | 953.2 / 1188.6 | 322.1 / 329.5 |
| Vulkan / RX 7600M XT | 761.4 / 4985.2 | 4242.4 / 4421.9 |

DX12 on RX 7600M XT is a candidate for GPU-only static streams with substantial
reuse, once asynchronous upload and lifetime validation are complete. Radeon
780M's DX12 result supports retaining CPU-visible streams in this workload.
Vulkan did not show a consistent GPU-only reuse benefit on these two adapters;
DX11 and OpenGL remain equivalent-path and driver-hint controls.

Repeated runs have measurable variability. The relative spread `(high-low)/mean`
of the two same-policy completion p50 values had a median of 4.0%, p90 of 14.4% and maximum
of 19.5%; completion p95 spread reached 42.6%. These results support further
device-specific validation, not a universal placement rule. Raw JSON, the run
index and the detailed local summary are under `target/p1a-memory/`; allocator
snapshots include retained staging and do not establish physical residency.
