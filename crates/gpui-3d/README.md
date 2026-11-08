# gpui-3d

`gpui-3d` provides the reusable 3D scene, geometry, camera, material, animation, query, and GPUI
scene-view layer. Applications convert their domain data into these types. GPUI owns generic
renderer-extension scheduling and Nova integration; `gpui-3d` owns 3D scene preparation and its
scene-view resources; BMCBL-specific world and preview policy stays in `src/ui`.

The API is in active development and the crate is not published. Map Viewer and Skin Pack use this
crate for preview scenes. GPUI owns the generic `RendererExtension` lifecycle; its dedicated mesh
primitive and Nova upload/cache/draw path have been removed without compatibility wrappers.

Two rendering paths share one scene/asset model:

The GPUI scene view selects rasterizer culling from `Material::double_sided` and `front_face`.
Single-sided materials cull back faces; double-sided materials render both sides. Mirrored
transforms reverse front-face winding and are batched separately. The authored winding defaults
to `FrontFace::Ccw`; clockwise meshes must explicitly use `FrontFace::Cw`. This selection currently
applies to the scene view; the offscreen PBR path retains its previous behavior.

- **GPUI scene view**: renders inside the host compositor pass through `RendererExtension`.
- **Offscreen renderer**: `OffscreenRenderer` renders a directional shadow pass, a linear PBR scene
  pass with optional image-based lighting and sky, and a display-referred post pass into an offscreen
  target with readback, resize, trim, debug views, deferred completion polling, and per-frame
  statistics. It uses an application-owned Nova device and adds no second window renderer, swapchain,
  cache owner, or render queue. Distance-selected `LodLevel` chains are resolved during scene
  preparation.

`Mesh::from_voxel_quads_without_raycast` builds compact voxel geometry from ordered quad vertices
and material parts without a temporary full vertex or index buffer. It preserves the implicit
two-triangle topology and skips mesh raycasts. The GPUI scene view uploads its compact vertices
without an index buffer and expands each quad into two triangles in the vertex shader; other
mesh topologies keep indexed draws.
`Mesh::same_voxel_quad_geometry` compares this packed representation exactly for update reuse.

On devices advertising buffer compute and indirect draws (DX11, DX12, Vulkan and eligible OpenGL drivers), voxel scene views
retain camera-independent opaque draw records. Compatible opaque quad ranges share indirect
instanced batches by resident page, material and vertex-count class. A compute shader tests bounds
and compacts visible instance indices. Each workgroup shares its frustum planes and accumulates
counts in workgroup memory before publishing one indirect record. Single-instance batches skip
workgroup barriers. GPU-generated meshes feed their actual counts into the same culling output;
they no longer repeat frustum tests on the CPU. Indirect vertex counts use the largest visible part, so hidden
larger parts do not add padding work. The voxel vertex shader expands quads and clips padding for
variable-size ranges. Camera changes update frame uniforms and dispatch compute; they do not
rebuild an opaque CPU visible list. Blended draws keep CPU depth ordering and individual batches.
DX11/DX12/Vulkan scene parameter buffers use device-local ordered writes so pending GPU frames
cannot observe later CPU mapped updates. Unchanged culling bounds skip their uniform upload.
Camera-only blended sorting orders compact indices and reorders retained bindings and draw
addresses only when depth order changes; already ordered draws keep an identity permutation.
Material, instance and culling input tables remain resident; only frame uniforms and
compute outputs change. Indexed fallbacks retain their original instance addresses as well.
Other backends retain CPU culling/direct draws. Non-procedural mesh ranges retain their CPU
frustum test before direct indexed submission.

`Mesh::from_gpu_voxels` accepts a padded `VoxelMeshSource` grid and shared six-face templates.
On compute/indirect-capable DX11, DX12, Vulkan and OpenGL devices, compute workgroups remove
neighbor-occluded faces and emit compact face descriptors and per-material vertex counts. The
vertex shader expands these descriptors directly; production rendering does not read counts
back to the CPU. Generation runs when mesh geometry changes, not on camera movement. Optional
static faces retain their order within each material after the generated cube faces.
`VoxelMeshSource::with_instances` accepts reusable arbitrary face templates, cell positions,
vertical scales and neighbor-class masks. Parallel count, per-material prefix-scan and emit
passes compact visible model/fluid faces while retaining instance order for blended materials. Mesh
generation uploads immutable parameters once and queues its stages through the ordered compute
batch API. Generated face descriptors use 16 bytes; common 8-bit neighbor masks are packed four
per word on the CPU and GPU, while wider masks retain the full 32-bit representation.
Retained cell model indices use bytes for IDs below 255, with a reserved halo sentinel; larger
IDs retain 32-bit storage. Upload encoding and CPU fallback preserve the original values.
Scene and culling input tables use batched uploads when updated.
Draw mappings keep bounded per-source buffers and skip unchanged mappings; output-buffer
replacement invalidates the cached copy even when mapping entries are unchanged. Mesh
counts and parts describe output capacities, rather than the GPU's final visible face counts.
Other render paths lazily generate padded CPU geometry. Voxel meshes do not support raycasts.
World decoding, cell/template extraction, connected-model state resolution, LOD selection and
material resolution remain CPU work. Applications may retain CPU generation for entity geometry;
there is no independent asynchronous compute queue.
`GPUI_3D_TRACE_TIMINGS` reports `gpu_cull_batches_max` and `gpu_resident_draws_max` alongside draws.
In development builds, `gpui3d gpu_owner_phase` reports CPU host/API latency while
`gpui3d gpu_hardware_phase` reports **completed native GPU timestamps** with the
Section publication `batch_id`. The latter measures GPU Generated Mesh compute
and resident/upload command windows on DX11, DX12, Vulkan and OpenGL, without
waiting for GPU fences. Host command encoding and native GPU execution are distinct.
Independent DrawIndirect hardware timings and production performance speedup
numbers have not yet been established.

`Scene::fork` shares copy-on-write node pages and records changed root leaves while preserving
previous snapshots and stable node/mesh/material IDs. Consecutive snapshots prepare only changed
nodes and update asset reference counts incrementally; hierarchy, LOD, animation and skipped
revision changes fall back to full preparation. Resident draw/instance/light and culling tables upload only
the changed record range; camera updates reuse geometry and asset tables.

GPU voxel culling includes a current-camera, buffer-backed 128x128 Hi-Z pyramid. Only explicitly
certified `Node::occluders` opaque box faces contribute depth; arbitrary mesh bounds do not.
Face masks preserve neighbor/height face removal, merged columns retain only actual outer faces,
and back faces do not contribute. Rotated, reflected or sheared proxies are omitted.
At most 512 nearby volumes are rasterized conservatively, including a maximum-depth hierarchy.
Near-plane intersections, biased/offset draws and incomplete tile coverage remain visible.
Removed volumes clear old depth. This path uses ordered compute without host depth-texture access
or GPU readback; it is not a copy of the Bedrock renderer or a measured FPS guarantee.

`SceneView::with_culling_bounds` updates an optional world-space culling region without replacing
the scene. Nodes opt in with `Node::with_culling_origin`; the region uses inclusive minimum and
exclusive maximum coordinates. Ray queries and other nodes are unaffected.

`SceneView::with_continuous_presentation` opts a view into compositor-owned presentation frames
even when its scene is unchanged. The window frame clock limits the rate, and these frames reuse
the committed GUI view and layout. This option is disabled by default.
`SceneView::with_camera_sampler` also enables continuous presentation and samples the camera
on the renderer thread for each 3D frame. Input can change shared camera state without a GPUI
view rebuild; the application remains responsible for synchronizing tools and streaming state.

`VoxelMeshSource::with_ambient_occlusion` opts into corner AO for unit cell-boundary faces.
The supplied mask selects full opaque instance classes; base cube grids treat occupied cells
as opaque. GPU generation packs four corner factors into the existing 16-byte face descriptor;
CPU fallback uses the same side/diagonal rule. Arbitrary detail and static faces retain their
supplied lighting. `voxel_ambient_light` exposes this rule to CPU mesh producers.
`SceneView::with_fog` accepts validated `DistanceFog` parameters, disabled by default.
Fog updates the shared frame uniform without rebuilding scene assets, preserves alpha masking
and premultiplied blending, and applies to both voxel and regular scene-view materials.
`VoxelMeshSource::with_lighting` attaches packed sky/block levels for the padded cell grid.
GPU vertex shading and CPU fallback use the same face sampling and ambient floor, independently
of corner AO, including static detail faces. `SceneView::with_voxel_shading` controls textured,
vertex-color and quad-wireframe views, full-bright/vertex/PBR illumination and daylight without
rebuilding scene assets. Wireframe fills face depth and retains Hi-Z occlusion to hide deeper edges.
`Occluder` may describe a planar rectangle: each selected face must be fully opaque geometry.
Unused bounds faces never occlude; rotation, reflection and shear exclude unsupported proxies.
`PbrTextureLayout` supports both glTF roughness/metallic and metallic/emissive/roughness channels.
PBR voxel normals use UV derivatives; they do not require a separate tangent stream.
`VoxelShading::color_space` distinguishes display bytes in linear voxel textures from
already-decoded linear material inputs, avoiding double sRGB decoding.

`SceneView::with_screen_effects(Some(ScreenEffects::default()))` opts into HDR scene capture,
material-aware geometry shadows, screen-space reflections, nearest-surface transmission/absorption,
height fog and textured cloud volumes. `SurfaceKind::Water` selects animated water normals and
water absorption. `SurfaceKind::Glass` explicitly selects solid transmission for `AlphaMode::Blend`; alpha blending alone
keeps ordinary diffuse coverage and does not imply refraction or a polished material. Disabling
`ScreenEffects::reflections` disables both SSR and the sky/environment reflection fallback.
Cloud RGB follows `TextureAsset` color-space metadata; alpha supplies coverage.
`water_texture` and `water_flow_texture` accept `FlipbookTexture` vertical square-frame strips.
Water frame selection and world-aligned texture sampling run in the screen resolve, sharing the
presentation clock with wave normals without recapturing static geometry. Frames use uniform timing;
the application resolves pack-specific ordering. Rough dielectric surfaces omit sharp reflections;
HDR voxel capture leaves environment reflection to the screen resolve to avoid counting it twice.
Finite shadow-map coverage fades at its bounds; this does not extend shadow visibility beyond
the map or replace cascaded shadow maps.
Water normals use four rotated, decaying world-space wave octaves with subpixel detail filtered
out. Horizontal waves do not affect water sides. Refraction projects a bounded world-space
transmission path and rejects foreground samples, rather than applying a fixed screen offset.
This independent image-space approximation does not implement the full original water simulation
or resource-driven wave parameters.
Clouds attenuate sunlight on surfaces and in fog. The 1024×1024 material shadow map spans 192 world
units and applies the same alpha cutoff as the visible geometry, including cutout/detail models.
The shadow projection is cached independently from the main camera's indirect visibility list.
PCF compares each tap against the continuous geometric receiver plane to avoid sloped-surface
self-shadowing. Normal maps do not change that plane; static frames do not accumulate shadows.

Scene geometry and material resources are shared by the capture and shadow passes. After the host
confirms submission, unchanged capture attachments are reused until camera, scene, material or
viewport changes. Cloud/water time is sampled on GPUI's presentation owner without rebuilding UI
layout. Internal resolution scales from 0.25 to 1.0, capped at 4,194,304 pixels. Rays use fixed step
limits; tiled local lighting uses 16-pixel tiles with an exact full-light fallback on overflow.
`Light::Spherical` carries source radius and an optional diffuse-only lobe. Local-light occlusion
and reflections are screen-space approximations and cannot see all offscreen geometry. Only the
nearest translucent surface is resolved. Cascaded shadows, full biome/weather behavior, height-map
conversion, subsurface scattering and a complete post-processing stack are not implemented.

`VoxelMeshSource::compact_mesh` emits only visible CPU quads, preserving per-material face order
and reusing the resident mesh arena. It allocates no capacity padding and retains no dynamic cell
light field. The world editor uses this path for default full-bright night vision, with light solving,
AO, fog and sun shadows disabled. Ordinary rendering retains GPU meshing and dynamic light fields,
using `bedrock-light`, face-direction light and AO. Experimental Vibrant
Visuals rendering supplies same-pack Texture Set color/normal/MER, resource-authored local-light
colors, two-block emitter clusters and the optional screen effects. The application owns Bedrock
resource, dimension and preset policy; these are not GPUI or Nova backend responsibilities. This
is an independent compatible preview, not the official RenderDragon implementation.
Night/day controls alter the shared frame uniform; a full cycle takes 20 real minutes. Streaming
event submission uses a soft 2 ms / 8 MiB estimated-payload budget between events (one event may
exceed it); hover picking runs at most 30 Hz, while explicit edit clicks still pick immediately.
These bounds are not measured FPS improvements or a hard GPU-upload budget. Large-world hardware
benchmarks and state-complete Bedrock lightmaps remain necessary.

The voxel mesh arena keeps retired ranges unavailable while GPU work may still read them. During
host memory trim, DX11, DX12 and Vulkan wait for queue completion before releasing
those ranges for reuse; this does not provide per-frame recycling or a GPU memory budget.

`load_path` / `load_bytes` import glTF 2.0 and GLB documents into the same scene types, including
PBR materials, decoded textures with mip chains, node hierarchies, punctual lights, cameras, and
transform animations. Licensed test models with pinned SHA-256 hashes live in `test_assets/`.

The offscreen path is implemented for DX12, DX11, Vulkan, and OpenGL 4.5. GPU timestamped pass
timings, skeletal/morph deformation, prefiltered cubemap irradiance probes and a real BRDF LUT,
true asynchronous GPU-to-CPU readback, and compositor hand-off of an offscreen target without CPU
readback remain unimplemented; see the crate rustdoc and
[architecture](../../docs/ARCHITECTURE.en.md) for ownership and backend status.

Memory behavior is bounded and observable: the renderer reports resident, peak, upload, cache-hit,
and eviction counters; incremental uploads are tracked per element with content hashes instead of a
full CPU copy; `set_memory_budget` evicts idle assets after a frame; texture import caps and
downscales oversized images; and the default shadow map is deliberately small. Run the benchmark
suite for current numbers instead of relying on README figures.

Static frames are retained: an unchanged scene, camera, and options return the last frame with zero
CPU preparation, uploads, or GPU submission, while a camera-only move re-culls without walking the
scene graph. Instanced batching writes one draw slot per batch, so large repeated cube fields render
as a single indexed draw with one instance per element instead of per-element streaming buffers.
Long-session budget cycling and two-renderers-on-one-device isolation are covered by ignored native
memory tests.

Rendering resolves in linear HDR: the scene pass writes `Rgba16Float`, a half-resolution bloom pass
bleeds bright emissive/specular light, and the post pass applies exposure, tone mapping, FXAA-style
edge blending, vignette, film grain, and ordered dithering into the readback target. Debug views and
object-id selection bypass grading. `RenderQuality` presets (Low/Medium/High/Ultra) select shadow
resolution, bloom strength, FXAA, and grading; overrides stay available per option. Per-frame CPU
encoders reuse staging buffers, evicted mesh buffers enter a bounded reuse pool (reuse counters are
exposed by the statistics API), and pipeline layouts are destroyed with the
renderer instead of leaking on every drop/resize. Allocator metrics are exposed through
`buffer_allocation_count`, `buffer_slack_bytes`, `pooled_buffer_bytes`, `pooled_buffer_count`,
`buffer_reuses`, and `cpu_staging_bytes`.

## Offscreen renderer quick start

```rust
// The application owns the Nova device created by its backend of choice.
let mut device = /* gfx_dx12::Dx12Device::new(&DeviceDescriptor::default())? */;
let mut renderer = OffscreenRenderer::new(&mut device, 1280, 720)?;
let image = renderer.render_to_image(&mut device, &scene, &textures, camera, &options)?;
// image.rgba() is display-referred RGBA8; save it or inspect it directly.
renderer.destroy(&mut device)?;
```

`RenderOptions` selects shadows (resolution/bias/strength), exposure, tone mapping (ACES, Reinhard,
none), debug views (albedo, normal, depth, UV, shadow visibility, object id), the clear color, and
post grading. `OffscreenRenderer::with_render_scale` / `set_render_scale` trade HDR resolution for
fill rate (`0.5`) or supersample it (`2.0`). `render_animated` samples caller-owned
`TransformTrack`s at a caller-owned clip time and `render_blended` crossfades two such clips;
`gpui-3d` owns no playback clock. After rendering, `poll_render` reports deferred DX12 GPU completion
(`Some(true)`/`Some(false)`, or `None` on backends without a handle) so CPU work can overlap before
`capture`; capture itself stays a synchronous readback.

Image-based lighting is attached to the scene rather than to render options:

```rust
let environment = std::sync::Arc::new(gpui_3d::EnvironmentMap::procedural_studio());
scene.set_environment(Some(environment)); // Scene::set_environment(None) restores the analytic fallback
```

`EnvironmentMap::load_path` accepts an sRGB equirectangular PNG/JPEG, and
`EnvironmentMap::new(Arc<TextureAsset>, intensity, rotation)` accepts caller-provided linear or sRGB
texture data (a complete mip chain improves diffuse/roughness filtering). A fullscreen sky triangle
renders the same map as background; debug and object-id views skip it. Environment textures are
cached by asset id, included in statistics, invalidated when the map changes, and released by
aggressive trim.

Render LOD is per node:

```rust
let near = Arc::new(gpui_3d::Mesh::uv_sphere(0.5, [64, 32])?);
let far = Arc::new(gpui_3d::Mesh::uv_sphere(0.5, [16, 8])?);
scene.node_mut(node)?.lod = vec![
    gpui_3d::LodLevel::new(near.clone(), 12.0)?,
    gpui_3d::LodLevel::new(far, 120.0)?,
];
```

LOD selection uses camera-to-node distance and disables the camera-only retained fast path so moving
the camera re-walks and re-selects. `Node::mesh` remains the bounds/raycast representation; keep the
highest-detail mesh there. The lab exposes IBL with `--env-map=studio` or an image path.

## glTF import quick start

```rust
let imported = gpui_3d::load_path("model.glb", &GltfLoadOptions::default())?;
let image = renderer.render_to_image(&mut device, &imported.scene, &imported.textures, camera, &options)?;
if let Some(clip) = imported.animations.first() {
    renderer.render_animated(
        &mut device,
        &imported.scene,
        &clip.tracks,
        std::time::Duration::from_millis(350),
        &imported.textures,
        camera,
        &options,
    )?;
}
```

## Picking

`pick` / `pick_evaluated` raycast from viewport pixels (stable node/mesh ids, distance, position,
normal, UV) and `project` maps world points back to pixels. `OffscreenRenderer::pick_object` decodes
the GPU object-id frame through a synchronous readback and returns the submitted draw, so CPU and GPU
selection agree on pose and culling. The lab exposes both through `--pick=x,y`.

## Test assets

```powershell
pwsh -NoProfile -File crates/gpui-3d/test_assets/download.ps1
```

The downloader verifies every SHA-256 pinned in `test_assets/manifest.toml`; a missing or mismatched
file is an error. Once cached, the import and GPU tests run offline. Native pixel tests are ignored by
default and run with `cargo test -p gpui-3d -- --ignored --test-threads=1`.

## Current scope

- Indexed meshes with material parts, area-weighted normal generation, MikkTSpace tangent
  generation with mirrored-UV seam splitting, optional per-triangle edge
  masks, fixed-topology vertex snapshots through `Mesh::with_vertices`, and cube, plane,
  UV-sphere, cylinder, and cone primitives.
- Parent/child scene nodes with translation, quaternion rotation, scale, pixel offset, depth bias, and
  per-node/subtree visibility that rendering, shadows, picking, and bounds all respect.
- CPU two-clip animation crossfades through `blend_transform`, `Scene::evaluate_blended`, and
  `OffscreenRenderer::render_blended`; nodes animated by only one clip fade toward rest.
- Render-scale control on the offscreen renderer: `0.25..=2.0` internal HDR resolution for dynamic
  resolution or supersampling while the post target, readback, and picking viewport stay at the
  requested output size.
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

The GPUI scene-view path does not yet cast shadows or use HDR environment lighting; the offscreen
renderer does provide directional shadows, equirectangular mip-prefiltered IBL with a sky background,
distance LOD, deferred DX12 completion polling, linear HDR, bloom, FXAA, and render scale. Still
missing: prefiltered cubemap/irradiance probes and a real BRDF LUT, point/spot shadows, MSAA/TAA,
SSAO/SSR/motion blur/depth of field, screen-space/HLOD and layer/culling masks, skeletal/morph
deformation, glTF material extensions (clearcoat/transmission/ior/variants), texture transforms/UV1,
compressed texture/mesh extensions, GPU timestamps, GPU-driven indirect rendering, true asynchronous
GPU-to-CPU readback, and compositor hand-off without CPU readback. Metal texture upload and runtime
validation remain unimplemented. Shader compilation and CPU benchmarks do not prove native pixel
correctness or frame pacing.

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

See [the workspace architecture](../../docs/ARCHITECTURE.en.md) for ownership, coordinate
conventions, materials, resource lifetime, examples, benchmark interpretation, and the backend
validation matrix.

The application-side adapters are [Map Viewer](../../src/ui/window/map_viewer/preview_3d.rs) and
[Skin Pack](../../src/ui/window/skin_pack/mesh.rs); keep Minecraft-specific conversion in those
application modules.
