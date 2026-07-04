use anyhow::{Context as _, Result};
use gfx_core::ShaderModuleId;

use super::super::*;

#[derive(Clone, Copy)]
pub(super) struct NovaRendererShaders {
    pub(super) solid_vertex: ShaderModuleId,
    pub(super) solid_fragment: ShaderModuleId,
    pub(super) quad_vertex: ShaderModuleId,
    pub(super) quad_fragment: ShaderModuleId,
    pub(super) shadow_vertex: ShaderModuleId,
    pub(super) shadow_fragment: ShaderModuleId,
    pub(super) path_rasterization_vertex: ShaderModuleId,
    pub(super) path_rasterization_fragment: ShaderModuleId,
    pub(super) path_vertex: ShaderModuleId,
    pub(super) path_fragment: ShaderModuleId,
    pub(super) mono_vertex: ShaderModuleId,
    pub(super) mono_fragment: ShaderModuleId,
    pub(super) poly_vertex: ShaderModuleId,
    pub(super) poly_fragment: ShaderModuleId,
    pub(super) underline_vertex: ShaderModuleId,
    pub(super) underline_fragment: ShaderModuleId,
    pub(super) backdrop_blur_pass_vertex: ShaderModuleId,
    pub(super) backdrop_blur_downsample_fragment: ShaderModuleId,
    pub(super) backdrop_blur_upsample_fragment: ShaderModuleId,
    pub(super) backdrop_blur_vertex: ShaderModuleId,
    pub(super) backdrop_blur_fragment: ShaderModuleId,
}

pub(super) fn create_renderer_shaders<D>(
    device: &mut D,
    label: &str,
    shader_binaries: NovaShaderBinaries,
) -> Result<NovaRendererShaders>
where
    D: BackendResources + BackendPipelines,
{
    let solid_vertex = device
        .create_shader_module(&ShaderModuleDescriptor {
            label: Some(format!("{label} solid quad vertex shader")),
            binary: shader_binaries.solid_vertex,
        })
        .context("creating nova solid quad vertex shader module")?;
    let solid_fragment = device
        .create_shader_module(&ShaderModuleDescriptor {
            label: Some(format!("{label} solid quad fragment shader")),
            binary: shader_binaries.solid_fragment,
        })
        .context("creating nova solid quad fragment shader module")?;
    let quad_vertex = device
        .create_shader_module(&ShaderModuleDescriptor {
            label: Some(format!("{label} quad vertex shader")),
            binary: shader_binaries.quad_vertex,
        })
        .context("creating nova quad vertex shader module")?;
    let quad_fragment = device
        .create_shader_module(&ShaderModuleDescriptor {
            label: Some(format!("{label} quad fragment shader")),
            binary: shader_binaries.quad_fragment,
        })
        .context("creating nova quad fragment shader module")?;
    let shadow_vertex = device
        .create_shader_module(&ShaderModuleDescriptor {
            label: Some(format!("{label} shadow vertex shader")),
            binary: shader_binaries.shadow_vertex,
        })
        .context("creating nova shadow vertex shader module")?;
    let shadow_fragment = device
        .create_shader_module(&ShaderModuleDescriptor {
            label: Some(format!("{label} shadow fragment shader")),
            binary: shader_binaries.shadow_fragment,
        })
        .context("creating nova shadow fragment shader module")?;
    let path_rasterization_vertex = device
        .create_shader_module(&ShaderModuleDescriptor {
            label: Some(format!("{label} path rasterization vertex shader")),
            binary: shader_binaries.path_rasterization_vertex,
        })
        .context("creating nova path rasterization vertex shader module")?;
    let path_rasterization_fragment = device
        .create_shader_module(&ShaderModuleDescriptor {
            label: Some(format!("{label} path rasterization fragment shader")),
            binary: shader_binaries.path_rasterization_fragment,
        })
        .context("creating nova path rasterization fragment shader module")?;
    let path_vertex = device
        .create_shader_module(&ShaderModuleDescriptor {
            label: Some(format!("{label} path vertex shader")),
            binary: shader_binaries.path_vertex,
        })
        .context("creating nova path vertex shader module")?;
    let path_fragment = device
        .create_shader_module(&ShaderModuleDescriptor {
            label: Some(format!("{label} path fragment shader")),
            binary: shader_binaries.path_fragment,
        })
        .context("creating nova path fragment shader module")?;
    let mono_vertex = device
        .create_shader_module(&ShaderModuleDescriptor {
            label: Some(format!("{label} mono sprite vertex shader")),
            binary: shader_binaries.mono_vertex,
        })
        .context("creating nova mono sprite vertex shader module")?;
    let mono_fragment = device
        .create_shader_module(&ShaderModuleDescriptor {
            label: Some(format!("{label} mono sprite fragment shader")),
            binary: shader_binaries.mono_fragment,
        })
        .context("creating nova mono sprite fragment shader module")?;
    let poly_vertex = device
        .create_shader_module(&ShaderModuleDescriptor {
            label: Some(format!("{label} poly sprite vertex shader")),
            binary: shader_binaries.poly_vertex,
        })
        .context("creating nova poly sprite vertex shader module")?;
    let poly_fragment = device
        .create_shader_module(&ShaderModuleDescriptor {
            label: Some(format!("{label} poly sprite fragment shader")),
            binary: shader_binaries.poly_fragment,
        })
        .context("creating nova poly sprite fragment shader module")?;
    let underline_vertex = device
        .create_shader_module(&ShaderModuleDescriptor {
            label: Some(format!("{label} underline vertex shader")),
            binary: shader_binaries.underline_vertex,
        })
        .context("creating nova underline vertex shader module")?;
    let underline_fragment = device
        .create_shader_module(&ShaderModuleDescriptor {
            label: Some(format!("{label} underline fragment shader")),
            binary: shader_binaries.underline_fragment,
        })
        .context("creating nova underline fragment shader module")?;
    let backdrop_blur_pass_vertex = device
        .create_shader_module(&ShaderModuleDescriptor {
            label: Some(format!("{label} backdrop blur pass vertex shader")),
            binary: shader_binaries.backdrop_blur_pass_vertex,
        })
        .context("creating nova backdrop blur pass vertex shader module")?;
    let backdrop_blur_downsample_fragment = device
        .create_shader_module(&ShaderModuleDescriptor {
            label: Some(format!("{label} backdrop blur downsample fragment shader")),
            binary: shader_binaries.backdrop_blur_downsample_fragment,
        })
        .context("creating nova backdrop blur downsample fragment shader module")?;
    let backdrop_blur_upsample_fragment = device
        .create_shader_module(&ShaderModuleDescriptor {
            label: Some(format!("{label} backdrop blur upsample fragment shader")),
            binary: shader_binaries.backdrop_blur_upsample_fragment,
        })
        .context("creating nova backdrop blur upsample fragment shader module")?;
    let backdrop_blur_vertex = device
        .create_shader_module(&ShaderModuleDescriptor {
            label: Some(format!("{label} backdrop blur vertex shader")),
            binary: shader_binaries.backdrop_blur_vertex,
        })
        .context("creating nova backdrop blur vertex shader module")?;
    let backdrop_blur_fragment = device
        .create_shader_module(&ShaderModuleDescriptor {
            label: Some(format!("{label} backdrop blur fragment shader")),
            binary: shader_binaries.backdrop_blur_fragment,
        })
        .context("creating nova backdrop blur fragment shader module")?;

    Ok(NovaRendererShaders {
        solid_vertex,
        solid_fragment,
        quad_vertex,
        quad_fragment,
        shadow_vertex,
        shadow_fragment,
        path_rasterization_vertex,
        path_rasterization_fragment,
        path_vertex,
        path_fragment,
        mono_vertex,
        mono_fragment,
        poly_vertex,
        poly_fragment,
        underline_vertex,
        underline_fragment,
        backdrop_blur_pass_vertex,
        backdrop_blur_downsample_fragment,
        backdrop_blur_upsample_fragment,
        backdrop_blur_vertex,
        backdrop_blur_fragment,
    })
}
