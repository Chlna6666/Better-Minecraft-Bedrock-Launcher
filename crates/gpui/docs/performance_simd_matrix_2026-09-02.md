# GPUI SIMD Crossover Report — 2026-09-02

This is the human-facing result of the GPUI bulk-data A/B benchmark. Criterion's JSON files are
intermediate data only and remain under `target/criterion/`; this Markdown file is the deliverable.

## Run

- Revision: `a09a2ee87e498b274520405d7c075146c402bb31`, dirty worktree.
- Platform: Windows, AMD Ryzen 7 7840H, same environment as
  [`performance_baseline_2026-08-24.md`](performance_baseline_2026-08-24.md).
- Profile: release Criterion bench, Plotters backend because Gnuplot is unavailable.
- Command settings: `--bench --warm-up-time 0.5 --measurement-time 1 --sample-size 20 --noplot`.
- RGBA: 16/32/64/256/1024/65536 pixels, padding 0/1/2. SIMD cases force the Fearless candidate
  so small-size dispatch cost is measured rather than silently falling back to scalar.
- Path: 64/512/4096 vertices for transform and cache-miss packing.
- Mesh: 256/1024/65536 vertices, u16/u32 indices.
- Primitive: 128/1024/8192 quads, retained scene upload encoding.
- Values are the median estimate and its 95% confidence interval, shown as
  `[lower / point / upper]`; delta is `(Fearless SIMD / scalar - 1)`.

All final benchmark commands completed successfully. The gpui build still emits 21 existing
warnings; no new benchmark-specific warning or failure was introduced.

## RGBA/BGRA conversion

The production path now copies/pads the upload first, then performs one runtime-selected bulk
conversion over the complete row set. Rows below 64B stay scalar. Fearless SIMD uses a cached
`Level`, caps x86 at AVX2, and does not detect features inside the pixel loop.

| Pixels | Padding | Scalar | Fearless SIMD | Delta |
| ---: | ---: | ---: | ---: | ---: |
| 16 | 0 | `[18.25 / 18.64 / 19.19] ns` | `[20.93 / 21.64 / 21.91] ns` | +16.1% |
| 16 | 1 | `[45.34 / 46.73 / 48.26] ns` | `[50.37 / 51.18 / 55.05] ns` | +9.5% |
| 16 | 2 | `[62.19 / 63.12 / 64.71] ns` | `[70.65 / 72.59 / 76.83] ns` | +15.0% |
| 32 | 0 | `[23.71 / 24.15 / 24.77] ns` | `[26.48 / 27.17 / 27.34] ns` | +12.5% |
| 32 | 1 | `[56.46 / 59.13 / 61.41] ns` | `[60.73 / 62.60 / 64.14] ns` | +5.9% |
| 32 | 2 | `[87.14 / 88.49 / 89.99] ns` | `[97.38 / 98.99 / 102.6] ns` | +11.9% |
| 64 | 0 | `[38.31 / 39.06 / 40.62] ns` | `[43.75 / 44.84 / 45.77] ns` | +14.8% |
| 64 | 1 | `[112.6 / 117.2 / 118.5] ns` | `[102.7 / 105.3 / 106.4] ns` | -10.2% |
| 64 | 2 | `[134.6 / 135.7 / 136.9] ns` | `[144.9 / 146.6 / 149.3] ns` | +8.0% |
| 256 | 0 | `[130.4 / 132.9 / 135.9] ns` | `[37.68 / 38.70 / 39.46] ns` | -70.9% |
| 256 | 1 | `[236.9 / 243.3 / 247.4] ns` | `[123.3 / 125.2 / 128.0] ns` | -48.6% |
| 256 | 2 | `[298.8 / 305.1 / 318.1] ns` | `[172.3 / 177.4 / 185.2] ns` | -41.8% |
| 1,024 | 0 | `[505.1 / 516.5 / 536.7] ns` | `[183.5 / 186.0 / 188.4] ns` | -64.0% |
| 1,024 | 1 | `[700.2 / 711.8 / 757.7] ns` | `[296.6 / 299.5 / 308.3] ns` | -57.9% |
| 1,024 | 2 | `[785.2 / 788.9 / 806.6] ns` | `[388.4 / 395.5 / 410.9] ns` | -49.9% |
| 65,536 | 0 | `[34.62 / 36.36 / 38.32] us` | `[11.11 / 11.32 / 11.72] us` | -68.9% |
| 65,536 | 1 | `[31.64 / 32.01 / 32.97] us` | `[10.51 / 10.68 / 10.87] us` | -66.6% |
| 65,536 | 2 | `[33.60 / 34.08 / 34.69] us` | `[10.51 / 10.76 / 10.92] us` | -68.4% |

The crossover is clear: 16/32 pixels are scalar; 64 pixels are still mixed and layout-dependent;
256 pixels and above are strong SIMD candidates. The production threshold is therefore applied
per row, with padding retained in the same upload buffer.

## Path transform and cache-miss packing

| Vertices | Transform scalar | Transform Fearless SIMD | Delta |
| ---: | ---: | ---: | ---: |
| 64 | `[108.4 / 109.4 / 112.3] ns` | `[188.0 / 190.2 / 191.9] ns` | +73.8% |
| 512 | `[581.2 / 594.1 / 617.0] ns` | `[913.3 / 929.5 / 950.0] ns` | +56.5% |
| 4,096 | `[4.38 / 4.44 / 4.51] us` | `[6.79 / 6.93 / 7.12] us` | +56.0% |

The current Fearless transform candidate loses at every tested size. The workload includes output
`PathVertex` construction and metadata movement, so vector arithmetic is not the dominant cost.
Production path transform remains scalar; the SIMD candidate is retained only in the benchmark for
future redesign of the packed representation.

Cache-miss packing remains a separate, much larger workload:

| Vertices | Cache-miss packing |
| ---: | ---: |
| 64 | `[7.43 / 7.51 / 7.57] us` |
| 512 | `[56.80 / 57.69 / 58.67] us` |
| 4,096 | `[485.1 / 494.4 / 506.3] us` |

This does not prove that path packing itself is SIMD-friendly; it is dominated by cache and scene
packing work and should not be replaced by a transform-only SIMD implementation.

## Mesh conversion

The A/B includes the existing scalar vertex packing plus index conversion. The SIMD candidate is
forced for both formats so the u32 result is also measured honestly.

| Vertices | Indices | Scalar | Fearless SIMD | Delta |
| ---: | ---: | ---: | ---: | ---: |
| 256 | u16 | `[7.72 / 7.80 / 7.88] us` | `[5.50 / 5.67 / 5.80] us` | -27.2% |
| 256 | u32 | `[5.58 / 5.68 / 5.71] us` | `[5.37 / 5.42 / 5.54] us` | -4.5% |
| 1,024 | u16 | `[30.66 / 30.98 / 31.28] us` | `[21.77 / 22.15 / 22.73] us` | -28.5% |
| 1,024 | u32 | `[22.50 / 22.74 / 23.34] us` | `[20.81 / 20.97 / 21.10] us` | -7.8% |
| 65,536 | u16 | `[1.99 / 2.01 / 2.05] ms` | `[1.48 / 1.50 / 1.51] ms` | -25.6% |
| 65,536 | u32 | `[1.48 / 1.49 / 1.51] ms` | `[1.45 / 1.46 / 1.48] ms` | -2.5% |

u16 has a stable roughly 25% whole-workload gain and is enabled in production for bulk inputs.
u32's gain is small and hardware-sensitive, so production keeps u32 scalar until an index-only
benchmark or target-GPU profile shows a larger stable benefit.

## Primitive packing signal

This is the existing retained scene upload path, not an isolated primitive packer:

| Quads | `scene_pack_time/quads` |
| ---: | ---: |
| 128 | `[18.23 / 18.40 / 18.61] us` |
| 1,024 | `[146.1 / 147.6 / 150.5] us` |
| 8,192 | `[1.22 / 1.23 / 1.26] ms` |

The cost scales with quad count, but this measurement cannot attribute the time to primitive
packing versus scene traversal, ordering, and buffer management. No SIMD primitive-packing change
is enabled from this result.

## Other SIMD candidates

- Premultiply/unpremultiply and image decode postprocess already use the safe Fearless SIMD path
  for large buffers, with scalar handling for small buffers and a cached runtime level.
- Hsla conversion, small rect/glyph batches, small vectors, and small hash keys remain scalar.
- AVX-512 is not selected for the default UI path. Fearless's runtime level supports the available
  SSE2/NEON fallback paths; x86 production conversion is capped at AVX2.

## Final decisions

| Workload | Production decision |
| --- | --- |
| RGBA/BGRA conversion | Enable Fearless SIMD for rows at or above the bulk boundary; retain scalar for smaller rows. |
| Premultiply/unpremultiply and image decode postprocess | Keep the existing large-buffer Fearless SIMD implementation. |
| Path transform | Keep scalar; current candidate is slower at all tested sizes. |
| Path cache-miss packing | Keep current packing path; no transform-only SIMD substitution. |
| Mesh conversion | Enable Fearless SIMD for large u16 index packing; keep u32 scalar. |
| Primitive packing | No SIMD change; attribution needs a split benchmark/profiler first. |

Fearless dispatch is performed once per bulk buffer/path/mesh operation through a cached `Level`,
not inside the pixel/index/vertex loop. The library's safe token-based dispatch API does not expose
an erasable zero-argument SIMD function pointer without unsafe target-feature wrappers, so the
implementation deliberately uses the cached level plus one dispatch per operation.

## Result locations

- Human-facing report: this Markdown file.
- Criterion raw data: `target/criterion/frame_upload_atlas_pixels/`,
  `target/criterion/path_transform/`, `target/criterion/path_packing_cache_miss/`,
  `target/criterion/mesh_packing/`, and `target/criterion/frame_upload_scene_pack_time_quads/`.
