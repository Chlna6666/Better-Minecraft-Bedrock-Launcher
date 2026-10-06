# Runtime WGSL Shaders

[Chinese](runtime_wgsl_shaders.zh-CN.md)

GPUI validates and embeds built-in renderer WGSL at build time. Applications and
examples that own custom Nova GPU rendering can also load and validate WGSL at
runtime before creating shader modules.

Runtime shader loading is intended for model viewers, visualizers, game views,
custom material systems, and other features that need shader code outside
GPUI's built-in elements.

## Loading WGSL

Use `WgslShaderSource` when validated shader source should be retained:

```rust
let source = gpui::WgslShaderSource::from_path("examples/viewer.wgsl")?;
```

Load a file through the associated constructor:

```rust
let shader = gpui::WgslShaderSource::from_path("examples/viewer.wgsl")?;
```

Generated or embedded shader strings can use a source label:

```rust
let shader = gpui::WgslShaderSource::from_source(
    "generated-material-shader",
    generated_wgsl,
)?;
```

The loader validates WGSL with `naga` before application rendering code creates
backend shader modules. File read errors include the path. Parse and validation
errors include the provided label or path and a formatted WGSL diagnostic.

## Integration With Renderer Extensions

Runtime WGSL belongs to the crate that implements custom GPU rendering. A GPUI
`RendererExtension` can create its pipelines and buffers from the extension
device, then append draw steps to the host render pass:

1. Load and validate WGSL with `WgslShaderSource`.
2. Cross-compile or translate it for the selected backend.
3. Build bind groups, pipelines, buffers, and textures in the extension renderer.
4. Return ordered `RenderStepDescriptor`s from `RendererExtensionRenderer::render`.
5. Let GPUI apply the element scissor and submit those steps at the extension's
   scene position.

The pipeline's color target must match `RendererExtensionContext::color_format`.
Extension callbacks run synchronously on the renderer owner; they must not do
blocking IO or call back into application UI state. GPUI owns window and render
pass lifetime, while the extension implementation owns its GPU resources.

## Error Handling

Treat shader loading as fallible application setup:

- Return or display file system errors with the source path.
- Surface parse and validation diagnostics to the user or developer log.
- Rebuild dependent pipelines when the shader or surface format changes.
- Keep runtime shader errors out of GPUI renderer internals unless the shader is
  part of the framework renderer.

## 3D examples

The old standalone `hatsune_miku_viewer` example used GPUI's removed mesh and
surface path. Reusable 3D examples now live in the `gpui-3d` crate; they exercise
the viewport API, while `WgslShaderSource` remains a generic validation helper.

```powershell
cargo run -p gpui-3d --example scene
cargo run -p gpui-3d --example scene_view --features native -- --backend=nova-dx12
cargo run -p gpui-3d --example scene_view --features native -- --backend=nova-vulkan
```

See [`gpui-3d`'s architecture guide](../../../docs/GPUI_3D.md) for the 3D/API boundary and current
backend validation status.
