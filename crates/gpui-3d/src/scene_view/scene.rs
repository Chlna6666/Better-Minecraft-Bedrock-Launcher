use super::{
    DRAW_PARAMS_STRIDE, FRAME_PARAMS_STRIDE, INSTANCE_STRIDE, LIGHT_STRIDE, SceneView,
    culling::{ARGUMENT_STRIDE, CullResources},
    gpu::{
        DrawBindingResources, MeshArena, MeshResources, PendingMeshUpload, TextureResources,
        buffer_memory_location, can_encode_voxel_mesh, create_draw_resource_set, create_mesh,
        create_storage_buffer, create_texture, destroy_mesh, destroy_texture, DrawRecordCache,
        encode_frame_params, encode_lights_into, grow_capacity,
    },
    projection::FrameProjection,
    renderer::trace_timings,
    resources::RendererResources,
    uploads,
};
use crate::{
    Aabb, AlphaMode, AnimationScratch, Camera, Mat4, MaterialId, MeshId, MeshPart, NodeId,
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
    collections::{HashMap, HashSet},
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
    scene: Option<Arc<Scene>>,
    active_assets: assets::AssetCounts,
    texture_table: Option<Arc<HashMap<TextureAssetId, Arc<crate::TextureAsset>>>>,
    animation: Option<Arc<[crate::TransformTrack]>>,
    animation_time: Option<Duration>,
    animation_scratch: AnimationScratch,
    camera: Option<Camera>,
    culling_bounds: Option<Aabb>,
    scene_transform: Option<Mat4>,
    projection: Option<FrameProjection>,
    prepared: PreparedScene,
    pub(super) command_revision: u64,
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
    arena: MeshArena,
    retired_meshes: Vec<MeshResources>,
    voxel_shader: bool,
    textures: HashMap<TextureAssetId, TextureResources>,
    seen_materials: HashSet<usize>,
    draw_resources: Vec<BindGroupId>,
    draw_resource_keys: Vec<DrawResourceKey>,
    /// Spare capacity for batch/resource lists, recycled across scene publications.
    spare_draw_resources: Vec<BindGroupId>,
    spare_draw_resource_keys: Vec<DrawResourceKey>,
    draw_addresses: Vec<DrawAddress>,
    reorder_positions: Vec<usize>,
    binding_cache: HashMap<DrawResourceKey, BindGroupId>,
    last_binding_report: Option<Instant>,
    sampler: Option<SamplerId>,
    // Resource-cache age only; frame-visible output samples the extension frame time.
    pub(super) last_used_at: Instant,
    pub(super) last_used_frame: u64,
}

const MESH_UPLOAD_BATCH_BYTES: usize = 16 * 1024 * 1024;

mod assets;
mod instance_cache;
mod stable_slots;
mod generated;
mod light_pass;
mod reorder;

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

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum DrawMeshKey {
    Mesh { id: MeshId, generation: u64 },
    VoxelPage(BufferId),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
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
                DrawMeshKey::VoxelPage(buffer) => voxel_pages.contains(&buffer),
            }
    }
}

impl SceneResources {
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
            scene: None,
            active_assets: assets::AssetCounts::default(),
            texture_table: None,
            animation: None,
            animation_time: None,
            animation_scratch: AnimationScratch::new(),
            camera: None,
            culling_bounds: None,
            scene_transform: None,
            projection: None,
            prepared: PreparedScene::default(),
            command_revision: 0,
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
            arena: MeshArena::default(),
            retired_meshes: Vec::new(),
            voxel_shader: false,
            textures: HashMap::new(),
            seen_materials: HashSet::new(),
            draw_resources: Vec::new(),
            draw_resource_keys: Vec::new(),
            spare_draw_resources: Vec::new(),
            spare_draw_resource_keys: Vec::new(),
            draw_addresses: Vec::new(),
            reorder_positions: Vec::new(),
            binding_cache: HashMap::new(),
            last_binding_report: None,
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
            && !sampler_changed
            && !voxel_shader_changed
        {
            return Ok(());
        }
        // Camera-only movement normally changes uniforms and GPU cull arguments, not
        // the native draw command list. Invalidate commands below only when their
        // bindings, draw batches or CPU-pruned visibility actually change.
        self.prepared.set_culling_bounds(scene_view.culling_bounds);
        let mut draw_list_changed = draw_state_changed || voxel_shader_changed;
        let mut reordered_blends = None;
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
                self.prepared
                    .update_nodes(&scene_view.scene, nodes.as_ref(), scene_view.scene_transform)
            });
            // A successful local update contains the complete new visible leaf set.
            // Unchanged leaves already have resident GPU meshes and textures.
            // Fall back to full scanning if a texture table or shader mode changes.
            let incremental_asset_sync = updated.is_some()
                && gpu_culling
                && !voxel_shader_changed
                && self
                    .texture_table
                    .as_ref()
                    .is_some_and(|old| Arc::ptr_eq(old, &scene_view.textures));
            if updated.is_some() {
                self.prepared
                    .update_camera(scene_view.camera, projection.aspect())?;
            } else {
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
                draw_list_changed = voxel_shader_changed
                    || if gpu_culling {
                        self.prepared
                            .blended_order()
                            .iter()
                            .enumerate()
                            .any(|(index, &previous)| index != previous)
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
                if draw_list_changed {
                    if gpu_culling && !voxel_shader_changed && !sampler_changed {
                        reordered_blends = Some(tracked_start);
                    }
                    refresh_culled_assets = reordered_blends.is_none();
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
        if shadow_created {
            reordered_blends = None;
        }
        if draw_list_changed && reordered_blends.is_none() {
            let batch_started = trace_timings().then(Instant::now);
            let _phase = gpui::gpu_owner_extension_phase(gpui::GpuOwnerExtensionPhase::BatchBuild);
            if gpu_culling {
                super::batching::group_voxel_batches_into(
                    &mut self.prepared.draws,
                    &self.meshes,
                    &mut self.batch_workspace,
                    &mut self.batches,
                );
            } else {
                self.batches.clear();
                self.batches.extend(self.prepared.draw_batches().map(|batch| DrawBatchRange {
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
        if reordered_blends.is_some() {
            // The CPU order is already changed. A failed binding remap must rebuild
            // draw state next frame instead of accepting partially reordered bindings.
            self.scene = None;
        }
        if gpu_culling
            && reordered_blends.is_none()
            && (draw_list_changed || self.culling.is_none())
        {
            if let Some(culling) = &mut self.culling {
                culling.update(
                    device,
                    &self.prepared.draws,
                    &self.batches,
                    Some(&self.meshes),
                )?;
            } else {
                self.culling = Some(CullResources::create(
                    device,
                    &self.prepared.draws,
                    &self.batches,
                    self.backend,
                    Some(&self.meshes),
                )?);
            }
        } else if !gpu_culling && let Some(previous) = self.culling.take() {
            previous.destroy(device)?;
        }
        if !shadow_enabled
            && (draw_state_changed || culling_bounds_changed)
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
                self.prepared.occluder_bounds(scene_view.culling_bounds),
                scene_view.camera.eye,
                shading.sun_direction,
                draw_state_changed || culling_bounds_changed,
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
            if let Some(start) = reordered_blends {
                self.sync_reordered_blends(start)?;
            } else {
                self.sync_buffers(
                    device,
                    renderer,
                    bind_group_layout,
                    sampler,
                    sampler_changed,
                    scene_view.voxel_shader,
                    assets_changed,
                )?;
            }
            if let Some(started) = buffer_started {
                buffer_ms = started.elapsed().as_secs_f64() * 1000.0;
            }
        } else if sampler_changed || shadow_created {
            self.sync_draw_resources(
                device,
                renderer,
                renderer.bind_group_layout()?,
                sampler,
                sampler_changed,
                scene_view.voxel_shader,
                false,
            )?;
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
                retirement_scan = true;
                let (active_meshes, active_textures) = self.active_assets.ids();
                self.retire_bindings(device, &active_meshes, &active_textures)?;
                if references_removed {
                    self.retire_textures(device, &active_textures)?;
                    self.retire_meshes(&active_meshes);
                }
            }
        }
        for mesh in self.retired_meshes.drain(..) {
            destroy_mesh(device, mesh, &mut self.arena)?;
        }
        if gpu_culling {
            if (draw_state_changed || culling_bounds_changed || visibility_changed || wire_changed)
                && let Some(culling) = &mut self.culling
            {
                culling.hiz.update(
                    device,
                    self.prepared.occluder_bounds(scene_view.culling_bounds),
                    scene_view.camera.eye,
                    scene_view.camera.target - scene_view.camera.eye,
                    draw_state_changed || culling_bounds_changed || wire_changed,
                )?;
            }
            if visibility_changed || draw_list_changed || culling_bounds_changed || wire_changed {
                self.culling
                    .as_mut()
                    .ok_or_else(|| anyhow!("GPU culling resources missing"))?
                    .dispatch(
                        device,
                        renderer
                            .cull_shader
                            .ok_or_else(|| anyhow!("GPU culling shader missing"))?,
                        self.frame_buffer,
                        scene_view.culling_bounds,
                        self.generated_arguments,
                    )?;
            }
        }
        drop(resource_phase);
        let command_visibility_changed = self.refresh_command_visibility();
        if draw_list_changed
            || sampler_changed
            || shadow_created
            || voxel_shader_changed
            || wire_changed
            || (shadow_enabled && frame_changed)
            || command_visibility_changed
        {
            self.command_revision = self.command_revision.wrapping_add(1);
        }
        self.scene = Some(scene_view.scene.clone());
        self.texture_table = Some(scene_view.textures.clone());
        self.animation = Some(scene_view.animation.clone());
        self.animation_time = Some(scene_view.animation_time);
        self.camera = Some(scene_view.camera);
        self.culling_bounds = scene_view.culling_bounds;
        self.scene_transform = Some(scene_view.scene_transform);
        self.projection = Some(projection);
        self.sampler = Some(sampler);
        self.voxel_shader = scene_view.voxel_shader;
        if let Some(started) = trace_started {
            let total_ms = started.elapsed().as_secs_f64() * 1000.0;
            if total_ms >= 16.0 {
                eprintln!(
                    "gpui3d prepare_detail draws={} assets_changed={} retirement_scan={} camera_ms={:.2} walk_ms={:.2} assets_ms={:.2} batch_ms={:.2} buffer_ms={:.2} total_ms={:.2}",
                    self.prepared.draws.len(),
                    assets_changed,
                    retirement_scan,
                    camera_ms,
                    walk_ms,
                    assets_ms,
                    batch_ms,
                    buffer_ms,
                    total_ms,
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
                    || prepared.draws.get(batch.first_instance).is_some_and(|draw| {
                        visible(draw.bounds) && prepared.is_draw_in_culling_bounds(draw)
                    })
            }),
        )
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
        let next_slots = if voxel_shader {
            Some(self.stable_slots.plan(&self.prepared.draws)?)
        } else {
            None
        };
        let physical_count = next_slots.as_ref().map_or(
            self.prepared.draws.len(),
            |(slots, _)| slots.high_water(),
        ).max(1);
        let draw_count = physical_count;
        let instance_count = physical_count;
        let light_count = self.prepared.lights.len().max(1);
        let next_draw_capacity = grow_capacity(self.draw_capacity, draw_count)?;
        let next_instance_capacity = grow_capacity(self.instance_capacity, instance_count)?;
        let next_light_capacity = grow_capacity(self.light_capacity, light_count)?;
        let next_slot_capacity = grow_capacity(
            self.draw_slot_capacity,
            self.prepared.draws.len().max(1),
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
        let encoded_instance_rows = if let Some((slots, mapping)) = next_slots.as_ref() {
            // Encode material snapshots directly into physical GPU slots. No
            // intermediate dense logical 80-byte table and no second reorder
            // copy are needed for the common streaming Section update.
            self.draw_record_cache.encode_stable(
                &self.prepared.draws,
                self.prepared.lights.len(),
                mapping,
                slots.high_water(),
                &self.draw_slot_keys,
                &self.uploaded_tables[0],
                &mut next_draw_keys,
                &mut draw_bytes,
                &mut self.scratch_packed_draws,
            )?;
            self.encode_stable_instances(
                mapping,
                slots.high_water(),
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
        if let Some((_, mapping)) = next_slots.as_ref() {
            for &slot in mapping {
                slot_bytes.extend_from_slice(&slot.to_le_bytes());
            }
        } else {
            for logical in 0..self.prepared.draws.len() {
                let index = u32::try_from(logical)
                    .context("GPUI 3D logical draw index exceeds u32")?;
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
            // becomes sparse. Keep at most 16 writes per table in one batch to
            // avoid trading bandwidth savings for excessive driver calls.
            (if i == 0 {
                // DrawParams have immutable material-snapshot identities.
                // Scan compact keys instead of comparing 80 bytes per draw.
                uploads::changed_ranges_for_keys(
                    if replace { &[] } else { &self.draw_slot_keys },
                    &next_draw_keys,
                    if replace { 0 } else { self.uploaded_tables[i].len() },
                    stride,
                    16,
                    4,
                )
            } else if i == 1 {
                // Instance rows already carry bit-exact GPU-visible keys.
                uploads::changed_ranges_for_keys(
                    if replace { &[] } else { &self.instance_slot_keys },
                    &next_instance_keys,
                    if replace { 0 } else { self.uploaded_tables[i].len() },
                    stride,
                    16,
                    4,
                )
            } else {
                uploads::changed_ranges(
                    if replace { &[] } else { &self.uploaded_tables[i] },
                    data,
                    stride,
                    16,
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
            if replace_slot_map { &[] } else { &self.uploaded_slot_map },
            &slot_bytes,
            4,
            16,
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
        let encoded_bytes = draw_bytes.len() + instance_bytes.len() + light_bytes.len() + slot_bytes.len();
        // A failed write or later binding update may leave existing buffers partially changed.
        let mut previous_tables = std::mem::take(&mut self.uploaded_tables);
        let write_result = if writes.is_empty() {
            Ok(())
        } else {
            device.write_buffer_batch(&writes).map(|_| ())
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
            sampler_changed || replace_draw || replace_instances || replace_lights || replace_slot_map,
            voxel_shader,
            defer_binding_cleanup,
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
        } else {
            // The standard scene shader uses dense indexing directly.
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
                "gpui3d buffer_detail draws={} instance_encoded={} bytes={} prepared_bytes={} writes={} encode_ms={:.2} write_ms={:.2} bind_ms={:.2} resized={}",
                draw_count,
                encoded_instance_rows,
                uploaded_bytes,
                encoded_bytes,
                write_count,
                encode_ms.unwrap_or_default(),
                write_ms.unwrap_or_default(),
                bind_ms.unwrap_or_default(),
                replace_draw || replace_instances || replace_lights,
            );
        }
        // Empty new tables are intentional sentinels used only when the
        // physical keys and table extents matched the committed snapshot.
        // Move the proven-good CPU storage after successful GPU updates rather
        // than memcpy'ing every resident row on a logical-only Section reorder.
        if voxel_shader && !replace_draw && draw_bytes.is_empty()
            && previous_tables[0].len() == physical_count * DRAW_PARAMS_STRIDE as usize
        {
            draw_bytes = std::mem::take(&mut previous_tables[0]);
        }
        if voxel_shader && !replace_instances && instance_bytes.is_empty()
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
        let mut pending = Vec::new();
        let mut pending_ids = HashSet::new();
        let mut pending_bytes = 0usize;
        // Device limits cannot change within the lifetime of this backend.
        // Query the virtual device only once per asset publication.
        let compute_caps = device.compute_capabilities();
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
            let creation = (|| {
                if id.1
                    && draw.mesh.gpu_voxels().is_some()
                    && compute_caps.voxel_execution_path()
                        == gfx_core::VoxelExecutionPath::GpuDriven
                {
                    if self.mesh_shader.is_none() {
                        self.mesh_shader = Some(super::generated::create_shaders(device, backend)?);
                    }
                    super::generated::create(
                        device,
                        &draw.mesh,
                        self.mesh_shader
                            .as_ref()
                            .ok_or_else(|| anyhow!("missing meshing shader"))?,
                    )
                    .map(|mesh| (mesh, None))
                } else {
                    create_mesh(device, &draw.mesh, backend, id.1, &mut self.arena)
                }
            })();
            let (mesh, upload) = match creation {
                Ok(created) => created,
                Err(error) => {
                    discard_pending_meshes(device, &mut pending, &mut self.arena)?;
                    return Err(error);
                }
            };
            pending_bytes = pending_bytes
                .saturating_add(upload.as_ref().map_or(0, |upload| upload.bytes.len()));
            pending.push((id, mesh, upload));
            if pending_bytes >= MESH_UPLOAD_BATCH_BYTES || pending.len() >= 256 {
                upload_pending_meshes(
                    device,
                    &mut pending,
                    &mut self.meshes,
                    &mut self.retired_meshes,
                    &mut self.arena,
                )?;
                pending_ids.clear();
                pending_bytes = 0;
            }
        }
        upload_pending_meshes(
            device,
            &mut pending,
            &mut self.meshes,
            &mut self.retired_meshes,
            &mut self.arena,
        )
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
                self.retired_meshes.push(mesh);
            }
        }
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
            .map(|(_, mesh)| mesh.vertices)
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
        for (key, binding) in stale {
            device.destroy_bind_group(binding)?;
            self.binding_cache.remove(&key);
        }
        Ok(())
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
    ) -> gpui::Result<()> {
        let mut next = std::mem::take(&mut self.spare_draw_resources);
        let mut next_keys = std::mem::take(&mut self.spare_draw_resource_keys);
        next.clear();
        next_keys.clear();
        next.reserve(self.batches.len());
        next_keys.reserve(self.batches.len());
        let mut created = HashMap::new();
        // Bind the allocated range so culling alone does not invalidate every bind group.
        let instance_count = self.instance_capacity;
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
            let mesh_id = draw.mesh.id();
            let compact_voxel = mesh_key(draw, voxel_shader).1;
            let generation = draw.mesh.generation();
            let mesh = self
                .meshes
                .get(&(mesh_id, compact_voxel))
                .ok_or_else(|| anyhow!("GPUI 3D mesh was not uploaded"))?;
            let albedo_id = draw.material.albedo_texture;
            let normal_id = draw.material.normal_texture;
            let occlusion_id = draw.material.occlusion_texture;
            let metallic_roughness_id = draw.material.metallic_roughness_texture;
            let key = DrawResourceKey {
                shadow: self.shadow.as_ref().map(|shadow| shadow.raster.depths),
                visible_instances: self
                    .culling
                    .as_ref()
                    .map(|culling| culling.visible_instances),
                mesh: if compact_voxel {
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
                }
                Err(error) => {
                    for bind_group in created.into_values() {
                        device.destroy_bind_group(bind_group)?;
                    }
                    return Err(error);
                }
            }
        }
        // Keep the previous Vec allocations as reusable scratch storage.
        std::mem::swap(&mut self.draw_resources, &mut next);
        std::mem::swap(&mut self.draw_resource_keys, &mut next_keys);
        self.spare_draw_resources = next;
        self.spare_draw_resource_keys = next_keys;
        if force {
            for bind_group in self.binding_cache.values().copied() {
                device.destroy_bind_group(bind_group)?;
            }
            self.binding_cache = created;
        } else {
            self.binding_cache.extend(created);
        }
        // Binding validation is already performed by retire_bindings after
        // asset changes. Do not perform the same full cache walk twice for
        // every streamed section. A forced rebind replaces the entire cache
        // with freshly created entries and also needs no second scan.
        if !defer_binding_cleanup && !force {
            let resident_voxel_pages = self
                .meshes
                .iter()
                .filter(|((_, compact), _)| *compact)
                .map(|(_, mesh)| mesh.vertices)
                .collect::<HashSet<_>>();
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
            for (key, binding) in stale {
                device.destroy_bind_group(binding)?;
                self.binding_cache.remove(&key);
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
                eprintln!(
                    "gpui3d bindings batches={} reused={} positional_reused={} created={} cached={} deferred_prune={} elapsed_ms={:.2} arena_pages={} arena_capacity_bytes={} arena_live_bytes={} arena_dead_bytes={}",
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
                );
                self.last_binding_report = Some(now);
            }
        }
        Ok(())
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
        steps.reserve(self.batches.len());
        let bounds_visible = self.prepared.bounds_visibility();
        for (batch_index, batch) in self.batches.iter().enumerate() {
            let index = batch.first_instance;
            let draw = self
                .prepared
                .draws
                .get(index)
                .ok_or_else(|| anyhow!("GPUI 3D draw batch has an invalid first draw"))?;
            let mesh = self
                .meshes
                .get(&mesh_key(draw, voxel_shader))
                .ok_or_else(|| anyhow!("GPUI 3D mesh was not uploaded"))?;
            if renderer.rejects_material(&draw.material)
                || (shadow.is_some() && draw.material.alpha_mode == AlphaMode::Blend)
            {
                continue;
            }
            // Reject single-instance batches before recording native commands, including
            // indirect draws whose GPU instance count would otherwise only become zero.
            // A multi-instance batch can contain visible instances after its first draw.
            // Shadow casters use a separate camera and must not use the main frustum.
            if shadow.is_none()
                && self.culling.is_some()
                && batch.instance_count == 1
                && (!bounds_visible(draw.bounds) || !self.prepared.is_draw_in_culling_bounds(draw))
            {
                continue;
            }
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
        if let Some(shadow) = self.shadow.take() {
            shadow.destroy(device)?;
        }
        if let Some(pass) = self.light_pass.take() {
            pass.destroy(device)?;
        }
        for (_, mesh) in self.meshes.drain() {
            destroy_mesh(device, mesh, &mut self.arena)?;
        }
        for mesh in self.retired_meshes.drain(..) {
            destroy_mesh(device, mesh, &mut self.arena)?;
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

fn discard_pending_meshes(
    device: &mut dyn ExtensionDevice,
    pending: &mut Vec<StagedMesh>,
    arena: &mut MeshArena,
) -> gpui::Result<()> {
    for (_, mesh, _) in pending.drain(..) {
        destroy_mesh(device, mesh, arena)?;
    }
    Ok(())
}

fn upload_pending_meshes(
    device: &mut dyn ExtensionDevice,
    pending: &mut Vec<StagedMesh>,
    meshes: &mut HashMap<(MeshId, bool), MeshResources>,
    retired_meshes: &mut Vec<MeshResources>,
    arena: &mut MeshArena,
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
    if let Err(error) = device.write_buffer_batch(&writes) {
        discard_pending_meshes(device, pending, arena)?;
        return Err(error.into());
    }
    for (id, mesh, _) in pending.drain(..) {
        if let Some(old) = meshes.insert(id, mesh) {
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
mod command_visibility_tests {
    use super::update_command_visibility;

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
            Some(&first), Some(Duration::ZERO), &second, Duration::from_secs(500),
        ));
        assert!(!animation_affects_draws(
            None, None, &first, Duration::from_secs(1),
        ));
    }

    #[test]
    fn active_tracks_invalidate_on_clip_time_or_snapshot_change() {
        let mut scene = Scene::new();
        let node = scene.insert(None, crate::Node::new()).unwrap();
        let active: Arc<[crate::TransformTrack]> =
            Arc::from([crate::TransformTrack::new(node)]);
        assert!(animation_affects_draws(
            None, None, &active, Duration::ZERO
        ));
        assert!(!animation_affects_draws(
            Some(&active), Some(Duration::ZERO), &active, Duration::ZERO,
        ));
        assert!(animation_affects_draws(
            Some(&active), Some(Duration::ZERO), &active, Duration::from_millis(16),
        ));
        let different: Arc<[crate::TransformTrack]> =
            Arc::from([crate::TransformTrack::new(node)]);
        assert!(animation_affects_draws(
            Some(&active), Some(Duration::ZERO), &different, Duration::ZERO,
        ));
        let empty: Arc<[crate::TransformTrack]> = Arc::from([]);
        assert!(animation_affects_draws(
            Some(&active), Some(Duration::ZERO), &empty, Duration::ZERO,
        ));
    }
}
