# gfx-dx11

Native Windows Direct3D 11 backend for nova-gfx, requiring hardware feature
level 11.0. It uses D3D11 immediate-context commands, DXGI flip-model swapchains
and DirectComposition for premultiplied window surfaces.

Production WGSL is compiled to Shader Model 5.0 bytecode at build time. Fixed
resource slots preserve the shared rendering contracts without changing the
DX12 shader path. Draw offsets use the reserved internal constant buffer b13;
application uniforms use b0 through b12.

The backend supports sampled BGRA/RGBA textures, mip views, shader-pulled
vertices, indexed draws, uniform ranges, depth and dual-source blending, GPU
completion queries, native pacing and readback. Resize retains the native chain;
opaque/premultiplied transitions prepare a replacement chain. Format changes
require a new surface.

Hardware gates:

```powershell
cargo test -p gfx-dx11 -- --ignored --nocapture --test-threads=1
cargo test -p gpui --no-default-features --features nova-gfx-dx11 --lib production_glyphs_and_images -- --ignored --nocapture --test-threads=1
```

The GPUI gate uses production shader artifacts, atlas formats and encoders,
sprite packing and layouts to verify monochrome/subpixel glyphs and images.
