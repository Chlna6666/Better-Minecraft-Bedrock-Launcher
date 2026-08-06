# Testing And Benchmarks

## Required Checks

```powershell
cargo fmt --all -- --check
cargo clippy --all-features --all-targets -- -D warnings
cargo test --all-features
cargo test --no-default-features
cargo doc --no-deps --all-features
cargo rustdoc --lib --all-features -- -D missing_docs
cargo run --example palette_tool -- audit --check
cargo run --example palette_tool -- generate-clean-room --check
cargo run --example palette_tool -- normalize --check
```

These checks are expected to pass on a fresh checkout without private world data.

## Optional Fixture

The integration test and benchmarks look for the sample Bedrock world in the
adjacent `bedrock-world` checkout:

```text
../bedrock-world/tests/fixtures/sample-bedrock-world
```

The folder should contain `level.dat` and `db/CURRENT`. If it is missing,
fixture tests and benches skip their world-backed cases instead of failing.

Do not commit real worlds. Add small in-memory tests for regressions whenever
possible.

## Feature Coverage

Run `cargo test --no-default-features` to validate CPU-only compilation. Run
`cargo test --all-features` for async, WebP, PNG, and GPU code paths.

GPU execution depends on host hardware and drivers. CI tests should assert API
contracts, cache identity, CPU fallback behavior, and diagnostics rather than
requiring a GPU device.

GPU backend contract checks must include:

```powershell
cargo test gpu_backend --features "gpu-dx11 gpu-vulkan"
cargo check --features gpu-dx11
cargo check --features gpu-vulkan
cargo check --features "gpu-dx11 gpu-vulkan"
```

These tests lock down:

- stable cache slugs: `cpu`, `auto`, `wgpu`, `dx11`, `dx12`, `vulkan`
- stable diagnostic labels: `cpu`, `dx11`, `wgpu-dx12`, `wgpu-vulkan`, `mixed`
- default `RenderGpuOptions`: `Auto`, `AllowCpu`, `ComposeOnly`
- backend-specific cache separation for UI callers such as BMCBL

For GPU changes, include a small direct compose test that accepts either a valid
GPU result or a non-empty fallback reason. When a GPU is available, check
`gpu_tiles`, `gpu_requested_backend`, `gpu_actual_backend`, `gpu_adapter_name`,
`gpu_device_name`, `gpu_fallback_reason`, `gpu_prepare_ms`, `gpu_upload_ms`,
`gpu_dispatch_ms`, `gpu_readback_ms`, `gpu_uploaded_bytes`,
`gpu_readback_bytes`, `gpu_peak_in_flight`, and `gpu_buffer_reuses` in the
streaming/export stats. Interactive streaming tests should also verify that
default render groups contain more than one tile when multiple cache misses are
available, and that `world_worker_threads` rises above one unless
`RenderThreadingOptions::Single` is explicitly selected.

Manual GPU performance comparison should use:

```powershell
$env:BEDROCK_RENDER_GPU_BENCH='1'
cargo bench --bench render --features "gpu-dx11 gpu-vulkan" -- --noplot __machine_report_only__
Remove-Item Env:\BEDROCK_RENDER_GPU_BENCH
```

Record the raw `bedrock_render_report` lines in the issue/PR. A machine is only
a valid DX11 baseline when `backend=dx11`, `gpu_actual=Dx11`, `gpu_tiles > 0`,
and `gpu_fallback=none`. A machine is only a valid Vulkan baseline when
`backend=vulkan`, `gpu_actual=Vulkan`, `gpu_tiles > 0`, and
`gpu_fallback=none`.

For surface rendering changes, cover both `SurfaceRenderOptions::block_boundaries`
enabled and disabled. Tests should include flat terrain, a sharp height step,
small height noise below the threshold, shallow water, and multi-block pixels
where per-block outlines are intentionally skipped.

For `bedrock_render::editor` changes, cover both facade behavior and render-side
invalidation:

- map/global record scan and single-record read helpers
- HSA scan/write/delete roundtrips
- block entity list, edit-at-coordinate, delete-at-coordinate helpers
- modern actor read/write/delete/move helpers with `digp -> actorprefix`
  preservation handled by `bedrock-world`
- heightmap and biome storage write helpers
- `MapEditInvalidation::merge`, metadata refresh, overlay refresh, affected
  chunk propagation, and tile-cache cleanup flags

Write tests should use temporary or in-memory worlds where possible. Manual
tests against real worlds must require explicit write mode and a per-operation
confirmation in the UI.

Legacy terrain changes must include a synthetic `LegacyTerrain` fixture. At a
minimum, assert that `SurfaceBlocks`, `HeightMap`, `LayerBlocks`, and
`CaveSlice` produce non-transparent pixels and do not report missing chunks for
the loaded legacy chunk. Real 0.16 worlds may be used manually, but must not be
committed.

Height and surface regressions must include mismatched raw-height fixtures:
write raw Data2D/Data3D/Legacy height values that disagree with the actual top
block, then assert `SurfaceBlocks` and `HeightMap` follow the computed surface
while `RawHeightMap` preserves the raw diagnostic output. Keep thin overlays,
transparent water, and a missing-`column_samples` bake in the fixture set so
canonical height fixes do not reintroduce renderer-side surface rescanning.

Coordinate-placement changes must include a synthetic signature fixture. Cover
negative chunk coordinates, multi-chunk tiles, region boundaries, and at least
one one-region-per-chunk layout. The same key pixels must match for direct tile,
shared bake, region/web tile, streaming session, and GPU-prepared paths when GPU
compose is available. `RenderPipelineStats::region_chunks_out_of_bounds` and
`tile_missing_region_samples` should remain zero for complete fixtures.

## Streaming Session Checks

For changes touching `MapRenderSession` or frontend integration, cover:

- cancellation before and during a tile batch
- `Ready` events from disk/memory cache on a second render with the same cache signature
- `Ready { source: Render }` and failed events for mixed valid/missing regions
- complete events containing diagnostics and pipeline stats
- CPU fallback when GPU compose is unavailable or rejected
- GPU queue cancellation before tile readback completes
- `RenderTilePriority::DistanceFrom` emits nearer ready tiles before
  farther tiles, independent of the original planned-tile vector order
- `render_web_tiles_streaming_channel` closes cleanly when the receiver is
  dropped and logs task-level failures

Manual UI testing should open a large world, pan before metadata indexing
finishes, zoom in/out, switch dimension/mode, toggle cache bypass, and verify
that stale generation events are ignored.
