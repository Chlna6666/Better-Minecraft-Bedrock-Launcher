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

use super::{CapturedImage, FrameStatistics, RenderError, RenderOptions, RendererStatistics};
use crate::scene_view::gpu::{
    encode_draws, encode_instances, encode_lights, encode_mesh, write_matrix, write_vec4,
};
use crate::scene_view::{
    DRAW_PARAMS_STRIDE, DRAW_SLOT_STRIDE, INSTANCE_STRIDE, LIGHT_STRIDE, PACKED_VERTEX_STRIDE,
};
use crate::{
    Aabb, AlphaMode, Camera, Mat4, Mesh, MeshId, PreparedLight, PreparedScene, TextureAsset,
    TextureAssetId, TextureColorSpace, Vec3,
};
use gfx_core::{
    AddressMode, BackendKind, BindingResource, BlendMode, BufferBinding, BufferDescriptor,
    BufferId, BufferUsage, ClearColor, ColorAttachmentDescriptor, CompareFunction,
    DepthAttachmentDescriptor, DepthState, Device, DrawIndexedStepDescriptor, DrawStepDescriptor,
    Extent2d, FilterMode, Format, IndexBufferBinding, IndexFormat, LoadOp, MemoryLocation,
    MemoryTrimLevel, Origin2d, PipelineLayoutDescriptor, PipelineLayoutId, PrimitiveTopology,
    RenderPassDepthAttachment, RenderPassDescriptor, RenderPassId, RenderPipelineDescriptor,
    RenderPipelineId, RenderStepDescriptor, RenderStepList, ResourceBinding, ResourceBindingType,
    ResourceSetDescriptor, ResourceSetId, ResourceSetLayoutDescriptor, ResourceSetLayoutEntry,
    ResourceSetLayoutId, SamplerBinding, SamplerDescriptor, SamplerId, ShaderModuleDescriptor,
    ShaderModuleId, ShaderStage, ShaderStages, TextureBinding, TextureDataLayout,
    TextureDescriptor, TextureDimension, TextureId, TextureRenderStepList, TextureTransferDevice,
    TextureUsage, TextureViewDescriptor, TextureViewId, TextureWriteDescriptor,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

/// Byte stride of the scene/shadow frame uniform buffer.
///
/// WGSL declares 240 bytes of fields; the trailing bytes keep the buffer a multiple of 16.
const FRAME_STRIDE: u64 = 256;
const SHADOW_VERTEX_BINDING: u32 = 0;
const SHADOW_INSTANCE_BINDING: u32 = 1;
const SHADOW_FRAME_BINDING: u32 = 2;
const SCENE_SHADOW_MAP_BINDING: u32 = 10;
const SCENE_METALLIC_ROUGHNESS_BINDING: u32 = 11;
const POST_COLOR_BINDING: u32 = 0;
const POST_SAMPLER_BINDING: u32 = 1;
const POST_FRAME_BINDING: u32 = 2;
const DEFAULT_SHADOW_RESOLUTION: u32 = 2048;
const IDLE_MESH_FRAMES: u64 = 120;

const SHADOW_COLOR_FORMAT: Format = Format::Rgba8Unorm;
const SCENE_COLOR_FORMAT: Format = Format::Rgba8Unorm;
const POST_COLOR_FORMAT: Format = Format::Rgba8Unorm;
const DEPTH_FORMAT: Format = Format::Depth32Float;

/// Everything the renderer owns on one GPU device.
pub(crate) struct GpuState {
    backend: BackendKind,
    width: u32,
    height: u32,
    pipelines: Option<Pipelines>,
    targets: Option<Targets>,
    frame_buffers: FrameBuffers,
    meshes: HashMap<(MeshId, u64), MeshResources>,
    textures: HashMap<TextureAssetId, TextureResources>,
    fallback_texture: Option<TextureResources>,
    batch_sets: HashMap<BatchKey, BatchSets>,
    used_batch_keys: Vec<BatchKey>,
    post_set: Option<ResourceSetId>,
    resource_epoch: u64,
    frame_index: u64,
    shadow_resolution: u32,
}

impl GpuState {
    pub(crate) fn new<D: Device + ?Sized>(
        device: &mut D,
        width: u32,
        height: u32,
    ) -> Result<Self, RenderError> {
        let backend = D::BACKEND_KIND;
        let pipelines = Pipelines::new(device, backend, width, height)?;
        let targets = match Targets::new(device, width, height, DEFAULT_SHADOW_RESOLUTION) {
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
            meshes: HashMap::new(),
            textures: HashMap::new(),
            fallback_texture: Some(fallback_texture),
            batch_sets: HashMap::new(),
            used_batch_keys: Vec::new(),
            post_set: None,
            resource_epoch: 1,
            frame_index: 0,
            shadow_resolution: DEFAULT_SHADOW_RESOLUTION,
        })
    }

    pub(crate) fn backend(&self) -> BackendKind {
        self.backend
    }

    pub(crate) fn resize<D: Device + ?Sized>(
        &mut self,
        device: &mut D,
        width: u32,
        height: u32,
    ) -> Result<(), RenderError> {
        if let Some(pipelines) = self.pipelines.take() {
            pipelines.destroy(device)?;
        }
        if let Some(targets) = self.targets.as_mut() {
            targets.resize_scene(device, width, height)?;
        }
        self.pipelines = Some(Pipelines::new(device, self.backend, width, height)?);
        self.width = width;
        self.height = height;
        self.invalidate_sets(device)?;
        self.resource_epoch = self.resource_epoch.wrapping_add(1);
        Ok(())
    }

    fn set_shadow_resolution<D: Device + ?Sized>(
        &mut self,
        device: &mut D,
        resolution: u32,
    ) -> Result<(), RenderError> {
        if resolution == self.shadow_resolution {
            return Ok(());
        }
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
        if let Some(set) = self.post_set.take() {
            device.destroy_resource_set(set)?;
        }
        Ok(())
    }

    /// Renders one frame into the offscreen targets.
    pub(crate) fn render<D: Device + ?Sized>(
        &mut self,
        device: &mut D,
        prepared: &PreparedScene,
        textures: &HashMap<TextureAssetId, Arc<TextureAsset>>,
        camera: Camera,
        options: &RenderOptions,
        target: (u32, u32),
        started: Instant,
    ) -> Result<FrameStatistics, RenderError> {
        self.frame_index = self.frame_index.wrapping_add(1);
        let mut statistics = FrameStatistics::default();

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

        let frame_bytes = encode_frame(
            prepared.view_projection,
            light_view_projection,
            camera,
            directional,
            options,
            target,
            shadow_enabled,
            shadow_texel,
        );
        let draw_bytes = encode_draws(prepared).map_err(prepare_error)?;
        let instance_bytes = encode_instances(prepared).map_err(prepare_error)?;
        let light_bytes = encode_lights(&prepared.lights);
        let upload = self.frame_buffers.sync(
            device,
            &draw_bytes,
            &instance_bytes,
            &light_bytes,
            &frame_bytes,
        )?;
        statistics.upload_bytes = statistics.upload_bytes.saturating_add(upload.bytes);
        if upload.reallocated {
            self.invalidate_sets(device)?;
            self.resource_epoch = self.resource_epoch.wrapping_add(1);
        }

        self.used_batch_keys.clear();
        let mut shadow_steps = Vec::new();
        let mut scene_steps = Vec::new();
        for batch in prepared.draw_batches() {
            let batch_steps = self.batch_steps(device, batch)?;
            statistics.draw_calls += 1;
            statistics.triangles += batch_steps.triangles;
            statistics.instances += batch_steps.instances;
            if shadow_enabled && batch_steps.alpha_mode != AlphaMode::Blend {
                shadow_steps.push(batch_steps.shadow);
                statistics.shadow_draw_calls += 1;
            }
            scene_steps.push(batch_steps.scene);
        }
        self.release_unused_batch_sets(device)?;

        let post_set = self.ensure_post_set(device)?;
        let post_steps = [RenderStepDescriptor::Draw(DrawStepDescriptor {
            pipeline: self.pipelines()?.post_pipeline,
            resource_sets: gfx_core::resource_set_list([post_set]),
            vertex_count: 3,
            first_vertex: 0,
            instance_count: 1,
            first_instance: 0,
            scissor: None,
        })];

        let targets = self.targets.as_ref().ok_or(RenderError::Destroyed)?;
        let pipelines = self.pipelines()?;
        let shadow_pass = TextureRenderStepList {
            texture_view: targets.shadow_color_view,
            render_pass: pipelines.shadow_pass,
            steps: RenderStepList::Render(&shadow_steps),
            color_load_op: LoadOp::Clear(ClearColor {
                red: 1.0,
                green: 1.0,
                blue: 1.0,
                alpha: 1.0,
            }),
            clear_region: None,
            depth_attachment: Some(RenderPassDepthAttachment {
                target: targets.shadow_depth_view,
                depth_load_op: LoadOp::Clear(1.0),
            }),
            viewport: None,
        };
        let scene_pass = TextureRenderStepList {
            texture_view: targets.scene_color_view,
            render_pass: pipelines.scene_pass,
            steps: RenderStepList::Render(&scene_steps),
            color_load_op: LoadOp::Clear(ClearColor {
                red: options.clear_color[0],
                green: options.clear_color[1],
                blue: options.clear_color[2],
                alpha: options.clear_color[3],
            }),
            clear_region: None,
            depth_attachment: Some(RenderPassDepthAttachment {
                target: targets.scene_depth_view,
                depth_load_op: LoadOp::Clear(1.0),
            }),
            viewport: None,
        };
        let post_pass = TextureRenderStepList {
            texture_view: targets.post_color_view,
            render_pass: pipelines.post_pass,
            steps: RenderStepList::Render(&post_steps),
            color_load_op: LoadOp::Clear(ClearColor::default()),
            clear_region: None,
            depth_attachment: None,
            viewport: None,
        };

        let submit_started = Instant::now();
        device.render_step_lists_to_textures_compat(&[shadow_pass, scene_pass, post_pass])?;
        statistics.submit_nanos = submit_started.elapsed().as_nanos() as u64;
        statistics.cpu_prepare_nanos = started.elapsed().as_nanos() as u64;
        Ok(statistics)
    }

    /// Reads the post-processing target back as display-referred RGBA8 pixels.
    pub(crate) fn capture<D: Device + TextureTransferDevice + ?Sized>(
        &mut self,
        device: &mut D,
    ) -> Result<CapturedImage, RenderError> {
        let targets = self.targets.as_ref().ok_or(RenderError::Destroyed)?;
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
        Ok(CapturedImage {
            width: self.width,
            height: self.height,
            rgba: readback.bytes,
        })
    }

    /// Releases cached assets according to a trim level.
    pub(crate) fn trim<D: Device + ?Sized>(
        &mut self,
        device: &mut D,
        level: MemoryTrimLevel,
    ) -> Result<(), RenderError> {
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
                        destroy_mesh(device, mesh)?;
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
                        destroy_texture(device, texture)?;
                    }
                }
                true
            }
            MemoryTrimLevel::Aggressive => {
                let mesh_keys = self.meshes.keys().copied().collect::<Vec<_>>();
                for key in mesh_keys {
                    if let Some(mesh) = self.meshes.remove(&key) {
                        destroy_mesh(device, mesh)?;
                    }
                }
                let texture_keys = self.textures.keys().copied().collect::<Vec<_>>();
                for key in texture_keys {
                    if let Some(texture) = self.textures.remove(&key) {
                        destroy_texture(device, texture)?;
                    }
                }
                true
            }
        };
        if release_assets {
            self.invalidate_sets(device)?;
            self.resource_epoch = self.resource_epoch.wrapping_add(1);
        }
        Ok(())
    }

    pub(crate) fn statistics(&self) -> RendererStatistics {
        let resident_buffer_bytes = self.frame_buffers.resident_bytes();
        let resident_texture_bytes = self.targets.as_ref().map_or(0, Targets::resident_bytes)
            + self
                .fallback_texture
                .as_ref()
                .map_or(0, |texture| texture.bytes)
            + self.meshes.values().map(|mesh| mesh.bytes).sum::<u64>()
            + self
                .textures
                .values()
                .map(|texture| texture.bytes)
                .sum::<u64>();
        RendererStatistics {
            live_meshes: self.meshes.len(),
            live_textures: self.textures.len(),
            resource_set_count: self.batch_sets.len() + usize::from(self.post_set.is_some()),
            resident_buffer_bytes,
            resident_texture_bytes,
            draw_capacity: self.frame_buffers.draw_capacity / DRAW_SLOT_STRIDE,
            instance_capacity: self.frame_buffers.instance_capacity / INSTANCE_STRIDE as usize,
            light_capacity: self.frame_buffers.light_capacity / LIGHT_STRIDE as usize,
            target_width: self.width,
            target_height: self.height,
            shadow_resolution: self.shadow_resolution,
        }
    }

    pub(crate) fn destroy<D: Device + ?Sized>(
        &mut self,
        device: &mut D,
    ) -> Result<(), RenderError> {
        let mut first_error = None;
        if let Err(error) = self.invalidate_sets(device) {
            first_error = Some(error);
        }
        let mesh_keys = self.meshes.keys().copied().collect::<Vec<_>>();
        for key in mesh_keys {
            if let Some(mesh) = self.meshes.remove(&key) {
                if let Err(error) = destroy_mesh(device, mesh) {
                    first_error.get_or_insert(error);
                }
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
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
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
            return Ok(());
        }
        let resources = create_mesh(device, self.backend, mesh)?;
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
            return Ok(());
        }
        let Some(asset) = textures.get(&texture_id) else {
            return Ok(());
        };
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
            shadow: RenderStepDescriptor::DrawIndexed(DrawIndexedStepDescriptor {
                pipeline: pipelines.shadow_pipeline,
                resource_sets: gfx_core::resource_set_list([sets.shadow]),
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
            scene: RenderStepDescriptor::DrawIndexed(DrawIndexedStepDescriptor {
                pipeline: scene_pipeline,
                resource_sets: gfx_core::resource_set_list([sets.scene]),
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
        let draw_offset = u64::from(key.draw_index) * DRAW_SLOT_STRIDE as u64;
        let draw_buffer = self.frame_buffers.draw;
        let instance_buffer = self.frame_buffers.instance;
        let instance_bytes = self.frame_buffers.instance_bytes;
        let light_buffer = self.frame_buffers.light;
        let light_bytes = self.frame_buffers.light_bytes;
        let frame_buffer = self.frame_buffers.frame;
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
        let pipelines = self.pipelines()?;
        let scene_layout = pipelines.scene_layout;
        let shadow_layout = pipelines.shadow_layout;
        let material_sampler = pipelines.material_sampler;
        let shadow_map_view = self
            .targets
            .as_ref()
            .ok_or(RenderError::Destroyed)?
            .shadow_color_view;
        let (tangent_buffer, tangent_offset, tangent_size) = match tangents {
            Some(tangents) => (tangents, 0, tangent_bytes.max(16)),
            None => (draw_buffer, draw_offset, 16),
        };
        let scene = device.create_resource_set(&ResourceSetDescriptor {
            label: Some("gpui-3d offscreen scene resource set".into()),
            layout: scene_layout,
            bindings: vec![
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
            ],
        })?;
        let shadow = match device.create_resource_set(&ResourceSetDescriptor {
            label: Some("gpui-3d offscreen shadow resource set".into()),
            layout: shadow_layout,
            bindings: vec![
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
                device.destroy_resource_set(scene)?;
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
    ) -> Result<ResourceSetId, RenderError> {
        if let Some(set) = self.post_set {
            return Ok(set);
        }
        let targets = self.targets.as_ref().ok_or(RenderError::Destroyed)?;
        let pipelines = self.pipelines()?;
        let set = device.create_resource_set(&ResourceSetDescriptor {
            label: Some("gpui-3d offscreen post resource set".into()),
            layout: pipelines.post_layout,
            bindings: vec![
                texture_binding(POST_COLOR_BINDING, targets.scene_color_view),
                sampler_binding(POST_SAMPLER_BINDING, pipelines.post_sampler),
                buffer_binding(
                    POST_FRAME_BINDING,
                    self.frame_buffers.frame,
                    0,
                    FRAME_STRIDE,
                    None,
                ),
            ],
        })?;
        self.post_set = Some(set);
        Ok(set)
    }
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
) -> ResourceBinding {
    ResourceBinding {
        binding,
        resource: BindingResource::Buffer(BufferBinding {
            buffer,
            offset,
            size,
            stride,
        }),
    }
}

fn texture_binding(binding: u32, texture_view: TextureViewId) -> ResourceBinding {
    ResourceBinding {
        binding,
        resource: BindingResource::Texture(TextureBinding { texture_view }),
    }
}

fn sampler_binding(binding: u32, sampler: SamplerId) -> ResourceBinding {
    ResourceBinding {
        binding,
        resource: BindingResource::Sampler(SamplerBinding { sampler }),
    }
}

struct BatchSteps {
    shadow: RenderStepDescriptor,
    scene: RenderStepDescriptor,
    alpha_mode: AlphaMode,
    triangles: u64,
    instances: u64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct BatchKey {
    mesh_id: MeshId,
    generation: u64,
    part_first_index: u32,
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
    scene: ResourceSetId,
    shadow: ResourceSetId,
}

fn destroy_batch_sets<D: Device + ?Sized>(
    device: &mut D,
    sets: BatchSets,
) -> Result<(), RenderError> {
    device.destroy_resource_set(sets.shadow)?;
    device.destroy_resource_set(sets.scene)?;
    Ok(())
}

struct MeshResources {
    vertices: BufferId,
    vertex_bytes: u64,
    tangents: Option<BufferId>,
    tangent_bytes: u64,
    indices: BufferId,
    bytes: u64,
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

fn encode_frame(
    view_projection: Mat4,
    light_view_projection: Mat4,
    camera: Camera,
    directional: Option<(Vec3, [f32; 3], f32)>,
    options: &RenderOptions,
    target: (u32, u32),
    shadow_enabled: bool,
    shadow_texel: f32,
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(FRAME_STRIDE as usize);
    write_matrix(&mut bytes, view_projection.columns());
    write_matrix(&mut bytes, light_view_projection.columns());
    write_vec4(&mut bytes, [camera.eye.x, camera.eye.y, camera.eye.z, 0.0]);
    let (direction, color, intensity) =
        directional.unwrap_or((Vec3::new(0.0, -1.0, 0.0), [1.0, 1.0, 1.0], 1.0));
    write_vec4(
        &mut bytes,
        [direction.x, direction.y, direction.z, intensity],
    );
    write_vec4(&mut bytes, [color[0], color[1], color[2], 0.0]);
    write_vec4(&mut bytes, [0.0, 0.0, 0.0, 0.0]);
    write_vec4(
        &mut bytes,
        [
            options.exposure,
            options.tonemap.shader_value() as f32,
            options.debug_view.shader_value() as f32,
            options.shadows.strength.clamp(0.0, 1.0),
        ],
    );
    write_vec4(
        &mut bytes,
        [
            shadow_texel,
            shadow_texel,
            options.shadows.depth_bias,
            if shadow_enabled { 1.0 } else { 0.0 },
        ],
    );
    write_vec4(
        &mut bytes,
        [
            target.0 as f32,
            target.1 as f32,
            options.shadows.normal_bias,
            if options.post_process { 1.0 } else { 0.0 },
        ],
    );
    bytes.resize(FRAME_STRIDE as usize, 0);
    bytes
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
) -> Result<MeshResources, RenderError> {
    let encoded = encode_mesh(mesh).map_err(prepare_error)?;
    let location = staging_location(backend);
    let vertex_bytes = encoded.vertices.len() as u64;
    let (vertices, vertex_capacity) = create_gpu_buffer(
        device,
        "gpui-3d offscreen vertices",
        vertex_bytes,
        BufferUsage::STORAGE | BufferUsage::COPY_DST,
        location,
    )?;
    if let Err(error) = device.write_buffer(vertices, 0, &encoded.vertices) {
        device.destroy_buffer(vertices)?;
        return Err(error.into());
    }
    let tangent_bytes = encoded
        .tangents
        .as_ref()
        .map_or(0_u64, |tangents| tangents.len() as u64);
    let tangents = if let Some(tangent_data) = &encoded.tangents {
        let (buffer, capacity) = create_gpu_buffer(
            device,
            "gpui-3d offscreen tangents",
            tangent_bytes,
            BufferUsage::STORAGE | BufferUsage::COPY_DST,
            location,
        )?;
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
    let (indices, index_capacity) = create_gpu_buffer(
        device,
        "gpui-3d offscreen indices",
        index_bytes,
        BufferUsage::INDEX | BufferUsage::COPY_DST,
        location,
    )?;
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
        tangents: tangents.map(|(buffer, _)| buffer),
        tangent_bytes,
        indices,
        bytes,
        last_used_frame: 0,
    })
}

fn destroy_mesh<D: Device + ?Sized>(
    device: &mut D,
    mesh: MeshResources,
) -> Result<(), RenderError> {
    device.destroy_buffer(mesh.indices)?;
    if let Some(tangents) = mesh.tangents {
        device.destroy_buffer(tangents)?;
    }
    device.destroy_buffer(mesh.vertices)?;
    Ok(())
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
        if let Err(error) = device.write_texture(
            TextureWriteDescriptor {
                texture,
                mip_level,
                layout,
                origin: Origin2d { x: 0, y: 0 },
                size: mip_size,
            },
            data,
        ) {
            device.destroy_texture(texture)?;
            return Err(error.into());
        }
        bytes += u64::from(width) * u64::from(height) * 4;
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
    layouts: Vec<ResourceSetLayoutId>,
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
            let _ = device.destroy_resource_set_layout(layout);
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
    shadow_layout: ResourceSetLayoutId,
    scene_layout: ResourceSetLayoutId,
    post_layout: ResourceSetLayoutId,
    shadow_pass: RenderPassId,
    scene_pass: RenderPassId,
    post_pass: RenderPassId,
    shadow_pipeline: RenderPipelineId,
    opaque_pipeline: RenderPipelineId,
    blended_pipeline: RenderPipelineId,
    post_pipeline: RenderPipelineId,
    material_sampler: SamplerId,
    post_sampler: SamplerId,
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
        let shadow_layout = device.create_resource_set_layout(&ResourceSetLayoutDescriptor {
            label: Some("gpui-3d offscreen shadow layout".into()),
            entries: vec![
                layout_entry(
                    SHADOW_VERTEX_BINDING,
                    ResourceBindingType::StorageBuffer,
                    ShaderStages::VERTEX,
                ),
                layout_entry(
                    SHADOW_INSTANCE_BINDING,
                    ResourceBindingType::StorageBuffer,
                    ShaderStages::VERTEX,
                ),
                layout_entry(
                    SHADOW_FRAME_BINDING,
                    ResourceBindingType::UniformBuffer,
                    ShaderStages::VERTEX,
                ),
            ],
        })?;
        scratch.layouts.push(shadow_layout);
        let scene_layout = device.create_resource_set_layout(&ResourceSetLayoutDescriptor {
            label: Some("gpui-3d offscreen scene layout".into()),
            entries: vec![
                layout_entry(
                    crate::scene_view::VERTEX_BINDING,
                    ResourceBindingType::StorageBuffer,
                    ShaderStages::VERTEX,
                ),
                layout_entry(
                    crate::scene_view::DRAW_BINDING,
                    ResourceBindingType::StorageBuffer,
                    ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                ),
                layout_entry(
                    crate::scene_view::LIGHT_BINDING,
                    ResourceBindingType::StorageBuffer,
                    ShaderStages::FRAGMENT,
                ),
                layout_entry(
                    crate::scene_view::ALBEDO_BINDING,
                    ResourceBindingType::SampledTexture,
                    ShaderStages::FRAGMENT,
                ),
                layout_entry(
                    crate::scene_view::SAMPLER_BINDING,
                    ResourceBindingType::Sampler,
                    ShaderStages::FRAGMENT,
                ),
                layout_entry(
                    crate::scene_view::NORMAL_MAP_BINDING,
                    ResourceBindingType::SampledTexture,
                    ShaderStages::FRAGMENT,
                ),
                layout_entry(
                    crate::scene_view::OCCLUSION_BINDING,
                    ResourceBindingType::SampledTexture,
                    ShaderStages::FRAGMENT,
                ),
                layout_entry(
                    crate::scene_view::TANGENT_BINDING,
                    ResourceBindingType::StorageBuffer,
                    ShaderStages::VERTEX,
                ),
                layout_entry(
                    crate::scene_view::INSTANCE_BINDING,
                    ResourceBindingType::StorageBuffer,
                    ShaderStages::VERTEX,
                ),
                layout_entry(
                    crate::scene_view::FRAME_BINDING,
                    ResourceBindingType::UniformBuffer,
                    ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                ),
                layout_entry(
                    SCENE_SHADOW_MAP_BINDING,
                    ResourceBindingType::SampledTexture,
                    ShaderStages::FRAGMENT,
                ),
                layout_entry(
                    SCENE_METALLIC_ROUGHNESS_BINDING,
                    ResourceBindingType::SampledTexture,
                    ShaderStages::FRAGMENT,
                ),
            ],
        })?;
        scratch.layouts.push(scene_layout);
        let post_layout = device.create_resource_set_layout(&ResourceSetLayoutDescriptor {
            label: Some("gpui-3d offscreen post layout".into()),
            entries: vec![
                layout_entry(
                    POST_COLOR_BINDING,
                    ResourceBindingType::SampledTexture,
                    ShaderStages::FRAGMENT,
                ),
                layout_entry(
                    POST_SAMPLER_BINDING,
                    ResourceBindingType::Sampler,
                    ShaderStages::FRAGMENT,
                ),
                layout_entry(
                    POST_FRAME_BINDING,
                    ResourceBindingType::UniformBuffer,
                    ShaderStages::FRAGMENT,
                ),
            ],
        })?;
        scratch.layouts.push(post_layout);

        let shadow_pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("gpui-3d offscreen shadow pipeline layout".into()),
            resource_set_layouts: vec![shadow_layout],
        })?;
        scratch.pipeline_layouts.push(shadow_pipeline_layout);
        let scene_pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("gpui-3d offscreen scene pipeline layout".into()),
            resource_set_layouts: vec![scene_layout],
        })?;
        scratch.pipeline_layouts.push(scene_pipeline_layout);
        let post_pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("gpui-3d offscreen post pipeline layout".into()),
            resource_set_layouts: vec![post_layout],
        })?;
        scratch.pipeline_layouts.push(post_pipeline_layout);

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

        Ok(Self {
            shadow_layout,
            scene_layout,
            post_layout,
            shadow_pass,
            scene_pass,
            post_pass,
            shadow_pipeline,
            opaque_pipeline,
            blended_pipeline,
            post_pipeline,
            material_sampler,
            post_sampler,
            shaders: vec![
                shadow_vertex_shader,
                shadow_fragment_shader,
                scene_vertex_shader,
                scene_fragment_shader,
                post_vertex_shader,
                post_fragment_shader,
            ],
        })
    }

    fn destroy<D: Device + ?Sized>(self, device: &mut D) -> Result<(), RenderError> {
        device.destroy_render_pipeline(self.shadow_pipeline)?;
        device.destroy_render_pipeline(self.opaque_pipeline)?;
        device.destroy_render_pipeline(self.blended_pipeline)?;
        device.destroy_render_pipeline(self.post_pipeline)?;
        device.destroy_sampler(self.material_sampler)?;
        device.destroy_sampler(self.post_sampler)?;
        for shader in self.shaders {
            device.destroy_shader_module(shader)?;
        }
        device.destroy_render_pass(self.shadow_pass)?;
        device.destroy_render_pass(self.scene_pass)?;
        device.destroy_render_pass(self.post_pass)?;
        device.destroy_resource_set_layout(self.shadow_layout)?;
        device.destroy_resource_set_layout(self.scene_layout)?;
        device.destroy_resource_set_layout(self.post_layout)?;
        Ok(())
    }
}

fn layout_entry(
    binding: u32,
    binding_type: ResourceBindingType,
    stages: ShaderStages,
) -> ResourceSetLayoutEntry {
    ResourceSetLayoutEntry {
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
    width: u32,
    height: u32,
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
}

impl Targets {
    fn new<D: Device + ?Sized>(
        device: &mut D,
        width: u32,
        height: u32,
        shadow_resolution: u32,
    ) -> Result<Self, RenderError> {
        let (shadow_color, shadow_color_view, shadow_depth, shadow_depth_view) =
            create_shadow_targets(device, shadow_resolution)?;
        let (
            scene_color,
            scene_color_view,
            scene_depth,
            scene_depth_view,
            post_color,
            post_color_view,
        ) = match create_scene_targets(device, width, height) {
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
        })
    }

    fn resize_scene<D: Device + ?Sized>(
        &mut self,
        device: &mut D,
        width: u32,
        height: u32,
    ) -> Result<(), RenderError> {
        destroy_scene_targets(
            device,
            self.scene_color,
            self.scene_color_view,
            self.scene_depth,
            self.scene_depth_view,
            self.post_color,
            self.post_color_view,
        )?;
        let (
            scene_color,
            scene_color_view,
            scene_depth,
            scene_depth_view,
            post_color,
            post_color_view,
        ) = create_scene_targets(device, width, height)?;
        self.width = width;
        self.height = height;
        self.scene_color = scene_color;
        self.scene_color_view = scene_color_view;
        self.scene_depth = scene_depth;
        self.scene_depth_view = scene_depth_view;
        self.post_color = post_color;
        self.post_color_view = post_color_view;
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
        let pixels = u64::from(self.width) * u64::from(self.height);
        let shadow_pixels = u64::from(self.shadow_resolution) * u64::from(self.shadow_resolution);
        // Shadow color (4) + depth (4), scene color (4) + depth (4), post color (4).
        shadow_pixels * 8 + pixels * 12
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
);

fn create_scene_targets<D: Device + ?Sized>(
    device: &mut D,
    width: u32,
    height: u32,
) -> Result<SceneTargets, RenderError> {
    let (scene_color, scene_color_view) = create_color_target(
        device,
        "gpui-3d offscreen scene color",
        width,
        height,
        SCENE_COLOR_FORMAT,
        TextureUsage::COLOR_ATTACHMENT | TextureUsage::SAMPLED,
    )?;
    let (scene_depth, scene_depth_view) =
        match create_depth_target(device, "gpui-3d offscreen scene depth", width, height) {
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
        width,
        height,
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
    Ok((
        scene_color,
        scene_color_view,
        scene_depth,
        scene_depth_view,
        post_color,
        post_color_view,
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
) -> Result<(), RenderError> {
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

struct FrameBuffers {
    backend: BackendKind,
    draw: BufferId,
    draw_capacity: usize,
    draw_last: Option<Vec<u8>>,
    instance: BufferId,
    instance_capacity: usize,
    instance_bytes: u64,
    instance_last: Option<Vec<u8>>,
    light: BufferId,
    light_capacity: usize,
    light_bytes: u64,
    light_last: Option<Vec<u8>>,
    frame: BufferId,
    frame_last: Option<Vec<u8>>,
}

impl FrameBuffers {
    fn new<D: Device + ?Sized>(device: &mut D, backend: BackendKind) -> Result<Self, RenderError> {
        let location = staging_location(backend);
        let mut created = HandleScratch::default();
        let result = (|| {
            let (draw, draw_size) = create_gpu_buffer(
                device,
                "gpui-3d offscreen draw slots",
                DRAW_SLOT_STRIDE as u64,
                BufferUsage::STORAGE | BufferUsage::COPY_DST,
                location,
            )?;
            created.buffers.push(draw);
            let (instance, instance_size) = create_gpu_buffer(
                device,
                "gpui-3d offscreen instances",
                u64::from(INSTANCE_STRIDE),
                BufferUsage::STORAGE | BufferUsage::COPY_DST,
                location,
            )?;
            created.buffers.push(instance);
            let (light, light_size) = create_gpu_buffer(
                device,
                "gpui-3d offscreen lights",
                u64::from(LIGHT_STRIDE),
                BufferUsage::STORAGE | BufferUsage::COPY_DST,
                location,
            )?;
            created.buffers.push(light);
            let (frame, _) = create_gpu_buffer(
                device,
                "gpui-3d offscreen frame uniform",
                FRAME_STRIDE,
                BufferUsage::UNIFORM | BufferUsage::COPY_DST,
                staging_location(backend),
            )?;
            created.buffers.push(frame);
            Ok(Self {
                backend,
                draw,
                draw_capacity: draw_size as usize,
                draw_last: None,
                instance,
                instance_capacity: instance_size as usize,
                instance_bytes: 0,
                instance_last: None,
                light,
                light_capacity: light_size as usize,
                light_bytes: 0,
                light_last: None,
                frame,
                frame_last: None,
            })
        })();
        match result {
            Ok(buffers) => Ok(buffers),
            Err(error) => {
                created.destroy(device);
                Err(error)
            }
        }
    }

    fn sync<D: Device + ?Sized>(
        &mut self,
        device: &mut D,
        draw_bytes: &[u8],
        instance_bytes: &[u8],
        light_bytes: &[u8],
        frame_bytes: &[u8],
    ) -> Result<FrameUpload, RenderError> {
        let mut reallocated = false;
        if draw_bytes.len() > self.draw_capacity {
            let (buffer, capacity) = create_gpu_buffer(
                device,
                "gpui-3d offscreen draw slots",
                draw_bytes.len() as u64,
                BufferUsage::STORAGE | BufferUsage::COPY_DST,
                staging_location(self.backend),
            )?;
            device.destroy_buffer(self.draw)?;
            self.draw = buffer;
            self.draw_capacity = capacity as usize;
            self.draw_last = None;
            reallocated = true;
        }
        if instance_bytes.len() > self.instance_capacity {
            let (buffer, capacity) = create_gpu_buffer(
                device,
                "gpui-3d offscreen instances",
                instance_bytes.len() as u64,
                BufferUsage::STORAGE | BufferUsage::COPY_DST,
                staging_location(self.backend),
            )?;
            device.destroy_buffer(self.instance)?;
            self.instance = buffer;
            self.instance_capacity = capacity as usize;
            self.instance_last = None;
            reallocated = true;
        }
        if light_bytes.len() > self.light_capacity {
            let (buffer, capacity) = create_gpu_buffer(
                device,
                "gpui-3d offscreen lights",
                light_bytes.len() as u64,
                BufferUsage::STORAGE | BufferUsage::COPY_DST,
                staging_location(self.backend),
            )?;
            device.destroy_buffer(self.light)?;
            self.light = buffer;
            self.light_capacity = capacity as usize;
            self.light_last = None;
            reallocated = true;
        }
        let mut uploaded = 0;
        uploaded += upload_bytes(device, self.draw, &mut self.draw_last, draw_bytes)?;
        uploaded += upload_bytes(
            device,
            self.instance,
            &mut self.instance_last,
            instance_bytes,
        )?;
        uploaded += upload_bytes(device, self.light, &mut self.light_last, light_bytes)?;
        uploaded += upload_bytes(device, self.frame, &mut self.frame_last, frame_bytes)?;
        self.instance_bytes = instance_bytes.len() as u64;
        self.light_bytes = light_bytes.len() as u64;
        Ok(FrameUpload {
            bytes: uploaded,
            reallocated,
        })
    }

    fn resident_bytes(&self) -> u64 {
        self.draw_capacity as u64
            + self.instance_capacity as u64
            + self.light_capacity as u64
            + FRAME_STRIDE
    }

    fn destroy<D: Device + ?Sized>(&mut self, device: &mut D) -> Result<(), RenderError> {
        device.destroy_buffer(self.frame)?;
        device.destroy_buffer(self.light)?;
        device.destroy_buffer(self.instance)?;
        device.destroy_buffer(self.draw)?;
        Ok(())
    }
}

fn upload_bytes<D: Device + ?Sized>(
    device: &mut D,
    buffer: BufferId,
    last: &mut Option<Vec<u8>>,
    data: &[u8],
) -> Result<u64, RenderError> {
    if last.as_deref() == Some(data) {
        return Ok(0);
    }
    device.write_buffer(buffer, 0, data)?;
    *last = Some(data.to_vec());
    Ok(data.len() as u64)
}
