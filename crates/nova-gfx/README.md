# nova-gfx

[中文文档](README.zh-CN.md)

`nova-gfx` is a lightweight graphics foundation for BMCBL and future projects.
`gfx-core` is the canonical public API surface: it defines backend-neutral
descriptors, typed handles, errors, and device traits. Backend crates implement
those traits for Vulkan, Direct3D 12, and Metal.

Current crates:

- `gfx-core`: backend-neutral descriptions, handles, errors, and the
  `Gfx*Device` traits. It has no GPUI, winit, raw-window-handle, Vulkan, DX12,
  or Metal dependency.
- `gfx-memory`: GPU allocation wrappers, upload-ring accounting, deferred-free
  queues, and memory statistics.
- `gfx-shader`: WGSL validation and SPIR-V / HLSL / MSL generation through Naga.
- `gfx-dx12`: Windows D3D12 resource backend built on `windows-rs`.
- `gfx-metal`: Apple Metal resource backend built on `objc2-metal`.
- `gfx-vulkan`: Vulkan resource backend built on `ash`.
- `examples/triangle*`: minimal windowed triangle baselines using the core
  traits.
- `examples/atlas-smoke*`: atlas/resource binding smoke tests using the core
  traits.

## Public API

Use the narrowest core trait required by the caller:

- `GfxSurfaceDevice` for surfaces and swapchains.
- `GfxResourceDevice` for buffers, textures, views, samplers, and resource sets.
- `GfxPipelineDevice` for shaders, render passes, layouts, and pipelines.
- `GfxCommandDevice` for explicit command encoder work.
- `GfxPresentationDevice` for `draw_steps_and_present` and
  `draw_steps_to_texture`.
- `GfxDiagnosticsDevice` for live resource statistics.

`GfxDevice` combines the full device contract when a call site genuinely needs
all of it. Prefer narrower bounds in helpers so backends do not grow accidental
API requirements.

## Publishing

See [PUBLISHING.md](PUBLISHING.md) for the crates.io release order and package
validation checklist. The release order matters because backend crates depend on
`gfx-core`, and crates.io dry-runs require published dependency versions.

## Feature Selection

`gfx-memory` has no default backend features. Select the allocator backend you
actually need:

```powershell
rtk cargo test -p gfx-memory --no-default-features
rtk cargo test -p gfx-memory --no-default-features --features vulkan
rtk cargo test -p gfx-memory --no-default-features --features dx12
```

The backend crates enable only their own memory feature:

- `gfx-vulkan` depends on `gfx-memory` with `features = ["vulkan"]`.
- `gfx-dx12` depends on `gfx-memory` with `features = ["dx12"]`.
- `gfx-metal` depends on `gfx-memory` with `features = ["metal"]`.

GPUI users must choose a concrete nova-gfx backend explicitly:

```toml
gpui = { path = "vendor/gpui", default-features = false, features = ["nova-gfx-vulkan"] }
```

Use `nova-gfx-vulkan`, `nova-gfx-dx12`, or `nova-gfx-metal`. The old
`nova-gfx` feature no longer selects a backend; this is a breaking compile-time
selection change intended to avoid compiling unused backend crates.

To verify the compile boundary, inspect the dependency tree for the selected
backend:

```powershell
rtk cargo tree -p gfx-vulkan --no-default-features
rtk cargo tree -p gpui --no-default-features --features nova-gfx-vulkan
```

The Vulkan tree should not contain `gfx-dx12`, `gfx-metal`, D3D12, or Metal
Objective-C dependencies.

## Crate Documentation

Each library crate has its own crates.io README and rustdoc entry:

- `gfx-core`: canonical descriptors, handles, errors, and traits.
- `gfx-memory`: backend memory allocator wrappers and upload-ring accounting.
- `gfx-shader`: WGSL validation plus SPIR-V, HLSL, and MSL generation.
- `gfx-vulkan`: Vulkan implementation of the core traits.
- `gfx-dx12`: Direct3D 12 implementation of the core traits.
- `gfx-metal`: Metal implementation of the core traits.

The English README is the crates.io default. `README.zh-CN.md` files carry the
Chinese documentation for users who prefer Chinese API guidance.

## Window Boundary

`gfx-core` intentionally does not depend on `raw-window-handle`. Surface targets
are modeled as a backend-associated type on `GfxSurfaceDevice`; concrete backend
crates decide which native presentation target they accept. The current Vulkan,
DX12, and Metal backends use `raw-window-handle` in their own crates because
native surface creation is a platform integration boundary, not a core trait
dependency.

## Breaking API Change

Backend public inherent methods that duplicate the core traits are no longer the
maintained API. Callers should import the relevant `gfx_core::Gfx*Device` trait
and use trait methods on the concrete backend device. Convenience triangle and
atlas helpers such as `draw_and_present` and `draw_resources_and_present` are
core trait default methods; production renderers should generally use
`draw_steps_and_present` directly.
