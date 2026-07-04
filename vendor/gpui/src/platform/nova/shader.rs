use super::*;

#[allow(dead_code)]
pub(super) const NOVA_SOLID_QUAD_SHADER_SOURCE: &str = concat!(
    include_str!("shaders/core.wgsl"),
    include_str!("shaders/solid_quad.wgsl"),
);

#[allow(dead_code)]
pub(super) const NOVA_MONO_SPRITE_SHADER_SOURCE: &str = concat!(
    include_str!("shaders/core.wgsl"),
    include_str!("shaders/text.wgsl"),
    include_str!("shaders/mono_sprite.wgsl"),
);

#[allow(dead_code)]
pub(super) const NOVA_QUAD_SHADER_SOURCE: &str = concat!(
    include_str!("shaders/core.wgsl"),
    include_str!("shaders/shape.wgsl"),
    include_str!("shaders/quad_common.wgsl"),
    include_str!("shaders/quad.wgsl"),
);

#[allow(dead_code)]
pub(super) const NOVA_SHADOW_SHADER_SOURCE: &str = concat!(
    include_str!("shaders/core.wgsl"),
    include_str!("shaders/shape.wgsl"),
    include_str!("shaders/shadow.wgsl"),
);

#[allow(dead_code)]
pub(super) const NOVA_PATH_SHADER_SOURCE: &str = concat!(
    include_str!("shaders/core.wgsl"),
    include_str!("shaders/quad_common.wgsl"),
    include_str!("shaders/path.wgsl"),
);

#[allow(dead_code)]
pub(super) const NOVA_UNDERLINE_SHADER_SOURCE: &str = concat!(
    include_str!("shaders/core.wgsl"),
    include_str!("shaders/underline.wgsl"),
);

#[allow(dead_code)]
pub(super) const NOVA_POLY_SPRITE_SHADER_SOURCE: &str = concat!(
    include_str!("shaders/core.wgsl"),
    include_str!("shaders/shape.wgsl"),
    include_str!("shaders/poly_sprite.wgsl"),
);

#[allow(dead_code)]
pub(super) const NOVA_SURFACE_SHADER_SOURCE: &str = concat!(
    include_str!("shaders/core.wgsl"),
    include_str!("shaders/surface.wgsl"),
);

#[allow(dead_code)]
pub(super) const NOVA_BACKDROP_BLUR_SHADER_SOURCE: &str = concat!(
    include_str!("shaders/core.wgsl"),
    include_str!("shaders/shape.wgsl"),
    include_str!("shaders/backdrop_blur.wgsl"),
);

pub(super) struct NovaShaderBinaries {
    pub(super) solid_vertex: gfx_core::ShaderBinary,
    pub(super) solid_fragment: gfx_core::ShaderBinary,
    pub(super) quad_vertex: gfx_core::ShaderBinary,
    pub(super) quad_fragment: gfx_core::ShaderBinary,
    pub(super) shadow_vertex: gfx_core::ShaderBinary,
    pub(super) shadow_fragment: gfx_core::ShaderBinary,
    pub(super) path_rasterization_vertex: gfx_core::ShaderBinary,
    pub(super) path_rasterization_fragment: gfx_core::ShaderBinary,
    pub(super) path_vertex: gfx_core::ShaderBinary,
    pub(super) path_fragment: gfx_core::ShaderBinary,
    pub(super) mono_vertex: gfx_core::ShaderBinary,
    pub(super) mono_fragment: gfx_core::ShaderBinary,
    pub(super) poly_vertex: gfx_core::ShaderBinary,
    pub(super) poly_fragment: gfx_core::ShaderBinary,
    pub(super) underline_vertex: gfx_core::ShaderBinary,
    pub(super) underline_fragment: gfx_core::ShaderBinary,
    pub(super) backdrop_blur_pass_vertex: gfx_core::ShaderBinary,
    pub(super) backdrop_blur_downsample_fragment: gfx_core::ShaderBinary,
    pub(super) backdrop_blur_upsample_fragment: gfx_core::ShaderBinary,
    pub(super) backdrop_blur_vertex: gfx_core::ShaderBinary,
    pub(super) backdrop_blur_fragment: gfx_core::ShaderBinary,
}

pub(super) struct NovaBlendPipelineDescriptor<'a> {
    pub(super) label: &'a str,
    pub(super) suffix: &'a str,
    pub(super) blend_mode: BlendMode,
    pub(super) size: Extent2d,
    pub(super) color_format: Format,
    pub(super) render_pass: RenderPassId,
    pub(super) quad_pipeline_layout: PipelineLayoutId,
    pub(super) shadow_pipeline_layout: PipelineLayoutId,
    pub(super) mono_pipeline_layout: PipelineLayoutId,
    pub(super) poly_pipeline_layout: PipelineLayoutId,
    pub(super) underline_pipeline_layout: PipelineLayoutId,
    pub(super) backdrop_blur_pipeline_layout: PipelineLayoutId,
    pub(super) solid_vertex: gfx_core::ShaderModuleId,
    pub(super) solid_fragment: gfx_core::ShaderModuleId,
    pub(super) quad_vertex: gfx_core::ShaderModuleId,
    pub(super) quad_fragment: gfx_core::ShaderModuleId,
    pub(super) shadow_vertex: gfx_core::ShaderModuleId,
    pub(super) shadow_fragment: gfx_core::ShaderModuleId,
    pub(super) mono_vertex: gfx_core::ShaderModuleId,
    pub(super) mono_fragment: gfx_core::ShaderModuleId,
    pub(super) poly_vertex: gfx_core::ShaderModuleId,
    pub(super) poly_fragment: gfx_core::ShaderModuleId,
    pub(super) underline_vertex: gfx_core::ShaderModuleId,
    pub(super) underline_fragment: gfx_core::ShaderModuleId,
    pub(super) backdrop_blur_vertex: gfx_core::ShaderModuleId,
    pub(super) backdrop_blur_fragment: gfx_core::ShaderModuleId,
}

pub(super) fn compile_nova_shader_binaries(
    mut compile: impl FnMut(
        &str,
        ShaderStage,
        &str,
    ) -> std::result::Result<gfx_core::ShaderBinary, gfx_shader::ShaderError>,
) -> Result<NovaShaderBinaries> {
    Ok(NovaShaderBinaries {
        solid_vertex: compile(
            NOVA_SOLID_QUAD_SHADER_SOURCE,
            ShaderStage::Vertex,
            "vs_solid_quad",
        )
        .context("compiling nova solid quad vertex shader")?,
        solid_fragment: compile(
            NOVA_SOLID_QUAD_SHADER_SOURCE,
            ShaderStage::Fragment,
            "fs_solid_quad",
        )
        .context("compiling nova solid quad fragment shader")?,
        quad_vertex: compile(NOVA_QUAD_SHADER_SOURCE, ShaderStage::Vertex, "vs_quad")
            .context("compiling nova quad vertex shader")?,
        quad_fragment: compile(NOVA_QUAD_SHADER_SOURCE, ShaderStage::Fragment, "fs_quad")
            .context("compiling nova quad fragment shader")?,
        shadow_vertex: compile(NOVA_SHADOW_SHADER_SOURCE, ShaderStage::Vertex, "vs_shadow")
            .context("compiling nova shadow vertex shader")?,
        shadow_fragment: compile(
            NOVA_SHADOW_SHADER_SOURCE,
            ShaderStage::Fragment,
            "fs_shadow",
        )
        .context("compiling nova shadow fragment shader")?,
        path_rasterization_vertex: compile(
            NOVA_PATH_SHADER_SOURCE,
            ShaderStage::Vertex,
            "vs_path_rasterization",
        )
        .context("compiling nova path rasterization vertex shader")?,
        path_rasterization_fragment: compile(
            NOVA_PATH_SHADER_SOURCE,
            ShaderStage::Fragment,
            "fs_path_rasterization",
        )
        .context("compiling nova path rasterization fragment shader")?,
        path_vertex: compile(NOVA_PATH_SHADER_SOURCE, ShaderStage::Vertex, "vs_path")
            .context("compiling nova path vertex shader")?,
        path_fragment: compile(NOVA_PATH_SHADER_SOURCE, ShaderStage::Fragment, "fs_path")
            .context("compiling nova path fragment shader")?,
        mono_vertex: compile(
            NOVA_MONO_SPRITE_SHADER_SOURCE,
            ShaderStage::Vertex,
            "vs_mono_sprite",
        )
        .context("compiling nova mono sprite vertex shader")?,
        mono_fragment: compile(
            NOVA_MONO_SPRITE_SHADER_SOURCE,
            ShaderStage::Fragment,
            "fs_mono_sprite",
        )
        .context("compiling nova mono sprite fragment shader")?,
        poly_vertex: compile(
            NOVA_POLY_SPRITE_SHADER_SOURCE,
            ShaderStage::Vertex,
            "vs_poly_sprite",
        )
        .context("compiling nova poly sprite vertex shader")?,
        poly_fragment: compile(
            NOVA_POLY_SPRITE_SHADER_SOURCE,
            ShaderStage::Fragment,
            "fs_poly_sprite",
        )
        .context("compiling nova poly sprite fragment shader")?,
        underline_vertex: compile(
            NOVA_UNDERLINE_SHADER_SOURCE,
            ShaderStage::Vertex,
            "vs_underline",
        )
        .context("compiling nova underline vertex shader")?,
        underline_fragment: compile(
            NOVA_UNDERLINE_SHADER_SOURCE,
            ShaderStage::Fragment,
            "fs_underline",
        )
        .context("compiling nova underline fragment shader")?,
        backdrop_blur_pass_vertex: compile(
            NOVA_BACKDROP_BLUR_SHADER_SOURCE,
            ShaderStage::Vertex,
            "vs_backdrop_blur_pass",
        )
        .context("compiling nova backdrop blur pass vertex shader")?,
        backdrop_blur_downsample_fragment: compile(
            NOVA_BACKDROP_BLUR_SHADER_SOURCE,
            ShaderStage::Fragment,
            "fs_backdrop_blur_downsample",
        )
        .context("compiling nova backdrop blur downsample fragment shader")?,
        backdrop_blur_upsample_fragment: compile(
            NOVA_BACKDROP_BLUR_SHADER_SOURCE,
            ShaderStage::Fragment,
            "fs_backdrop_blur_upsample",
        )
        .context("compiling nova backdrop blur upsample fragment shader")?,
        backdrop_blur_vertex: compile(
            NOVA_BACKDROP_BLUR_SHADER_SOURCE,
            ShaderStage::Vertex,
            "vs_backdrop_blur",
        )
        .context("compiling nova backdrop blur vertex shader")?,
        backdrop_blur_fragment: compile(
            NOVA_BACKDROP_BLUR_SHADER_SOURCE,
            ShaderStage::Fragment,
            "fs_backdrop_blur",
        )
        .context("compiling nova backdrop blur fragment shader")?,
    })
}
