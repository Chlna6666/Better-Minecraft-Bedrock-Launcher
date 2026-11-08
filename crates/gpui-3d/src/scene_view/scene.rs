use super::{
    DRAW_PARAMS_STRIDE, FRAME_PARAMS_STRIDE, INSTANCE_STRIDE, LIGHT_STRIDE, SceneView,
    culling::{ARGUMENT_STRIDE, CullResources},
    gpu::{
        DrawBindingResources, DrawRecordCache, MeshArena, MeshResources, PendingMeshUpload,
        TextureResources, buffer_memory_location, can_encode_voxel_mesh, create_draw_resource_set,
        create_mesh, create_storage_buffer, create_texture, destroy_mesh, destroy_texture,
        dx11_streaming_headroom, encode_frame_params, encode_lights_into, grow_capacity,
    },
    projection::FrameProjection,
    renderer::trace_timings,
    resources::RendererResources,
    uploads,
};
use crate::{
    Aabb, AlphaMode, AnimationScratch, Camera, Mat4, MaterialId, Mesh, MeshId, MeshPart, NodeId,
    PreparedDraw, PreparedScene, Scene, ShadingModel, TextureAssetId,
};
use anyhow::{Context as _, anyhow, bail};
use gfx_core::{
    BackendKind, BindGroupId, BindGroupLayoutId, BufferDescriptor, BufferId, BufferUsage,
    BufferWrite, BufferWriteDescriptor, DrawCommand, DrawIndexedCommand, DrawIndirectCommand,
    IndexBufferBinding, RenderCommand, SamplerId, bind_group_list,
};
use gpui::ExtensionDevice;
use gpui::RendererExtensionContext;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

pub(super) struct SceneResources {
    backend: BackendKind,
    culling: Option<CullResources>,
    mesh_shader: Option<super::generated::Shaders>,
    mesh_draw_shader: Option<gfx_core::ShaderModuleId>,
    generated_arguments: Option<(BufferId, usize)>,
    generated_mappings: HashMap<BufferId, generated::Mapping>,
    /// Published by the native binding traversal, consumed by the GPU Draw
    /// table update without another full Batch/Mesh pass.
    generated_pending_groups: Option<HashMap<BufferId, (usize, Vec<u32>)>>,
    /// When true, pending groups contain only touched physical Argument
    /// Pages. Unaffected Pages must not be retired or rebuilt.
    generated_pending_partial: bool,
    /// Retains the Page-group HashMap buckets across Section publications.
    generated_group_scratch: HashMap<BufferId, (usize, Vec<u32>)>,
    scene: Option<Arc<Scene>>,
    active_assets: assets::AssetCounts,
    texture_table: Option<Arc<HashMap<TextureAssetId, Arc<crate::TextureAsset>>>>,
    animation: Option<Arc<[crate::TransformTrack]>>,
    animation_time: Option<Duration>,
    animation_scratch: AnimationScratch,
    camera: Option<Camera>,
    culling_bounds: Option<Aabb>,
    /// Per-view occlusion mask keeps immutable Scene and physical Draw Slots.
    hidden_nodes: Arc<HashSet<NodeId>>,
    scene_transform: Option<Mat4>,
    projection: Option<FrameProjection>,
    prepared: PreparedScene,
    pub(super) command_revision: u64,
    /// Increments after a successfully prepared *visible frame change*, not
    /// merely after rebuilding native draw commands. Camera/uniform changes
    /// also invalidate a cached offscreen capture even with identical draws.
    /// Zero means no complete scene frame has yet been published.
    pub(super) capture_revision: u64,
    /// Exact structural range mapping eligible for a native-command splice.
    /// Set only after stable GPU slots and survivor BindGroups were committed.
    command_delta: Option<crate::prepare::StructuralDrawDelta>,
    structural_bindings_spliced: bool,
    // Cached native commands remain valid while camera motion leaves CPU-pruned batches unchanged.
    command_visibility: Vec<bool>,
    batches: Vec<DrawBatchRange>,
    batch_workspace: super::batching::BatchWorkspace,
    previous_draws: Vec<(NodeId, MeshId, MeshPart, MaterialId)>,
    draw_buffer: BufferId,
    draw_capacity: usize,
    /// Dense logical-to-stable-physical draw slot map for voxel shaders.
    draw_slot_buffer: BufferId,
    draw_slot_capacity: usize,
    stable_slots: stable_slots::StableDrawSlots,
    uploaded_slot_map: Vec<u8>,
    scratch_slot_map: Vec<u8>,
    scratch_packed_draws: Vec<u8>,
    instance_buffer: BufferId,
    instance_capacity: usize,
    light_buffer: BufferId,
    light_capacity: usize,
    frame_buffer: BufferId,
    // Last committed GPU frame uniform bytes. Section updates often preserve
    // the camera, shadow matrix and render settings; skip redundant uploads.
    uploaded_frame_params: Vec<u8>,
    frame_state: Option<FrameState>,
    shadow: Option<super::shadow::Shadow>,
    light_pass: Option<light_pass::LightPass>,
    uploaded_tables: [Vec<u8>; 3],
    scratch_tables: [Vec<u8>; 3],
    draw_record_cache: DrawRecordCache,
    /// Committed identities for 80-byte GPU DrawParams records.
    draw_slot_keys: Vec<super::gpu::DrawParamKey>,
    scratch_draw_slot_keys: Vec<super::gpu::DrawParamKey>,
    /// Previous committed 144-byte GPU instance row identities.
    instance_slot_keys: Vec<instance_cache::InstanceSlotKey>,
    scratch_instance_keys: Vec<instance_cache::InstanceSlotKey>,
    dirty_instance_run: Vec<u8>,
    meshes: HashMap<(MeshId, bool), MeshResources>,
    /// Live compact-Mesh references to each face/template buffer page.
    page_refs: page_refs::PageRefs,
    arena: MeshArena,
    /// GPU-owner-local arena shared by SceneViews on one renderer/device.
    /// No global GPU handles or cross-device sharing.
    generated_arena: Rc<RefCell<MeshArena>>,
    template_arena: Rc<RefCell<MeshArena>>,
    argument_arena: Rc<RefCell<MeshArena>>,
    retired_meshes: Vec<MeshResources>,
    voxel_shader: bool,
    textures: HashMap<TextureAssetId, TextureResources>,
    seen_materials: HashSet<usize>,
    draw_resources: Vec<BindGroupId>,
    /// Layout stamp captured by the last full sync_draw_resources pass. The
    /// O(1) early-out above reuses the resident tables when every stamp input
    /// is unchanged; None forces the next pass to run.
    draw_resource_layout: usize,
    draw_resource_shadow: Option<BufferId>,
    draw_resource_visible_instances: Option<BufferId>,
    draw_resource_voxel: bool,
    draw_resource_sampler: Option<SamplerId>,
    draw_resource_keys: Vec<DrawResourceKey>,
    /// Mesh identity for each binding. Allows positional hits to skip the
    /// resident MeshResources lookup for unchanged streamed sections.
    draw_resource_sources: Vec<((MeshId, bool), u64)>,
    /// Generated Page and material-part offsets captured in the existing
    /// binding pass, so Draw Mapping does not re-hash every mesh/part.
    generated_batch_sources: Vec<Option<generated::BatchMappingSource>>,
    spare_generated_batch_sources: Vec<Option<generated::BatchMappingSource>>,
    /// Spare capacity for batch/resource lists, recycled across scene publications.
    spare_draw_resources: Vec<BindGroupId>,
    spare_draw_resource_keys: Vec<DrawResourceKey>,
    spare_draw_resource_sources: Vec<((MeshId, bool), u64)>,
    /// Immutable material snapshot and raster winding for each native Batch.
    /// A translation-only Section update keeps the exact cached GPU command
    /// pipeline when both fields remain unchanged.
    native_command_sources: Vec<NativeCommandSource>,
    spare_native_command_sources: Vec<NativeCommandSource>,
    draw_addresses: Vec<DrawAddress>,
    /// Test-only scratch for the retained transparent-reorder helper; the
    /// GPU-culling path now renders the blended tail through blend_order.
    #[cfg(test)]
    reorder_positions: Vec<usize>,
    binding_cache: HashMap<DrawResourceKey, BindGroupId>,
    binding_index: binding_index::BindingIndex,
    last_binding_report: Option<Instant>,
    draw_sync_audit: DrawSyncAudit,
    sampler: Option<SamplerId>,
    // Resource-cache age only; frame-visible output samples the extension frame time.
    pub(super) last_used_at: Instant,
    pub(super) last_used_frame: u64,
}

const MESH_UPLOAD_BATCH_BYTES: usize = 16 * 1024 * 1024;

/// Only value changes with the exact same light count and resident storage
/// allocation are independently patchable. A change in light count must
/// regenerate DrawParams.light_count for EVERY surviving draw.
fn structural_light_patchable(
    previous: &[u8],
    next: &[u8],
    resident_light_capacity: usize,
) -> bool {
    !previous.is_empty()
        && previous.len() == next.len()
        && next.len() <= resident_light_capacity.saturating_mul(LIGHT_STRIDE as usize)
        && next.len().is_multiple_of(LIGHT_STRIDE as usize)
}

#[cfg(test)]
mod structural_light_delta_tests {
    use super::*;

    #[test]
    fn unchanged_light_count_preserves_independent_light_patch() {
        let previous = vec![0u8; LIGHT_STRIDE as usize * 2];
        let mut changed = previous.clone();
        changed[0] = 1;
        assert!(structural_light_patchable(&previous, &changed, 2));
        assert!(!structural_light_patchable(&previous, &changed, 1));
        changed.extend_from_slice(&[0u8; LIGHT_STRIDE as usize]);
        assert!(!structural_light_patchable(&previous, &changed, 4));
    }
}


#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DrawSyncPath {
    Resident,
    Structural,
    Full,
}

#[derive(Default)]
struct DrawSyncAudit {
    since: Option<Instant>,
    resident: u64,
    structural: u64,
    full: u64,
    full_no_gpu_cull: u64,
    full_no_delta: u64,
    full_no_slots: u64,
    full_ineligible: u64,
}

impl DrawSyncAudit {
    fn record(
        &mut self,
        path: DrawSyncPath,
        gpu_culling: bool,
        has_resident_delta: bool,
        has_structural_delta: bool,
        stable_culling_slots: bool,
        draws: usize,
    ) {
        if !cfg!(debug_assertions) && !trace_timings() {
            return;
        }
        match path {
            DrawSyncPath::Resident => self.resident += 1,
            DrawSyncPath::Structural => self.structural += 1,
            DrawSyncPath::Full if !gpu_culling => {
                self.full += 1;
                self.full_no_gpu_cull += 1;
            }
            DrawSyncPath::Full if !has_resident_delta && !has_structural_delta => {
                self.full += 1;
                self.full_no_delta += 1;
            }
            DrawSyncPath::Full if !stable_culling_slots => {
                self.full += 1;
                self.full_no_slots += 1;
            }
            DrawSyncPath::Full => {
                self.full += 1;
                self.full_ineligible += 1;
            }
        }
        let now = Instant::now();
        let first = *self.since.get_or_insert(now);
        if now.saturating_duration_since(first) < Duration::from_secs(2) {
            return;
        }
        let total = self.resident + self.structural + self.full;
        if total > 0 {
            let upload = crate::gpu_upload_cost_estimate();
            eprintln!(
                "gpui3d draw_sync_audit draws={} samples={} resident={} structural={} full={} resident_pct={:.1} structural_pct={:.1} full_pct={:.1} full_no_gpu_cull={} full_no_delta={} full_no_slots={} full_ineligible={} gpu_owner_upload_samples={} gpu_owner_ns_per_mib={} gpu_owner_ns_per_write={}",
                draws, total, self.resident, self.structural, self.full,
                self.resident as f64 * 100.0 / total as f64,
                self.structural as f64 * 100.0 / total as f64,
                self.full as f64 * 100.0 / total as f64,
                self.full_no_gpu_cull, self.full_no_delta,
                self.full_no_slots, self.full_ineligible,
                upload.map_or(0, |s| s.samples),
                upload.map_or(0, |s| s.ns_per_mib),
                upload.map_or(0, |s| s.ns_per_write),
            );
        }
        *self = Self { since: Some(now), ..Self::default() };
    }
}


/// Native Vulkan/DX12 uploads stage all buffer slices in a single command
/// submission. Large scene deltas benefit from retaining more sparse spans,
/// while the smaller budget avoids excessive per-write work on DX11/GL.
fn sparse_upload_range_budget(backend: BackendKind, draw_count: usize) -> usize {
    if draw_count >= 32_768 && matches!(backend, BackendKind::Vulkan | BackendKind::Dx12) {
        64
    } else {
        16
    }
}

/// Amortize light-buffer growth so a streaming Section's next light does not
/// invalidate every draw BindGroup at each power-of-two threshold. Bound the
/// reservation by the active device's reported storage-binding limit.
fn streaming_light_headroom(required: usize, max_binding_bytes: u64) -> usize {
    let max_rows =
        usize::try_from(max_binding_bytes / u64::from(LIGHT_STRIDE)).unwrap_or(usize::MAX);
    if max_rows == 0 {
        return required;
    }
    let max_power_of_two = 1usize << (usize::BITS - 1 - max_rows.leading_zeros());
    if required > max_power_of_two {
        // Do not ask the growth policy for a larger reservation than the
        // device can bind. Its normal validation handles this corner case.
        return required;
    }
    required.saturating_mul(4).min(max_power_of_two)
}

mod assets;
mod binding_index;
mod generated;
mod instance_cache;
mod light_pass;
mod page_refs;
mod reorder;
pub(in crate::scene_view) mod stable_slots;

/// Values that only affect the frame-wide uniform buffer, not the prepared draw list.
#[derive(Clone, Copy, PartialEq)]
struct FrameState {
    camera: Camera,
    projection: FrameProjection,
}

#[derive(Clone, Copy)]
pub(super) struct DrawBatchRange {
    pub(super) first_instance: usize,
    pub(super) instance_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DrawAddress {
    arguments_offset: u64,
    first_instance: usize,
}

/// These are the per-Draw inputs that can change native pipeline selection
/// while its Mesh/Part identity, BindGroup and indirect argument slot remain
/// the same. Material pointer retention comes from the published Scene Arc,
/// and world winding changes alter raster front-face orientation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NativeCommandSource {
    material_snapshot: usize,
    mirrored: bool,
}
impl NativeCommandSource {
    fn for_draw(draw: &PreparedDraw) -> Self {
        Self {
            material_snapshot: Arc::as_ptr(&draw.material) as usize,
            mirrored: draw.is_mirrored(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum DrawMeshKey {
    Mesh {
        id: MeshId,
        generation: u64,
    },
    // Each generated draw selects its own header via the Instance table.
    // BindGroups can now be reused by Sections sharing BOTH resident pages.
    //
    // ADR 0002 §3 (single bind group + global face-template table) cannot
    // simply drop these two BufferIds: generated_template/face data is
    // suballocated into 8 MiB MeshArena pages (185 pages / 1.4 GB in the r64
    // streaming benchmark), and voxel.wgsl addresses them with *page-local*
    // base indices. A single bind group would need one buffer covering all
    // pages (D3D11 storage binding caps ~16 MiB) or a shader-side page-table
    // lookup. Keep the page identity in the key until cells meshing shrinks
    // the data enough for a global table.
    GeneratedPage {
        faces: BufferId,
        templates: BufferId,
    },
    VoxelPage(BufferId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct DrawResourceKey {
    shadow: Option<BufferId>,
    mesh: DrawMeshKey,
    visible_instances: Option<BufferId>,
    albedo_id: Option<TextureAssetId>,
    normal_id: Option<TextureAssetId>,
    occlusion_id: Option<TextureAssetId>,
    metallic_roughness_id: Option<TextureAssetId>,
}

impl DrawResourceKey {
    fn uses_active_assets(
        self,
        meshes: &HashSet<MeshId>,
        voxel_pages: &HashSet<BufferId>,
        textures: &HashSet<TextureAssetId>,
    ) -> bool {
        (match self.mesh {
            DrawMeshKey::Mesh { id, .. } => meshes.contains(&id),
            DrawMeshKey::GeneratedPage { faces, templates } => {
                voxel_pages.contains(&faces) && voxel_pages.contains(&templates)
            }
            DrawMeshKey::VoxelPage(buffer) => voxel_pages.contains(&buffer),
        }) && [
            self.albedo_id,
            self.normal_id,
            self.occlusion_id,
            self.metallic_roughness_id,
        ]
        .into_iter()
        .flatten()
        .all(|id| textures.contains(&id))
    }

    fn is_resident(
        self,
        meshes: &HashMap<(MeshId, bool), MeshResources>,
        voxel_pages: &HashSet<BufferId>,
        visible_instances: Option<BufferId>,
    ) -> bool {
        self.visible_instances == visible_instances
            && match self.mesh {
                DrawMeshKey::Mesh { id, generation } => meshes
                    .get(&(id, false))
                    .is_some_and(|mesh| mesh.generation == generation),
                DrawMeshKey::GeneratedPage { faces, templates } => {
                    voxel_pages.contains(&faces) && voxel_pages.contains(&templates)
                }
                DrawMeshKey::VoxelPage(buffer) => voxel_pages.contains(&buffer),
            }
    }
}

impl SceneResources {
    /// Whether this scene is already committed for the requested immutable
    /// input. A pending worker must not hide or rebuild a matching GPU Scene.
    pub(super) fn matches_published_scene(&self, view: &SceneView) -> bool {
        self.scene.as_ref().is_some_and(|previous| Arc::ptr_eq(previous, &view.scene))
            && self.scene_transform == Some(view.scene_transform)
    }

    /// Keep the last fully published 3D geometry visible while an independent
    /// CPU worker prepares a completely unrelated Scene snapshot. Camera
    /// uniforms, culling and presentation still advance normally on GPU owner.
    /// This avoids a long full-scene CPU traversal inside the GPU Owner's
    /// shared 2D/3D submission slot during progressive preview replacements.
    ///
    /// Incremental forks always take the normal Structural Delta fast path.
    /// Never mix old geometry with a different transform, shader mode or an
    /// animated state. The actual new Scene is published only when the worker
    /// has completed and its matching immutable snapshot is available.
    pub(super) fn retained_sidecar_view(&self, incoming: &SceneView) -> Option<SceneView> {
        if !super::sidecar::pending(incoming)
            || !incoming.animation.is_empty()
            || self.animation.as_ref().is_some_and(|tracks| !tracks.is_empty())
            || self.scene_transform != Some(incoming.scene_transform)
            || self.voxel_shader != incoming.voxel_shader
        {
            return None;
        }
        let previous = self.scene.as_ref()?;
        if Arc::ptr_eq(previous, &incoming.scene)
            || incoming.scene.changed_nodes(previous).is_some()
        {
            return None;
        }
        let mut retained = incoming.clone();
        retained.scene = previous.clone();
        retained.textures = self.texture_table.as_ref()?.clone();
        retained.hidden_nodes = self.hidden_nodes.clone();
        Some(retained)
    }

    pub(super) fn gpu_culling_counts(&self) -> (usize, usize) {
        self.culling.as_ref().map_or((0, 0), CullResources::counts)
    }
    pub(super) fn has_retired_arena_ranges(&self) -> bool {
        self.arena.has_retired_ranges()
    }

    pub(super) fn reclaim_arena_after_gpu_idle(&mut self) {
        self.arena.reclaim_after_gpu_idle();
    }
    pub(super) fn new(
        device: &mut dyn ExtensionDevice,
        backend: BackendKind,
    ) -> gpui::Result<Self> {
        // Query the immutable device limits before passing the exclusive GPU
        // device reference to the constructor.
        let limits = device.buffer_binding_limits();
        let faces = Rc::new(RefCell::new(MeshArena::generated_output(limits)));
        let templates = Rc::new(RefCell::new(MeshArena::generated_templates(limits)));
        let arguments = Rc::new(RefCell::new(MeshArena::generated_arguments(limits)));
        Self::new_shared(device, backend, faces, templates, arguments)
    }

    pub(super) fn new_shared(
        device: &mut dyn ExtensionDevice,
        backend: BackendKind,
        shared_generated_arena: Rc<RefCell<MeshArena>>,
        shared_template_arena: Rc<RefCell<MeshArena>>,
        shared_argument_arena: Rc<RefCell<MeshArena>>,
    ) -> gpui::Result<Self> {
        let draw_capacity = 1;
        let light_capacity = 1;
        let draw_buffer = create_storage_buffer(
            device,
            "gpui-3d draw data",
            draw_capacity * DRAW_PARAMS_STRIDE as usize,
            buffer_memory_location(backend),
        )?;
        let light_buffer = match create_storage_buffer(
            device,
            "gpui-3d lights",
            light_capacity * LIGHT_STRIDE as usize,
            buffer_memory_location(backend),
        ) {
            Ok(buffer) => buffer,
            Err(error) => {
                device.destroy_buffer(draw_buffer)?;
                return Err(error);
            }
        };
        let instance_capacity = 1;
        let instance_buffer = match create_storage_buffer(
            device,
            "gpui-3d instance data",
            instance_capacity * INSTANCE_STRIDE as usize,
            buffer_memory_location(backend),
        ) {
            Ok(buffer) => buffer,
            Err(error) => {
                device.destroy_buffer(light_buffer)?;
                device.destroy_buffer(draw_buffer)?;
                return Err(error);
            }
        };
        let draw_slot_capacity = 1;
        let draw_slot_buffer = match create_storage_buffer(
            device,
            "gpui-3d logical-to-physical draw slots",
            draw_slot_capacity * 4,
            buffer_memory_location(backend),
        ) {
            Ok(buffer) => buffer,
            Err(error) => {
                device.destroy_buffer(instance_buffer)?;
                device.destroy_buffer(light_buffer)?;
                device.destroy_buffer(draw_buffer)?;
                return Err(error);
            }
        };
        let frame_buffer = match device.create_buffer(&BufferDescriptor {
            label: Some("gpui-3d frame params".into()),
            size: FRAME_PARAMS_STRIDE as u64,
            usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
            memory_location: buffer_memory_location(backend),
        }) {
            Ok(buffer) => buffer,
            Err(error) => {
                device.destroy_buffer(draw_slot_buffer)?;
                device.destroy_buffer(instance_buffer)?;
                device.destroy_buffer(light_buffer)?;
                device.destroy_buffer(draw_buffer)?;
                return Err(error.into());
            }
        };
        Ok(Self {
            backend,
            culling: None,
            mesh_shader: None,
            mesh_draw_shader: None,
            generated_arguments: None,
            generated_mappings: HashMap::new(),
            generated_pending_groups: None,
            generated_pending_partial: false,
            generated_group_scratch: HashMap::new(),
            scene: None,
            active_assets: assets::AssetCounts::default(),
            texture_table: None,
            animation: None,
            animation_time: None,
            animation_scratch: AnimationScratch::new(),
            camera: None,
            culling_bounds: None,
            hidden_nodes: Arc::new(HashSet::new()),
            scene_transform: None,
            projection: None,
            prepared: PreparedScene::default(),
            command_revision: 0,
            capture_revision: 0,
            command_delta: None,
            structural_bindings_spliced: false,
            command_visibility: Vec::new(),
            batches: Vec::new(),
            batch_workspace: super::batching::BatchWorkspace::default(),
            previous_draws: Vec::new(),
            draw_buffer,
            draw_capacity,
            draw_slot_buffer,
            draw_slot_capacity,
            stable_slots: stable_slots::StableDrawSlots::default(),
            uploaded_slot_map: Vec::new(),
            scratch_slot_map: Vec::new(),
            scratch_packed_draws: Vec::new(),
            instance_buffer,
            instance_capacity,
            light_buffer,
            light_capacity,
            frame_buffer,
            uploaded_frame_params: Vec::new(),
            frame_state: None,
            shadow: None,
            light_pass: None,
            uploaded_tables: Default::default(),
            scratch_tables: Default::default(),
            draw_record_cache: DrawRecordCache::default(),
            draw_slot_keys: Vec::new(),
            scratch_draw_slot_keys: Vec::new(),
            instance_slot_keys: Vec::new(),
            scratch_instance_keys: Vec::new(),
            dirty_instance_run: Vec::new(),
            meshes: HashMap::new(),
            page_refs: page_refs::PageRefs::default(),
            arena: MeshArena::default(),
            generated_arena: shared_generated_arena,
            template_arena: shared_template_arena,
            argument_arena: shared_argument_arena,
            retired_meshes: Vec::new(),
            voxel_shader: false,
            textures: HashMap::new(),
            seen_materials: HashSet::new(),
            draw_resources: Vec::new(),
            draw_resource_layout: 0,
            draw_resource_shadow: None,
            draw_resource_visible_instances: None,
            draw_resource_voxel: false,
            draw_resource_sampler: None,
            draw_resource_keys: Vec::new(),
            draw_resource_sources: Vec::new(),
            generated_batch_sources: Vec::new(),
            spare_generated_batch_sources: Vec::new(),
            spare_draw_resources: Vec::new(),
            spare_draw_resource_keys: Vec::new(),
            spare_draw_resource_sources: Vec::new(),
            native_command_sources: Vec::new(),
            spare_native_command_sources: Vec::new(),
            draw_addresses: Vec::new(),
            #[cfg(test)]
            reorder_positions: Vec::new(),
            binding_cache: HashMap::new(),
            binding_index: binding_index::BindingIndex::default(),
            last_binding_report: None,
            draw_sync_audit: DrawSyncAudit::default(),
            sampler: None,
            last_used_at: Instant::now(),
            last_used_frame: 0,
        })
    }

    pub(super) fn prepare(
        &mut self,
        device: &mut dyn ExtensionDevice,
        renderer: &mut RendererResources,
        scene_view: &SceneView,
        context: &RendererExtensionContext,
    ) -> gpui::Result<()> {
        let trace_started = trace_timings().then(Instant::now);
        let mut camera_ms = 0.0;
        let mut buffer_ms = 0.0;
        let mut resident_ms = 0.0;
        let mut structural_ms = 0.0;
        let mut sbuffers_ms = 0.0;
        let mut sdres_ms = 0.0;
        let mut cull_update_ms = 0.0;
        let mut cull_dispatch_ms = 0.0;
        let mut walk_ms = 0.0;
        let mut assets_ms = 0.0;
        let mut batch_ms = 0.0;
        let mut retirement_scan = false;
        let projection = FrameProjection::new(scene_view, context)?;
        let sampler =
            renderer.sampler(device, scene_view.sampling, scene_view.anisotropy_enabled)?;
        let assets_changed =
            scene_assets_changed(self.scene.as_ref(), self.texture_table.as_ref(), scene_view);
        let camera_changed = self.camera != Some(scene_view.camera)
            || self.projection.map(FrameProjection::aspect) != Some(projection.aspect());
        let culling_bounds_changed = self.culling_bounds != scene_view.culling_bounds;
        // A camera-only update can reuse the retained draw list unless distance-based LOD may
        // select a different mesh. In that case re-walk the scene before uploading draw state.
        // Device-neutral GPU path: DX11, DX12, Vulkan and OpenGL use the
        // same capability policy and WGSL shader modules.
        let gpu_culling = scene_view.voxel_shader
            && device.compute_capabilities().voxel_execution_path()
                == gfx_core::VoxelExecutionPath::GpuDriven
            && renderer.cull_shader.is_some();
        // A resized/recreated culling buffer invalidates old bind groups even
        // if the scene change only added assets.
        // Portal masks are a GPU compute overlay. The CPU fallback still
        // draws the original authored geometry and must retain its shadows.
        let node_mask_changed = gpu_culling
            && !Arc::ptr_eq(&self.hidden_nodes, &scene_view.hidden_nodes)
            && self.hidden_nodes != scene_view.hidden_nodes;
        let empty_hidden_nodes = HashSet::new();
        let shadow_hidden_nodes = if gpu_culling {
            scene_view.hidden_nodes.as_ref()
        } else {
            &empty_hidden_nodes
        };
        let previous_visible_instances = self
            .culling
            .as_ref()
            .map(|culling| culling.visible_instances);
        let culling_mode_changed = self.prepared.set_gpu_culling(gpu_culling);
        let draw_state_changed = assets_changed
            || culling_mode_changed
            || (camera_changed && self.prepared.uses_lod())
            || self.scene_transform != Some(scene_view.scene_transform)
            || animation_affects_draws(
                self.animation.as_ref(),
                self.animation_time,
                &scene_view.animation,
                scene_view.animation_time,
            );
        let frame_changed = camera_changed
            || self.frame_state.is_none_or(|state| {
                state.camera != scene_view.camera || state.projection != projection
            });
        let visibility_changed = camera_changed
            || self
                .frame_state
                .is_none_or(|state| !state.projection.visibility_matches(projection));
        let sampler_changed = self.sampler != Some(sampler);
        let voxel_shader_changed = self.voxel_shader != scene_view.voxel_shader;
        if !draw_state_changed
            && !frame_changed
            && !culling_bounds_changed
            && !node_mask_changed
            && !sampler_changed
            && !voxel_shader_changed
        {
            return Ok(());
        }
        // Camera-only movement normally changes uniforms and GPU cull arguments, not
        // the native draw command list. Invalidate commands below only when their
        // bindings, draw batches or CPU-pruned visibility actually change.
        self.command_delta = None;
        self.structural_bindings_spliced = false;
        self.prepared.set_culling_bounds(scene_view.culling_bounds);
        let mut draw_list_changed = draw_state_changed || voxel_shader_changed;
        let mut blend_order_changed = false;
        let mut reused_resident_slots = false;
        let mut resident_commands_stable = false;
        let mut native_structural_updated = false;
        let mut nodes_updated = false;
        if draw_state_changed {
            let stage_started = trace_timings().then(Instant::now);
            let camera_phase =
                gpui::gpu_owner_extension_phase(gpui::GpuOwnerExtensionPhase::CameraCull);
            let local_changes = self
                .scene
                .as_ref()
                .filter(|_| {
                    !culling_mode_changed
                        && scene_view.animation.is_empty()
                        && self.scene_transform == Some(scene_view.scene_transform)
                        && !(camera_changed && self.prepared.uses_lod())
                })
                .and_then(|previous| scene_view.scene.changed_nodes(previous));
            let updated = local_changes.and_then(|nodes| {
                self.prepared.update_nodes(
                    &scene_view.scene,
                    nodes.as_ref(),
                    scene_view.scene_transform,
                )
            });
            nodes_updated = updated.is_some();
            // A successful local update contains the complete new visible leaf set.
            // Unchanged leaves already have resident GPU meshes and textures.
            // Fall back to full scanning if a texture table or shader mode changes.
            // Streaming deltas add textures almost every frame, so a
            // table-pointer match would force the O(all draws) validation walk
            // on every moving frame. The delta draw list validates against the
            // CURRENT table, and the table prune only drops textures no
            // resident Section references, so the delta path is safe.
            let incremental_asset_sync = updated.is_some() && gpu_culling && !voxel_shader_changed;
            if updated.is_some() {
                self.prepared
                    .update_camera(scene_view.camera, projection.aspect())?;
            } else if scene_view.animation.is_empty()
                && let Some(prepared) = super::sidecar::try_take(
                    scene_view, projection.aspect(), gpu_culling,
                )?
            {
                // Independent CPU backend finished the immutable scene walk.
                // The GPU owner only finalizes camera/projection, asset
                // residency and native command submission. The mailbox
                // never carries an ExtensionDevice or native GPU object.
                self.prepared = prepared;
                if trace_timings() {
                    eprintln!(
                        "gpui3d sidecar_prepared draws={} source_scene_version={}",
                        self.prepared.draws.len(),
                        scene_view.scene.version(),
                    );
                }
            } else {
                // First frame, stale worker result, LOD camera mismatch or
                // disabled sidecar: preserve exactly the former rendering
                // behavior without waiting for any producer/worker.
                // The GPU owner already handled the publication directly,
                // so any redundant queued full walk can be invalidated.
                super::sidecar::discard(scene_view);
                update_prepared(
                    &mut self.prepared,
                    &mut self.animation_scratch,
                    scene_view,
                    projection.aspect(),
                )?;
            }
            drop(camera_phase);
            if let Some(started) = stage_started {
                walk_ms = started.elapsed().as_secs_f64() * 1000.0;
            }
            let stage_started = trace_timings().then(Instant::now);
            {
                let _phase =
                    gpui::gpu_owner_extension_phase(gpui::GpuOwnerExtensionPhase::ResourceSync);
                if incremental_asset_sync {
                    validate_draw_textures(self.prepared.incremental_asset_draws(), scene_view)?;
                } else {
                    validate_textures(&self.prepared, scene_view)?;
                }
                self.ensure_textures(device, scene_view, incremental_asset_sync)?;
                self.ensure_meshes_for_draws(
                    device,
                    context.backend_kind(),
                    scene_view.voxel_shader,
                    incremental_asset_sync,
                )?;
            }
            if let Some(started) = stage_started {
                assets_ms = started.elapsed().as_secs_f64() * 1000.0;
            }
        } else if camera_changed || culling_bounds_changed {
            let camera_started = trace_timings().then(Instant::now);
            let mut refresh_culled_assets = false;
            let camera_phase =
                gpui::gpu_owner_extension_phase(gpui::GpuOwnerExtensionPhase::CameraCull);
            let tracked_start = if gpu_culling {
                self.prepared
                    .draws
                    .partition_point(|draw| draw.material.alpha_mode != AlphaMode::Blend)
            } else {
                0
            };
            self.previous_draws.clear();
            if !gpu_culling {
                self.previous_draws.extend(
                    self.prepared
                        .draws
                        .iter()
                        .skip(tracked_start)
                        .map(draw_identity),
                );
            }
            if self
                .prepared
                .update_camera(scene_view.camera, projection.aspect())?
            {
                // A GPU-culled camera-only frame never moves draw rows: the
                // blended tail keeps its candidate order and only the render
                // permutation (blend_order) changes, which native commands
                // follow at rebuild time. GPU tables are untouched.
                draw_list_changed = voxel_shader_changed
                    || if gpu_culling {
                        false
                    } else {
                        self.previous_draws.len()
                            != self.prepared.draws.len().saturating_sub(tracked_start)
                            || self.previous_draws.iter().copied().ne(self
                                .prepared
                                .draws
                                .iter()
                                .skip(tracked_start)
                                .map(draw_identity))
                    };
                if gpu_culling {
                    blend_order_changed = self.prepared.blend_order_changed();
                }
                if draw_list_changed {
                    refresh_culled_assets = true;
                }
            }
            drop(camera_phase);
            if refresh_culled_assets {
                // CPU re-culling can reveal assets not uploaded by earlier frames.
                let _phase =
                    gpui::gpu_owner_extension_phase(gpui::GpuOwnerExtensionPhase::ResourceSync);
                self.ensure_textures(device, scene_view, false)?;
                self.ensure_meshes(device, context.backend_kind(), scene_view.voxel_shader)?;
            }
            if let Some(started) = camera_started {
                camera_ms = started.elapsed().as_secs_f64() * 1000.0;
            }
        }
        if voxel_shader_changed && !draw_state_changed {
            let _phase =
                gpui::gpu_owner_extension_phase(gpui::GpuOwnerExtensionPhase::ResourceSync);
            self.ensure_meshes(device, context.backend_kind(), scene_view.voxel_shader)?;
        }
        let shading = projection.voxel_shading();
        let shadow_enabled = scene_view.voxel_shader
            && shading.shadow_strength > 0.
            && shading.illumination == super::VoxelIllumination::Sunlight
            && shading.display != super::VoxelDisplay::Wireframe
            && device
                .compute_capabilities()
                .max_workgroups
                .iter()
                .all(|&size| size > 0);
        let shadow_created = shadow_enabled && self.shadow.is_none();
        if draw_list_changed {
            let batch_started = trace_timings().then(Instant::now);
            let _phase = gpui::gpu_owner_extension_phase(gpui::GpuOwnerExtensionPhase::BatchBuild);
            if gpu_culling {
                let native_quads = self.page_refs.has_non_generated_quads();
                if native_quads {
                    // This path can reorder opaque draws independently of
                    // the original CandidateGroup order, so future fixed
                    // candidate indices must not be treated as GPU slots.
                    self.prepared.invalidate_resident_candidate_order();
                }
                super::batching::group_voxel_batches_with_quad_presence(
                    &mut self.prepared.draws,
                    &self.meshes,
                    &mut self.batch_workspace,
                    &mut self.batches,
                    native_quads,
                );
            } else {
                self.batches.clear();
                self.batches
                    .extend(self.prepared.draw_batches().map(|batch| DrawBatchRange {
                        first_instance: batch.first_instance(),
                        instance_count: batch.draws().len(),
                    }));
            }
            if let Some(started) = batch_started {
                batch_ms = started.elapsed().as_secs_f64() * 1000.0;
            }
        }
        let resource_phase =
            gpui::gpu_owner_extension_phase(gpui::GpuOwnerExtensionPhase::ResourceSync);
        let mut cull_update_started = trace_timings().then(Instant::now);
        if gpu_culling && (draw_list_changed || self.culling.is_none()) {
            // Both proofs are maintained incrementally: no non-generated
            // resident Mesh and no native Quad permutation. The GPU culling
            // Batch ABI is then [generated=1, count=1, 0, logical_index]
            // regardless of which Section occupies each dense index.
            let generated_singletons = self.page_refs.only_generated_meshes()
                && self.batch_workspace.singleton_layout()
                && self.batches.len() == self.prepared.draws.len();
            if trace_timings() {
                eprintln!(
                    "gpui3d cull_gate draws={} nodes_updated={} delta={} only_generated={} singleton_layout={} batches_eq_draws={} resident_prefix={} structural_committed={}",
                    self.prepared.draws.len(),
                    nodes_updated,
                    self.prepared.structural_draw_delta().is_some(),
                    self.page_refs.only_generated_meshes(),
                    self.batch_workspace.singleton_layout(),
                    self.batches.len() == self.prepared.draws.len(),
                    self.prepared.resident_prefix_order_proven(),
                    self.culling
                        .as_ref()
                        .is_some_and(CullResources::structural_slots_committed),
                );
            }
            if let Some(culling) = &mut self.culling {
                // Candidate Delta carries exact logical Draw positions when
                // GPU opaque slot identity is preserved. With only singleton
                // batches the native Batch and logical->physical tables are
                // unchanged: patch only the affected resident Bounds rows.
                // Quad instancing/structural deltas retain the full planner.
                let delta = self
                    .prepared
                    .resident_delta_draws()
                    .filter(|_| !self.page_refs.has_non_generated_quads());
                let reused = if let Some(dirty) = delta {
                    culling.update_dirty_opaque(
                        device,
                        &self.prepared.draws,
                        &self.batches,
                        Some(&self.meshes),
                        dirty,
                    )?
                } else {
                    false
                };
                if !reused {
                    culling.update_generated_singletons(
                        device,
                        &self.prepared.draws,
                        &self.batches,
                        Some(&self.meshes),
                        generated_singletons,
                        self.prepared.structural_draw_delta(),
                    )?;
                }
                reused_resident_slots = reused;
            } else {
                let mut culling = CullResources::create(
                    device,
                    &self.prepared.draws,
                    &self.batches,
                    self.backend,
                    Some(&self.meshes),
                )?;
                // Initial creation already computed the full Batch table.
                // Mark it eligible for tail-only growth on future deltas.
                culling.mark_generated_singletons(generated_singletons);
                self.culling = Some(culling);
            }
        } else if !gpu_culling && let Some(previous) = self.culling.take() {
            previous.destroy(device)?;
        }
        if let Some(started) = cull_update_started.take() {
            cull_update_ms = started.elapsed().as_secs_f64() * 1000.0;
        }
        if !shadow_enabled
            && (draw_state_changed || culling_bounds_changed || node_mask_changed)
            && let Some(shadow) = &mut self.shadow
        {
            shadow.invalidate();
        }
        if shadow_created {
            self.shadow = Some(super::shadow::Shadow::create(device, self.backend)?);
        }
        if shadow_enabled && let Some(shadow) = &mut self.shadow {
            shadow.update(
                device,
                self.prepared.occluder_bounds_excluding(
                    scene_view.culling_bounds,
                    shadow_hidden_nodes,
                ),
                scene_view.camera.eye,
                shading.sun_direction,
                draw_state_changed || culling_bounds_changed || node_mask_changed,
            )?;
        }
        let wire_changed = self
            .frame_state
            .is_some_and(|previous| previous.projection.voxel_shading().display != shading.display);
        if frame_changed {
            self.sync_frame_params(device, projection, scene_view.camera)?;
            self.frame_state = Some(FrameState {
                camera: scene_view.camera,
                projection,
            });
        }
        if draw_list_changed {
            let buffer_started = trace_timings().then(Instant::now);
            let _phase =
                gpui::gpu_owner_extension_phase(gpui::GpuOwnerExtensionPhase::BufferEncode);
            let bind_group_layout = renderer.bind_group_layout()?;
            let mut selected_sync = DrawSyncPath::Full;
            {
                // Fixed-layout opaque Candidate Deltas have already updated
                // only their resident GPU Bounds rows in CullResources. Use
                // the same physical slots for sparse DrawParams/Instance
                // uploads, preserving all native and indirect draw addresses.
                // If the CPU mirrors, capacities or layout cannot be proven
                // stable, sync_buffers() safely re-plans the entire table.
                let textures_unchanged = self
                    .texture_table
                    .as_ref()
                    .is_some_and(|previous| Arc::ptr_eq(previous, &scene_view.textures));
                let stage_started = trace_timings().then(Instant::now);
                let result = if reused_resident_slots {
                    self.sync_resident_draw_delta(
                        device,
                        renderer,
                        bind_group_layout,
                        sampler,
                        sampler_changed,
                        assets_changed,
                        textures_unchanged,
                    )?
                } else {
                    None
                };
                resident_ms = stage_started
                    .map(|started| started.elapsed().as_secs_f64() * 1000.0)
                    .unwrap_or(0.0);
                resident_commands_stable = result.unwrap_or(false);
                if result.is_some() {
                    selected_sync = DrawSyncPath::Resident;
                }
                if result.is_none() {
                    let stage_started = trace_timings().then(Instant::now);
                    let structural_updated = self
                        .culling
                        .as_ref()
                        .is_some_and(CullResources::structural_slots_committed)
                        && self.sync_structural_draw_delta(
                            device,
                            renderer,
                            bind_group_layout,
                            sampler,
                            sampler_changed,
                            assets_changed,
                        )?;
                    structural_ms = stage_started
                        .map(|started| started.elapsed().as_secs_f64() * 1000.0)
                        .unwrap_or(0.0);
                    native_structural_updated = structural_updated;
                    if structural_updated {
                        selected_sync = DrawSyncPath::Structural;
                    }
                    if !structural_updated {
                        let stage_started = trace_timings().then(Instant::now);
                        self.sync_buffers(
                            device,
                            renderer,
                            bind_group_layout,
                            sampler,
                            sampler_changed,
                            scene_view.voxel_shader,
                            assets_changed,
                        )?;
                        sbuffers_ms = stage_started
                            .map(|started| started.elapsed().as_secs_f64() * 1000.0)
                            .unwrap_or(0.0);
                    }
                }
            }
            self.draw_sync_audit.record(
                selected_sync,
                gpu_culling,
                self.prepared.resident_delta_draws().is_some(),
                self.prepared.structural_draw_delta().is_some(),
                self.culling.as_ref().is_some_and(CullResources::structural_slots_committed),
                self.prepared.draws.len(),
            );
            if let Some(started) = buffer_started {
                buffer_ms = started.elapsed().as_secs_f64() * 1000.0;
            }
        } else if sampler_changed || shadow_created {
            let stage_started = trace_timings().then(Instant::now);
            let outcome = self.sync_draw_resources(
                device,
                renderer,
                renderer.bind_group_layout()?,
                sampler,
                sampler_changed,
                scene_view.voxel_shader,
                false,
                !assets_changed,
            );
            sdres_ms = stage_started
                .map(|started| started.elapsed().as_secs_f64() * 1000.0)
                .unwrap_or(0.0);
            outcome?;
        }
        if assets_changed {
            // The common streaming update only adds new section meshes. No
            // existing GPU resource can become unreachable from an add-only
            // scene delta, so avoid repeatedly walking every resident
            // bind group, texture, and mesh. Keep the full sweep for removed
            // references, GPU mesh replacement, or culling buffer relocation.
            let references_removed = self
                .active_assets
                .sync(self.scene.as_deref(), scene_view.scene());
            let visible_instances_changed = previous_visible_instances
                != self
                    .culling
                    .as_ref()
                    .map(|culling| culling.visible_instances);
            let mesh_replaced = !self.retired_meshes.is_empty();
            if references_removed || mesh_replaced || visible_instances_changed {
                // The normal Section Delta path contains exact final-reference
                // removals. Do not walk every resident mesh and BindGroup when
                // only a handful of Sections were unloaded.
                if references_removed
                    && !mesh_replaced
                    && !visible_instances_changed
                    && !self.active_assets.needs_full_rescan()
                {
                    let (removed_meshes, removed_textures) = self.active_assets.removed_ids();
                    let removed_meshes = removed_meshes.to_vec();
                    let removed_textures = removed_textures.to_vec();
                    self.retire_binding_delta(device, &removed_meshes, &removed_textures)?;
                    self.retire_texture_delta(device, &removed_textures)?;
                    self.retire_mesh_delta(&removed_meshes);
                } else {
                    retirement_scan = true;
                    let (active_meshes, active_textures) = self.active_assets.ids();
                    self.retire_bindings(device, &active_meshes, &active_textures)?;
                    if references_removed {
                        self.retire_textures(device, &active_textures)?;
                        self.retire_meshes(&active_meshes);
                    }
                }
            }
        }
        for mesh in self.retired_meshes.drain(..) {
            destroy_mesh(
                device,
                mesh,
                &mut self.arena,
                &mut *self.generated_arena.borrow_mut(),
                &mut *self.template_arena.borrow_mut(),
                &mut *self.argument_arena.borrow_mut(),
            )?;
        }
        if gpu_culling {
            // Patch only changed 4-byte portal visibility words at the
            // stable physical Bounds slots. No Scene fork / DrawParams upload.
            // Do this AFTER structural/sparse Bounds updates, so newly reused
            // slots cannot inherit another Section's hidden state.
            // Camera-only frames keep the exact same immutable visibility
            // snapshot and resident Bounds rows. Avoid rehashing every hidden
            // NodeId and rebuilding the full physical-slot HashSet at 60+ FPS.
            // Structural/asset changes may overwrite GPU Bounds rows and must
            // reapply the overlay, even when the mask itself is unchanged.
            let mask_uploaded = if node_mask_changed || draw_list_changed || draw_state_changed {
                if let Some(culling) = &mut self.culling {
                    culling.update_hidden_nodes(
                        device, &self.prepared.draws, &scene_view.hidden_nodes,
                    )?
                } else {
                    false
                }
            } else {
                false
            };
            if (draw_state_changed || culling_bounds_changed || visibility_changed || wire_changed
                || node_mask_changed)
                && let Some(culling) = &mut self.culling
            {
                culling.hiz.update(
                    device,
                    self.prepared.occluder_bounds_excluding(
                        scene_view.culling_bounds,
                        &scene_view.hidden_nodes,
                    ),
                    scene_view.camera.eye,
                    scene_view.camera.target - scene_view.camera.eye,
                    draw_state_changed || culling_bounds_changed || wire_changed || node_mask_changed,
                )?;
            }
            if visibility_changed || draw_list_changed || culling_bounds_changed || wire_changed
                || mask_uploaded
            {
                let dispatch_started = trace_timings().then(Instant::now);
                self.culling
                    .as_mut()
                    .ok_or_else(|| anyhow!("GPU culling resources missing"))?
                    .dispatch_optimized(
                        device,
                        renderer
                            .cull_shader
                            .ok_or_else(|| anyhow!("GPU culling shader missing"))?,
                        renderer.singleton_cull_shader,
                        self.frame_buffer,
                        scene_view.culling_bounds,
                        self.generated_arguments,
                    )?;
                if let Some(started) = dispatch_started {
                    cull_dispatch_ms = started.elapsed().as_secs_f64() * 1000.0;
                }
            }
        }
        drop(resource_phase);
        // A shading-only frame update changes uniforms but never the CPU
        // command inclusion mask. Avoid checking every resident Draw batch
        // unless the draw layout or visibility volume actually changes.
        // A canonical all-singleton scene publishes every Section command and
        // lets the GPU cull own visibility, so the CPU mask is never consumed;
        // skipping its per-camera refresh also keeps the native command list
        // stable while the camera moves.
        let singleton_only = self.generated_singleton_command_count().is_some();
        let command_visibility_changed = if singleton_only {
            false
        } else if reused_resident_slots && !visibility_changed && !culling_bounds_changed {
            // The culling system has validated unchanged physical/layout
            // identities. Recheck CPU-native inclusion only for the handful
            // of dirty singleton draws; avoid a second whole-scene frustum
            // pass for an ordinary fixed-camera Section update.
            let dirty = self
                .prepared
                .resident_delta_draws()
                .map(|indices| indices.to_vec());
            dirty
                .as_deref()
                .and_then(|indices| self.refresh_dirty_command_visibility(indices))
                .unwrap_or_else(|| self.refresh_command_visibility())
        } else if draw_list_changed || visibility_changed || culling_bounds_changed {
            self.refresh_command_visibility()
        } else {
            false
        };
        if native_structural_updated
            && self.structural_bindings_spliced
            && !culling_bounds_changed
            && !sampler_changed
            && !shadow_created
            && !voxel_shader_changed
            && !wire_changed
            && self.generated_singleton_command_count().is_some()
        {
            self.command_delta = self.prepared.structural_draw_delta().cloned();
        }
        if (draw_list_changed && !resident_commands_stable)
            || sampler_changed
            || shadow_created
            || voxel_shader_changed
            || wire_changed
            || command_visibility_changed
            || blend_order_changed
        {
            self.command_revision = self.command_revision.wrapping_add(1);
        }
        self.scene = Some(scene_view.scene.clone());
        self.texture_table = Some(scene_view.textures.clone());
        self.animation = Some(scene_view.animation.clone());
        self.animation_time = Some(scene_view.animation_time);
        self.camera = Some(scene_view.camera);
        self.culling_bounds = scene_view.culling_bounds;
        self.hidden_nodes = Arc::clone(&scene_view.hidden_nodes);
        self.scene_transform = Some(scene_view.scene_transform);
        self.projection = Some(projection);
        self.sampler = Some(sampler);
        self.voxel_shader = scene_view.voxel_shader;
        // Scene/texture, draw, frame uniform, culling, sampling and shader
        // state has now been committed to the GPU owner. Offscreen producer
        // textures are reusable until this epoch changes. Failed prepares
        // never publish a new revision.
        self.capture_revision = self.capture_revision.wrapping_add(1);
        if let Some(started) = trace_started {
            let total_ms = started.elapsed().as_secs_f64() * 1000.0;
            if total_ms >= 16.0 {
                eprintln!(
                    "gpui3d prepare_detail draws={} assets_changed={} retirement_scan={} camera_ms={:.2} walk_ms={:.2} assets_ms={:.2} batch_ms={:.2} buffer_ms={:.2} (resident {:.2} structural {:.2} sbuffers {:.2} sdres {:.2}) cull_update={:.2} cull_dispatch={:.2} total_ms={:.2} batches={} merged_instances={}",
                    self.prepared.draws.len(),
                    assets_changed,
                    retirement_scan,
                    camera_ms,
                    walk_ms,
                    assets_ms,
                    batch_ms,
                    buffer_ms,
                    resident_ms,
                    structural_ms,
                    sbuffers_ms,
                    sdres_ms,
                    cull_update_ms,
                    cull_dispatch_ms,
                    total_ms,
                    self.batches.len(),
                    self.prepared.draws.len().saturating_sub(self.batches.len()),
                );
            }
        }
        Ok(())
    }

    /// Tracks the native command inclusion mask for single-instance GPU-culling
    /// batches. GPU indirect arguments can change each frame without replacing
    /// their draw commands, but CPU frustum-pruned batches must invalidate the list.
    /// The scratch allocation is retained across frames and section growth.
    fn refresh_command_visibility(&mut self) -> bool {
        let gpu_culling = self.culling.is_some();
        let prepared = &self.prepared;
        let visible = prepared.bounds_visibility();
        update_command_visibility(
            &mut self.command_visibility,
            self.batches.iter().map(|batch| {
                !gpu_culling
                    || batch.instance_count != 1
                    || prepared
                        .draws
                        .get(batch.first_instance)
                        .is_some_and(|draw| {
                            visible(draw.bounds) && prepared.is_draw_in_culling_bounds(draw)
                        })
            }),
        )
    }

    /// O(changed Section draws) CPU-native command visibility update.
    /// Full-camera movement and any unsupported batch arrangement retain the
    /// original full mask builder.
    fn refresh_dirty_command_visibility(&mut self, dirty: &[usize]) -> Option<bool> {
        if self.culling.is_none() || self.command_visibility.len() != self.batches.len() {
            return None;
        }
        let prepared = &self.prepared;
        let visible = prepared.bounds_visibility();
        update_dirty_command_visibility(&mut self.command_visibility, dirty, |index| {
            let batch = self.batches.get(index)?;
            if batch.instance_count != 1 || batch.first_instance != index {
                return None;
            }
            let draw = prepared.draws.get(index)?;
            Some(visible(draw.bounds) && prepared.is_draw_in_culling_bounds(draw))
        })
    }

    /// Uploads the camera, viewport, and extension-bounds values shared by every draw.
    fn sync_frame_params(
        &mut self,
        device: &mut dyn ExtensionDevice,
        projection: FrameProjection,
        camera: Camera,
    ) -> gpui::Result<()> {
        let mut bytes = encode_frame_params(
            self.prepared.view_projection,
            camera,
            projection.bounds(),
            projection.target_extent(),
            projection.blend_edge_feather(),
            u32::try_from(self.prepared.lights.len()).unwrap_or(u32::MAX),
        );
        if let Some(fog) = projection.fog() {
            for (destination, value) in bytes[112..].chunks_exact_mut(4).zip(fog.encoded()) {
                destination.copy_from_slice(&value.to_le_bytes());
            }
        }
        for (destination, value) in bytes[144..192]
            .chunks_exact_mut(4)
            .zip(projection.voxel_shading().encoded())
        {
            destination.copy_from_slice(&value.to_le_bytes());
        }
        if projection.voxel_shading().display != super::VoxelDisplay::Wireframe
            && let Some(shadow) = &self.shadow
        {
            for (dst, value) in bytes[192..256]
                .chunks_exact_mut(4)
                .zip(shadow.matrix.columns().into_iter().flatten())
            {
                dst.copy_from_slice(&value.to_le_bytes());
            }
            for (dst, value) in bytes[256..].chunks_exact_mut(4).zip([
                projection.voxel_shading().shadow_strength,
                0.0008,
                512.,
                0.,
            ]) {
                dst.copy_from_slice(&value.to_le_bytes());
            }
        }
        if projection.is_hdr() {
            let flags = if projection.has_water_animation() {
                3f32
            } else {
                1f32
            };
            bytes[268..272].copy_from_slice(&flags.to_le_bytes());
        }
        if self.uploaded_frame_params != bytes {
            device.write_buffer(self.frame_buffer, 0, &bytes)?;
            // Commit the exact byte identity only after the native upload
            // succeeds; failed writes must be retried on the next update.
            self.uploaded_frame_params = bytes;
        }
        Ok(())
    }

    /// A pure world-transform delta changes only GPU Bounds/Instance rows.
    /// Skip the O(all batches) BindGroup and generated Page mapping passes
    /// only when every changed draw still refers to the same immutable mesh
    /// generation, material texture set and already-bound GPU resources.
    /// Structural/texture/shadow changes use the original full sync.
    fn resident_bindings_unchanged(
        &self,
        dirty: &[usize],
        sampler_changed: bool,
        textures_unchanged: bool,
    ) -> bool {
        if sampler_changed
            || !textures_unchanged
            || !self.retired_meshes.is_empty()
            || self.generated_pending_groups.is_some()
            || self.draw_resource_sources.len() != self.batches.len()
            || self.draw_resource_keys.len() != self.batches.len()
            || self.draw_resources.len() != self.batches.len()
            || self.generated_batch_sources.len() != self.batches.len()
        {
            return false;
        }
        let shadow = self.shadow.as_ref().map(|shadow| shadow.raster.depths);
        let visible = self
            .culling
            .as_ref()
            .map(|culling| culling.visible_instances);
        dirty.iter().all(|&index| {
            let Some(draw) = self.prepared.draws.get(index) else {
                return false;
            };
            let source = (mesh_key(draw, true), draw.mesh.generation());
            if self.draw_resource_sources.get(index) != Some(&source) {
                return false;
            }
            let Some(key) = self.draw_resource_keys.get(index) else {
                return false;
            };
            key.shadow == shadow
                && key.visible_instances == visible
                && key.albedo_id == draw.material.albedo_texture
                && key.normal_id == draw.material.normal_texture
                && key.occlusion_id == draw.material.occlusion_texture
                && key.metallic_roughness_id == draw.material.metallic_roughness_texture
                && self
                    .generated_batch_sources
                    .get(index)
                    .is_some_and(|metadata| metadata.is_none_or(|source| source.part == draw.part))
        })
    }

    /// A native command is independent of world translation. Reuse it only
    /// if every changed singleton retains the exact material snapshot
    /// (pipeline/rejection/alpha state) and raster winding direction.
    fn resident_command_pipelines_unchanged(&self, dirty: &[usize]) -> bool {
        self.native_command_sources.len() == self.batches.len()
            && dirty.iter().all(|&logical| {
                self.prepared.draws.get(logical).is_some_and(|draw| {
                    self.native_command_sources.get(logical)
                        == Some(&NativeCommandSource::for_draw(draw))
                })
            })
    }

    /// Section Delta -> retained GPU Draw Slot update. The Culler has
    /// already validated and committed stable opaque singleton identities.
    /// We write only changed DrawParams/Instance records at their physical
    /// addresses, leaving the logical slot map and native command addresses
    /// untouched. Any unexpected capacity/layout change uses sync_buffers().
    fn sync_resident_draw_delta(
        &mut self,
        device: &mut dyn ExtensionDevice,
        renderer: &RendererResources,
        bind_group_layout: BindGroupLayoutId,
        sampler: SamplerId,
        sampler_changed: bool,
        defer_binding_cleanup: bool,
        textures_unchanged: bool,
    ) -> gpui::Result<Option<bool>> {
        let Some(dirty) = self
            .prepared
            .resident_delta_draws()
            .map(|indices| indices.to_vec())
        else {
            return Ok(None);
        };
        let Some(culler) = &self.culling else {
            return Ok(None);
        };
        let (high_water, mapping) = culler.committed_slot_layout();
        let capacity = high_water.max(1);
        if high_water == 0
            || mapping.len() != self.prepared.draws.len()
            || self.draw_slot_keys.len() != high_water
            || self.instance_slot_keys.len() != high_water
            || self.uploaded_tables[0].len() != capacity * DRAW_PARAMS_STRIDE as usize
            || self.uploaded_tables[1].len() != capacity * INSTANCE_STRIDE as usize
            || self.draw_capacity < capacity
            || self.instance_capacity < capacity
            || self.uploaded_slot_map.len() != self.prepared.draws.len() * 4
            || self.draw_addresses.len() != self.batches.len()
            || self.prepared.draws.len() != self.batches.len()
            || dirty.len().saturating_mul(4) > self.prepared.draws.len()
        {
            return Ok(None);
        }
        let commands_unchanged =
            self.resident_bindings_unchanged(&dirty, sampler_changed, textures_unchanged)
                && self.resident_command_pipelines_unchanged(&dirty);
        let trace_started = trace_timings().then(Instant::now);
        let mut scratch = std::mem::take(&mut self.scratch_packed_draws);
        let draw_result = self.draw_record_cache.encode_resident_delta(
            &self.prepared.draws,
            self.prepared.lights.len(),
            mapping,
            &dirty,
            &self.draw_slot_keys,
            &self.uploaded_tables[0],
            &mut scratch,
        );
        self.scratch_packed_draws = scratch;
        let draw_patches = draw_result?;
        let mut scratch = std::mem::take(&mut self.dirty_instance_run);
        let instance_result = self.encode_resident_instance_delta(
            mapping,
            &dirty,
            &self.instance_slot_keys,
            &self.uploaded_tables[1],
            &mut scratch,
        );
        self.dirty_instance_run = scratch;
        let instance_patches = instance_result?;

        let mut writes = Vec::with_capacity(draw_patches.len() + instance_patches.len());
        for (slot, _, bytes) in &draw_patches {
            writes.push(BufferWrite {
                descriptor: BufferWriteDescriptor {
                    buffer: self.draw_buffer,
                    offset: *slot as u64 * DRAW_PARAMS_STRIDE as u64,
                },
                data: &bytes[..],
            });
        }
        for (slot, _, bytes) in &instance_patches {
            writes.push(BufferWrite {
                descriptor: BufferWriteDescriptor {
                    buffer: self.instance_buffer,
                    offset: *slot as u64 * INSTANCE_STRIDE as u64,
                },
                data: &bytes[..],
            });
        }
        let draw_patch_count = draw_patches.len();
        let instance_patch_count = instance_patches.len();
        let uploaded_bytes = writes.iter().map(|write| write.data.len()).sum::<usize>();
        if !writes.is_empty() {
            let upload_started = Instant::now();
            device.write_buffer_batch(&writes)?;
            crate::upload_feedback::observe_buffer_upload(
                self.backend, uploaded_bytes, writes.len(), upload_started.elapsed(),
            );
        }
        drop(writes);
        // A new Mesh generation can change shared Page bindings and GPU Part
        // Counts even when DrawParams bytes remain identical. Keep the same
        // binding/mapping lifecycle as the full sync path.
        if !self.resident_bindings_unchanged(&dirty, sampler_changed, textures_unchanged) {
            self.sync_draw_resources(
                device,
                renderer,
                bind_group_layout,
                sampler,
                sampler_changed,
                true,
                defer_binding_cleanup,
                false,
            )?;
            self.sync_generated_draws(device, true)?;
        }

        // Commit exact CPU mirrors only after the GPU updates and resource
        // bindings have succeeded. A failed operation must be retried against
        // the previous committed snapshot, not silently skipped.
        for (slot, key, bytes) in draw_patches {
            self.draw_slot_keys[slot] = key;
            let start = slot * DRAW_PARAMS_STRIDE as usize;
            self.uploaded_tables[0][start..start + DRAW_PARAMS_STRIDE as usize]
                .copy_from_slice(&bytes);
        }
        for (slot, key, bytes) in instance_patches {
            self.instance_slot_keys[slot] = key;
            let start = slot * INSTANCE_STRIDE as usize;
            self.uploaded_tables[1][start..start + INSTANCE_STRIDE as usize]
                .copy_from_slice(&bytes);
        }
        if let Some(started) = trace_started {
            eprintln!(
                "gpui3d resident_delta draws={} dirty={} draw_patches={} instance_patches={} uploaded_bytes={} native_commands_stable={} ms={:.2}",
                self.prepared.draws.len(),
                dirty.len(),
                draw_patch_count,
                instance_patch_count,
                uploaded_bytes,
                commands_unchanged,
                started.elapsed().as_secs_f64() * 1000.0,
            );
        }
        Ok(Some(commands_unchanged))
    }

    /// A structural Section splice changes dense logical positions but
    /// leaves all survivor DrawParams/Instance rows at their original physical
    /// GPU addresses. The Culler has already committed the new slot registry.
    /// Touch only newly mapped Section rows and the unavoidable compact
    /// u32 indirection suffix; do not materialize any full physical table.
    fn sync_structural_draw_delta(
        &mut self,
        device: &mut dyn ExtensionDevice,
        renderer: &RendererResources,
        bind_group_layout: BindGroupLayoutId,
        sampler: SamplerId,
        sampler_changed: bool,
        defer_binding_cleanup: bool,
    ) -> gpui::Result<bool> {
        let Some(delta) = self.prepared.structural_draw_delta() else {
            return Ok(false);
        };
        let Some(culler) = &self.culling else {
            return Ok(false);
        };
        let (high_water, mapping) = culler.committed_slot_layout();
        let count = self.prepared.draws.len();
        let previous_count = delta.previous_count;
        let old_high_water = self.draw_slot_keys.len();
        let offset = delta
            .edits
            .iter()
            .map(|edit| edit.new.start)
            .min()
            .unwrap_or(count);
        let dirty = delta
            .edits
            .iter()
            .flat_map(|edit| edit.new.clone())
            .collect::<Vec<_>>();
        if high_water == 0
            || old_high_water == 0
            || mapping.len() != count
            || high_water < old_high_water
            || self.instance_slot_keys.len() != old_high_water
            || self.uploaded_tables[0].len() != old_high_water * DRAW_PARAMS_STRIDE as usize
            || self.uploaded_tables[1].len() != old_high_water * INSTANCE_STRIDE as usize
            || self.uploaded_slot_map.len() != previous_count * 4
            || self.draw_capacity < high_water
            || self.instance_capacity < high_water
            || self.draw_slot_capacity < count
            || self.draw_addresses.len() != previous_count
            || self.batches.len() != count
            || offset > count
            || !self.page_refs.only_generated_meshes()
            || !self.batch_workspace.singleton_layout()
        {
            return Ok(false);
        }
        // A streaming publish rebuilds the texture table Arc almost every
        // frame, but Sections own self-contained bind groups: untouched
        // groups keep their pinned texture views and new Sections resolve
        // against the current table by asset id, so a changed table identity
        // cannot stale them. The draw-ratio guard is likewise gone: the
        // whole-tail structural edit legitimately dirties every blend row
        // and patch bytes stay bounded by the table size.
        // A changed light *count* changes every DrawParams.light_count and
        // must use full sync. When the count and buffer allocation are stable,
        // changed light values are an independent sparse storage-buffer patch:
        // survivor DrawParams, instances, BindGroups and logical slot indices
        // are unchanged. This is common while torches or sky lighting update.
        let mut light_bytes = Vec::new();
        encode_lights_into(&self.prepared.lights, &mut light_bytes);
        if !structural_light_patchable(
            &self.uploaded_tables[2], &light_bytes, self.light_capacity,
        ) {
            return Ok(false);
        }
        let light_ranges = uploads::changed_ranges(
            &self.uploaded_tables[2],
            &light_bytes,
            LIGHT_STRIDE as usize,
            sparse_upload_range_budget(self.backend, count),
            4,
        );

        let trace_started = trace_timings().then(Instant::now);
        let mut scratch = std::mem::take(&mut self.scratch_packed_draws);
        let draw_result = self.draw_record_cache.encode_resident_delta(
            &self.prepared.draws,
            self.prepared.lights.len(),
            mapping,
            &dirty,
            &self.draw_slot_keys,
            &self.uploaded_tables[0],
            &mut scratch,
        );
        self.scratch_packed_draws = scratch;
        let draw_patches = draw_result?;
        let mut scratch = std::mem::take(&mut self.dirty_instance_run);
        let instance_result = self.encode_resident_instance_delta(
            mapping,
            &dirty,
            &self.instance_slot_keys,
            &self.uploaded_tables[1],
            &mut scratch,
        );
        self.dirty_instance_run = scratch;
        let instance_patches = instance_result?;

        // The culler already committed exact structural physical slots.
        // Publish the same narrow changed logical ranges to Draw Slot shaders
        // instead of re-encoding the full suffix after every Section edit.
        let slot_patches = structural_draw_slot_patches(
            &self.uploaded_slot_map, mapping, delta,
        ).unwrap_or_else(|| {
            let mut suffix = Vec::with_capacity((count - offset) * 4);
            for &slot in &mapping[offset..] {
                suffix.extend_from_slice(&slot.to_le_bytes());
            }
            if suffix.is_empty() { Vec::new() } else { vec![(offset * 4, suffix)] }
        });
        let mut writes = Vec::with_capacity(
            draw_patches.len() + instance_patches.len() + slot_patches.len(),
        );
        for (slot, _, bytes) in &draw_patches {
            writes.push(BufferWrite {
                descriptor: BufferWriteDescriptor {
                    buffer: self.draw_buffer,
                    offset: *slot as u64 * DRAW_PARAMS_STRIDE as u64,
                },
                data: &bytes[..],
            });
        }
        for (slot, _, bytes) in &instance_patches {
            writes.push(BufferWrite {
                descriptor: BufferWriteDescriptor {
                    buffer: self.instance_buffer,
                    offset: *slot as u64 * INSTANCE_STRIDE as u64,
                },
                data: &bytes[..],
            });
        }
        for (offset, bytes) in &slot_patches {
            writes.push(BufferWrite {
                descriptor: BufferWriteDescriptor {
                    buffer: self.draw_slot_buffer,
                    offset: *offset as u64,
                },
                data: bytes,
            });
        }
        for range in &light_ranges {
            writes.push(BufferWrite {
                descriptor: BufferWriteDescriptor {
                    buffer: self.light_buffer,
                    offset: range.start as u64,
                },
                data: &light_bytes[range.clone()],
            });
        }
        let bytes_uploaded = writes.iter().map(|write| write.data.len()).sum::<usize>();
        if !writes.is_empty() {
            let upload_started = Instant::now();
            device.write_buffer_batch(&writes)?;
            crate::upload_feedback::observe_buffer_upload(
                self.backend, bytes_uploaded, writes.len(), upload_started.elapsed(),
            );
        }
        drop(writes);
        // The stable physical Draw Slot table is already committed by the
        // culler. Copy BindGroup metadata only across unchanged logical
        // intervals, resolving Mesh/Material bindings for new Sections only.
        // Generated Page mapping remains a separate GPU compute publication.
        let spliced_bindings = self.sync_structural_draw_resources(
            device,
            renderer,
            bind_group_layout,
            sampler,
            sampler_changed,
        )?;
        self.structural_bindings_spliced = spliced_bindings;
        if !spliced_bindings {
            self.sync_draw_resources(
                device,
                renderer,
                bind_group_layout,
                sampler,
                sampler_changed,
                true,
                defer_binding_cleanup,
                false,
            )?;
        }
        // When the native bindings were copied by structural splice, the
        // old Page sources and CPU logical slot map are still available.
        // Update only Pages owning a changed physical slot; a full binding
        // fallback or GPU count reallocation publishes all Pages normally.
        if !spliced_bindings || !self.sync_structural_generated_pages(device)? {
            self.sync_generated_draws(device, true)?;
        }

        // All dependent GPU operations completed successfully. Extend only
        // the new physical tail; unchanged rows remain byte-identical.
        let draw_fill = *self
            .draw_slot_keys
            .last()
            .ok_or_else(|| anyhow!("missing previous GPU DrawParams snapshot"))?;
        let instance_fill = *self
            .instance_slot_keys
            .last()
            .ok_or_else(|| anyhow!("missing previous GPU Instance snapshot"))?;
        self.draw_slot_keys.resize(high_water, draw_fill);
        self.instance_slot_keys.resize(high_water, instance_fill);
        self.uploaded_tables[0].resize(high_water * DRAW_PARAMS_STRIDE as usize, 0);
        self.uploaded_tables[1].resize(high_water * INSTANCE_STRIDE as usize, 0);
        for (slot, key, bytes) in draw_patches {
            self.draw_slot_keys[slot] = key;
            let start = slot * DRAW_PARAMS_STRIDE as usize;
            self.uploaded_tables[0][start..start + DRAW_PARAMS_STRIDE as usize]
                .copy_from_slice(&bytes);
        }
        for (slot, key, bytes) in instance_patches {
            self.instance_slot_keys[slot] = key;
            let start = slot * INSTANCE_STRIDE as usize;
            self.uploaded_tables[1][start..start + INSTANCE_STRIDE as usize]
                .copy_from_slice(&bytes);
        }
        self.uploaded_slot_map.resize(count * 4, 0);
        for (offset, bytes) in &slot_patches {
            self.uploaded_slot_map[*offset..*offset + bytes.len()].copy_from_slice(bytes);
        }
        // Commit the new light snapshot only after all native writes,
        // bindings and generated mappings succeeded.
        if !light_ranges.is_empty() {
            self.uploaded_tables[2] = light_bytes;
        }
        // Canonical singleton DrawIndirect offsets are a function of their
        // current logical index, independent of which Section owns the slot.
        // The structural path must update the effective native command
        // address count as well as GPU buffers. A middle insert/remove
        // changes the owner at each index but never its {index * 16, index}
        // address; only append or truncate the dense address tail.
        let old_addresses = self.draw_addresses.len();
        self.draw_addresses.truncate(count);
        for index in old_addresses..count {
            self.draw_addresses.push(DrawAddress {
                arguments_offset: index as u64 * ARGUMENT_STRIDE,
                first_instance: index,
            });
        }
        if let Some(started) = trace_started {
            eprintln!(
                "gpui3d structural_slot_delta previous={} current={} dirty={} slot_patch_ranges={} slot_patch_bytes={} light_patch_ranges={} uploaded_bytes={} ms={:.2}",
                previous_count,
                count,
                dirty.len(),
                slot_patches.len(),
                slot_patches.iter().map(|(_, data)| data.len()).sum::<usize>(),
                light_ranges.len(),
                bytes_uploaded,
                started.elapsed().as_secs_f64() * 1000.0,
            );
        }
        Ok(true)
    }

    fn sync_buffers(
        &mut self,
        device: &mut dyn ExtensionDevice,
        renderer: &RendererResources,
        bind_group_layout: BindGroupLayoutId,
        sampler: SamplerId,
        sampler_changed: bool,
        voxel_shader: bool,
        defer_binding_cleanup: bool,
    ) -> gpui::Result<()> {
        let trace_started = trace_timings().then(Instant::now);
        // Culling retains its dense logical order; voxel DrawParams/Instance
        // live in persistent physical slots. The u32 mapping is the only
        // dense table that moves when an earlier Section is inserted.
        // The culler already owns the authoritative logical->physical
        // Section slot mapping. Borrow its resident snapshot directly:
        // cloning a 100K+ u32 map on every scene delta just to re-encode its
        // identical bytes wastes bandwidth and allocates on the GPU owner.
        //
        // A locally planned successor is necessary only when the culler is
        // unavailable, and remains transactional until all GPU writes succeed.
        let culling_layout = if voxel_shader {
            self.culling
                .as_ref()
                .filter(|culling| culling.counts().1 == self.prepared.draws.len())
                .map(CullResources::committed_slot_layout)
        } else {
            None
        };
        let next_slots = if voxel_shader && culling_layout.is_none() {
            Some(self.stable_slots.plan(&self.prepared.draws)?)
        } else {
            None
        };
        let slot_layout = culling_layout.or_else(|| {
            next_slots
                .as_ref()
                .map(|(slots, mapping)| (slots.high_water(), mapping.as_slice()))
        });
        let physical_count = slot_layout
            .map_or(self.prepared.draws.len(), |(high_water, _)| high_water)
            .max(1);
        let draw_count = physical_count;
        let light_count = self.prepared.lights.len().max(1);
        let draw_headroom = dx11_streaming_headroom(self.backend, draw_count);
        let next_draw_capacity = grow_capacity(self.draw_capacity, draw_headroom)?;
        let next_instance_capacity = grow_capacity(self.instance_capacity, draw_headroom)?;
        let light_headroom = streaming_light_headroom(
            light_count,
            device.buffer_binding_limits().max_storage_binding_bytes,
        );
        let next_light_capacity = grow_capacity(self.light_capacity, light_headroom)?;
        let next_slot_capacity = grow_capacity(
            self.draw_slot_capacity,
            dx11_streaming_headroom(self.backend, self.prepared.draws.len().max(1)),
        )?;
        let replace_draw = next_draw_capacity != self.draw_capacity;
        let replace_instances = next_instance_capacity != self.instance_capacity;
        let replace_lights = next_light_capacity != self.light_capacity;
        let replace_slot_map = next_slot_capacity != self.draw_slot_capacity;
        let [mut draw_bytes, mut instance_bytes, mut light_bytes] =
            std::mem::take(&mut self.scratch_tables);
        let mut next_draw_keys = std::mem::take(&mut self.scratch_draw_slot_keys);
        let mut next_instance_keys = std::mem::take(&mut self.scratch_instance_keys);
        let mut dirty_instance_run = std::mem::take(&mut self.dirty_instance_run);
        let encoded_instance_rows = if let Some((high_water, mapping)) = slot_layout {
            // Encode material snapshots directly into physical GPU slots. No
            // intermediate dense logical 80-byte table and no second reorder
            // copy are needed for the common streaming Section update.
            self.draw_record_cache.encode_stable(
                &self.prepared.draws,
                self.prepared.lights.len(),
                mapping,
                high_water,
                &self.draw_slot_keys,
                &self.uploaded_tables[0],
                &mut next_draw_keys,
                &mut draw_bytes,
                &mut self.scratch_packed_draws,
            )?;
            self.encode_stable_instances(
                mapping,
                high_water,
                &self.instance_slot_keys,
                &self.uploaded_tables[1],
                &mut next_instance_keys,
                &mut instance_bytes,
                &mut dirty_instance_run,
            )?
        } else {
            self.draw_record_cache.encode(
                &self.prepared.draws,
                self.prepared.lights.len(),
                &mut draw_bytes,
                &mut next_draw_keys,
            )?;
            self.encode_draw_instances_cached(
                &self.prepared.draws,
                voxel_shader,
                &self.instance_slot_keys,
                &self.uploaded_tables[1],
                &mut next_instance_keys,
                &mut instance_bytes,
                &mut dirty_instance_run,
            )?
        };
        self.dirty_instance_run = dirty_instance_run;
        let mut slot_bytes = std::mem::take(&mut self.scratch_slot_map);
        slot_bytes.clear();
        if let Some((_, mapping)) = slot_layout {
            for &slot in mapping {
                slot_bytes.extend_from_slice(&slot.to_le_bytes());
            }
        } else {
            for logical in 0..self.prepared.draws.len() {
                let index =
                    u32::try_from(logical).context("GPUI 3D logical draw index exceeds u32")?;
                slot_bytes.extend_from_slice(&index.to_le_bytes());
            }
        }
        if slot_bytes.is_empty() {
            slot_bytes.extend_from_slice(&0u32.to_le_bytes());
        }
        light_bytes.clear();
        light_bytes.reserve(
            self.prepared
                .lights
                .len()
                .max(1)
                .saturating_mul(LIGHT_STRIDE as usize),
        );
        encode_lights_into(&self.prepared.lights, &mut light_bytes);
        // An empty encoded table is a no-change sentinel only for an existing
        // GPU buffer. Reallocation must populate the new resident allocation.
        if replace_draw && draw_bytes.is_empty() {
            draw_bytes.extend_from_slice(&self.uploaded_tables[0]);
        }
        if replace_instances && instance_bytes.is_empty() {
            instance_bytes.extend_from_slice(&self.uploaded_tables[1]);
        }
        // Never allow compact-key diff ranges to address beyond actual
        // encoded rows; fail before allocating replacement GPU buffers.
        let expected_draw_bytes = next_draw_keys.len()
            .checked_mul(DRAW_PARAMS_STRIDE as usize)
            .ok_or_else(|| anyhow!("GPUI 3D DrawParams byte length overflow"))?;
        let expected_instance_bytes = next_instance_keys.len()
            .checked_mul(INSTANCE_STRIDE as usize)
            .ok_or_else(|| anyhow!("GPUI 3D Instance byte length overflow"))?;
        if next_draw_keys.len() > physical_count
            || next_instance_keys.len() > physical_count
            || (!draw_bytes.is_empty() && draw_bytes.len() < expected_draw_bytes)
            || (!instance_bytes.is_empty() && instance_bytes.len() < expected_instance_bytes)
            || (draw_bytes.is_empty() && (replace_draw || next_draw_keys != self.draw_slot_keys))
            || (instance_bytes.is_empty() && (replace_instances || next_instance_keys != self.instance_slot_keys))
        {
            bail!(
                "GPUI 3D physical table mismatch: slots={} draw_keys={} draw_bytes={} instance_keys={} instance_bytes={}",
                physical_count, next_draw_keys.len(), draw_bytes.len(),
                next_instance_keys.len(), instance_bytes.len()
            );
        }
        let encode_ms = trace_started.map(|started| started.elapsed().as_secs_f64() * 1000.0);
        let new_draw = if replace_draw {
            Some(create_storage_buffer(
                device,
                "gpui-3d draw data",
                next_draw_capacity * DRAW_PARAMS_STRIDE as usize,
                buffer_memory_location(self.backend),
            )?)
        } else {
            None
        };
        let new_lights = if replace_lights {
            match create_storage_buffer(
                device,
                "gpui-3d lights",
                next_light_capacity * LIGHT_STRIDE as usize,
                buffer_memory_location(self.backend),
            ) {
                Ok(buffer) => Some(buffer),
                Err(error) => {
                    if let Some(buffer) = new_draw {
                        device.destroy_buffer(buffer)?;
                    }
                    return Err(error);
                }
            }
        } else {
            None
        };
        let new_instances = if replace_instances {
            match create_storage_buffer(
                device,
                "gpui-3d instance data",
                next_instance_capacity * INSTANCE_STRIDE as usize,
                buffer_memory_location(self.backend),
            ) {
                Ok(buffer) => Some(buffer),
                Err(error) => {
                    if let Some(buffer) = new_lights {
                        device.destroy_buffer(buffer)?;
                    }
                    if let Some(buffer) = new_draw {
                        device.destroy_buffer(buffer)?;
                    }
                    return Err(error);
                }
            }
        } else {
            None
        };
        let new_slot_map = if replace_slot_map {
            match create_storage_buffer(
                device,
                "gpui-3d logical draw slot mapping",
                next_slot_capacity * 4,
                buffer_memory_location(self.backend),
            ) {
                Ok(buffer) => Some(buffer),
                Err(error) => {
                    if let Some(buffer) = new_instances {
                        device.destroy_buffer(buffer)?;
                    }
                    if let Some(buffer) = new_lights {
                        device.destroy_buffer(buffer)?;
                    }
                    if let Some(buffer) = new_draw {
                        device.destroy_buffer(buffer)?;
                    }
                    return Err(error);
                }
            }
        } else {
            None
        };
        let draw_buffer = new_draw.unwrap_or(self.draw_buffer);
        let instance_buffer = new_instances.unwrap_or(self.instance_buffer);
        let light_buffer = new_lights.unwrap_or(self.light_buffer);
        let slot_map_buffer = new_slot_map.unwrap_or(self.draw_slot_buffer);
        let write_started = trace_started.map(|_| Instant::now());
        let upload_range_budget =
            sparse_upload_range_budget(self.backend, self.prepared.draws.len());
        let writes = [
            (
                draw_buffer,
                draw_bytes.as_slice(),
                replace_draw,
                DRAW_PARAMS_STRIDE as usize,
            ),
            (
                instance_buffer,
                instance_bytes.as_slice(),
                replace_instances,
                INSTANCE_STRIDE as usize,
            ),
            (
                light_buffer,
                light_bytes.as_slice(),
                replace_lights,
                LIGHT_STRIDE as usize,
            ),
        ]
        .into_iter()
        .enumerate()
        .flat_map(|(i, (buffer, data, replace, stride))| {
            // The full draw/instance layout is unchanged; only CPU->GPU transfer
            // becomes sparse. Batch modern GPU-copy backends can carry more
            // patches in one native submission; keep a tighter limit for
            // legacy drivers to bound their per-write overhead.
            (if i == 0 {
                // DrawParams have immutable material-snapshot identities.
                // Scan compact keys instead of comparing 80 bytes per draw.
                uploads::changed_ranges_for_keys(
                    if replace { &[] } else { &self.draw_slot_keys },
                    &next_draw_keys,
                    if replace {
                        0
                    } else {
                        self.uploaded_tables[i].len()
                    },
                    stride,
                    upload_range_budget,
                    4,
                )
            } else if i == 1 {
                // Instance rows already carry bit-exact GPU-visible keys.
                uploads::changed_ranges_for_keys(
                    if replace {
                        &[]
                    } else {
                        &self.instance_slot_keys
                    },
                    &next_instance_keys,
                    if replace {
                        0
                    } else {
                        self.uploaded_tables[i].len()
                    },
                    stride,
                    upload_range_budget,
                    4,
                )
            } else {
                uploads::changed_ranges(
                    if replace {
                        &[]
                    } else {
                        &self.uploaded_tables[i]
                    },
                    data,
                    stride,
                    upload_range_budget,
                    4,
                )
            })
            .into_iter()
            .map(move |range| BufferWrite {
                descriptor: BufferWriteDescriptor {
                    buffer,
                    offset: range.start as u64,
                },
                data: &data[range],
            })
        })
        .collect::<Vec<_>>();
        let mut writes = writes;
        // Only four bytes per logical draw move when a Section is inserted
        // before another. The 80+144-byte physical records remain resident.
        for range in uploads::changed_ranges(
            if replace_slot_map {
                &[]
            } else {
                &self.uploaded_slot_map
            },
            &slot_bytes,
            4,
            upload_range_budget,
            4,
        ) {
            writes.push(BufferWrite {
                descriptor: BufferWriteDescriptor {
                    buffer: slot_map_buffer,
                    offset: range.start as u64,
                },
                data: &slot_bytes[range],
            });
        }
        let uploaded_bytes = writes.iter().map(|write| write.data.len()).sum::<usize>();
        let write_count = writes.len();
        let encoded_bytes =
            draw_bytes.len() + instance_bytes.len() + light_bytes.len() + slot_bytes.len();
        // A failed write or later binding update may leave existing buffers partially changed.
        let mut previous_tables = std::mem::take(&mut self.uploaded_tables);
        let write_result = if writes.is_empty() {
            Ok(())
        } else {
            let upload_started = Instant::now();
            let result = device.write_buffer_batch(&writes).map(|_| ());
            if result.is_ok() {
                crate::upload_feedback::observe_buffer_upload(
                    self.backend, uploaded_bytes, write_count, upload_started.elapsed(),
                );
            }
            result
        };
        if let Err(error) = write_result {
            if let Some(buffer) = new_slot_map {
                device.destroy_buffer(buffer)?;
            }
            if let Some(buffer) = new_draw {
                device.destroy_buffer(buffer)?;
            }
            if let Some(buffer) = new_instances {
                device.destroy_buffer(buffer)?;
            }
            if let Some(buffer) = new_lights {
                device.destroy_buffer(buffer)?;
            }
            return Err(error.into());
        }
        let write_ms = write_started.map(|started| started.elapsed().as_secs_f64() * 1000.0);
        let old_draw = self.draw_buffer;
        let old_draw_capacity = self.draw_capacity;
        let old_slot_map = self.draw_slot_buffer;
        let old_slot_capacity = self.draw_slot_capacity;
        let old_instances = self.instance_buffer;
        let old_instance_capacity = self.instance_capacity;
        let old_lights = self.light_buffer;
        let old_light_capacity = self.light_capacity;
        self.draw_buffer = draw_buffer;
        self.draw_capacity = next_draw_capacity;
        self.draw_slot_buffer = slot_map_buffer;
        self.draw_slot_capacity = next_slot_capacity;
        self.instance_buffer = instance_buffer;
        self.instance_capacity = next_instance_capacity;
        self.light_buffer = light_buffer;
        self.light_capacity = next_light_capacity;
        let bind_started = trace_started.map(|_| Instant::now());
        if let Err(error) = self.sync_draw_resources(
            device,
            renderer,
            bind_group_layout,
            sampler,
            sampler_changed
                || replace_draw
                || replace_instances
                || replace_lights
                || replace_slot_map,
            voxel_shader,
            defer_binding_cleanup,
            true,
        ) {
            self.draw_buffer = old_draw;
            self.draw_capacity = old_draw_capacity;
            self.draw_slot_buffer = old_slot_map;
            self.draw_slot_capacity = old_slot_capacity;
            self.instance_buffer = old_instances;
            self.instance_capacity = old_instance_capacity;
            self.light_buffer = old_lights;
            self.light_capacity = old_light_capacity;
            if replace_slot_map {
                device.destroy_buffer(slot_map_buffer)?;
            }
            if replace_draw {
                device.destroy_buffer(draw_buffer)?;
            }
            if replace_lights {
                device.destroy_buffer(light_buffer)?;
            }
            if replace_instances {
                device.destroy_buffer(instance_buffer)?;
            }
            return Err(error);
        }
        self.draw_addresses.clear();
        self.draw_addresses
            .extend(
                self.batches
                    .iter()
                    .enumerate()
                    .map(|(index, batch)| DrawAddress {
                        arguments_offset: index as u64 * ARGUMENT_STRIDE,
                        first_instance: batch.first_instance,
                    }),
            );
        self.sync_generated_draws(device, voxel_shader)?;
        if let Some((slots, _)) = next_slots {
            self.stable_slots = slots;
        } else if !voxel_shader {
            // The standard scene shader uses dense indexing directly. The
            // culler owns voxel layouts when GPU culling is enabled.
            self.stable_slots = stable_slots::StableDrawSlots::default();
        }
        let mut previous_slot_bytes = std::mem::replace(&mut self.uploaded_slot_map, slot_bytes);
        previous_slot_bytes.clear();
        self.scratch_slot_map = previous_slot_bytes;
        let mut old_draw_keys = std::mem::replace(&mut self.draw_slot_keys, next_draw_keys);
        old_draw_keys.clear();
        self.scratch_draw_slot_keys = old_draw_keys;
        // Commit slot identities only after the corresponding GPU upload and
        // binding preparation have both completed. A failed update must not
        // make future frames reuse records that were never committed.
        let mut prior_keys = std::mem::replace(&mut self.instance_slot_keys, next_instance_keys);
        prior_keys.clear();
        self.scratch_instance_keys = prior_keys;
        let bind_ms = bind_started.map(|started| started.elapsed().as_secs_f64() * 1000.0);
        if replace_slot_map {
            device.destroy_buffer(old_slot_map)?;
        }
        if replace_draw {
            device.destroy_buffer(old_draw)?;
        }
        if replace_lights {
            device.destroy_buffer(old_lights)?;
        }
        if replace_instances {
            device.destroy_buffer(old_instances)?;
        }
        if let Some(started) = trace_started
            && started.elapsed() >= std::time::Duration::from_millis(16)
        {
            eprintln!(
                "gpui3d buffer_detail draws={} instance_encoded={} bytes={} prepared_bytes={} writes={} encode_ms={:.2} write_ms={:.2} bind_ms={:.2} resized={} resize_draw={} resize_instances={} resize_lights={} resize_slots={}",
                draw_count,
                encoded_instance_rows,
                uploaded_bytes,
                encoded_bytes,
                write_count,
                encode_ms.unwrap_or_default(),
                write_ms.unwrap_or_default(),
                bind_ms.unwrap_or_default(),
                replace_draw || replace_instances || replace_lights || replace_slot_map,
                replace_draw,
                replace_instances,
                replace_lights,
                replace_slot_map,
            );
        }
        // Empty new tables are intentional sentinels used only when the
        // physical keys and table extents matched the committed snapshot.
        // Move the proven-good CPU storage after successful GPU updates rather
        // than memcpy'ing every resident row on a logical-only Section reorder.
        if voxel_shader
            && !replace_draw
            && draw_bytes.is_empty()
            && previous_tables[0].len() == physical_count * DRAW_PARAMS_STRIDE as usize
        {
            draw_bytes = std::mem::take(&mut previous_tables[0]);
        }
        if voxel_shader
            && !replace_instances
            && instance_bytes.is_empty()
            && previous_tables[1].len() == physical_count * INSTANCE_STRIDE as usize
        {
            instance_bytes = std::mem::take(&mut previous_tables[1]);
        }
        self.uploaded_tables = [draw_bytes, instance_bytes, light_bytes];
        for table in &mut previous_tables {
            table.clear();
        }
        self.scratch_tables = previous_tables;
        Ok(())
    }

    fn ensure_textures(
        &mut self,
        device: &mut dyn ExtensionDevice,
        scene_view: &SceneView,
        incremental: bool,
    ) -> gpui::Result<()> {
        self.seen_materials.clear();
        let draws: &[PreparedDraw] = if incremental {
            self.prepared.incremental_asset_draws()
        } else {
            &self.prepared.draws
        };
        for draw in draws {
            if !self
                .seen_materials
                .insert(Arc::as_ptr(&draw.material) as usize)
            {
                continue;
            }
            for id in [
                draw.material.albedo_texture,
                draw.material.normal_texture,
                draw.material.occlusion_texture,
                draw.material.metallic_roughness_texture,
            ]
            .into_iter()
            .flatten()
            {
                if self.textures.contains_key(&id) {
                    continue;
                }
                let asset = scene_view
                    .texture(id)
                    .ok_or_else(|| anyhow!("GPUI 3D scene view has no texture asset {}", id.0))?;
                let resources = create_texture(device, asset)?;
                self.textures.insert(id, resources);
            }
        }
        Ok(())
    }

    fn retire_textures(
        &mut self,
        device: &mut dyn ExtensionDevice,
        active: &HashSet<TextureAssetId>,
    ) -> gpui::Result<()> {
        let stale = self
            .textures
            .keys()
            .filter(|id| !active.contains(id))
            .copied()
            .collect::<Vec<_>>();
        for id in stale {
            if let Some(texture) = self.textures.remove(&id) {
                destroy_texture(device, texture)?;
            }
        }
        Ok(())
    }

    fn retire_texture_delta(
        &mut self,
        device: &mut dyn ExtensionDevice,
        removed: &[TextureAssetId],
    ) -> gpui::Result<()> {
        for id in removed {
            if let Some(texture) = self.textures.remove(id) {
                destroy_texture(device, texture)?;
            }
        }
        Ok(())
    }

    fn ensure_meshes(
        &mut self,
        device: &mut dyn ExtensionDevice,
        backend: BackendKind,
        voxel_shader: bool,
    ) -> gpui::Result<()> {
        self.ensure_meshes_for_draws(device, backend, voxel_shader, false)
    }

    fn ensure_meshes_for_draws(
        &mut self,
        device: &mut dyn ExtensionDevice,
        backend: BackendKind,
        voxel_shader: bool,
        incremental: bool,
    ) -> gpui::Result<()> {
        // The face/template/argument pools are shared with the renderer's
        // completion polling on this thread. A polling borrow alive during
        // this call used to panic the render-owner thread and tear every
        // window's renderer down; skip this frame's uploads instead and let
        // the next prepare retry them.
        if self.generated_arena.try_borrow().is_err()
            || self.template_arena.try_borrow().is_err()
            || self.argument_arena.try_borrow().is_err()
        {
            static SKIPPED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let skipped = SKIPPED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if skipped % 32 == 0 {
                eprintln!("gpui-3d mesh upload deferred: a shared mesh arena is still borrowed");
            }
            return Ok(());
        }
        let mut pending = Vec::<StagedMesh>::new();
        let mut pending_gpu = Vec::<((MeshId, bool), Arc<Mesh>)>::new();
        let mut pending_ids = HashSet::new();
        let mut pending_bytes = 0usize;
        let gpu_meshing = device.compute_capabilities().voxel_execution_path()
            == gfx_core::VoxelExecutionPath::GpuDriven;
        let draws: &[PreparedDraw] = if incremental {
            self.prepared.incremental_asset_draws()
        } else {
            &self.prepared.draws
        };
        for draw in draws {
            let id = mesh_key(draw, voxel_shader);
            let generation = draw.mesh.generation();
            if self
                .meshes
                .get(&id)
                .is_some_and(|mesh| mesh.generation == generation)
            {
                continue;
            }
            if !pending_ids.insert((id, generation)) {
                continue;
            }
            if id.1
                && gpu_meshing
                && let Some(source) = draw.mesh.gpu_voxels()
            {
                if self.mesh_shader.is_none() {
                    self.mesh_shader = Some(super::generated::create_shaders(device, backend)?);
                }
                // Budget temporary input and worst-case resident output, not
                // just already-packed CPU geometry upload bytes.
                pending_bytes = pending_bytes.saturating_add(
                    source
                        .cells
                        .len()
                        .saturating_mul(4)
                        .saturating_add(source.templates.len().saturating_mul(112))
                        .saturating_add(source.capacity().saturating_mul(16)),
                );
                pending_gpu.push((id, Arc::clone(&draw.mesh)));
            } else {
                let (mesh, upload) =
                    match create_mesh(device, &draw.mesh, backend, id.1, &mut self.arena) {
                        Ok(created) => created,
                        Err(error) => {
                            discard_pending_meshes(
                                device,
                                &mut pending,
                                &mut self.arena,
                                &mut *self.generated_arena.borrow_mut(),
                                &mut *self.template_arena.borrow_mut(),
                                &mut *self.argument_arena.borrow_mut(),
                            )?;
                            return Err(error);
                        }
                    };
                pending_bytes = pending_bytes
                    .saturating_add(upload.as_ref().map_or(0, |upload| upload.bytes.len()));
                pending.push((id, mesh, upload));
            }
            if pending_bytes >= MESH_UPLOAD_BATCH_BYTES
                || pending_gpu.len() >= 16
                || pending.len() + pending_gpu.len() >= 256
            {
                // The flush call's arena borrow_mut temporaries live until the
                // end of this if-let statement; bind the result first so the
                // error path can re-borrow the same arenas to discard.
                let flushed = flush_pending_generated_meshes(
                    device,
                    &mut pending_gpu,
                    self.mesh_shader.as_ref(),
                    &mut pending,
                    &mut self.meshes,
                    &mut self.generated_mappings,
                    &mut self.page_refs,
                    &mut self.retired_meshes,
                    &mut self.arena,
                    &mut *self.generated_arena.borrow_mut(),
                    &mut *self.template_arena.borrow_mut(),
                    &mut *self.argument_arena.borrow_mut(),
                    backend,
                );
                if let Err(error) = flushed {
                    discard_pending_meshes(
                        device,
                        &mut pending,
                        &mut self.arena,
                        &mut *self.generated_arena.borrow_mut(),
                        &mut *self.template_arena.borrow_mut(),
                        &mut *self.argument_arena.borrow_mut(),
                    )?;
                    return Err(error);
                }
                pending_ids.clear();
                pending_bytes = 0;
            }
        }
        let flushed = flush_pending_generated_meshes(
            device,
            &mut pending_gpu,
            self.mesh_shader.as_ref(),
            &mut pending,
            &mut self.meshes,
            &mut self.generated_mappings,
            &mut self.page_refs,
            &mut self.retired_meshes,
            &mut self.arena,
            &mut *self.generated_arena.borrow_mut(),
            &mut *self.template_arena.borrow_mut(),
            &mut *self.argument_arena.borrow_mut(),
            backend,
        );
        if let Err(error) = flushed {
            discard_pending_meshes(
                device,
                &mut pending,
                &mut self.arena,
                &mut *self.generated_arena.borrow_mut(),
                &mut *self.template_arena.borrow_mut(),
                &mut *self.argument_arena.borrow_mut(),
            )?;
            return Err(error);
        }
        Ok(())
    }

    fn retire_meshes(&mut self, active: &HashSet<MeshId>) {
        let stale = self
            .meshes
            .keys()
            .filter(|(id, _)| !active.contains(id))
            .copied()
            .collect::<Vec<_>>();
        for id in stale {
            if let Some(mesh) = self.meshes.remove(&id) {
                self.page_refs.remove(&mesh, id.1);
                self.retired_meshes.push(mesh);
            }
        }
    }

    fn retire_mesh_delta(&mut self, removed: &[MeshId]) {
        for &id in removed {
            for compact in [false, true] {
                if let Some(mesh) = self.meshes.remove(&(id, compact)) {
                    self.page_refs.remove(&mesh, compact);
                    self.retired_meshes.push(mesh);
                }
            }
        }
    }

    fn destroy_cached_binding(
        &mut self,
        device: &mut dyn ExtensionDevice,
        key: DrawResourceKey,
    ) -> gpui::Result<()> {
        if let Some(&binding) = self.binding_cache.get(&key) {
            device.destroy_bind_group(binding)?;
            self.binding_cache.remove(&key);
            self.binding_index.remove(key);
        }
        Ok(())
    }

    fn retire_binding_delta(
        &mut self,
        device: &mut dyn ExtensionDevice,
        removed_meshes: &[MeshId],
        removed_textures: &[TextureAssetId],
    ) -> gpui::Result<()> {
        // Only the changed Section IDs and their physical GPU pages need
        // inspection. No full resident MeshResources walk on ordinary unloads.
        let pages = self.page_refs.orphaned(removed_meshes, &self.meshes);
        let stale = self
            .binding_index
            .affected(removed_meshes, &pages, removed_textures);
        for key in stale {
            self.destroy_cached_binding(device, key)?;
        }
        Ok(())
    }

    fn retire_bindings(
        &mut self,
        device: &mut dyn ExtensionDevice,
        active_meshes: &HashSet<MeshId>,
        active_textures: &HashSet<TextureAssetId>,
    ) -> gpui::Result<()> {
        let active_voxel_pages = self
            .meshes
            .iter()
            .filter(|((id, compact), _)| *compact && active_meshes.contains(id))
            .flat_map(|(_, mesh)| {
                std::iter::once(mesh.vertices)
                    .chain(mesh.generated.iter().map(|generated| generated.templates))
            })
            .collect::<HashSet<_>>();
        let stale = self
            .binding_cache
            .iter()
            .filter_map(|(key, &binding)| {
                (!key.uses_active_assets(active_meshes, &active_voxel_pages, active_textures)
                    || !key.is_resident(
                        &self.meshes,
                        &active_voxel_pages,
                        self.culling
                            .as_ref()
                            .map(|culling| culling.visible_instances),
                    ))
                .then_some((*key, binding))
            })
            .collect::<Vec<_>>();
        for (key, _) in stale {
            self.destroy_cached_binding(device, key)?;
        }
        Ok(())
    }

    /// Stage a single newly inserted Section's native binding using the
    /// existing device-owned cache. This deliberately does not mutate the
    /// committed cache: any failure rolls back all newly created BindGroups.
    fn resolve_structural_binding(
        &self,
        device: &mut dyn ExtensionDevice,
        renderer: &RendererResources,
        bind_group_layout: BindGroupLayoutId,
        sampler: SamplerId,
        logical: usize,
        created: &mut HashMap<DrawResourceKey, BindGroupId>,
    ) -> gpui::Result<(
        BindGroupId,
        DrawResourceKey,
        ((MeshId, bool), u64),
        NativeCommandSource,
        Option<generated::BatchMappingSource>,
    )> {
        let draw = self
            .prepared
            .draws
            .get(logical)
            .ok_or_else(|| anyhow!("new Section binding has invalid logical Draw index"))?;
        let mesh_key = mesh_key(draw, true);
        let mesh = self
            .meshes
            .get(&mesh_key)
            .ok_or_else(|| anyhow!("new Section binding references an unuploaded Mesh"))?;
        let source = (mesh_key, draw.mesh.generation());
        let generated_source = generated::BatchMappingSource::for_draw(draw, mesh)?;
        let key = DrawResourceKey {
            shadow: self.shadow.as_ref().map(|shadow| shadow.raster.depths),
            visible_instances: self
                .culling
                .as_ref()
                .map(|culling| culling.visible_instances),
            mesh: if let Some(generated) = &mesh.generated {
                DrawMeshKey::GeneratedPage {
                    faces: mesh.vertices,
                    templates: generated.templates,
                }
            } else if mesh_key.1 {
                DrawMeshKey::VoxelPage(mesh.vertices)
            } else {
                DrawMeshKey::Mesh {
                    id: mesh_key.0,
                    generation: draw.mesh.generation(),
                }
            },
            albedo_id: draw.material.albedo_texture,
            normal_id: draw.material.normal_texture,
            occlusion_id: draw.material.occlusion_texture,
            metallic_roughness_id: draw.material.metallic_roughness_texture,
        };
        let native = NativeCommandSource::for_draw(draw);
        if let Some(&binding) = created.get(&key).or_else(|| self.binding_cache.get(&key)) {
            return Ok((binding, key, source, native, generated_source));
        }
        let texture_view = |id: Option<TextureAssetId>| -> gpui::Result<_> {
            if let Some(id) = id {
                Ok(self
                    .textures
                    .get(&id)
                    .ok_or_else(|| {
                        anyhow!(
                            "new Section binding references an unuploaded texture {}",
                            id.0
                        )
                    })?
                    .view)
            } else {
                renderer.fallback_texture_view()
            }
        };
        let binding = create_draw_resource_set(
            device,
            DrawBindingResources {
                shadow: self.shadow.as_ref().map(|shadow| shadow.raster.depths),
                bind_group_layout,
                mesh,
                draw_buffer: self.draw_buffer,
                draw_capacity: self.draw_capacity,
                draw_slots: self.draw_slot_buffer,
                draw_slot_capacity: self.draw_slot_capacity,
                instance_buffer: self.instance_buffer,
                instance_count: self.instance_capacity,
                light_buffer: self.light_buffer,
                light_capacity: self.light_capacity,
                albedo_view: texture_view(draw.material.albedo_texture)?,
                normal_texture_view: texture_view(draw.material.normal_texture)?,
                occlusion_view: texture_view(draw.material.occlusion_texture)?,
                metallic_roughness_view: texture_view(draw.material.metallic_roughness_texture)?,
                material_sampler: sampler,
                frame_buffer: self.frame_buffer,
                visible_instances: self.culling.as_ref().map(|culling| {
                    (
                        culling.visible_instances,
                        culling.instance_capacity as u64 * 4,
                    )
                }),
            },
        )?;
        created.insert(key, binding);
        Ok((binding, key, source, native, generated_source))
    }

    /// Structural Section Delta: copy unchanged parallel native BindGroup
    /// snapshots in contiguous slices and resolve ONLY inserted/replaced
    /// draws. This avoids hashing 100K resident Mesh/Material references
    /// after a middle insertion. The generated count mapping remains a
    /// separate GPU publication from already-resolved per-slot sources.
    fn sync_structural_draw_resources(
        &mut self,
        device: &mut dyn ExtensionDevice,
        renderer: &RendererResources,
        bind_group_layout: BindGroupLayoutId,
        sampler: SamplerId,
        sampler_changed: bool,
    ) -> gpui::Result<bool> {
        let Some(delta) = self.prepared.structural_draw_delta().cloned() else {
            return Ok(false);
        };
        let count = self.prepared.draws.len();
        let old_count = delta.previous_count;
        let shadow = self.shadow.as_ref().map(|shadow| shadow.raster.depths);
        let visible = self.culling.as_ref().map(|culler| culler.visible_instances);
        // Sections own self-contained bind groups, so a rebuilt texture
        // table Arc cannot stale the surviving intervals; only a sampler
        // swap invalidates every group at once.
        if sampler_changed
            || !self.page_refs.only_generated_meshes()
            || !self.batch_workspace.singleton_layout()
            || !self
                .culling
                .as_ref()
                .is_some_and(CullResources::structural_slots_committed)
            // An unpublished Page mapping from an earlier failed update
            // cannot be assumed to describe these structural slots. Keep the
            // full binding revalidation in that exceptional retry case.
            || self.generated_pending_groups.is_some()
            || !self.retired_meshes.is_empty()
            || self.batches.len() != count
            || self.draw_resources.len() != old_count
            || self.draw_resource_keys.len() != old_count
            || self.draw_resource_sources.len() != old_count
            || self.native_command_sources.len() != old_count
            || self.generated_batch_sources.len() != old_count
            || self
                .draw_resource_keys
                .first()
                .is_none_or(|key| key.shadow != shadow || key.visible_instances != visible)
        {
            return Ok(false);
        }
        // Validate the complete splice shape before any device allocations
        // or snapshots are touched; malformed overlaps use the full path.
        let mut old_cursor = 0usize;
        let mut new_cursor = 0usize;
        for edit in &delta.edits {
            if edit.old.start < old_cursor
                || edit.new.start < new_cursor
                || edit.old.end > old_count
                || edit.new.end > count
                || edit.old.start > edit.old.end
                || edit.new.start > edit.new.end
                || edit.old.start - old_cursor != edit.new.start - new_cursor
            {
                return Ok(false);
            }
            old_cursor = edit.old.end;
            new_cursor = edit.new.end;
        }
        if old_count - old_cursor != count - new_cursor {
            return Ok(false);
        }

        let trace_started = trace_timings().then(Instant::now);
        let mut bindings = std::mem::take(&mut self.spare_draw_resources);
        let mut keys = std::mem::take(&mut self.spare_draw_resource_keys);
        let mut sources = std::mem::take(&mut self.spare_draw_resource_sources);
        let mut natives = std::mem::take(&mut self.spare_native_command_sources);
        let mut generated = std::mem::take(&mut self.spare_generated_batch_sources);
        bindings.clear();
        keys.clear();
        sources.clear();
        natives.clear();
        generated.clear();
        bindings.reserve(count);
        keys.reserve(count);
        sources.reserve(count);
        natives.reserve(count);
        generated.reserve(count);
        let mut created = HashMap::<DrawResourceKey, BindGroupId>::new();
        let mut old_cursor = 0usize;
        let mut inserted = 0usize;
        let append_survivors =
            |range: std::ops::Range<usize>,
             bindings: &mut Vec<BindGroupId>,
             keys: &mut Vec<DrawResourceKey>,
             sources: &mut Vec<((MeshId, bool), u64)>,
             natives: &mut Vec<NativeCommandSource>,
             generated: &mut Vec<Option<generated::BatchMappingSource>>| {
                bindings.extend_from_slice(&self.draw_resources[range.clone()]);
                keys.extend_from_slice(&self.draw_resource_keys[range.clone()]);
                sources.extend_from_slice(&self.draw_resource_sources[range.clone()]);
                natives.extend_from_slice(&self.native_command_sources[range.clone()]);
                generated.extend_from_slice(&self.generated_batch_sources[range]);
            };
        for edit in &delta.edits {
            append_survivors(
                old_cursor..edit.old.start,
                &mut bindings,
                &mut keys,
                &mut sources,
                &mut natives,
                &mut generated,
            );
            for index in edit.new.clone() {
                let result = self.resolve_structural_binding(
                    device,
                    renderer,
                    bind_group_layout,
                    sampler,
                    index,
                    &mut created,
                );
                let (binding, key, source, native, mapping) = match result {
                    Ok(entry) => entry,
                    Err(error) => {
                        for binding in created.into_values() {
                            device.destroy_bind_group(binding)?;
                        }
                        return Err(error);
                    }
                };
                bindings.push(binding);
                keys.push(key);
                sources.push(source);
                natives.push(native);
                generated.push(mapping);
                inserted += 1;
            }
            old_cursor = edit.old.end;
        }
        append_survivors(
            old_cursor..old_count,
            &mut bindings,
            &mut keys,
            &mut sources,
            &mut natives,
            &mut generated,
        );
        debug_assert_eq!(bindings.len(), count);
        std::mem::swap(&mut self.draw_resources, &mut bindings);
        std::mem::swap(&mut self.draw_resource_keys, &mut keys);
        std::mem::swap(&mut self.draw_resource_sources, &mut sources);
        std::mem::swap(&mut self.native_command_sources, &mut natives);
        std::mem::swap(&mut self.generated_batch_sources, &mut generated);
        self.spare_draw_resources = bindings;
        self.spare_draw_resource_keys = keys;
        self.spare_draw_resource_sources = sources;
        self.spare_native_command_sources = natives;
        self.spare_generated_batch_sources = generated;
        for &key in created.keys() {
            self.binding_index.insert(key);
        }
        self.binding_cache.extend(created);
        if let Some(started) = trace_started {
            eprintln!(
                "gpui3d structural_bindings batches={} resolved={} reused={} elapsed_ms={:.2}",
                count,
                inserted,
                count.saturating_sub(inserted),
                started.elapsed().as_secs_f64() * 1000.0,
            );
        }
        Ok(true)
    }

    fn sync_draw_resources(
        &mut self,
        device: &mut dyn ExtensionDevice,
        renderer: &RendererResources,
        bind_group_layout: BindGroupLayoutId,
        sampler: SamplerId,
        force: bool,
        voxel_shader: bool,
        defer_binding_cleanup: bool,
        textures_unchanged: bool,
    ) -> gpui::Result<()> {
        // Texture-table-only or light-only frames rebuild nothing: the batch
        // stream, its per-batch sources and the frame resource keys are
        // unchanged, so the rebuilt tables would equal the resident ones.
        // This must run BEFORE the scratch buffers are taken below — returning
        // between the take and the commit leaves the spares and the committed
        // GPU mapping snapshot inconsistent.
        let shadow_buffer = self.shadow.as_ref().map(|shadow| shadow.raster.depths);
        let visible_instances = self
            .culling
            .as_ref()
            .map(|culling| culling.visible_instances);
        if !force
            && self.retired_meshes.is_empty()
            && self.batches.len() == self.draw_resources.len()
            && self.prepared.draws.len() == self.draw_resource_layout
            && self.draw_resource_shadow == shadow_buffer
            && self.draw_resource_visible_instances == visible_instances
            && self.draw_resource_voxel == voxel_shader
            && self.draw_resource_sampler == Some(sampler)
            && textures_unchanged
        {
            return Ok(());
        }
        let mut next = std::mem::take(&mut self.spare_draw_resources);
        let mut next_keys = std::mem::take(&mut self.spare_draw_resource_keys);
        let mut next_sources = std::mem::take(&mut self.spare_draw_resource_sources);
        let mut next_native = std::mem::take(&mut self.spare_native_command_sources);
        next_native.clear();
        let mut next_generated = std::mem::take(&mut self.spare_generated_batch_sources);
        // This pass already touches every native Batch. Assemble its GPU
        // mapping records now, instead of walking all batches a second time.
        let mut next_page_groups = std::mem::take(&mut self.generated_group_scratch);
        next_page_groups.clear();
        next_generated.clear();
        next.clear();
        next_keys.clear();
        next_sources.clear();
        next.reserve(self.batches.len());
        next_keys.reserve(self.batches.len());
        next_sources.reserve(self.batches.len());
        next_native.reserve(self.batches.len());
        next_generated.reserve(self.batches.len());
        let mut created = HashMap::new();
        // Bind the allocated range so culling alone does not invalidate every bind group.
        let instance_count = self.instance_capacity;
        let shadow_buffer = self.shadow.as_ref().map(|shadow| shadow.raster.depths);
        let visible_instances = self
            .culling
            .as_ref()
            .map(|culling| culling.visible_instances);
        // A replacement only invalidates the BindGroups referencing its own
        // mesh generation. Unrelated retired meshes must not disable fast
        // positional reuse for the whole 100K+ batch stream.
        let resident_meshes_unchanged = self.retired_meshes.is_empty();
        let trace_started = trace_timings().then(Instant::now);
        let mut reused = 0usize;
        let mut positional_reused = 0usize;
        let mut created_count = 0usize;
        for (batch_index, batch) in self.batches.iter().enumerate() {
            let index = batch.first_instance;
            let draw = self
                .prepared
                .draws
                .get(index)
                .ok_or_else(|| anyhow!("GPUI 3D draw batch has an invalid first draw"))?;
            next_native.push(NativeCommandSource::for_draw(draw));
            let mesh_id = draw.mesh.id();
            let compact_voxel = mesh_key(draw, voxel_shader).1;
            let generation = draw.mesh.generation();
            let albedo_id = draw.material.albedo_texture;
            let normal_id = draw.material.normal_texture;
            let occlusion_id = draw.material.occlusion_texture;
            let metallic_roughness_id = draw.material.metallic_roughness_texture;
            let source = ((mesh_id, compact_voxel), generation);
            // Most Section deltas retain the same binding at a given batch
            // position. Match immutable source IDs and the resource-bearing
            // texture/frame keys before touching the large GPU mesh map.
            if !force
                && self.draw_resource_sources.get(batch_index) == Some(&source)
                && let Some(&previous_key) = self.draw_resource_keys.get(batch_index)
                && previous_key.shadow == shadow_buffer
                && previous_key.visible_instances == visible_instances
                && previous_key.albedo_id == albedo_id
                && previous_key.normal_id == normal_id
                && previous_key.occlusion_id == occlusion_id
                && previous_key.metallic_roughness_id == metallic_roughness_id
                && let Some(&bind_group) = self.draw_resources.get(batch_index)
                // A batch may switch material-part while retaining the same
                // Mesh and BindGroup. Do not reuse the wrong count source.
                && self.generated_batch_sources.get(batch_index).is_some_and(|source| {
                    source.is_none_or(|metadata| metadata.part == draw.part)
                })
                // A removed/replaced mesh may have retired an old binding.
                // Verify residency only on that uncommon retirement path.
                && (resident_meshes_unchanged
                    || self.binding_cache.get(&previous_key) == Some(&bind_group))
            {
                reused += 1;
                positional_reused += 1;
                next.push(bind_group);
                next_keys.push(previous_key);
                next_sources.push(source);
                let metadata = self.generated_batch_sources[batch_index];
                let (slot, first_instance) = generated::generated_mapping_target(
                    self.culling
                        .as_ref()
                        .map(|culling| culling.committed_slot_layout().1),
                    batch_index,
                    index,
                )?;
                generated::push_page_record(
                    &mut next_page_groups,
                    &mut self.generated_mappings,
                    metadata,
                    slot,
                    first_instance,
                )?;
                next_generated.push(metadata);
                continue;
            }
            let mesh = self
                .meshes
                .get(&(mesh_id, compact_voxel))
                .ok_or_else(|| anyhow!("GPUI 3D mesh was not uploaded"))?;
            // This lookup and binary material-part resolution now happen in
            // the same pass that already resolves the native BindGroup.
            let generated_source = generated::BatchMappingSource::for_draw(draw, mesh)?;
            let (slot, first_instance) = generated::generated_mapping_target(
                self.culling
                    .as_ref()
                    .map(|culling| culling.committed_slot_layout().1),
                batch_index,
                index,
            )?;
            generated::push_page_record(
                &mut next_page_groups,
                &mut self.generated_mappings,
                generated_source,
                slot,
                first_instance,
            )?;
            next_generated.push(generated_source);
            let key = DrawResourceKey {
                shadow: shadow_buffer,
                visible_instances,
                mesh: if let Some(generated) = &mesh.generated {
                    DrawMeshKey::GeneratedPage {
                        faces: mesh.vertices,
                        templates: generated.templates,
                    }
                } else if compact_voxel {
                    DrawMeshKey::VoxelPage(mesh.vertices)
                } else {
                    DrawMeshKey::Mesh {
                        id: mesh_id,
                        generation,
                    }
                },
                albedo_id,
                normal_id,
                occlusion_id,
                metallic_roughness_id,
            };
            if !force
                && self.draw_resource_keys.get(batch_index) == Some(&key)
                && let Some(&bind_group) = self.draw_resources.get(batch_index)
            {
                reused += 1;
                positional_reused += 1;
                next.push(bind_group);
                next_keys.push(key);
                next_sources.push(source);
                continue;
            }
            if let Some(&bind_group) = created.get(&key).or_else(|| {
                if force {
                    None
                } else {
                    self.binding_cache.get(&key)
                }
            }) {
                reused += 1;
                next.push(bind_group);
                next_keys.push(key);
                next_sources.push(source);
                continue;
            }
            let albedo_view = if let Some(id) = albedo_id {
                self.textures
                    .get(&id)
                    .map(|texture| texture.view)
                    .ok_or_else(|| anyhow!("GPUI 3D texture {} was not uploaded", id.0))?
            } else {
                renderer.fallback_texture_view()?
            };
            let normal_texture_view = if let Some(id) = normal_id {
                self.textures
                    .get(&id)
                    .map(|texture| texture.view)
                    .ok_or_else(|| anyhow!("GPUI 3D texture {} was not uploaded", id.0))?
            } else {
                renderer.fallback_texture_view()?
            };
            let occlusion_view = if let Some(id) = occlusion_id {
                self.textures
                    .get(&id)
                    .map(|texture| texture.view)
                    .ok_or_else(|| anyhow!("GPUI 3D texture {} was not uploaded", id.0))?
            } else {
                renderer.fallback_texture_view()?
            };
            let metallic_roughness_view = if let Some(id) = metallic_roughness_id {
                self.textures
                    .get(&id)
                    .map(|texture| texture.view)
                    .ok_or_else(|| anyhow!("GPUI 3D texture {} was not uploaded", id.0))?
            } else {
                renderer.fallback_texture_view()?
            };
            match create_draw_resource_set(
                device,
                DrawBindingResources {
                    shadow: self.shadow.as_ref().map(|shadow| shadow.raster.depths),
                    bind_group_layout,
                    mesh,
                    draw_buffer: self.draw_buffer,
                    draw_capacity: self.draw_capacity,
                    draw_slots: self.draw_slot_buffer,
                    draw_slot_capacity: self.draw_slot_capacity,
                    instance_buffer: self.instance_buffer,
                    instance_count,
                    light_buffer: self.light_buffer,
                    light_capacity: self.light_capacity,
                    albedo_view,
                    normal_texture_view,
                    occlusion_view,
                    metallic_roughness_view,
                    material_sampler: sampler,
                    frame_buffer: self.frame_buffer,
                    visible_instances: self.culling.as_ref().map(|culling| {
                        (
                            culling.visible_instances,
                            culling.instance_capacity as u64 * 4,
                        )
                    }),
                },
            ) {
                Ok(bind_group) => {
                    created_count += 1;
                    created.insert(key, bind_group);
                    next.push(bind_group);
                    next_keys.push(key);
                    next_sources.push(source);
                }
                Err(error) => {
                    for bind_group in created.into_values() {
                        device.destroy_bind_group(bind_group)?;
                    }
                    return Err(error);
                }
            }
        }
        // Publish the completed GPU mapping snapshot alongside the native
        // bindings. A failed BindGroup creation above leaves the previous
        // committed sources intact; the next retry will rebuild the scratch.
        self.generated_pending_groups = Some(next_page_groups);
        self.draw_resource_layout = self.prepared.draws.len();
        self.draw_resource_shadow = shadow_buffer;
        self.draw_resource_visible_instances = visible_instances;
        self.draw_resource_voxel = voxel_shader;
        self.draw_resource_sampler = Some(sampler);
        // Keep the previous Vec allocations as reusable scratch storage.
        std::mem::swap(&mut self.draw_resources, &mut next);
        std::mem::swap(&mut self.draw_resource_keys, &mut next_keys);
        std::mem::swap(&mut self.draw_resource_sources, &mut next_sources);
        std::mem::swap(&mut self.native_command_sources, &mut next_native);
        std::mem::swap(&mut self.generated_batch_sources, &mut next_generated);
        self.spare_draw_resources = next;
        self.spare_draw_resource_keys = next_keys;
        self.spare_draw_resource_sources = next_sources;
        self.spare_native_command_sources = next_native;
        self.spare_generated_batch_sources = next_generated;
        if force {
            for bind_group in self.binding_cache.values().copied() {
                device.destroy_bind_group(bind_group)?;
            }
            self.binding_index.clear();
            for &key in created.keys() {
                self.binding_index.insert(key);
            }
            self.binding_cache = created;
        } else {
            for &key in created.keys() {
                self.binding_index.insert(key);
            }
            self.binding_cache.extend(created);
        }
        // Binding validation is already performed by retire_bindings after
        // asset changes. Do not perform the same full cache walk twice for
        // every streamed section. A forced rebind replaces the entire cache
        // with freshly created entries and also needs no second scan.
        if !defer_binding_cleanup && !force {
            let resident_voxel_pages = self.page_refs.active_pages();
            let stale = self
                .binding_cache
                .iter()
                .filter_map(|(key, &binding)| {
                    (!key.is_resident(
                        &self.meshes,
                        &resident_voxel_pages,
                        self.culling
                            .as_ref()
                            .map(|culling| culling.visible_instances),
                    ))
                    .then_some((*key, binding))
                })
                .collect::<Vec<_>>();
            for (key, _) in stale {
                self.destroy_cached_binding(device, key)?;
            }
        }
        if let Some(started) = trace_started {
            let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
            let now = Instant::now();
            if elapsed_ms >= 16.0
                || self
                    .last_binding_report
                    .is_none_or(|last| now.duration_since(last) >= Duration::from_secs(1))
            {
                let (arena_pages, arena_capacity, arena_live, arena_dead) = self.arena.stats();
                let (generated_pages, generated_capacity, generated_live, generated_dead) =
                    self.generated_arena.borrow().stats();
                eprintln!(
                    "gpui3d bindings batches={} reused={} positional_reused={} created={} cached={} deferred_prune={} elapsed_ms={:.2} arena_pages={} arena_capacity_bytes={} arena_live_bytes={} arena_dead_bytes={} generated_pages={} generated_capacity_bytes={} generated_live_bytes={} generated_dead_bytes={}",
                    self.batches.len(),
                    reused,
                    positional_reused,
                    created_count,
                    self.binding_cache.len(),
                    defer_binding_cleanup,
                    elapsed_ms,
                    arena_pages,
                    arena_capacity,
                    arena_live,
                    arena_dead,
                    generated_pages,
                    generated_capacity,
                    generated_live,
                    generated_dead,
                );
                self.last_binding_report = Some(now);
            }
        }
        Ok(())
    }

    /// Native scene command eligibility is independent of the backend's
    /// native indirect ABI. This view has one indirect address per canonical
    /// generated singleton, with resident bindings for every logical draw.
    pub(super) fn generated_singleton_command_count(&self) -> Option<usize> {
        let count = self.prepared.draws.len();
        (self.voxel_shader
            && self.culling.is_some()
            && self.page_refs.only_generated_meshes()
            && self.batch_workspace.singleton_layout()
            && self.batches.len() == count
            && self.draw_resources.len() == count
            && self.draw_addresses.len() == count)
            .then_some(count)
    }

    pub(super) fn native_command_delta(&self) -> Option<&crate::prepare::StructuralDrawDelta> {
        self.command_delta.as_ref()
    }

    /// The indirect argument offset addresses the original Batch index.
    /// Preserve the back-to-front order of the blended suffix when the
    /// renderer groups opaque indirect commands by native state.
    pub(super) fn first_blended_batch_index(&self) -> Option<usize> {
        self.batches.iter().position(|batch| {
            self.prepared.draws.get(batch.first_instance).is_some_and(|draw| {
                draw.material.alpha_mode == AlphaMode::Blend
            })
        })
    }

    pub(super) fn append_steps(
        &self,
        renderer: &RendererResources,
        voxel_shader: bool,
        steps: &mut Vec<RenderCommand>,
    ) -> gpui::Result<()> {
        self.append_pass_steps(renderer, voxel_shader, steps, None)
    }
    fn append_pass_steps(
        &self,
        renderer: &RendererResources,
        voxel_shader: bool,
        steps: &mut Vec<RenderCommand>,
        shadow: Option<(&[BindGroupId], BufferId)>,
    ) -> gpui::Result<()> {
        let command_started = trace_timings().then(Instant::now);
        let previous_step_count = steps.len();
        steps.reserve(self.batches.len());
        // A pure Generated Mesh scene has a canonical singleton Batch layout.
        // Its BindGroups, indirect offsets and GPU argument source have already
        // been resolved at scene-sync time. Avoid one Mesh HashMap probe and
        // mesh-kind dispatch for every native command after Section splices.
        let generated_singletons = shadow.is_none()
            && voxel_shader
            && self.culling.is_some()
            && self.page_refs.only_generated_meshes()
            && self.batch_workspace.singleton_layout()
            && self.batches.len() == self.prepared.draws.len()
            && self.draw_resources.len() == self.prepared.draws.len()
            && self.draw_addresses.len() == self.prepared.draws.len();
        if generated_singletons {
            self.append_generated_singleton_range(renderer, steps, 0..self.prepared.draws.len())?;
        } else {
            let bounds_visible = self.prepared.bounds_visibility();
            for (batch_index, batch) in self.batches.iter().enumerate() {
                let index = batch.first_instance;
                let draw = self
                    .prepared
                    .draws
                    .get(index)
                    .ok_or_else(|| anyhow!("GPUI 3D draw batch has an invalid first draw"))?;
                if renderer.rejects_material(&draw.material)
                    || (shadow.is_some() && draw.material.alpha_mode == AlphaMode::Blend)
                {
                    continue;
                }
                // Reject single-instance batches before recording native commands, including
                // indirect draws whose GPU instance count would otherwise only become zero.
                // A multi-instance batch can contain visible instances after its first draw.
                // Shadow casters use a separate camera and must not use the main frustum.
                if shadow.is_none() && self.culling.is_some() && batch.instance_count == 1 {
                    // prepare() already evaluates the identical CPU native-command
                    // inclusion mask when camera, range or draw layout changes.
                    // Reuse that result instead of repeating 6-plane AABB tests
                    // while encoding potentially 100K+ DrawIndirect commands.
                    // The fallback preserves standalone native test paths that
                    // call append_steps() before the visibility snapshot exists.
                    let included = self
                        .command_visibility
                        .get(batch_index)
                        .copied()
                        .unwrap_or_else(|| {
                            bounds_visible(draw.bounds)
                                && self.prepared.is_draw_in_culling_bounds(draw)
                        });
                    if !included {
                        continue;
                    }
                }
                // Generated Section batches are normally singletons. Reject offscreen
                // instances before probing the resident mesh map or choosing a
                // pipeline; the native command list is already CPU-frustum-pruned.
                let mesh = self
                    .meshes
                    .get(&mesh_key(draw, voxel_shader))
                    .ok_or_else(|| anyhow!("GPUI 3D mesh was not uploaded"))?;
                let draw_resources = shadow
                    .map_or(self.draw_resources.as_slice(), |value| value.0)
                    .get(batch_index)
                    .ok_or_else(|| anyhow!("GPUI 3D draw resources were not initialized"))?;
                let voxel = mesh_key(draw, voxel_shader).1;
                let pipeline = if mesh.generated.is_some() {
                    renderer.generated_pipeline(
                        draw.material.alpha_mode == AlphaMode::Blend,
                        draw.raster_front_face(),
                    )?
                } else {
                    renderer.pipeline(
                        voxel,
                        draw.material.alpha_mode == AlphaMode::Blend,
                        draw.raster_front_face(),
                    )?
                };
                let instance_count = u32::try_from(batch.instance_count)
                    .context("GPUI 3D instance count exceeds u32")?;
                let address = self
                    .draw_addresses
                    .get(batch_index)
                    .ok_or_else(|| anyhow!("GPUI 3D draw address was not initialized"))?;
                let first_instance = u32::try_from(address.first_instance)
                    .context("GPUI 3D first instance exceeds u32")?;
                if mesh.generated.is_some() {
                    steps.push(RenderCommand::DrawIndirect(DrawIndirectCommand {
                        pipeline,
                        bind_groups: bind_group_list([*draw_resources]),
                        arguments: shadow
                            .map(|value| value.1)
                            .or_else(|| {
                                self.culling
                                    .as_ref()
                                    .map(|culling| culling.arguments)
                                    .or_else(|| self.generated_arguments.map(|(buffer, _)| buffer))
                            })
                            .ok_or_else(|| anyhow!("missing generated mesh arguments"))?,
                        offset: address.arguments_offset,
                        scissor: None,
                    }));
                } else if mesh.procedural_quads {
                    if let Some(culling) = &self.culling {
                        steps.push(RenderCommand::DrawIndirect(DrawIndirectCommand {
                            pipeline,
                            bind_groups: bind_group_list([*draw_resources]),
                            arguments: shadow.map_or(culling.arguments, |value| value.1),
                            offset: address.arguments_offset,
                            scissor: None,
                        }));
                    } else {
                        steps.push(RenderCommand::Draw(DrawCommand {
                            pipeline,
                            bind_groups: bind_group_list([*draw_resources]),
                            vertex_count: draw.part.index_count(),
                            first_vertex: 0,
                            instance_count,
                            first_instance,
                            scissor: None,
                        }));
                    }
                } else {
                    steps.push(RenderCommand::DrawIndexed(DrawIndexedCommand {
                        pipeline,
                        bind_groups: bind_group_list([*draw_resources]),
                        index_buffer: IndexBufferBinding {
                            buffer: mesh.indices,
                            format: mesh.index_format,
                            offset: mesh.index_offset,
                        },
                        index_count: draw.part.index_count(),
                        first_index: draw.part.first_index(),
                        base_vertex: 0,
                        instance_count,
                        first_instance,
                        scissor: None,
                    }));
                }
            }
        }
        if let Some(started) = command_started {
            let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
            if elapsed_ms >= 16.0 {
                eprintln!(
                    "gpui3d command_detail pass={} draws={} batches={} native_commands={} elapsed_ms={:.2}",
                    if shadow.is_some() { "shadow" } else { "scene" },
                    self.prepared.draws.len(),
                    self.batches.len(),
                    steps.len() - previous_step_count,
                    elapsed_ms,
                );
            }
        }
        Ok(())
    }

    /// Encodes an all-generated, canonical singleton scene directly from the
    /// retained native BindGroup and indirect-address tables. The GPU culler
    /// owns visibility counts; the CPU inclusion mask only avoids issuing
    /// commands for completely offscreen singletons.
    pub(super) fn append_generated_singleton_range(
        &self,
        renderer: &RendererResources,
        steps: &mut Vec<RenderCommand>,
        range: std::ops::Range<usize>,
    ) -> gpui::Result<()> {
        if self.generated_singleton_command_count().is_none()
            || range.start > range.end
            || range.end > self.prepared.draws.len()
        {
            return Err(anyhow!("native generated Section range is invalid"));
        }
        let arguments = self
            .culling
            .as_ref()
            .ok_or_else(|| anyhow!("GPU culling arguments are unavailable"))?
            .arguments;
        // No CPU frustum prune: the GPU cull already zeroes the instance count
        // of every unseen Section. Keeping the list camera-independent lets the
        // native command cache and bind-group run coalescing work on every
        // camera-only frame; pruned commands only saved native record time
        // that grouping now removes.
        //
        // The blended tail keeps its camera-independent candidate order in the
        // draw tables; back-to-front render order comes from `blend_order`,
        // which whole-tail ranges follow at emission time.
        let draws_len = self.prepared.draws.len();
        let blend_order = self.prepared.blended_order();
        let tail_start = draws_len.checked_sub(blend_order.len()).filter(|_| {
            blend_order
                .iter()
                .enumerate()
                .all(|(slot, &index)| index < blend_order.len())
        });
        let push_command = |index: usize, steps: &mut Vec<RenderCommand>| -> gpui::Result<()> {
            let draw = &self.prepared.draws[index];
            if renderer.rejects_material(&draw.material) {
                return Ok(());
            }
            let binding = self
                .draw_resources
                .get(index)
                .ok_or_else(|| anyhow!("GPUI 3D draw resources were not initialized"))?;
            let address = self
                .draw_addresses
                .get(index)
                .ok_or_else(|| anyhow!("GPUI 3D draw address was not initialized"))?;
            let pipeline = renderer.generated_pipeline(
                draw.material.alpha_mode == AlphaMode::Blend,
                draw.raster_front_face(),
            )?;
            steps.push(RenderCommand::DrawIndirect(DrawIndirectCommand {
                pipeline,
                bind_groups: bind_group_list([*binding]),
                arguments,
                offset: address.arguments_offset,
                scissor: None,
            }));
            Ok(())
        };
        let split = range.end.min(tail_start.unwrap_or(draws_len));
        for index in range.start..split {
            push_command(index, steps)?;
        }
        match tail_start {
            Some(tail_start) if range.end > tail_start => {
                let local_start = range.start.max(tail_start) - tail_start;
                let local_end = range.end - tail_start;
                for (slot, &tail_index) in blend_order.iter().enumerate() {
                    if slot >= local_start && slot < local_end {
                        push_command(tail_start + tail_index, steps)?;
                    }
                }
            }
            _ => {
                for index in split..range.end {
                    push_command(index, steps)?;
                }
            }
        }
        Ok(())
    }

    pub(super) fn destroy(&mut self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        if let Some((buffer, _)) = self.generated_arguments.take() {
            device.destroy_buffer(buffer)?;
        }
        for mapping in self.generated_mappings.drain().map(|(_, mapping)| mapping) {
            device.destroy_buffer(mapping.buffer)?;
        }
        if let Some(shaders) = self.mesh_shader.take() {
            for shader in shaders.0 {
                device.destroy_shader_module(shader)?;
            }
        }
        if let Some(shader) = self.mesh_draw_shader.take() {
            device.destroy_shader_module(shader)?;
        }
        if let Some(culling) = self.culling.take() {
            culling.destroy(device)?;
        }
        self.draw_resources.clear();
        self.draw_resource_keys.clear();
        self.draw_resource_sources.clear();
        self.spare_draw_resource_sources.clear();
        self.draw_slot_keys.clear();
        self.scratch_draw_slot_keys.clear();
        self.spare_draw_resources.clear();
        self.spare_draw_resource_keys.clear();
        self.instance_slot_keys.clear();
        self.scratch_instance_keys.clear();
        self.dirty_instance_run.clear();
        self.draw_addresses.clear();
        for bind_group in self.binding_cache.drain().map(|(_, binding)| binding) {
            device.destroy_bind_group(bind_group)?;
        }
        self.binding_index.clear();
        if let Some(shadow) = self.shadow.take() {
            shadow.destroy(device)?;
        }
        if let Some(pass) = self.light_pass.take() {
            pass.destroy(device)?;
        }
        self.page_refs.clear();
        for (_, mesh) in self.meshes.drain() {
            destroy_mesh(
                device,
                mesh,
                &mut self.arena,
                &mut *self.generated_arena.borrow_mut(),
                &mut *self.template_arena.borrow_mut(),
                &mut *self.argument_arena.borrow_mut(),
            )?;
        }
        for mesh in self.retired_meshes.drain(..) {
            destroy_mesh(
                device,
                mesh,
                &mut self.arena,
                &mut *self.generated_arena.borrow_mut(),
                &mut *self.template_arena.borrow_mut(),
                &mut *self.argument_arena.borrow_mut(),
            )?;
        }
        for (_, texture) in self.textures.drain() {
            destroy_texture(device, texture)?;
        }
        device.destroy_buffer(self.draw_buffer)?;
        device.destroy_buffer(self.draw_slot_buffer)?;
        device.destroy_buffer(self.instance_buffer)?;
        device.destroy_buffer(self.light_buffer)?;
        device.destroy_buffer(self.frame_buffer)?;
        Ok(())
    }
}

type StagedMesh = ((MeshId, bool), MeshResources, Option<PendingMeshUpload>);

/// Collect independently decoded Section upserts, perform one ordered compute
/// submission for the group, then commit persistent buffers by stable MeshId.
fn flush_pending_generated_meshes(
    device: &mut dyn ExtensionDevice,
    staged_gpu: &mut Vec<((MeshId, bool), Arc<Mesh>)>,
    shaders: Option<&super::generated::Shaders>,
    pending: &mut Vec<StagedMesh>,
    meshes: &mut HashMap<(MeshId, bool), MeshResources>,
    generated_mappings: &mut HashMap<BufferId, generated::Mapping>,
    page_refs: &mut page_refs::PageRefs,
    retired_meshes: &mut Vec<MeshResources>,
    arena: &mut MeshArena,
    generated_arena: &mut MeshArena,
    template_arena: &mut MeshArena,
    argument_arena: &mut MeshArena,
    backend: BackendKind,
) -> gpui::Result<()> {
    if !staged_gpu.is_empty() {
        let shaders = shaders.ok_or_else(|| anyhow!("missing GPU meshing shaders"))?;
        let inputs = staged_gpu
            .iter()
            .map(|(_, mesh)| mesh.as_ref())
            .collect::<Vec<_>>();
        let created = super::generated::create_many_pooled(
            device,
            &inputs,
            shaders,
            generated_arena,
            template_arena,
            argument_arena,
            backend,
        )?;
        for ((id, _), resource) in staged_gpu.drain(..).zip(created) {
            if let Some(generated) = &resource.generated {
                // Reused Argument Page/slot identifiers alone cannot reveal
                // that GPU Compute has written new Part Counts. Force this
                // Page's indirect table to consume freshly generated counts.
                generated::invalidate_argument_page(generated_mappings, generated.arguments);
            }
            pending.push((id, resource, None));
        }
    }
    upload_pending_meshes(
        device,
        pending,
        backend,
        meshes,
        page_refs,
        retired_meshes,
        arena,
        generated_arena,
        template_arena,
        argument_arena,
    )
}

fn discard_pending_meshes(
    device: &mut dyn ExtensionDevice,
    pending: &mut Vec<StagedMesh>,
    arena: &mut MeshArena,
    generated_arena: &mut MeshArena,
    template_arena: &mut MeshArena,
    argument_arena: &mut MeshArena,
) -> gpui::Result<()> {
    for (_, mesh, _) in pending.drain(..) {
        destroy_mesh(
            device,
            mesh,
            arena,
            generated_arena,
            template_arena,
            argument_arena,
        )?;
    }
    Ok(())
}

fn upload_pending_meshes(
    device: &mut dyn ExtensionDevice,
    pending: &mut Vec<StagedMesh>,
    backend: BackendKind,
    meshes: &mut HashMap<(MeshId, bool), MeshResources>,
    page_refs: &mut page_refs::PageRefs,
    retired_meshes: &mut Vec<MeshResources>,
    arena: &mut MeshArena,
    generated_arena: &mut MeshArena,
    template_arena: &mut MeshArena,
    argument_arena: &mut MeshArena,
) -> gpui::Result<()> {
    if pending.is_empty() {
        return Ok(());
    }
    let writes = pending
        .iter()
        .filter_map(|(_, _, upload)| upload.as_ref())
        .map(|upload| BufferWrite {
            descriptor: BufferWriteDescriptor {
                buffer: upload.buffer,
                offset: upload.offset,
            },
            data: &upload.bytes,
        })
        .collect::<Vec<_>>();
    let uploaded_bytes = writes.iter().map(|write| write.data.len()).sum::<usize>();
    let write_count = writes.len();
    let upload_started = Instant::now();
    if let Err(error) = device.write_buffer_batch(&writes) {
        discard_pending_meshes(
            device,
            pending,
            arena,
            generated_arena,
            template_arena,
            argument_arena,
        )?;
        return Err(error.into());
    }
    if write_count > 0 {
        crate::upload_feedback::observe_buffer_upload(
            backend, uploaded_bytes, write_count, upload_started.elapsed(),
        );
    }
    for (id, mesh, _) in pending.drain(..) {
        // Account for the incoming generation before retiring an older
        // generation which may refer to the same shared buffer.
        page_refs.insert(&mesh, id.1);
        if let Some(old) = meshes.insert(id, mesh) {
            page_refs.remove(&old, id.1);
            retired_meshes.push(old);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        DrawMeshKey, DrawResourceKey, mesh_key, scene_asset_ids, scene_assets_changed,
        update_prepared, validate_textures,
    };
    use crate::{
        AnimationScratch, Camera, Material, Mesh, MeshId, Node, PreparedScene, Scene, SceneView,
        ShadingModel, TextureAsset, TextureAssetId, TransformTrack, Vec3, Vec3Track,
    };
    use std::{collections::HashSet, sync::Arc, time::Duration};

    #[test]
    fn cached_binding_remains_valid_while_mesh_and_texture_belong_to_scene() {
        let mesh = MeshId(7);
        let texture = TextureAssetId(11);
        let key = DrawResourceKey {
            shadow: None,
            visible_instances: None,
            mesh: DrawMeshKey::Mesh {
                id: mesh,
                generation: 2,
            },
            albedo_id: Some(texture),
            normal_id: None,
            occlusion_id: None,
            metallic_roughness_id: None,
        };
        let meshes = HashSet::from([mesh]);
        let textures = HashSet::from([texture]);

        assert!(key.uses_active_assets(&meshes, &HashSet::new(), &textures));
        assert!(!key.uses_active_assets(&HashSet::new(), &HashSet::new(), &textures));
        assert!(!key.uses_active_assets(&meshes, &HashSet::new(), &HashSet::new()));
        let page = gfx_core::BufferId::from_parts(7, 9);
        let voxel_key = DrawResourceKey {
            mesh: DrawMeshKey::VoxelPage(page),
            ..key
        };
        assert!(voxel_key.uses_active_assets(&HashSet::new(), &HashSet::from([page]), &textures));
        let visible = gfx_core::BufferId::from_parts(9, 1);
        let culling_key = DrawResourceKey {
            visible_instances: Some(visible),
            ..voxel_key
        };
        let pages = HashSet::from([page]);
        assert!(culling_key.is_resident(&std::collections::HashMap::new(), &pages, Some(visible)));
        assert!(
            !culling_key.is_resident(
                &std::collections::HashMap::new(),
                &pages,
                Some(gfx_core::BufferId::from_parts(9, 2))
            ),
            "resized visible-instance buffers must invalidate cached bindings"
        );
    }

    #[test]
    fn pooled_generated_buffers_never_alias_mesh_specific_template_bindings() {
        // Two Sections may share one generated-face GPU page while their
        // template buffers, static light data and indirect arguments differ.
        // Resource keys must retain face-page and template-buffer identity so
        // shared bindings are never aliased across pooled draws.
        let faces = gfx_core::BufferId::from_parts(1, 1);
        let shared_templates = gfx_core::BufferId::from_parts(2, 1);
        let generic = DrawResourceKey {
            shadow: None,
            visible_instances: None,
            mesh: DrawMeshKey::GeneratedPage {
                faces,
                templates: shared_templates,
            },
            albedo_id: Some(TextureAssetId(9)),
            normal_id: None,
            occlusion_id: None,
            metallic_roughness_id: None,
        };
        let other = DrawResourceKey {
            mesh: DrawMeshKey::GeneratedPage {
                faces,
                templates: gfx_core::BufferId::from_parts(3, 1),
            },
            ..generic
        };
        let moved = DrawResourceKey {
            mesh: DrawMeshKey::GeneratedPage {
                faces: gfx_core::BufferId::from_parts(4, 1),
                templates: shared_templates,
            },
            ..generic
        };
        let ordinary = DrawResourceKey {
            mesh: DrawMeshKey::VoxelPage(faces),
            ..generic
        };
        assert_ne!(generic, other);
        assert_ne!(generic, moved);
        assert_ne!(generic, ordinary);
        let pages = HashSet::from([faces, shared_templates]);
        let textures = HashSet::from([TextureAssetId(9)]);
        assert!(generic.uses_active_assets(&HashSet::new(), &pages, &textures));
        assert!(
            !other.uses_active_assets(&HashSet::new(), &pages, &textures),
            "a missing template page must not count the key as resident"
        );
    }

    #[test]
    fn shared_mesh_uses_separate_voxel_and_regular_cache_keys() {
        let mesh = Arc::new(Mesh::cube());
        let mut unlit = Material::new();
        unlit.shading_model = ShadingModel::Unlit;
        let mut scene = Scene::new();
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(mesh.clone())
                    .with_materials([Arc::new(unlit)]),
            )
            .unwrap();
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(mesh.clone())
                    .with_materials([Arc::new(Material::new())]),
            )
            .unwrap();
        let camera = Camera::perspective(
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::ZERO,
            Vec3::Y,
            1.0,
            0.1,
            10.0,
        );
        let view = SceneView::new(Arc::new(scene), camera).with_voxel_shader();
        let mut prepared = PreparedScene::default();
        update_prepared(&mut prepared, &mut AnimationScratch::new(), &view, 1.0).unwrap();

        let keys = prepared
            .draws
            .iter()
            .map(|draw| mesh_key(draw, true))
            .collect::<std::collections::HashSet<_>>();
        assert!(keys.contains(&(mesh.id(), true)));
        assert!(keys.contains(&(mesh.id(), false)));
        assert!(prepared.draws.iter().all(|draw| !mesh_key(draw, false).1));
    }

    #[test]
    fn scene_asset_ids_keep_offscreen_resources_until_nodes_are_removed() {
        let mesh = Arc::new(Mesh::cube());
        let mut material = Material::new();
        material.albedo_texture = Some(TextureAssetId(42));
        let mut scene = Scene::new();
        let node = scene
            .insert(
                None,
                Node::new()
                    .with_mesh(mesh.clone())
                    .with_materials([Arc::new(material)])
                    .with_visible(false),
            )
            .unwrap();

        let (meshes, textures) = scene_asset_ids(&scene);
        assert!(meshes.contains(&mesh.id()));
        assert!(textures.contains(&TextureAssetId(42)));

        scene.remove(node).unwrap();
        let (meshes, textures) = scene_asset_ids(&scene);
        assert!(meshes.is_empty());
        assert!(textures.is_empty());
    }

    #[test]
    fn camera_changes_keep_scene_assets_reusable() {
        let scene_view = SceneView::new(
            Arc::new(Scene::new()),
            Camera::perspective(
                Vec3::new(0.0, 0.0, 3.0),
                Vec3::ZERO,
                Vec3::Y,
                1.0,
                0.1,
                10.0,
            ),
        );
        let rotated = scene_view.with_camera(Camera {
            eye: Vec3::new(3.0, 0.0, 0.0),
            ..scene_view.camera
        });

        assert!(!scene_assets_changed(
            Some(&scene_view.scene),
            Some(&scene_view.textures),
            &rotated,
        ));
        // A failed suffix upload clears the committed scene identity, forcing a full resync
        // even if the CPU has already sorted this same camera's transparent draws.
        assert!(scene_assets_changed(
            None,
            Some(&scene_view.textures),
            &rotated
        ));
    }

    #[test]
    fn camera_recull_can_reveal_a_previously_hidden_mesh() {
        let mut scene = Scene::new();
        scene
            .insert(None, Node::new().with_mesh(Arc::new(Mesh::cube())))
            .unwrap();
        let away = Camera::perspective(
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::new(0.0, 0.0, 6.0),
            Vec3::Y,
            1.0,
            0.1,
            10.0,
        );
        let mut prepared = PreparedScene::new(&scene, away, 1.0).unwrap();
        assert!(prepared.draws.is_empty());

        let toward = Camera::perspective(away.eye, Vec3::ZERO, Vec3::Y, 1.0, 0.1, 10.0);
        assert!(prepared.update_camera(toward, 1.0).unwrap());
        assert_eq!(prepared.draws.len(), 1);
    }

    #[test]
    fn scene_view_preparation_samples_animation_at_snapshot_time() {
        let mut scene = Scene::new();
        let node = scene
            .insert(None, Node::new().with_mesh(Arc::new(Mesh::cube())))
            .unwrap();
        let track = TransformTrack::new(node).with_translation(
            Vec3Track::new([
                crate::Keyframe::new(Duration::ZERO, Vec3::ZERO),
                crate::Keyframe::new(Duration::from_secs(1), Vec3::new(2.0, 0.0, 0.0)),
            ])
            .unwrap(),
        );
        let camera = Camera::perspective(
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::ZERO,
            Vec3::Y,
            1.0,
            0.1,
            10.0,
        );
        let scene_view = SceneView::new(Arc::new(scene), camera)
            .with_animation([track], Duration::from_millis(500))
            .unwrap();
        let mut prepared = crate::PreparedScene::default();
        let mut animation_scratch = AnimationScratch::new();

        update_prepared(&mut prepared, &mut animation_scratch, &scene_view, 1.0).unwrap();

        assert_eq!(prepared.draws.len(), 1);
        assert!((prepared.draws[0].world.transform_point(Vec3::ZERO).x - 1.0).abs() < 1e-5);

        update_prepared(
            &mut prepared,
            &mut animation_scratch,
            &scene_view.with_animation_time(Duration::from_secs(1)),
            1.0,
        )
        .unwrap();

        assert!((prepared.draws[0].world.transform_point(Vec3::ZERO).x - 2.0).abs() < 1e-5);
    }

    #[test]
    fn scene_view_rejects_missing_occlusion_texture_assets() {
        let mut material = Material::new();
        material.occlusion_texture = Some(TextureAssetId(19));
        let mut scene = Scene::new();
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(Arc::new(Mesh::cube()))
                    .with_materials([Arc::new(material)]),
            )
            .unwrap();
        let camera = Camera::perspective(
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::ZERO,
            Vec3::Y,
            1.0,
            0.1,
            10.0,
        );
        let scene_view = SceneView::new(Arc::new(scene), camera);
        let prepared = PreparedScene::new(scene_view.scene(), camera, 1.0).unwrap();

        let error = validate_textures(&prepared, &scene_view).unwrap_err();

        assert!(error.to_string().contains("texture asset 19"));
    }

    #[test]
    fn texture_validation_checks_distinct_material_snapshots_with_same_id() {
        let material = Material::new();
        let mut changed = material.clone();
        changed.occlusion_texture = Some(TextureAssetId(19));
        let mut scene = Scene::new();
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(Arc::new(Mesh::cube()))
                    .with_materials([Arc::new(material)]),
            )
            .unwrap();
        let camera = Camera::perspective(
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::ZERO,
            Vec3::Y,
            1.0,
            0.1,
            10.0,
        );
        let view = SceneView::new(Arc::new(scene), camera);
        let mut prepared = PreparedScene::new(view.scene(), camera, 1.0).unwrap();
        let mut other = prepared.draws[0].clone();
        other.material = Arc::new(changed);
        prepared.draws.push(other);
        let error = validate_textures(&prepared, &view).unwrap_err();
        assert!(error.to_string().contains("texture asset 19"));
    }

    #[test]
    fn scene_view_rejects_missing_metallic_roughness_texture_assets() {
        let mut material = Material::new();
        material.metallic_roughness_texture = Some(TextureAssetId(23));
        let mut scene = Scene::new();
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(Arc::new(Mesh::cube()))
                    .with_materials([Arc::new(material)]),
            )
            .unwrap();
        let camera = Camera::perspective(
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::ZERO,
            Vec3::Y,
            1.0,
            0.1,
            10.0,
        );
        let scene_view = SceneView::new(Arc::new(scene), camera);
        let prepared = PreparedScene::new(scene_view.scene(), camera, 1.0).unwrap();

        let error = validate_textures(&prepared, &scene_view).unwrap_err();

        assert!(error.to_string().contains("texture asset 23"));
    }

    #[test]
    fn scene_view_rejects_normal_maps_without_mesh_tangents() {
        let texture = Arc::new(
            TextureAsset::linear_rgba8(1, 1, Arc::<[u8]>::from([128, 128, 255, 255])).unwrap(),
        );
        let mut material = Material::new();
        material.normal_texture = Some(texture.id());
        let mut scene = Scene::new();
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(Arc::new(Mesh::cube()))
                    .with_materials([Arc::new(material)]),
            )
            .unwrap();
        let camera = Camera::perspective(
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::ZERO,
            Vec3::Y,
            1.0,
            0.1,
            10.0,
        );
        let scene_view = SceneView::new(Arc::new(scene), camera)
            .with_textures([texture])
            .unwrap();
        let prepared = PreparedScene::new(scene_view.scene(), camera, 1.0).unwrap();

        let error = validate_textures(&prepared, &scene_view).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("require generated or supplied mesh tangents")
        );
    }

    #[test]
    fn scene_view_rejects_srgb_normal_texture_assets() {
        let texture = Arc::new(TextureAsset::rgba8(1, 1, Arc::<[u8]>::from([128; 4])).unwrap());
        let mut material = Material::new();
        material.normal_texture = Some(texture.id());
        let mesh = Mesh::cube().generate_tangents().unwrap().into_parts().0;
        let mut scene = Scene::new();
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(Arc::new(mesh))
                    .with_materials([Arc::new(material)]),
            )
            .unwrap();
        let camera = Camera::perspective(
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::ZERO,
            Vec3::Y,
            1.0,
            0.1,
            10.0,
        );
        let scene_view = SceneView::new(Arc::new(scene), camera)
            .with_textures([texture])
            .unwrap();
        let prepared = PreparedScene::new(scene_view.scene(), camera, 1.0).unwrap();

        let error = validate_textures(&prepared, &scene_view).unwrap_err();

        assert!(error.to_string().contains("must use linear color space"));
    }

    #[test]
    fn scene_view_rejects_srgb_occlusion_texture_assets() {
        let texture = Arc::new(TextureAsset::rgba8(1, 1, Arc::<[u8]>::from([128; 4])).unwrap());
        let mut material = Material::new();
        material.occlusion_texture = Some(texture.id());
        let mut scene = Scene::new();
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(Arc::new(Mesh::cube()))
                    .with_materials([Arc::new(material)]),
            )
            .unwrap();
        let camera = Camera::perspective(
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::ZERO,
            Vec3::Y,
            1.0,
            0.1,
            10.0,
        );
        let scene_view = SceneView::new(Arc::new(scene), camera)
            .with_textures([texture])
            .unwrap();
        let prepared = PreparedScene::new(scene_view.scene(), camera, 1.0).unwrap();

        let error = validate_textures(&prepared, &scene_view).unwrap_err();

        assert!(error.to_string().contains("must use linear color space"));
    }
}

#[cfg(test)]
fn scene_asset_ids(scene: &Scene) -> (HashSet<MeshId>, HashSet<TextureAssetId>) {
    let mut meshes = HashSet::new();
    let mut textures = HashSet::new();
    let mut pending = scene.roots().to_vec();
    while let Some(id) = pending.pop() {
        let Some(node) = scene.node(id) else {
            continue;
        };
        if let Some(mesh) = &node.mesh {
            meshes.insert(mesh.id());
        }
        for level in &node.lod {
            meshes.insert(level.mesh.id());
        }
        for material in node
            .materials
            .iter()
            .chain(node.lod.iter().flat_map(|level| level.materials.iter()))
        {
            textures.extend(
                [
                    material.albedo_texture,
                    material.normal_texture,
                    material.occlusion_texture,
                    material.metallic_roughness_texture,
                ]
                .into_iter()
                .flatten(),
            );
        }
        if let Some(children) = scene.children(id) {
            pending.extend_from_slice(children);
        }
    }
    (meshes, textures)
}

pub(super) fn mesh_key(draw: &PreparedDraw, voxel_shader: bool) -> (MeshId, bool) {
    (
        draw.mesh.id(),
        voxel_shader
            && draw.material.shading_model == ShadingModel::Unlit
            && can_encode_voxel_mesh(&draw.mesh),
    )
}

fn draw_identity(draw: &PreparedDraw) -> (NodeId, MeshId, MeshPart, MaterialId) {
    (draw.node, draw.mesh.id(), draw.part, draw.material.id())
}

fn scene_assets_changed(
    scene: Option<&Arc<Scene>>,
    texture_table: Option<&Arc<HashMap<TextureAssetId, Arc<crate::TextureAsset>>>>,
    scene_view: &SceneView,
) -> bool {
    scene.is_none_or(|scene| !Arc::ptr_eq(scene, &scene_view.scene))
        || texture_table.is_none_or(|textures| !Arc::ptr_eq(textures, &scene_view.textures))
}

fn validate_textures(prepared: &PreparedScene, scene_view: &SceneView) -> anyhow::Result<()> {
    validate_draw_textures(&prepared.draws, scene_view)
}

fn validate_draw_textures(draws: &[PreparedDraw], scene_view: &SceneView) -> anyhow::Result<()> {
    let mut seen = HashSet::new();
    for draw in draws {
        if draw.material.shading_model == ShadingModel::MetallicRoughness
            && draw.material.normal_texture.is_some()
            && draw
                .mesh
                .tangents()
                .is_none_or(|tangents| tangents.len() != draw.mesh.vertices().len())
        {
            bail!("GPUI 3D normal maps require generated or supplied mesh tangents")
        }
        if !seen.insert(Arc::as_ptr(&draw.material)) {
            continue;
        }
        for texture_id in [
            draw.material.albedo_texture,
            draw.material.normal_texture,
            draw.material.occlusion_texture,
            draw.material.metallic_roughness_texture,
        ]
        .into_iter()
        .flatten()
        {
            let asset = scene_view.texture(texture_id).ok_or_else(|| {
                anyhow!("GPUI 3D scene view has no texture asset {}", texture_id.0)
            })?;
            if draw.material.occlusion_texture == Some(texture_id)
                && asset.color_space() != crate::TextureColorSpace::Linear
            {
                bail!(
                    "GPUI 3D occlusion texture {} must use linear color space",
                    texture_id.0
                )
            }
            if draw.material.normal_texture == Some(texture_id)
                && asset.color_space() != crate::TextureColorSpace::Linear
            {
                bail!(
                    "GPUI 3D normal texture {} must use linear color space",
                    texture_id.0
                )
            }
            if draw.material.metallic_roughness_texture == Some(texture_id)
                && asset.color_space() != crate::TextureColorSpace::Linear
            {
                bail!(
                    "GPUI 3D metallic-roughness texture {} must use linear color space",
                    texture_id.0
                )
            }
        }
    }
    Ok(())
}

/// Animation clock advances without changing a static scene. Only the
/// transition into/out of active tracks, changing the track snapshot, or
/// advancing an actively sampled clip invalidates CPU draw preparation.
fn animation_affects_draws(
    previous: Option<&Arc<[crate::TransformTrack]>>,
    previous_time: Option<Duration>,
    current: &Arc<[crate::TransformTrack]>,
    current_time: Duration,
) -> bool {
    if current.is_empty() {
        // Two empty track sets are semantically equivalent regardless of Arc
        // allocation identity or clock time.
        return previous.is_some_and(|tracks| !tracks.is_empty());
    }
    previous.is_none_or(|tracks| !Arc::ptr_eq(tracks, current))
        || previous_time != Some(current_time)
}

fn update_prepared(
    prepared: &mut PreparedScene,
    animation_scratch: &mut AnimationScratch,
    scene_view: &SceneView,
    aspect: f32,
) -> anyhow::Result<()> {
    if scene_view.animation.is_empty() {
        prepared
            .update_transformed(
                &scene_view.scene,
                scene_view.camera,
                aspect,
                scene_view.scene_transform,
            )
            .context("preparing GPUI 3D scene")?;
    } else {
        let evaluated = scene_view
            .scene
            .evaluate_with(
                &scene_view.animation,
                scene_view.animation_time,
                animation_scratch,
            )
            .context("evaluating GPUI 3D scene animation")?;
        prepared
            .update_transformed_evaluated(
                &evaluated,
                scene_view.camera,
                aspect,
                scene_view.scene_transform,
            )
            .context("preparing animated GPUI 3D scene")?;
    }
    Ok(())
}

/// Update a retained native-command inclusion mask without per-frame allocation.
/// A camera movement that leaves the mask unchanged can reuse the previous commands.
/// Update a subset of a previously committed visibility mask without
/// enumerating every native Batch. Resolve all changes before mutating it,
/// so invalid indices cannot leave a partially updated mask.
fn update_dirty_command_visibility(
    previous: &mut [bool],
    dirty: &[usize],
    value_at: impl Fn(usize) -> Option<bool>,
) -> Option<bool> {
    let mut patches = Vec::with_capacity(dirty.len());
    for &index in dirty {
        let old = *previous.get(index)?;
        patches.push((index, old, value_at(index)?));
    }
    let mut changed = false;
    for (index, old, new) in patches {
        if old != new {
            previous[index] = new;
            changed = true;
        }
    }
    Some(changed)
}

fn update_command_visibility(
    previous: &mut Vec<bool>,
    current: impl IntoIterator<Item = bool>,
) -> bool {
    let mut changed = false;
    let mut len = 0;
    for included in current {
        if let Some(old) = previous.get_mut(len) {
            if *old != included {
                *old = included;
                changed = true;
            }
        } else {
            previous.push(included);
            changed = true;
        }
        len += 1;
    }
    if previous.len() != len {
        previous.truncate(len);
        changed = true;
    }
    changed
}

#[cfg(test)]
mod native_command_snapshot_tests {
    use super::*;
    use crate::{Material, Node, Vec3};

    #[test]
    fn translation_keeps_native_pipeline_but_mirror_and_material_changes_do_not() {
        let mut scene = Scene::new();
        let material = Arc::new(Material::new());
        let id = scene
            .insert(
                None,
                Node::new()
                    .with_mesh(Arc::new(Mesh::cube()))
                    .with_materials([material]),
            )
            .unwrap();
        let camera =
            Camera::perspective(Vec3::new(0., 0., 20.), Vec3::ZERO, Vec3::Y, 1., 0.1, 100.);
        let initial = PreparedScene::new(&scene, camera, 1.).unwrap();
        let original = NativeCommandSource::for_draw(&initial.draws[0]);

        let mut translated = scene.fork();
        translated.node_mut(id).unwrap().transform.translation.x = 3.;
        let translated_draws = PreparedScene::new(&translated, camera, 1.).unwrap();
        assert_eq!(
            original,
            NativeCommandSource::for_draw(&translated_draws.draws[0])
        );

        translated.node_mut(id).unwrap().transform.scale.x = -1.;
        let mirrored = PreparedScene::new(&translated, camera, 1.).unwrap();
        assert_ne!(original, NativeCommandSource::for_draw(&mirrored.draws[0]));

        let mut different_material = initial.draws[0].clone();
        different_material.material = Arc::new(Material::new());
        assert_ne!(original, NativeCommandSource::for_draw(&different_material),);
    }
}

#[cfg(test)]
mod streaming_light_headroom_tests {
    use super::*;

    #[test]
    fn light_growth_keeps_headroom_without_rebinding_each_threshold() {
        let limit = 512 * 1024 * 1024;
        let headroom = streaming_light_headroom(1_955, limit);
        assert_eq!(headroom, 7_820);
        assert_eq!(grow_capacity(1, headroom).unwrap(), 8_192);
        assert_eq!(streaming_light_headroom(2_048, limit), 8_192);
    }

    #[test]
    fn light_growth_respects_storage_binding_limits() {
        let limit = u64::from(LIGHT_STRIDE) * 256;
        assert_eq!(streaming_light_headroom(128, limit), 256);
        assert_eq!(streaming_light_headroom(256, limit), 256);
        assert_eq!(streaming_light_headroom(300, limit), 300);
    }
}

#[cfg(test)]
mod sparse_upload_budget_tests {
    use super::*;

    #[test]
    fn high_draw_count_uses_more_ranges_only_for_batched_native_uploads() {
        assert_eq!(sparse_upload_range_budget(BackendKind::Vulkan, 103_305), 64);
        assert_eq!(sparse_upload_range_budget(BackendKind::Dx12, 103_305), 64);
        assert_eq!(sparse_upload_range_budget(BackendKind::Vulkan, 10_000), 16);
        assert_eq!(sparse_upload_range_budget(BackendKind::Dx11, 103_305), 16);
        assert_eq!(sparse_upload_range_budget(BackendKind::OpenGl, 103_305), 16);
    }
}

/// Derive narrow logical-to-physical u32 writes from validated Section
/// splices. Surviving physical GPU records stay at their original addresses,
/// and only logical indirection entries which changed need a driver write.
///
/// A middle insertion/removal moves the intervening logical indices, but once
/// opposing splices restore the offset the untouched tail needs no upload.
/// Malformed or heavily fragmented deltas use the established suffix fallback.
fn structural_draw_slot_patches(
    previous: &[u8],
    next: &[u32],
    delta: &crate::prepare::StructuralDrawDelta,
) -> Option<Vec<(usize, Vec<u8>)>> {
    if previous.len() % 4 != 0
        || delta.previous_count != previous.len() / 4
        || delta.edits.is_empty()
    {
        return None;
    }
    let old_len = previous.len() / 4;
    let mut ranges = Vec::<std::ops::Range<usize>>::new();
    let mut push_range = |range: std::ops::Range<usize>| {
        if range.is_empty() {
            return;
        }
        if let Some(last) = ranges.last_mut()
            && range.start <= last.end
        {
            last.end = last.end.max(range.end);
        } else {
            ranges.push(range);
        }
    };
    let (mut old_cursor, mut new_cursor) = (0usize, 0usize);
    for edit in &delta.edits {
        if edit.old.start < old_cursor
            || edit.new.start < new_cursor
            || edit.old.start > edit.old.end
            || edit.new.start > edit.new.end
            || edit.old.end > old_len
            || edit.new.end > next.len()
            || edit.old.start - old_cursor != edit.new.start - new_cursor
        {
            return None;
        }
        if old_cursor != new_cursor {
            push_range(new_cursor..edit.new.start);
        }
        if edit.old.start == edit.new.start && edit.old.len() == edit.new.len() {
            // Equal-size edits may preserve most physical slots. Compare
            // only their small exact changed interval, not the full table.
            let mut dirty_start = None;
            for logical in edit.new.clone() {
                let start = logical * 4;
                let unchanged = previous.get(start..start + 4)
                    == Some(&next[logical].to_le_bytes()[..]);
                if !unchanged && dirty_start.is_none() {
                    dirty_start = Some(logical);
                } else if unchanged && let Some(first) = dirty_start.take() {
                    push_range(first..logical);
                }
            }
            if let Some(first) = dirty_start {
                push_range(first..edit.new.end);
            }
        } else {
            push_range(edit.new.clone());
        }
        old_cursor = edit.old.end;
        new_cursor = edit.new.end;
    }
    if old_len - old_cursor != next.len() - new_cursor {
        return None;
    }
    if old_cursor != new_cursor {
        push_range(new_cursor..next.len());
    }
    drop(push_range);
    // Avoid excessive driver writes when many small independent edits arrive.
    if ranges.len() > 12 {
        return None;
    }
    Some(ranges.into_iter().map(|range| {
        let mut bytes = Vec::with_capacity(range.len() * 4);
        for &physical in &next[range.clone()] {
            bytes.extend_from_slice(&physical.to_le_bytes());
        }
        (range.start * 4, bytes)
    }).collect())
}

#[cfg(test)]
mod structural_draw_slot_patch_tests {
    use super::structural_draw_slot_patches;
    use crate::prepare::{StructuralDrawDelta, StructuralDrawEdit};

    fn edit(old: std::ops::Range<usize>, new: std::ops::Range<usize>) -> StructuralDrawEdit {
        StructuralDrawEdit { old, new }
    }

    fn encoded(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|value| value.to_le_bytes()).collect()
    }

    fn patched(previous: &[u32], next: &[u32], delta: StructuralDrawDelta) -> (Vec<u8>, usize) {
        let mut bytes = encoded(previous);
        let patches = structural_draw_slot_patches(&bytes, next, &delta).unwrap();
        bytes.resize(next.len() * 4, 0);
        let uploaded = patches.iter().map(|(_, bytes)| bytes.len()).sum();
        for (offset, data) in patches {
            bytes[offset..offset + data.len()].copy_from_slice(&data);
        }
        (bytes, uploaded)
    }

    #[test]
    fn append_uploads_only_new_slots_and_tail_retirement_uploads_nothing() {
        let old = (0..65_536).collect::<Vec<u32>>();
        let mut appended = old.clone();
        appended.extend([75_001, 75_002]);
        let (bytes, uploaded) = patched(&old, &appended, StructuralDrawDelta {
            previous_count: old.len(),
            edits: vec![edit(old.len()..old.len(), old.len()..appended.len())],
        });
        assert_eq!(bytes, encoded(&appended));
        assert_eq!(uploaded, 8);
        let retained = &old[..old.len() - 5];
        let (bytes, uploaded) = patched(&old, retained, StructuralDrawDelta {
            previous_count: old.len(),
            edits: vec![edit(retained.len()..old.len(), retained.len()..retained.len())],
        });
        assert_eq!(bytes, encoded(retained));
        assert_eq!(uploaded, 0);
    }

    #[test]
    fn fixed_width_slot_edits_do_not_upload_unchanged_rows() {
        let old = (0..4096).collect::<Vec<u32>>();
        let mut next = old.clone();
        next[1024] = 8001;
        next[1027] = 8002;
        let (bytes, uploaded) = patched(&old, &next, StructuralDrawDelta {
            previous_count: old.len(), edits: vec![edit(1000..1030, 1000..1030)],
        });
        assert_eq!(bytes, encoded(&next));
        assert_eq!(uploaded, 8);
    }

    #[test]
    fn opposite_middle_splices_leave_the_later_tail_resident() {
        let old = (0..4096).collect::<Vec<u32>>();
        let mut next = old.clone();
        next.splice(100..100, [9001, 9002]);
        next.drain(504..506);
        let (bytes, uploaded) = patched(&old, &next, StructuralDrawDelta {
            previous_count: old.len(),
            edits: vec![edit(100..100, 100..102), edit(502..504, 504..504)],
        });
        assert_eq!(bytes, encoded(&next));
        assert!(uploaded <= 404 * 4);
    }

    #[test]
    fn invalid_or_fragmented_edits_use_the_general_fallback() {
        let old = (0..1024).collect::<Vec<u32>>();
        let bytes = encoded(&old);
        let bad = StructuralDrawDelta {
            previous_count: old.len(),
            edits: vec![edit(30..31, 30..31), edit(20..21, 20..21)],
        };
        assert!(structural_draw_slot_patches(&bytes, &old, &bad).is_none());
        let mut edits = Vec::new();
        for n in 0..15 {
            let start = n * 40;
            edits.push(edit(start..start + 1, start..start + 1));
        }
        let mut changed = old.clone();
        for n in 0..15 {
            changed[n * 40] += 10_000;
        }
        assert!(structural_draw_slot_patches(&bytes, &changed, &StructuralDrawDelta {
            previous_count: old.len(), edits,
        }).is_none());
    }
}

#[cfg(test)]
mod command_visibility_tests {
    use super::{update_command_visibility, update_dirty_command_visibility};

    #[test]
    fn dirty_section_visibility_changes_only_the_targeted_batches() {
        let mut mask = vec![true; 65_536];
        let changed =
            update_dirty_command_visibility(&mut mask, &[15, 63_992], |index| Some(index != 15));
        assert_eq!(changed, Some(true));
        assert!(!mask[15]);
        assert!(mask[63_992]);
        assert!(mask[16]);
        assert_eq!(
            update_dirty_command_visibility(&mut mask, &[15], |_| Some(false)),
            Some(false),
        );
    }

    #[test]
    fn invalid_dirty_indices_do_not_commit_a_partial_mask() {
        let mut mask = vec![true; 8];
        assert_eq!(
            update_dirty_command_visibility(&mut mask, &[1, 99], |_| Some(false)),
            None
        );
        assert_eq!(mask, vec![true; 8]);
    }

    #[test]
    fn camera_motion_reuses_commands_until_batch_visibility_changes() {
        let mut mask = Vec::new();
        assert!(update_command_visibility(&mut mask, [true, false, true]));
        assert!(!update_command_visibility(&mut mask, [true, false, true]));
        assert_eq!(mask, [true, false, true]);
        assert!(update_command_visibility(&mut mask, [true, true, true]));
        assert_eq!(mask, [true, true, true]);
        assert!(!update_command_visibility(&mut mask, [true, true, true]));
    }

    #[test]
    fn mask_tracks_section_growth_and_removal() {
        let mut mask = vec![true, false, true];
        assert!(update_command_visibility(&mut mask, [true, false]));
        assert_eq!(mask, [true, false]);
        assert!(update_command_visibility(&mut mask, [true, false, false]));
        assert_eq!(mask, [true, false, false]);
        assert!(update_command_visibility(&mut mask, []));
        assert!(mask.is_empty());
        assert!(!update_command_visibility(&mut mask, []));
    }
}

#[cfg(test)]
mod animation_invalidation_tests {
    use super::*;

    #[test]
    fn empty_animation_clock_changes_do_not_rebuild_static_draws() {
        let first: Arc<[crate::TransformTrack]> = Arc::from([]);
        let second: Arc<[crate::TransformTrack]> = Arc::from([]);
        assert!(!animation_affects_draws(
            Some(&first),
            Some(Duration::ZERO),
            &second,
            Duration::from_secs(500),
        ));
        assert!(!animation_affects_draws(
            None,
            None,
            &first,
            Duration::from_secs(1),
        ));
    }

    #[test]
    fn active_tracks_invalidate_on_clip_time_or_snapshot_change() {
        let mut scene = Scene::new();
        let node = scene.insert(None, crate::Node::new()).unwrap();
        let active: Arc<[crate::TransformTrack]> = Arc::from([crate::TransformTrack::new(node)]);
        assert!(animation_affects_draws(None, None, &active, Duration::ZERO));
        assert!(!animation_affects_draws(
            Some(&active),
            Some(Duration::ZERO),
            &active,
            Duration::ZERO,
        ));
        assert!(animation_affects_draws(
            Some(&active),
            Some(Duration::ZERO),
            &active,
            Duration::from_millis(16),
        ));
        let different: Arc<[crate::TransformTrack]> = Arc::from([crate::TransformTrack::new(node)]);
        assert!(animation_affects_draws(
            Some(&active),
            Some(Duration::ZERO),
            &different,
            Duration::ZERO,
        ));
        let empty: Arc<[crate::TransformTrack]> = Arc::from([]);
        assert!(animation_affects_draws(
            Some(&active),
            Some(Duration::ZERO),
            &empty,
            Duration::ZERO,
        ));
    }
}
