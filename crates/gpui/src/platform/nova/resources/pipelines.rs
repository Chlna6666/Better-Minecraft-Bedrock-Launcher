use anyhow::{Context as _, Result};

use super::super::*;
use super::shaders::RendererShaders;

pub(super) fn create_renderer_pipelines<D>(
    device: &mut D,
    label: &str,
    surface_config: SurfaceConfig,
    render_pass: RenderPassId,
    layouts: &ResourceLayouts,
    shaders: RendererShaders,
) -> Result<Pipelines>
where
    D: BackendResources + BackendPipelines,
{
    let alpha = create_blend_pipelines(
        device,
        BlendPipelineDescriptor {
            label,
            suffix: "alpha",
            blend_mode: BlendMode::Alpha,
            size: surface_config.size,
            color_format: surface_config.format,
            render_pass,
            quad_pipeline_layout: layouts.quad_pipeline_layout,
            shadow_pipeline_layout: layouts.shadow_pipeline_layout,
            mono_pipeline_layout: layouts.mono_pipeline_layout,
            poly_pipeline_layout: layouts.poly_pipeline_layout,
            underline_pipeline_layout: layouts.underline_pipeline_layout,
            backdrop_blur_pipeline_layout: layouts.backdrop_blur_pipeline_layout,
            solid_vertex: shaders.solid_vertex,
            solid_fragment: shaders.solid_fragment,
            quad_vertex: shaders.quad_vertex,
            quad_fragment: shaders.quad_fragment,
            shadow_vertex: shaders.shadow_vertex,
            shadow_fragment: shaders.shadow_fragment,
            mono_vertex: shaders.mono_vertex,
            mono_fragment: shaders.mono_fragment,
            #[cfg(target_os = "windows")]
            subpixel_vertex: shaders.subpixel_vertex,
            #[cfg(target_os = "windows")]
            subpixel_fragment: shaders.subpixel_fragment,
            poly_vertex: shaders.poly_vertex,
            poly_fragment: shaders.poly_fragment,
            underline_vertex: shaders.underline_vertex,
            underline_fragment: shaders.underline_fragment,
            backdrop_blur_vertex: shaders.backdrop_blur_vertex,
            backdrop_blur_fragment: shaders.backdrop_blur_fragment,
        },
    )?;
    let premultiplied = create_blend_pipelines(
        device,
        BlendPipelineDescriptor {
            label,
            suffix: "premultiplied",
            blend_mode: BlendMode::PremultipliedAlpha,
            size: surface_config.size,
            color_format: surface_config.format,
            render_pass,
            quad_pipeline_layout: layouts.quad_pipeline_layout,
            shadow_pipeline_layout: layouts.shadow_pipeline_layout,
            mono_pipeline_layout: layouts.mono_pipeline_layout,
            poly_pipeline_layout: layouts.poly_pipeline_layout,
            underline_pipeline_layout: layouts.underline_pipeline_layout,
            backdrop_blur_pipeline_layout: layouts.backdrop_blur_pipeline_layout,
            solid_vertex: shaders.solid_vertex,
            solid_fragment: shaders.solid_fragment,
            quad_vertex: shaders.quad_vertex,
            quad_fragment: shaders.quad_fragment,
            shadow_vertex: shaders.shadow_vertex,
            shadow_fragment: shaders.shadow_fragment,
            mono_vertex: shaders.mono_vertex,
            mono_fragment: shaders.mono_fragment,
            #[cfg(target_os = "windows")]
            subpixel_vertex: shaders.subpixel_vertex,
            #[cfg(target_os = "windows")]
            subpixel_fragment: shaders.subpixel_fragment,
            poly_vertex: shaders.poly_vertex,
            poly_fragment: shaders.poly_fragment,
            underline_vertex: shaders.underline_vertex,
            underline_fragment: shaders.underline_fragment,
            backdrop_blur_vertex: shaders.backdrop_blur_vertex,
            backdrop_blur_fragment: shaders.backdrop_blur_fragment,
        },
    )?;
    let backdrop_blur_downsample = device
        .create_render_pipeline(
            &RenderPipelineDescriptor {
                label: Some(format!("{label} backdrop blur downsample pipeline")),
                vertex_shader: shaders.backdrop_blur_pass_vertex,
                vertex_entry_point: "vs_backdrop_blur_pass".to_string(),
                fragment_shader: shaders.backdrop_blur_downsample_fragment,
                fragment_entry_point: "fs_backdrop_blur_downsample".to_string(),
                vertex_buffers: Vec::new(),
                render_pass,
                pipeline_layout: Some(layouts.backdrop_blur_pass_pipeline_layout),
                color_format: surface_config.format,
                blend_mode: BlendMode::Replace,
                primitive_topology: PrimitiveTopology::TriangleStrip,
                depth_state: None,
            },
            surface_config.size,
        )
        .context("creating nova backdrop blur downsample render pipeline")?;
    let backdrop_blur_upsample = device
        .create_render_pipeline(
            &RenderPipelineDescriptor {
                label: Some(format!("{label} backdrop blur upsample pipeline")),
                vertex_shader: shaders.backdrop_blur_pass_vertex,
                vertex_entry_point: "vs_backdrop_blur_pass".to_string(),
                fragment_shader: shaders.backdrop_blur_upsample_fragment,
                fragment_entry_point: "fs_backdrop_blur_upsample".to_string(),
                vertex_buffers: Vec::new(),
                render_pass,
                pipeline_layout: Some(layouts.backdrop_blur_pass_pipeline_layout),
                color_format: surface_config.format,
                blend_mode: BlendMode::Replace,
                primitive_topology: PrimitiveTopology::TriangleStrip,
                depth_state: None,
            },
            surface_config.size,
        )
        .context("creating nova backdrop blur upsample render pipeline")?;
    let path_rasterization = device
        .create_render_pipeline(
            &RenderPipelineDescriptor {
                label: Some(format!("{label} path rasterization pipeline")),
                vertex_shader: shaders.path_rasterization_vertex,
                vertex_entry_point: "vs_path_rasterization".to_string(),
                fragment_shader: shaders.path_rasterization_fragment,
                fragment_entry_point: "fs_path_rasterization".to_string(),
                vertex_buffers: Vec::new(),
                render_pass,
                pipeline_layout: Some(layouts.path_rasterization_pipeline_layout),
                color_format: surface_config.format,
                blend_mode: BlendMode::PremultipliedAlpha,
                primitive_topology: PrimitiveTopology::TriangleList,
                depth_state: None,
            },
            surface_config.size,
        )
        .context("creating nova path rasterization render pipeline")?;
    let paths = device
        .create_render_pipeline(
            &RenderPipelineDescriptor {
                label: Some(format!("{label} path pipeline")),
                vertex_shader: shaders.path_vertex,
                vertex_entry_point: "vs_path".to_string(),
                fragment_shader: shaders.path_fragment,
                fragment_entry_point: "fs_path".to_string(),
                vertex_buffers: Vec::new(),
                render_pass,
                pipeline_layout: Some(layouts.path_pipeline_layout),
                color_format: surface_config.format,
                blend_mode: BlendMode::AdditiveAlpha,
                primitive_topology: PrimitiveTopology::TriangleStrip,
                depth_state: None,
            },
            surface_config.size,
        )
        .context("creating nova path render pipeline")?;
    Ok(Pipelines {
        alpha,
        premultiplied,
        path_rasterization,
        paths,
        backdrop_blur_downsample,
        backdrop_blur_upsample,
    })
}
