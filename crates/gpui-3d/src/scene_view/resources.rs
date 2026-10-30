use super::{
    ALBEDO_BINDING, DRAW_BINDING, FRAME_BINDING, INSTANCE_BINDING, LIGHT_BINDING,
    METALLIC_ROUGHNESS_BINDING, NORMAL_MAP_BINDING, OCCLUSION_BINDING, SAMPLER_BINDING,
    TANGENT_BINDING, VERTEX_BINDING, VISIBLE_INSTANCE_BINDING,
};
use crate::TextureSampling;
use anyhow::anyhow;
use gfx_core::{
    AddressMode, BackendKind, BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindGroupLayoutId,
    BindingType, BlendMode, CompareFunction, DepthState, Extent2d, FilterMode, Format,
    MemoryLocation, PipelineLayoutDescriptor, PipelineLayoutId, PrimitiveTopology,
    RenderPipelineDescriptor, RenderPipelineId, SamplerDescriptor, SamplerId, ShaderBinary,
    ShaderModuleDescriptor, ShaderModuleId, ShaderStage, ShaderStages, TextureDescriptor,
    TextureDimension, TextureId, TextureUsage, TextureViewDescriptor, TextureViewId,
};
use gpui::ExtensionDevice;
use gpui::RendererExtensionContext;

pub(super) struct RendererResources {
    pub(super) cull_shader: Option<ShaderModuleId>,
    bind_group_layout: Option<BindGroupLayoutId>,
    pipeline_layout: Option<PipelineLayoutId>,
    vertex_shader: Option<ShaderModuleId>,
    voxel_vertex_shader: Option<ShaderModuleId>,
    generated_vertex_shader: Option<ShaderModuleId>,
    fragment_shader: Option<ShaderModuleId>,
    voxel_fragment_shader: Option<ShaderModuleId>,
    pipelines: [Option<RenderPipelineId>; 18],
    fallback_texture: Option<TextureId>,
    fallback_texture_view: Option<TextureViewId>,
    samplers: [Option<SamplerId>; 6],
}

impl RendererResources {
    #[cfg(test)]
    pub(super) fn test_pipeline(
        layout: BindGroupLayoutId,
        pipeline: RenderPipelineId,
        fallback: TextureViewId,
    ) -> Self {
        Self {
            cull_shader: None,
            bind_group_layout: Some(layout),
            pipeline_layout: None,
            vertex_shader: None,
            voxel_vertex_shader: None,
            generated_vertex_shader: None,
            fragment_shader: None,
            voxel_fragment_shader: None,
            pipelines: [Some(pipeline); 18],
            fallback_texture: None,
            fallback_texture_view: Some(fallback),
            samplers: [None; 6],
        }
    }
    pub(super) fn new(
        device: &mut dyn ExtensionDevice,
        context: &RendererExtensionContext,
    ) -> gpui::Result<Self> {
        let vertex_binary = embedded_shader(context, ShaderStage::Vertex, "vs_main")?;
        let compute = device.compute_capabilities();
        let voxel_entry = if compute.indirect_draw && !compute.max_workgroups.contains(&0) {
            "vs_voxel_gpu"
        } else {
            "vs_voxel"
        };
        let voxel_vertex_binary = embedded_shader(context, ShaderStage::Vertex, voxel_entry)?;
        let fragment_binary = embedded_shader(context, ShaderStage::Fragment, "fs_main")?;
        let voxel_fragment_binary = embedded_shader(context, ShaderStage::Fragment, "fs_voxel")?;
        let mut resources = Self {
            cull_shader: None,
            bind_group_layout: None,
            pipeline_layout: None,
            vertex_shader: None,
            voxel_vertex_shader: None,
            generated_vertex_shader: None,
            fragment_shader: None,
            voxel_fragment_shader: None,
            pipelines: [None; 18],
            fallback_texture: None,
            fallback_texture_view: None,
            samplers: [None; 6],
        };
        let result = (|| {
            resources.generated_vertex_shader =
                Some(device.create_shader_module(&ShaderModuleDescriptor {
                    label: None,
                    binary: embedded_shader(context, ShaderStage::Vertex, "vs_generated_voxel")?,
                })?);
            let compute = device.compute_capabilities();
            if compute.indirect_draw && !compute.max_workgroups.contains(&0) {
                resources.cull_shader =
                    Some(device.create_shader_module(&ShaderModuleDescriptor {
                        label: Some("gpui-3d GPU batch culling".into()),
                        binary: embedded_shader(context, ShaderStage::Compute, "cs_cull")?,
                    })?);
            }
            resources.bind_group_layout =
                Some(device.create_bind_group_layout(&BindGroupLayoutDescriptor {
                    label: Some("gpui-3d scene-view resource-set layout".into()),
                    entries: vec![
                        BindGroupLayoutEntry {
                            binding: 13,
                            binding_type: BindingType::StorageBuffer,
                            stages: ShaderStages::VERTEX,
                        },
                        BindGroupLayoutEntry {
                            binding: 12,
                            binding_type: BindingType::StorageBuffer,
                            stages: ShaderStages::VERTEX,
                        },
                        BindGroupLayoutEntry {
                            binding: VISIBLE_INSTANCE_BINDING,
                            binding_type: BindingType::StorageBuffer,
                            stages: ShaderStages::VERTEX,
                        },
                        BindGroupLayoutEntry {
                            binding: VERTEX_BINDING,
                            binding_type: BindingType::StorageBuffer,
                            stages: ShaderStages::VERTEX,
                        },
                        BindGroupLayoutEntry {
                            binding: DRAW_BINDING,
                            binding_type: BindingType::StorageBuffer,
                            stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                        },
                        BindGroupLayoutEntry {
                            binding: LIGHT_BINDING,
                            binding_type: BindingType::StorageBuffer,
                            stages: ShaderStages::FRAGMENT,
                        },
                        BindGroupLayoutEntry {
                            binding: ALBEDO_BINDING,
                            binding_type: BindingType::SampledTexture,
                            stages: ShaderStages::FRAGMENT,
                        },
                        BindGroupLayoutEntry {
                            binding: SAMPLER_BINDING,
                            binding_type: BindingType::Sampler,
                            stages: ShaderStages::FRAGMENT,
                        },
                        BindGroupLayoutEntry {
                            binding: NORMAL_MAP_BINDING,
                            binding_type: BindingType::SampledTexture,
                            stages: ShaderStages::FRAGMENT,
                        },
                        BindGroupLayoutEntry {
                            binding: OCCLUSION_BINDING,
                            binding_type: BindingType::SampledTexture,
                            stages: ShaderStages::FRAGMENT,
                        },
                        BindGroupLayoutEntry {
                            binding: METALLIC_ROUGHNESS_BINDING,
                            binding_type: BindingType::SampledTexture,
                            stages: ShaderStages::FRAGMENT,
                        },
                        BindGroupLayoutEntry {
                            binding: TANGENT_BINDING,
                            binding_type: BindingType::StorageBuffer,
                            stages: ShaderStages::VERTEX,
                        },
                        BindGroupLayoutEntry {
                            binding: INSTANCE_BINDING,
                            binding_type: BindingType::StorageBuffer,
                            stages: ShaderStages::VERTEX,
                        },
                        BindGroupLayoutEntry {
                            binding: FRAME_BINDING,
                            binding_type: BindingType::UniformBuffer,
                            stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                        },
                    ],
                })?);
            resources.pipeline_layout =
                Some(device.create_pipeline_layout(&PipelineLayoutDescriptor {
                    label: Some("gpui-3d pipeline layout".into()),
                    bind_group_layouts: vec![resources.bind_group_layout()?],
                })?);
            resources.vertex_shader =
                Some(device.create_shader_module(&ShaderModuleDescriptor {
                    label: Some("gpui-3d vertex shader".into()),
                    binary: vertex_binary,
                })?);
            resources.voxel_vertex_shader =
                Some(device.create_shader_module(&ShaderModuleDescriptor {
                    label: Some("gpui-3d voxel vertex shader".into()),
                    binary: voxel_vertex_binary,
                })?);
            resources.fragment_shader =
                Some(device.create_shader_module(&ShaderModuleDescriptor {
                    label: Some("gpui-3d fragment shader".into()),
                    binary: fragment_binary,
                })?);
            resources.voxel_fragment_shader =
                Some(device.create_shader_module(&ShaderModuleDescriptor {
                    label: Some("gpui-3d voxel fragment shader".into()),
                    binary: voxel_fragment_binary,
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

    pub(super) fn bind_group_layout(&self) -> gpui::Result<BindGroupLayoutId> {
        self.bind_group_layout
            .ok_or_else(|| anyhow!("GPUI 3D resource-set layout is unavailable"))
    }

    pub(super) fn fallback_texture_view(&self) -> gpui::Result<TextureViewId> {
        self.fallback_texture_view
            .ok_or_else(|| anyhow!("GPUI 3D fallback texture view is unavailable"))
    }

    /// Returns a shared sampler for the requested texel filtering and anisotropy.
    ///
    /// Atlas images request nearest-texel filtering so neighbouring skin or map regions cannot
    /// bleed into each other; pixel-art scenes use nearest magnification with linear
    /// minification; other images keep linear filtering.
    pub(super) fn sampler(
        &mut self,
        device: &mut dyn ExtensionDevice,
        sampling: TextureSampling,
        anisotropy_enabled: bool,
    ) -> gpui::Result<SamplerId> {
        let sampler_index = sampler_slot(sampling) + 3 * usize::from(anisotropy_enabled);
        if let Some(sampler) = self.samplers[sampler_index] {
            return Ok(sampler);
        }
        let (mag_filter, min_filter) = sampler_filters(sampling);
        let sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("gpui-3d material sampler".into()),
            mag_filter,
            min_filter,
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
        let mut next = [None; 18];
        for (index, slot) in next.iter_mut().enumerate() {
            if index >= 12 && self.cull_shader.is_none() {
                continue;
            }
            let mode = if (index / 3) % 2 == 0 {
                PipelineMode::Opaque
            } else {
                PipelineMode::Blend
            };
            let face = match index % 3 {
                0 => None,
                1 => Some(gfx_core::FrontFace::Ccw),
                _ => Some(gfx_core::FrontFace::Cw),
            };
            match create_pipeline(device, self, context, mode, index / 6, face) {
                Ok(pipeline) => *slot = Some(pipeline),
                Err(error) => {
                    for pipeline in next.into_iter().flatten() {
                        device.destroy_render_pipeline(pipeline)?;
                    }
                    return Err(error);
                }
            }
        }
        for pipeline in std::mem::replace(&mut self.pipelines, next)
            .into_iter()
            .flatten()
        {
            device.destroy_render_pipeline(pipeline)?;
        }
        Ok(())
    }

    pub(super) fn pipeline(
        &self,
        voxel: bool,
        blended: bool,
        front_face: Option<gfx_core::FrontFace>,
    ) -> gpui::Result<RenderPipelineId> {
        self.pipeline_variant(usize::from(voxel), blended, front_face)
    }
    pub(super) fn generated_pipeline(
        &self,
        blended: bool,
        front_face: Option<gfx_core::FrontFace>,
    ) -> gpui::Result<RenderPipelineId> {
        self.pipeline_variant(2, blended, front_face)
    }
    fn pipeline_variant(
        &self,
        variant: usize,
        blended: bool,
        front_face: Option<gfx_core::FrontFace>,
    ) -> gpui::Result<RenderPipelineId> {
        let face = match front_face {
            None => 0,
            Some(gfx_core::FrontFace::Ccw) => 1,
            Some(gfx_core::FrontFace::Cw) => 2,
        };
        self.pipelines[variant * 6 + usize::from(blended) * 3 + face]
            .ok_or_else(|| anyhow!("GPUI 3D draw pipeline is unavailable"))
    }
    pub(super) fn destroy(&mut self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        if let Some(shader) = self.cull_shader.take() {
            device.destroy_shader_module(shader)?;
        }
        for pipeline in &mut self.pipelines {
            if let Some(pipeline) = pipeline.take() {
                device.destroy_render_pipeline(pipeline)?;
            }
        }
        if let Some(shader) = self.generated_vertex_shader.take() {
            device.destroy_shader_module(shader)?;
        }
        if let Some(shader) = self.voxel_fragment_shader.take() {
            device.destroy_shader_module(shader)?;
        }
        if let Some(shader) = self.fragment_shader.take() {
            device.destroy_shader_module(shader)?;
        }
        if let Some(shader) = self.vertex_shader.take() {
            device.destroy_shader_module(shader)?;
        }
        if let Some(shader) = self.voxel_vertex_shader.take() {
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
        if let Some(layout) = self.bind_group_layout.take() {
            device.destroy_bind_group_layout(layout)?;
        }
        Ok(())
    }
}

/// Stable slot index for one material sampling mode inside the sampler cache.
const fn sampler_slot(sampling: TextureSampling) -> usize {
    match sampling {
        TextureSampling::Linear => 0,
        TextureSampling::Nearest => 1,
        TextureSampling::PixelArt => 2,
    }
}

/// Magnification and minification filters for one material sampling mode.
const fn sampler_filters(sampling: TextureSampling) -> (FilterMode, FilterMode) {
    match sampling {
        TextureSampling::Linear => (FilterMode::Linear, FilterMode::Linear),
        TextureSampling::Nearest => (FilterMode::Nearest, FilterMode::Nearest),
        // Nearest magnification keeps pixel-art texels sharp up close; linear minification with
        // mip blending avoids distant shimmer and moire without re-blurring magnified texels.
        TextureSampling::PixelArt => (FilterMode::Nearest, FilterMode::Linear),
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
    variant: usize,
    front_face: Option<gfx_core::FrontFace>,
) -> gpui::Result<RenderPipelineId> {
    let voxel = variant != 0;
    let (label, blend, depth_write) = match (mode, voxel) {
        (PipelineMode::Opaque, false) => ("gpui-3d opaque pipeline", false, true),
        (PipelineMode::Blend, false) => ("gpui-3d blended pipeline", true, false),
        (PipelineMode::Opaque, true) => ("gpui-3d voxel opaque pipeline", false, true),
        (PipelineMode::Blend, true) => ("gpui-3d voxel blended pipeline", true, false),
    };
    Ok(device.create_render_pipeline(
        &RenderPipelineDescriptor {
            label: Some(label.into()),
            vertex_shader: if variant == 2 {
                resources.generated_vertex_shader
            } else if voxel {
                resources.voxel_vertex_shader
            } else {
                resources.vertex_shader
            }
            .ok_or_else(|| anyhow!("GPUI 3D vertex shader is unavailable"))?,
            vertex_entry_point: if variant == 2 {
                "vs_generated_voxel"
            } else if voxel {
                if resources.cull_shader.is_some() {
                    "vs_voxel_gpu"
                } else {
                    "vs_voxel"
                }
            } else {
                "vs_main"
            }
            .into(),
            fragment_shader: if voxel {
                resources.voxel_fragment_shader
            } else {
                resources.fragment_shader
            }
            .ok_or_else(|| anyhow!("GPUI 3D fragment shader is unavailable"))?,
            fragment_entry_point: if voxel { "fs_voxel" } else { "fs_main" }.into(),
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
            raster: gfx_core::RasterState {
                front_face: front_face.unwrap_or_default(),
                cull_mode: front_face.map(|_| gfx_core::Face::Back),
            },
            depth_state: Some(DepthState {
                compare: CompareFunction::LessEqual,
                write_enabled: depth_write,
            }),
        },
        context.viewport(),
    )?)
}
