# nova-gfx

[中文文档](README.zh-CN.md)

`nova-gfx` is a lightweight graphics foundation for BMCBL and future projects.
`gfx-core` is the canonical public API surface: it defines backend-neutral
descriptors, typed handles, errors, and device traits. Backend crates implement
those traits for Vulkan, Direct3D 11/12, OpenGL 4.5, and Metal.

Current crates:

- `gfx-core`: backend-neutral descriptions, handles, errors, and the device
  capability traits. It has no UI framework, winit, raw-window-handle, Vulkan,
  DX12, or Metal dependency.
- `gfx-memory`: GPU allocation wrappers, upload-ring accounting, deferred-free
  queues, and memory statistics.
- `gfx-shader`: WGSL validation and SPIR-V / HLSL / MSL / GLSL generation through Naga.
- `gfx-dx11`: Windows D3D11 resources, native presentation and SM5.0 bytecode.
- `gfx-opengl`: native OpenGL 4.5 through WGL on Windows and EGL on Linux.
- `gfx-dx12`: Windows D3D12 resource backend built on `windows-rs`.
- `gfx-metal`: Apple Metal resource backend built on `objc2-metal`.
- `gfx-vulkan`: Vulkan resource backend built on `ash`.
- `examples/triangle*`: minimal windowed triangle baselines using the core
  traits.
- `examples/atlas-smoke*`: atlas/resource binding smoke tests using the core
  traits.

## Public API

Use the narrowest core trait required by the caller:

- `SurfaceDevice` for surfaces, swapchains, and surface reconfiguration.
- `ResourceDevice` for buffers, textures, views, samplers, and bind groups.
- `PipelineDevice` for shaders, render pass layouts, pipeline layouts, and pipelines.
- `CommandDevice` for explicit command encoder work.
- `SubmissionDevice` for deferred submissions and completion tracking.
- `PresentationDevice` for `render_and_present`, `render_to_textures`,
  surface readiness, and presentation capabilities.
- `DiagnosticsDevice` for live resource statistics.

`Device` combines the full device contract when a call site genuinely needs
all of it. Prefer narrower bounds in helpers so backends do not grow accidental
API requirements.

## Publishing

Release order follows the dependency direction: publish `gfx-core` first, then
`gfx-memory`/`gfx-shader`, then the backend crates. Validate each package with a
crates.io dry-run before publishing dependants.

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

Consumers choose a concrete nova-gfx backend explicitly:

```toml
gfx-vulkan = { path = "../nova-gfx/gfx-vulkan" }
```

Use `nova-gfx-vulkan`, `nova-gfx-dx11`, `nova-gfx-dx12`, `nova-gfx-opengl`, or `nova-gfx-metal`. The old
`nova-gfx` feature no longer selects a backend; this is a breaking compile-time
selection change intended to avoid compiling unused backend crates.

To verify the compile boundary, inspect the dependency tree for the selected
backend:

```powershell
rtk cargo tree -p gfx-vulkan --no-default-features
rtk cargo tree -p gfx-vulkan --no-default-features
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
are modeled as a backend-associated type on `SurfaceDevice`; concrete backend
crates decide which native presentation target they accept. The current Vulkan,
DX12, and Metal backends use `raw-window-handle` in their own crates because
native surface creation is a platform integration boundary, not a core trait
dependency.

## Boundary

nova-gfx is a graphics library, not a UI toolkit. Frame policy such as damage
collection, retained layers, frame scheduling, partial-present decisions, and
GPU work tracking belongs to the renderer consuming this crate. `gfx-core`
contains only graphics vocabulary: devices, queues, resources, pipelines,
commands, surfaces, and presentation primitives. It must not reference a UI
framework, a widget tree, or renderer-specific damage models.

The canonical frame entry points are descriptor-based:

```rust
use gfx_core::{
    ClearColor, LoadOp, PresentedFrame, PresentFrameDescriptor, RenderCommand,
    PresentationDevice, RenderPassId, SwapchainId,
};

fn present<D: PresentationDevice>(
    device: &mut D,
    swapchain: SwapchainId,
    render_pass: RenderPassId,
    commands: &[RenderCommand],
) -> gfx_core::Result<Option<PresentedFrame>> {
    device.render_and_present(&PresentFrameDescriptor {
        swapchain,
        render_pass,
        commands,
        color_load_op: LoadOp::Clear(ClearColor::default()),
        depth_attachment: None,
        present_region: None,
    })
}
```

Backend public inherent methods that duplicate the core traits are not the
maintained API. Import the relevant capability trait and call trait methods on
the concrete backend device.
