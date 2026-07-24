use super::*;

pub(super) struct NovaPipelines {
    pub(super) alpha: NovaBlendPipelines,
    pub(super) premultiplied: NovaBlendPipelines,
    pub(super) path_rasterization: RenderPipelineId,
    pub(super) paths: RenderPipelineId,
    pub(super) present_copy: RenderPipelineId,
    pub(super) backdrop_blur_downsample: RenderPipelineId,
    pub(super) backdrop_blur_upsample: RenderPipelineId,
}

#[derive(Clone, Copy)]
pub(super) struct NovaBlendPipelines {
    pub(super) solid_quads: RenderPipelineId,
    pub(super) quads: RenderPipelineId,
    pub(super) shadows: RenderPipelineId,
    pub(super) mono_sprites: RenderPipelineId,
    pub(super) poly_sprites: RenderPipelineId,
    pub(super) underlines: RenderPipelineId,
    pub(super) backdrop_blurs: RenderPipelineId,
}

pub(super) fn create_blend_pipelines<D>(
    device: &mut D,
    descriptor: NovaBlendPipelineDescriptor<'_>,
) -> Result<NovaBlendPipelines>
where
    D: BackendResources + BackendPipelines,
{
    let solid_quads = device
        .create_render_pipeline(
            &RenderPipelineDescriptor {
                label: Some(format!(
                    "{} {} solid quad pipeline",
                    descriptor.label, descriptor.suffix
                )),
                vertex_shader: descriptor.solid_vertex,
                vertex_entry_point: "vs_solid_quad".to_string(),
                fragment_shader: descriptor.solid_fragment,
                fragment_entry_point: "fs_solid_quad".to_string(),
                vertex_buffers: Vec::new(),
                render_pass: descriptor.render_pass,
                pipeline_layout: Some(descriptor.quad_pipeline_layout),
                color_format: descriptor.color_format,
                blend_mode: descriptor.blend_mode,
                primitive_topology: PrimitiveTopology::TriangleStrip,
                depth_state: None,
            },
            descriptor.size,
        )
        .with_context(|| {
            format!(
                "creating nova {} solid quad render pipeline",
                descriptor.suffix
            )
        })?;
    let quads = device
        .create_render_pipeline(
            &RenderPipelineDescriptor {
                label: Some(format!(
                    "{} {} quad pipeline",
                    descriptor.label, descriptor.suffix
                )),
                vertex_shader: descriptor.quad_vertex,
                vertex_entry_point: "vs_quad".to_string(),
                fragment_shader: descriptor.quad_fragment,
                fragment_entry_point: "fs_quad".to_string(),
                vertex_buffers: Vec::new(),
                render_pass: descriptor.render_pass,
                pipeline_layout: Some(descriptor.quad_pipeline_layout),
                color_format: descriptor.color_format,
                blend_mode: descriptor.blend_mode,
                primitive_topology: PrimitiveTopology::TriangleStrip,
                depth_state: None,
            },
            descriptor.size,
        )
        .with_context(|| format!("creating nova {} quad render pipeline", descriptor.suffix))?;
    let shadows = device
        .create_render_pipeline(
            &RenderPipelineDescriptor {
                label: Some(format!(
                    "{} {} shadow pipeline",
                    descriptor.label, descriptor.suffix
                )),
                vertex_shader: descriptor.shadow_vertex,
                vertex_entry_point: "vs_shadow".to_string(),
                fragment_shader: descriptor.shadow_fragment,
                fragment_entry_point: "fs_shadow".to_string(),
                vertex_buffers: Vec::new(),
                render_pass: descriptor.render_pass,
                pipeline_layout: Some(descriptor.shadow_pipeline_layout),
                color_format: descriptor.color_format,
                blend_mode: descriptor.blend_mode,
                primitive_topology: PrimitiveTopology::TriangleStrip,
                depth_state: None,
            },
            descriptor.size,
        )
        .with_context(|| format!("creating nova {} shadow render pipeline", descriptor.suffix))?;
    let mono_sprites = device
        .create_render_pipeline(
            &RenderPipelineDescriptor {
                label: Some(format!(
                    "{} {} mono sprite pipeline",
                    descriptor.label, descriptor.suffix
                )),
                vertex_shader: descriptor.mono_vertex,
                vertex_entry_point: "vs_mono_sprite".to_string(),
                fragment_shader: descriptor.mono_fragment,
                fragment_entry_point: "fs_mono_sprite".to_string(),
                vertex_buffers: Vec::new(),
                render_pass: descriptor.render_pass,
                pipeline_layout: Some(descriptor.mono_pipeline_layout),
                color_format: descriptor.color_format,
                blend_mode: descriptor.blend_mode,
                primitive_topology: PrimitiveTopology::TriangleStrip,
                depth_state: None,
            },
            descriptor.size,
        )
        .with_context(|| {
            format!(
                "creating nova {} mono sprite render pipeline",
                descriptor.suffix
            )
        })?;
    let poly_sprites = device
        .create_render_pipeline(
            &RenderPipelineDescriptor {
                label: Some(format!(
                    "{} {} poly sprite pipeline",
                    descriptor.label, descriptor.suffix
                )),
                vertex_shader: descriptor.poly_vertex,
                vertex_entry_point: "vs_poly_sprite".to_string(),
                fragment_shader: descriptor.poly_fragment,
                fragment_entry_point: "fs_poly_sprite".to_string(),
                vertex_buffers: Vec::new(),
                render_pass: descriptor.render_pass,
                pipeline_layout: Some(descriptor.poly_pipeline_layout),
                color_format: descriptor.color_format,
                blend_mode: descriptor.blend_mode,
                primitive_topology: PrimitiveTopology::TriangleStrip,
                depth_state: None,
            },
            descriptor.size,
        )
        .with_context(|| {
            format!(
                "creating nova {} poly sprite render pipeline",
                descriptor.suffix
            )
        })?;
    let underlines = device
        .create_render_pipeline(
            &RenderPipelineDescriptor {
                label: Some(format!(
                    "{} {} underline pipeline",
                    descriptor.label, descriptor.suffix
                )),
                vertex_shader: descriptor.underline_vertex,
                vertex_entry_point: "vs_underline".to_string(),
                fragment_shader: descriptor.underline_fragment,
                fragment_entry_point: "fs_underline".to_string(),
                vertex_buffers: Vec::new(),
                render_pass: descriptor.render_pass,
                pipeline_layout: Some(descriptor.underline_pipeline_layout),
                color_format: descriptor.color_format,
                blend_mode: descriptor.blend_mode,
                primitive_topology: PrimitiveTopology::TriangleStrip,
                depth_state: None,
            },
            descriptor.size,
        )
        .with_context(|| {
            format!(
                "creating nova {} underline render pipeline",
                descriptor.suffix
            )
        })?;
    let backdrop_blurs = device
        .create_render_pipeline(
            &RenderPipelineDescriptor {
                label: Some(format!(
                    "{} {} backdrop blur pipeline",
                    descriptor.label, descriptor.suffix
                )),
                vertex_shader: descriptor.backdrop_blur_vertex,
                vertex_entry_point: "vs_backdrop_blur".to_string(),
                fragment_shader: descriptor.backdrop_blur_fragment,
                fragment_entry_point: "fs_backdrop_blur".to_string(),
                vertex_buffers: Vec::new(),
                render_pass: descriptor.render_pass,
                pipeline_layout: Some(descriptor.backdrop_blur_pipeline_layout),
                color_format: descriptor.color_format,
                blend_mode: descriptor.blend_mode,
                primitive_topology: PrimitiveTopology::TriangleStrip,
                depth_state: None,
            },
            descriptor.size,
        )
        .with_context(|| {
            format!(
                "creating nova {} backdrop blur render pipeline",
                descriptor.suffix
            )
        })?;

    Ok(NovaBlendPipelines {
        solid_quads,
        quads,
        shadows,
        mono_sprites,
        poly_sprites,
        underlines,
        backdrop_blurs,
    })
}

pub(super) fn create_custom_mesh_3d_pipeline<D>(
    device: &mut D,
    label: &str,
    render_pass: RenderPassId,
    pipeline_layout: PipelineLayoutId,
    surface_config: SurfaceConfig,
    vertex_shader: gfx_core::ShaderBinary,
    fragment_shader: gfx_core::ShaderBinary,
    vertex_entry_point: &str,
    fragment_entry_point: &str,
) -> Result<RenderPipelineId>
where
    D: BackendResources + BackendPipelines,
{
    let vertex_shader = device
        .create_shader_module(&ShaderModuleDescriptor {
            label: Some(format!("{label} custom GPU mesh 3D vertex shader")),
            binary: vertex_shader,
        })
        .context("creating nova custom GPU mesh 3D vertex shader module")?;
    let fragment_shader = device
        .create_shader_module(&ShaderModuleDescriptor {
            label: Some(format!("{label} custom GPU mesh 3D fragment shader")),
            binary: fragment_shader,
        })
        .context("creating nova custom GPU mesh 3D fragment shader module")?;

    device
        .create_render_pipeline(
            &RenderPipelineDescriptor {
                label: Some(format!("{label} custom GPU mesh 3D pipeline")),
                vertex_shader,
                vertex_entry_point: vertex_entry_point.to_string(),
                fragment_shader,
                fragment_entry_point: fragment_entry_point.to_string(),
                vertex_buffers: Vec::new(),
                render_pass,
                pipeline_layout: Some(pipeline_layout),
                color_format: surface_config.format,
                blend_mode: BlendMode::PremultipliedAlpha,
                primitive_topology: PrimitiveTopology::TriangleList,
                depth_state: Some(DepthState::default()),
            },
            surface_config.size,
        )
        .context("creating nova custom GPU mesh 3D render pipeline")
}
