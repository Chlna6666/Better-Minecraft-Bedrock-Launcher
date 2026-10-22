//! GPU resources, render-graph passes, and caches for the offscreen scene renderer.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::collapsible_if,
    clippy::manual_midpoint,
    clippy::needless_pass_by_value,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "The renderer encodes fixed GPU layouts (byte counts, texel sizes) from validated u32/u64 values and keeps one function per render stage so the pass order stays reviewable."
)]

use super::{
    CapturedImage, DebugView, FrameStatistics, RenderError, RenderOptions, RendererStatistics,
};
use crate::scene_view::gpu::{
    encode_draw_slot, encode_instances_into, encode_lights_into, encode_mesh, write_matrix,
    write_vec4,
};
use crate::scene_view::{
    DRAW_PARAMS_STRIDE, DRAW_SLOT_STRIDE, INSTANCE_STRIDE, LIGHT_STRIDE, PACKED_VERTEX_STRIDE,
};
use crate::{
    Aabb, AlphaMode, Camera, EnvironmentMap, Mat4, Mesh, MeshId, PreparedLight, PreparedScene,
    TextureAsset, TextureAssetId, TextureColorSpace, Vec3,
};
use gfx_core::{
    AddressMode, BackendKind, BindGroupDescriptor, BindGroupEntry, BindGroupId,
    BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindGroupLayoutId, BindingResource,
    BindingType, BlendMode, BufferBinding, BufferDescriptor, BufferId, BufferUsage, ClearColor,
    ColorAttachmentDescriptor, CompareFunction, DepthAttachment, DepthAttachmentDescriptor,
    DepthState, Device, DrawCommand, DrawIndexedCommand, Extent2d, FilterMode, Format,
    IndexBufferBinding, IndexFormat, LoadOp, MemoryLocation, MemoryTrimLevel, Origin2d,
    PipelineLayoutDescriptor, PipelineLayoutId, PrimitiveTopology, RenderCommand,
    RenderPassCommands, RenderPassDescriptor, RenderPassId, RenderPipelineDescriptor,
    RenderPipelineId, SamplerBinding, SamplerDescriptor, SamplerId, ShaderModuleDescriptor,
    ShaderModuleId, ShaderStage, ShaderStages, SubmissionId, SubmissionStatus, TextureBinding,
    TextureDataLayout, TextureDescriptor, TextureDimension, TextureId, TextureTransferDevice,
    TextureUsage, TextureViewDescriptor, TextureViewId, TextureWrite, TextureWriteDescriptor,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

/// Byte stride of the scene/shadow/post frame uniform buffer.
///
/// The WGSL Frame layout declares matrices, effect vectors, the inverse view-projection used by the
/// sky pass, and environment parameters: 368 bytes total.
const FRAME_STRIDE: u64 = 368;
const SHADOW_VERTEX_BINDING: u32 = 0;
const SHADOW_INSTANCE_BINDING: u32 = 1;
const SHADOW_FRAME_BINDING: u32 = 2;
const SCENE_SHADOW_MAP_BINDING: u32 = 10;
const SCENE_METALLIC_ROUGHNESS_BINDING: u32 = 11;
const SCENE_ENVIRONMENT_BINDING: u32 = 12;
const SCENE_ENVIRONMENT_SAMPLER_BINDING: u32 = 13;
const SKY_ENVIRONMENT_BINDING: u32 = 0;
const SKY_SAMPLER_BINDING: u32 = 1;
const SKY_FRAME_BINDING: u32 = 2;
const POST_COLOR_BINDING: u32 = 0;
const POST_SAMPLER_BINDING: u32 = 1;
const POST_FRAME_BINDING: u32 = 2;
const POST_BLOOM_BINDING: u32 = 3;
const BLOOM_SCENE_BINDING: u32 = 0;
const BLOOM_SAMPLER_BINDING: u32 = 1;
const BLOOM_FRAME_BINDING: u32 = 2;
const DEFAULT_SHADOW_RESOLUTION: u32 = 2048;
const IDLE_MESH_FRAMES: u64 = 120;
const FRAME_BUFFER_COUNT: usize = 4;

const SHADOW_COLOR_FORMAT: Format = Format::Rgba8Unorm;
const SCENE_COLOR_FORMAT: Format = Format::Rgba16Float;
const POST_COLOR_FORMAT: Format = Format::Rgba8Unorm;
const BLOOM_COLOR_FORMAT: Format = Format::Rgba8Unorm;
const DEPTH_FORMAT: Format = Format::Depth32Float;

/// Untyped GPU buffer kept for reuse after a mesh is evicted.
struct PooledBuffer {
    buffer: BufferId,
    capacity: u64,
    usage: BufferUsage,
    location: MemoryLocation,
}

/// Bounded pool of freed mesh buffers, keyed by usage and fitted to the next mesh request.
///
/// Reusing exact or slightly larger buffers avoids repeated create/destroy cycles (and the
/// allocator/virtual-memory churn that fragments GPU heaps) while scene meshes are evicted and
/// rebuilt during budget trimming or scene switching.
#[derive(Default)]
struct MeshBufferPool {
    entries: Vec<PooledBuffer>,
    bytes: u64,
    reuses: u64,
}

const MESH_POOL_LIMIT_BYTES: u64 = 16 * 1024 * 1024;
const MESH_POOL_LIMIT_ENTRIES: usize = 64;

impl MeshBufferPool {
    fn take(
        &mut self,
        needed: u64,
        usage: BufferUsage,
        location: MemoryLocation,
    ) -> Option<(BufferId, u64)> {
        let needed = aligned_buffer_size(needed);
        let mut best: Option<(usize, u64)> = None;
        for (index, entry) in self.entries.iter().enumerate() {
            if entry.usage != usage
                || entry.location != location
                || entry.capacity < needed
                || entry.capacity > needed.saturating_mul(2)
            {
                continue;
            }
            let fits_better = best.is_none_or(|(_, capacity)| entry.capacity < capacity);
            if fits_better {
                best = Some((index, entry.capacity));
            }
        }
        let (index, capacity) = best?;
        let entry = self.entries.swap_remove(index);
        self.bytes = self.bytes.saturating_sub(entry.capacity);
        self.reuses = self.reuses.saturating_add(1);
        Some((entry.buffer, capacity))
    }

    /// Recycles a buffer, or destroys it when the pool is at capacity or the buffer is oversized.
    fn recycle<D: Device + ?Sized>(&mut self, device: &mut D, entry: PooledBuffer, reserved: u64) {
        let fits = self.entries.len() < MESH_POOL_LIMIT_ENTRIES
            && entry.capacity + self.bytes <= MESH_POOL_LIMIT_BYTES
            && entry.capacity + reserved <= MESH_POOL_LIMIT_BYTES;
        if fits {
            self.bytes = self.bytes.saturating_add(entry.capacity);
            self.entries.push(entry);
        } else {
            let _ = device.destroy_buffer(entry.buffer);
        }
    }

    fn clear<D: Device + ?Sized>(&mut self, device: &mut D) -> Result<u64, RenderError> {
        let released = self.bytes;
        let mut first_error = None;
        for entry in self.entries.drain(..) {
            if let Err(error) = device.destroy_buffer(entry.buffer) {
                first_error.get_or_insert(error);
            }
        }
        self.bytes = 0;
        match first_error {
            Some(error) => Err(error.into()),
            None => Ok(released),
        }
    }
}

/// Reusable CPU staging buffers for per-frame encoders.
///
/// Dirty frames rewrite draw, instance, light, and frame bytes; keeping their capacity alive avoids
/// four heap allocations (and the associated allocator churn) on every scene or camera change.
#[derive(Default)]
struct StagingBuffers {
    draw: Vec<u8>,
    instance: Vec<u8>,
    light: Vec<u8>,
    frame: Vec<u8>,
}

impl StagingBuffers {
    fn cpu_bytes(&self) -> u64 {
        (self.draw.capacity()
            + self.instance.capacity()
            + self.light.capacity()
            + self.frame.capacity()) as u64
    }

    fn shrink(&mut self) {
        self.draw.shrink_to_fit();
        self.instance.shrink_to_fit();
        self.light.shrink_to_fit();
        self.frame.shrink_to_fit();
    }
}

/// Everything the renderer owns on one GPU device.
pub(crate) struct GpuState {
    backend: BackendKind,
    width: u32,
    height: u32,
    pipelines: Option<Pipelines>,
    targets: Option<Targets>,
    frame_buffers: FrameBuffers,
    render_scale: f32,
    staging: StagingBuffers,
    mesh_pool: MeshBufferPool,
    meshes: HashMap<(MeshId, u64), MeshResources>,
    textures: HashMap<TextureAssetId, TextureResources>,
    fallback_texture: Option<TextureResources>,
    batch_sets: HashMap<BatchKey, BatchSets>,
    used_batch_keys: Vec<BatchKey>,
    post_set: Option<BindGroupId>,
    bloom_set: Option<BindGroupId>,
    sky_set: Option<BindGroupId>,
    environment: Option<(TextureAssetId, TextureResources)>,
    resource_epoch: u64,
    frame_index: u64,
    shadow_resolution: u32,
    memory_budget: Option<u64>,
    peak_resident_bytes: u64,
    upload_bytes_total: u64,
    mesh_cache_hits: u64,
    mesh_cache_misses: u64,
    texture_cache_hits: u64,
    texture_cache_misses: u64,
    evictions: u64,
    /// True when a submitted frame has not been observed through a readback yet.
    ///
    /// The compatibility submission API does not expose a completion handle, so the renderer
    /// drains the previous frame with the post-pass readback before it overwrites buffers or
    /// destroys GPU resources. Callers that always capture do not pay this extra wait.
    submission_unobserved: bool,
    /// Deferred completion handle for the most recent offscreen submission, when the backend
    /// supports it. Polling or waiting consumes the handle.
    pending_submission: Option<SubmissionId>,
}

impl GpuState {
    pub(crate) fn new<D: Device + ?Sized>(
        device: &mut D,
        width: u32,
        height: u32,
    ) -> Result<Self, RenderError> {
        let backend = D::BACKEND_KIND;
        let pipelines = Pipelines::new(device, backend, width, height)?;
        let targets = match Targets::new(device, width, height, DEFAULT_SHADOW_RESOLUTION, 1.0) {
            Ok(targets) => targets,
            Err(error) => {
                pipelines.destroy(device)?;
                return Err(error);
            }
        };
        let mut frame_buffers = match FrameBuffers::new(device, backend) {
            Ok(buffers) => buffers,
            Err(error) => {
                targets.destroy(device)?;
                pipelines.destroy(device)?;
                return Err(error);
            }
        };
        let fallback_texture = match create_fallback_texture(device) {
            Ok(texture) => texture,
            Err(error) => {
                frame_buffers.destroy(device)?;
                targets.destroy(device)?;
                pipelines.destroy(device)?;
                return Err(error);
            }
        };
        Ok(Self {
            backend,
            width,
            height,
            pipelines: Some(pipelines),
            targets: Some(targets),
            frame_buffers,
            render_scale: 1.0,
            staging: StagingBuffers::default(),
            mesh_pool: MeshBufferPool::default(),
            meshes: HashMap::new(),
            textures: HashMap::new(),
            fallback_texture: Some(fallback_texture),
            batch_sets: HashMap::new(),
            used_batch_keys: Vec::new(),
            post_set: None,
            bloom_set: None,
            sky_set: None,
            environment: None,
            resource_epoch: 1,
            frame_index: 0,
            shadow_resolution: DEFAULT_SHADOW_RESOLUTION,
            memory_budget: None,
            peak_resident_bytes: 0,
            upload_bytes_total: 0,
            mesh_cache_hits: 0,
            mesh_cache_misses: 0,
            texture_cache_hits: 0,
            texture_cache_misses: 0,
            evictions: 0,
            submission_unobserved: false,
            pending_submission: None,
        })
    }

    pub(crate) fn backend(&self) -> BackendKind {
        self.backend
    }

    pub(crate) fn resize<D: Device + TextureTransferDevice + ?Sized>(
        &mut self,
        device: &mut D,
        width: u32,
        height: u32,
    ) -> Result<(), RenderError> {
        self.ensure_idle(device)?;
        if let Some(pipelines) = self.pipelines.take() {
            pipelines.destroy(device)?;
        }
        if let Some(targets) = self.targets.as_mut() {
            targets.resize_scene(device, width, height, self.render_scale)?;
        }
        self.pipelines = Some(Pipelines::new(device, self.backend, width, height)?);
        self.width = width;
        self.height = height;
        self.invalidate_sets(device)?;
        self.resource_epoch = self.resource_epoch.wrapping_add(1);
        Ok(())
    }

    pub(crate) fn set_render_scale<D: Device + TextureTransferDevice + ?Sized>(
        &mut self,
        device: &mut D,
        render_scale: f32,
    ) -> Result<(), RenderError> {
        if !render_scale.is_finite() || !(0.25..=2.0).contains(&render_scale) {
            return Err(RenderError::InvalidSize(format!(
                "render scale {render_scale} outside 0.25..=2.0"
            )));
        }
        if (render_scale - self.render_scale).abs() < 1e-4 {
            return Ok(());
        }
        self.ensure_idle(device)?;
        if let Some(targets) = self.targets.as_mut() {
            targets.resize_scene(device, self.width, self.height, render_scale)?;
        }
        self.render_scale = render_scale;
        self.invalidate_sets(device)?;
        self.resource_epoch = self.resource_epoch.wrapping_add(1);
        Ok(())
    }

    pub(crate) fn render_scale(&self) -> f32 {
        self.render_scale
    }

    fn set_shadow_resolution<D: Device + TextureTransferDevice + ?Sized>(
        &mut self,
        device: &mut D,
        resolution: u32,
    ) -> Result<(), RenderError> {
        if resolution == self.shadow_resolution {
            return Ok(());
        }
        self.ensure_idle(device)?;
        if let Some(targets) = self.targets.as_mut() {
            targets.resize_shadow(device, resolution)?;
        }
        self.shadow_resolution = resolution;
        self.invalidate_sets(device)?;
        self.resource_epoch = self.resource_epoch.wrapping_add(1);
        Ok(())
    }

    fn invalidate_sets<D: Device + ?Sized>(&mut self, device: &mut D) -> Result<(), RenderError> {
        let keys = self.batch_sets.keys().copied().collect::<Vec<_>>();
        for key in keys {
            if let Some(sets) = self.batch_sets.remove(&key) {
                destroy_batch_sets(device, sets)?;
            }
        }
        if let Some(set) = self.sky_set.take() {
            device.destroy_bind_group(set)?;
        }
        if let Some(set) = self.bloom_set.take() {
            device.destroy_bind_group(set)?;
        }
        if let Some(set) = self.post_set.take() {
            device.destroy_bind_group(set)?;
        }
        Ok(())
    }

    /// Renders one frame into the offscreen targets.
    pub(crate) fn render<D: Device + TextureTransferDevice + ?Sized>(
        &mut self,
        device: &mut D,
        prepared: &PreparedScene,
        textures: &HashMap<TextureAssetId, Arc<TextureAsset>>,
        camera: Camera,
        options: &RenderOptions,
        target: (u32, u32),
        started: Instant,
        frame_data_dirty: bool,
        environment: Option<&Arc<EnvironmentMap>>,
    ) -> Result<FrameStatistics, RenderError> {
        self.ensure_idle(device)?;
        self.frame_index = self.frame_index.wrapping_add(1);
        self.ensure_environment(device, environment)?;
        let mut statistics = FrameStatistics::default();
        let render_target = self.targets.as_ref().map_or(target, Targets::render_target);

        let shadow_resolution = options.shadows.resolution.clamp(1, 8192);
        if options.shadows.enabled && shadow_resolution != self.shadow_resolution {
            self.set_shadow_resolution(device, shadow_resolution)?;
        }

        for draw in &prepared.draws {
            self.ensure_mesh(device, &draw.mesh, &mut statistics)?;
            for texture_id in [
                draw.material.albedo_texture,
                draw.material.normal_texture,
                draw.material.occlusion_texture,
                draw.material.metallic_roughness_texture,
            ]
            .into_iter()
            .flatten()
            {
                self.ensure_texture(device, texture_id, textures, &mut statistics)?;
            }
        }

        let scene_bounds = prepared_bounds(prepared);
        let directional = prepared.lights.iter().find_map(|light| match *light {
            PreparedLight::Directional {
                direction,
                color,
                intensity,
            } => Some((direction, color, intensity)),
            _ => None,
        });
        let mut light_view_projection = Mat4::IDENTITY;
        let mut shadow_enabled = false;
        if options.shadows.enabled {
            if let (Some(bounds), Some((direction, _, _))) = (scene_bounds, directional) {
                if let Some(view_projection) = fit_directional_shadow(bounds, direction) {
                    light_view_projection = view_projection;
                    shadow_enabled = true;
                }
            }
        }
        let shadow_texel = 1.0 / self.shadow_resolution as f32;

        let object_id_pass = options.debug_view == DebugView::ObjectId;
        let mut effective_options = *options;
        if object_id_pass {
            // Object ids must survive the post pass byte-exactly, and the background must decode
            // to id zero instead of the display clear color.
            effective_options.post_process = false;
        }
        let clear_color = if object_id_pass {
            [0.0, 0.0, 0.0, 1.0]
        } else {
            options.clear_color
        };
        let encoded_environment = match environment {
            Some(map) if self.environment.is_some() => [
                map.intensity(),
                map.rotation(),
                map.texture().mip_level_count() as f32,
                1.0,
            ],
            _ => [0.0, 0.0, 0.0, 0.0],
        };
        self.staging.frame.clear();
        encode_frame_into(
            prepared.view_projection,
            light_view_projection,
            camera,
            directional,
            &effective_options,
            target,
            render_target,
            shadow_enabled,
            shadow_texel,
            encoded_environment,
            &mut self.staging.frame,
        );
        if frame_data_dirty {
            self.staging.draw.clear();
            encode_batch_draws_into(prepared, &mut self.staging.draw)?;
            self.staging.instance.clear();
            encode_instances_into(prepared, &mut self.staging.instance).map_err(prepare_error)?;
            self.staging.light.clear();
            encode_lights_into(&prepared.lights, &mut self.staging.light);
        }
        let staging = &self.staging;
        let drawn = frame_data_dirty.then_some(staging.draw.as_slice());
        let instanced = frame_data_dirty.then_some(staging.instance.as_slice());
        let lighted = frame_data_dirty.then_some(staging.light.as_slice());
        let upload = self
            .frame_buffers
            .sync(device, drawn, instanced, lighted, &staging.frame)?;
        statistics.upload_bytes = statistics.upload_bytes.saturating_add(upload.bytes);
        if upload.reallocated {
            self.invalidate_sets(device)?;
            self.resource_epoch = self.resource_epoch.wrapping_add(1);
        }
        self.upload_bytes_total = self
            .upload_bytes_total
            .saturating_add(statistics.upload_bytes);
        self.enforce_memory_budget(device)?;
        self.peak_resident_bytes = self.peak_resident_bytes.max(self.resident_bytes());

        self.used_batch_keys.clear();
        let mut shadow_steps = Vec::new();
        let mut scene_steps = Vec::new();
        for (batch_slot, batch) in prepared.draw_batches().enumerate() {
            let batch_steps = self.batch_steps(
                device,
                batch,
                u32::try_from(batch_slot)
                    .map_err(|_| RenderError::Preparation("draw batch count exceeds u32".into()))?,
            )?;
            statistics.draw_calls += 1;
            statistics.triangles += batch_steps.triangles;
            statistics.instances += batch_steps.instances;
            if shadow_enabled && batch_steps.alpha_mode != AlphaMode::Blend {
                shadow_steps.push(batch_steps.shadow);
                statistics.shadow_draw_calls += 1;
            }
            scene_steps.push(batch_steps.scene);
        }
        if environment.is_some() && options.debug_view == DebugView::Shaded {
            if let Some(sky_set) = self.ensure_sky_set(device)? {
                let sky_pipeline = self.pipelines()?.sky_pipeline;
                scene_steps.push(RenderCommand::Draw(DrawCommand {
                    pipeline: sky_pipeline,
                    bind_groups: gfx_core::bind_group_list([sky_set]),
                    vertex_count: 3,
                    first_vertex: 0,
                    instance_count: 1,
                    first_instance: 0,
                    scissor: None,
                }));
            }
        }
        self.release_unused_batch_sets(device)?;

        let post_set = self.ensure_post_set(device)?;
        let bloom_enabled = options.post_process
            && options.bloom.enabled
            && options.bloom.strength > 0.0
            && options.debug_view == DebugView::Shaded;
        let bloom_set = if bloom_enabled {
            Some(self.ensure_bloom_set(device)?)
        } else {
            None
        };
        let post_steps = [RenderCommand::Draw(DrawCommand {
            pipeline: self.pipelines()?.post_pipeline,
            bind_groups: gfx_core::bind_group_list([post_set]),
            vertex_count: 3,
            first_vertex: 0,
            instance_count: 1,
            first_instance: 0,
            scissor: None,
        })];
        let mut bloom_steps = Vec::with_capacity(1);
        if let Some(bloom_set) = bloom_set {
            bloom_steps.push(RenderCommand::Draw(DrawCommand {
                pipeline: self.pipelines()?.bloom_pipeline,
                bind_groups: gfx_core::bind_group_list([bloom_set]),
                vertex_count: 3,
                first_vertex: 0,
                instance_count: 1,
                first_instance: 0,
                scissor: None,
            }));
        }

        let targets = self.targets.as_ref().ok_or(RenderError::Destroyed)?;
        let pipelines = self.pipelines()?;
        let shadow_pass = RenderPassCommands {
            texture_view: targets.shadow_color_view,
            render_pass: pipelines.shadow_pass,
            commands: &shadow_steps,
            color_load_op: LoadOp::Clear(ClearColor {
                red: 1.0,
                green: 1.0,
                blue: 1.0,
                alpha: 1.0,
            }),
            clear_region: None,
            depth_attachment: Some(DepthAttachment {
                target: targets.shadow_depth_view,
                depth_load_op: LoadOp::Clear(1.0),
            }),
            viewport: None,
        };
        let scene_pass = RenderPassCommands {
            texture_view: targets.scene_color_view,
            render_pass: pipelines.scene_pass,
            commands: &scene_steps,
            color_load_op: LoadOp::Clear(ClearColor {
                red: clear_color[0],
                green: clear_color[1],
                blue: clear_color[2],
                alpha: clear_color[3],
            }),
            clear_region: None,
            depth_attachment: Some(DepthAttachment {
                target: targets.scene_depth_view,
                depth_load_op: LoadOp::Clear(1.0),
            }),
            viewport: None,
        };
        let bloom_pass = RenderPassCommands {
            texture_view: targets.bloom_color_view,
            render_pass: pipelines.bloom_pass,
            commands: &bloom_steps,
            color_load_op: LoadOp::Clear(ClearColor::default()),
            clear_region: None,
            depth_attachment: None,
            viewport: None,
        };
        let post_pass = RenderPassCommands {
            texture_view: targets.post_color_view,
            render_pass: pipelines.post_pass,
            commands: &post_steps,
            color_load_op: LoadOp::Clear(ClearColor::default()),
            clear_region: None,
            depth_attachment: None,
            viewport: None,
        };

        let mut passes = Vec::with_capacity(4);
        passes.push(shadow_pass);
        passes.push(scene_pass);
        if bloom_enabled {
            passes.push(bloom_pass);
        }
        passes.push(post_pass);
        let submit_started = Instant::now();
        self.pending_submission = device.render_to_textures(&passes)?;
        statistics.submit_nanos = submit_started.elapsed().as_nanos() as u64;
        statistics.cpu_prepare_nanos = started.elapsed().as_nanos() as u64;
        self.submission_unobserved = true;
        Ok(statistics)
    }

    /// Reads the post-processing target back as display-referred RGBA8 pixels.
    pub(crate) fn capture<D: Device + TextureTransferDevice + ?Sized>(
        &mut self,
        device: &mut D,
    ) -> Result<CapturedImage, RenderError> {
        let targets = self.targets.as_ref().ok_or(RenderError::Destroyed)?;
        if let Some(submission) = self.pending_submission.take() {
            device.wait_submission(submission)?;
        }
        let readback = device.read_texture(targets.post_color)?;
        let expected_row = self.width as usize * POST_COLOR_FORMAT.bytes_per_pixel() as usize;
        if readback.size != Extent2d::new(self.width, self.height)?
            || readback.bytes_per_row as usize != expected_row
            || readback.bytes.len() != expected_row * self.height as usize
        {
            return Err(RenderError::Readback(format!(
                "backend returned {}x{} rows of {} bytes",
                readback.size.width(),
                readback.size.height(),
                readback.bytes_per_row
            )));
        }
        self.submission_unobserved = false;
        Ok(CapturedImage {
            width: self.width,
            height: self.height,
            rgba: readback.bytes,
        })
    }

    /// Releases cached assets according to a trim level.
    pub(crate) fn trim<D: Device + TextureTransferDevice + ?Sized>(
        &mut self,
        device: &mut D,
        level: MemoryTrimLevel,
    ) -> Result<(), RenderError> {
        self.ensure_idle(device)?;
        let mut released_bytes = 0_u64;
        let mut evicted_assets = 0_u64;
        let release_assets = match level {
            MemoryTrimLevel::Light => false,
            MemoryTrimLevel::Moderate => {
                let idle_before = self.frame_index.saturating_sub(IDLE_MESH_FRAMES);
                let stale_meshes = self
                    .meshes
                    .iter()
                    .filter_map(|(key, mesh)| (mesh.last_used_frame < idle_before).then_some(*key))
                    .collect::<Vec<_>>();
                for key in stale_meshes {
                    if let Some(mesh) = self.meshes.remove(&key) {
                        released_bytes += mesh.bytes;
                        evicted_assets += 1;
                        destroy_mesh(device, mesh, &mut self.mesh_pool);
                    }
                }
                let stale_textures = self
                    .textures
                    .iter()
                    .filter_map(|(key, texture)| {
                        (texture.last_used_frame < idle_before).then_some(*key)
                    })
                    .collect::<Vec<_>>();
                for key in stale_textures {
                    if let Some(texture) = self.textures.remove(&key) {
                        released_bytes += texture.bytes;
                        evicted_assets += 1;
                        destroy_texture(device, texture)?;
                    }
                }
                true
            }
            MemoryTrimLevel::Aggressive => {
                let mesh_keys = self.meshes.keys().copied().collect::<Vec<_>>();
                for key in mesh_keys {
                    if let Some(mesh) = self.meshes.remove(&key) {
                        released_bytes += mesh.bytes;
                        evicted_assets += 1;
                        destroy_mesh(device, mesh, &mut self.mesh_pool);
                    }
                }
                let texture_keys = self.textures.keys().copied().collect::<Vec<_>>();
                for key in texture_keys {
                    if let Some(texture) = self.textures.remove(&key) {
                        released_bytes += texture.bytes;
                        evicted_assets += 1;
                        destroy_texture(device, texture)?;
                    }
                }
                true
            }
        };
        let _ = released_bytes;
        if release_assets {
            self.evictions = self.evictions.saturating_add(evicted_assets);
            self.invalidate_sets(device)?;
            self.resource_epoch = self.resource_epoch.wrapping_add(1);
            if matches!(level, MemoryTrimLevel::Aggressive) {
                if self.frame_buffers.shrink(device)? {
                    self.resource_epoch = self.resource_epoch.wrapping_add(1);
                }
                self.staging.shrink();
                let _ = self.mesh_pool.clear(device)?;
                if let Some((_, texture)) = self.environment.take() {
                    self.evictions = self.evictions.saturating_add(1);
                    destroy_texture(device, texture)?;
                }
                self.sky_set = None;
            }
        }
        Ok(())
    }
    pub(crate) fn statistics(&self) -> RendererStatistics {
        let resident_buffer_bytes = self.frame_buffers.resident_bytes() + self.mesh_pool.bytes;
        let target_bytes = self.targets.as_ref().map_or(0, Targets::resident_bytes);
        let cached_mesh_bytes = self.meshes.values().map(|mesh| mesh.bytes).sum::<u64>();
        let cached_texture_bytes = self
            .fallback_texture
            .as_ref()
            .map_or(0, |texture| texture.bytes)
            + self
                .environment
                .as_ref()
                .map_or(0, |(_, texture)| texture.bytes)
            + self
                .textures
                .values()
                .map(|texture| texture.bytes)
                .sum::<u64>();
        RendererStatistics {
            live_meshes: self.meshes.len(),
            live_textures: self.textures.len() + usize::from(self.environment.is_some()),
            bind_group_count: self.batch_sets.len()
                + usize::from(self.post_set.is_some())
                + usize::from(self.bloom_set.is_some())
                + usize::from(self.sky_set.is_some()),
            resident_buffer_bytes,
            resident_texture_bytes: target_bytes + cached_texture_bytes,
            draw_capacity: self.frame_buffers.draw_capacity(),
            instance_capacity: self.frame_buffers.instance_capacity(),
            light_capacity: self.frame_buffers.light_capacity(),
            target_width: self.width,
            target_height: self.height,
            render_target_width: self
                .targets
                .as_ref()
                .map_or(self.width, |targets| targets.render_width),
            render_target_height: self
                .targets
                .as_ref()
                .map_or(self.height, |targets| targets.render_height),
            shadow_resolution: self.shadow_resolution,
            target_bytes,
            cached_mesh_bytes,
            cached_texture_bytes,
            peak_resident_bytes: self.peak_resident_bytes,
            upload_bytes_total: self.upload_bytes_total,
            mesh_cache_hits: self.mesh_cache_hits,
            mesh_cache_misses: self.mesh_cache_misses,
            texture_cache_hits: self.texture_cache_hits,
            texture_cache_misses: self.texture_cache_misses,
            evictions: self.evictions,
            tracker_cpu_bytes: self.frame_buffers.cpu_tracker_bytes(),
            memory_budget_bytes: self.memory_budget,
            cpu_staging_bytes: self.staging.cpu_bytes(),
            buffer_allocation_count: FRAME_BUFFER_COUNT
                + self
                    .meshes
                    .values()
                    .map(|mesh| 2 + usize::from(mesh.tangents.is_some()))
                    .sum::<usize>(),
            buffer_slack_bytes: self
                .frame_buffers
                .capacity_bytes()
                .saturating_sub(self.frame_buffers.used_bytes())
                + self
                    .meshes
                    .values()
                    .map(|mesh| mesh.bytes.saturating_sub(mesh.requested_bytes))
                    .sum::<u64>(),
            pooled_buffer_bytes: self.mesh_pool.bytes,
            pooled_buffer_count: self.mesh_pool.entries.len(),
            buffer_reuses: self.mesh_pool.reuses,
        }
    }

    /// Resident GPU bytes owned by this renderer, excluding CPU-side bookkeeping.
    fn resident_bytes(&self) -> u64 {
        self.frame_buffers.resident_bytes()
            + self.mesh_pool.bytes
            + self.targets.as_ref().map_or(0, Targets::resident_bytes)
            + self
                .fallback_texture
                .as_ref()
                .map_or(0, |texture| texture.bytes)
            + self
                .environment
                .as_ref()
                .map_or(0, |(_, texture)| texture.bytes)
            + self.meshes.values().map(|mesh| mesh.bytes).sum::<u64>()
            + self
                .textures
                .values()
                .map(|texture| texture.bytes)
                .sum::<u64>()
    }

    pub(crate) fn set_memory_budget(&mut self, budget_bytes: Option<u64>) {
        self.memory_budget = budget_bytes.map(|bytes| bytes.max(1));
    }

    pub(crate) fn memory_budget(&self) -> Option<u64> {
        self.memory_budget
    }

    /// Evicts least-recently-used assets that the current frame did not reference until the
    /// resident total fits the configured soft budget. Returns the released GPU bytes.
    pub(crate) fn enforce_memory_budget<D: Device + ?Sized>(
        &mut self,
        device: &mut D,
    ) -> Result<u64, RenderError> {
        let Some(budget) = self.memory_budget else {
            return Ok(0);
        };
        let mut released = 0_u64;
        let mut evicted = false;
        loop {
            // The reuse pool is bounded separately from the asset budget: evicting a mesh only
            // moves bytes into the pool, so counting it here would make budget pressure churn
            // allocations instead of shrinking the live working set.
            if self.budget_resident_bytes() <= budget {
                break;
            }
            let mesh_candidate = self
                .meshes
                .iter()
                .filter(|(_, mesh)| mesh.last_used_frame < self.frame_index)
                .min_by_key(|(_, mesh)| mesh.last_used_frame)
                .map(|(key, mesh)| (*key, mesh.bytes));
            let texture_candidate = self
                .textures
                .iter()
                .filter(|(_, texture)| texture.last_used_frame < self.frame_index)
                .min_by_key(|(_, texture)| texture.last_used_frame)
                .map(|(key, texture)| (*key, texture.bytes));
            let evict_texture = match (mesh_candidate, texture_candidate) {
                (Some((_, mesh_bytes)), Some((_, texture_bytes))) => texture_bytes > mesh_bytes,
                (Some(_), None) => false,
                (None, Some(_)) => true,
                (None, None) => break,
            };
            if evict_texture {
                let Some((key, bytes)) = texture_candidate else {
                    break;
                };
                if let Some(texture) = self.textures.remove(&key) {
                    destroy_texture(device, texture)?;
                    released += bytes;
                    self.evictions += 1;
                    evicted = true;
                }
            } else {
                let Some((key, bytes)) = mesh_candidate else {
                    break;
                };
                if let Some(mesh) = self.meshes.remove(&key) {
                    destroy_mesh(device, mesh, &mut self.mesh_pool);
                    released += bytes;
                    self.evictions += 1;
                    evicted = true;
                }
            }
        }
        if evicted {
            self.invalidate_sets(device)?;
            self.resource_epoch = self.resource_epoch.wrapping_add(1);
        }
        // Bound the pool independently and release it once no live asset can reuse it.
        if self.mesh_pool.bytes > MESH_POOL_LIMIT_BYTES / 2
            || (self.meshes.is_empty() && self.textures.is_empty() && self.mesh_pool.bytes > 0)
        {
            released += self.mesh_pool.clear(device)?;
        }
        Ok(released)
    }

    /// Resident bytes counted against the asset budget (excludes the separately-bounded pool).
    fn budget_resident_bytes(&self) -> u64 {
        self.resident_bytes().saturating_sub(self.mesh_pool.bytes)
    }

    pub(crate) fn destroy<D: Device + TextureTransferDevice + ?Sized>(
        &mut self,
        device: &mut D,
    ) -> Result<(), RenderError> {
        self.ensure_idle(device)?;
        let mut first_error = None;
        if let Err(error) = self.invalidate_sets(device) {
            first_error = Some(error);
        }
        let mesh_keys = self.meshes.keys().copied().collect::<Vec<_>>();
        for key in mesh_keys {
            if let Some(mesh) = self.meshes.remove(&key) {
                destroy_mesh(device, mesh, &mut self.mesh_pool);
            }
        }
        let texture_keys = self.textures.keys().copied().collect::<Vec<_>>();
        for key in texture_keys {
            if let Some(texture) = self.textures.remove(&key) {
                if let Err(error) = destroy_texture(device, texture) {
                    first_error.get_or_insert(error);
                }
            }
        }
        if let Some(texture) = self.fallback_texture.take() {
            if let Err(error) = destroy_texture(device, texture) {
                first_error.get_or_insert(error);
            }
        }
        if let Some((_, texture)) = self.environment.take() {
            if let Err(error) = destroy_texture(device, texture) {
                first_error.get_or_insert(error);
            }
        }
        if let Err(error) = self.frame_buffers.destroy(device) {
            first_error.get_or_insert(error);
        }
        if let Some(targets) = self.targets.take() {
            if let Err(error) = targets.destroy(device) {
                first_error.get_or_insert(error);
            }
        }
        if let Some(pipelines) = self.pipelines.take() {
            if let Err(error) = pipelines.destroy(device) {
                first_error.get_or_insert(error);
            }
        }
        if let Err(error) = self.mesh_pool.clear(device) {
            first_error.get_or_insert(error);
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// Waits for the most recent submission by reading the post target back when the caller has
    /// not captured it yet. This is the renderer's only completion primitive because the Nova
    /// compatibility submission API exposes no fence handle.
    fn ensure_idle<D: Device + TextureTransferDevice + ?Sized>(
        &mut self,
        device: &mut D,
    ) -> Result<(), RenderError> {
        if !self.submission_unobserved {
            return Ok(());
        }
        self.submission_unobserved = false;
        if let Some(submission) = self.pending_submission.take() {
            device.wait_submission(submission)?;
            return Ok(());
        }
        let Some(targets) = self.targets.as_ref() else {
            return Ok(());
        };
        device.read_texture(targets.post_color)?;
        Ok(())
    }

    /// Polls the deferred offscreen submission without blocking.
    ///
    /// Returns `Ok(None)` when the backend has no deferred completion handle (or no frame is
    /// pending), `Ok(Some(false))` while the GPU is still running, and `Ok(Some(true))` once the
    /// frame is complete. Capture and maintenance operations wait for the same handle, so polling
    /// is an optimization, not a correctness requirement.
    pub(crate) fn poll_pending_submission<D: Device + ?Sized>(
        &mut self,
        device: &mut D,
    ) -> Result<Option<bool>, RenderError> {
        let Some(submission) = self.pending_submission else {
            return Ok(None);
        };
        match device.poll_submission(submission)? {
            SubmissionStatus::Pending => Ok(Some(false)),
            SubmissionStatus::Complete => {
                self.pending_submission = None;
                Ok(Some(true))
            }
            SubmissionStatus::Failed(message) => {
                self.pending_submission = None;
                Err(RenderError::Gpu(gfx_core::Error::Backend(format!(
                    "offscreen submission failed: {message}"
                ))))
            }
        }
    }

    fn pipelines(&self) -> Result<&Pipelines, RenderError> {
        self.pipelines.as_ref().ok_or(RenderError::Destroyed)
    }

    fn ensure_mesh<D: Device + ?Sized>(
        &mut self,
        device: &mut D,
        mesh: &Mesh,
        statistics: &mut FrameStatistics,
    ) -> Result<(), RenderError> {
        let key = (mesh.id(), mesh.generation());
        if let Some(resources) = self.meshes.get_mut(&key) {
            resources.last_used_frame = self.frame_index;
            self.mesh_cache_hits = self.mesh_cache_hits.saturating_add(1);
            return Ok(());
        }
        self.mesh_cache_misses = self.mesh_cache_misses.saturating_add(1);
        let mut resources = create_mesh(device, self.backend, mesh, &mut self.mesh_pool)?;
        resources.last_used_frame = self.frame_index;
        statistics.mesh_uploads += 1;
        self.meshes.insert(key, resources);
        Ok(())
    }

    fn ensure_texture<D: Device + ?Sized>(
        &mut self,
        device: &mut D,
        texture_id: TextureAssetId,
        textures: &HashMap<TextureAssetId, Arc<TextureAsset>>,
        statistics: &mut FrameStatistics,
    ) -> Result<(), RenderError> {
        if let Some(resources) = self.textures.get_mut(&texture_id) {
            resources.last_used_frame = self.frame_index;
            self.texture_cache_hits = self.texture_cache_hits.saturating_add(1);
            return Ok(());
        }
        let Some(asset) = textures.get(&texture_id) else {
            return Ok(());
        };
        self.texture_cache_misses = self.texture_cache_misses.saturating_add(1);
        let (texture, view, bytes) = create_texture(device, asset)?;
        statistics.texture_uploads += 1;
        self.textures.insert(
            texture_id,
            TextureResources {
                texture,
                view,
                bytes,
                last_used_frame: self.frame_index,
            },
        );
        Ok(())
    }

    fn batch_steps<D: Device + ?Sized>(
        &mut self,
        device: &mut D,
        batch: crate::DrawBatch<'_>,
        slot: u32,
    ) -> Result<BatchSteps, RenderError> {
        let first = batch
            .draws()
            .first()
            .ok_or_else(|| RenderError::Preparation("draw batch is empty".into()))?;
        let mesh_key = (first.mesh.id(), first.mesh.generation());
        let part = first.part;
        let material = first.material.clone();
        let key = BatchKey {
            mesh_id: mesh_key.0,
            generation: mesh_key.1,
            part_first_index: part.first_index(),
            slot,
            draw_index: batch.first_instance() as u32,
            instance_count: batch.draws().len() as u32,
            albedo: material.albedo_texture,
            normal: material.normal_texture,
            occlusion: material.occlusion_texture,
            metallic_roughness: material.metallic_roughness_texture,
            epoch: self.resource_epoch,
        };
        let sets = if let Some(sets) = self.batch_sets.get(&key) {
            *sets
        } else {
            let sets = self.create_batch_sets(device, &key)?;
            self.batch_sets.insert(key, sets);
            sets
        };
        self.used_batch_keys.push(key);
        let indices = self
            .meshes
            .get(&mesh_key)
            .ok_or_else(|| RenderError::Preparation("mesh GPU resources are missing".into()))?
            .indices;
        let instance_count = key.instance_count;
        let pipelines = self.pipelines()?;
        let scene_pipeline = if material.alpha_mode == AlphaMode::Blend {
            pipelines.blended_pipeline
        } else {
            pipelines.opaque_pipeline
        };
        let index_count = part.index_count();
        Ok(BatchSteps {
            shadow: RenderCommand::DrawIndexed(DrawIndexedCommand {
                pipeline: pipelines.shadow_pipeline,
                bind_groups: gfx_core::bind_group_list([sets.shadow]),
                index_buffer: IndexBufferBinding {
                    buffer: indices,
                    format: IndexFormat::Uint32,
                    offset: 0,
                },
                index_count,
                first_index: part.first_index(),
                base_vertex: 0,
                instance_count,
                first_instance: key.draw_index,
                scissor: None,
            }),
            scene: RenderCommand::DrawIndexed(DrawIndexedCommand {
                pipeline: scene_pipeline,
                bind_groups: gfx_core::bind_group_list([sets.scene]),
                index_buffer: IndexBufferBinding {
                    buffer: indices,
                    format: IndexFormat::Uint32,
                    offset: 0,
                },
                index_count,
                first_index: part.first_index(),
                base_vertex: 0,
                instance_count,
                first_instance: key.draw_index,
                scissor: None,
            }),
            alpha_mode: material.alpha_mode,
            triangles: u64::from(index_count / 3) * u64::from(instance_count),
            instances: u64::from(instance_count),
        })
    }

    fn create_batch_sets<D: Device + ?Sized>(
        &mut self,
        device: &mut D,
        key: &BatchKey,
    ) -> Result<BatchSets, RenderError> {
        let mesh = self
            .meshes
            .get(&(key.mesh_id, key.generation))
            .ok_or_else(|| RenderError::Preparation("mesh GPU resources are missing".into()))?;
        let vertices = mesh.vertices;
        let vertex_bytes = mesh.vertex_bytes;
        let tangents = mesh.tangents;
        let tangent_bytes = mesh.tangent_bytes;
        let draw_offset = u64::from(key.slot) * DRAW_SLOT_STRIDE as u64;
        let draw_buffer = self.frame_buffers.draw.buffer;
        let instance_buffer = self.frame_buffers.instance.buffer;
        let instance_bytes = self.frame_buffers.instance_bytes;
        let light_buffer = self.frame_buffers.light.buffer;
        let light_bytes = self.frame_buffers.light_bytes;
        let frame_buffer = self.frame_buffers.frame.buffer;
        let fallback_view = self
            .fallback_texture
            .as_ref()
            .ok_or(RenderError::Destroyed)?
            .view;
        let albedo_view = key
            .albedo
            .and_then(|id| self.textures.get(&id))
            .map_or(fallback_view, |texture| texture.view);
        let normal_view = key
            .normal
            .and_then(|id| self.textures.get(&id))
            .map_or(fallback_view, |texture| texture.view);
        let occlusion_view = key
            .occlusion
            .and_then(|id| self.textures.get(&id))
            .map_or(fallback_view, |texture| texture.view);
        let metallic_roughness_view = key
            .metallic_roughness
            .and_then(|id| self.textures.get(&id))
            .map_or(fallback_view, |texture| texture.view);
        let environment_view = self
            .environment
            .as_ref()
            .map_or(fallback_view, |(_, texture)| texture.view);
        let pipelines = self.pipelines()?;
        let scene_layout = pipelines.scene_layout;
        let shadow_layout = pipelines.shadow_layout;
        let material_sampler = pipelines.material_sampler;
        let environment_sampler = pipelines.environment_sampler;
        let shadow_map_view = self
            .targets
            .as_ref()
            .ok_or(RenderError::Destroyed)?
            .shadow_color_view;
        let (tangent_buffer, tangent_offset, tangent_size) = match tangents {
            Some(tangents) => (tangents, 0, tangent_bytes.max(16)),
            None => (draw_buffer, draw_offset, 16),
        };
        let scene = device.create_bind_group(&BindGroupDescriptor {
            label: Some("gpui-3d offscreen scene resource set".into()),
            layout: scene_layout,
            entries: vec![
                buffer_binding(
                    crate::scene_view::VERTEX_BINDING,
                    vertices,
                    0,
                    vertex_bytes,
                    Some(PACKED_VERTEX_STRIDE),
                ),
                buffer_binding(
                    crate::scene_view::DRAW_BINDING,
                    draw_buffer,
                    draw_offset,
                    u64::from(DRAW_PARAMS_STRIDE),
                    Some(DRAW_PARAMS_STRIDE),
                ),
                buffer_binding(
                    crate::scene_view::LIGHT_BINDING,
                    light_buffer,
                    0,
                    light_bytes.max(u64::from(LIGHT_STRIDE)),
                    Some(LIGHT_STRIDE),
                ),
                texture_binding(crate::scene_view::ALBEDO_BINDING, albedo_view),
                sampler_binding(crate::scene_view::SAMPLER_BINDING, material_sampler),
                texture_binding(crate::scene_view::NORMAL_MAP_BINDING, normal_view),
                texture_binding(crate::scene_view::OCCLUSION_BINDING, occlusion_view),
                buffer_binding(
                    crate::scene_view::TANGENT_BINDING,
                    tangent_buffer,
                    tangent_offset,
                    tangent_size,
                    Some(16),
                ),
                buffer_binding(
                    crate::scene_view::INSTANCE_BINDING,
                    instance_buffer,
                    0,
                    instance_bytes.max(u64::from(INSTANCE_STRIDE)),
                    Some(INSTANCE_STRIDE),
                ),
                buffer_binding(
                    crate::scene_view::FRAME_BINDING,
                    frame_buffer,
                    0,
                    FRAME_STRIDE,
                    None,
                ),
                texture_binding(SCENE_SHADOW_MAP_BINDING, shadow_map_view),
                texture_binding(SCENE_METALLIC_ROUGHNESS_BINDING, metallic_roughness_view),
                texture_binding(SCENE_ENVIRONMENT_BINDING, environment_view),
                sampler_binding(SCENE_ENVIRONMENT_SAMPLER_BINDING, environment_sampler),
            ],
        })?;
        let shadow = match device.create_bind_group(&BindGroupDescriptor {
            label: Some("gpui-3d offscreen shadow resource set".into()),
            layout: shadow_layout,
            entries: vec![
                buffer_binding(
                    SHADOW_VERTEX_BINDING,
                    vertices,
                    0,
                    vertex_bytes,
                    Some(PACKED_VERTEX_STRIDE),
                ),
                buffer_binding(
                    SHADOW_INSTANCE_BINDING,
                    instance_buffer,
                    0,
                    instance_bytes.max(u64::from(INSTANCE_STRIDE)),
                    Some(INSTANCE_STRIDE),
                ),
                buffer_binding(SHADOW_FRAME_BINDING, frame_buffer, 0, FRAME_STRIDE, None),
            ],
        }) {
            Ok(set) => set,
            Err(error) => {
                device.destroy_bind_group(scene)?;
                return Err(error.into());
            }
        };
        Ok(BatchSets { scene, shadow })
    }

    fn release_unused_batch_sets<D: Device + ?Sized>(
        &mut self,
        device: &mut D,
    ) -> Result<(), RenderError> {
        let used = self.used_batch_keys.iter().copied().collect::<HashSet<_>>();
        let stale = self
            .batch_sets
            .keys()
            .filter(|key| !used.contains(key))
            .copied()
            .collect::<Vec<_>>();
        for key in stale {
            if let Some(sets) = self.batch_sets.remove(&key) {
                destroy_batch_sets(device, sets)?;
            }
        }
        Ok(())
    }

    fn ensure_post_set<D: Device + ?Sized>(
        &mut self,
        device: &mut D,
    ) -> Result<BindGroupId, RenderError> {
        if let Some(set) = self.post_set {
            return Ok(set);
        }
        let targets = self.targets.as_ref().ok_or(RenderError::Destroyed)?;
        let pipelines = self.pipelines()?;
        let set = device.create_bind_group(&BindGroupDescriptor {
            label: Some("gpui-3d offscreen post resource set".into()),
            layout: pipelines.post_layout,
            entries: vec![
                texture_binding(POST_COLOR_BINDING, targets.scene_color_view),
                sampler_binding(POST_SAMPLER_BINDING, pipelines.post_sampler),
                buffer_binding(
                    POST_FRAME_BINDING,
                    self.frame_buffers.frame.buffer,
                    0,
                    FRAME_STRIDE,
                    None,
                ),
                texture_binding(POST_BLOOM_BINDING, targets.bloom_color_view),
            ],
        })?;
        self.post_set = Some(set);
        Ok(set)
    }

    fn ensure_bloom_set<D: Device + ?Sized>(
        &mut self,
        device: &mut D,
    ) -> Result<BindGroupId, RenderError> {
        if let Some(set) = self.bloom_set {
            return Ok(set);
        }
        let targets = self.targets.as_ref().ok_or(RenderError::Destroyed)?;
        let pipelines = self.pipelines()?;
        let set = device.create_bind_group(&BindGroupDescriptor {
            label: Some("gpui-3d offscreen bloom resource set".into()),
            layout: pipelines.bloom_layout,
            entries: vec![
                texture_binding(BLOOM_SCENE_BINDING, targets.scene_color_view),
                sampler_binding(BLOOM_SAMPLER_BINDING, pipelines.post_sampler),
                buffer_binding(
                    BLOOM_FRAME_BINDING,
                    self.frame_buffers.frame.buffer,
                    0,
                    FRAME_STRIDE,
                    None,
                ),
            ],
        })?;
        self.bloom_set = Some(set);
        Ok(set)
    }

    /// Uploads or reuses the scene environment texture.
    ///
    /// Changing the environment destroys the previous texture, invalidates cached resource sets,
    /// and bumps the resource epoch so batch sets bind the new view.
    fn ensure_environment<D: Device + ?Sized>(
        &mut self,
        device: &mut D,
        environment: Option<&Arc<EnvironmentMap>>,
    ) -> Result<(), RenderError> {
        let Some(environment) = environment else {
            return Ok(());
        };
        if self
            .environment
            .as_ref()
            .is_some_and(|(id, _)| *id == environment.id())
        {
            return Ok(());
        }
        if let Some((_, texture)) = self.environment.take() {
            destroy_texture(device, texture)?;
        }
        let (texture, view, bytes) = create_texture(device, environment.texture())?;
        self.environment = Some((
            environment.id(),
            TextureResources {
                texture,
                view,
                bytes,
                last_used_frame: self.frame_index,
            },
        ));
        self.invalidate_sets(device)?;
        self.resource_epoch = self.resource_epoch.wrapping_add(1);
        Ok(())
    }

    fn ensure_sky_set<D: Device + ?Sized>(
        &mut self,
        device: &mut D,
    ) -> Result<Option<BindGroupId>, RenderError> {
        if self.sky_set.is_some() {
            return Ok(self.sky_set);
        }
        let Some((_, texture)) = self.environment.as_ref() else {
            return Ok(None);
        };
        let texture_view = texture.view;
        let pipelines = self.pipelines()?;
        let layout = pipelines.sky_layout;
        let environment_sampler = pipelines.environment_sampler;
        let frame_buffer = self.frame_buffers.frame.buffer;
        let set = device.create_bind_group(&BindGroupDescriptor {
            label: Some("gpui-3d offscreen sky resource set".into()),
            layout,
            entries: vec![
                texture_binding(SKY_ENVIRONMENT_BINDING, texture_view),
                sampler_binding(SKY_SAMPLER_BINDING, environment_sampler),
                buffer_binding(SKY_FRAME_BINDING, frame_buffer, 0, FRAME_STRIDE, None),
            ],
        })?;
        self.sky_set = Some(set);
        Ok(Some(set))
    }
}

/// Encodes one draw-parameter slot per instanced batch.
///
/// The scene-view extension still writes one slot per draw because the host pass has no instanced
/// batch contract. The offscreen renderer owns batching, so its draw buffer is sized by batch count
/// and a 200k-instance field keeps a single 512-byte slot instead of 102 MiB of duplicated slots.
fn encode_batch_draws_into(
    prepared: &PreparedScene,
    bytes: &mut Vec<u8>,
) -> Result<(), RenderError> {
    let light_count = u32::try_from(prepared.lights.len())
        .map_err(|_| RenderError::Preparation("GPUI 3D light count exceeds u32".into()))?;
    let start_len = bytes.len();
    for batch in prepared.draw_batches() {
        let Some(draw) = batch.draws().first() else {
            continue;
        };
        encode_draw_slot(draw, light_count, bytes).map_err(prepare_error)?;
    }
    if bytes.len() == start_len {
        bytes.resize(DRAW_SLOT_STRIDE, 0);
    }
    Ok(())
}

fn prepared_bounds(prepared: &PreparedScene) -> Option<Aabb> {
    let mut bounds: Option<Aabb> = None;
    for draw in &prepared.draws {
        bounds = Some(match bounds {
            Some(current) => Aabb {
                min: current.min.min(draw.bounds.min),
                max: current.max.max(draw.bounds.max),
            },
            None => draw.bounds,
        });
    }
    bounds
}

fn prepare_error(error: anyhow::Error) -> RenderError {
    RenderError::Preparation(format!("{error:#}"))
}

fn buffer_binding(
    binding: u32,
    buffer: BufferId,
    offset: u64,
    size: u64,
    stride: Option<u32>,
) -> BindGroupEntry {
    BindGroupEntry {
        binding,
        resource: BindingResource::Buffer(BufferBinding {
            buffer,
            offset,
            size,
            stride,
        }),
    }
}

fn texture_binding(binding: u32, texture_view: TextureViewId) -> BindGroupEntry {
    BindGroupEntry {
        binding,
        resource: BindingResource::Texture(TextureBinding { texture_view }),
    }
}

fn sampler_binding(binding: u32, sampler: SamplerId) -> BindGroupEntry {
    BindGroupEntry {
        binding,
        resource: BindingResource::Sampler(SamplerBinding { sampler }),
    }
}

struct BatchSteps {
    shadow: RenderCommand,
    scene: RenderCommand,
    alpha_mode: AlphaMode,
    triangles: u64,
    instances: u64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct BatchKey {
    mesh_id: MeshId,
    generation: u64,
    part_first_index: u32,
    /// Streaming draw-buffer slot; one slot per instanced batch.
    slot: u32,
    /// First instance index in the instance buffer.
    draw_index: u32,
    instance_count: u32,
    albedo: Option<TextureAssetId>,
    normal: Option<TextureAssetId>,
    occlusion: Option<TextureAssetId>,
    metallic_roughness: Option<TextureAssetId>,
    epoch: u64,
}

#[derive(Clone, Copy)]
struct BatchSets {
    scene: BindGroupId,
    shadow: BindGroupId,
}

fn destroy_batch_sets<D: Device + ?Sized>(
    device: &mut D,
    sets: BatchSets,
) -> Result<(), RenderError> {
    device.destroy_bind_group(sets.shadow)?;
    device.destroy_bind_group(sets.scene)?;
    Ok(())
}

struct MeshResources {
    vertices: BufferId,
    vertex_bytes: u64,
    vertex_capacity: u64,
    tangents: Option<BufferId>,
    tangent_bytes: u64,
    tangent_capacity: u64,
    indices: BufferId,
    index_capacity: u64,
    /// Allocated (aligned) buffer bytes.
    bytes: u64,
    /// Requested payload bytes, used to report allocator slack.
    requested_bytes: u64,
    location: MemoryLocation,
    last_used_frame: u64,
}

struct TextureResources {
    texture: TextureId,
    view: TextureViewId,
    bytes: u64,
    last_used_frame: u64,
}

/// Builds the light-space view-projection for a directional shadow map fitted to scene bounds.
fn fit_directional_shadow(bounds: Aabb, direction: Vec3) -> Option<Mat4> {
    let direction = direction.normalized()?;
    let center = Vec3::new(
        (bounds.min.x + bounds.max.x) * 0.5,
        (bounds.min.y + bounds.max.y) * 0.5,
        (bounds.min.z + bounds.max.z) * 0.5,
    );
    let extent = Vec3::new(
        bounds.max.x - bounds.min.x,
        bounds.max.y - bounds.min.y,
        bounds.max.z - bounds.min.z,
    );
    let radius = (extent.x * extent.x + extent.y * extent.y + extent.z * extent.z).sqrt() * 0.5;
    let radius = radius.max(0.01);
    let up = if direction.y.abs() > 0.9 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let distance = radius * 2.0 + 1.0;
    let eye = center - direction * distance;
    let camera = Camera::orthographic(
        eye,
        center,
        up,
        radius * 2.0 * 1.05,
        0.05,
        distance + radius * 2.0 + 1.0,
    );
    camera.view_projection(1.0).ok()
}

fn encode_frame_into(
    view_projection: Mat4,
    light_view_projection: Mat4,
    camera: Camera,
    directional: Option<(Vec3, [f32; 3], f32)>,
    options: &RenderOptions,
    target: (u32, u32),
    render_target: (u32, u32),
    shadow_enabled: bool,
    shadow_texel: f32,
    environment: [f32; 4],
    bytes: &mut Vec<u8>,
) {
    write_matrix(bytes, view_projection.columns());
    write_matrix(bytes, light_view_projection.columns());
    write_vec4(bytes, [camera.eye.x, camera.eye.y, camera.eye.z, 0.0]);
    let (direction, color, intensity) =
        directional.unwrap_or((Vec3::new(0.0, -1.0, 0.0), [1.0, 1.0, 1.0], 1.0));
    write_vec4(bytes, [direction.x, direction.y, direction.z, intensity]);
    write_vec4(bytes, [color[0], color[1], color[2], 0.0]);
    write_vec4(bytes, [0.0, 0.0, 0.0, 0.0]);
    write_vec4(
        bytes,
        [
            options.exposure,
            options.tonemap.shader_value() as f32,
            options.debug_view.shader_value() as f32,
            options.shadows.strength.clamp(0.0, 1.0),
        ],
    );
    write_vec4(
        bytes,
        [
            shadow_texel,
            shadow_texel,
            options.shadows.depth_bias,
            if shadow_enabled { 1.0 } else { 0.0 },
        ],
    );
    write_vec4(
        bytes,
        [
            target.0 as f32,
            target.1 as f32,
            options.shadows.normal_bias,
            if options.post_process { 1.0 } else { 0.0 },
        ],
    );
    write_vec4(
        bytes,
        [
            options.bloom.threshold,
            if options.bloom.enabled {
                options.bloom.strength.clamp(0.0, 4.0)
            } else {
                0.0
            },
            options.shadows.softness.clamp(0.2, 8.0),
            if options.antialias == super::AntialiasMode::Fxaa {
                1.0
            } else {
                0.0
            },
        ],
    );
    write_vec4(
        bytes,
        [
            options.post_effects.vignette.clamp(0.0, 1.0),
            options.post_effects.grain.clamp(0.0, 1.0),
            options.post_effects.dither.clamp(0.0, 1.0),
            if options.debug_view == super::DebugView::Shaded {
                0.0
            } else {
                1.0
            },
        ],
    );
    write_vec4(
        bytes,
        [
            options.bloom.radius.clamp(0.5, 4.0),
            options.bloom.threshold.max(0.0),
            render_target.0 as f32,
            render_target.1 as f32,
        ],
    );
    let inverse_view_projection = view_projection.inverse().unwrap_or(Mat4::IDENTITY);
    write_matrix(bytes, inverse_view_projection.columns());
    write_vec4(bytes, environment);
    debug_assert_eq!(bytes.len(), FRAME_STRIDE as usize);
}

fn staging_location(backend: BackendKind) -> MemoryLocation {
    match backend {
        BackendKind::Dx11 | BackendKind::Dx12 | BackendKind::Vulkan => MemoryLocation::GpuOnly,
        BackendKind::Metal | BackendKind::OpenGl | BackendKind::WebGl => MemoryLocation::CpuToGpu,
    }
}

fn aligned_buffer_size(bytes: u64) -> u64 {
    (bytes.max(16) + 15) & !15
}

fn create_gpu_buffer<D: Device + ?Sized>(
    device: &mut D,
    label: &str,
    bytes: u64,
    usage: BufferUsage,
    memory_location: MemoryLocation,
) -> Result<(BufferId, u64), RenderError> {
    let size = aligned_buffer_size(bytes);
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some(label.into()),
        size,
        usage,
        memory_location,
    })?;
    Ok((buffer, size))
}

fn create_mesh<D: Device + ?Sized>(
    device: &mut D,
    backend: BackendKind,
    mesh: &Mesh,
    pool: &mut MeshBufferPool,
) -> Result<MeshResources, RenderError> {
    let encoded = encode_mesh(mesh).map_err(prepare_error)?;
    let location = staging_location(backend);
    let vertex_bytes = encoded.vertices.len() as u64;
    let vertex_usage = BufferUsage::STORAGE | BufferUsage::COPY_DST;
    let (vertices, vertex_capacity) = match pool.take(vertex_bytes, vertex_usage, location) {
        Some(buffer) => buffer,
        None => create_gpu_buffer(
            device,
            "gpui-3d offscreen vertices",
            vertex_bytes,
            vertex_usage,
            location,
        )?,
    };
    if let Err(error) = device.write_buffer(vertices, 0, &encoded.vertices) {
        device.destroy_buffer(vertices)?;
        return Err(error.into());
    }
    let tangent_bytes = encoded
        .tangents
        .as_ref()
        .map_or(0_u64, |tangents| tangents.len() as u64);
    let tangents = if let Some(tangent_data) = &encoded.tangents {
        let (buffer, capacity) = match pool.take(tangent_bytes, vertex_usage, location) {
            Some(buffer) => buffer,
            None => create_gpu_buffer(
                device,
                "gpui-3d offscreen tangents",
                tangent_bytes,
                vertex_usage,
                location,
            )?,
        };
        if let Err(error) = device.write_buffer(buffer, 0, tangent_data) {
            device.destroy_buffer(buffer)?;
            device.destroy_buffer(vertices)?;
            return Err(error.into());
        }
        Some((buffer, capacity))
    } else {
        None
    };
    let index_bytes = encoded.indices.len() as u64;
    let index_usage = BufferUsage::INDEX | BufferUsage::COPY_DST;
    let (indices, index_capacity) = match pool.take(index_bytes, index_usage, location) {
        Some(buffer) => buffer,
        None => create_gpu_buffer(
            device,
            "gpui-3d offscreen indices",
            index_bytes,
            index_usage,
            location,
        )?,
    };
    if let Err(error) = device.write_buffer(indices, 0, &encoded.indices) {
        device.destroy_buffer(indices)?;
        if let Some((buffer, _)) = tangents {
            device.destroy_buffer(buffer)?;
        }
        device.destroy_buffer(vertices)?;
        return Err(error.into());
    }
    let mut bytes = vertex_capacity + index_capacity;
    if let Some((_, capacity)) = tangents {
        bytes += capacity;
    }
    Ok(MeshResources {
        vertices,
        vertex_bytes,
        vertex_capacity,
        tangents: tangents.map(|(buffer, _)| buffer),
        tangent_bytes,
        tangent_capacity: tangents.map_or(0, |(_, capacity)| capacity),
        indices,
        index_capacity,
        bytes,
        requested_bytes: vertex_bytes + index_bytes + tangent_bytes,
        location,
        last_used_frame: 0,
    })
}

fn destroy_mesh<D: Device + ?Sized>(
    device: &mut D,
    mesh: MeshResources,
    pool: &mut MeshBufferPool,
) {
    // Recycle instead of destroying so a rebuild can reuse the allocation without a fresh
    // GPU allocation; the pool is bounded and cleared by aggressive trim/budget pressure.
    pool.recycle(
        device,
        PooledBuffer {
            buffer: mesh.indices,
            capacity: mesh.index_capacity,
            usage: BufferUsage::INDEX | BufferUsage::COPY_DST,
            location: mesh.location,
        },
        0,
    );
    if let Some(tangents) = mesh.tangents {
        pool.recycle(
            device,
            PooledBuffer {
                buffer: tangents,
                capacity: mesh.tangent_capacity,
                usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
                location: mesh.location,
            },
            0,
        );
    }
    pool.recycle(
        device,
        PooledBuffer {
            buffer: mesh.vertices,
            capacity: mesh.vertex_capacity,
            usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
            location: mesh.location,
        },
        0,
    );
}

fn create_texture<D: Device + ?Sized>(
    device: &mut D,
    asset: &TextureAsset,
) -> Result<(TextureId, TextureViewId, u64), RenderError> {
    let size = Extent2d::new(asset.width(), asset.height())?;
    let format = match asset.color_space() {
        TextureColorSpace::Srgb => Format::Rgba8UnormSrgb,
        TextureColorSpace::Linear => Format::Rgba8Unorm,
    };
    let texture = device.create_texture(&TextureDescriptor {
        label: Some(format!("gpui-3d offscreen texture {}", asset.id().0)),
        size,
        mip_level_count: asset.mip_level_count(),
        format,
        usage: TextureUsage::SAMPLED | TextureUsage::COPY_DST,
        memory_location: MemoryLocation::GpuOnly,
        dimension: TextureDimension::D2,
    })?;
    let mut bytes = 0_u64;
    let mut writes = Vec::with_capacity(asset.mip_level_count() as usize);
    for mip_level in 0..asset.mip_level_count() {
        let (width, height) = asset
            .mip_extent(mip_level)
            .ok_or_else(|| RenderError::Preparation("texture mip extent is missing".into()))?;
        let mip_size = Extent2d::new(width, height)?;
        let bytes_per_row = width
            .checked_mul(4)
            .ok_or_else(|| RenderError::Preparation("texture row size overflow".into()))?;
        let layout = TextureDataLayout::new(0, bytes_per_row, height)?;
        let data = asset
            .mip_pixels(mip_level)
            .ok_or_else(|| RenderError::Preparation("texture mip pixels are missing".into()))?;
        writes.push(TextureWrite {
            descriptor: TextureWriteDescriptor {
                texture,
                mip_level,
                layout,
                origin: Origin2d { x: 0, y: 0 },
                size: mip_size,
            },
            data,
        });
        bytes += u64::from(width) * u64::from(height) * 4;
    }
    if let Err(error) = device.write_texture_batch(writes) {
        device.destroy_texture(texture)?;
        return Err(error.into());
    }
    let view = match device.create_texture_view(&TextureViewDescriptor {
        label: Some(format!("gpui-3d offscreen texture view {}", asset.id().0)),
        texture,
        base_mip_level: 0,
        mip_level_count: asset.mip_level_count(),
        format,
    }) {
        Ok(view) => view,
        Err(error) => {
            device.destroy_texture(texture)?;
            return Err(error.into());
        }
    };
    Ok((texture, view, bytes))
}

fn create_fallback_texture<D: Device + ?Sized>(
    device: &mut D,
) -> Result<TextureResources, RenderError> {
    let texture = device.create_texture(&TextureDescriptor {
        label: Some("gpui-3d offscreen fallback texture".into()),
        size: Extent2d::new(1, 1)?,
        mip_level_count: 1,
        format: Format::Rgba8Unorm,
        usage: TextureUsage::SAMPLED | TextureUsage::COPY_DST,
        memory_location: MemoryLocation::GpuOnly,
        dimension: TextureDimension::D2,
    })?;
    if let Err(error) = device.write_texture(
        TextureWriteDescriptor {
            texture,
            mip_level: 0,
            layout: TextureDataLayout::new(0, 4, 1)?,
            origin: Origin2d { x: 0, y: 0 },
            size: Extent2d::new(1, 1)?,
        },
        &[255, 255, 255, 255],
    ) {
        device.destroy_texture(texture)?;
        return Err(error.into());
    }
    let view = match device.create_texture_view(&TextureViewDescriptor {
        label: Some("gpui-3d offscreen fallback texture view".into()),
        texture,
        base_mip_level: 0,
        mip_level_count: 1,
        format: Format::Rgba8Unorm,
    }) {
        Ok(view) => view,
        Err(error) => {
            device.destroy_texture(texture)?;
            return Err(error.into());
        }
    };
    Ok(TextureResources {
        texture,
        view,
        bytes: 4,
        last_used_frame: 0,
    })
}

fn destroy_texture<D: Device + ?Sized>(
    device: &mut D,
    texture: TextureResources,
) -> Result<(), RenderError> {
    device.destroy_texture_view(texture.view)?;
    device.destroy_texture(texture.texture)?;
    Ok(())
}

#[derive(Default)]
struct HandleScratch {
    shaders: Vec<ShaderModuleId>,
    passes: Vec<RenderPassId>,
    layouts: Vec<BindGroupLayoutId>,
    pipeline_layouts: Vec<PipelineLayoutId>,
    pipelines: Vec<RenderPipelineId>,
    samplers: Vec<SamplerId>,
    buffers: Vec<BufferId>,
}

impl HandleScratch {
    fn destroy<D: Device + ?Sized>(self, device: &mut D) {
        for pipeline in self.pipelines {
            let _ = device.destroy_render_pipeline(pipeline);
        }
        for sampler in self.samplers {
            let _ = device.destroy_sampler(sampler);
        }
        for layout in self.layouts {
            let _ = device.destroy_bind_group_layout(layout);
        }
        for layout in self.pipeline_layouts {
            let _ = device.destroy_pipeline_layout(layout);
        }
        for shader in self.shaders {
            let _ = device.destroy_shader_module(shader);
        }
        for pass in self.passes {
            let _ = device.destroy_render_pass(pass);
        }
        for buffer in self.buffers {
            let _ = device.destroy_buffer(buffer);
        }
    }
}

struct Pipelines {
    shadow_layout: BindGroupLayoutId,
    scene_layout: BindGroupLayoutId,
    post_layout: BindGroupLayoutId,
    bloom_layout: BindGroupLayoutId,
    sky_layout: BindGroupLayoutId,
    shadow_pass: RenderPassId,
    scene_pass: RenderPassId,
    post_pass: RenderPassId,
    bloom_pass: RenderPassId,
    shadow_pipeline: RenderPipelineId,
    opaque_pipeline: RenderPipelineId,
    blended_pipeline: RenderPipelineId,
    post_pipeline: RenderPipelineId,
    bloom_pipeline: RenderPipelineId,
    sky_pipeline: RenderPipelineId,
    shadow_pipeline_layout: PipelineLayoutId,
    scene_pipeline_layout: PipelineLayoutId,
    post_pipeline_layout: PipelineLayoutId,
    bloom_pipeline_layout: PipelineLayoutId,
    sky_pipeline_layout: PipelineLayoutId,
    material_sampler: SamplerId,
    post_sampler: SamplerId,
    environment_sampler: SamplerId,
    shaders: Vec<ShaderModuleId>,
}

impl Pipelines {
    fn new<D: Device + ?Sized>(
        device: &mut D,
        backend: BackendKind,
        width: u32,
        height: u32,
    ) -> Result<Self, RenderError> {
        let mut scratch = HandleScratch::default();
        match Self::build(device, backend, width, height, &mut scratch) {
            Ok(pipelines) => Ok(pipelines),
            Err(error) => {
                scratch.destroy(device);
                Err(error)
            }
        }
    }

    fn build<D: Device + ?Sized>(
        device: &mut D,
        backend: BackendKind,
        width: u32,
        height: u32,
        scratch: &mut HandleScratch,
    ) -> Result<Self, RenderError> {
        let extent = Extent2d::new(width, height)?;
        let shadow_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("gpui-3d offscreen shadow layout".into()),
            entries: vec![
                layout_entry(
                    SHADOW_VERTEX_BINDING,
                    BindingType::StorageBuffer,
                    ShaderStages::VERTEX,
                ),
                layout_entry(
                    SHADOW_INSTANCE_BINDING,
                    BindingType::StorageBuffer,
                    ShaderStages::VERTEX,
                ),
                layout_entry(
                    SHADOW_FRAME_BINDING,
                    BindingType::UniformBuffer,
                    ShaderStages::VERTEX,
                ),
            ],
        })?;
        scratch.layouts.push(shadow_layout);
        let scene_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("gpui-3d offscreen scene layout".into()),
            entries: vec![
                layout_entry(
                    crate::scene_view::VERTEX_BINDING,
                    BindingType::StorageBuffer,
                    ShaderStages::VERTEX,
                ),
                layout_entry(
                    crate::scene_view::DRAW_BINDING,
                    BindingType::StorageBuffer,
                    ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                ),
                layout_entry(
                    crate::scene_view::LIGHT_BINDING,
                    BindingType::StorageBuffer,
                    ShaderStages::FRAGMENT,
                ),
                layout_entry(
                    crate::scene_view::ALBEDO_BINDING,
                    BindingType::SampledTexture,
                    ShaderStages::FRAGMENT,
                ),
                layout_entry(
                    crate::scene_view::SAMPLER_BINDING,
                    BindingType::Sampler,
                    ShaderStages::FRAGMENT,
                ),
                layout_entry(
                    crate::scene_view::NORMAL_MAP_BINDING,
                    BindingType::SampledTexture,
                    ShaderStages::FRAGMENT,
                ),
                layout_entry(
                    crate::scene_view::OCCLUSION_BINDING,
                    BindingType::SampledTexture,
                    ShaderStages::FRAGMENT,
                ),
                layout_entry(
                    crate::scene_view::TANGENT_BINDING,
                    BindingType::StorageBuffer,
                    ShaderStages::VERTEX,
                ),
                layout_entry(
                    crate::scene_view::INSTANCE_BINDING,
                    BindingType::StorageBuffer,
                    ShaderStages::VERTEX,
                ),
                layout_entry(
                    crate::scene_view::FRAME_BINDING,
                    BindingType::UniformBuffer,
                    ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                ),
                layout_entry(
                    SCENE_SHADOW_MAP_BINDING,
                    BindingType::SampledTexture,
                    ShaderStages::FRAGMENT,
                ),
                layout_entry(
                    SCENE_METALLIC_ROUGHNESS_BINDING,
                    BindingType::SampledTexture,
                    ShaderStages::FRAGMENT,
                ),
                layout_entry(
                    SCENE_ENVIRONMENT_BINDING,
                    BindingType::SampledTexture,
                    ShaderStages::FRAGMENT,
                ),
                layout_entry(
                    SCENE_ENVIRONMENT_SAMPLER_BINDING,
                    BindingType::Sampler,
                    ShaderStages::FRAGMENT,
                ),
            ],
        })?;
        scratch.layouts.push(scene_layout);
        let post_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("gpui-3d offscreen post layout".into()),
            entries: vec![
                layout_entry(
                    POST_COLOR_BINDING,
                    BindingType::SampledTexture,
                    ShaderStages::FRAGMENT,
                ),
                layout_entry(
                    POST_SAMPLER_BINDING,
                    BindingType::Sampler,
                    ShaderStages::FRAGMENT,
                ),
                layout_entry(
                    POST_FRAME_BINDING,
                    BindingType::UniformBuffer,
                    ShaderStages::FRAGMENT,
                ),
                layout_entry(
                    POST_BLOOM_BINDING,
                    BindingType::SampledTexture,
                    ShaderStages::FRAGMENT,
                ),
            ],
        })?;
        scratch.layouts.push(post_layout);
        let bloom_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("gpui-3d offscreen bloom layout".into()),
            entries: vec![
                layout_entry(
                    BLOOM_SCENE_BINDING,
                    BindingType::SampledTexture,
                    ShaderStages::FRAGMENT,
                ),
                layout_entry(
                    BLOOM_SAMPLER_BINDING,
                    BindingType::Sampler,
                    ShaderStages::FRAGMENT,
                ),
                layout_entry(
                    BLOOM_FRAME_BINDING,
                    BindingType::UniformBuffer,
                    ShaderStages::FRAGMENT,
                ),
            ],
        })?;
        scratch.layouts.push(bloom_layout);
        let sky_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("gpui-3d offscreen sky layout".into()),
            entries: vec![
                layout_entry(
                    SKY_ENVIRONMENT_BINDING,
                    BindingType::SampledTexture,
                    ShaderStages::FRAGMENT,
                ),
                layout_entry(
                    SKY_SAMPLER_BINDING,
                    BindingType::Sampler,
                    ShaderStages::FRAGMENT,
                ),
                layout_entry(
                    SKY_FRAME_BINDING,
                    BindingType::UniformBuffer,
                    ShaderStages::FRAGMENT,
                ),
            ],
        })?;
        scratch.layouts.push(sky_layout);

        let shadow_pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("gpui-3d offscreen shadow pipeline layout".into()),
            bind_group_layouts: vec![shadow_layout],
        })?;
        scratch.pipeline_layouts.push(shadow_pipeline_layout);
        let scene_pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("gpui-3d offscreen scene pipeline layout".into()),
            bind_group_layouts: vec![scene_layout],
        })?;
        scratch.pipeline_layouts.push(scene_pipeline_layout);
        let post_pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("gpui-3d offscreen post pipeline layout".into()),
            bind_group_layouts: vec![post_layout],
        })?;
        scratch.pipeline_layouts.push(post_pipeline_layout);
        let bloom_pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("gpui-3d offscreen bloom pipeline layout".into()),
            bind_group_layouts: vec![bloom_layout],
        })?;
        scratch.pipeline_layouts.push(bloom_pipeline_layout);
        let sky_pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("gpui-3d offscreen sky pipeline layout".into()),
            bind_group_layouts: vec![sky_layout],
        })?;
        scratch.pipeline_layouts.push(sky_pipeline_layout);

        let shadow_vertex_shader =
            shader_module(device, backend, "vs_shadow_depth", ShaderStage::Vertex)?;
        scratch.shaders.push(shadow_vertex_shader);
        let shadow_fragment_shader =
            shader_module(device, backend, "fs_shadow_depth", ShaderStage::Fragment)?;
        scratch.shaders.push(shadow_fragment_shader);
        let scene_vertex_shader = shader_module(device, backend, "vs_scene", ShaderStage::Vertex)?;
        scratch.shaders.push(scene_vertex_shader);
        let scene_fragment_shader =
            shader_module(device, backend, "fs_scene", ShaderStage::Fragment)?;
        scratch.shaders.push(scene_fragment_shader);
        let post_vertex_shader = shader_module(device, backend, "vs_post", ShaderStage::Vertex)?;
        scratch.shaders.push(post_vertex_shader);
        let post_fragment_shader =
            shader_module(device, backend, "fs_post", ShaderStage::Fragment)?;
        scratch.shaders.push(post_fragment_shader);
        let bloom_vertex_shader = shader_module(device, backend, "vs_bloom", ShaderStage::Vertex)?;
        scratch.shaders.push(bloom_vertex_shader);
        let bloom_fragment_shader =
            shader_module(device, backend, "fs_bloom", ShaderStage::Fragment)?;
        scratch.shaders.push(bloom_fragment_shader);
        let sky_vertex_shader = shader_module(device, backend, "vs_sky", ShaderStage::Vertex)?;
        scratch.shaders.push(sky_vertex_shader);
        let sky_fragment_shader = shader_module(device, backend, "fs_sky", ShaderStage::Fragment)?;
        scratch.shaders.push(sky_fragment_shader);

        let shadow_pass = device.create_render_pass(&RenderPassDescriptor {
            label: Some("gpui-3d offscreen shadow pass".into()),
            color_attachment: ColorAttachmentDescriptor {
                format: SHADOW_COLOR_FORMAT,
            },
            depth_attachment: Some(DepthAttachmentDescriptor {
                format: DEPTH_FORMAT,
            }),
        })?;
        scratch.passes.push(shadow_pass);
        let scene_pass = device.create_render_pass(&RenderPassDescriptor {
            label: Some("gpui-3d offscreen scene pass".into()),
            color_attachment: ColorAttachmentDescriptor {
                format: SCENE_COLOR_FORMAT,
            },
            depth_attachment: Some(DepthAttachmentDescriptor {
                format: DEPTH_FORMAT,
            }),
        })?;
        scratch.passes.push(scene_pass);
        let post_pass = device.create_render_pass(&RenderPassDescriptor {
            label: Some("gpui-3d offscreen post pass".into()),
            color_attachment: ColorAttachmentDescriptor {
                format: POST_COLOR_FORMAT,
            },
            depth_attachment: None,
        })?;
        scratch.passes.push(post_pass);
        let bloom_pass = device.create_render_pass(&RenderPassDescriptor {
            label: Some("gpui-3d offscreen bloom pass".into()),
            color_attachment: ColorAttachmentDescriptor {
                format: BLOOM_COLOR_FORMAT,
            },
            depth_attachment: None,
        })?;
        scratch.passes.push(bloom_pass);

        let create_pipeline = |device: &mut D,
                               label: &str,
                               vertex_shader: ShaderModuleId,
                               vertex_entry: &str,
                               fragment_shader: ShaderModuleId,
                               fragment_entry: &str,
                               render_pass: RenderPassId,
                               pipeline_layout: PipelineLayoutId,
                               color_format: Format,
                               blend_mode: BlendMode,
                               depth_state: Option<DepthState>|
         -> Result<RenderPipelineId, RenderError> {
            device
                .create_render_pipeline(
                    &RenderPipelineDescriptor {
                        label: Some(label.into()),
                        vertex_shader,
                        vertex_entry_point: vertex_entry.into(),
                        fragment_shader,
                        fragment_entry_point: fragment_entry.into(),
                        vertex_buffers: Vec::new(),
                        render_pass,
                        pipeline_layout: Some(pipeline_layout),
                        color_format,
                        blend_mode,
                        primitive_topology: PrimitiveTopology::TriangleList,
                        depth_state,
                    },
                    extent,
                )
                .map_err(|error| {
                    RenderError::Gpu(gfx_core::Error::Shader(format!("{label}: {error}")))
                })
        };

        let shadow_pipeline = create_pipeline(
            device,
            "gpui-3d offscreen shadow pipeline",
            shadow_vertex_shader,
            "vs_shadow_depth",
            shadow_fragment_shader,
            "fs_shadow_depth",
            shadow_pass,
            shadow_pipeline_layout,
            SHADOW_COLOR_FORMAT,
            BlendMode::Replace,
            Some(DepthState {
                compare: CompareFunction::Less,
                write_enabled: true,
            }),
        )?;
        scratch.pipelines.push(shadow_pipeline);
        let opaque_pipeline = create_pipeline(
            device,
            "gpui-3d offscreen opaque pipeline",
            scene_vertex_shader,
            "vs_scene",
            scene_fragment_shader,
            "fs_scene",
            scene_pass,
            scene_pipeline_layout,
            SCENE_COLOR_FORMAT,
            BlendMode::Replace,
            Some(DepthState {
                compare: CompareFunction::LessEqual,
                write_enabled: true,
            }),
        )?;
        scratch.pipelines.push(opaque_pipeline);
        let blended_pipeline = create_pipeline(
            device,
            "gpui-3d offscreen blended pipeline",
            scene_vertex_shader,
            "vs_scene",
            scene_fragment_shader,
            "fs_scene",
            scene_pass,
            scene_pipeline_layout,
            SCENE_COLOR_FORMAT,
            BlendMode::PremultipliedAlpha,
            Some(DepthState {
                compare: CompareFunction::LessEqual,
                write_enabled: false,
            }),
        )?;
        scratch.pipelines.push(blended_pipeline);
        let post_pipeline = create_pipeline(
            device,
            "gpui-3d offscreen post pipeline",
            post_vertex_shader,
            "vs_post",
            post_fragment_shader,
            "fs_post",
            post_pass,
            post_pipeline_layout,
            POST_COLOR_FORMAT,
            BlendMode::Replace,
            None,
        )?;
        scratch.pipelines.push(post_pipeline);
        let bloom_pipeline = create_pipeline(
            device,
            "gpui-3d offscreen bloom pipeline",
            bloom_vertex_shader,
            "vs_bloom",
            bloom_fragment_shader,
            "fs_bloom",
            bloom_pass,
            bloom_pipeline_layout,
            BLOOM_COLOR_FORMAT,
            BlendMode::Replace,
            None,
        )?;
        scratch.pipelines.push(bloom_pipeline);
        let sky_pipeline = create_pipeline(
            device,
            "gpui-3d offscreen sky pipeline",
            sky_vertex_shader,
            "vs_sky",
            sky_fragment_shader,
            "fs_sky",
            scene_pass,
            sky_pipeline_layout,
            SCENE_COLOR_FORMAT,
            BlendMode::Replace,
            Some(DepthState {
                compare: CompareFunction::LessEqual,
                write_enabled: false,
            }),
        )?;
        scratch.pipelines.push(sky_pipeline);

        let material_sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("gpui-3d offscreen material sampler".into()),
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: FilterMode::Linear,
            anisotropic: true,
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
        })?;
        scratch.samplers.push(material_sampler);
        let post_sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("gpui-3d offscreen post sampler".into()),
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: FilterMode::Nearest,
            anisotropic: false,
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
        })?;
        scratch.samplers.push(post_sampler);
        let environment_sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("gpui-3d offscreen environment sampler".into()),
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: FilterMode::Linear,
            anisotropic: false,
            address_mode_u: AddressMode::Repeat,
            address_mode_v: AddressMode::ClampToEdge,
        })?;
        scratch.samplers.push(environment_sampler);

        Ok(Self {
            shadow_layout,
            scene_layout,
            post_layout,
            bloom_layout,
            sky_layout,
            shadow_pass,
            scene_pass,
            post_pass,
            bloom_pass,
            shadow_pipeline,
            opaque_pipeline,
            blended_pipeline,
            post_pipeline,
            bloom_pipeline,
            sky_pipeline,
            shadow_pipeline_layout,
            scene_pipeline_layout,
            post_pipeline_layout,
            bloom_pipeline_layout,
            sky_pipeline_layout,
            material_sampler,
            post_sampler,
            environment_sampler,
            shaders: vec![
                shadow_vertex_shader,
                shadow_fragment_shader,
                scene_vertex_shader,
                scene_fragment_shader,
                post_vertex_shader,
                post_fragment_shader,
                bloom_vertex_shader,
                bloom_fragment_shader,
                sky_vertex_shader,
                sky_fragment_shader,
            ],
        })
    }

    fn destroy<D: Device + ?Sized>(self, device: &mut D) -> Result<(), RenderError> {
        device.destroy_render_pipeline(self.shadow_pipeline)?;
        device.destroy_render_pipeline(self.opaque_pipeline)?;
        device.destroy_render_pipeline(self.blended_pipeline)?;
        device.destroy_render_pipeline(self.post_pipeline)?;
        device.destroy_render_pipeline(self.bloom_pipeline)?;
        device.destroy_render_pipeline(self.sky_pipeline)?;
        device.destroy_sampler(self.material_sampler)?;
        device.destroy_sampler(self.post_sampler)?;
        device.destroy_sampler(self.environment_sampler)?;
        for shader in self.shaders {
            device.destroy_shader_module(shader)?;
        }
        device.destroy_render_pass(self.shadow_pass)?;
        device.destroy_render_pass(self.scene_pass)?;
        device.destroy_render_pass(self.post_pass)?;
        device.destroy_render_pass(self.bloom_pass)?;
        device.destroy_bind_group_layout(self.shadow_layout)?;
        device.destroy_bind_group_layout(self.scene_layout)?;
        device.destroy_bind_group_layout(self.post_layout)?;
        device.destroy_bind_group_layout(self.bloom_layout)?;
        device.destroy_bind_group_layout(self.sky_layout)?;
        // Pipeline layouts were previously leaked on every renderer drop/resize.
        device.destroy_pipeline_layout(self.shadow_pipeline_layout)?;
        device.destroy_pipeline_layout(self.scene_pipeline_layout)?;
        device.destroy_pipeline_layout(self.post_pipeline_layout)?;
        device.destroy_pipeline_layout(self.bloom_pipeline_layout)?;
        device.destroy_pipeline_layout(self.sky_pipeline_layout)?;
        Ok(())
    }
}

fn layout_entry(
    binding: u32,
    binding_type: BindingType,
    stages: ShaderStages,
) -> BindGroupLayoutEntry {
    BindGroupLayoutEntry {
        binding,
        binding_type,
        stages,
    }
}

fn shader_module<D: Device + ?Sized>(
    device: &mut D,
    backend: BackendKind,
    entry_point: &str,
    stage: ShaderStage,
) -> Result<ShaderModuleId, RenderError> {
    let binary = super::artifacts::embedded_shader(backend, stage, entry_point)?;
    device
        .create_shader_module(&ShaderModuleDescriptor {
            label: Some(format!("gpui-3d offscreen {entry_point}")),
            binary,
        })
        .map_err(|error| {
            RenderError::Gpu(gfx_core::Error::Shader(format!(
                "create {entry_point}: {error}"
            )))
        })
}

struct Targets {
    /// Output (post/readback) width.
    width: u32,
    /// Output (post/readback) height.
    height: u32,
    /// Internal HDR scene width; equals output width when render scale is 1.
    render_width: u32,
    /// Internal HDR scene height.
    render_height: u32,
    /// Width ratio of the HDR scene target over the output target.
    render_scale: f32,
    shadow_resolution: u32,
    shadow_color: TextureId,
    shadow_color_view: TextureViewId,
    shadow_depth: TextureId,
    shadow_depth_view: TextureViewId,
    scene_color: TextureId,
    scene_color_view: TextureViewId,
    scene_depth: TextureId,
    scene_depth_view: TextureViewId,
    post_color: TextureId,
    post_color_view: TextureViewId,
    bloom_color: TextureId,
    bloom_color_view: TextureViewId,
}

impl Targets {
    fn new<D: Device + ?Sized>(
        device: &mut D,
        width: u32,
        height: u32,
        shadow_resolution: u32,
        render_scale: f32,
    ) -> Result<Self, RenderError> {
        let (render_width, render_height) = scaled_extent(width, height, render_scale);
        let (shadow_color, shadow_color_view, shadow_depth, shadow_depth_view) =
            create_shadow_targets(device, shadow_resolution)?;
        let (
            scene_color,
            scene_color_view,
            scene_depth,
            scene_depth_view,
            post_color,
            post_color_view,
            bloom_color,
            bloom_color_view,
        ) = match create_scene_targets(device, width, height, render_width, render_height) {
            Ok(targets) => targets,
            Err(error) => {
                let _ = device.destroy_texture_view(shadow_depth_view);
                let _ = device.destroy_texture(shadow_depth);
                let _ = device.destroy_texture_view(shadow_color_view);
                let _ = device.destroy_texture(shadow_color);
                return Err(error);
            }
        };
        Ok(Self {
            width,
            height,
            render_width,
            render_height,
            render_scale,
            shadow_resolution,
            shadow_color,
            shadow_color_view,
            shadow_depth,
            shadow_depth_view,
            scene_color,
            scene_color_view,
            scene_depth,
            scene_depth_view,
            post_color,
            post_color_view,
            bloom_color,
            bloom_color_view,
        })
    }

    fn resize_scene<D: Device + ?Sized>(
        &mut self,
        device: &mut D,
        width: u32,
        height: u32,
        render_scale: f32,
    ) -> Result<(), RenderError> {
        let (render_width, render_height) = scaled_extent(width, height, render_scale);
        destroy_scene_targets(
            device,
            self.scene_color,
            self.scene_color_view,
            self.scene_depth,
            self.scene_depth_view,
            self.post_color,
            self.post_color_view,
            self.bloom_color,
            self.bloom_color_view,
        )?;
        let (
            scene_color,
            scene_color_view,
            scene_depth,
            scene_depth_view,
            post_color,
            post_color_view,
            bloom_color,
            bloom_color_view,
        ) = create_scene_targets(device, width, height, render_width, render_height)?;
        self.width = width;
        self.height = height;
        self.render_width = render_width;
        self.render_height = render_height;
        self.render_scale = render_scale;
        self.scene_color = scene_color;
        self.scene_color_view = scene_color_view;
        self.scene_depth = scene_depth;
        self.scene_depth_view = scene_depth_view;
        self.post_color = post_color;
        self.post_color_view = post_color_view;
        self.bloom_color = bloom_color;
        self.bloom_color_view = bloom_color_view;
        Ok(())
    }

    fn resize_shadow<D: Device + ?Sized>(
        &mut self,
        device: &mut D,
        resolution: u32,
    ) -> Result<(), RenderError> {
        destroy_shadow_targets(
            device,
            self.shadow_color,
            self.shadow_color_view,
            self.shadow_depth,
            self.shadow_depth_view,
        )?;
        let (shadow_color, shadow_color_view, shadow_depth, shadow_depth_view) =
            create_shadow_targets(device, resolution)?;
        self.shadow_resolution = resolution;
        self.shadow_color = shadow_color;
        self.shadow_color_view = shadow_color_view;
        self.shadow_depth = shadow_depth;
        self.shadow_depth_view = shadow_depth_view;
        Ok(())
    }

    fn resident_bytes(&self) -> u64 {
        let output_pixels = u64::from(self.width) * u64::from(self.height);
        let render_pixels = u64::from(self.render_width) * u64::from(self.render_height);
        let bloom_pixels =
            u64::from((self.render_width / 2).max(1)) * u64::from((self.render_height / 2).max(1));
        let shadow_pixels = u64::from(self.shadow_resolution) * u64::from(self.shadow_resolution);
        // Shadow color (4) + depth (4), HDR scene color (8) + depth (4), post color (4), bloom (4).
        shadow_pixels * 8 + render_pixels * 12 + bloom_pixels * 4 + output_pixels * 4
    }

    fn render_target(&self) -> (u32, u32) {
        (self.render_width, self.render_height)
    }

    fn destroy<D: Device + ?Sized>(self, device: &mut D) -> Result<(), RenderError> {
        destroy_scene_targets(
            device,
            self.scene_color,
            self.scene_color_view,
            self.scene_depth,
            self.scene_depth_view,
            self.post_color,
            self.post_color_view,
            self.bloom_color,
            self.bloom_color_view,
        )?;
        destroy_shadow_targets(
            device,
            self.shadow_color,
            self.shadow_color_view,
            self.shadow_depth,
            self.shadow_depth_view,
        )?;
        Ok(())
    }
}

type SceneTargets = (
    TextureId,
    TextureViewId,
    TextureId,
    TextureViewId,
    TextureId,
    TextureViewId,
    TextureId,
    TextureViewId,
);

fn scaled_extent(width: u32, height: u32, render_scale: f32) -> (u32, u32) {
    let scale = if render_scale.is_finite() {
        render_scale.clamp(0.25, 2.0)
    } else {
        1.0
    };
    (
        ((width as f32 * scale).round() as u32).max(1),
        ((height as f32 * scale).round() as u32).max(1),
    )
}

fn create_scene_targets<D: Device + ?Sized>(
    device: &mut D,
    output_width: u32,
    output_height: u32,
    render_width: u32,
    render_height: u32,
) -> Result<SceneTargets, RenderError> {
    let (scene_color, scene_color_view) = create_color_target(
        device,
        "gpui-3d offscreen scene color",
        render_width,
        render_height,
        SCENE_COLOR_FORMAT,
        TextureUsage::COLOR_ATTACHMENT | TextureUsage::SAMPLED,
    )?;
    let (scene_depth, scene_depth_view) = match create_depth_target(
        device,
        "gpui-3d offscreen scene depth",
        render_width,
        render_height,
    ) {
        Ok(targets) => targets,
        Err(error) => {
            let _ = device.destroy_texture_view(scene_color_view);
            let _ = device.destroy_texture(scene_color);
            return Err(error);
        }
    };
    let (post_color, post_color_view) = match create_color_target(
        device,
        "gpui-3d offscreen post color",
        output_width,
        output_height,
        POST_COLOR_FORMAT,
        TextureUsage::COLOR_ATTACHMENT | TextureUsage::COPY_SRC,
    ) {
        Ok(targets) => targets,
        Err(error) => {
            let _ = device.destroy_texture_view(scene_depth_view);
            let _ = device.destroy_texture(scene_depth);
            let _ = device.destroy_texture_view(scene_color_view);
            let _ = device.destroy_texture(scene_color);
            return Err(error);
        }
    };
    let (bloom_color, bloom_color_view) = match create_color_target(
        device,
        "gpui-3d offscreen bloom color",
        (render_width / 2).max(1),
        (render_height / 2).max(1),
        BLOOM_COLOR_FORMAT,
        TextureUsage::COLOR_ATTACHMENT | TextureUsage::SAMPLED,
    ) {
        Ok(targets) => targets,
        Err(error) => {
            let _ = device.destroy_texture_view(post_color_view);
            let _ = device.destroy_texture(post_color);
            let _ = device.destroy_texture_view(scene_depth_view);
            let _ = device.destroy_texture(scene_depth);
            let _ = device.destroy_texture_view(scene_color_view);
            let _ = device.destroy_texture(scene_color);
            return Err(error);
        }
    };
    Ok((
        scene_color,
        scene_color_view,
        scene_depth,
        scene_depth_view,
        post_color,
        post_color_view,
        bloom_color,
        bloom_color_view,
    ))
}
fn destroy_scene_targets<D: Device + ?Sized>(
    device: &mut D,
    scene_color: TextureId,
    scene_color_view: TextureViewId,
    scene_depth: TextureId,
    scene_depth_view: TextureViewId,
    post_color: TextureId,
    post_color_view: TextureViewId,
    bloom_color: TextureId,
    bloom_color_view: TextureViewId,
) -> Result<(), RenderError> {
    device.destroy_texture_view(bloom_color_view)?;
    device.destroy_texture(bloom_color)?;
    device.destroy_texture_view(post_color_view)?;
    device.destroy_texture(post_color)?;
    device.destroy_texture_view(scene_depth_view)?;
    device.destroy_texture(scene_depth)?;
    device.destroy_texture_view(scene_color_view)?;
    device.destroy_texture(scene_color)?;
    Ok(())
}

type ShadowTargets = (TextureId, TextureViewId, TextureId, TextureViewId);

fn create_shadow_targets<D: Device + ?Sized>(
    device: &mut D,
    resolution: u32,
) -> Result<ShadowTargets, RenderError> {
    let (color, color_view) = create_color_target(
        device,
        "gpui-3d offscreen shadow color",
        resolution,
        resolution,
        SHADOW_COLOR_FORMAT,
        TextureUsage::COLOR_ATTACHMENT | TextureUsage::SAMPLED,
    )?;
    let (depth, depth_view) = match create_depth_target(
        device,
        "gpui-3d offscreen shadow depth",
        resolution,
        resolution,
    ) {
        Ok(targets) => targets,
        Err(error) => {
            let _ = device.destroy_texture_view(color_view);
            let _ = device.destroy_texture(color);
            return Err(error);
        }
    };
    Ok((color, color_view, depth, depth_view))
}

fn destroy_shadow_targets<D: Device + ?Sized>(
    device: &mut D,
    color: TextureId,
    color_view: TextureViewId,
    depth: TextureId,
    depth_view: TextureViewId,
) -> Result<(), RenderError> {
    device.destroy_texture_view(depth_view)?;
    device.destroy_texture(depth)?;
    device.destroy_texture_view(color_view)?;
    device.destroy_texture(color)?;
    Ok(())
}

fn create_color_target<D: Device + ?Sized>(
    device: &mut D,
    label: &str,
    width: u32,
    height: u32,
    format: Format,
    usage: TextureUsage,
) -> Result<(TextureId, TextureViewId), RenderError> {
    let size = Extent2d::new(width, height)?;
    let texture = device.create_texture(&TextureDescriptor {
        label: Some(label.into()),
        size,
        mip_level_count: 1,
        format,
        usage,
        memory_location: MemoryLocation::GpuOnly,
        dimension: TextureDimension::D2,
    })?;
    let view = match device.create_texture_view(&TextureViewDescriptor {
        label: Some(format!("{label} view")),
        texture,
        base_mip_level: 0,
        mip_level_count: 1,
        format,
    }) {
        Ok(view) => view,
        Err(error) => {
            device.destroy_texture(texture)?;
            return Err(error.into());
        }
    };
    Ok((texture, view))
}

fn create_depth_target<D: Device + ?Sized>(
    device: &mut D,
    label: &str,
    width: u32,
    height: u32,
) -> Result<(TextureId, TextureViewId), RenderError> {
    let size = Extent2d::new(width, height)?;
    let texture = device.create_texture(&TextureDescriptor {
        label: Some(label.into()),
        size,
        mip_level_count: 1,
        format: DEPTH_FORMAT,
        usage: TextureUsage::DEPTH_ATTACHMENT,
        memory_location: MemoryLocation::GpuOnly,
        dimension: TextureDimension::D2,
    })?;
    let view = match device.create_texture_view(&TextureViewDescriptor {
        label: Some(format!("{label} view")),
        texture,
        base_mip_level: 0,
        mip_level_count: 1,
        format: DEPTH_FORMAT,
    }) {
        Ok(view) => view,
        Err(error) => {
            device.destroy_texture(texture)?;
            return Err(error.into());
        }
    };
    Ok((texture, view))
}

struct FrameUpload {
    bytes: u64,
    reallocated: bool,
}

/// CPU bookkeeping for one GPU buffer that only remembers per-element content hashes.
///
/// One 64-bit hash per element instead of a full CPU byte copy keeps host memory bounded for
/// large draw/instance tables while still enabling exact byte-range uploads of changed elements.
struct StrideTracker {
    stride: usize,
    len: usize,
    hashes: Vec<u64>,
}

impl StrideTracker {
    fn new(stride: usize) -> Self {
        Self {
            stride,
            len: 0,
            hashes: Vec::new(),
        }
    }

    fn discard(&mut self) {
        self.len = 0;
        self.hashes.clear();
    }

    fn cpu_bytes(&self) -> u64 {
        (self.hashes.capacity() * std::mem::size_of::<u64>()) as u64
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

struct TrackedBuffer {
    label: &'static str,
    buffer: BufferId,
    capacity: usize,
    tracker: StrideTracker,
    usage: BufferUsage,
    location: MemoryLocation,
}

impl TrackedBuffer {
    fn new<D: Device + ?Sized>(
        device: &mut D,
        label: &'static str,
        stride: u32,
        usage: BufferUsage,
        location: MemoryLocation,
    ) -> Result<Self, RenderError> {
        let (buffer, capacity) =
            create_gpu_buffer(device, label, u64::from(stride), usage, location)?;
        Ok(Self {
            label,
            buffer,
            capacity: capacity as usize,
            tracker: StrideTracker::new(stride.max(1) as usize),
            usage,
            location,
        })
    }

    fn stride(&self) -> usize {
        self.tracker.stride
    }

    /// Uploads only changed element ranges and updates the per-element hashes.
    ///
    /// Returns the uploaded byte count and whether the GPU buffer was reallocated, which
    /// invalidates resource sets that reference the old handle.
    fn sync<D: Device + ?Sized>(
        &mut self,
        device: &mut D,
        data: &[u8],
    ) -> Result<(u64, bool), RenderError> {
        if data.is_empty() {
            self.tracker.discard();
            return Ok((0, false));
        }
        let stride = self.tracker.stride;
        if !data.len().is_multiple_of(stride) {
            device.write_buffer(self.buffer, 0, data)?;
            return Ok((data.len() as u64, false));
        }
        let count = data.len() / stride;
        let mut reallocated = false;
        if data.len() > self.capacity {
            let (buffer, capacity) = create_gpu_buffer(
                device,
                self.label,
                data.len() as u64,
                self.usage,
                self.location,
            )?;
            device.destroy_buffer(self.buffer)?;
            self.buffer = buffer;
            self.capacity = capacity as usize;
            self.tracker.discard();
            reallocated = true;
        }
        if self.tracker.hashes.len() < count {
            self.tracker.hashes.resize(count, 0);
        }
        let mut uploaded = 0_u64;
        let mut run_start: Option<usize> = None;
        for element in 0..count {
            let start = element * stride;
            let mut hash = fnv1a(&data[start..start + stride]);
            if hash == 0 {
                hash = 1;
            }
            if self.tracker.hashes[element] != hash {
                self.tracker.hashes[element] = hash;
                if run_start.is_none() {
                    run_start = Some(element);
                }
            } else if let Some(first) = run_start.take() {
                uploaded += self.write_run(device, data, first, element)?;
            }
        }
        if let Some(first) = run_start {
            uploaded += self.write_run(device, data, first, count)?;
        }
        self.tracker.len = count;
        Ok((uploaded, reallocated))
    }

    fn write_run<D: Device + ?Sized>(
        &mut self,
        device: &mut D,
        data: &[u8],
        first: usize,
        end: usize,
    ) -> Result<u64, RenderError> {
        let stride = self.tracker.stride;
        let byte_start = first * stride;
        let byte_end = end * stride;
        device.write_buffer(self.buffer, byte_start as u64, &data[byte_start..byte_end])?;
        Ok((byte_end - byte_start) as u64)
    }

    fn resident_bytes(&self) -> u64 {
        self.capacity as u64
    }

    /// Recreates the buffer at the current live size when the allocation is much larger.
    ///
    /// Returns the number of bytes released and whether the buffer handle changed.
    fn shrink<D: Device + ?Sized>(&mut self, device: &mut D) -> Result<(u64, bool), RenderError> {
        let used = (self.tracker.len.max(1) * self.tracker.stride) as u64;
        let retained = used.max(4096);
        if (self.capacity as u64) <= retained.saturating_mul(2) {
            return Ok((0, false));
        }
        let (buffer, capacity) =
            create_gpu_buffer(device, self.label, used, self.usage, self.location)?;
        let released = (self.capacity.saturating_sub(capacity as usize)) as u64;
        device.destroy_buffer(self.buffer)?;
        self.buffer = buffer;
        self.capacity = capacity as usize;
        self.tracker.discard();
        Ok((released, true))
    }

    fn destroy<D: Device + ?Sized>(&mut self, device: &mut D) -> Result<(), RenderError> {
        device.destroy_buffer(self.buffer)?;
        Ok(())
    }
}

struct FrameBuffers {
    draw: TrackedBuffer,
    instance: TrackedBuffer,
    instance_bytes: u64,
    light: TrackedBuffer,
    light_bytes: u64,
    frame: TrackedBuffer,
}

impl FrameBuffers {
    fn new<D: Device + ?Sized>(device: &mut D, backend: BackendKind) -> Result<Self, RenderError> {
        let location = staging_location(backend);
        let draw = TrackedBuffer::new(
            device,
            "gpui-3d offscreen draw slots",
            DRAW_SLOT_STRIDE as u32,
            BufferUsage::STORAGE | BufferUsage::COPY_DST,
            location,
        )?;
        let instance = match TrackedBuffer::new(
            device,
            "gpui-3d offscreen instances",
            INSTANCE_STRIDE,
            BufferUsage::STORAGE | BufferUsage::COPY_DST,
            location,
        ) {
            Ok(buffer) => buffer,
            Err(error) => {
                let _ = device.destroy_buffer(draw.buffer);
                return Err(error);
            }
        };
        let light = match TrackedBuffer::new(
            device,
            "gpui-3d offscreen lights",
            LIGHT_STRIDE,
            BufferUsage::STORAGE | BufferUsage::COPY_DST,
            location,
        ) {
            Ok(buffer) => buffer,
            Err(error) => {
                let _ = device.destroy_buffer(instance.buffer);
                let _ = device.destroy_buffer(draw.buffer);
                return Err(error);
            }
        };
        let frame = match TrackedBuffer::new(
            device,
            "gpui-3d offscreen frame uniform",
            FRAME_STRIDE as u32,
            BufferUsage::UNIFORM | BufferUsage::COPY_DST,
            staging_location(backend),
        ) {
            Ok(buffer) => buffer,
            Err(error) => {
                let _ = device.destroy_buffer(light.buffer);
                let _ = device.destroy_buffer(instance.buffer);
                let _ = device.destroy_buffer(draw.buffer);
                return Err(error);
            }
        };
        Ok(Self {
            draw,
            instance,
            instance_bytes: 0,
            light,
            light_bytes: 0,
            frame,
        })
    }

    fn sync<D: Device + ?Sized>(
        &mut self,
        device: &mut D,
        draw_bytes: Option<&[u8]>,
        instance_bytes: Option<&[u8]>,
        light_bytes: Option<&[u8]>,
        frame_bytes: &[u8],
    ) -> Result<FrameUpload, RenderError> {
        let mut reallocated = false;
        let mut bytes = 0_u64;
        if let Some(draw_bytes) = draw_bytes {
            let (uploaded, grew) = self.draw.sync(device, draw_bytes)?;
            bytes += uploaded;
            reallocated |= grew;
        }
        if let Some(instance_bytes) = instance_bytes {
            let (uploaded, grew) = self.instance.sync(device, instance_bytes)?;
            bytes += uploaded;
            reallocated |= grew;
            self.instance_bytes = instance_bytes.len() as u64;
        }
        if let Some(light_bytes) = light_bytes {
            let (uploaded, grew) = self.light.sync(device, light_bytes)?;
            bytes += uploaded;
            reallocated |= grew;
            self.light_bytes = light_bytes.len() as u64;
        }
        let (frame_uploaded, frame_reallocated) = self.frame.sync(device, frame_bytes)?;
        bytes += frame_uploaded;
        reallocated |= frame_reallocated;
        Ok(FrameUpload { bytes, reallocated })
    }

    fn resident_bytes(&self) -> u64 {
        self.draw.resident_bytes()
            + self.instance.resident_bytes()
            + self.light.resident_bytes()
            + self.frame.resident_bytes()
    }

    fn cpu_tracker_bytes(&self) -> u64 {
        self.draw.tracker.cpu_bytes()
            + self.instance.tracker.cpu_bytes()
            + self.light.tracker.cpu_bytes()
            + self.frame.tracker.cpu_bytes()
    }

    fn capacity_bytes(&self) -> u64 {
        (self.draw.capacity + self.instance.capacity + self.light.capacity + self.frame.capacity)
            as u64
    }

    fn used_bytes(&self) -> u64 {
        (self.draw.tracker.len * self.draw.stride()
            + self.instance.tracker.len * self.instance.stride()
            + self.light.tracker.len * self.light.stride()
            + self.frame.tracker.len * self.frame.stride()) as u64
    }

    fn draw_capacity(&self) -> usize {
        self.draw.capacity / self.draw.stride()
    }

    fn instance_capacity(&self) -> usize {
        self.instance.capacity / self.instance.stride()
    }

    fn light_capacity(&self) -> usize {
        self.light.capacity / self.light.stride()
    }

    /// Shrinks over-allocated streaming buffers during an explicit aggressive trim.
    fn shrink<D: Device + ?Sized>(&mut self, device: &mut D) -> Result<bool, RenderError> {
        let mut changed = false;
        let mut released = 0_u64;
        let (draw_released, draw_changed) = self.draw.shrink(device)?;
        released += draw_released;
        changed |= draw_changed;
        let (instance_released, instance_changed) = self.instance.shrink(device)?;
        released += instance_released;
        changed |= instance_changed;
        let (light_released, light_changed) = self.light.shrink(device)?;
        released += light_released;
        changed |= light_changed;
        let (frame_released, frame_changed) = self.frame.shrink(device)?;
        released += frame_released;
        changed |= frame_changed;
        let _ = released;
        Ok(changed)
    }

    fn destroy<D: Device + ?Sized>(&mut self, device: &mut D) -> Result<(), RenderError> {
        self.frame.destroy(device)?;
        self.light.destroy(device)?;
        self.instance.destroy(device)?;
        self.draw.destroy(device)?;
        Ok(())
    }
}
