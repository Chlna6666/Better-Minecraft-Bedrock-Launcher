# GPUI Performance Baseline — 2026-08-24

This report is the first reproducible Criterion baseline for the local GPUI fork. It is a
comparison anchor, not a platform-wide performance claim. Future changes must be measured on the
same machine and power profile in the same session, or record a new environment section.

## Environment

- Repository revision: `2780c973977f10dbfa5b5b3de9f7641a45d95e89` with a dirty worktree.
- Command: `rtk cargo bench --manifest-path crates/gpui/Cargo.toml --features bench`.
- Result: exit code 0; 18 Criterion inputs completed.
- OS: Windows 11 IoT Enterprise LTSC, version `10.0.26100`, build `26100`.
- CPU: AMD Ryzen 7 7840H, 8 cores / 16 logical processors.
- Memory: 31.2 GiB visible to Windows.
- Power scheme: Balanced (`381b4222-f694-41f0-9685-ff5bb260df2e`).
- Graphics adapters: AMD Radeon 780M and AMD Radeon RX 7600M XT, driver
  `32.0.31007.1017`. These CPU benchmarks do not exercise a presentation backend.
- Toolchain: `rustc 1.95.0 (59807616e 2026-04-14)`, `cargo 1.95.0`, MSVC host,
  LLVM 22.1.2.
- Criterion plotting backend: Plotters; Gnuplot was not installed.

The worktree state is recorded because this baseline measures the complete in-progress GPUI
change set, not only committed revision `2780c973`. A clean candidate must not be compared against
the revision alone without preserving or reconstructing this diff.

## Results

Criterion time estimates are shown as `[lower bound / point estimate / upper bound]`. Sample size
is 20 for static image and layout groups, 10 for animated WebP, 30 for the bitmap pool, and 40 for
path tessellation. Fixtures are deterministic and are constructed outside measured iterations.

| Group / input | Time estimate |
| --- | ---: |
| `image_render/full_size/png` | `[47.095 / 47.607 / 48.085] ms` |
| `image_render/full_size/webp` | `[46.638 / 46.868 / 47.142] ms` |
| `image_render/contain/png/320x180` | `[39.527 / 39.982 / 40.494] ms` |
| `image_render/contain/png/1280x720` | `[137.86 / 139.51 / 141.21] ms` |
| `image_render/contain/webp/320x180` | `[64.064 / 64.985 / 66.146] ms` |
| `image_render/contain/webp/1280x720` | `[147.47 / 149.70 / 151.99] ms` |
| `image_render/animated_webp/resident_24_frames_320x180` | `[42.169 / 43.011 / 43.569] ms` |
| `layout/cold_flat_tree/128` | `[77.732 / 80.957 / 84.818] us` |
| `layout/cold_flat_tree/1024` | `[0.97981 / 1.0033 / 1.0298] ms` |
| `layout/cold_flat_tree/4096` | `[6.1846 / 6.2852 / 6.4088] ms` |
| `layout/retained_flat_tree/128` | `[68.738 / 70.377 / 71.673] us` |
| `layout/retained_flat_tree/1024` | `[0.67803 / 0.70915 / 0.74543] ms` |
| `layout/retained_flat_tree/4096` | `[3.1917 / 3.2950 / 3.3910] ms` |
| `bitmap_pool/steady_state/uniform_64k` | `[1.7402 / 1.7789 / 1.8339] us` |
| `bitmap_pool/steady_state/mixed_256b_to_1m` | `[4.2410 / 4.2776 / 4.3145] us` |
| `path_tessellation/64` | `[27.677 / 28.261 / 28.784] us` |
| `path_tessellation/512` | `[0.55768 / 0.57077 / 0.58367] ms` |
| `path_tessellation/4096` | `[23.161 / 23.574 / 24.010] ms` |

## Phase 2 focused measurements

These measurements were collected later in the same worktree and environment after adding the
retained frame-upload suite and dense large-buffer workload. They extend the first baseline; they
do not replace its original 18 inputs.

| Group / input | Time estimate |
| --- | ---: |
| `frame_upload/quads/128` | `[27.161 / 27.432 / 27.730] us` |
| `frame_upload/quads/1024` | `[222.83 / 226.17 / 229.48] us` |
| `frame_upload/quads/8192` | `[1.7635 / 1.7927 / 1.8235] ms` |
| `frame_upload/backdrop_blurs/1` | `[506.97 / 513.37 / 521.34] ns` |
| `frame_upload/backdrop_blurs/16` | `[3.0746 / 3.1224 / 3.1653] us` |
| `frame_upload/backdrop_blurs/128` | `[22.290 / 22.459 / 22.649] us` |
| `frame_upload/atlas_pixels/rgba_320x180_padding_0` | `[89.472 / 91.576 / 94.275] us` |
| `frame_upload/atlas_pixels/rgba_320x180_padding_1` | `[96.393 / 102.36 / 107.39] us` |
| `frame_upload/atlas_pixels/bgra_320x180_padding_0` | `[6.9906 / 7.0557 / 7.1121] us` |
| `frame_upload/atlas_pixels/bgra_320x180_padding_1` | `[6.9640 / 7.1322 / 7.3317] us` |
| `frame_upload/atlas_monochrome/16x16_padding_0` | `[233.85 / 242.48 / 254.17] ns` |
| `frame_upload/atlas_monochrome/16x16_padding_1` | `[273.73 / 289.65 / 304.80] ns` |
| `frame_upload/atlas_monochrome/32x32_padding_0` | `[823.08 / 858.24 / 899.30] ns` |
| `frame_upload/atlas_monochrome/32x32_padding_1` | `[900.07 / 949.01 / 995.99] ns` |
| `frame_upload/atlas_subpixel/16x16_padding_0` | `[1.1083 / 1.1366 / 1.1737] us` |
| `frame_upload/atlas_subpixel/16x16_padding_1` | `[1.1640 / 1.1877 / 1.2147] us` |
| `frame_upload/atlas_subpixel/32x32_padding_0` | `[3.5395 / 3.6793 / 3.8190] us` |
| `frame_upload/atlas_subpixel/32x32_padding_1` | `[3.9498 / 4.1552 / 4.3293] us` |
| `frame_upload/atlas_writes/1x1/1` | `[2.0999 / 2.2026 / 2.3058] us` |
| `frame_upload/atlas_writes/1x1/8` | `[2.1056 / 2.1921 / 2.2800] us` |
| `frame_upload/atlas_writes/1x1/64` | `[4.8562 / 5.0689 / 5.2654] us` |
| `frame_upload/atlas_writes/16x16/1` | `[2.0371 / 2.1008 / 2.1765] us` |
| `frame_upload/atlas_writes/16x16/8` | `[2.3944 / 2.4536 / 2.5170] us` |
| `frame_upload/atlas_writes/16x16/64` | `[5.6386 / 5.9267 / 6.2703] us` |
| `frame_upload/atlas_writes/64x64/1` | `[2.7973 / 2.9704 / 3.1034] us` |
| `frame_upload/atlas_writes/64x64/8` | `[4.3281 / 4.7935 / 5.3658] us` |
| `frame_upload/atlas_writes/64x64/64` | `[76.956 / 81.876 / 89.662] us` |
| `bitmap_pool/steady_state/dense_1m_to_1_125m` | `[1.1676 / 1.1887 / 1.2108] us` |
| `image_render/animated_webp/streamed_24_frames_320x180` | `[46.314 / 46.928 / 47.484] ms` |
| `image_render/failure/truncated_animated_webp` | `[128.53 / 130.00 / 131.57] ns` |
| `image_render/animated_webp/streamed_loop_restart_320x180` | `[57.574 / 58.512 / 60.255] ms` |
| `image_render/animated_webp/streamed_8_streams_24_frames_320x180` | `[292.44 / 296.23 / 300.29] ms` |
| `image_render/animation_queue/stale_12_frames` | `[1.9587 / 2.0855 / 2.2222] us` |

The dense bitmap workload also asserts that 32 retained buffers spanning 1 MiB + 1 byte through
approximately 1.125 MiB consume no more than 40 MiB. This guards internal fragmentation directly;
the previous power-of-two bucketing could retain approximately 64 MiB for the same requests.

## Native texture-transfer measurements — 2026-08-25

These Criterion runs use the real DX12 and Vulkan backends on the AMD Radeon 780M. Each timed
iteration encodes one batch, submits it, and waits for completion, so the wall interval includes
CPU planning, driver submission, synchronization, and GPU work. The GPU column is the median of 32
native timestamp-query samples collected for the same batch shape; it does not include CPU work.

| Backend / batch | Wall time 95% interval | Point estimate | Throughput point estimate | GPU median |
| --- | ---: | ---: | ---: | ---: |
| DX12 / 1 | `151.30–158.54 us` | `154.56 us` | `6.3184 MiB/s` | `2.200 us` |
| DX12 / 8 | `203.75–215.62 us` | `209.58 us` | `37.276 MiB/s` | `12.600 us` |
| DX12 / 64 | `598.19–615.28 us` | `605.89 us` | `103.15 MiB/s` | `108.800 us` |
| Vulkan / 1 | `159.72–171.13 us` | `165.18 us` | `5.9123 MiB/s` | `4.080 us` |
| Vulkan / 8 | `173.75–187.77 us` | `180.02 us` | `43.398 MiB/s` | `4.320 us` |
| Vulkan / 64 | `231.44–238.90 us` | `235.19 us` | `265.74 MiB/s` | `7.800 us` |

Commands:

```text
rtk cargo bench --manifest-path crates/nova-gfx/gfx-dx12/Cargo.toml --bench texture_write -- --noplot
rtk cargo bench --manifest-path crates/nova-gfx/gfx-vulkan/Cargo.toml --bench texture_write -- --noplot
```

The Vulkan run used `amdvlk64.dll` through Vulkan loader 1.4.341. These numbers establish a
reproducible crossover matrix for this adapter; they are not portable constants for discrete GPUs,
software adapters, virtual machines, or different drivers.

## Hardware acceleration boundary

- Nova's DX12 and Vulkan backends already batch pending atlas writes through an upload ring and
  submit the texture copies together. Atlas conversion and padding remain CPU work; the new
  `scene_pack_time` diagnostic isolates CPU scene packing from atlas command queueing and GPU
  submission waits.
- Upload-ring trimming now returns the retained native-page count. DX12 drops trailing upload
  resources and Vulkan explicitly destroys their buffers and allocator records after the owning
  fence completes; logical page trimming can no longer leave backend staging allocations resident.
- DX12 upload pages remain mapped from page creation through page destruction instead of mapping
  once per texture region. Its batch path also prepares copies directly from the caller iterator,
  without a compatibility retry allocation or quadratic unique-texture scan. These are structural
  reductions in mapping and bookkeeping operations; the existing `atlas_writes` workload does not
  execute a native backend, so no GPU-time or end-to-end speedup is claimed from it.
- Upload batches are now planned into page-sized contiguous ranges and committed only after the
  complete plan succeeds. Only a single write larger than the configured page size creates an
  oversized page, while ordinary multi-page batches scan once per packed range instead of once per
  write. The allocator tests cover contiguous carving, page-sized splitting, checked alignment,
  existing offsets, and rejection before mutation; native savings still require a backend
  benchmark before any percentage claim.
- Vulkan texture staging now carries the caller's `TextureDataLayout::offset` into the native buffer
  copy region. A focused test locks down the resulting allocation-relative offset so batched writes
  do not silently read from the beginning of a non-zero-offset source slice. Vulkan also rejects
  offsets and row pitches that are not aligned to the currently supported four-byte texel formats,
  rather than truncating `bytes_per_row` while encoding the native copy.
- DX12 and Vulkan now expose one narrow texture-transfer contract for waiting, timestamp capability,
  the most recently completed native GPU transfer duration, and tightly packed texture readback.
  The backend contract fixtures upload a 2x2 RGBA image with a source prefix and padded rows, then
  compare GPU readback bytes; unavailable adapters may skip, while data or synchronization failures
  remain test failures.
- Vulkan ordinary buffers and images use allocator-managed blocks instead of forcing one dedicated
  device-memory block per resource. Its upload command pools, command buffers, and timestamp query
  pools are recycled only after the submission fence completes, with a bounded idle pool. DX12
  recycles the corresponding command allocator, command list, timestamp query heap, and readback
  resource under the same fence rule.
- Vulkan diagnostics now read the allocator's detailed live/reserved report, expose unused reserved
  bytes and whole-percent utilization, and exercise a 64-texture allocate/free contract on the real
  backend. Timestamp deltas honor each queue family's valid-bit width, including counter wrap.
- Dynamic atlas synchronization stages every new or resized texture before committing the map.
  Partial texture-view/resource-set construction rolls back all native objects, and a failed batch
  leaves the previous atlas set intact instead of destroying usable resources first.
- Native DX12 and Vulkan `texture_write` Criterion workloads now execute and wait for real 1/8/64
  texture-write batches. They report synchronized end-to-end throughput and retain the resolved GPU
  timestamp as a measured backend diagnostic; the older GPUI `frame_upload` benchmark remains the
  separate CPU descriptor/staging baseline.
- Moderate and aggressive window memory trims now include layout-engine maps and scratch vectors,
  frame element-state tables, and input dispatch-tree storage. Frame retained-capacity diagnostics
  include these containers, preventing large page high-watermarks from remaining invisible after
  navigation or window deactivation.
- DX12 adapter selection still prefers hardware, then falls back to the WARP software adapter when
  no hardware adapter is visible. This preserves the same CPU staging and copy semantics for
  virtual machines and remote environments instead of turning optional hardware acceleration into
  a startup requirement.
- A GPU conversion path is not considered implemented until `nova-gfx` has one cross-backend
  contract for compute pipelines, compute shader stages, storage resources, dispatch, and explicit
  capability reporting. Adding a DX12/Vulkan-only GPUI path would leave Metal with different
  semantics and is therefore rejected during this phase.
- Metal must first provide real texture creation and texture writes. Hardware timings also require
  backend timestamp-query support; CPU command-encoding time must not be reported as GPU execution
  time.
- A future GPU atlas converter must batch enough dirty pixels per dispatch to amortize command and
  synchronization overhead, preserve the exact BGRA and subpixel premultiplication results, and
  beat the CPU row path on the same fixture and hardware. Small glyph uploads stay on CPU unless
  measurements establish a profitable crossover threshold.
- The CPU converters now operate on contiguous center rows and replicate converted edges. This
  shape is suitable for compiler auto-vectorization without architecture-specific `unsafe` code.
  Explicit SIMD remains gated on assembly inspection plus 16 px and 32 px glyph benchmarks; a
  SIMD dispatch layer is not justified when its setup cost erases the gain on typical glyphs.
- GPUI denies `unsafe` code, so an experimental runtime SSSE3/NEON implementation was rejected
  before benchmarking. Replacing four-byte copies with individual safe byte assignments was also
  rejected: 11 of 12 default-profile atlas workloads regressed significantly, including roughly
  51% to 70% for padded RGBA and 46% to 58% for 16 px padded monochrome glyphs. No part of either
  experiment remains in production code. Architecture-specific acceleration must therefore use a
  reviewed safe abstraction, retain a scalar fallback for virtualized or older CPUs, and beat both
  the normal bench profile and the production `release` profile before adoption.

## Interpretation boundary

- The retained-layout results demonstrate the retained path, but do not by themselves prove an
  end-to-end frame-time improvement.
- The animated WebP result measures decoding a 24-frame fixture, not a 24 FPS playback policy.
  GIF, APNG, and animated WebP playback follows each file's frame delays; the default playback
  ceiling is 90 FPS and applications can explicitly configure a higher ceiling.
  Streaming queue latency,
  dropped deadlines, upload bytes, and playback pacing require the 300-frame window protocol.
- The streaming WebP result includes bounded worker handoff and consumption of the 23 frames after
  the initial frame. It excludes window scheduling, atlas upload, GPU rendering, and presentation.
  The loop-restart workload consumes one additional frame from the next loop, so its difference
  from the 24-frame workload includes both iterator restart and one additional frame; it is a
  regression sentinel, not an isolated restart-duration measurement. The eight-stream workload
  starts all streams before consuming them, but consumption is deterministic and sequential; it
  measures bounded multi-stream throughput rather than presentation fairness.
- The stale-queue workload preconstructs twelve one-pixel queued frames and excludes fixture setup
  from the measured interval. Acquiring the receiver mutex once per drain reduced its estimate from
  `[3.9153 / 4.2359 / 4.7255] us` to `[3.5667 / 3.5970 / 3.6222] us`; Criterion reported a
  `[-28.997%, -24.050%, -18.522%]` change with `p = 0.00`. Waking the worker pool only after a
  producer observes global queue backpressure reduced it again to
  `[1.9587 / 2.0855 / 2.2222] us`, a `[-43.403%, -40.852%, -37.637%]` change with `p = 0.00`.
  The same wake policy reduced the eight-stream workload from
  `[357.67 / 360.66 / 363.84] ms` to `[292.44 / 296.23 / 300.29] ms`; Criterion reported a
  `[-19.030%, -17.865%, -16.468%]` time change and a
  `[+19.715%, +21.751%, +23.503%]` throughput change with `p = 0.00`. No outliers were reported
  for either post-change workload. Global-pressure releases still wake the complete pool so the
  optimization does not assign newly available shared capacity to an arbitrary single worker.
- The atlas-pixel workloads reuse both source and destination buffers and consume the destination
  through `black_box`; allocation and GPU submission are excluded. Replacing per-output-pixel edge
  clamping with center-row processing plus horizontal and vertical edge replication reduced RGBA
  padding-one conversion from `[217.14 / 226.24 / 234.23] us` to
  `[96.393 / 102.36 / 107.39] us` (`[-58.027%, -56.000%, -54.164%]`, `p = 0.00`). The same change
  reduced BGRA padding-one copying from `[99.029 / 100.11 / 101.09] us` to
  `[6.9640 / 7.1322 / 7.3317] us` (`[-93.026%, -92.874%, -92.705%]`, `p = 0.00`). These are CPU
  atlas-packing results, not GPU transfer bandwidth or presentation time. A descriptor-container
  experiment (`SmallVec` and a single-write branch) produced no stable improvement and was removed.
- The CPU atlas-write matrix covers 1 px, 16 px, and 64 px square RGBA tiles at batch counts 1, 8,
  and 64. Criterion excludes deterministic tile construction from the timed region. The measured
  region drains the prepared batch and builds the CPU-side upload descriptors; it does not submit
  commands to a GPU. The reported byte throughput is therefore a workload normalization value,
  not PCIe, unified-memory, or device-copy bandwidth. Use the native texture-transfer matrix above
  for synchronized backend wall time and GPU timestamp comparisons.
- Applying the same center-row conversion and edge-replication structure to glyph uploads reduced
  monochrome 16 px and 32 px padding-one estimates from
  `[754.90 / 766.88 / 782.08] ns` and `[2.7479 / 2.9106 / 3.0640] us` to
  `[273.73 / 289.65 / 304.80] ns` and `[900.07 / 949.01 / 995.99] ns`. Criterion reported
  `[-65.078%, -63.853%, -62.453%]` and `[-68.585%, -66.984%, -65.412%]` changes, both with
  `p = 0.00`. Subpixel padding-one estimates fell from
  `[1.7238 / 1.7792 / 1.8534] us` and `[5.8469 / 5.9384 / 6.0194] us` to
  `[1.1640 / 1.1877 / 1.2147] us` and `[3.9498 / 4.1552 / 4.3293] us`; changes were
  `[-37.609%, -35.501%, -33.344%]` and `[-35.106%, -32.989%, -30.283%]`, both with
  `p = 0.00`. Padding-zero workloads also improved, so the shared row path does not trade the
  unpadded case for padded throughput. Some post-change glyph samples contained high outliers;
  the statistically significant intervals above, rather than a single sample, are the comparison
  evidence.
- The failure-path result uses a container header truncated before any complete frame; a file with
  a complete first frame and a truncated tail is intentionally allowed to degrade to that frame.
- Path cost grows sharply at 4,096 segments. Treat that result as a regression sentinel and profile
  before changing the tessellator; the baseline alone does not identify the cause.
- GPU presentation, compositor behavior, peak RSS, Linux, and macOS are not measured by this run.
