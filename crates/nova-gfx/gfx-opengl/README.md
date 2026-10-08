# gfx-opengl

OpenGL backend crate for nova-gfx.

The native backend requires desktop OpenGL 4.5 core. It uses WGL on Windows and
EGL on Linux (X11 or Wayland), with a context and GPU resources owned by the
rendering thread. Linux display connections must outlive the device.

WGSL is translated to GLSL 4.50 at build time; the native driver compiles and
links GLSL when pipelines are created. Resource reflection maps logical bindings
to compact native UBO/SSBO slots and combined texture/sampler uniforms. Clip
control preserves GPUI's top-left origin and zero-to-one depth convention.

The backend supports buffers, BGRA/RGBA and depth textures, mip views, samplers,
indexed draws, uniform ranges, depth and dual-source blending, GPU fences,
readback and native window presentation. Initialization failures return errors
so callers can try another compiled backend. An adapter name verifies the
driver's selected renderer; GPU selection is controlled by the native driver.

Windows hardware gates (including pixel readback):

```powershell
cargo test -p gfx-opengl -- --ignored --nocapture --test-threads=1
```

GPUI's production glyph/image paths have a separate gate:

```powershell
cargo test -p gpui --no-default-features --features nova-gfx-opengl --lib production_glyphs_and_images -- --ignored --nocapture --test-threads=1
```
