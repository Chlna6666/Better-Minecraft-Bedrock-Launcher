# gfx-opengl

Graphics passes skip unchanged uniform/storage buffer ranges, textures, and samplers. Binding-cache
storage is reused between passes but its entries reset at every pass boundary. Required mirror
copies still execute before each draw. Presentation reports host command, swap, signal, and
cleanup timings through `PresentTimings`.

[中文文档](README.zh-CN.md)

OpenGL backend crate for nova-gfx.

The shared buffer compute API uses cached compute programs and memory barriers before subsequent
consumers. Non-indexed indirect drawing is advertised only with `GL_ARB_shader_draw_parameters`,
which preserves the shared first-instance semantics. Explicit queue completion uses `glFinish`;
ordinary dispatch and draw submission does not wait for completion.

The native backend requires desktop OpenGL 4.5 core. It uses WGL on Windows and
EGL on Linux (X11 or Wayland), with a context and GPU resources owned by the
rendering thread. Linux display connections must outlive the device.

WGSL is translated to GLSL 4.50 at build time; the native driver compiles and
links GLSL when pipelines are created. Resource reflection maps logical bindings
to compact native UBO/SSBO slots and combined texture/sampler uniforms. Clip
control preserves a top-left origin and zero-to-one depth convention.

The backend supports buffers, BGRA/RGBA and depth textures, mip views, samplers,
indexed draws, uniform ranges, depth and dual-source blending, GPU fences,
readback and native window presentation. Initialization failures return errors
so callers can try another compiled backend. An adapter name verifies the
driver's selected renderer; GPU selection is controlled by the native driver.

Windows hardware gates (including pixel readback):

```powershell
cargo test -p gfx-opengl -- --ignored --nocapture --test-threads=1
```

Production glyph and image paths are validated by the consumer renderer with
its production shader artifacts and atlas encoders.
