mod factory;
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
    pass: PassKind,
    owns_layout: bool,
    pub(super) cull_shader: Option<ShaderModuleId>,
    pub(super) singleton_cull_shader: Option<ShaderModuleId>,
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
    pub(super) fn rejects_material(&self, material: &crate::Material) -> bool {
        material.alpha_mode == crate::AlphaMode::Blend
            && (self.pass == PassKind::Shadow
                || (self.pass == PassKind::Hdr && material.surface != crate::SurfaceKind::Standard))
    }
    #[cfg(test)]
    pub(super) fn test_pipeline(
        layout: BindGroupLayoutId,
        pipeline: RenderPipelineId,
        fallback: TextureViewId,
    ) -> Self {
        Self {
            pass: PassKind::Color,
            owns_layout: false,
            cull_shader: None,
            singleton_cull_shader: None,
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
        if let Some(shader) = self.singleton_cull_shader.take() {
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
        if self.owns_layout
            && let Some(layout) = self.pipeline_layout.take()
        {
            device.destroy_pipeline_layout(layout)?;
        }
        if self.owns_layout
            && let Some(layout) = self.bind_group_layout.take()
        {
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
pub(super) fn embedded_shader(
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

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PassKind {
    Color,
    Hdr,
    Surface,
    Albedo,
    Shadow,
}
impl PassKind {
    fn entries(self) -> [&'static str; 2] {
        match self {
            Self::Color => ["fs_main", "fs_voxel"],
            Self::Hdr => ["fs_hdr", "fs_voxel_capture"],
            Self::Surface => ["fs_scene_surface", "fs_voxel_surface"],
            Self::Albedo => ["fs_scene_albedo", "fs_voxel_albedo"],
            Self::Shadow => ["fs_scene_shadow", "fs_voxel_shadow"],
        }
    }
    fn is_attribute(self) -> bool {
        matches!(self, Self::Surface | Self::Albedo | Self::Shadow)
    }
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
            fragment_entry_point: resources.pass.entries()[usize::from(voxel)].into(),
            vertex_buffers: Vec::new(),
            render_pass: context.render_pass(),
            pipeline_layout: resources.pipeline_layout,
            color_format: context.color_format(),
            blend_mode: if blend && !resources.pass.is_attribute() {
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
                write_enabled: depth_write || resources.pass.is_attribute(),
            }),
        },
        context.viewport(),
    )?)
}
