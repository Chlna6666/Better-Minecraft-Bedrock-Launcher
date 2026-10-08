# gpui-3d

`gpui-3d` provides the reusable 3D scene, geometry, camera, material, animation, query, and GPUI
scene-view layer. Applications convert their domain data into these types. GPUI owns generic
renderer-extension scheduling and Nova integration; `gpui-3d` owns 3D scene preparation and its
scene-view resources; BMCBL-specific world and preview policy stays in `src/ui`.

The API is in active development and the crate is not published. Map Viewer and Skin Pack use this
crate for preview scenes. GPUI owns the generic `RendererExtension` lifecycle; its dedicated mesh
primitive and Nova upload/cache/draw path have been removed without compatibility wrappers. The
crate and BMCBL checks pass, and the CPU scene example runs. Native DX12 and Vulkan smoke captures
show the scene geometry and the three shared-resource sphere instances after fixing the scene-view
element to fill its available layout bounds; frame pacing and GPU draw counts remain unmeasured.

## Current scope

- Indexed meshes with material parts, area-weighted normal generation, MikkTSpace tangent
  generation with mirrored-UV seam splitting, optional per-triangle edge
  masks, fixed-topology vertex snapshots through `Mesh::with_vertices`, and cube, plane,
  UV-sphere, cylinder, and cone primitives.
- Parent/child scene nodes with translation, quaternion rotation, scale, pixel offset, and depth bias.
- Ambient, directional, point, and soft-cone spot lights; metallic-roughness and unlit materials;
  sRGB albedo, linear normal, and red-channel occlusion maps; opaque, masked, and premultiplied blend modes.
- Camera-frustum culling, back-to-front ordering for blended draws, retained per-scene-view GPU mesh
  resources, and BVH-accelerated ray queries.
- Automatic indexed instancing for adjacent opaque or masked draws that share the same mesh part
  and complete material values. `PreparedScene::draw_batches()` exposes the grouping without
  allocation; blended draws remain separate to preserve back-to-front order.
- Absolute-time translation, scale, and rotation tracks. `Scene::evaluate` leaves the authored
  scene untouched and gives rendering preparation and picking one consistent pose; repeated CPU
  samples can reuse `AnimationScratch` through `Scene::evaluate_with`.
- A GPUI element backed by Nova indexed rendering that fills its available layout bounds.
- One-pixel barycentric coverage smoothing for selected triangle edges. Meshes with any selected
  edges expand to triangle-local vertices at GPU upload; unmasked meshes keep shared indexed data.
- Selectable projection into element bounds or the visible rectangular content-mask intersection,
  a centered square fit for clipped previews, responsive per-axis insets, and optional linear
  pixel-width feathering for blended draws.
- Perspective and orthographic cameras with validated orbit, pan, zoom, and scene-bounds framing.
  `Scene::bounds()` and `EvaluatedScene::bounds()` report authored or sampled world-space mesh
  bounds; `OrbitCamera::fit_bounds()` centers the camera target on those bounds. Viewport-local
  picking reuses animation and traversal storage through `SceneViewRaycastScratch`.

Texture data is supplied as immutable RGBA8 `TextureAsset`s and referenced by material asset ID.
Spotlight cone angles are validated radians (`0 <= inner < outer < PI`). Node transforms move the
light pose and scale its range. Light intensity fades smoothly from the inner cone to zero at the
outer cone, in addition to range attenuation. The scene view uploads supplied mip levels and defaults
to linear texel/mip filtering with clamp-to-edge addressing. `SceneView::with_anisotropy(true)` opts
into hardware anisotropic filtering; Vulkan uses the adapter's reported limit and falls back to the
configured filters when sampler anisotropy is unavailable. Occlusion maps use mesh UVs and reduce ambient lighting according to the
material strength; they do not affect direct lights. Create them with `TextureAsset::linear_rgba8`.
The renderer rejects sRGB occlusion maps. Nova supports texture uploads on DX11, DX12, Vulkan and OpenGL 4.5. Metal shader compilation
is tested, but `gfx-metal` does not upload texture pixels.

Normal-map assets also use linear RGBA8; the renderer rejects sRGB normal maps. A lit metallic-roughness material with a normal map requires `Mesh::tangents()`;
generate frames from UV set zero with `Mesh::generate_tangents()`, or attach imported frames with
`Mesh::with_tangents()`. Generation follows MikkTSpace and splits vertices at tangent seams while
preserving triangle order and returning a source-vertex map.

The crate accepts caller-generated mip chains but does not generate them. It does not yet provide
shadows or HDR environment lighting. It also lacks custom material programs and mesh passes, IK,
morph and skin deformation, headless capture, and asynchronous readback. Shader compilation and CPU
benchmarks do not prove native pixel correctness or frame pacing.

## Examples

Run the CPU scene, animation, preparation, and picking example:

```powershell
cargo run -p gpui-3d --example scene
```

Run the native scene-view example with a compiled Windows renderer:

```powershell
cargo run -p gpui-3d --example scene_view --features native -- --backend=nova-dx12
cargo run -p gpui-3d --example scene_view --features native -- --backend=nova-vulkan
cargo run -p gpui-3d --example scene_view --features native -- --backend=nova-dx11
cargo run -p gpui-3d --example scene_view --features native -- --backend=nova-opengl
```

The CPU scene example replaces vertex attributes through `Mesh::with_vertices`, leaving the source
snapshot intact while rebuilding bounds and query data. It also generates normals and tangents for an
imported triangle and reuses `AnimationScratch` across absolute-time samples. It gets bounds from the
evaluated scene pose, fits an orthographic camera, applies orbit, pan, and zoom, then runs a scene ray
query and a viewport-local pixel query.

The native window example renders a two-level sRGB albedo chain on a normal-mapped metallic cube,
with generated MikkTSpace tangents, and a PBR sphere. Three small spheres share one mesh and material and exercise indexed
instancing. It also demonstrates point and spot lights, a linear occlusion map, an unlit
marker, alpha blending, selected triangle-edge smoothing,
pixel offset, depth bias, and a fixed animation sample. It also shows responsive insets, visible-content
projection, and a one-pixel edge fade for blended surfaces. `ProjectionRegion::VisibleSquare` fits a
centered square inside the visible preview region.
Rounded content-mask coverage remains provided by the host scissor only as a rectangle. Triangle
edge coverage and projection-edge fade apply to blended draws only.
`SceneView::with_animation` attaches validated scene-node tracks at an absolute clip time;
`SceneView::with_animation_time` selects another sample. `SceneView::raycast` uses the same sampled
pose as render preparation and `SceneViewRaycastScratch` retains query and animation storage between events. The
window example uses a fixed sample; it does not implement autonomous playback. `with_camera`,
`with_scene`, `with_textures`, `with_projection_region`, `with_projection_inset`, and
`with_blend_edge_feather` update immutable snapshots while retaining scene-view identity and compatible GPU resources. `Mesh::with_edge_masks` creates a fresh mesh identity; GPU upload expands only meshes with a non-empty mask. `Node::with_pixel_offset` and `Node::with_depth_bias` affect rasterization while leaving world-space bounds and CPU queries unchanged. Replacing the scene
clears its scene-bound tracks. For `VisibleContent`, viewport-local raycast coordinates must use the
same rectangle as the render projection.

`Mesh::with_vertices` is a fixed-topology CPU snapshot replacement: vertex count stays constant,
indices, material parts, and edge masks stay unchanged, and mesh/part bounds plus the ray-query BVH
are rebuilt. It clears tangent frames, which must be regenerated or reattached before normal mapping.
Each replacement has a fresh mesh identity and receives a complete GPU geometry upload when
submitted; partial in-place GPU updates are not currently supported.

`TextureAsset::rgba8` and `TextureAsset::linear_rgba8` create single-level textures.
`TextureAsset::rgba8_mip_chain` and `TextureAsset::linear_rgba8_mip_chain` accept level zero first,
followed by floor-halved extents down to `1x1`; partial chains are valid. Pixel decoding, color-space
conversion, and mip generation remain caller responsibilities. Nova uploads the supplied chain in
one batch on DX12 and Vulkan. Metal texture uploads are not implemented.

## Benchmarks and checks

```powershell
cargo bench -p gpui-3d --bench raycast
cargo bench -p gpui-3d --bench scene
cargo test --locked -p gpui-3d
cargo check --locked -p gpui-3d --all-targets --all-features
cargo test --locked -p gfx-dx12 --test texture_transfer
cargo test --locked -p gfx-vulkan --test texture_transfer
cargo bench --locked -p gfx-dx12 --bench texture_write -- mip-chain
cargo bench --locked -p gfx-vulkan --bench texture_write -- mip-chain
```

The Criterion benches measure CPU BVH ray queries, authored and evaluated scene bounds, scene
preparation and retained-capacity reuse, animated transform evaluation plus prepared-scene update,
draw-batch discovery for 100, 1,000, and 10,000 repeated draws, built-in primitive generation,
fixed-topology vertex snapshot rebuilding, and MikkTSpace tangent generation for a 64-by-32 UV
sphere. Run an individual group with:

```powershell
cargo bench -p gpui-3d --bench scene -- scene/batching
cargo bench -p gpui-3d --bench scene -- scene/bounds
cargo bench -p gpui-3d --bench scene -- vertex-snapshot/uv-sphere-64x32
cargo bench -p gpui-3d --bench scene -- generate-tangents/uv-sphere-64x32
```

The `gpui-3d` Criterion benches measure CPU work and do not measure GPU presentation latency. The
Nova `mip-chain` cases measure batched upload cost for a 128x128 RGBA8 texture with eight levels;
they report native GPU copy timing when timestamp support is available, not frame pacing.

See [the GPUI 3D architecture and API guide](../../docs/GPUI_3D.md) for ownership, coordinate
conventions, materials, resource lifetime, examples, benchmark interpretation, and the backend
validation matrix.

The application-side adapters are [Map Viewer](../../src/ui/window/map_viewer/preview_3d.rs) and
[Skin Pack](../../src/ui/window/skin_pack/mesh.rs); keep Minecraft-specific conversion in those
application modules.
