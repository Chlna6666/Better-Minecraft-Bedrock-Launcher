use super::{
    ALBEDO_BINDING, DRAW_BINDING, FRAME_BINDING, INSTANCE_BINDING, LIGHT_BINDING,
    NORMAL_MAP_BINDING, OCCLUSION_BINDING, SAMPLER_BINDING, TANGENT_BINDING, VERTEX_BINDING,
};
use crate::TextureSampling;
use anyhow::anyhow;
use gfx_core::{
    AddressMode, BackendKind, BlendMode, CompareFunction, DepthState, ExtensionDevice, Extent2d,
    FilterMode, Format, MemoryLocation, PipelineLayoutDescriptor, PipelineLayoutId,
    PrimitiveTopology, RenderPipelineDescriptor, RenderPipelineId, ResourceBindingType,
    ResourceSetLayoutDescriptor, ResourceSetLayoutEntry, ResourceSetLayoutId, SamplerDescriptor,
    SamplerId, ShaderBinary, ShaderModuleDescriptor, ShaderModuleId, ShaderStage, ShaderStages,
    TextureDescriptor, TextureDimension, TextureId, TextureUsage, TextureViewDescriptor,
    TextureViewId,
};
use gpui::RendererExtensionContext;

pub(super) struct RendererResources {
    resource_set_layout: Option<ResourceSetLayoutId>,
    pipeline_layout: Option<PipelineLayoutId>,
    vertex_shader: Option<ShaderModuleId>,
    fragment_shader: Option<ShaderModuleId>,
    pub(super) opaque_pipeline: Option<RenderPipelineId>,
    pub(super) blended_pipeline: Option<RenderPipelineId>,
    fallback_texture: Option<TextureId>,
    fallback_texture_view: Option<TextureViewId>,
    samplers: [Option<SamplerId>; 4],
}

impl RendererResources {
    pub(super) fn new(
        device: &mut dyn ExtensionDevice,
        context: &RendererExtensionContext,
    ) -> gpui::Result<Self> {
        let vertex_binary = embedded_shader(context, ShaderStage::Vertex, "vs_main")?;
        let fragment_binary = embedded_shader(context, ShaderStage::Fragment, "fs_main")?;
        let mut resources = Self {
            resource_set_layout: None,
            pipeline_layout: None,
            vertex_shader: None,
            fragment_shader: None,
            opaque_pipeline: None,
            blended_pipeline: None,
            fallback_texture: None,
            fallback_texture_view: None,
            samplers: [None; 4],
        };
        let result = (|| {
            resources.resource_set_layout = Some(device.create_resource_set_layout(
                &ResourceSetLayoutDescriptor {
                    label: Some("gpui-3d scene-view resource-set layout".into()),
                    entries: vec![
                        ResourceSetLayoutEntry {
                            binding: VERTEX_BINDING,
                            binding_type: ResourceBindingType::StorageBuffer,
                            stages: ShaderStages::VERTEX,
                        },
                        ResourceSetLayoutEntry {
                            binding: DRAW_BINDING,
                            binding_type: ResourceBindingType::StorageBuffer,
                            stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                        },
                        ResourceSetLayoutEntry {
                            binding: LIGHT_BINDING,
                            binding_type: ResourceBindingType::StorageBuffer,
                            stages: ShaderStages::FRAGMENT,
                        },
                        ResourceSetLayoutEntry {
                            binding: ALBEDO_BINDING,
                            binding_type: ResourceBindingType::SampledTexture,
                            stages: ShaderStages::FRAGMENT,
                        },
                        ResourceSetLayoutEntry {
                            binding: SAMPLER_BINDING,
                            binding_type: ResourceBindingType::Sampler,
                            stages: ShaderStages::FRAGMENT,
                        },
                        ResourceSetLayoutEntry {
                            binding: NORMAL_MAP_BINDING,
                            binding_type: ResourceBindingType::SampledTexture,
                            stages: ShaderStages::FRAGMENT,
                        },
                        ResourceSetLayoutEntry {
                            binding: OCCLUSION_BINDING,
                            binding_type: ResourceBindingType::SampledTexture,
                            stages: ShaderStages::FRAGMENT,
                        },
                        ResourceSetLayoutEntry {
                            binding: TANGENT_BINDING,
                            binding_type: ResourceBindingType::StorageBuffer,
                            stages: ShaderStages::VERTEX,
                        },
                        ResourceSetLayoutEntry {
                            binding: INSTANCE_BINDING,
                            binding_type: ResourceBindingType::StorageBuffer,
                            stages: ShaderStages::VERTEX,
                        },
                        ResourceSetLayoutEntry {
                            binding: FRAME_BINDING,
                            binding_type: ResourceBindingType::UniformBuffer,
                            stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                        },
                    ],
                },
            )?);
            resources.pipeline_layout =
                Some(device.create_pipeline_layout(&PipelineLayoutDescriptor {
                    label: Some("gpui-3d pipeline layout".into()),
                    resource_set_layouts: vec![resources.resource_set_layout()?],
                })?);
            resources.vertex_shader =
                Some(device.create_shader_module(&ShaderModuleDescriptor {
                    label: Some("gpui-3d vertex shader".into()),
                    binary: vertex_binary,
                })?);
            resources.fragment_shader =
                Some(device.create_shader_module(&ShaderModuleDescriptor {
                    label: Some("gpui-3d fragment shader".into()),
                    binary: fragment_binary,
                })?);
            let fallback_texture = device.create_texture(&TextureDescriptor {
                label: Some("gpui-3d fallback texture".into()),
                size: Extent2d::new(1, 1)?,
                mip_level_count: 1,
                format: Format::Rgba8UnormSrgb,
                usage: TextureUsage::SAMPLED | TextureUsage::COPY_DST,
                memory_location: MemoryLocation::GpuOnly,
                dimension: TextureDimension::D2,
            })?;
            resources.fallback_texture = Some(fallback_texture);
            resources.fallback_texture_view =
                Some(device.create_texture_view(&TextureViewDescriptor {
                    label: Some("gpui-3d fallback texture view".into()),
                    texture: fallback_texture,
                    base_mip_level: 0,
                    mip_level_count: 1,
                    format: Format::Rgba8UnormSrgb,
                })?);
            resources.replace_pipelines(device, context)
        })();
        if let Err(error) = result {
            if let Err(cleanup_error) = resources.destroy(device) {
                return Err(anyhow!(
                    "{error:#}; renderer cleanup also failed: {cleanup_error:#}"
                ));
            }
            return Err(error);
        }
        Ok(resources)
    }

    pub(super) fn resource_set_layout(&self) -> gpui::Result<ResourceSetLayoutId> {
        self.resource_set_layout
            .ok_or_else(|| anyhow!("GPUI 3D resource-set layout is unavailable"))
    }

    pub(super) fn fallback_texture_view(&self) -> gpui::Result<TextureViewId> {
        self.fallback_texture_view
            .ok_or_else(|| anyhow!("GPUI 3D fallback texture view is unavailable"))
    }

    /// Returns a shared sampler for the requested texel filtering and anisotropy.
    ///
    /// Atlas images request nearest-texel filtering so neighbouring skin or map regions cannot
    /// bleed into each other; other images keep linear filtering.
    pub(super) fn sampler(
        &mut self,
        device: &mut dyn ExtensionDevice,
        sampling: TextureSampling,
        anisotropy_enabled: bool,
    ) -> gpui::Result<SamplerId> {
        let sampler_index =
            usize::from(sampling == TextureSampling::Nearest) + 2 * usize::from(anisotropy_enabled);
        if let Some(sampler) = self.samplers[sampler_index] {
            return Ok(sampler);
        }
        let filter = match sampling {
            TextureSampling::Linear => FilterMode::Linear,
            TextureSampling::Nearest => FilterMode::Nearest,
        };
        let sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("gpui-3d material sampler".into()),
            mag_filter: filter,
            min_filter: filter,
            mipmap_filter: FilterMode::Linear,
            anisotropic: anisotropy_enabled,
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
        })?;
        self.samplers[sampler_index] = Some(sampler);
        Ok(sampler)
    }

    pub(super) fn replace_pipelines(
        &mut self,
        device: &mut dyn ExtensionDevice,
        context: &RendererExtensionContext,
    ) -> gpui::Result<()> {
        if let Some(pipeline) = self.opaque_pipeline.take() {
            device.destroy_render_pipeline(pipeline)?;
        }
        if let Some(pipeline) = self.blended_pipeline.take() {
            device.destroy_render_pipeline(pipeline)?;
        }
        let opaque = create_pipeline(device, self, context, PipelineMode::Opaque)?;
        let blended = match create_pipeline(device, self, context, PipelineMode::Blend) {
            Ok(pipeline) => pipeline,
            Err(error) => {
                device.destroy_render_pipeline(opaque)?;
                return Err(error);
            }
        };
        self.opaque_pipeline = Some(opaque);
        self.blended_pipeline = Some(blended);
        Ok(())
    }

    pub(super) fn destroy(&mut self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        if let Some(pipeline) = self.opaque_pipeline.take() {
            device.destroy_render_pipeline(pipeline)?;
        }
        if let Some(pipeline) = self.blended_pipeline.take() {
            device.destroy_render_pipeline(pipeline)?;
        }
        if let Some(shader) = self.fragment_shader.take() {
            device.destroy_shader_module(shader)?;
        }
        if let Some(shader) = self.vertex_shader.take() {
            device.destroy_shader_module(shader)?;
        }
        if let Some(view) = self.fallback_texture_view.take() {
            device.destroy_texture_view(view)?;
        }
        if let Some(texture) = self.fallback_texture.take() {
            device.destroy_texture(texture)?;
        }
        for sampler in &mut self.samplers {
            if let Some(sampler) = sampler.take() {
                device.destroy_sampler(sampler)?;
            }
        }
        if let Some(layout) = self.pipeline_layout.take() {
            device.destroy_pipeline_layout(layout)?;
        }
        if let Some(layout) = self.resource_set_layout.take() {
            device.destroy_resource_set_layout(layout)?;
        }
        Ok(())
    }
}

/// Resolves the build-generated scene-view shader for the active backend.
///
/// `build.rs` compiles every entry point for every backend the platform supports, so
/// this is a lookup rather than a shader compile while a scene view is being created.
/// The arms follow the platform, not this crate's Cargo features, because the host
/// application decides which backend renders.
fn embedded_shader(
    context: &RendererExtensionContext,
    stage: ShaderStage,
    entry_point: &str,
) -> gpui::Result<ShaderBinary> {
    let artifact = match context.backend_kind() {
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        BackendKind::OpenGl => super::artifacts::scene_view_opengl_shader(entry_point),
        #[cfg(target_os = "windows")]
        BackendKind::Dx11 => super::artifacts::scene_view_dx11_shader(entry_point),
        #[cfg(target_os = "windows")]
        BackendKind::Dx12 => super::artifacts::scene_view_dx12_shader(entry_point),
        #[cfg(target_os = "macos")]
        BackendKind::Metal => super::artifacts::scene_view_metal_shader(entry_point),
        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
        BackendKind::Vulkan => super::artifacts::scene_view_vulkan_shader(entry_point),
        _ => None,
    };

    let artifact = artifact.ok_or_else(|| {
        anyhow!(
            "gpui-3d scene-view shader `{entry_point}` is not embedded for backend {:?}",
            context.backend_kind()
        )
    })?;

    Ok(artifact.to_binary(stage, entry_point)?)
}

#[derive(Clone, Copy)]
enum PipelineMode {
    Opaque,
    Blend,
}

fn create_pipeline(
    device: &mut dyn ExtensionDevice,
    resources: &RendererResources,
    context: &RendererExtensionContext,
    mode: PipelineMode,
) -> gpui::Result<RenderPipelineId> {
    let (label, blend, depth_write) = match mode {
        PipelineMode::Opaque => ("gpui-3d opaque pipeline", false, true),
        PipelineMode::Blend => ("gpui-3d blended pipeline", true, false),
    };
    Ok(device.create_render_pipeline(
        &RenderPipelineDescriptor {
            label: Some(label.into()),
            vertex_shader: resources
                .vertex_shader
                .ok_or_else(|| anyhow!("GPUI 3D vertex shader is unavailable"))?,
            vertex_entry_point: "vs_main".into(),
            fragment_shader: resources
                .fragment_shader
                .ok_or_else(|| anyhow!("GPUI 3D fragment shader is unavailable"))?,
            fragment_entry_point: "fs_main".into(),
            vertex_buffers: Vec::new(),
            render_pass: context.render_pass(),
            pipeline_layout: resources.pipeline_layout,
            color_format: context.color_format(),
            blend_mode: if blend {
                BlendMode::PremultipliedAlpha
            } else {
                BlendMode::Replace
            },
            primitive_topology: PrimitiveTopology::TriangleList,
            depth_state: Some(DepthState {
                compare: CompareFunction::LessEqual,
                write_enabled: depth_write,
            }),
        },
        context.viewport(),
    )?)
}
