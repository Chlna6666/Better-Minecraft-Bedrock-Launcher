use super::*;

pub(super) use super::pipeline_legacy::{
    NovaBlendPipelineDescriptor, NovaBlendPipelines, NovaPipelines, create_blend_pipelines,
};

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

    let opaque_or_cutout = fragment_entry_point.ends_with("_opaque")
        || fragment_entry_point.ends_with("_cutout");
    let blend_mode = if opaque_or_cutout {
        BlendMode::Replace
    } else {
        BlendMode::PremultipliedAlpha
    };

    device
        .create_render_pipeline(
            &RenderPipelineDescriptor {
                label: Some(format!(
                    "{label} custom GPU mesh 3D {} pipeline",
                    if opaque_or_cutout { "opaque" } else { "transparent" }
                )),
                vertex_shader,
                vertex_entry_point: vertex_entry_point.to_string(),
                fragment_shader,
                fragment_entry_point: fragment_entry_point.to_string(),
                vertex_buffers: Vec::new(),
                render_pass,
                pipeline_layout: Some(pipeline_layout),
                color_format: surface_config.format,
                blend_mode,
                primitive_topology: PrimitiveTopology::TriangleList,
                depth_state: Some(DepthState::default()),
            },
            surface_config.size,
        )
        .context("creating nova custom GPU mesh 3D material pipeline")
}
