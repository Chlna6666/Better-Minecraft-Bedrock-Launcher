from pathlib import Path


def read(path: str) -> str:
    return Path(path).read_text(encoding="utf-8")


def write(path: str, text: str) -> None:
    Path(path).write_text(text, encoding="utf-8")


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected exactly one match, got {count}")
    return text.replace(old, new, 1)


# -----------------------------------------------------------------------------
# Public scene API: immutable RGBA texture + per-draw resources.
# -----------------------------------------------------------------------------
path = "crates/gpui/src/scene/mesh.rs"
s = read(path)
s = replace_once(
    s,
    "pub struct GpuMesh3dShaderId(pub usize);\n",
    """pub struct GpuMesh3dShaderId(pub usize);\n\n#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]\n/// Stable identity for a GPU-resident 2D texture sampled by application-owned shaders.\npub struct GpuTexture2dId(pub usize);\n\n/// Immutable RGBA8 texture data uploaded lazily by GPUI's Nova renderer.\n///\n/// The byte storage is retained by the application object, while each Nova surface caches one\n/// backend texture by `(id, generation)`. Updating content creates a new generation and never\n/// performs a per-frame upload.\n#[derive(Clone)]\npub struct GpuTexture2d {\n    /// Stable renderer cache identity.\n    pub id: GpuTexture2dId,\n    /// Generation used to invalidate the cached GPU allocation.\n    pub generation: u64,\n    /// Width in texels.\n    pub width: u32,\n    /// Height in texels.\n    pub height: u32,\n    /// Tightly packed RGBA8 pixels.\n    pub pixels: Arc<[u8]>,\n}\n\nimpl std::fmt::Debug for GpuTexture2d {\n    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n        formatter\n            .debug_struct(\"GpuTexture2d\")\n            .field(\"id\", &self.id)\n            .field(\"generation\", &self.generation)\n            .field(\"width\", &self.width)\n            .field(\"height\", &self.height)\n            .field(\"byte_len\", &self.pixels.len())\n            .finish()\n    }\n}\n\nimpl GpuTexture2d {\n    /// Creates a tightly-packed RGBA8 sampled texture.\n    ///\n    /// Returns an error when dimensions overflow or the byte length is not `width * height * 4`.\n    pub fn from_rgba8(\n        width: u32,\n        height: u32,\n        pixels: impl Into<Arc<[u8]>>,\n    ) -> Result<Self, String> {\n        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);\n\n        if width == 0 || height == 0 {\n            return Err(\"GPU texture dimensions must be non-zero\".to_string());\n        }\n        let pixels = pixels.into();\n        let expected_len = usize::try_from(width)\n            .ok()\n            .and_then(|width| usize::try_from(height).ok().and_then(|height| width.checked_mul(height)))\n            .and_then(|pixels| pixels.checked_mul(4))\n            .ok_or_else(|| \"GPU texture dimensions overflow usize\".to_string())?;\n        if pixels.len() != expected_len {\n            return Err(format!(\n                \"GPU RGBA8 texture byte length mismatch: expected {expected_len}, got {}\",\n                pixels.len()\n            ));\n        }\n        Ok(Self {\n            id: GpuTexture2dId(NEXT_ID.fetch_add(1, SeqCst)),\n            generation: 0,\n            width,\n            height,\n            pixels,\n        })\n    }\n\n    /// Reuses this logical texture id while invalidating renderer-side content.\n    pub fn with_generation(mut self, generation: u64) -> Self {\n        self.generation = generation;\n        self\n    }\n}\n\n/// Optional resources bound for one application-owned GPU mesh draw.\n#[derive(Clone, Debug, Default)]\npub struct GpuMesh3dDrawResources {\n    /// RGBA texture exposed to WGSL at `@group(0) @binding(4)`.\n    pub sampled_texture: Option<Arc<GpuTexture2d>>,\n}\n""",
    "scene mesh sampled texture types",
)
s = replace_once(
    s,
    """    pub mesh: Arc<GpuMesh3d>,\n    pub parameters: GpuMesh3dDrawParameters,\n}""",
    """    pub mesh: Arc<GpuMesh3d>,\n    pub parameters: GpuMesh3dDrawParameters,\n    pub resources: GpuMesh3dDrawResources,\n}""",
    "paint mesh resources",
)
write(path, s)

path = "crates/gpui/src/scene.rs"
s = read(path)
s = replace_once(
    s,
    """    GpuMesh3d, GpuMesh3dDrawParameters, GpuMesh3dDrawRanges, GpuMesh3dId, GpuMesh3dRange,\n    GpuMesh3dShader, GpuMesh3dShaderId, GpuMesh3dVertex,\n""",
    """    GpuMesh3d, GpuMesh3dDrawParameters, GpuMesh3dDrawRanges, GpuMesh3dDrawResources,\n    GpuMesh3dId, GpuMesh3dRange, GpuMesh3dShader, GpuMesh3dShaderId, GpuMesh3dVertex,\n    GpuTexture2d, GpuTexture2dId,\n""",
    "scene exports",
)
write(path, s)

path = "crates/gpui/src/window/paint_resources.rs"
s = read(path)
old = """    pub fn paint_gpu_mesh_3d(\n        &mut self,\n        bounds: Bounds<Pixels>,\n        mesh: Arc<GpuMesh3d>,\n        parameters: GpuMesh3dDrawParameters,\n    ) {\n        self.invalidator.debug_assert_paint();\n\n        let scale_factor = self.scale_factor();\n        let bounds = self.visual_bounds(bounds).scale(scale_factor);\n        let content_mask = self.visual_content_mask().scale(scale_factor);\n        self.next_frame.scene.insert_primitive(PaintGpuMesh3d {\n            order: 0,\n            bounds,\n            content_mask,\n            mesh,\n            parameters,\n        });\n    }\n"""
new = """    pub fn paint_gpu_mesh_3d(\n        &mut self,\n        bounds: Bounds<Pixels>,\n        mesh: Arc<GpuMesh3d>,\n        parameters: GpuMesh3dDrawParameters,\n    ) {\n        self.paint_gpu_mesh_3d_with_resources(\n            bounds,\n            mesh,\n            parameters,\n            GpuMesh3dDrawResources::default(),\n        );\n    }\n\n    /// Paint a GPU mesh with application-owned sampled resources.\n    ///\n    /// Sampled textures are uploaded once per `(texture id, generation)` by Nova and are exposed to\n    /// runtime WGSL as `@group(0) @binding(4)` with the shared linear sampler at binding 5.\n    pub fn paint_gpu_mesh_3d_with_resources(\n        &mut self,\n        bounds: Bounds<Pixels>,\n        mesh: Arc<GpuMesh3d>,\n        parameters: GpuMesh3dDrawParameters,\n        resources: GpuMesh3dDrawResources,\n    ) {\n        self.invalidator.debug_assert_paint();\n\n        let scale_factor = self.scale_factor();\n        let bounds = self.visual_bounds(bounds).scale(scale_factor);\n        let content_mask = self.visual_content_mask().scale(scale_factor);\n        self.next_frame.scene.insert_primitive(PaintGpuMesh3d {\n            order: 0,\n            bounds,\n            content_mask,\n            mesh,\n            parameters,\n            resources,\n        });\n    }\n"""
s = replace_once(s, old, new, "window paint gpu mesh resources")
write(path, s)

# -----------------------------------------------------------------------------
# Nova custom-mesh layout: bindings 4/5 match GPUI's existing sampled-texture convention.
# -----------------------------------------------------------------------------
path = "crates/gpui/src/platform/nova/resource_layouts.rs"
s = read(path)
old = """                ResourceSetLayoutEntry {\n                    binding: 20,\n                    binding_type: ResourceBindingType::StorageBuffer,\n                    stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,\n                },\n"""
new = """                ResourceSetLayoutEntry {\n                    binding: 4,\n                    binding_type: ResourceBindingType::SampledTexture,\n                    stages: ShaderStages::FRAGMENT,\n                },\n                ResourceSetLayoutEntry {\n                    binding: 5,\n                    binding_type: ResourceBindingType::Sampler,\n                    stages: ShaderStages::FRAGMENT,\n                },\n                ResourceSetLayoutEntry {\n                    binding: 20,\n                    binding_type: ResourceBindingType::StorageBuffer,\n                    stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,\n                },\n"""
# Match only inside custom mesh layout: use rsplit because binding20 is unique today.
s = replace_once(s, old, new, "custom mesh texture layout")
write(path, s)

path = "crates/gpui/src/platform/nova/resource_bindings.rs"
s = read(path)
s = replace_once(
    s,
    """pub(super) fn custom_mesh_3d_resource_bindings(\n    global_buffer: BufferId,\n    parameters_buffer: BufferId,\n    vertices_buffer: BufferId,\n    vertex_capacity: usize,\n) -> Vec<ResourceBinding> {""",
    """pub(super) fn custom_mesh_3d_resource_bindings(\n    global_buffer: BufferId,\n    parameters_buffer: BufferId,\n    vertices_buffer: BufferId,\n    vertex_capacity: usize,\n    sampled_texture_view: TextureViewId,\n    sampler: SamplerId,\n) -> Vec<ResourceBinding> {""",
    "custom mesh binding signature",
)
needle = """        ResourceBinding {\n            binding: 20,\n            resource: ResourceBindingResource::Buffer(BufferBinding {\n"""
s = replace_once(
    s,
    needle,
    """        ResourceBinding {\n            binding: 4,\n            resource: ResourceBindingResource::Texture(TextureBinding {\n                texture_view: sampled_texture_view,\n            }),\n        },\n        ResourceBinding {\n            binding: 5,\n            resource: ResourceBindingResource::Sampler(SamplerBinding { sampler }),\n        },\n        ResourceBinding {\n            binding: 20,\n            resource: ResourceBindingResource::Buffer(BufferBinding {\n""",
    "custom mesh sampled bindings",
)
write(path, s)

path = "crates/gpui/src/platform/nova/resources/resource_sets.rs"
s = read(path)
s = replace_once(
    s,
    """    vertices_buffer: BufferId,\n    vertex_capacity: usize,\n) -> Result<ResourceSetId>""",
    """    vertices_buffer: BufferId,\n    vertex_capacity: usize,\n    sampled_texture_view: TextureViewId,\n    sampler: SamplerId,\n) -> Result<ResourceSetId>""",
    "resource set signature",
)
s = replace_once(
    s,
    """            vertices_buffer,\n            vertex_capacity,\n        ),""",
    """            vertices_buffer,\n            vertex_capacity,\n            sampled_texture_view,\n            sampler,\n        ),""",
    "resource set binding call",
)
s = replace_once(
    s,
    """    custom_mesh_3d_vertices_buffer: BufferId,\n    custom_mesh_3d_vertex_capacity: usize,\n) -> Result<FrameResourceSets>""",
    """    custom_mesh_3d_vertices_buffer: BufferId,\n    custom_mesh_3d_vertex_capacity: usize,\n    custom_mesh_default_texture_view: TextureViewId,\n    sampler: SamplerId,\n) -> Result<FrameResourceSets>""",
    "renderer resource set signature",
)
s = replace_once(
    s,
    """        custom_mesh_3d_vertices_buffer,\n        custom_mesh_3d_vertex_capacity,\n    )?;""",
    """        custom_mesh_3d_vertices_buffer,\n        custom_mesh_3d_vertex_capacity,\n        custom_mesh_default_texture_view,\n        sampler,\n    )?;""",
    "default custom resource set args",
)
write(path, s)

path = "crates/gpui/src/platform/nova/resources/create.rs"
s = read(path)
s = replace_once(
    s,
    """            shared_buffers.custom_mesh_3d_vertices_buffer,\n            CUSTOM_MESH_3D_PLACEHOLDER_VERTICES,\n        )?;""",
    """            shared_buffers.custom_mesh_3d_vertices_buffer,\n            CUSTOM_MESH_3D_PLACEHOLDER_VERTICES,\n            atlas_resources.texture_view,\n            shared_buffers.atlas_sampler,\n        )?;""",
    "renderer resource sets custom texture args",
)
write(path, s)

# -----------------------------------------------------------------------------
# Flatten sampled texture identity into the retained upload stream.
# -----------------------------------------------------------------------------
path = "crates/gpui/src/platform/nova/frame_upload/batch.rs"
s = read(path)
s = replace_once(
    s,
    """        shader_id: GpuMesh3dShaderId,\n        range: GpuMesh3dRange,\n        first_parameter_index: u32,\n""",
    """        shader_id: GpuMesh3dShaderId,\n        sampled_texture_id: Option<GpuTexture2dId>,\n        sampled_texture_generation: u64,\n        range: GpuMesh3dRange,\n        first_parameter_index: u32,\n""",
    "uploaded batch sampled texture",
)
write(path, s)

path = "crates/gpui/src/platform/nova/frame_upload/frame.rs"
s = read(path)
s = replace_once(
    s,
    """    pub(in crate::platform::nova) custom_mesh_3d_shaders: Vec<Arc<GpuMesh3dShader>>,\n    pub(in crate::platform::nova) custom_mesh_3d_ids: FxHashSet<GpuMesh3dId>,\n""",
    """    pub(in crate::platform::nova) custom_mesh_3d_shaders: Vec<Arc<GpuMesh3dShader>>,\n    pub(in crate::platform::nova) custom_mesh_3d_textures: Vec<Arc<GpuTexture2d>>,\n    pub(in crate::platform::nova) custom_mesh_3d_ids: FxHashSet<GpuMesh3dId>,\n    pub(in crate::platform::nova) custom_mesh_3d_texture_ids: FxHashSet<GpuTexture2dId>,\n""",
    "frame upload sampled texture fields",
)
write(path, s)

path = "crates/gpui/src/platform/nova/frame_upload/encode.rs"
s = read(path)
s = replace_once(
    s,
    """            self.custom_mesh_3d_shaders.clear();\n            self.custom_mesh_3d_ids.clear();\n            self.custom_mesh_3d_shader_ids.clear();\n""",
    """            self.custom_mesh_3d_shaders.clear();\n            self.custom_mesh_3d_textures.clear();\n            self.custom_mesh_3d_ids.clear();\n            self.custom_mesh_3d_texture_ids.clear();\n            self.custom_mesh_3d_shader_ids.clear();\n""",
    "frame reset sampled textures",
)
needle = """                        let first_parameter_index = (self.custom_mesh_3d_parameters.len()\n                            / PACKED_CUSTOM_MESH_3D_PARAMETERS_BYTES)\n                            as u32;\n"""
insert = """                        let sampled_texture = painted.resources.sampled_texture.as_ref();\n                        if let Some(texture) = sampled_texture\n                            && self.custom_mesh_3d_texture_ids.insert(texture.id)\n                        {\n                            self.custom_mesh_3d_textures.push(texture.clone());\n                        }\n                        let sampled_texture_id = sampled_texture.map(|texture| texture.id);\n                        let sampled_texture_generation =\n                            sampled_texture.map_or(0, |texture| texture.generation);\n                        let first_parameter_index = (self.custom_mesh_3d_parameters.len()\n                            / PACKED_CUSTOM_MESH_3D_PARAMETERS_BYTES)\n                            as u32;\n"""
s = replace_once(s, needle, insert, "encode sampled texture collection")
s = replace_once(
    s,
    """                                shader_id: painted.mesh.shader.id,\n                                range,\n                                first_parameter_index,\n""",
    """                                shader_id: painted.mesh.shader.id,\n                                sampled_texture_id,\n                                sampled_texture_generation,\n                                range,\n                                first_parameter_index,\n""",
    "encode sampled texture batch",
)
write(path, s)

# -----------------------------------------------------------------------------
# Renderer-side texture cache. One upload per id/generation, resource sets per frame slot.
# -----------------------------------------------------------------------------
path = "crates/gpui/src/platform/nova/renderer.rs"
s = read(path)
s = replace_once(
    s,
    """mod mesh_cache;\nmod mesh_cache_release;\n""",
    """mod mesh_cache;\nmod mesh_cache_release;\nmod texture_cache;\n""",
    "renderer texture cache module",
)
s = replace_once(
    s,
    """pub(super) struct MeshCacheEntry {\n    pub(super) generation: u64,\n    pub(super) vertex_offset: u32,\n    pub(super) vertex_count: u32,\n    pub(super) index_offset: u32,\n    pub(super) index_count: u32,\n}\n""",
    """pub(super) struct MeshCacheEntry {\n    pub(super) generation: u64,\n    pub(super) vertex_offset: u32,\n    pub(super) vertex_count: u32,\n    pub(super) index_offset: u32,\n    pub(super) index_count: u32,\n}\n\n#[derive(Clone)]\npub(super) struct CustomMeshTextureCacheEntry {\n    pub(super) generation: u64,\n    pub(super) texture: TextureId,\n    pub(super) texture_view: TextureViewId,\n    pub(super) resource_sets: Vec<ResourceSetId>,\n}\n""",
    "renderer texture cache entry",
)
s = replace_once(
    s,
    """    custom_mesh_3d_mesh_cache: FxHashMap<GpuMesh3dId, MeshCacheEntry>,\n    custom_mesh_3d_vertex_cursor: usize,\n""",
    """    custom_mesh_3d_mesh_cache: FxHashMap<GpuMesh3dId, MeshCacheEntry>,\n    custom_mesh_3d_texture_cache: FxHashMap<GpuTexture2dId, CustomMeshTextureCacheEntry>,\n    custom_mesh_3d_vertex_cursor: usize,\n""",
    "renderer texture cache field",
)
write(path, s)

# Initialize the new cache for every backend constructor.
path = "crates/gpui/src/platform/nova/renderer/init.rs"
s = read(path)
count = s.count("custom_mesh_3d_mesh_cache: FxHashMap::default(),")
if count < 1:
    raise RuntimeError("renderer init cache anchor missing")
s = s.replace(
    "custom_mesh_3d_mesh_cache: FxHashMap::default(),",
    "custom_mesh_3d_mesh_cache: FxHashMap::default(),\n                    custom_mesh_3d_texture_cache: FxHashMap::default(),",
)
write(path, s)

# New backend-generic texture cache implementation.
path = "crates/gpui/src/platform/nova/renderer/texture_cache.rs"
write(
    path,
    r'''use super::*;

impl NovaRenderer {
    pub(super) fn ensure_custom_mesh_3d_textures_for_current_backend(&mut self) -> Result<()> {
        let textures = std::mem::take(&mut self.frame_upload.custom_mesh_3d_textures);
        let result = match &mut self.backend {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => ensure_custom_mesh_textures_on_device(
                device,
                "gpui nova dx12",
                &textures,
                &self.frame_resources,
                self.custom_mesh_3d_resource_set_layout,
                self.custom_mesh_3d_vertices_buffer,
                self.atlas_sampler,
                &mut self.custom_mesh_3d_texture_cache,
            ),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => ensure_custom_mesh_textures_on_device(
                device,
                "gpui nova metal",
                &textures,
                &self.frame_resources,
                self.custom_mesh_3d_resource_set_layout,
                self.custom_mesh_3d_vertices_buffer,
                self.atlas_sampler,
                &mut self.custom_mesh_3d_texture_cache,
            ),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => ensure_custom_mesh_textures_on_device(
                device,
                "gpui nova vulkan",
                &textures,
                &self.frame_resources,
                self.custom_mesh_3d_resource_set_layout,
                self.custom_mesh_3d_vertices_buffer,
                self.atlas_sampler,
                &mut self.custom_mesh_3d_texture_cache,
            ),
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            NovaBackend::Unavailable => Ok(()),
        };
        self.frame_upload.custom_mesh_3d_textures = textures;
        result
    }

    pub(super) fn custom_mesh_3d_resource_set_for_texture(
        &self,
        texture_id: Option<GpuTexture2dId>,
        generation: u64,
        frame_index: usize,
    ) -> Option<ResourceSetId> {
        let Some(texture_id) = texture_id else {
            return Some(self.custom_mesh_3d_resource_set);
        };
        let cached = self.custom_mesh_3d_texture_cache.get(&texture_id)?;
        if cached.generation != generation {
            return None;
        }
        cached.resource_sets.get(frame_index).copied()
    }
}

fn ensure_custom_mesh_textures_on_device<D>(
    device: &mut D,
    label: &str,
    textures: &[Arc<GpuTexture2d>],
    frame_resources: &[FrameResources],
    layout: ResourceSetLayoutId,
    vertices_buffer: BufferId,
    sampler: SamplerId,
    cache: &mut FxHashMap<GpuTexture2dId, CustomMeshTextureCacheEntry>,
) -> Result<()>
where
    D: BackendResources,
{
    let live = textures.iter().map(|texture| texture.id).collect::<FxHashSet<_>>();
    let stale = cache
        .keys()
        .copied()
        .filter(|id| !live.contains(id))
        .collect::<Vec<_>>();
    for id in stale {
        if let Some(entry) = cache.remove(&id) {
            destroy_custom_mesh_texture(device, entry, label, id);
        }
    }

    for texture in textures {
        if cache
            .get(&texture.id)
            .is_some_and(|entry| entry.generation == texture.generation)
        {
            continue;
        }
        if let Some(old) = cache.remove(&texture.id) {
            destroy_custom_mesh_texture(device, old, label, texture.id);
        }
        let entry = create_custom_mesh_texture(
            device,
            label,
            texture,
            frame_resources,
            layout,
            vertices_buffer,
            sampler,
        )?;
        cache.insert(texture.id, entry);
    }
    Ok(())
}

fn create_custom_mesh_texture<D>(
    device: &mut D,
    label: &str,
    source: &GpuTexture2d,
    frame_resources: &[FrameResources],
    layout: ResourceSetLayoutId,
    vertices_buffer: BufferId,
    sampler: SamplerId,
) -> Result<CustomMeshTextureCacheEntry>
where
    D: BackendResources,
{
    let size = Extent2d::new(source.width, source.height)?;
    let texture = device.create_texture(&TextureDescriptor {
        label: Some(format!("{label} custom shader texture {:?}", source.id)),
        size,
        format: Format::Rgba8Unorm,
        usage: TextureUsage::COPY_DST | TextureUsage::SAMPLED,
        memory_location: MemoryLocation::GpuOnly,
        dimension: TextureDimension::D2,
    })?;
    let row_bytes = source
        .width
        .checked_mul(4)
        .ok_or_else(|| anyhow::anyhow!("custom shader texture row byte count overflow"))?;
    device.write_texture(
        TextureWriteDescriptor {
            texture,
            layout: TextureDataLayout::new(0, row_bytes, source.height)?,
            origin: Origin2d::ZERO,
            size,
        },
        source.pixels.as_ref(),
    )?;
    let texture_view = device.create_texture_view(&TextureViewDescriptor {
        label: Some(format!("{label} custom shader texture {:?} view", source.id)),
        texture,
        format: Format::Rgba8Unorm,
    })?;

    let mut resource_sets = Vec::with_capacity(frame_resources.len());
    for (frame_index, frame) in frame_resources.iter().enumerate() {
        resource_sets.push(create_custom_mesh_3d_resource_set(
            device,
            &format!("{label} custom shader texture {:?} frame {frame_index}", source.id),
            layout,
            frame.buffers.global_buffer,
            frame.buffers.custom_mesh_3d_parameters_buffer,
            vertices_buffer,
            MAX_CUSTOM_MESH_3D_VERTICES,
            texture_view,
            sampler,
        )?);
    }

    Ok(CustomMeshTextureCacheEntry {
        generation: source.generation,
        texture,
        texture_view,
        resource_sets,
    })
}

fn destroy_custom_mesh_texture<D>(
    device: &mut D,
    entry: CustomMeshTextureCacheEntry,
    label: &str,
    id: GpuTexture2dId,
) where
    D: BackendResources,
{
    for resource_set in entry.resource_sets {
        if let Err(error) = device.destroy_resource_set(resource_set) {
            log::debug!("failed to destroy {label} custom shader texture {:?} resource set: {error}", id);
        }
    }
    if let Err(error) = device.destroy_texture_view(entry.texture_view) {
        log::debug!("failed to destroy {label} custom shader texture {:?} view: {error}", id);
    }
    if let Err(error) = device.destroy_texture(entry.texture) {
        log::debug!("failed to destroy {label} custom shader texture {:?}: {error}", id);
    }
}
''',
)

# Ensure texture uploads happen after mesh buffer promotion but before draw-step preparation.
path = "crates/gpui/src/platform/nova/renderer/present.rs"
s = read(path)
anchor = """        self.sync_atlas_textures_for_current_backend()?;\n        self.ensure_custom_mesh_3d_cache_for_current_backend()?;\n"""
replacement = """        self.sync_atlas_textures_for_current_backend()?;\n        self.ensure_custom_mesh_3d_cache_for_current_backend()?;\n        self.ensure_custom_mesh_3d_textures_for_current_backend()?;\n"""
if anchor not in s:
    raise RuntimeError("present custom mesh ensure anchor missing")
s = s.replace(anchor, replacement)
write(path, s)

# Draw-step routing chooses the resource set associated with the batch's sampled texture.
path = "crates/gpui/src/platform/nova/draw.rs"
s = read(path)
s = replace_once(
    s,
    """    custom_mesh_3d_resource_set: ResourceSetId,\n    custom_mesh_3d_indices_buffer: BufferId,\n""",
    """    mut custom_mesh_3d_resource_set: impl FnMut(Option<GpuTexture2dId>, u64) -> Option<ResourceSetId>,\n    custom_mesh_3d_indices_buffer: BufferId,\n""",
    "draw custom resource set callback signature",
)
s = replace_once(
    s,
    """                shader_id,\n                range,\n                first_parameter_index,\n""",
    """                shader_id,\n                sampled_texture_id,\n                sampled_texture_generation,\n                range,\n                first_parameter_index,\n""",
    "draw custom batch destructure",
)
s = replace_once(
    s,
    """                if let Some(pipeline) = custom_mesh_3d_pipeline(shader_id) {\n                    steps.push(RenderStepDescriptor::DrawIndexed(\n                        DrawIndexedStepDescriptor {\n                            pipeline,\n                            resource_sets: resource_set_list([custom_mesh_3d_resource_set]),\n""",
    """                if let Some(pipeline) = custom_mesh_3d_pipeline(shader_id)\n                    && let Some(resource_set) = custom_mesh_3d_resource_set(\n                        sampled_texture_id,\n                        sampled_texture_generation,\n                    )\n                {\n                    steps.push(RenderStepDescriptor::DrawIndexed(\n                        DrawIndexedStepDescriptor {\n                            pipeline,\n                            resource_sets: resource_set_list([resource_set]),\n""",
    "draw custom resource selection",
)
# Test helper forwards one constant resource set through the callback.
s = replace_once(
    s,
    """        custom_mesh_3d_resource_set,\n        custom_mesh_3d_indices_buffer,\n""",
    """        |_, _| Some(custom_mesh_3d_resource_set),\n        custom_mesh_3d_indices_buffer,\n""",
    "draw test helper callback",
)
write(path, s)

# Renderer draw-step preparation: replace every direct custom mesh resource-set argument with a
# lookup closure. Define immutable cache/default handles in each method that already captures mesh cache.
path = "crates/gpui/src/platform/nova/renderer/draw_steps.rs"
s = read(path)
# Add cache/default locals after every custom mesh cache binding in this file.
anchor = """        let custom_mesh_3d_mesh_cache = &self.custom_mesh_3d_mesh_cache;\n"""
replacement = """        let custom_mesh_3d_mesh_cache = &self.custom_mesh_3d_mesh_cache;\n        let custom_mesh_3d_texture_cache = &self.custom_mesh_3d_texture_cache;\n        let default_custom_mesh_3d_resource_set = self.custom_mesh_3d_resource_set;\n"""
if anchor not in s:
    raise RuntimeError("draw_steps mesh cache local anchor missing")
s = s.replace(anchor, replacement)
# Replace all direct resource-set arguments in calls.
s = s.replace(
    """            self.custom_mesh_3d_resource_set,\n            self.custom_mesh_3d_indices_buffer,""",
    """            |texture_id, generation| {\n                custom_mesh_resource_set(\n                    custom_mesh_3d_texture_cache,\n                    default_custom_mesh_3d_resource_set,\n                    texture_id,\n                    generation,\n                    frame_resource_index,\n                )\n            },\n            self.custom_mesh_3d_indices_buffer,""",
)
# Some nested helper uses a source resource set and has same direct argument indentation.
s = s.replace(
    """                self.custom_mesh_3d_resource_set,\n                self.custom_mesh_3d_indices_buffer,""",
    """                |texture_id, generation| {\n                    custom_mesh_resource_set(\n                        custom_mesh_3d_texture_cache,\n                        default_custom_mesh_3d_resource_set,\n                        texture_id,\n                        generation,\n                        frame_resource_index,\n                    )\n                },\n                self.custom_mesh_3d_indices_buffer,""",
)
# Append a tiny borrow-only lookup helper.
s += r'''

fn custom_mesh_resource_set(
    textures: &FxHashMap<GpuTexture2dId, CustomMeshTextureCacheEntry>,
    default_resource_set: ResourceSetId,
    texture_id: Option<GpuTexture2dId>,
    generation: u64,
    frame_index: usize,
) -> Option<ResourceSetId> {
    let Some(texture_id) = texture_id else {
        return Some(default_resource_set);
    };
    let texture = textures.get(&texture_id)?;
    if texture.generation != generation {
        return None;
    }
    texture.resource_sets.get(frame_index).copied()
}
'''
write(path, s)

# Mesh buffer promotion recreates default resource sets. Supply a harmless fallback sampled texture
# from the existing path mask texture/sampler so legacy untextured shaders keep their old behavior.
path = "crates/gpui/src/platform/nova/renderer/mesh_cache.rs"
s = read(path)
# The helper call inside promotion is patched by extending the helper signature and existing calls.
s = s.replace(
    """                old_vertices,\n                old_indices,\n            )?""",
    """                old_vertices,\n                old_indices,\n                self.path_texture_view,\n                self.atlas_sampler,\n            )?""",
)
s = replace_once(
    s,
    """    old_vertices: BufferId,\n    old_indices: BufferId,\n) -> Result<(BufferId, BufferId)>""",
    """    old_vertices: BufferId,\n    old_indices: BufferId,\n    sampled_texture_view: TextureViewId,\n    sampler: SamplerId,\n) -> Result<(BufferId, BufferId)>""",
    "mesh promotion helper signature",
)
s = replace_once(
    s,
    """            vertices,\n            MAX_CUSTOM_MESH_3D_VERTICES,\n        )?;""",
    """            vertices,\n            MAX_CUSTOM_MESH_3D_VERTICES,\n            sampled_texture_view,\n            sampler,\n        )?;""",
    "mesh promotion resource set args",
)
write(path, s)

# Tests constructing PaintGpuMesh3d need default resources. Add field where the literal is used.
for path in [
    "crates/gpui/src/platform/nova/tests.rs",
    "crates/gpui/src/scene/tests.rs",
]:
    s = read(path)
    s = s.replace(
        """            parameters: GpuMesh3dDrawParameters {""",
        """            resources: GpuMesh3dDrawResources::default(),\n            parameters: GpuMesh3dDrawParameters {""",
    )
    s = s.replace(
        """        parameters: GpuMesh3dDrawParameters {""",
        """        resources: GpuMesh3dDrawResources::default(),\n        parameters: GpuMesh3dDrawParameters {""",
    )
    write(path, s)

print("custom mesh sampled texture patch applied")
