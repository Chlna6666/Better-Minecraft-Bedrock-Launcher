use super::*;

#[cfg(test)]
#[allow(dead_code)]
pub(super) const NOVA_SOLID_QUAD_SHADER_SOURCE: &str = concat!(
    include_str!("shaders/core.wgsl"),
    include_str!("shaders/animation.wgsl"),
    include_str!("shaders/solid_quad.wgsl"),
);

#[cfg(test)]
#[allow(dead_code)]
pub(super) const NOVA_MONO_SPRITE_SHADER_SOURCE: &str = concat!(
    include_str!("shaders/core.wgsl"),
    include_str!("shaders/animation.wgsl"),
    include_str!("shaders/text.wgsl"),
    include_str!("shaders/sprite_common.wgsl"),
    include_str!("shaders/mono_sprite.wgsl"),
);

#[cfg(all(test, target_os = "windows"))]
#[allow(dead_code)]
pub(super) const NOVA_SUBPIXEL_SPRITE_SHADER_SOURCE: &str = concat!(
    "enable dual_source_blending;\n",
    include_str!("shaders/core.wgsl"),
    include_str!("shaders/animation.wgsl"),
    include_str!("shaders/text.wgsl"),
    include_str!("shaders/sprite_common.wgsl"),
    include_str!("shaders/subpixel_sprite_common.wgsl"),
    include_str!("shaders/subpixel_sprite.wgsl"),
);

#[cfg(all(test, target_os = "windows"))]
#[allow(dead_code)]
pub(super) const NOVA_SUBPIXEL_GRAYSCALE_SHADER_SOURCE: &str = concat!(
    include_str!("shaders/core.wgsl"),
    include_str!("shaders/animation.wgsl"),
    include_str!("shaders/text.wgsl"),
    include_str!("shaders/sprite_common.wgsl"),
    include_str!("shaders/subpixel_sprite_common.wgsl"),
    include_str!("shaders/subpixel_sprite_grayscale.wgsl"),
);

#[cfg(test)]
#[allow(dead_code)]
pub(super) const NOVA_QUAD_SHADER_SOURCE: &str = concat!(
    include_str!("shaders/core.wgsl"),
    include_str!("shaders/animation.wgsl"),
    include_str!("shaders/shape.wgsl"),
    include_str!("shaders/quad_common.wgsl"),
    include_str!("shaders/quad.wgsl"),
);

#[cfg(test)]
#[allow(dead_code)]
pub(super) const NOVA_SHADOW_SHADER_SOURCE: &str = concat!(
    include_str!("shaders/core.wgsl"),
    include_str!("shaders/animation.wgsl"),
    include_str!("shaders/shape.wgsl"),
    include_str!("shaders/shadow.wgsl"),
);

#[cfg(test)]
#[allow(dead_code)]
pub(super) const NOVA_PATH_SHADER_SOURCE: &str = concat!(
    include_str!("shaders/core.wgsl"),
    include_str!("shaders/quad_common.wgsl"),
    include_str!("shaders/path.wgsl"),
);

#[cfg(test)]
#[allow(dead_code)]
pub(super) const NOVA_UNDERLINE_SHADER_SOURCE: &str = concat!(
    include_str!("shaders/core.wgsl"),
    include_str!("shaders/underline.wgsl"),
);

#[cfg(test)]
#[allow(dead_code)]
pub(super) const NOVA_POLY_SPRITE_SHADER_SOURCE: &str = concat!(
    include_str!("shaders/core.wgsl"),
    include_str!("shaders/animation.wgsl"),
    include_str!("shaders/shape.wgsl"),
    include_str!("shaders/poly_sprite.wgsl"),
);

#[cfg(test)]
#[allow(dead_code)]
pub(super) const NOVA_SURFACE_SHADER_SOURCE: &str = concat!(
    include_str!("shaders/core.wgsl"),
    include_str!("shaders/surface.wgsl"),
);

#[cfg(test)]
#[allow(dead_code)]
pub(super) const NOVA_BACKDROP_BLUR_SHADER_SOURCE: &str = concat!(
    include_str!("shaders/core.wgsl"),
    include_str!("shaders/shape.wgsl"),
    include_str!("shaders/animation.wgsl"),
    include_str!("shaders/blur.wgsl"),
);

#[derive(Clone)]
pub(super) struct ShaderBinaries {
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
    #[cfg(target_os = "windows")]
    pub(super) subpixel_vertex: gfx_core::ShaderBinary,
    #[cfg(target_os = "windows")]
    pub(super) subpixel_fragment: gfx_core::ShaderBinary,
    #[cfg(target_os = "windows")]
    pub(super) subpixel_grayscale_fragment: gfx_core::ShaderBinary,
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

type ShaderCacheEntry = std::result::Result<ShaderBinaries, Arc<str>>;

fn cached_nova_shader_binaries(
    cache: &'static std::sync::OnceLock<ShaderCacheEntry>,
    resolve: fn(&str) -> Option<gfx_core::EmbeddedShader>,
) -> Result<ShaderBinaries> {
    match cache.get_or_init(|| {
        generated_nova_shader_binaries(resolve).map_err(|error| format!("{error:#}").into())
    }) {
        Ok(binaries) => Ok(binaries.clone()),
        Err(error) => Err(anyhow::anyhow!(error.to_string())),
    }
}

/// Resolves one build-generated artifact into the backend binary uploaded to the device.
fn generated_shader_binary(
    resolve: fn(&str) -> Option<gfx_core::EmbeddedShader>,
    stage: ShaderStage,
    entry_point: &str,
) -> Result<gfx_core::ShaderBinary> {
    let generated = resolve(entry_point).ok_or_else(|| {
        anyhow::anyhow!("missing build-generated Nova shader artifact {entry_point}")
    })?;
    generated
        .to_binary(stage, entry_point)
        .with_context(|| format!("decoding build-generated Nova shader {entry_point}"))
}

/// Builds the fixed Nova shader table without carrying WGSL source through production startup.
fn generated_nova_shader_binaries(
    resolve: fn(&str) -> Option<gfx_core::EmbeddedShader>,
) -> Result<ShaderBinaries> {
    Ok(ShaderBinaries {
        solid_vertex: generated_shader_binary(resolve, ShaderStage::Vertex, "vs_solid_quad")?,
        solid_fragment: generated_shader_binary(resolve, ShaderStage::Fragment, "fs_solid_quad")?,
        quad_vertex: generated_shader_binary(resolve, ShaderStage::Vertex, "vs_quad")?,
        quad_fragment: generated_shader_binary(resolve, ShaderStage::Fragment, "fs_quad")?,
        shadow_vertex: generated_shader_binary(resolve, ShaderStage::Vertex, "vs_shadow")?,
        shadow_fragment: generated_shader_binary(resolve, ShaderStage::Fragment, "fs_shadow")?,
        path_rasterization_vertex: generated_shader_binary(
            resolve,
            ShaderStage::Vertex,
            "vs_path_rasterization",
        )?,
        path_rasterization_fragment: generated_shader_binary(
            resolve,
            ShaderStage::Fragment,
            "fs_path_rasterization",
        )?,
        path_vertex: generated_shader_binary(resolve, ShaderStage::Vertex, "vs_path")?,
        path_fragment: generated_shader_binary(resolve, ShaderStage::Fragment, "fs_path")?,
        mono_vertex: generated_shader_binary(resolve, ShaderStage::Vertex, "vs_mono_sprite")?,
        mono_fragment: generated_shader_binary(resolve, ShaderStage::Fragment, "fs_mono_sprite")?,
        #[cfg(target_os = "windows")]
        subpixel_vertex: generated_shader_binary(
            resolve,
            ShaderStage::Vertex,
            "vs_subpixel_sprite",
        )?,
        #[cfg(target_os = "windows")]
        subpixel_fragment: generated_shader_binary(
            resolve,
            ShaderStage::Fragment,
            "fs_subpixel_sprite",
        )?,
        #[cfg(target_os = "windows")]
        subpixel_grayscale_fragment: generated_shader_binary(
            resolve,
            ShaderStage::Fragment,
            "fs_subpixel_sprite_grayscale",
        )?,
        poly_vertex: generated_shader_binary(resolve, ShaderStage::Vertex, "vs_poly_sprite")?,
        poly_fragment: generated_shader_binary(resolve, ShaderStage::Fragment, "fs_poly_sprite")?,
        underline_vertex: generated_shader_binary(resolve, ShaderStage::Vertex, "vs_underline")?,
        underline_fragment: generated_shader_binary(
            resolve,
            ShaderStage::Fragment,
            "fs_underline",
        )?,
        backdrop_blur_pass_vertex: generated_shader_binary(
            resolve,
            ShaderStage::Vertex,
            "vs_backdrop_blur_pass",
        )?,
        backdrop_blur_downsample_fragment: generated_shader_binary(
            resolve,
            ShaderStage::Fragment,
            "fs_backdrop_blur_downsample",
        )?,
        backdrop_blur_upsample_fragment: generated_shader_binary(
            resolve,
            ShaderStage::Fragment,
            "fs_backdrop_blur_upsample",
        )?,
        backdrop_blur_vertex: generated_shader_binary(
            resolve,
            ShaderStage::Vertex,
            "vs_backdrop_blur",
        )?,
        backdrop_blur_fragment: generated_shader_binary(
            resolve,
            ShaderStage::Fragment,
            "fs_backdrop_blur",
        )?,
    })
}

#[cfg(all(
    feature = "nova-gfx-opengl",
    any(target_os = "windows", target_os = "linux")
))]
pub(super) fn cached_nova_opengl_shader_binaries() -> Result<ShaderBinaries> {
    static CACHE: std::sync::OnceLock<ShaderCacheEntry> = std::sync::OnceLock::new();
    static REPORTED: std::sync::Once = std::sync::Once::new();

    let binaries = cached_nova_shader_binaries(&CACHE, nova_opengl_shader)?;
    REPORTED.call_once(|| {
        log::info!("nova OpenGL shader artifacts: {NOVA_OPENGL_SHADER_ARTIFACT_KIND}");
    });
    Ok(binaries)
}
#[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
pub(super) fn cached_nova_dx11_shader_binaries() -> Result<ShaderBinaries> {
    static CACHE: std::sync::OnceLock<ShaderCacheEntry> = std::sync::OnceLock::new();
    static REPORTED: std::sync::Once = std::sync::Once::new();

    let binaries = cached_nova_shader_binaries(&CACHE, nova_dx11_shader)?;
    REPORTED.call_once(|| {
        log::info!("nova DX11 shader artifacts: {NOVA_DX11_SHADER_ARTIFACT_KIND}");
    });
    Ok(binaries)
}
#[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
pub(super) fn cached_nova_dx12_shader_binaries() -> Result<ShaderBinaries> {
    static CACHE: std::sync::OnceLock<ShaderCacheEntry> = std::sync::OnceLock::new();
    static REPORTED: std::sync::Once = std::sync::Once::new();

    let binaries = cached_nova_shader_binaries(&CACHE, nova_dx12_shader)?;
    REPORTED.call_once(|| {
        log::info!("nova DX12 shader artifacts: {NOVA_DX12_SHADER_ARTIFACT_KIND}");
    });
    Ok(binaries)
}

#[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
pub(super) fn cached_nova_metal_shader_binaries() -> Result<ShaderBinaries> {
    static CACHE: std::sync::OnceLock<ShaderCacheEntry> = std::sync::OnceLock::new();
    static REPORTED: std::sync::Once = std::sync::Once::new();

    let binaries = cached_nova_shader_binaries(&CACHE, nova_metal_shader)?;
    REPORTED.call_once(|| {
        log::info!("nova Metal shader artifacts: {NOVA_METAL_SHADER_ARTIFACT_KIND}");
    });
    Ok(binaries)
}

#[cfg(all(
    feature = "nova-gfx-vulkan",
    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
))]
pub(super) fn cached_nova_vulkan_shader_binaries() -> Result<ShaderBinaries> {
    static CACHE: std::sync::OnceLock<ShaderCacheEntry> = std::sync::OnceLock::new();
    static REPORTED: std::sync::Once = std::sync::Once::new();

    let binaries = cached_nova_shader_binaries(&CACHE, nova_vulkan_shader)?;
    REPORTED.call_once(|| {
        log::info!("nova Vulkan shader artifacts: {NOVA_VULKAN_SHADER_ARTIFACT_KIND}");
    });
    Ok(binaries)
}
pub(super) struct BlendPipelineDescriptor<'a> {
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
    #[cfg(target_os = "windows")]
    pub(super) subpixel_vertex: gfx_core::ShaderModuleId,
    #[cfg(target_os = "windows")]
    pub(super) subpixel_fragment: gfx_core::ShaderModuleId,
    #[cfg(target_os = "windows")]
    pub(super) subpixel_grayscale_fragment: gfx_core::ShaderModuleId,
    pub(super) poly_vertex: gfx_core::ShaderModuleId,
    pub(super) poly_fragment: gfx_core::ShaderModuleId,
    pub(super) underline_vertex: gfx_core::ShaderModuleId,
    pub(super) underline_fragment: gfx_core::ShaderModuleId,
    pub(super) backdrop_blur_vertex: gfx_core::ShaderModuleId,
    pub(super) backdrop_blur_fragment: gfx_core::ShaderModuleId,
}

#[cfg(test)]
pub(super) fn compile_nova_shader_binaries(
    mut compile: impl FnMut(
        &str,
        ShaderStage,
        &str,
    ) -> std::result::Result<gfx_core::ShaderBinary, gfx_shader::ShaderError>,
) -> Result<ShaderBinaries> {
    Ok(ShaderBinaries {
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
        #[cfg(target_os = "windows")]
        subpixel_vertex: compile(
            NOVA_SUBPIXEL_SPRITE_SHADER_SOURCE,
            ShaderStage::Vertex,
            "vs_subpixel_sprite",
        )
        .context("compiling nova RGB subpixel sprite vertex shader")?,
        #[cfg(target_os = "windows")]
        subpixel_fragment: compile(
            NOVA_SUBPIXEL_SPRITE_SHADER_SOURCE,
            ShaderStage::Fragment,
            "fs_subpixel_sprite",
        )
        .context("compiling nova RGB subpixel sprite fragment shader")?,
        #[cfg(target_os = "windows")]
        subpixel_grayscale_fragment: compile(
            NOVA_SUBPIXEL_GRAYSCALE_SHADER_SOURCE,
            ShaderStage::Fragment,
            "fs_subpixel_sprite_grayscale",
        )
        .context("compiling nova transparent subpixel sprite fragment shader")?,
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
