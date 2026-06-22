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
let shader = source.compile(surface.device());
```

For one-shot compilation, use the helper functions:

```rust
let shader = gpui::compile_wgsl_shader_module_from_path(
    surface.device(),
    "examples/viewer.wgsl",
)?;
```

Generated or embedded shader strings can use a source label:

```rust
let shader = gpui::compile_wgsl_shader_module(
    surface.device(),
    "generated-material-shader",
    generated_wgsl,
)?;
```

The loader validates WGSL with `naga` before creating the Nova shader module.
File read errors include the path. Parse and validation errors include the
provided label or path and a formatted WGSL diagnostic.

## Integration With GPU Surfaces

Runtime WGSL is normally paired with `Window::paint_gpu_mesh_3d`:

1. Create a GPUI-managed GPU surface.
2. Compile the shader with the surface device.
3. Build bind groups, pipelines, buffers, and textures from the same device.
4. Render into `GpuSurfaceHandle::back_buffer_view`.
5. Present or swap the rendered buffer and paint the surface into the GPUI
   scene.

The custom render pipeline's color target must match the surface texture
format.

## Error Handling

Treat shader loading as fallible application setup:

- Return or display file system errors with the source path.
- Surface parse and validation diagnostics to the user or developer log.
- Rebuild dependent pipelines when the shader or surface format changes.
- Keep runtime shader errors out of GPUI renderer internals unless the shader is
  part of the framework renderer.

## Example

`hatsune_miku_viewer` demonstrates the full path on Windows:

- load WGSL from an example shader file;
- parse OBJ and MTL files with `tobj`;
- load material textures with `image`;
- render per-material submeshes into a GPUI-managed GPU surface;
- support mouse drag rotation, wheel zoom, and resize.

Set `GPUI_HATSUNE_MIKU_DIR` to point it at an OBJ asset directory.

```powershell
cargo run --example hatsune_miku_viewer
```
