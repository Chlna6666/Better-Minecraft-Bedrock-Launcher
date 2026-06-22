#![cfg_attr(
    not(any(
        all(feature = "nova-gfx-dx12", target_os = "windows"),
        all(feature = "nova-gfx-metal", target_os = "macos"),
        all(
            feature = "nova-gfx-vulkan",
            any(target_os = "windows", target_os = "linux", target_os = "freebsd")
        )
    )),
    allow(
        dead_code,
        unreachable_code,
        unused_assignments,
        unused_imports,
        unused_variables
    )
)]

use std::{
    borrow::Cow,
    sync::{Arc, Mutex},
    time::Instant,
};

use anyhow::{Context as _, Result};
use collections::FxHashMap;

use crate::{
    AtlasKey, AtlasTextureId, AtlasTextureKind, AtlasTile, Bounds, DevicePixels, FrameRenderPlan,
    GlyphRasterization, GpuSpecs, GpuSubmissionMode, GpuiMemoryTrimLevel, MonochromeSprite,
    PartialPresentMode, PlatformAtlas, Point, PolychromeSprite, PreparedSceneBatch, Quad,
    RenderGlyphParams, RendererBackend, Shadow, Size, TileId, Underline,
};

use gfx_core::{
    AddressMode, BlendMode, BufferBinding, BufferDesc, BufferId, BufferUsage, ClearColor,
    ColorAttachmentDesc, CompositeAlphaMode, DeviceDesc, DrawStepDesc, Extent2d, FilterMode,
    Format, GfxPipelineDevice, GfxPresentationDevice, GfxResourceDevice, GfxSubmissionDevice,
    GfxSurfaceDevice, LoadOp, MemoryLocation, Origin2d, PipelineLayoutDesc, PipelineLayoutId,
    PrimitiveTopology, RenderPassDesc, RenderPassId, RenderPipelineDesc, RenderPipelineId,
    ResourceBinding, ResourceBindingResource, ResourceBindingType, ResourceSetDesc, ResourceSetId,
    ResourceSetLayoutDesc, ResourceSetLayoutEntry, ResourceSetLayoutId, SamplerBinding,
    SamplerDesc, SamplerId, ScissorRect, ShaderModuleDesc, ShaderStage, ShaderStages, SubmissionId,
    SubmissionStatus, SurfaceConfig, SurfaceDesc, SurfaceId, SwapchainId, TextureBinding,
    TextureDataLayout, TextureDesc, TextureDimension, TextureId, TextureUsage, TextureViewDesc,
    TextureViewId, TextureWriteDesc,
};
#[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
use gfx_dx12::Dx12Device;
#[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
use gfx_metal::MetalDevice;
#[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
use gfx_shader::compile_wgsl_to_hlsl;
#[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
use gfx_shader::compile_wgsl_to_msl;
#[cfg(all(
    feature = "nova-gfx-vulkan",
    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
))]
use gfx_shader::compile_wgsl_to_spirv;
#[cfg(all(
    feature = "nova-gfx-vulkan",
    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
))]
use gfx_vulkan::VulkanDevice;

const MAX_QUADS: usize = 8192;
const MAX_SHADOWS: usize = 4096;
const MAX_PATH_VERTICES: usize = 65_536;
const MAX_PATH_SPRITES: usize = 4096;
const MAX_MONO_SPRITES: usize = 8192;
const MAX_POLY_SPRITES: usize = 4096;
const MAX_UNDERLINES: usize = 4096;
const MAX_BACKDROP_BLURS: usize = 1024;
const MAX_IN_FLIGHT_SUBMISSIONS: usize = 1;
const GLOBAL_UPLOAD_BYTES: usize = 16;
const TEXT_RASTER_UPLOAD_BYTES: usize = 32;
const BACKDROP_BLUR_PASS_BYTES: usize = 16;
const PACKED_QUAD_BYTES: usize = 160;
const PACKED_SHADOW_BYTES: usize = 72;
const PACKED_PATH_RASTERIZATION_VERTEX_BYTES: usize = 104;
const PACKED_PATH_SPRITE_BYTES: usize = 16;
const PACKED_MONO_SPRITE_BYTES: usize = 112;
const PACKED_POLY_SPRITE_BYTES: usize = 96;
const PACKED_UNDERLINE_BYTES: usize = 64;
const PACKED_BACKDROP_BLUR_BYTES: usize = 104;
const NOVA_ATLAS_SIZE: u32 = 4096;
const NOVA_ATLAS_BYTES_PER_PIXEL: usize = 4;
const NOVA_ATLAS_TILE_PADDING: u32 = 1;
const DEFAULT_BACKDROP_BLUR_DOWNSAMPLE: u8 = 2;
const MAX_BACKDROP_BLUR_LEVELS: u8 = 6;

#[allow(dead_code)]
const NOVA_SOLID_QUAD_SHADER_SOURCE: &str = concat!(
    include_str!("nova/shaders/core.wgsl"),
    include_str!("nova/shaders/solid_quad.wgsl"),
);

#[allow(dead_code)]
const NOVA_MONO_SPRITE_SHADER_SOURCE: &str = concat!(
    include_str!("nova/shaders/core.wgsl"),
    include_str!("nova/shaders/text.wgsl"),
    include_str!("nova/shaders/mono_sprite.wgsl"),
);

#[allow(dead_code)]
const NOVA_QUAD_SHADER_SOURCE: &str = concat!(
    include_str!("nova/shaders/core.wgsl"),
    include_str!("nova/shaders/shape.wgsl"),
    include_str!("nova/shaders/quad_common.wgsl"),
    include_str!("nova/shaders/quad.wgsl"),
);

#[allow(dead_code)]
const NOVA_SHADOW_SHADER_SOURCE: &str = concat!(
    include_str!("nova/shaders/core.wgsl"),
    include_str!("nova/shaders/shape.wgsl"),
    include_str!("nova/shaders/shadow.wgsl"),
);

#[allow(dead_code)]
const NOVA_PATH_SHADER_SOURCE: &str = concat!(
    include_str!("nova/shaders/core.wgsl"),
    include_str!("nova/shaders/quad_common.wgsl"),
    include_str!("nova/shaders/path.wgsl"),
);

#[allow(dead_code)]
const NOVA_UNDERLINE_SHADER_SOURCE: &str = concat!(
    include_str!("nova/shaders/core.wgsl"),
    include_str!("nova/shaders/underline.wgsl"),
);

#[allow(dead_code)]
const NOVA_POLY_SPRITE_SHADER_SOURCE: &str = concat!(
    include_str!("nova/shaders/core.wgsl"),
    include_str!("nova/shaders/shape.wgsl"),
    include_str!("nova/shaders/poly_sprite.wgsl"),
);

#[allow(dead_code)]
const NOVA_SURFACE_SHADER_SOURCE: &str = concat!(
    include_str!("nova/shaders/core.wgsl"),
    include_str!("nova/shaders/surface.wgsl"),
);

#[allow(dead_code)]
const NOVA_BACKDROP_BLUR_SHADER_SOURCE: &str = concat!(
    include_str!("nova/shaders/core.wgsl"),
    include_str!("nova/shaders/shape.wgsl"),
    include_str!("nova/shaders/backdrop_blur.wgsl"),
);

#[allow(dead_code)]
const NOVA_MESH_3D_SHADER_SOURCE: &str = concat!(
    include_str!("nova/shaders/core.wgsl"),
    include_str!("nova/shaders/mesh_3d.wgsl"),
);

#[derive(Clone, Copy, PartialEq, Eq)]
struct DrawableSize {
    width: u32,
    height: u32,
}

pub(crate) struct NovaRenderer {
    backend: NovaBackend,
    surface: SurfaceId,
    swapchain: SwapchainId,
    surface_format: Format,
    surface_alpha: NovaSurfaceAlphaState,
    render_pass: RenderPassId,
    pipelines: NovaPipelines,
    global_buffer: BufferId,
    text_raster_buffer: BufferId,
    quad_buffer: BufferId,
    shadow_buffer: BufferId,
    path_rasterization_vertex_buffer: BufferId,
    path_sprite_buffer: BufferId,
    mono_sprite_buffer: BufferId,
    poly_sprite_buffer: BufferId,
    underline_buffer: BufferId,
    backdrop_blur_pass_buffer: BufferId,
    backdrop_blur_buffer: BufferId,
    quad_resource_set: ResourceSetId,
    shadow_resource_set: ResourceSetId,
    path_rasterization_resource_set: ResourceSetId,
    path_resource_set_layout: ResourceSetLayoutId,
    path_resource_set: ResourceSetId,
    mono_sprite_resource_set: ResourceSetId,
    poly_sprite_resource_set: ResourceSetId,
    underline_resource_set: ResourceSetId,
    backdrop_blur_pass_resource_set_layout: ResourceSetLayoutId,
    backdrop_blur_resource_set_layout: ResourceSetLayoutId,
    backdrop_blur_targets: NovaBackdropBlurTargets,
    atlas_texture: TextureId,
    atlas_texture_view: TextureViewId,
    atlas_sampler: SamplerId,
    path_texture: TextureId,
    path_texture_view: TextureViewId,
    frame_upload: NovaFrameUpload,
    current_size: DrawableSize,
    atlas: Arc<NovaAtlas>,
    rendering_parameters: NovaRenderingParameters,
    diagnostics: NovaRenderDiagnostics,
    submission_mode: GpuSubmissionMode,
    pending_submissions: Vec<SubmissionId>,
    metrics_started_at: Instant,
    first_frame_reported: bool,
    submitted_frames: u64,
}

struct NovaPipelines {
    alpha: NovaBlendPipelines,
    premultiplied: NovaBlendPipelines,
    path_rasterization: RenderPipelineId,
    paths: RenderPipelineId,
    backdrop_blur_downsample: RenderPipelineId,
    backdrop_blur_upsample: RenderPipelineId,
}

#[derive(Clone, Copy)]
struct NovaBlendPipelines {
    solid_quads: RenderPipelineId,
    quads: RenderPipelineId,
    shadows: RenderPipelineId,
    mono_sprites: RenderPipelineId,
    poly_sprites: RenderPipelineId,
    underlines: RenderPipelineId,
    backdrop_blurs: RenderPipelineId,
}

struct NovaShaderBinaries {
    solid_vertex: gfx_core::ShaderBinary,
    solid_fragment: gfx_core::ShaderBinary,
    quad_vertex: gfx_core::ShaderBinary,
    quad_fragment: gfx_core::ShaderBinary,
    shadow_vertex: gfx_core::ShaderBinary,
    shadow_fragment: gfx_core::ShaderBinary,
    path_rasterization_vertex: gfx_core::ShaderBinary,
    path_rasterization_fragment: gfx_core::ShaderBinary,
    path_vertex: gfx_core::ShaderBinary,
    path_fragment: gfx_core::ShaderBinary,
    mono_vertex: gfx_core::ShaderBinary,
    mono_fragment: gfx_core::ShaderBinary,
    poly_vertex: gfx_core::ShaderBinary,
    poly_fragment: gfx_core::ShaderBinary,
    underline_vertex: gfx_core::ShaderBinary,
    underline_fragment: gfx_core::ShaderBinary,
    backdrop_blur_pass_vertex: gfx_core::ShaderBinary,
    backdrop_blur_downsample_fragment: gfx_core::ShaderBinary,
    backdrop_blur_upsample_fragment: gfx_core::ShaderBinary,
    backdrop_blur_vertex: gfx_core::ShaderBinary,
    backdrop_blur_fragment: gfx_core::ShaderBinary,
}

struct NovaBlendPipelineDesc<'a> {
    label: &'a str,
    suffix: &'a str,
    blend_mode: BlendMode,
    size: Extent2d,
    color_format: Format,
    render_pass: RenderPassId,
    quad_pipeline_layout: PipelineLayoutId,
    shadow_pipeline_layout: PipelineLayoutId,
    mono_pipeline_layout: PipelineLayoutId,
    poly_pipeline_layout: PipelineLayoutId,
    underline_pipeline_layout: PipelineLayoutId,
    backdrop_blur_pipeline_layout: PipelineLayoutId,
    solid_vertex: gfx_core::ShaderModuleId,
    solid_fragment: gfx_core::ShaderModuleId,
    quad_vertex: gfx_core::ShaderModuleId,
    quad_fragment: gfx_core::ShaderModuleId,
    shadow_vertex: gfx_core::ShaderModuleId,
    shadow_fragment: gfx_core::ShaderModuleId,
    mono_vertex: gfx_core::ShaderModuleId,
    mono_fragment: gfx_core::ShaderModuleId,
    poly_vertex: gfx_core::ShaderModuleId,
    poly_fragment: gfx_core::ShaderModuleId,
    underline_vertex: gfx_core::ShaderModuleId,
    underline_fragment: gfx_core::ShaderModuleId,
    backdrop_blur_vertex: gfx_core::ShaderModuleId,
    backdrop_blur_fragment: gfx_core::ShaderModuleId,
}

fn compile_nova_shader_binaries(
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

enum NovaBackend {
    #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
    Dx12(Dx12Device),
    #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
    Metal(MetalDevice),
    #[cfg(all(
        feature = "nova-gfx-vulkan",
        any(target_os = "windows", target_os = "linux", target_os = "freebsd")
    ))]
    Vulkan(VulkanDevice),
    #[cfg(not(any(
        all(feature = "nova-gfx-dx12", target_os = "windows"),
        all(feature = "nova-gfx-metal", target_os = "macos"),
        all(
            feature = "nova-gfx-vulkan",
            any(target_os = "windows", target_os = "linux", target_os = "freebsd")
        )
    )))]
    Unavailable,
}

impl NovaBackend {
    fn label(&self) -> &'static str {
        match self {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            Self::Dx12(_) => "nova-dx12",
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            Self::Metal(_) => "nova-metal",
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            Self::Vulkan(_) => "nova-vulkan",
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            Self::Unavailable => "nova-unavailable",
        }
    }

    fn async_capabilities(&self) -> gfx_core::GfxAsyncCapabilities {
        match self {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            Self::Dx12(device) => device.async_capabilities(),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            Self::Metal(device) => device.async_capabilities(),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            Self::Vulkan(device) => device.async_capabilities(),
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            Self::Unavailable => gfx_core::GfxAsyncCapabilities::default(),
        }
    }

    fn poll_submission(&mut self, submission: SubmissionId) -> Result<SubmissionStatus> {
        match self {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            Self::Dx12(device) => Ok(device.poll_submission(submission)?),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            Self::Metal(device) => Ok(device.poll_submission(submission)?),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            Self::Vulkan(device) => Ok(device.poll_submission(submission)?),
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            Self::Unavailable => Ok(SubmissionStatus::Complete),
        }
    }

    fn wait_submission(&mut self, submission: SubmissionId) -> Result<()> {
        match self {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            Self::Dx12(device) => Ok(device.wait_submission(submission)?),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            Self::Metal(device) => Ok(device.wait_submission(submission)?),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            Self::Vulkan(device) => Ok(device.wait_submission(submission)?),
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            Self::Unavailable => Ok(()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NovaSurfaceAlphaState {
    swapchain_mode: CompositeAlphaMode,
    output_mode: NovaSurfaceOutputMode,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NovaSurfaceOutputMode {
    Straight,
    Premultiplied,
}

impl NovaRenderer {
    pub(crate) fn new<W>(
        window: &W,
        backend: RendererBackend,
        submission_mode: GpuSubmissionMode,
        drawable_size: Size<DevicePixels>,
        transparent: bool,
    ) -> Result<Self>
    where
        W: raw_window_handle::HasDisplayHandle + raw_window_handle::HasWindowHandle + 'static,
    {
        let metrics_started_at = Instant::now();
        let width = drawable_size.width.0.max(1) as u32;
        let height = drawable_size.height.0.max(1) as u32;
        log::info!("renderer_path=nova-gfx backend={backend}");
        let mut surface_config = SurfaceConfig::new(width, height, Format::Bgra8Unorm)
            .context("creating nova-gfx surface config")?;
        surface_config.present_mode = gfx_core::PresentMode::Fifo;
        let surface_alpha =
            Self::alpha_state_for_window_transparency_on_backend(backend, transparent);
        surface_config.alpha_mode = surface_alpha.swapchain_mode;
        let current_size = DrawableSize { width, height };
        match backend {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            RendererBackend::NovaDx12 => {
                let shader_binaries = compile_nova_shader_binaries(compile_wgsl_to_hlsl)?;
                let mut device = Dx12Device::new(&DeviceDesc {
                    application_name: "gpui nova dx12".to_string(),
                })
                .context("creating nova DX12 device")?;
                let surface = device
                    .create_surface(window, &SurfaceDesc { label: None })
                    .context("creating nova DX12 surface")?;
                let swapchain = device
                    .create_swapchain(surface, surface_config)
                    .context("creating nova DX12 swapchain")?;
                let resources = create_gpui_resources(
                    &mut device,
                    surface_config,
                    "gpui nova dx12",
                    shader_binaries,
                )
                .context("creating GPUI nova DX12 render resources")?;
                Ok(Self {
                    backend: NovaBackend::Dx12(device),
                    surface,
                    swapchain,
                    surface_format: surface_config.format,
                    surface_alpha,
                    render_pass: resources.render_pass,
                    pipelines: resources.pipelines,
                    global_buffer: resources.global_buffer,
                    text_raster_buffer: resources.text_raster_buffer,
                    quad_buffer: resources.quad_buffer,
                    shadow_buffer: resources.shadow_buffer,
                    path_rasterization_vertex_buffer: resources.path_rasterization_vertex_buffer,
                    path_sprite_buffer: resources.path_sprite_buffer,
                    mono_sprite_buffer: resources.mono_sprite_buffer,
                    poly_sprite_buffer: resources.poly_sprite_buffer,
                    underline_buffer: resources.underline_buffer,
                    backdrop_blur_pass_buffer: resources.backdrop_blur_pass_buffer,
                    backdrop_blur_buffer: resources.backdrop_blur_buffer,
                    quad_resource_set: resources.quad_resource_set,
                    shadow_resource_set: resources.shadow_resource_set,
                    path_rasterization_resource_set: resources.path_rasterization_resource_set,
                    path_resource_set_layout: resources.path_resource_set_layout,
                    path_resource_set: resources.path_resource_set,
                    mono_sprite_resource_set: resources.mono_sprite_resource_set,
                    poly_sprite_resource_set: resources.poly_sprite_resource_set,
                    underline_resource_set: resources.underline_resource_set,
                    backdrop_blur_pass_resource_set_layout: resources
                        .backdrop_blur_pass_resource_set_layout,
                    backdrop_blur_resource_set_layout: resources.backdrop_blur_resource_set_layout,
                    backdrop_blur_targets: resources.backdrop_blur_targets,
                    atlas_texture: resources.atlas_texture,
                    atlas_texture_view: resources.atlas_texture_view,
                    atlas_sampler: resources.atlas_sampler,
                    path_texture: resources.path_texture,
                    path_texture_view: resources.path_texture_view,
                    frame_upload: NovaFrameUpload::default(),
                    current_size,
                    atlas: Arc::new(NovaAtlas::new()),
                    rendering_parameters: NovaRenderingParameters::from_env(),
                    diagnostics: NovaRenderDiagnostics::from_env(),
                    submission_mode,
                    pending_submissions: Vec::new(),
                    metrics_started_at,
                    first_frame_reported: false,
                    submitted_frames: 0,
                })
            }
            #[cfg(not(all(feature = "nova-gfx-dx12", target_os = "windows")))]
            RendererBackend::NovaDx12 => {
                anyhow::bail!(
                    "nova-gfx DX12 renderer requires the nova-gfx-dx12 feature on Windows"
                )
            }
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            RendererBackend::NovaMetal => {
                let shader_binaries = compile_nova_shader_binaries(compile_wgsl_to_msl)?;
                let mut device = MetalDevice::new(&DeviceDesc {
                    application_name: "gpui nova metal".to_string(),
                })
                .context("creating nova Metal device")?;
                let surface = device
                    .create_surface(window, &SurfaceDesc { label: None })
                    .context("creating nova Metal surface")?;
                let swapchain = device
                    .create_swapchain(surface, surface_config)
                    .context("creating nova Metal swapchain")?;
                let resources = create_gpui_resources(
                    &mut device,
                    surface_config,
                    "gpui nova metal",
                    shader_binaries,
                )
                .context("creating GPUI nova Metal render resources")?;
                Ok(Self {
                    backend: NovaBackend::Metal(device),
                    surface,
                    swapchain,
                    surface_format: surface_config.format,
                    surface_alpha,
                    render_pass: resources.render_pass,
                    pipelines: resources.pipelines,
                    global_buffer: resources.global_buffer,
                    text_raster_buffer: resources.text_raster_buffer,
                    quad_buffer: resources.quad_buffer,
                    shadow_buffer: resources.shadow_buffer,
                    path_rasterization_vertex_buffer: resources.path_rasterization_vertex_buffer,
                    path_sprite_buffer: resources.path_sprite_buffer,
                    mono_sprite_buffer: resources.mono_sprite_buffer,
                    poly_sprite_buffer: resources.poly_sprite_buffer,
                    underline_buffer: resources.underline_buffer,
                    backdrop_blur_pass_buffer: resources.backdrop_blur_pass_buffer,
                    backdrop_blur_buffer: resources.backdrop_blur_buffer,
                    quad_resource_set: resources.quad_resource_set,
                    shadow_resource_set: resources.shadow_resource_set,
                    path_rasterization_resource_set: resources.path_rasterization_resource_set,
                    path_resource_set_layout: resources.path_resource_set_layout,
                    path_resource_set: resources.path_resource_set,
                    mono_sprite_resource_set: resources.mono_sprite_resource_set,
                    poly_sprite_resource_set: resources.poly_sprite_resource_set,
                    underline_resource_set: resources.underline_resource_set,
                    backdrop_blur_pass_resource_set_layout: resources
                        .backdrop_blur_pass_resource_set_layout,
                    backdrop_blur_resource_set_layout: resources.backdrop_blur_resource_set_layout,
                    backdrop_blur_targets: resources.backdrop_blur_targets,
                    atlas_texture: resources.atlas_texture,
                    atlas_texture_view: resources.atlas_texture_view,
                    atlas_sampler: resources.atlas_sampler,
                    path_texture: resources.path_texture,
                    path_texture_view: resources.path_texture_view,
                    frame_upload: NovaFrameUpload::default(),
                    current_size,
                    atlas: Arc::new(NovaAtlas::new()),
                    rendering_parameters: NovaRenderingParameters::from_env(),
                    diagnostics: NovaRenderDiagnostics::from_env(),
                    submission_mode,
                    pending_submissions: Vec::new(),
                    metrics_started_at,
                    first_frame_reported: false,
                    submitted_frames: 0,
                })
            }
            #[cfg(not(all(feature = "nova-gfx-metal", target_os = "macos")))]
            RendererBackend::NovaMetal => {
                anyhow::bail!(
                    "nova-gfx Metal renderer requires the nova-gfx-metal feature on macOS"
                )
            }
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            RendererBackend::NovaVulkan => {
                let shader_binaries = compile_nova_shader_binaries(compile_wgsl_to_spirv)?;
                let mut device = VulkanDevice::new(&DeviceDesc {
                    application_name: "gpui nova vulkan".to_string(),
                })
                .context("creating nova Vulkan device")?;
                let surface = device
                    .create_surface(window, &SurfaceDesc { label: None })
                    .context("creating nova Vulkan surface")?;
                let swapchain = device
                    .create_swapchain(surface, surface_config)
                    .context("creating nova Vulkan swapchain")?;
                let resources = create_gpui_resources(
                    &mut device,
                    surface_config,
                    "gpui nova vulkan",
                    shader_binaries,
                )
                .context("creating GPUI nova Vulkan render resources")?;
                Ok(Self {
                    backend: NovaBackend::Vulkan(device),
                    surface,
                    swapchain,
                    surface_format: surface_config.format,
                    surface_alpha,
                    render_pass: resources.render_pass,
                    pipelines: resources.pipelines,
                    global_buffer: resources.global_buffer,
                    text_raster_buffer: resources.text_raster_buffer,
                    quad_buffer: resources.quad_buffer,
                    shadow_buffer: resources.shadow_buffer,
                    path_rasterization_vertex_buffer: resources.path_rasterization_vertex_buffer,
                    path_sprite_buffer: resources.path_sprite_buffer,
                    mono_sprite_buffer: resources.mono_sprite_buffer,
                    poly_sprite_buffer: resources.poly_sprite_buffer,
                    underline_buffer: resources.underline_buffer,
                    backdrop_blur_pass_buffer: resources.backdrop_blur_pass_buffer,
                    backdrop_blur_buffer: resources.backdrop_blur_buffer,
                    quad_resource_set: resources.quad_resource_set,
                    shadow_resource_set: resources.shadow_resource_set,
                    path_rasterization_resource_set: resources.path_rasterization_resource_set,
                    path_resource_set_layout: resources.path_resource_set_layout,
                    path_resource_set: resources.path_resource_set,
                    mono_sprite_resource_set: resources.mono_sprite_resource_set,
                    poly_sprite_resource_set: resources.poly_sprite_resource_set,
                    underline_resource_set: resources.underline_resource_set,
                    backdrop_blur_pass_resource_set_layout: resources
                        .backdrop_blur_pass_resource_set_layout,
                    backdrop_blur_resource_set_layout: resources.backdrop_blur_resource_set_layout,
                    backdrop_blur_targets: resources.backdrop_blur_targets,
                    atlas_texture: resources.atlas_texture,
                    atlas_texture_view: resources.atlas_texture_view,
                    atlas_sampler: resources.atlas_sampler,
                    path_texture: resources.path_texture,
                    path_texture_view: resources.path_texture_view,
                    frame_upload: NovaFrameUpload::default(),
                    current_size,
                    atlas: Arc::new(NovaAtlas::new()),
                    rendering_parameters: NovaRenderingParameters::from_env(),
                    diagnostics: NovaRenderDiagnostics::from_env(),
                    submission_mode,
                    pending_submissions: Vec::new(),
                    metrics_started_at,
                    first_frame_reported: false,
                    submitted_frames: 0,
                })
            }
            #[cfg(not(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            )))]
            RendererBackend::NovaVulkan => {
                anyhow::bail!(
                    "nova-gfx Vulkan renderer requires the nova-gfx-vulkan feature on Windows/Linux"
                )
            }
            RendererBackend::Auto | RendererBackend::HeadlessTest => {
                anyhow::bail!("{backend} is not a concrete nova-gfx renderer")
            }
        }
    }

    pub(crate) fn resize(&mut self, size: Size<DevicePixels>) -> Result<()> {
        self.wait_for_pending_submissions()?;
        let width = size.width.0.max(1) as u32;
        let height = size.height.0.max(1) as u32;
        let next_size = DrawableSize { width, height };
        if next_size == self.current_size {
            return Ok(());
        }
        let target_size = Extent2d::new(width, height)?;
        let path_target_desc = self.path_target_desc(target_size);
        let backdrop_blur_target_desc = self.backdrop_blur_target_desc(target_size);
        let old_path_target = self.current_path_target();
        let old_backdrop_blur_targets = self.current_backdrop_blur_targets();
        let (next_path_target, next_backdrop_blur_targets): (
            NovaPathTarget,
            NovaBackdropBlurTargets,
        ) = match &mut self.backend {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => {
                device.resize_swapchain(self.swapchain, width, height)?;
                let next_path_target =
                    create_path_target(device, "gpui nova dx12", path_target_desc)?;
                let next_backdrop_blur_targets = create_backdrop_blur_targets(
                    device,
                    "gpui nova dx12",
                    backdrop_blur_target_desc,
                )?;
                destroy_path_target(device, old_path_target, "DX12");
                destroy_backdrop_blur_targets(device, old_backdrop_blur_targets, "DX12");
                (next_path_target, next_backdrop_blur_targets)
            }
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => {
                device.resize_swapchain(self.swapchain, width, height)?;
                let next_path_target =
                    create_path_target(device, "gpui nova metal", path_target_desc)?;
                let next_backdrop_blur_targets = create_backdrop_blur_targets(
                    device,
                    "gpui nova metal",
                    backdrop_blur_target_desc,
                )?;
                destroy_path_target(device, old_path_target, "Metal");
                destroy_backdrop_blur_targets(device, old_backdrop_blur_targets, "Metal");
                (next_path_target, next_backdrop_blur_targets)
            }
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => {
                device.resize_swapchain(self.swapchain, width, height)?;
                let next_path_target =
                    create_path_target(device, "gpui nova vulkan", path_target_desc)?;
                let next_backdrop_blur_targets = create_backdrop_blur_targets(
                    device,
                    "gpui nova vulkan",
                    backdrop_blur_target_desc,
                )?;
                destroy_path_target(device, old_path_target, "Vulkan");
                destroy_backdrop_blur_targets(device, old_backdrop_blur_targets, "Vulkan");
                (next_path_target, next_backdrop_blur_targets)
            }
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            NovaBackend::Unavailable => {
                anyhow::bail!("nova-gfx renderer requires an explicit nova-gfx backend feature")
            }
        };
        self.path_texture = next_path_target.texture;
        self.path_texture_view = next_path_target.texture_view;
        self.path_resource_set = next_path_target.resource_set;
        self.backdrop_blur_targets = next_backdrop_blur_targets;
        self.current_size = next_size;
        Ok(())
    }

    #[cfg_attr(target_os = "windows", allow(dead_code))]
    pub(crate) fn viewport_size(&self) -> Size<DevicePixels> {
        Size {
            width: DevicePixels(self.current_size.width as i32),
            height: DevicePixels(self.current_size.height as i32),
        }
    }

    pub(crate) fn draw(&mut self, render_plan: FrameRenderPlan<'_>) -> Result<()> {
        self.observe_render_plan(render_plan);
        let upload = self.frame_upload.encode(
            render_plan.scene,
            self.current_size,
            &self.rendering_parameters,
            self.surface_alpha.outputs_premultiplied_alpha(),
        );
        if !self.frame_upload.backdrop_blurs.is_empty() {
            self.ensure_backdrop_blur_targets()?;
        }
        self.draw_present(upload, render_plan)
    }

    fn ensure_backdrop_blur_targets(&mut self) -> Result<()> {
        let downsample = self.frame_upload.backdrop_blur_downsample();
        if self.backdrop_blur_targets.downsample == downsample {
            return Ok(());
        }
        let target_size = Extent2d::new(self.current_size.width, self.current_size.height)?;
        let backdrop_blur_target_desc = self.backdrop_blur_target_desc(target_size);
        let old_backdrop_blur_targets = self.current_backdrop_blur_targets();
        let next_backdrop_blur_targets = match &mut self.backend {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => {
                let targets = create_backdrop_blur_targets(
                    device,
                    "gpui nova dx12",
                    backdrop_blur_target_desc,
                )?;
                destroy_backdrop_blur_targets(device, old_backdrop_blur_targets, "DX12");
                targets
            }
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => {
                let targets = create_backdrop_blur_targets(
                    device,
                    "gpui nova metal",
                    backdrop_blur_target_desc,
                )?;
                destroy_backdrop_blur_targets(device, old_backdrop_blur_targets, "Metal");
                targets
            }
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => {
                let targets = create_backdrop_blur_targets(
                    device,
                    "gpui nova vulkan",
                    backdrop_blur_target_desc,
                )?;
                destroy_backdrop_blur_targets(device, old_backdrop_blur_targets, "Vulkan");
                targets
            }
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            NovaBackend::Unavailable => {
                anyhow::bail!("nova-gfx renderer requires an explicit nova-gfx backend feature")
            }
        };
        self.backdrop_blur_targets = next_backdrop_blur_targets;
        Ok(())
    }

    pub(crate) fn present_framebuffer_only(
        &mut self,
        render_plan: FrameRenderPlan<'_>,
    ) -> Result<()> {
        self.observe_render_plan(render_plan);
        let render_plan = FrameRenderPlan::full_redraw(render_plan.scene, render_plan.dirty_region);
        let upload = self.frame_upload.encode(
            render_plan.scene,
            self.current_size,
            &self.rendering_parameters,
            self.surface_alpha.outputs_premultiplied_alpha(),
        );
        self.draw_present(upload, render_plan)
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn draw_scene_for_platform(&mut self, scene: &crate::Scene) -> Result<()> {
        let upload = self.frame_upload.encode(
            scene,
            self.current_size,
            &self.rendering_parameters,
            self.surface_alpha.outputs_premultiplied_alpha(),
        );
        let dirty_region = crate::DirtyRegion::default();
        self.draw_present(upload, FrameRenderPlan::full_redraw(scene, &dirty_region))
    }

    pub(crate) fn update_transparency(&mut self, transparent: bool) {
        let previous_alpha = self.surface_alpha;
        let next_alpha = self.alpha_state_for_current_backend_transparency(transparent);
        if self.surface_alpha == next_alpha {
            return;
        }
        if let Err(error) = self.reconfigure_surface_alpha(next_alpha) {
            log::warn!(
                concat!(
                    "failed to reconfigure nova-gfx surface alpha mode: backend={} ",
                    "swapchain=index:{} generation:{} old_swapchain={:?} old_output={:?} ",
                    "new_swapchain={:?} new_output={:?} error={:#}"
                ),
                self.backend.label(),
                self.swapchain.index(),
                self.swapchain.generation(),
                previous_alpha.swapchain_mode,
                previous_alpha.output_mode,
                next_alpha.swapchain_mode,
                next_alpha.output_mode,
                error
            );
        }
    }

    pub(crate) fn is_subpixel_rendering_supported(&self) -> bool {
        false
    }

    pub(crate) fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.atlas.clone()
    }

    pub(crate) fn gpu_specs(&self) -> GpuSpecs {
        let (device_name, driver_name) = match self.backend {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(_) => ("nova-gfx DX12", "nova-dx12"),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(_) => ("nova-gfx Metal", "nova-metal"),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(_) => ("nova-gfx Vulkan", "nova-vulkan"),
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            NovaBackend::Unavailable => ("nova-gfx unavailable", "nova-unavailable"),
        };
        GpuSpecs {
            is_software_emulated: false,
            device_name: device_name.to_string(),
            driver_name: driver_name.to_string(),
            driver_info: "phase2b2-nova-batch-smoke".to_string(),
        }
    }

    pub(crate) fn trim_gpui_memory(&mut self, level: GpuiMemoryTrimLevel) {
        self.atlas.trim(level);
        self.frame_upload.trim_retained_capacity(level);
    }

    pub(crate) fn destroy(&mut self) {
        if let Err(error) = self.wait_for_pending_submissions() {
            log::debug!("failed to drain nova-gfx submissions during renderer destroy: {error}");
        }
    }

    fn observe_render_plan(&mut self, render_plan: FrameRenderPlan<'_>) {
        let _ = (
            render_plan.dirty_region.is_full(),
            render_plan.dirty_region.rect_count(),
            render_plan.partial_present_mode,
            render_plan.trim_policy,
        );
    }

    fn prepare_for_frame_submission(&mut self) -> Result<()> {
        if self.presentation_submission_mode() == GpuSubmissionMode::Synchronous {
            self.wait_for_pending_submissions()?;
            return Ok(());
        }
        self.poll_pending_submissions()?;
        if self.pending_submissions.len() >= MAX_IN_FLIGHT_SUBMISSIONS {
            self.wait_for_oldest_submission()?;
        }
        Ok(())
    }

    fn poll_pending_submissions(&mut self) -> Result<()> {
        let mut index = 0;
        while index < self.pending_submissions.len() {
            let submission = self.pending_submissions[index];
            let status = self.backend.poll_submission(submission)?;
            match status {
                SubmissionStatus::Pending => index += 1,
                SubmissionStatus::Complete => {
                    self.pending_submissions.remove(index);
                }
                SubmissionStatus::Failed(error) => {
                    self.pending_submissions.remove(index);
                    return Err(gfx_core::GfxError::Backend(error).into());
                }
            }
        }
        Ok(())
    }

    fn wait_for_oldest_submission(&mut self) -> Result<()> {
        let Some(submission) = self.pending_submissions.first().copied() else {
            return Ok(());
        };
        self.backend.wait_submission(submission)?;
        self.pending_submissions.remove(0);
        Ok(())
    }

    fn wait_for_pending_submissions(&mut self) -> Result<()> {
        while let Some(submission) = self.pending_submissions.first().copied() {
            self.backend.wait_submission(submission)?;
            self.pending_submissions.remove(0);
        }
        Ok(())
    }

    fn submit_present_frame<D>(
        submission_mode: GpuSubmissionMode,
        pending_submissions: &mut Vec<SubmissionId>,
        device: &mut D,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: &[DrawStepDesc],
        clear_color: ClearColor,
    ) -> Result<()>
    where
        D: GfxPresentationDevice + GfxSubmissionDevice,
    {
        if submission_mode == GpuSubmissionMode::Synchronous {
            device.draw_steps_and_present(swapchain, render_pass, steps, clear_color)?;
            return Ok(());
        }

        let submission =
            device.draw_steps_and_present_deferred(swapchain, render_pass, steps, clear_color)?;
        if submission.raw() != 0 {
            pending_submissions.push(submission);
        }
        Ok(())
    }

    fn presentation_submission_mode(&self) -> GpuSubmissionMode {
        if self.requires_synchronous_present_boundary() {
            GpuSubmissionMode::Synchronous
        } else {
            self.submission_mode
        }
    }

    fn requires_synchronous_present_boundary(&self) -> bool {
        #[cfg(target_os = "windows")]
        {
            matches!(self.backend, NovaBackend::Dx12(_))
                && self.surface_alpha.outputs_premultiplied_alpha()
        }
        #[cfg(not(target_os = "windows"))]
        {
            false
        }
    }

    fn draw_present(
        &mut self,
        upload: FrameUploadSummary,
        render_plan: FrameRenderPlan<'_>,
    ) -> Result<()> {
        self.prepare_for_frame_submission()?;
        let frame_started = Instant::now();
        let atlas_texture = self.atlas_texture;
        let backend_label = self.backend.label();
        let async_capabilities = self.backend.async_capabilities();
        let submission_mode = self.presentation_submission_mode();
        let draw_steps = self.draw_steps(
            render_plan,
            upload.unsupported_batches,
            async_capabilities.partial_presentation,
        );
        let path_mask_steps = self.path_mask_draw_steps();
        let has_backdrop_blurs = self.has_backdrop_blurs();
        let backdrop_blur_source_steps = if has_backdrop_blurs {
            self.backdrop_blur_source_steps()
        } else {
            Vec::new()
        };
        let backdrop_blur_passes = if has_backdrop_blurs {
            self.backdrop_blur_render_passes()
        } else {
            Vec::new()
        };
        let mask_pass_count = usize::from(!path_mask_steps.is_empty());
        let main_pass_count = 1;
        let composite_pass_count =
            usize::from(has_backdrop_blurs).saturating_add(backdrop_blur_passes.len());
        crate::performance_metrics::record_gpu_pass_metrics(
            mask_pass_count,
            main_pass_count,
            composite_pass_count,
        );
        let unsupported = upload.unsupported_batches;
        let uploaded_bytes = self.frame_upload.uploaded_bytes();
        if self.diagnostics.should_warn_unsupported(unsupported) {
            log::warn!(
                concat!(
                    "nova-gfx unsupported or fallback batches: backend={} ",
                    "paths={} surfaces={} backdrop_blurs={} backdrop_blur_tint_fallbacks={} ",
                    "gpu_meshes_3d={} set GPUI_NOVA_RENDER_DIAGNOSTICS=1 for every-frame details"
                ),
                backend_label,
                unsupported.paths,
                unsupported.surfaces,
                unsupported.backdrop_blurs,
                unsupported.backdrop_blur_tint_fallbacks,
                unsupported.gpu_meshes_3d,
            );
        }
        if self.diagnostics.enabled {
            log::warn!(
                concat!(
                    "nova-gfx frame diagnostics: backend={} alpha_swapchain={:?} ",
                    "alpha_output={:?} premultiplied={} quads={} shadows={} paths={} ",
                    "path_vertices={} mono_sprites={} poly_sprites={} underlines={} ",
                    "draw_steps={} path_mask_steps={} gpu_passes={} upload_bytes={} ",
                    "async_submission={} async_wait={} async_presentation={} ",
                    "partial_presentation={} threading={:?}"
                ),
                backend_label,
                self.surface_alpha.swapchain_mode,
                self.surface_alpha.output_mode,
                self.surface_alpha.outputs_premultiplied_alpha(),
                upload.quad_count,
                upload.shadow_count,
                upload.path_sprite_count,
                upload.path_vertex_count,
                upload.mono_sprite_count,
                upload.poly_sprite_count,
                upload.underline_count,
                draw_steps.len(),
                path_mask_steps.len(),
                mask_pass_count
                    .saturating_add(main_pass_count)
                    .saturating_add(composite_pass_count),
                uploaded_bytes,
                async_capabilities.async_submission,
                async_capabilities.async_wait,
                async_capabilities.async_presentation,
                async_capabilities.partial_presentation,
                async_capabilities.threading_mode,
            );
        } else {
            log::debug!(
                concat!(
                    "nova-gfx frame upload: alpha_swapchain={:?} alpha_output={:?} ",
                    "quads={} shadows={} paths={} mono_sprites={} poly_sprites={} ",
                    "underlines={} draw_steps={} path_mask_steps={} gpu_passes={}"
                ),
                self.surface_alpha.swapchain_mode,
                self.surface_alpha.output_mode,
                upload.quad_count,
                upload.shadow_count,
                upload.path_sprite_count,
                upload.mono_sprite_count,
                upload.poly_sprite_count,
                upload.underline_count,
                draw_steps.len(),
                path_mask_steps.len(),
                mask_pass_count
                    .saturating_add(main_pass_count)
                    .saturating_add(composite_pass_count),
            );
        }
        let render_result: Result<()> = match &mut self.backend {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => {
                device.write_buffer(self.global_buffer, 0, &self.frame_upload.globals)?;
                device.write_buffer(
                    self.text_raster_buffer,
                    0,
                    &self.frame_upload.text_raster_params,
                )?;
                if !self.frame_upload.quads.is_empty() {
                    device.write_buffer(self.quad_buffer, 0, &self.frame_upload.quads)?;
                }
                if !self.frame_upload.shadows.is_empty() {
                    device.write_buffer(self.shadow_buffer, 0, &self.frame_upload.shadows)?;
                }
                if !self.frame_upload.path_rasterization_vertices.is_empty() {
                    device.write_buffer(
                        self.path_rasterization_vertex_buffer,
                        0,
                        &self.frame_upload.path_rasterization_vertices,
                    )?;
                }
                if !self.frame_upload.path_sprites.is_empty() {
                    device.write_buffer(
                        self.path_sprite_buffer,
                        0,
                        &self.frame_upload.path_sprites,
                    )?;
                }
                if !self.frame_upload.mono_sprites.is_empty() {
                    device.write_buffer(
                        self.mono_sprite_buffer,
                        0,
                        &self.frame_upload.mono_sprites,
                    )?;
                }
                if !self.frame_upload.poly_sprites.is_empty() {
                    device.write_buffer(
                        self.poly_sprite_buffer,
                        0,
                        &self.frame_upload.poly_sprites,
                    )?;
                }
                if !self.frame_upload.underlines.is_empty() {
                    device.write_buffer(self.underline_buffer, 0, &self.frame_upload.underlines)?;
                }
                if has_backdrop_blurs {
                    device.write_buffer(
                        self.backdrop_blur_pass_buffer,
                        0,
                        &self.frame_upload.backdrop_blur_passes,
                    )?;
                    device.write_buffer(
                        self.backdrop_blur_buffer,
                        0,
                        &self.frame_upload.backdrop_blurs,
                    )?;
                }
                let atlas_stats = upload_pending_atlas(&self.atlas, device, atlas_texture)?;
                record_nova_upload_metrics(self.frame_upload.uploaded_bytes(), atlas_stats);
                if !path_mask_steps.is_empty() {
                    device.draw_steps_to_texture(
                        self.path_texture_view,
                        self.render_pass,
                        &path_mask_steps,
                        LoadOp::Clear(clear_color()),
                    )?;
                }
                if has_backdrop_blurs {
                    device.draw_steps_to_texture(
                        self.backdrop_blur_targets.source.texture_view,
                        self.render_pass,
                        &backdrop_blur_source_steps,
                        LoadOp::Clear(clear_color()),
                    )?;
                    for pass in &backdrop_blur_passes {
                        device.draw_steps_to_texture(
                            pass.target_texture_view,
                            self.render_pass,
                            &pass.steps,
                            LoadOp::Clear(clear_color()),
                        )?;
                    }
                }
                Self::submit_present_frame(
                    submission_mode,
                    &mut self.pending_submissions,
                    device,
                    self.swapchain,
                    self.render_pass,
                    &draw_steps,
                    clear_color(),
                )?;
                Ok(())
            }
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => {
                device.write_buffer(self.global_buffer, 0, &self.frame_upload.globals)?;
                device.write_buffer(
                    self.text_raster_buffer,
                    0,
                    &self.frame_upload.text_raster_params,
                )?;
                if !self.frame_upload.quads.is_empty() {
                    device.write_buffer(self.quad_buffer, 0, &self.frame_upload.quads)?;
                }
                if !self.frame_upload.shadows.is_empty() {
                    device.write_buffer(self.shadow_buffer, 0, &self.frame_upload.shadows)?;
                }
                if !self.frame_upload.path_rasterization_vertices.is_empty() {
                    device.write_buffer(
                        self.path_rasterization_vertex_buffer,
                        0,
                        &self.frame_upload.path_rasterization_vertices,
                    )?;
                }
                if !self.frame_upload.path_sprites.is_empty() {
                    device.write_buffer(
                        self.path_sprite_buffer,
                        0,
                        &self.frame_upload.path_sprites,
                    )?;
                }
                if !self.frame_upload.mono_sprites.is_empty() {
                    device.write_buffer(
                        self.mono_sprite_buffer,
                        0,
                        &self.frame_upload.mono_sprites,
                    )?;
                }
                if !self.frame_upload.poly_sprites.is_empty() {
                    device.write_buffer(
                        self.poly_sprite_buffer,
                        0,
                        &self.frame_upload.poly_sprites,
                    )?;
                }
                if !self.frame_upload.underlines.is_empty() {
                    device.write_buffer(self.underline_buffer, 0, &self.frame_upload.underlines)?;
                }
                if has_backdrop_blurs {
                    device.write_buffer(
                        self.backdrop_blur_pass_buffer,
                        0,
                        &self.frame_upload.backdrop_blur_passes,
                    )?;
                    device.write_buffer(
                        self.backdrop_blur_buffer,
                        0,
                        &self.frame_upload.backdrop_blurs,
                    )?;
                }
                let atlas_stats = upload_pending_atlas(&self.atlas, device, atlas_texture)?;
                record_nova_upload_metrics(self.frame_upload.uploaded_bytes(), atlas_stats);
                if !path_mask_steps.is_empty() {
                    device.draw_steps_to_texture(
                        self.path_texture_view,
                        self.render_pass,
                        &path_mask_steps,
                        LoadOp::Clear(clear_color()),
                    )?;
                }
                if has_backdrop_blurs {
                    device.draw_steps_to_texture(
                        self.backdrop_blur_targets.source.texture_view,
                        self.render_pass,
                        &backdrop_blur_source_steps,
                        LoadOp::Clear(clear_color()),
                    )?;
                    for pass in &backdrop_blur_passes {
                        device.draw_steps_to_texture(
                            pass.target_texture_view,
                            self.render_pass,
                            &pass.steps,
                            LoadOp::Clear(clear_color()),
                        )?;
                    }
                }
                Self::submit_present_frame(
                    submission_mode,
                    &mut self.pending_submissions,
                    device,
                    self.swapchain,
                    self.render_pass,
                    &draw_steps,
                    clear_color(),
                )?;
                Ok(())
            }
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => {
                device.write_buffer(self.global_buffer, 0, &self.frame_upload.globals)?;
                device.write_buffer(
                    self.text_raster_buffer,
                    0,
                    &self.frame_upload.text_raster_params,
                )?;
                if !self.frame_upload.quads.is_empty() {
                    device.write_buffer(self.quad_buffer, 0, &self.frame_upload.quads)?;
                }
                if !self.frame_upload.shadows.is_empty() {
                    device.write_buffer(self.shadow_buffer, 0, &self.frame_upload.shadows)?;
                }
                if !self.frame_upload.path_rasterization_vertices.is_empty() {
                    device.write_buffer(
                        self.path_rasterization_vertex_buffer,
                        0,
                        &self.frame_upload.path_rasterization_vertices,
                    )?;
                }
                if !self.frame_upload.path_sprites.is_empty() {
                    device.write_buffer(
                        self.path_sprite_buffer,
                        0,
                        &self.frame_upload.path_sprites,
                    )?;
                }
                if !self.frame_upload.mono_sprites.is_empty() {
                    device.write_buffer(
                        self.mono_sprite_buffer,
                        0,
                        &self.frame_upload.mono_sprites,
                    )?;
                }
                if !self.frame_upload.poly_sprites.is_empty() {
                    device.write_buffer(
                        self.poly_sprite_buffer,
                        0,
                        &self.frame_upload.poly_sprites,
                    )?;
                }
                if !self.frame_upload.underlines.is_empty() {
                    device.write_buffer(self.underline_buffer, 0, &self.frame_upload.underlines)?;
                }
                if has_backdrop_blurs {
                    device.write_buffer(
                        self.backdrop_blur_pass_buffer,
                        0,
                        &self.frame_upload.backdrop_blur_passes,
                    )?;
                    device.write_buffer(
                        self.backdrop_blur_buffer,
                        0,
                        &self.frame_upload.backdrop_blurs,
                    )?;
                }
                let atlas_stats = upload_pending_atlas(&self.atlas, device, atlas_texture)?;
                record_nova_upload_metrics(self.frame_upload.uploaded_bytes(), atlas_stats);
                if !path_mask_steps.is_empty() {
                    device.draw_steps_to_texture(
                        self.path_texture_view,
                        self.render_pass,
                        &path_mask_steps,
                        LoadOp::Clear(clear_color()),
                    )?;
                }
                if has_backdrop_blurs {
                    device.draw_steps_to_texture(
                        self.backdrop_blur_targets.source.texture_view,
                        self.render_pass,
                        &backdrop_blur_source_steps,
                        LoadOp::Clear(clear_color()),
                    )?;
                    for pass in &backdrop_blur_passes {
                        device.draw_steps_to_texture(
                            pass.target_texture_view,
                            self.render_pass,
                            &pass.steps,
                            LoadOp::Clear(clear_color()),
                        )?;
                    }
                }
                Self::submit_present_frame(
                    submission_mode,
                    &mut self.pending_submissions,
                    device,
                    self.swapchain,
                    self.render_pass,
                    &draw_steps,
                    clear_color(),
                )?;
                Ok(())
            }
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            NovaBackend::Unavailable => {
                anyhow::bail!("nova-gfx renderer requires an explicit nova-gfx backend feature")
            }
        };
        let frame_elapsed_ms = frame_started.elapsed().as_millis();
        if let Err(error) = &render_result {
            log::error!(
                concat!(
                    "nova-gfx frame render failed: backend={} alpha_swapchain={:?} ",
                    "alpha_output={:?} quads={} shadows={} paths={} mono_sprites={} ",
                    "poly_sprites={} underlines={} draw_steps={} path_mask_steps={} ",
                    "upload_bytes={} elapsed_ms={} error={:#}"
                ),
                backend_label,
                self.surface_alpha.swapchain_mode,
                self.surface_alpha.output_mode,
                upload.quad_count,
                upload.shadow_count,
                upload.path_sprite_count,
                upload.mono_sprite_count,
                upload.poly_sprite_count,
                upload.underline_count,
                draw_steps.len(),
                path_mask_steps.len(),
                uploaded_bytes,
                frame_elapsed_ms,
                error,
            );
        }
        render_result?;
        crate::performance_metrics::record_present();
        if self.diagnostics.should_log_frame(frame_elapsed_ms) {
            log::warn!(
                concat!(
                    "nova-gfx frame completed: backend={} elapsed_ms={} ",
                    "alpha_swapchain={:?} alpha_output={:?} quads={} shadows={} paths={} ",
                    "mono_sprites={} poly_sprites={} underlines={} draw_steps={} ",
                    "path_mask_steps={} gpu_passes={} upload_bytes={}"
                ),
                backend_label,
                frame_elapsed_ms,
                self.surface_alpha.swapchain_mode,
                self.surface_alpha.output_mode,
                upload.quad_count,
                upload.shadow_count,
                upload.path_sprite_count,
                upload.mono_sprite_count,
                upload.poly_sprite_count,
                upload.underline_count,
                draw_steps.len(),
                path_mask_steps.len(),
                mask_pass_count
                    .saturating_add(main_pass_count)
                    .saturating_add(composite_pass_count),
                uploaded_bytes,
            );
        }
        self.submitted_frames = self.submitted_frames.saturating_add(1);
        if !self.first_frame_reported {
            self.first_frame_reported = true;
            log::info!(
                "GPUI nova-gfx first frame: renderer_path=nova-gfx phase=path-offscreen first_frame_time_ms={} submitted_frames={} quads={} paths={} mono_sprites={}",
                self.metrics_started_at.elapsed().as_millis(),
                self.submitted_frames,
                upload.quad_count,
                upload.path_sprite_count,
                upload.mono_sprite_count
            );
        }
        let _ = (
            self.surface,
            self.atlas_texture_view,
            self.atlas_sampler,
            self.path_texture,
        );
        Ok(())
    }

    fn draw_steps(
        &self,
        render_plan: FrameRenderPlan<'_>,
        unsupported_batches: UnsupportedBatchSummary,
        partial_presentation_supported: bool,
    ) -> Vec<DrawStepDesc> {
        let blend_pipelines = self.current_blend_pipelines();
        let mut steps = draw_steps_for_upload(
            &self.frame_upload,
            &self.pipelines,
            blend_pipelines,
            self.quad_resource_set,
            self.shadow_resource_set,
            self.path_resource_set,
            self.mono_sprite_resource_set,
            self.poly_sprite_resource_set,
            self.underline_resource_set,
            self.backdrop_blur_targets.target_resource_set,
            NovaDrawStepMode::Present,
        );
        if partial_presentation_supported
            && unsupported_batches.total() == 0
            && let Some(scissor) = partial_scissor_for_plan(render_plan, self.current_size)
        {
            apply_scissor_to_steps(&mut steps, scissor);
        }
        steps
    }

    fn backdrop_blur_source_steps(&self) -> Vec<DrawStepDesc> {
        let blend_pipelines = self.current_blend_pipelines();
        draw_steps_for_upload(
            &self.frame_upload,
            &self.pipelines,
            blend_pipelines,
            self.quad_resource_set,
            self.shadow_resource_set,
            self.path_resource_set,
            self.mono_sprite_resource_set,
            self.poly_sprite_resource_set,
            self.underline_resource_set,
            self.backdrop_blur_targets.target_resource_set,
            NovaDrawStepMode::BackdropSource,
        )
    }

    fn backdrop_blur_render_passes(&self) -> Vec<NovaBackdropBlurRenderPass> {
        backdrop_blur_render_passes_for_targets(
            &self.pipelines,
            &self.backdrop_blur_targets,
            self.frame_upload.backdrop_blur_levels(),
        )
    }

    fn has_backdrop_blurs(&self) -> bool {
        !self.frame_upload.backdrop_blurs.is_empty()
    }

    fn current_blend_pipelines(&self) -> NovaBlendPipelines {
        if self.surface_alpha.outputs_premultiplied_alpha() {
            self.pipelines.premultiplied
        } else {
            self.pipelines.alpha
        }
    }

    fn path_mask_draw_steps(&self) -> Vec<DrawStepDesc> {
        path_mask_draw_steps_for_upload(
            &self.frame_upload,
            &self.pipelines,
            self.path_rasterization_resource_set,
        )
    }

    fn alpha_state_for_window_transparency(transparent: bool) -> NovaSurfaceAlphaState {
        NovaSurfaceAlphaState::for_window_transparency(transparent)
    }

    fn alpha_state_for_window_transparency_on_backend(
        _backend: RendererBackend,
        transparent: bool,
    ) -> NovaSurfaceAlphaState {
        Self::alpha_state_for_window_transparency(transparent)
    }

    fn alpha_state_for_current_backend_transparency(
        &self,
        transparent: bool,
    ) -> NovaSurfaceAlphaState {
        let backend = match self.backend {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(_) => RendererBackend::NovaDx12,
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(_) => RendererBackend::NovaMetal,
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(_) => RendererBackend::NovaVulkan,
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            NovaBackend::Unavailable => {
                return NovaSurfaceAlphaState::for_window_transparency(transparent);
            }
        };
        Self::alpha_state_for_window_transparency_on_backend(backend, transparent)
    }

    fn reconfigure_surface_alpha(&mut self, alpha: NovaSurfaceAlphaState) -> Result<()> {
        self.wait_for_pending_submissions()?;
        if self.surface_alpha.swapchain_mode == alpha.swapchain_mode {
            log::debug!(
                concat!(
                    "nova-gfx surface alpha output changed without swapchain reconfigure: ",
                    "backend={} swapchain=index:{} generation:{} swapchain_alpha={:?} ",
                    "old_output={:?} new_output={:?}"
                ),
                self.backend.label(),
                self.swapchain.index(),
                self.swapchain.generation(),
                alpha.swapchain_mode,
                self.surface_alpha.output_mode,
                alpha.output_mode,
            );
            self.surface_alpha = alpha;
            return Ok(());
        }

        let config = SurfaceConfig {
            size: Extent2d::new(self.current_size.width, self.current_size.height)?,
            format: self.surface_format,
            present_mode: gfx_core::PresentMode::Fifo,
            alpha_mode: alpha.swapchain_mode,
        };
        let path_target_desc = self.path_target_desc(config.size);
        let backdrop_blur_target_desc = self.backdrop_blur_target_desc(config.size);
        let old_path_target = self.current_path_target();
        let old_backdrop_blur_targets = self.current_backdrop_blur_targets();
        let (next_path_target, next_backdrop_blur_targets): (
            NovaPathTarget,
            NovaBackdropBlurTargets,
        ) = match &mut self.backend {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => {
                device.reconfigure_swapchain(self.swapchain, config)?;
                let next_path_target =
                    create_path_target(device, "gpui nova dx12", path_target_desc)?;
                let next_backdrop_blur_targets = create_backdrop_blur_targets(
                    device,
                    "gpui nova dx12",
                    backdrop_blur_target_desc,
                )?;
                destroy_path_target(device, old_path_target, "DX12");
                destroy_backdrop_blur_targets(device, old_backdrop_blur_targets, "DX12");
                (next_path_target, next_backdrop_blur_targets)
            }
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => {
                device.resize_swapchain(
                    self.swapchain,
                    config.size.width(),
                    config.size.height(),
                )?;
                let next_path_target =
                    create_path_target(device, "gpui nova metal", path_target_desc)?;
                let next_backdrop_blur_targets = create_backdrop_blur_targets(
                    device,
                    "gpui nova metal",
                    backdrop_blur_target_desc,
                )?;
                destroy_path_target(device, old_path_target, "Metal");
                destroy_backdrop_blur_targets(device, old_backdrop_blur_targets, "Metal");
                (next_path_target, next_backdrop_blur_targets)
            }
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => {
                device.reconfigure_swapchain(self.swapchain, config)?;
                let next_path_target =
                    create_path_target(device, "gpui nova vulkan", path_target_desc)?;
                let next_backdrop_blur_targets = create_backdrop_blur_targets(
                    device,
                    "gpui nova vulkan",
                    backdrop_blur_target_desc,
                )?;
                destroy_path_target(device, old_path_target, "Vulkan");
                destroy_backdrop_blur_targets(device, old_backdrop_blur_targets, "Vulkan");
                (next_path_target, next_backdrop_blur_targets)
            }
            #[cfg(not(any(
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            NovaBackend::Unavailable => {
                anyhow::bail!("nova-gfx renderer requires an explicit nova-gfx backend feature")
            }
        };
        self.path_texture = next_path_target.texture;
        self.path_texture_view = next_path_target.texture_view;
        self.path_resource_set = next_path_target.resource_set;
        self.backdrop_blur_targets = next_backdrop_blur_targets;
        self.surface_alpha = alpha;
        Ok(())
    }

    fn current_path_target(&self) -> NovaPathTarget {
        NovaPathTarget {
            texture: self.path_texture,
            texture_view: self.path_texture_view,
            resource_set: self.path_resource_set,
        }
    }

    fn current_backdrop_blur_targets(&self) -> NovaBackdropBlurTargets {
        self.backdrop_blur_targets.clone()
    }

    fn path_target_desc(&self, size: Extent2d) -> NovaPathTargetDesc {
        NovaPathTargetDesc {
            size,
            format: self.surface_format,
            resource_set_layout: self.path_resource_set_layout,
            global_buffer: self.global_buffer,
            path_sprite_buffer: self.path_sprite_buffer,
            sampler: self.atlas_sampler,
        }
    }

    fn backdrop_blur_target_desc(&self, size: Extent2d) -> NovaBackdropBlurTargetDesc {
        NovaBackdropBlurTargetDesc {
            size,
            format: self.surface_format,
            downsample: self.frame_upload.backdrop_blur_downsample(),
            pass_resource_set_layout: self.backdrop_blur_pass_resource_set_layout,
            blur_resource_set_layout: self.backdrop_blur_resource_set_layout,
            global_buffer: self.global_buffer,
            pass_buffer: self.backdrop_blur_pass_buffer,
            blur_buffer: self.backdrop_blur_buffer,
            sampler: self.atlas_sampler,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NovaDrawStepMode {
    Present,
    BackdropSource,
}

fn draw_steps_for_upload(
    upload: &NovaFrameUpload,
    pipelines: &NovaPipelines,
    blend_pipelines: NovaBlendPipelines,
    quad_resource_set: ResourceSetId,
    shadow_resource_set: ResourceSetId,
    path_resource_set: ResourceSetId,
    mono_sprite_resource_set: ResourceSetId,
    poly_sprite_resource_set: ResourceSetId,
    underline_resource_set: ResourceSetId,
    backdrop_blur_resource_set: ResourceSetId,
    mode: NovaDrawStepMode,
) -> Vec<DrawStepDesc> {
    let mut steps = upload
        .batches
        .iter()
        .take_while(|batch| {
            mode == NovaDrawStepMode::Present
                || !matches!(batch, NovaUploadedBatch::BackdropBlurs { .. })
        })
        .map(|batch| match *batch {
            NovaUploadedBatch::SolidQuads { first, count } => DrawStepDesc {
                pipeline: blend_pipelines.solid_quads,
                resource_sets: vec![quad_resource_set],
                vertex_count: 4,
                first_vertex: 0,
                instance_count: count,
                first_instance: first,
                scissor: None,
            },
            NovaUploadedBatch::Quads { first, count } => DrawStepDesc {
                pipeline: blend_pipelines.quads,
                resource_sets: vec![quad_resource_set],
                vertex_count: 4,
                first_vertex: 0,
                instance_count: count,
                first_instance: first,
                scissor: None,
            },
            NovaUploadedBatch::Shadows { first, count } => DrawStepDesc {
                pipeline: blend_pipelines.shadows,
                resource_sets: vec![shadow_resource_set],
                vertex_count: 4,
                first_vertex: 0,
                instance_count: count,
                first_instance: first,
                scissor: None,
            },
            NovaUploadedBatch::PathRasterization { .. } => DrawStepDesc {
                pipeline: pipelines.path_rasterization,
                resource_sets: Vec::new(),
                vertex_count: 0,
                first_vertex: 0,
                instance_count: 0,
                first_instance: 0,
                scissor: None,
            },
            NovaUploadedBatch::Paths { first, count } => DrawStepDesc {
                pipeline: pipelines.paths,
                resource_sets: vec![path_resource_set],
                vertex_count: 4,
                first_vertex: 0,
                instance_count: count,
                first_instance: first,
                scissor: None,
            },
            NovaUploadedBatch::MonoSprites { first, count } => DrawStepDesc {
                pipeline: blend_pipelines.mono_sprites,
                resource_sets: vec![mono_sprite_resource_set],
                vertex_count: 4,
                first_vertex: 0,
                instance_count: count,
                first_instance: first,
                scissor: None,
            },
            NovaUploadedBatch::PolySprites { first, count } => DrawStepDesc {
                pipeline: blend_pipelines.poly_sprites,
                resource_sets: vec![poly_sprite_resource_set],
                vertex_count: 4,
                first_vertex: 0,
                instance_count: count,
                first_instance: first,
                scissor: None,
            },
            NovaUploadedBatch::Underlines { first, count } => DrawStepDesc {
                pipeline: blend_pipelines.underlines,
                resource_sets: vec![underline_resource_set],
                vertex_count: 4,
                first_vertex: 0,
                instance_count: count,
                first_instance: first,
                scissor: None,
            },
            NovaUploadedBatch::BackdropBlurs { first, count } => {
                if mode == NovaDrawStepMode::Present {
                    DrawStepDesc {
                        pipeline: blend_pipelines.backdrop_blurs,
                        resource_sets: vec![backdrop_blur_resource_set],
                        vertex_count: 4,
                        first_vertex: 0,
                        instance_count: count,
                        first_instance: first,
                        scissor: None,
                    }
                } else {
                    DrawStepDesc {
                        pipeline: blend_pipelines.backdrop_blurs,
                        resource_sets: Vec::new(),
                        vertex_count: 0,
                        first_vertex: 0,
                        instance_count: 0,
                        first_instance: 0,
                        scissor: None,
                    }
                }
            }
        })
        .collect::<Vec<_>>();
    steps.retain(|step| step.vertex_count > 0 || step.instance_count > 0);
    if steps.is_empty() {
        steps.push(DrawStepDesc {
            pipeline: blend_pipelines.solid_quads,
            resource_sets: vec![quad_resource_set],
            vertex_count: 4,
            first_vertex: 0,
            instance_count: 0,
            first_instance: 0,
            scissor: None,
        });
    }
    steps
}

fn apply_scissor_to_steps(steps: &mut [DrawStepDesc], scissor: ScissorRect) {
    for step in steps {
        step.scissor = Some(scissor);
    }
}

fn partial_scissor_for_plan(
    render_plan: FrameRenderPlan<'_>,
    target_size: DrawableSize,
) -> Option<ScissorRect> {
    if render_plan.partial_present_mode != PartialPresentMode::Partial {
        return None;
    }

    let bounds = render_plan.dirty_region.union_bounds()?;
    let target_width = target_size.width;
    let target_height = target_size.height;
    let x = scaled_pixels_floor_u32(bounds.origin.x).min(target_width);
    let y = scaled_pixels_floor_u32(bounds.origin.y).min(target_height);
    let right = scaled_pixels_ceil_u32(bounds.right()).min(target_width);
    let bottom = scaled_pixels_ceil_u32(bounds.bottom()).min(target_height);
    let scissor = ScissorRect {
        x,
        y,
        width: right.saturating_sub(x),
        height: bottom.saturating_sub(y),
    };
    (!scissor.is_empty()).then_some(scissor)
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped scaled-pixel bounds are converted to integer scissor coordinates"
)]
fn scaled_pixels_floor_u32(value: crate::ScaledPixels) -> u32 {
    let value = f64::from(value).floor();
    if value <= 0.0 {
        0
    } else if value >= f64::from(u32::MAX) {
        u32::MAX
    } else {
        value as u32
    }
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped scaled-pixel bounds are converted to integer scissor coordinates"
)]
fn scaled_pixels_ceil_u32(value: crate::ScaledPixels) -> u32 {
    let value = f64::from(value).ceil();
    if value <= 0.0 {
        0
    } else if value >= f64::from(u32::MAX) {
        u32::MAX
    } else {
        value as u32
    }
}

#[derive(Clone, Debug, PartialEq)]
struct NovaBackdropBlurRenderPass {
    target_texture_view: TextureViewId,
    steps: Vec<DrawStepDesc>,
}

fn backdrop_blur_render_passes_for_targets(
    pipelines: &NovaPipelines,
    targets: &NovaBackdropBlurTargets,
    levels: usize,
) -> Vec<NovaBackdropBlurRenderPass> {
    if targets.levels.is_empty() {
        return Vec::new();
    }
    let levels = levels.clamp(1, targets.levels.len());
    let mut passes = Vec::with_capacity(levels.saturating_mul(2).saturating_sub(1));
    for (level_index, level) in targets.levels.iter().take(levels).enumerate() {
        let resource_set = if level_index == 0 {
            targets.source_pass_resource_set
        } else {
            targets.levels[level_index - 1].pass_resource_set
        };
        passes.push(NovaBackdropBlurRenderPass {
            target_texture_view: level.texture_view,
            steps: vec![DrawStepDesc {
                pipeline: pipelines.backdrop_blur_downsample,
                resource_sets: vec![resource_set],
                vertex_count: 4,
                first_vertex: 0,
                instance_count: 1,
                first_instance: 0,
                scissor: None,
            }],
        });
    }
    for target_index in (0..levels.saturating_sub(1)).rev() {
        passes.push(NovaBackdropBlurRenderPass {
            target_texture_view: targets.levels[target_index].texture_view,
            steps: vec![DrawStepDesc {
                pipeline: pipelines.backdrop_blur_upsample,
                resource_sets: vec![targets.levels[target_index + 1].pass_resource_set],
                vertex_count: 4,
                first_vertex: 0,
                instance_count: 1,
                first_instance: 0,
                scissor: None,
            }],
        });
    }
    passes
}

fn path_mask_draw_steps_for_upload(
    upload: &NovaFrameUpload,
    pipelines: &NovaPipelines,
    path_rasterization_resource_set: ResourceSetId,
) -> Vec<DrawStepDesc> {
    upload
        .batches
        .iter()
        .filter_map(|batch| match *batch {
            NovaUploadedBatch::PathRasterization {
                first_vertex,
                vertex_count,
            } => Some(DrawStepDesc {
                pipeline: pipelines.path_rasterization,
                resource_sets: vec![path_rasterization_resource_set],
                vertex_count,
                first_vertex,
                instance_count: 1,
                first_instance: 0,
                scissor: None,
            }),
            NovaUploadedBatch::SolidQuads { .. }
            | NovaUploadedBatch::Quads { .. }
            | NovaUploadedBatch::Shadows { .. }
            | NovaUploadedBatch::Paths { .. }
            | NovaUploadedBatch::MonoSprites { .. }
            | NovaUploadedBatch::PolySprites { .. }
            | NovaUploadedBatch::Underlines { .. }
            | NovaUploadedBatch::BackdropBlurs { .. } => None,
        })
        .collect()
}

struct NovaGpuiResources {
    render_pass: RenderPassId,
    pipelines: NovaPipelines,
    global_buffer: BufferId,
    text_raster_buffer: BufferId,
    quad_buffer: BufferId,
    shadow_buffer: BufferId,
    path_rasterization_vertex_buffer: BufferId,
    path_sprite_buffer: BufferId,
    mono_sprite_buffer: BufferId,
    poly_sprite_buffer: BufferId,
    underline_buffer: BufferId,
    backdrop_blur_pass_buffer: BufferId,
    backdrop_blur_buffer: BufferId,
    quad_resource_set: ResourceSetId,
    shadow_resource_set: ResourceSetId,
    path_rasterization_resource_set: ResourceSetId,
    path_resource_set_layout: ResourceSetLayoutId,
    path_resource_set: ResourceSetId,
    mono_sprite_resource_set: ResourceSetId,
    poly_sprite_resource_set: ResourceSetId,
    underline_resource_set: ResourceSetId,
    backdrop_blur_pass_resource_set_layout: ResourceSetLayoutId,
    backdrop_blur_resource_set_layout: ResourceSetLayoutId,
    backdrop_blur_targets: NovaBackdropBlurTargets,
    atlas_texture: TextureId,
    atlas_texture_view: TextureViewId,
    atlas_sampler: SamplerId,
    path_texture: TextureId,
    path_texture_view: TextureViewId,
}

#[derive(Clone, Copy)]
struct NovaPathTarget {
    texture: TextureId,
    texture_view: TextureViewId,
    resource_set: ResourceSetId,
}

#[derive(Clone, Copy)]
struct NovaPathTargetDesc {
    size: Extent2d,
    format: Format,
    resource_set_layout: ResourceSetLayoutId,
    global_buffer: BufferId,
    path_sprite_buffer: BufferId,
    sampler: SamplerId,
}

#[derive(Clone)]
struct NovaBackdropBlurTargets {
    downsample: u8,
    source: NovaTextureTarget,
    levels: Vec<NovaBackdropBlurLevelTarget>,
    source_pass_resource_set: ResourceSetId,
    target_resource_set: ResourceSetId,
}

#[derive(Clone, Copy)]
struct NovaBackdropBlurLevelTarget {
    texture: TextureId,
    texture_view: TextureViewId,
    pass_resource_set: ResourceSetId,
}

#[derive(Clone, Copy)]
struct NovaBackdropBlurTargetDesc {
    size: Extent2d,
    format: Format,
    downsample: u8,
    pass_resource_set_layout: ResourceSetLayoutId,
    blur_resource_set_layout: ResourceSetLayoutId,
    global_buffer: BufferId,
    pass_buffer: BufferId,
    blur_buffer: BufferId,
    sampler: SamplerId,
}

fn create_path_target<D>(
    device: &mut D,
    label: &str,
    desc: NovaPathTargetDesc,
) -> Result<NovaPathTarget>
where
    D: GfxResourceDevice + GfxPipelineDevice,
{
    let texture = device.create_texture(&TextureDesc {
        label: Some(format!("{label} path intermediate texture")),
        size: desc.size,
        format: desc.format,
        usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::SAMPLED,
        memory_location: MemoryLocation::GpuOnly,
        dimension: TextureDimension::D2,
    })?;
    let texture_view = device.create_texture_view(&TextureViewDesc {
        label: Some(format!("{label} path intermediate texture view")),
        texture,
        format: desc.format,
    })?;
    let resource_set = device.create_resource_set(&ResourceSetDesc {
        label: Some(format!("{label} path resource set")),
        layout: desc.resource_set_layout,
        bindings: path_resource_bindings(
            desc.global_buffer,
            texture_view,
            desc.sampler,
            desc.path_sprite_buffer,
        ),
    })?;
    Ok(NovaPathTarget {
        texture,
        texture_view,
        resource_set,
    })
}

fn create_backdrop_blur_targets<D>(
    device: &mut D,
    label: &str,
    desc: NovaBackdropBlurTargetDesc,
) -> Result<NovaBackdropBlurTargets>
where
    D: GfxResourceDevice + GfxPipelineDevice,
{
    let source = create_texture_target(
        device,
        &format!("{label} backdrop blur source"),
        desc.size,
        desc.format,
    )?;
    let source_pass_resource_set = device.create_resource_set(&ResourceSetDesc {
        label: Some(format!("{label} backdrop blur source pass resource set")),
        layout: desc.pass_resource_set_layout,
        bindings: backdrop_blur_pass_resource_bindings(
            source.texture_view,
            desc.sampler,
            desc.pass_buffer,
        ),
    })?;
    let downsample = desc.downsample.max(1);
    let mut levels = Vec::with_capacity(usize::from(MAX_BACKDROP_BLUR_LEVELS));
    for level in 0..MAX_BACKDROP_BLUR_LEVELS {
        let factor = u32::from(downsample).saturating_mul(1_u32 << u32::from(level));
        let target_size = Extent2d::new(
            (desc.size.width() / factor).max(1),
            (desc.size.height() / factor).max(1),
        )?;
        let target = create_texture_target(
            device,
            &format!("{label} backdrop blur target level {level}"),
            target_size,
            desc.format,
        )?;
        let pass_resource_set = device.create_resource_set(&ResourceSetDesc {
            label: Some(format!(
                "{label} backdrop blur target level {level} pass resource set"
            )),
            layout: desc.pass_resource_set_layout,
            bindings: backdrop_blur_pass_resource_bindings(
                target.texture_view,
                desc.sampler,
                desc.pass_buffer,
            ),
        })?;
        levels.push(NovaBackdropBlurLevelTarget {
            texture: target.texture,
            texture_view: target.texture_view,
            pass_resource_set,
        });
    }
    let target_resource_set = device.create_resource_set(&ResourceSetDesc {
        label: Some(format!("{label} backdrop blur target resource set")),
        layout: desc.blur_resource_set_layout,
        bindings: backdrop_blur_resource_bindings(
            desc.global_buffer,
            levels
                .first()
                .map_or(source.texture_view, |level| level.texture_view),
            desc.sampler,
            desc.blur_buffer,
        ),
    })?;
    Ok(NovaBackdropBlurTargets {
        downsample,
        source,
        levels,
        source_pass_resource_set,
        target_resource_set,
    })
}

fn destroy_backdrop_blur_targets<D>(
    device: &mut D,
    targets: NovaBackdropBlurTargets,
    backend_name: &str,
) where
    D: GfxResourceDevice + GfxPipelineDevice,
{
    if let Err(error) = device.destroy_resource_set(targets.source_pass_resource_set) {
        log::debug!("failed to destroy {backend_name} backdrop blur source resource set: {error}");
    }
    if let Err(error) = device.destroy_resource_set(targets.target_resource_set) {
        log::debug!("failed to destroy {backend_name} backdrop blur target resource set: {error}");
    }
    for target in targets.levels {
        if let Err(error) = device.destroy_resource_set(target.pass_resource_set) {
            log::debug!(
                "failed to destroy {backend_name} backdrop blur level resource set: {error}"
            );
        }
        destroy_texture_target(
            device,
            NovaTextureTarget {
                texture: target.texture,
                texture_view: target.texture_view,
            },
            backend_name,
        );
    }
    destroy_texture_target(device, targets.source, backend_name);
}

#[derive(Clone, Copy)]
struct NovaTextureTarget {
    texture: TextureId,
    texture_view: TextureViewId,
}

fn create_texture_target<D>(
    device: &mut D,
    label: &str,
    size: Extent2d,
    format: Format,
) -> Result<NovaTextureTarget>
where
    D: GfxResourceDevice + GfxPipelineDevice,
{
    let texture = device.create_texture(&TextureDesc {
        label: Some(format!("{label} texture")),
        size,
        format,
        usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::SAMPLED,
        memory_location: MemoryLocation::GpuOnly,
        dimension: TextureDimension::D2,
    })?;
    let texture_view = device.create_texture_view(&TextureViewDesc {
        label: Some(format!("{label} texture view")),
        texture,
        format,
    })?;
    Ok(NovaTextureTarget {
        texture,
        texture_view,
    })
}

fn destroy_texture_target<D>(device: &mut D, target: NovaTextureTarget, backend_name: &str)
where
    D: GfxResourceDevice + GfxPipelineDevice,
{
    if let Err(error) = device.destroy_texture_view(target.texture_view) {
        log::debug!("failed to destroy {backend_name} texture target view: {error}");
    }
    if let Err(error) = device.destroy_texture(target.texture) {
        log::debug!("failed to destroy {backend_name} texture target: {error}");
    }
}

fn destroy_path_target<D>(device: &mut D, target: NovaPathTarget, backend_name: &str)
where
    D: GfxResourceDevice + GfxPipelineDevice,
{
    if let Err(error) = device.destroy_resource_set(target.resource_set) {
        log::debug!("failed to destroy {backend_name} old path resource set: {error}");
    }
    if let Err(error) = device.destroy_texture_view(target.texture_view) {
        log::debug!("failed to destroy {backend_name} old path texture view: {error}");
    }
    if let Err(error) = device.destroy_texture(target.texture) {
        log::debug!("failed to destroy {backend_name} old path texture: {error}");
    }
}

fn path_resource_bindings(
    global_buffer: BufferId,
    path_texture_view: TextureViewId,
    sampler: SamplerId,
    path_sprite_buffer: BufferId,
) -> Vec<ResourceBinding> {
    vec![
        ResourceBinding {
            binding: 0,
            resource: ResourceBindingResource::Buffer(BufferBinding {
                buffer: global_buffer,
                offset: 0,
                size: GLOBAL_UPLOAD_BYTES as u64,
                stride: None,
            }),
        },
        ResourceBinding {
            binding: 4,
            resource: ResourceBindingResource::Texture(TextureBinding {
                texture_view: path_texture_view,
            }),
        },
        ResourceBinding {
            binding: 5,
            resource: ResourceBindingResource::Sampler(SamplerBinding { sampler }),
        },
        ResourceBinding {
            binding: 6,
            resource: ResourceBindingResource::Buffer(BufferBinding {
                buffer: path_sprite_buffer,
                offset: 0,
                size: (MAX_PATH_SPRITES * PACKED_PATH_SPRITE_BYTES) as u64,
                stride: Some(PACKED_PATH_SPRITE_BYTES as u32),
            }),
        },
    ]
}

fn backdrop_blur_pass_resource_bindings(
    source_texture_view: TextureViewId,
    sampler: SamplerId,
    pass_buffer: BufferId,
) -> Vec<ResourceBinding> {
    vec![
        ResourceBinding {
            binding: 4,
            resource: ResourceBindingResource::Texture(TextureBinding {
                texture_view: source_texture_view,
            }),
        },
        ResourceBinding {
            binding: 5,
            resource: ResourceBindingResource::Sampler(SamplerBinding { sampler }),
        },
        ResourceBinding {
            binding: 15,
            resource: ResourceBindingResource::Buffer(BufferBinding {
                buffer: pass_buffer,
                offset: 0,
                size: BACKDROP_BLUR_PASS_BYTES as u64,
                stride: Some(BACKDROP_BLUR_PASS_BYTES as u32),
            }),
        },
    ]
}

fn backdrop_blur_resource_bindings(
    global_buffer: BufferId,
    source_texture_view: TextureViewId,
    sampler: SamplerId,
    blur_buffer: BufferId,
) -> Vec<ResourceBinding> {
    vec![
        ResourceBinding {
            binding: 0,
            resource: ResourceBindingResource::Buffer(BufferBinding {
                buffer: global_buffer,
                offset: 0,
                size: GLOBAL_UPLOAD_BYTES as u64,
                stride: None,
            }),
        },
        ResourceBinding {
            binding: 4,
            resource: ResourceBindingResource::Texture(TextureBinding {
                texture_view: source_texture_view,
            }),
        },
        ResourceBinding {
            binding: 5,
            resource: ResourceBindingResource::Sampler(SamplerBinding { sampler }),
        },
        ResourceBinding {
            binding: 16,
            resource: ResourceBindingResource::Buffer(BufferBinding {
                buffer: blur_buffer,
                offset: 0,
                size: (MAX_BACKDROP_BLURS * PACKED_BACKDROP_BLUR_BYTES) as u64,
                stride: Some(PACKED_BACKDROP_BLUR_BYTES as u32),
            }),
        },
    ]
}

fn create_blend_pipelines<D>(
    device: &mut D,
    desc: NovaBlendPipelineDesc<'_>,
) -> Result<NovaBlendPipelines>
where
    D: GfxResourceDevice + GfxPipelineDevice,
{
    let solid_quads = device
        .create_render_pipeline(
            &RenderPipelineDesc {
                label: Some(format!(
                    "{} {} solid quad pipeline",
                    desc.label, desc.suffix
                )),
                vertex_shader: desc.solid_vertex,
                vertex_entry_point: "vs_solid_quad".to_string(),
                fragment_shader: desc.solid_fragment,
                fragment_entry_point: "fs_solid_quad".to_string(),
                vertex_buffers: Vec::new(),
                render_pass: desc.render_pass,
                pipeline_layout: Some(desc.quad_pipeline_layout),
                color_format: desc.color_format,
                blend_mode: desc.blend_mode,
                primitive_topology: PrimitiveTopology::TriangleStrip,
            },
            desc.size,
        )
        .with_context(|| format!("creating nova {} solid quad render pipeline", desc.suffix))?;
    let quads = device
        .create_render_pipeline(
            &RenderPipelineDesc {
                label: Some(format!("{} {} quad pipeline", desc.label, desc.suffix)),
                vertex_shader: desc.quad_vertex,
                vertex_entry_point: "vs_quad".to_string(),
                fragment_shader: desc.quad_fragment,
                fragment_entry_point: "fs_quad".to_string(),
                vertex_buffers: Vec::new(),
                render_pass: desc.render_pass,
                pipeline_layout: Some(desc.quad_pipeline_layout),
                color_format: desc.color_format,
                blend_mode: desc.blend_mode,
                primitive_topology: PrimitiveTopology::TriangleStrip,
            },
            desc.size,
        )
        .with_context(|| format!("creating nova {} quad render pipeline", desc.suffix))?;
    let shadows = device
        .create_render_pipeline(
            &RenderPipelineDesc {
                label: Some(format!("{} {} shadow pipeline", desc.label, desc.suffix)),
                vertex_shader: desc.shadow_vertex,
                vertex_entry_point: "vs_shadow".to_string(),
                fragment_shader: desc.shadow_fragment,
                fragment_entry_point: "fs_shadow".to_string(),
                vertex_buffers: Vec::new(),
                render_pass: desc.render_pass,
                pipeline_layout: Some(desc.shadow_pipeline_layout),
                color_format: desc.color_format,
                blend_mode: desc.blend_mode,
                primitive_topology: PrimitiveTopology::TriangleStrip,
            },
            desc.size,
        )
        .with_context(|| format!("creating nova {} shadow render pipeline", desc.suffix))?;
    let mono_sprites = device
        .create_render_pipeline(
            &RenderPipelineDesc {
                label: Some(format!(
                    "{} {} mono sprite pipeline",
                    desc.label, desc.suffix
                )),
                vertex_shader: desc.mono_vertex,
                vertex_entry_point: "vs_mono_sprite".to_string(),
                fragment_shader: desc.mono_fragment,
                fragment_entry_point: "fs_mono_sprite".to_string(),
                vertex_buffers: Vec::new(),
                render_pass: desc.render_pass,
                pipeline_layout: Some(desc.mono_pipeline_layout),
                color_format: desc.color_format,
                blend_mode: desc.blend_mode,
                primitive_topology: PrimitiveTopology::TriangleStrip,
            },
            desc.size,
        )
        .with_context(|| format!("creating nova {} mono sprite render pipeline", desc.suffix))?;
    let poly_sprites = device
        .create_render_pipeline(
            &RenderPipelineDesc {
                label: Some(format!(
                    "{} {} poly sprite pipeline",
                    desc.label, desc.suffix
                )),
                vertex_shader: desc.poly_vertex,
                vertex_entry_point: "vs_poly_sprite".to_string(),
                fragment_shader: desc.poly_fragment,
                fragment_entry_point: "fs_poly_sprite".to_string(),
                vertex_buffers: Vec::new(),
                render_pass: desc.render_pass,
                pipeline_layout: Some(desc.poly_pipeline_layout),
                color_format: desc.color_format,
                blend_mode: desc.blend_mode,
                primitive_topology: PrimitiveTopology::TriangleStrip,
            },
            desc.size,
        )
        .with_context(|| format!("creating nova {} poly sprite render pipeline", desc.suffix))?;
    let underlines = device
        .create_render_pipeline(
            &RenderPipelineDesc {
                label: Some(format!("{} {} underline pipeline", desc.label, desc.suffix)),
                vertex_shader: desc.underline_vertex,
                vertex_entry_point: "vs_underline".to_string(),
                fragment_shader: desc.underline_fragment,
                fragment_entry_point: "fs_underline".to_string(),
                vertex_buffers: Vec::new(),
                render_pass: desc.render_pass,
                pipeline_layout: Some(desc.underline_pipeline_layout),
                color_format: desc.color_format,
                blend_mode: desc.blend_mode,
                primitive_topology: PrimitiveTopology::TriangleStrip,
            },
            desc.size,
        )
        .with_context(|| format!("creating nova {} underline render pipeline", desc.suffix))?;
    let backdrop_blurs = device
        .create_render_pipeline(
            &RenderPipelineDesc {
                label: Some(format!(
                    "{} {} backdrop blur pipeline",
                    desc.label, desc.suffix
                )),
                vertex_shader: desc.backdrop_blur_vertex,
                vertex_entry_point: "vs_backdrop_blur".to_string(),
                fragment_shader: desc.backdrop_blur_fragment,
                fragment_entry_point: "fs_backdrop_blur".to_string(),
                vertex_buffers: Vec::new(),
                render_pass: desc.render_pass,
                pipeline_layout: Some(desc.backdrop_blur_pipeline_layout),
                color_format: desc.color_format,
                blend_mode: desc.blend_mode,
                primitive_topology: PrimitiveTopology::TriangleStrip,
            },
            desc.size,
        )
        .with_context(|| {
            format!(
                "creating nova {} backdrop blur render pipeline",
                desc.suffix
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

fn create_gpui_resources<D>(
    device: &mut D,
    surface_config: SurfaceConfig,
    label: &str,
    shader_binaries: NovaShaderBinaries,
) -> Result<NovaGpuiResources>
where
    D: GfxResourceDevice + GfxPipelineDevice,
{
    let quad_resource_set_layout = device.create_resource_set_layout(&ResourceSetLayoutDesc {
        label: Some(format!("{label} quad layout")),
        entries: vec![
            ResourceSetLayoutEntry {
                binding: 0,
                binding_type: ResourceBindingType::UniformBuffer,
                stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
            },
            ResourceSetLayoutEntry {
                binding: 1,
                binding_type: ResourceBindingType::StorageBuffer,
                stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
            },
        ],
    })?;
    let shadow_resource_set_layout = device.create_resource_set_layout(&ResourceSetLayoutDesc {
        label: Some(format!("{label} shadow layout")),
        entries: vec![
            ResourceSetLayoutEntry {
                binding: 0,
                binding_type: ResourceBindingType::UniformBuffer,
                stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
            },
            ResourceSetLayoutEntry {
                binding: 2,
                binding_type: ResourceBindingType::StorageBuffer,
                stages: ShaderStages::VERTEX,
            },
        ],
    })?;
    let path_rasterization_resource_set_layout =
        device.create_resource_set_layout(&ResourceSetLayoutDesc {
            label: Some(format!("{label} path rasterization layout")),
            entries: vec![
                ResourceSetLayoutEntry {
                    binding: 0,
                    binding_type: ResourceBindingType::UniformBuffer,
                    stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 3,
                    binding_type: ResourceBindingType::StorageBuffer,
                    stages: ShaderStages::VERTEX,
                },
            ],
        })?;
    let path_resource_set_layout = device.create_resource_set_layout(&ResourceSetLayoutDesc {
        label: Some(format!("{label} path layout")),
        entries: vec![
            ResourceSetLayoutEntry {
                binding: 0,
                binding_type: ResourceBindingType::UniformBuffer,
                stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
            },
            ResourceSetLayoutEntry {
                binding: 4,
                binding_type: ResourceBindingType::SampledTexture,
                stages: ShaderStages::FRAGMENT,
            },
            ResourceSetLayoutEntry {
                binding: 5,
                binding_type: ResourceBindingType::Sampler,
                stages: ShaderStages::FRAGMENT,
            },
            ResourceSetLayoutEntry {
                binding: 6,
                binding_type: ResourceBindingType::StorageBuffer,
                stages: ShaderStages::VERTEX,
            },
        ],
    })?;
    let mono_resource_set_layout = device.create_resource_set_layout(&ResourceSetLayoutDesc {
        label: Some(format!("{label} mono sprite layout")),
        entries: vec![
            ResourceSetLayoutEntry {
                binding: 0,
                binding_type: ResourceBindingType::UniformBuffer,
                stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
            },
            ResourceSetLayoutEntry {
                binding: 1,
                binding_type: ResourceBindingType::UniformBuffer,
                stages: ShaderStages::FRAGMENT,
            },
            ResourceSetLayoutEntry {
                binding: 4,
                binding_type: ResourceBindingType::SampledTexture,
                stages: ShaderStages::FRAGMENT,
            },
            ResourceSetLayoutEntry {
                binding: 5,
                binding_type: ResourceBindingType::Sampler,
                stages: ShaderStages::FRAGMENT,
            },
            ResourceSetLayoutEntry {
                binding: 8,
                binding_type: ResourceBindingType::StorageBuffer,
                stages: ShaderStages::VERTEX,
            },
        ],
    })?;
    let poly_resource_set_layout = device.create_resource_set_layout(&ResourceSetLayoutDesc {
        label: Some(format!("{label} poly sprite layout")),
        entries: vec![
            ResourceSetLayoutEntry {
                binding: 0,
                binding_type: ResourceBindingType::UniformBuffer,
                stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
            },
            ResourceSetLayoutEntry {
                binding: 4,
                binding_type: ResourceBindingType::SampledTexture,
                stages: ShaderStages::FRAGMENT,
            },
            ResourceSetLayoutEntry {
                binding: 5,
                binding_type: ResourceBindingType::Sampler,
                stages: ShaderStages::FRAGMENT,
            },
            ResourceSetLayoutEntry {
                binding: 9,
                binding_type: ResourceBindingType::StorageBuffer,
                stages: ShaderStages::VERTEX,
            },
        ],
    })?;
    let underline_resource_set_layout =
        device.create_resource_set_layout(&ResourceSetLayoutDesc {
            label: Some(format!("{label} underline layout")),
            entries: vec![
                ResourceSetLayoutEntry {
                    binding: 0,
                    binding_type: ResourceBindingType::UniformBuffer,
                    stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 7,
                    binding_type: ResourceBindingType::StorageBuffer,
                    stages: ShaderStages::VERTEX,
                },
            ],
        })?;
    let backdrop_blur_pass_resource_set_layout =
        device.create_resource_set_layout(&ResourceSetLayoutDesc {
            label: Some(format!("{label} backdrop blur pass layout")),
            entries: vec![
                ResourceSetLayoutEntry {
                    binding: 4,
                    binding_type: ResourceBindingType::SampledTexture,
                    stages: ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 5,
                    binding_type: ResourceBindingType::Sampler,
                    stages: ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 15,
                    binding_type: ResourceBindingType::StorageBuffer,
                    stages: ShaderStages::VERTEX,
                },
            ],
        })?;
    let backdrop_blur_resource_set_layout =
        device.create_resource_set_layout(&ResourceSetLayoutDesc {
            label: Some(format!("{label} backdrop blur layout")),
            entries: vec![
                ResourceSetLayoutEntry {
                    binding: 0,
                    binding_type: ResourceBindingType::UniformBuffer,
                    stages: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 4,
                    binding_type: ResourceBindingType::SampledTexture,
                    stages: ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 5,
                    binding_type: ResourceBindingType::Sampler,
                    stages: ShaderStages::FRAGMENT,
                },
                ResourceSetLayoutEntry {
                    binding: 16,
                    binding_type: ResourceBindingType::StorageBuffer,
                    stages: ShaderStages::VERTEX,
                },
            ],
        })?;
    let quad_pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDesc {
        label: Some(format!("{label} quad pipeline layout")),
        resource_set_layouts: vec![quad_resource_set_layout],
    })?;
    let shadow_pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDesc {
        label: Some(format!("{label} shadow pipeline layout")),
        resource_set_layouts: vec![shadow_resource_set_layout],
    })?;
    let path_rasterization_pipeline_layout =
        device.create_pipeline_layout(&PipelineLayoutDesc {
            label: Some(format!("{label} path rasterization pipeline layout")),
            resource_set_layouts: vec![path_rasterization_resource_set_layout],
        })?;
    let path_pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDesc {
        label: Some(format!("{label} path pipeline layout")),
        resource_set_layouts: vec![path_resource_set_layout],
    })?;
    let mono_pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDesc {
        label: Some(format!("{label} mono sprite pipeline layout")),
        resource_set_layouts: vec![mono_resource_set_layout],
    })?;
    let poly_pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDesc {
        label: Some(format!("{label} poly sprite pipeline layout")),
        resource_set_layouts: vec![poly_resource_set_layout],
    })?;
    let underline_pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDesc {
        label: Some(format!("{label} underline pipeline layout")),
        resource_set_layouts: vec![underline_resource_set_layout],
    })?;
    let backdrop_blur_pass_pipeline_layout =
        device.create_pipeline_layout(&PipelineLayoutDesc {
            label: Some(format!("{label} backdrop blur pass pipeline layout")),
            resource_set_layouts: vec![backdrop_blur_pass_resource_set_layout],
        })?;
    let backdrop_blur_pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDesc {
        label: Some(format!("{label} backdrop blur pipeline layout")),
        resource_set_layouts: vec![backdrop_blur_resource_set_layout],
    })?;
    let global_buffer = device.create_buffer(&BufferDesc {
        label: Some(format!("{label} globals")),
        size: GLOBAL_UPLOAD_BYTES as u64,
        usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let text_raster_buffer = device.create_buffer(&BufferDesc {
        label: Some(format!("{label} text raster params")),
        size: TEXT_RASTER_UPLOAD_BYTES as u64,
        usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let quad_buffer = device.create_buffer(&BufferDesc {
        label: Some(format!("{label} quads")),
        size: (MAX_QUADS * PACKED_QUAD_BYTES) as u64,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let shadow_buffer = device.create_buffer(&BufferDesc {
        label: Some(format!("{label} shadows")),
        size: (MAX_SHADOWS * PACKED_SHADOW_BYTES) as u64,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let path_rasterization_vertex_buffer = device.create_buffer(&BufferDesc {
        label: Some(format!("{label} path rasterization vertices")),
        size: (MAX_PATH_VERTICES * PACKED_PATH_RASTERIZATION_VERTEX_BYTES) as u64,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let path_sprite_buffer = device.create_buffer(&BufferDesc {
        label: Some(format!("{label} path sprites")),
        size: (MAX_PATH_SPRITES * PACKED_PATH_SPRITE_BYTES) as u64,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let mono_sprite_buffer = device.create_buffer(&BufferDesc {
        label: Some(format!("{label} mono sprites")),
        size: (MAX_MONO_SPRITES * PACKED_MONO_SPRITE_BYTES) as u64,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let poly_sprite_buffer = device.create_buffer(&BufferDesc {
        label: Some(format!("{label} poly sprites")),
        size: (MAX_POLY_SPRITES * PACKED_POLY_SPRITE_BYTES) as u64,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let underline_buffer = device.create_buffer(&BufferDesc {
        label: Some(format!("{label} underlines")),
        size: (MAX_UNDERLINES * PACKED_UNDERLINE_BYTES) as u64,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let backdrop_blur_pass_buffer = device.create_buffer(&BufferDesc {
        label: Some(format!("{label} backdrop blur pass")),
        size: BACKDROP_BLUR_PASS_BYTES as u64,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let backdrop_blur_buffer = device.create_buffer(&BufferDesc {
        label: Some(format!("{label} backdrop blurs")),
        size: (MAX_BACKDROP_BLURS * PACKED_BACKDROP_BLUR_BYTES) as u64,
        usage: BufferUsage::STORAGE | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::CpuToGpu,
    })?;
    let atlas_texture = device.create_texture(&TextureDesc {
        label: Some(format!("{label} glyph atlas texture")),
        size: Extent2d::new(NOVA_ATLAS_SIZE, NOVA_ATLAS_SIZE)?,
        format: Format::Rgba8Unorm,
        usage: TextureUsage::COPY_DST | TextureUsage::SAMPLED,
        memory_location: MemoryLocation::GpuOnly,
        dimension: TextureDimension::D2,
    })?;
    let atlas_texture_view = device.create_texture_view(&TextureViewDesc {
        label: Some(format!("{label} glyph atlas texture view")),
        texture: atlas_texture,
        format: Format::Rgba8Unorm,
    })?;
    let atlas_sampler = device.create_sampler(&SamplerDesc {
        label: Some(format!("{label} glyph atlas sampler")),
        mag_filter: FilterMode::Linear,
        min_filter: FilterMode::Linear,
        address_mode_u: AddressMode::ClampToEdge,
        address_mode_v: AddressMode::ClampToEdge,
    })?;
    let path_target = create_path_target(
        device,
        label,
        NovaPathTargetDesc {
            size: surface_config.size,
            format: surface_config.format,
            resource_set_layout: path_resource_set_layout,
            global_buffer,
            path_sprite_buffer,
            sampler: atlas_sampler,
        },
    )?;
    let backdrop_blur_targets = create_backdrop_blur_targets(
        device,
        label,
        NovaBackdropBlurTargetDesc {
            size: surface_config.size,
            format: surface_config.format,
            downsample: DEFAULT_BACKDROP_BLUR_DOWNSAMPLE,
            pass_resource_set_layout: backdrop_blur_pass_resource_set_layout,
            blur_resource_set_layout: backdrop_blur_resource_set_layout,
            global_buffer,
            pass_buffer: backdrop_blur_pass_buffer,
            blur_buffer: backdrop_blur_buffer,
            sampler: atlas_sampler,
        },
    )?;
    let quad_resource_set = device.create_resource_set(&ResourceSetDesc {
        label: Some(format!("{label} quad resource set")),
        layout: quad_resource_set_layout,
        bindings: vec![
            ResourceBinding {
                binding: 0,
                resource: ResourceBindingResource::Buffer(BufferBinding {
                    buffer: global_buffer,
                    offset: 0,
                    size: GLOBAL_UPLOAD_BYTES as u64,
                    stride: None,
                }),
            },
            ResourceBinding {
                binding: 1,
                resource: ResourceBindingResource::Buffer(BufferBinding {
                    buffer: quad_buffer,
                    offset: 0,
                    size: (MAX_QUADS * PACKED_QUAD_BYTES) as u64,
                    stride: Some(PACKED_QUAD_BYTES as u32),
                }),
            },
        ],
    })?;
    let shadow_resource_set = device.create_resource_set(&ResourceSetDesc {
        label: Some(format!("{label} shadow resource set")),
        layout: shadow_resource_set_layout,
        bindings: vec![
            ResourceBinding {
                binding: 0,
                resource: ResourceBindingResource::Buffer(BufferBinding {
                    buffer: global_buffer,
                    offset: 0,
                    size: GLOBAL_UPLOAD_BYTES as u64,
                    stride: None,
                }),
            },
            ResourceBinding {
                binding: 2,
                resource: ResourceBindingResource::Buffer(BufferBinding {
                    buffer: shadow_buffer,
                    offset: 0,
                    size: (MAX_SHADOWS * PACKED_SHADOW_BYTES) as u64,
                    stride: Some(PACKED_SHADOW_BYTES as u32),
                }),
            },
        ],
    })?;
    let path_rasterization_resource_set = device.create_resource_set(&ResourceSetDesc {
        label: Some(format!("{label} path rasterization resource set")),
        layout: path_rasterization_resource_set_layout,
        bindings: vec![
            ResourceBinding {
                binding: 0,
                resource: ResourceBindingResource::Buffer(BufferBinding {
                    buffer: global_buffer,
                    offset: 0,
                    size: GLOBAL_UPLOAD_BYTES as u64,
                    stride: None,
                }),
            },
            ResourceBinding {
                binding: 3,
                resource: ResourceBindingResource::Buffer(BufferBinding {
                    buffer: path_rasterization_vertex_buffer,
                    offset: 0,
                    size: (MAX_PATH_VERTICES * PACKED_PATH_RASTERIZATION_VERTEX_BYTES) as u64,
                    stride: Some(PACKED_PATH_RASTERIZATION_VERTEX_BYTES as u32),
                }),
            },
        ],
    })?;
    let mono_sprite_resource_set = device.create_resource_set(&ResourceSetDesc {
        label: Some(format!("{label} mono sprite resource set")),
        layout: mono_resource_set_layout,
        bindings: vec![
            ResourceBinding {
                binding: 0,
                resource: ResourceBindingResource::Buffer(BufferBinding {
                    buffer: global_buffer,
                    offset: 0,
                    size: GLOBAL_UPLOAD_BYTES as u64,
                    stride: None,
                }),
            },
            ResourceBinding {
                binding: 1,
                resource: ResourceBindingResource::Buffer(BufferBinding {
                    buffer: text_raster_buffer,
                    offset: 0,
                    size: TEXT_RASTER_UPLOAD_BYTES as u64,
                    stride: None,
                }),
            },
            ResourceBinding {
                binding: 4,
                resource: ResourceBindingResource::Texture(TextureBinding {
                    texture_view: atlas_texture_view,
                }),
            },
            ResourceBinding {
                binding: 5,
                resource: ResourceBindingResource::Sampler(SamplerBinding {
                    sampler: atlas_sampler,
                }),
            },
            ResourceBinding {
                binding: 8,
                resource: ResourceBindingResource::Buffer(BufferBinding {
                    buffer: mono_sprite_buffer,
                    offset: 0,
                    size: (MAX_MONO_SPRITES * PACKED_MONO_SPRITE_BYTES) as u64,
                    stride: Some(PACKED_MONO_SPRITE_BYTES as u32),
                }),
            },
        ],
    })?;
    let underline_resource_set = device.create_resource_set(&ResourceSetDesc {
        label: Some(format!("{label} underline resource set")),
        layout: underline_resource_set_layout,
        bindings: vec![
            ResourceBinding {
                binding: 0,
                resource: ResourceBindingResource::Buffer(BufferBinding {
                    buffer: global_buffer,
                    offset: 0,
                    size: GLOBAL_UPLOAD_BYTES as u64,
                    stride: None,
                }),
            },
            ResourceBinding {
                binding: 7,
                resource: ResourceBindingResource::Buffer(BufferBinding {
                    buffer: underline_buffer,
                    offset: 0,
                    size: (MAX_UNDERLINES * PACKED_UNDERLINE_BYTES) as u64,
                    stride: Some(PACKED_UNDERLINE_BYTES as u32),
                }),
            },
        ],
    })?;
    let poly_sprite_resource_set = device.create_resource_set(&ResourceSetDesc {
        label: Some(format!("{label} poly sprite resource set")),
        layout: poly_resource_set_layout,
        bindings: vec![
            ResourceBinding {
                binding: 0,
                resource: ResourceBindingResource::Buffer(BufferBinding {
                    buffer: global_buffer,
                    offset: 0,
                    size: GLOBAL_UPLOAD_BYTES as u64,
                    stride: None,
                }),
            },
            ResourceBinding {
                binding: 4,
                resource: ResourceBindingResource::Texture(TextureBinding {
                    texture_view: atlas_texture_view,
                }),
            },
            ResourceBinding {
                binding: 5,
                resource: ResourceBindingResource::Sampler(SamplerBinding {
                    sampler: atlas_sampler,
                }),
            },
            ResourceBinding {
                binding: 9,
                resource: ResourceBindingResource::Buffer(BufferBinding {
                    buffer: poly_sprite_buffer,
                    offset: 0,
                    size: (MAX_POLY_SPRITES * PACKED_POLY_SPRITE_BYTES) as u64,
                    stride: Some(PACKED_POLY_SPRITE_BYTES as u32),
                }),
            },
        ],
    })?;
    let render_pass = device.create_render_pass(&RenderPassDesc {
        label: Some(format!("{label} render pass")),
        color_attachment: ColorAttachmentDesc {
            format: surface_config.format,
        },
    })?;
    let solid_vertex = device
        .create_shader_module(&ShaderModuleDesc {
            label: Some(format!("{label} solid quad vertex shader")),
            binary: shader_binaries.solid_vertex,
        })
        .context("creating nova solid quad vertex shader module")?;
    let solid_fragment = device
        .create_shader_module(&ShaderModuleDesc {
            label: Some(format!("{label} solid quad fragment shader")),
            binary: shader_binaries.solid_fragment,
        })
        .context("creating nova solid quad fragment shader module")?;
    let quad_vertex = device
        .create_shader_module(&ShaderModuleDesc {
            label: Some(format!("{label} quad vertex shader")),
            binary: shader_binaries.quad_vertex,
        })
        .context("creating nova quad vertex shader module")?;
    let quad_fragment = device
        .create_shader_module(&ShaderModuleDesc {
            label: Some(format!("{label} quad fragment shader")),
            binary: shader_binaries.quad_fragment,
        })
        .context("creating nova quad fragment shader module")?;
    let shadow_vertex = device
        .create_shader_module(&ShaderModuleDesc {
            label: Some(format!("{label} shadow vertex shader")),
            binary: shader_binaries.shadow_vertex,
        })
        .context("creating nova shadow vertex shader module")?;
    let shadow_fragment = device
        .create_shader_module(&ShaderModuleDesc {
            label: Some(format!("{label} shadow fragment shader")),
            binary: shader_binaries.shadow_fragment,
        })
        .context("creating nova shadow fragment shader module")?;
    let path_rasterization_vertex = device
        .create_shader_module(&ShaderModuleDesc {
            label: Some(format!("{label} path rasterization vertex shader")),
            binary: shader_binaries.path_rasterization_vertex,
        })
        .context("creating nova path rasterization vertex shader module")?;
    let path_rasterization_fragment = device
        .create_shader_module(&ShaderModuleDesc {
            label: Some(format!("{label} path rasterization fragment shader")),
            binary: shader_binaries.path_rasterization_fragment,
        })
        .context("creating nova path rasterization fragment shader module")?;
    let path_vertex = device
        .create_shader_module(&ShaderModuleDesc {
            label: Some(format!("{label} path vertex shader")),
            binary: shader_binaries.path_vertex,
        })
        .context("creating nova path vertex shader module")?;
    let path_fragment = device
        .create_shader_module(&ShaderModuleDesc {
            label: Some(format!("{label} path fragment shader")),
            binary: shader_binaries.path_fragment,
        })
        .context("creating nova path fragment shader module")?;
    let mono_vertex = device
        .create_shader_module(&ShaderModuleDesc {
            label: Some(format!("{label} mono sprite vertex shader")),
            binary: shader_binaries.mono_vertex,
        })
        .context("creating nova mono sprite vertex shader module")?;
    let mono_fragment = device
        .create_shader_module(&ShaderModuleDesc {
            label: Some(format!("{label} mono sprite fragment shader")),
            binary: shader_binaries.mono_fragment,
        })
        .context("creating nova mono sprite fragment shader module")?;
    let poly_vertex = device
        .create_shader_module(&ShaderModuleDesc {
            label: Some(format!("{label} poly sprite vertex shader")),
            binary: shader_binaries.poly_vertex,
        })
        .context("creating nova poly sprite vertex shader module")?;
    let poly_fragment = device
        .create_shader_module(&ShaderModuleDesc {
            label: Some(format!("{label} poly sprite fragment shader")),
            binary: shader_binaries.poly_fragment,
        })
        .context("creating nova poly sprite fragment shader module")?;
    let underline_vertex = device
        .create_shader_module(&ShaderModuleDesc {
            label: Some(format!("{label} underline vertex shader")),
            binary: shader_binaries.underline_vertex,
        })
        .context("creating nova underline vertex shader module")?;
    let underline_fragment = device
        .create_shader_module(&ShaderModuleDesc {
            label: Some(format!("{label} underline fragment shader")),
            binary: shader_binaries.underline_fragment,
        })
        .context("creating nova underline fragment shader module")?;
    let backdrop_blur_pass_vertex = device
        .create_shader_module(&ShaderModuleDesc {
            label: Some(format!("{label} backdrop blur pass vertex shader")),
            binary: shader_binaries.backdrop_blur_pass_vertex,
        })
        .context("creating nova backdrop blur pass vertex shader module")?;
    let backdrop_blur_downsample_fragment = device
        .create_shader_module(&ShaderModuleDesc {
            label: Some(format!("{label} backdrop blur downsample fragment shader")),
            binary: shader_binaries.backdrop_blur_downsample_fragment,
        })
        .context("creating nova backdrop blur downsample fragment shader module")?;
    let backdrop_blur_upsample_fragment = device
        .create_shader_module(&ShaderModuleDesc {
            label: Some(format!("{label} backdrop blur upsample fragment shader")),
            binary: shader_binaries.backdrop_blur_upsample_fragment,
        })
        .context("creating nova backdrop blur upsample fragment shader module")?;
    let backdrop_blur_vertex = device
        .create_shader_module(&ShaderModuleDesc {
            label: Some(format!("{label} backdrop blur vertex shader")),
            binary: shader_binaries.backdrop_blur_vertex,
        })
        .context("creating nova backdrop blur vertex shader module")?;
    let backdrop_blur_fragment = device
        .create_shader_module(&ShaderModuleDesc {
            label: Some(format!("{label} backdrop blur fragment shader")),
            binary: shader_binaries.backdrop_blur_fragment,
        })
        .context("creating nova backdrop blur fragment shader module")?;
    let alpha = create_blend_pipelines(
        device,
        NovaBlendPipelineDesc {
            label,
            suffix: "alpha",
            blend_mode: BlendMode::Alpha,
            size: surface_config.size,
            color_format: surface_config.format,
            render_pass,
            quad_pipeline_layout,
            shadow_pipeline_layout,
            mono_pipeline_layout,
            poly_pipeline_layout,
            underline_pipeline_layout,
            backdrop_blur_pipeline_layout,
            solid_vertex,
            solid_fragment,
            quad_vertex,
            quad_fragment,
            shadow_vertex,
            shadow_fragment,
            mono_vertex,
            mono_fragment,
            poly_vertex,
            poly_fragment,
            underline_vertex,
            underline_fragment,
            backdrop_blur_vertex,
            backdrop_blur_fragment,
        },
    )?;
    let premultiplied = create_blend_pipelines(
        device,
        NovaBlendPipelineDesc {
            label,
            suffix: "premultiplied",
            blend_mode: BlendMode::PremultipliedAlpha,
            size: surface_config.size,
            color_format: surface_config.format,
            render_pass,
            quad_pipeline_layout,
            shadow_pipeline_layout,
            mono_pipeline_layout,
            poly_pipeline_layout,
            underline_pipeline_layout,
            backdrop_blur_pipeline_layout,
            solid_vertex,
            solid_fragment,
            quad_vertex,
            quad_fragment,
            shadow_vertex,
            shadow_fragment,
            mono_vertex,
            mono_fragment,
            poly_vertex,
            poly_fragment,
            underline_vertex,
            underline_fragment,
            backdrop_blur_vertex,
            backdrop_blur_fragment,
        },
    )?;
    let backdrop_blur_downsample = device
        .create_render_pipeline(
            &RenderPipelineDesc {
                label: Some(format!("{label} backdrop blur downsample pipeline")),
                vertex_shader: backdrop_blur_pass_vertex,
                vertex_entry_point: "vs_backdrop_blur_pass".to_string(),
                fragment_shader: backdrop_blur_downsample_fragment,
                fragment_entry_point: "fs_backdrop_blur_downsample".to_string(),
                vertex_buffers: Vec::new(),
                render_pass,
                pipeline_layout: Some(backdrop_blur_pass_pipeline_layout),
                color_format: surface_config.format,
                blend_mode: BlendMode::Replace,
                primitive_topology: PrimitiveTopology::TriangleStrip,
            },
            surface_config.size,
        )
        .context("creating nova backdrop blur downsample render pipeline")?;
    let backdrop_blur_upsample = device
        .create_render_pipeline(
            &RenderPipelineDesc {
                label: Some(format!("{label} backdrop blur upsample pipeline")),
                vertex_shader: backdrop_blur_pass_vertex,
                vertex_entry_point: "vs_backdrop_blur_pass".to_string(),
                fragment_shader: backdrop_blur_upsample_fragment,
                fragment_entry_point: "fs_backdrop_blur_upsample".to_string(),
                vertex_buffers: Vec::new(),
                render_pass,
                pipeline_layout: Some(backdrop_blur_pass_pipeline_layout),
                color_format: surface_config.format,
                blend_mode: BlendMode::Replace,
                primitive_topology: PrimitiveTopology::TriangleStrip,
            },
            surface_config.size,
        )
        .context("creating nova backdrop blur upsample render pipeline")?;
    let path_rasterization = device
        .create_render_pipeline(
            &RenderPipelineDesc {
                label: Some(format!("{label} path rasterization pipeline")),
                vertex_shader: path_rasterization_vertex,
                vertex_entry_point: "vs_path_rasterization".to_string(),
                fragment_shader: path_rasterization_fragment,
                fragment_entry_point: "fs_path_rasterization".to_string(),
                vertex_buffers: Vec::new(),
                render_pass,
                pipeline_layout: Some(path_rasterization_pipeline_layout),
                color_format: surface_config.format,
                blend_mode: BlendMode::PremultipliedAlpha,
                primitive_topology: PrimitiveTopology::TriangleList,
            },
            surface_config.size,
        )
        .context("creating nova path rasterization render pipeline")?;
    let paths = device
        .create_render_pipeline(
            &RenderPipelineDesc {
                label: Some(format!("{label} path pipeline")),
                vertex_shader: path_vertex,
                vertex_entry_point: "vs_path".to_string(),
                fragment_shader: path_fragment,
                fragment_entry_point: "fs_path".to_string(),
                vertex_buffers: Vec::new(),
                render_pass,
                pipeline_layout: Some(path_pipeline_layout),
                color_format: surface_config.format,
                blend_mode: BlendMode::AdditiveAlpha,
                primitive_topology: PrimitiveTopology::TriangleStrip,
            },
            surface_config.size,
        )
        .context("creating nova path render pipeline")?;
    Ok(NovaGpuiResources {
        render_pass,
        pipelines: NovaPipelines {
            alpha,
            premultiplied,
            path_rasterization,
            paths,
            backdrop_blur_downsample,
            backdrop_blur_upsample,
        },
        global_buffer,
        text_raster_buffer,
        quad_buffer,
        shadow_buffer,
        path_rasterization_vertex_buffer,
        path_sprite_buffer,
        mono_sprite_buffer,
        poly_sprite_buffer,
        underline_buffer,
        backdrop_blur_pass_buffer,
        backdrop_blur_buffer,
        quad_resource_set,
        shadow_resource_set,
        path_rasterization_resource_set,
        path_resource_set_layout,
        path_resource_set: path_target.resource_set,
        mono_sprite_resource_set,
        poly_sprite_resource_set,
        underline_resource_set,
        backdrop_blur_pass_resource_set_layout,
        backdrop_blur_resource_set_layout,
        backdrop_blur_targets,
        atlas_texture,
        atlas_texture_view,
        atlas_sampler,
        path_texture: path_target.texture,
        path_texture_view: path_target.texture_view,
    })
}

fn clear_color() -> ClearColor {
    ClearColor {
        red: 0.0,
        green: 0.0,
        blue: 0.0,
        alpha: 0.0,
    }
}

impl NovaSurfaceAlphaState {
    #[cfg(test)]
    fn new(swapchain_mode: CompositeAlphaMode) -> Self {
        let output_mode = if matches!(swapchain_mode, CompositeAlphaMode::Premultiplied) {
            NovaSurfaceOutputMode::Premultiplied
        } else {
            NovaSurfaceOutputMode::Straight
        };
        Self {
            swapchain_mode,
            output_mode,
        }
    }

    fn for_window_transparency(transparent: bool) -> Self {
        if transparent {
            Self {
                swapchain_mode: CompositeAlphaMode::Premultiplied,
                output_mode: NovaSurfaceOutputMode::Premultiplied,
            }
        } else {
            Self {
                swapchain_mode: CompositeAlphaMode::Opaque,
                output_mode: NovaSurfaceOutputMode::Straight,
            }
        }
    }

    fn outputs_premultiplied_alpha(self) -> bool {
        matches!(self.output_mode, NovaSurfaceOutputMode::Premultiplied)
    }
}

struct NovaRenderDiagnostics {
    enabled: bool,
    warned_unsupported: bool,
}

impl NovaRenderDiagnostics {
    fn from_env() -> Self {
        Self {
            enabled: env_flag("GPUI_NOVA_RENDER_DIAGNOSTICS"),
            warned_unsupported: false,
        }
    }

    fn should_log_frame(&self, elapsed_ms: u128) -> bool {
        self.enabled || elapsed_ms >= 16
    }

    fn should_warn_unsupported(&mut self, unsupported: UnsupportedBatchSummary) -> bool {
        if unsupported.total() == 0 {
            return false;
        }
        if self.enabled {
            return true;
        }
        if self.warned_unsupported {
            return false;
        }
        self.warned_unsupported = true;
        true
    }
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
}

fn write_atlas_texture<D>(
    device: &mut D,
    texture: TextureId,
    origin: Origin2d,
    size: Extent2d,
    bytes_per_row: u32,
    data: &[u8],
) -> Result<()>
where
    D: GfxResourceDevice,
{
    device.write_texture(
        TextureWriteDesc {
            texture,
            layout: TextureDataLayout::new(0, bytes_per_row, size.height())?,
            origin,
            size,
        },
        data,
    )?;
    Ok(())
}

fn upload_pending_atlas<D>(
    atlas: &NovaAtlas,
    device: &mut D,
    texture: TextureId,
) -> Result<AtlasUploadStats>
where
    D: GfxResourceDevice,
{
    let started_at = Instant::now();
    let stats = atlas.upload_pending_rgba_pixels(|origin, size, bytes_per_row, atlas_pixels| {
        write_atlas_texture(device, texture, origin, size, bytes_per_row, atlas_pixels)
    })?;
    if stats.upload_count > 0 {
        crate::performance_metrics::record_atlas_upload_metrics(
            stats.uploaded_bytes,
            stats.upload_count,
            started_at.elapsed(),
        );
    }
    Ok(stats)
}

fn record_nova_upload_metrics(frame_upload_bytes: usize, atlas_stats: AtlasUploadStats) {
    let atlas_texture_bytes =
        NOVA_ATLAS_SIZE as usize * NOVA_ATLAS_SIZE as usize * NOVA_ATLAS_BYTES_PER_PIXEL;
    crate::performance_metrics::record_upload_bytes(
        frame_upload_bytes.saturating_add(atlas_stats.uploaded_bytes),
    );
    crate::performance_metrics::record_upload_arena_metrics(
        frame_upload_bytes,
        atlas_stats.arena_capacity,
        frame_upload_bytes,
        atlas_stats.arena_used_bytes,
    );
    crate::performance_metrics::record_gpu_resource_breakdown(
        atlas_texture_bytes,
        false,
        false,
        false,
        false,
        0,
        0,
    );
    crate::performance_metrics::record_gpu_retained_bytes(
        atlas_texture_bytes.saturating_add(frame_upload_bytes),
    );
}

struct NovaAtlas {
    state: Mutex<NovaAtlasState>,
}

#[derive(Default)]
struct NovaAtlasState {
    next_tile_id: u32,
    cursor_x: u32,
    cursor_y: u32,
    row_height: u32,
    tiles: FxHashMap<AtlasKey, AtlasTile>,
    upload_bytes: Vec<u8>,
    pending_uploads: Vec<PendingAtlasUpload>,
}

#[derive(Clone, Copy)]
struct PendingAtlasUpload {
    origin: Origin2d,
    size: Extent2d,
    bytes_per_row: u32,
    offset: usize,
    len: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct AtlasUploadStats {
    uploaded_bytes: usize,
    upload_count: usize,
    arena_used_bytes: usize,
    arena_capacity: usize,
}

#[derive(Default)]
struct AtlasUploadBatch {
    bytes: Vec<u8>,
    uploads: Vec<PendingAtlasUpload>,
}

impl NovaAtlas {
    fn new() -> Self {
        Self {
            state: Mutex::new(NovaAtlasState::default()),
        }
    }

    fn upload_pending_rgba_pixels(
        &self,
        mut upload: impl FnMut(Origin2d, Extent2d, u32, &[u8]) -> Result<()>,
    ) -> Result<AtlasUploadStats> {
        let batch = self.take_pending_uploads();
        let mut stats = AtlasUploadStats {
            arena_used_bytes: batch.bytes.len(),
            arena_capacity: batch.bytes.capacity(),
            ..AtlasUploadStats::default()
        };
        let result = (|| {
            for pending_upload in &batch.uploads {
                let end = pending_upload
                    .offset
                    .checked_add(pending_upload.len)
                    .ok_or_else(|| anyhow::anyhow!("nova atlas upload range overflow"))?;
                let pixels = batch.bytes.get(pending_upload.offset..end).ok_or_else(|| {
                    anyhow::anyhow!("nova atlas pending upload range is out of bounds")
                })?;
                upload(
                    pending_upload.origin,
                    pending_upload.size,
                    pending_upload.bytes_per_row,
                    pixels,
                )?;
                stats.uploaded_bytes = stats.uploaded_bytes.saturating_add(pixels.len());
                stats.upload_count = stats.upload_count.saturating_add(1);
            }
            Ok(())
        })();
        if result.is_ok() {
            self.recycle_upload_batch(batch);
        } else {
            self.restore_upload_batch(batch);
        }
        result.map(|()| stats)
    }

    fn take_pending_uploads(&self) -> AtlasUploadBatch {
        let mut state = self.state.lock().expect("nova atlas lock poisoned");
        AtlasUploadBatch {
            bytes: std::mem::take(&mut state.upload_bytes),
            uploads: std::mem::take(&mut state.pending_uploads),
        }
    }

    fn recycle_upload_batch(&self, mut batch: AtlasUploadBatch) {
        batch.bytes.clear();
        batch.uploads.clear();
        let mut state = self.state.lock().expect("nova atlas lock poisoned");
        if state.upload_bytes.is_empty()
            && state.pending_uploads.is_empty()
            && batch.bytes.capacity() > state.upload_bytes.capacity()
        {
            state.upload_bytes = batch.bytes;
        }
        if state.pending_uploads.is_empty()
            && batch.uploads.capacity() > state.pending_uploads.capacity()
        {
            state.pending_uploads = batch.uploads;
        }
    }

    fn restore_upload_batch(&self, batch: AtlasUploadBatch) {
        let mut state = self.state.lock().expect("nova atlas lock poisoned");
        let base_offset = state.upload_bytes.len();
        state.upload_bytes.extend_from_slice(&batch.bytes);
        state.pending_uploads.reserve(batch.uploads.len());
        for mut upload in batch.uploads {
            upload.offset = upload.offset.saturating_add(base_offset);
            state.pending_uploads.push(upload);
        }
    }

    fn trim(&self, level: GpuiMemoryTrimLevel) {
        let mut state = self.state.lock().expect("nova atlas lock poisoned");
        match level {
            GpuiMemoryTrimLevel::Light | GpuiMemoryTrimLevel::Moderate => {
                if state.pending_uploads.is_empty() {
                    state.upload_bytes.clear();
                    state.upload_bytes.shrink_to(0);
                    state.pending_uploads.shrink_to(0);
                }
            }
            GpuiMemoryTrimLevel::Aggressive => {
                *state = NovaAtlasState::default();
            }
        }
    }

    #[cfg(test)]
    fn pending_upload_bytes_for_test(&self) -> Vec<u8> {
        self.state
            .lock()
            .expect("nova atlas lock poisoned")
            .upload_bytes
            .clone()
    }

    #[cfg(test)]
    fn pending_upload_count_for_test(&self) -> usize {
        self.state
            .lock()
            .expect("nova atlas lock poisoned")
            .pending_uploads
            .len()
    }
}

impl PlatformAtlas for NovaAtlas {
    fn get_or_insert_with<'a>(
        &self,
        key: &AtlasKey,
        build: &mut dyn FnMut() -> Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> Result<Option<AtlasTile>> {
        let mut state = self
            .state
            .lock()
            .expect("nova placeholder atlas lock poisoned");
        if let Some(tile) = state.tiles.get(key) {
            return Ok(Some(*tile));
        }
        drop(state);

        let Some((size, bytes)) = build()? else {
            return Ok(None);
        };

        let mut state = self.state.lock().expect("nova atlas lock poisoned");
        let Some(tile) = state.allocate_and_upload(key, size, &bytes) else {
            return Ok(None);
        };
        state.tiles.insert(key.clone(), tile);
        Ok(Some(tile))
    }

    fn get_or_update_with<'a>(
        &self,
        key: &AtlasKey,
        build: &mut dyn FnMut() -> Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> Result<Option<AtlasTile>> {
        let Some((size, bytes)) = build()? else {
            return Ok(None);
        };
        let mut state = self.state.lock().expect("nova atlas lock poisoned");
        if let Some(tile) = state.tiles.get(key).copied() {
            if tile.bounds.size == size {
                if state.enqueue_tile_upload(
                    key,
                    tile.bounds.origin,
                    size,
                    bytes.as_ref(),
                    tile.padding,
                ) {
                    return Ok(Some(tile));
                }
                return Ok(None);
            }
        }
        drop(state);
        self.remove(key);
        self.get_or_insert_with(key, &mut || Ok(Some((size, Cow::Borrowed(bytes.as_ref())))))
    }

    fn get_or_insert_glyph(
        &self,
        params: &RenderGlyphParams,
        build: &mut dyn FnMut() -> Result<GlyphRasterization>,
    ) -> Result<Option<AtlasTile>> {
        let key = AtlasKey::from(params.clone());
        let mut build_tile = || match build()? {
            GlyphRasterization::Bitmap { size, bytes } => Ok(Some((size, Cow::Owned(bytes)))),
            GlyphRasterization::ColorLayers { fallback, .. } => {
                Ok(Some((fallback.size, Cow::Owned(fallback.bytes))))
            }
        };
        if let Some(tile) = self.get_or_insert_with(&key, &mut build_tile)? {
            return Ok(Some(tile));
        }

        let texture_kind = key.texture_kind();
        log::warn!(
            "nova atlas glyph allocation failed; resetting atlas kind and retrying: kind={:?}",
            texture_kind
        );
        {
            let mut state = self.state.lock().expect("nova atlas lock poisoned");
            state.reset_kind(texture_kind);
        }
        self.get_or_insert_with(&key, &mut build_tile)
    }

    fn clear_glyphs(&self) {
        let mut state = self.state.lock().expect("nova atlas lock poisoned");
        state
            .tiles
            .retain(|key, _tile| !matches!(key, AtlasKey::Glyph(_)));
    }

    fn remove(&self, key: &AtlasKey) {
        let mut state = self.state.lock().expect("nova atlas lock poisoned");
        state.tiles.remove(key);
    }
}

impl NovaAtlasState {
    fn reset_kind(&mut self, texture_kind: AtlasTextureKind) {
        self.tiles
            .retain(|key, _tile| key.texture_kind() != texture_kind);
        self.pending_uploads.clear();
        self.upload_bytes.clear();
        self.cursor_x = 0;
        self.cursor_y = 0;
        self.row_height = 0;
    }

    fn allocate_and_upload(
        &mut self,
        key: &AtlasKey,
        size: Size<DevicePixels>,
        bytes: &[u8],
    ) -> Option<AtlasTile> {
        let texture_kind = key.texture_kind();
        let width = size.width.0.max(1) as u32;
        let height = size.height.0.max(1) as u32;
        let padded_width = width.saturating_add(NOVA_ATLAS_TILE_PADDING.saturating_mul(2));
        let padded_height = height.saturating_add(NOVA_ATLAS_TILE_PADDING.saturating_mul(2));
        if padded_width > NOVA_ATLAS_SIZE || padded_height > NOVA_ATLAS_SIZE {
            return None;
        }
        if self.cursor_x + padded_width > NOVA_ATLAS_SIZE {
            self.cursor_x = 0;
            self.cursor_y = self.cursor_y.saturating_add(self.row_height);
            self.row_height = 0;
        }
        if self.cursor_y + padded_height > NOVA_ATLAS_SIZE {
            return None;
        }

        let origin = Point {
            x: DevicePixels(i32::try_from(self.cursor_x + NOVA_ATLAS_TILE_PADDING).ok()?),
            y: DevicePixels(i32::try_from(self.cursor_y + NOVA_ATLAS_TILE_PADDING).ok()?),
        };
        if !self.enqueue_tile_upload(key, origin, size, bytes, NOVA_ATLAS_TILE_PADDING) {
            return None;
        }
        self.next_tile_id = self.next_tile_id.saturating_add(1);
        let tile = AtlasTile {
            texture_id: AtlasTextureId {
                index: 0,
                kind: texture_kind,
            },
            tile_id: TileId(self.next_tile_id),
            padding: NOVA_ATLAS_TILE_PADDING,
            bounds: Bounds { origin, size },
        };
        self.cursor_x = self.cursor_x.saturating_add(padded_width);
        self.row_height = self.row_height.max(padded_height);
        Some(tile)
    }

    fn enqueue_tile_upload(
        &mut self,
        key: &AtlasKey,
        origin: Point<DevicePixels>,
        size: Size<DevicePixels>,
        bytes: &[u8],
        padding: u32,
    ) -> bool {
        let width = size.width.0.max(1) as u32;
        let height = size.height.0.max(1) as u32;
        let upload_width = width.saturating_add(padding.saturating_mul(2));
        let upload_height = height.saturating_add(padding.saturating_mul(2));
        let Ok(extent) = Extent2d::new(upload_width, upload_height) else {
            return false;
        };
        let Some(bytes_per_row) = upload_width.checked_mul(NOVA_ATLAS_BYTES_PER_PIXEL as u32)
        else {
            return false;
        };
        let Some(len) = bytes_per_row
            .checked_mul(upload_height)
            .and_then(|value| usize::try_from(value).ok())
        else {
            return false;
        };
        let offset = self.upload_bytes.len();
        let Some(end) = offset.checked_add(len) else {
            return false;
        };
        self.upload_bytes.resize(end, 0);
        if encode_rgba_upload_with_padding(
            &mut self.upload_bytes[offset..end],
            size,
            bytes,
            key.texture_kind(),
            padding,
        )
        .is_none()
        {
            self.upload_bytes.truncate(offset);
            return false;
        }
        self.pending_uploads.push(PendingAtlasUpload {
            origin: Origin2d {
                x: origin
                    .x
                    .0
                    .saturating_sub(i32::try_from(padding).unwrap_or(0))
                    .max(0) as u32,
                y: origin
                    .y
                    .0
                    .saturating_sub(i32::try_from(padding).unwrap_or(0))
                    .max(0) as u32,
            },
            size: extent,
            bytes_per_row,
            offset,
            len,
        });
        true
    }
}

#[cfg(test)]
fn encode_rgba_upload(
    pixels: &mut [u8],
    size: Size<DevicePixels>,
    bytes: &[u8],
    texture_kind: AtlasTextureKind,
) -> Option<()> {
    encode_rgba_upload_with_padding(pixels, size, bytes, texture_kind, 0)
}

fn encode_rgba_upload_with_padding(
    pixels: &mut [u8],
    size: Size<DevicePixels>,
    bytes: &[u8],
    texture_kind: AtlasTextureKind,
    padding: u32,
) -> Option<()> {
    let width = size.width.0.max(1) as usize;
    let height = size.height.0.max(1) as usize;
    let padding = padding as usize;
    let upload_width = width.saturating_add(padding.saturating_mul(2));
    let upload_height = height.saturating_add(padding.saturating_mul(2));
    if pixels.len()
        < upload_width
            .saturating_mul(upload_height)
            .saturating_mul(NOVA_ATLAS_BYTES_PER_PIXEL)
    {
        return None;
    }
    for upload_y in 0..upload_height {
        let y = upload_y
            .saturating_sub(padding)
            .min(height.saturating_sub(1));
        for upload_x in 0..upload_width {
            let x = upload_x
                .saturating_sub(padding)
                .min(width.saturating_sub(1));
            let (red, green, blue, alpha) = match texture_kind {
                AtlasTextureKind::Monochrome => {
                    let source_index = y.saturating_mul(width).saturating_add(x);
                    let coverage = bytes.get(source_index).copied()?;
                    (coverage, 0, 0, 255)
                }
                AtlasTextureKind::Rgba => {
                    let source_index = y
                        .saturating_mul(width)
                        .saturating_add(x)
                        .saturating_mul(NOVA_ATLAS_BYTES_PER_PIXEL);
                    let source = bytes.get(source_index..source_index + 4)?;
                    let red = source[0];
                    let green = source[1];
                    let blue = source[2];
                    let alpha = source[3];
                    (red, green, blue, alpha)
                }
                AtlasTextureKind::Bgra => {
                    let source_index = y
                        .saturating_mul(width)
                        .saturating_add(x)
                        .saturating_mul(NOVA_ATLAS_BYTES_PER_PIXEL);
                    let source = bytes.get(source_index..source_index + 4)?;
                    let blue = source[0];
                    let green = source[1];
                    let red = source[2];
                    let alpha = source[3];
                    (red, green, blue, alpha)
                }
                AtlasTextureKind::Subpixel => {
                    let source_index = y
                        .saturating_mul(width)
                        .saturating_add(x)
                        .saturating_mul(NOVA_ATLAS_BYTES_PER_PIXEL);
                    let source = bytes.get(source_index..source_index + 4)?;
                    let coverage = subpixel_coverage(source[0], source[1], source[2], source[3]);
                    (coverage, 0, 0, 255)
                }
            };
            let atlas_index = (upload_y * upload_width + upload_x) * NOVA_ATLAS_BYTES_PER_PIXEL;
            if let Some(pixel) = pixels.get_mut(atlas_index..atlas_index + 4) {
                pixel.copy_from_slice(&[red, green, blue, alpha]);
            }
        }
    }
    Some(())
}

fn subpixel_coverage(red: u8, green: u8, blue: u8, alpha: u8) -> u8 {
    let coverage = u16::from(red)
        .saturating_add(u16::from(green))
        .saturating_add(u16::from(blue))
        / 3;
    let premultiplied_coverage = coverage.saturating_mul(u16::from(alpha)) / 255;
    u8::try_from(premultiplied_coverage).unwrap_or(u8::MAX)
}

fn write_backdrop_blur_pass(bytes: &mut Vec<u8>, offset: f32) {
    write_f32_vec(bytes, offset);
    write_f32_vec(bytes, 0.0);
    write_f32_vec(bytes, 0.0);
    write_u32_vec(bytes, 0);
}

fn write_backdrop_blur(
    bytes: &mut Vec<u8>,
    blur: &crate::PaintBackdropBlur,
    drawable_size: DrawableSize,
) {
    write_u32_vec(bytes, blur.order);
    write_u32_vec(bytes, u32::from(blur.downsample));
    write_u32_vec(bytes, u32::from(blur.levels.clamp(1, 6)));
    write_u32_vec(bytes, 0);
    write_bounds_scaled(bytes, &blur.bounds);
    write_bounds_scaled(bytes, &blur.content_mask.bounds);
    write_corners(bytes, &blur.corner_radii);
    write_hsla(
        bytes,
        blur.tint.unwrap_or_else(crate::Hsla::transparent_black),
    );
    write_f32_vec(bytes, blur.radius.0);
    write_f32_vec(bytes, blur.saturation);
    write_f32_vec(bytes, drawable_size.width as f32);
    write_f32_vec(bytes, drawable_size.height as f32);
    write_u32_vec(bytes, 0);
    write_u32_vec(bytes, 0);
}

fn backdrop_blur_offset(radius: f32, downsample: u8, levels: u8) -> f32 {
    let downsample = f32::from(downsample.max(1));
    let levels = f32::from(levels.clamp(1, 6));
    (radius / downsample / levels).clamp(0.5, 6.0)
}

#[derive(Default)]
struct FrameUploadSummary {
    quad_count: u32,
    shadow_count: u32,
    path_vertex_count: u32,
    path_sprite_count: u32,
    mono_sprite_count: u32,
    poly_sprite_count: u32,
    underline_count: u32,
    unsupported_batches: UnsupportedBatchSummary,
}

#[derive(Clone, Copy, Default)]
struct UnsupportedBatchSummary {
    paths: u32,
    surfaces: u32,
    backdrop_blurs: u32,
    backdrop_blur_tint_fallbacks: u32,
    gpu_meshes_3d: u32,
}

impl UnsupportedBatchSummary {
    fn total(self) -> u32 {
        self.paths
            .saturating_add(self.surfaces)
            .saturating_add(self.backdrop_blurs)
            .saturating_add(self.backdrop_blur_tint_fallbacks)
            .saturating_add(self.gpu_meshes_3d)
    }
}

#[derive(Clone, Copy)]
enum NovaUploadedBatch {
    SolidQuads {
        first: u32,
        count: u32,
    },
    Quads {
        first: u32,
        count: u32,
    },
    Shadows {
        first: u32,
        count: u32,
    },
    PathRasterization {
        first_vertex: u32,
        vertex_count: u32,
    },
    Paths {
        first: u32,
        count: u32,
    },
    MonoSprites {
        first: u32,
        count: u32,
    },
    PolySprites {
        first: u32,
        count: u32,
    },
    Underlines {
        first: u32,
        count: u32,
    },
    BackdropBlurs {
        first: u32,
        count: u32,
    },
}

#[derive(Default)]
struct NovaFrameUpload {
    globals: Vec<u8>,
    text_raster_params: Vec<u8>,
    quads: Vec<u8>,
    shadows: Vec<u8>,
    path_rasterization_vertices: Vec<u8>,
    path_sprites: Vec<u8>,
    mono_sprites: Vec<u8>,
    poly_sprites: Vec<u8>,
    underlines: Vec<u8>,
    backdrop_blur_passes: Vec<u8>,
    backdrop_blurs: Vec<u8>,
    batches: Vec<NovaUploadedBatch>,
    backdrop_blur_downsample: u8,
    backdrop_blur_levels: u8,
}

impl NovaFrameUpload {
    fn trim_retained_capacity(&mut self, level: GpuiMemoryTrimLevel) {
        let multiplier = match level {
            GpuiMemoryTrimLevel::Light => 16,
            GpuiMemoryTrimLevel::Moderate => 8,
            GpuiMemoryTrimLevel::Aggressive => 1,
        };
        trim_upload_vec(&mut self.globals, GLOBAL_UPLOAD_BYTES, multiplier);
        trim_upload_vec(
            &mut self.text_raster_params,
            TEXT_RASTER_UPLOAD_BYTES,
            multiplier,
        );
        trim_upload_vec(&mut self.quads, 64 * PACKED_QUAD_BYTES, multiplier);
        trim_upload_vec(&mut self.shadows, 64 * PACKED_SHADOW_BYTES, multiplier);
        trim_upload_vec(
            &mut self.path_rasterization_vertices,
            256 * PACKED_PATH_RASTERIZATION_VERTEX_BYTES,
            multiplier,
        );
        trim_upload_vec(
            &mut self.path_sprites,
            64 * PACKED_PATH_SPRITE_BYTES,
            multiplier,
        );
        trim_upload_vec(
            &mut self.mono_sprites,
            64 * PACKED_MONO_SPRITE_BYTES,
            multiplier,
        );
        trim_upload_vec(
            &mut self.poly_sprites,
            64 * PACKED_POLY_SPRITE_BYTES,
            multiplier,
        );
        trim_upload_vec(
            &mut self.underlines,
            64 * PACKED_UNDERLINE_BYTES,
            multiplier,
        );
        trim_upload_vec(
            &mut self.backdrop_blur_passes,
            BACKDROP_BLUR_PASS_BYTES,
            multiplier,
        );
        trim_upload_vec(
            &mut self.backdrop_blurs,
            PACKED_BACKDROP_BLUR_BYTES,
            multiplier,
        );
        trim_upload_vec(&mut self.batches, 64, multiplier);
    }

    fn encode(
        &mut self,
        scene: &crate::Scene,
        drawable_size: DrawableSize,
        rendering_parameters: &NovaRenderingParameters,
        premultiplied_alpha: bool,
    ) -> FrameUploadSummary {
        self.globals.clear();
        self.text_raster_params.clear();
        self.quads.clear();
        self.shadows.clear();
        self.path_rasterization_vertices.clear();
        self.path_sprites.clear();
        self.mono_sprites.clear();
        self.poly_sprites.clear();
        self.underlines.clear();
        self.backdrop_blur_passes.clear();
        self.backdrop_blurs.clear();
        self.batches.clear();
        self.backdrop_blur_downsample = DEFAULT_BACKDROP_BLUR_DOWNSAMPLE;
        self.backdrop_blur_levels = 1;
        self.globals.reserve(GLOBAL_UPLOAD_BYTES);
        self.text_raster_params.reserve(TEXT_RASTER_UPLOAD_BYTES);
        self.path_rasterization_vertices
            .reserve(PACKED_PATH_RASTERIZATION_VERTEX_BYTES);
        self.path_sprites.reserve(PACKED_PATH_SPRITE_BYTES);
        self.backdrop_blur_passes.reserve(BACKDROP_BLUR_PASS_BYTES);
        self.backdrop_blurs.reserve(PACKED_BACKDROP_BLUR_BYTES);
        write_backdrop_blur_pass(&mut self.backdrop_blur_passes, 1.0);
        write_f32_vec(&mut self.globals, drawable_size.width as f32);
        write_f32_vec(&mut self.globals, drawable_size.height as f32);
        write_u32_vec(&mut self.globals, u32::from(premultiplied_alpha));
        write_u32_vec(&mut self.globals, 0);
        for value in rendering_parameters.gamma_ratios {
            write_f32_vec(&mut self.text_raster_params, value);
        }
        write_f32_vec(
            &mut self.text_raster_params,
            rendering_parameters.grayscale_enhanced_contrast,
        );
        write_f32_vec(&mut self.text_raster_params, 0.0);
        write_f32_vec(&mut self.text_raster_params, 0.0);
        write_f32_vec(&mut self.text_raster_params, 0.0);

        let mut summary = FrameUploadSummary::default();
        for batch in scene.prepared_batches() {
            match batch {
                PreparedSceneBatch::Quads(quad_run) => {
                    let first = (self.quads.len() / PACKED_QUAD_BYTES) as u32;
                    let mut count = 0_u32;
                    for quad in &scene.quads[quad_run.range.clone()] {
                        if self.quads.len() / PACKED_QUAD_BYTES >= MAX_QUADS {
                            break;
                        }
                        write_quad(&mut self.quads, quad);
                        count = count.saturating_add(1);
                    }
                    if count > 0 {
                        self.batches.push(if quad_run.is_solid {
                            NovaUploadedBatch::SolidQuads { first, count }
                        } else {
                            NovaUploadedBatch::Quads { first, count }
                        });
                        summary.quad_count = summary.quad_count.saturating_add(count);
                    }
                }
                PreparedSceneBatch::Shadows(range) => {
                    let first = (self.shadows.len() / PACKED_SHADOW_BYTES) as u32;
                    let mut count = 0_u32;
                    for shadow in &scene.shadows[range.clone()] {
                        if self.shadows.len() / PACKED_SHADOW_BYTES >= MAX_SHADOWS {
                            break;
                        }
                        write_shadow(&mut self.shadows, shadow);
                        count = count.saturating_add(1);
                    }
                    if count > 0 {
                        self.batches
                            .push(NovaUploadedBatch::Shadows { first, count });
                        summary.shadow_count = summary.shadow_count.saturating_add(count);
                    }
                }
                PreparedSceneBatch::MonochromeSprites { range, .. } => {
                    let first = (self.mono_sprites.len() / PACKED_MONO_SPRITE_BYTES) as u32;
                    let mut count = 0_u32;
                    for sprite in &scene.monochrome_sprites[range.clone()] {
                        if self.mono_sprites.len() / PACKED_MONO_SPRITE_BYTES >= MAX_MONO_SPRITES {
                            break;
                        }
                        write_monochrome_sprite(&mut self.mono_sprites, sprite);
                        count = count.saturating_add(1);
                    }
                    if count > 0 {
                        self.batches
                            .push(NovaUploadedBatch::MonoSprites { first, count });
                        summary.mono_sprite_count = summary.mono_sprite_count.saturating_add(count);
                    }
                }
                PreparedSceneBatch::SubpixelSprites { range, .. } => {
                    let first = (self.mono_sprites.len() / PACKED_MONO_SPRITE_BYTES) as u32;
                    let mut count = 0_u32;
                    for sprite in &scene.subpixel_sprites[range.clone()] {
                        if self.mono_sprites.len() / PACKED_MONO_SPRITE_BYTES >= MAX_MONO_SPRITES {
                            break;
                        }
                        write_subpixel_sprite_as_mono(&mut self.mono_sprites, sprite);
                        count = count.saturating_add(1);
                    }
                    if count > 0 {
                        self.batches
                            .push(NovaUploadedBatch::MonoSprites { first, count });
                        summary.mono_sprite_count = summary.mono_sprite_count.saturating_add(count);
                    }
                }
                PreparedSceneBatch::PolychromeSprites { range, .. } => {
                    let first = (self.poly_sprites.len() / PACKED_POLY_SPRITE_BYTES) as u32;
                    let mut count = 0_u32;
                    for sprite in &scene.polychrome_sprites[range.clone()] {
                        if self.poly_sprites.len() / PACKED_POLY_SPRITE_BYTES >= MAX_POLY_SPRITES {
                            break;
                        }
                        write_polychrome_sprite(&mut self.poly_sprites, sprite);
                        count = count.saturating_add(1);
                    }
                    if count > 0 {
                        self.batches
                            .push(NovaUploadedBatch::PolySprites { first, count });
                        summary.poly_sprite_count = summary.poly_sprite_count.saturating_add(count);
                    }
                }
                PreparedSceneBatch::Underlines(range) => {
                    let first = (self.underlines.len() / PACKED_UNDERLINE_BYTES) as u32;
                    let mut count = 0_u32;
                    for underline in &scene.underlines[range.clone()] {
                        if self.underlines.len() / PACKED_UNDERLINE_BYTES >= MAX_UNDERLINES {
                            break;
                        }
                        write_underline(&mut self.underlines, underline);
                        count = count.saturating_add(1);
                    }
                    if count > 0 {
                        self.batches
                            .push(NovaUploadedBatch::Underlines { first, count });
                        summary.underline_count = summary.underline_count.saturating_add(count);
                    }
                }
                PreparedSceneBatch::Paths(range) => {
                    let paths = &scene.paths[range.clone()];
                    let first_vertex = (self.path_rasterization_vertices.len()
                        / PACKED_PATH_RASTERIZATION_VERTEX_BYTES)
                        as u32;
                    let mut vertex_count = 0_u32;
                    for path in paths {
                        let bounds = path.clipped_bounds();
                        for vertex in &path.vertices {
                            if self.path_rasterization_vertices.len()
                                / PACKED_PATH_RASTERIZATION_VERTEX_BYTES
                                >= MAX_PATH_VERTICES
                            {
                                break;
                            }
                            write_path_rasterization_vertex(
                                &mut self.path_rasterization_vertices,
                                vertex,
                                &path.color,
                                &bounds,
                            );
                            vertex_count = vertex_count.saturating_add(1);
                        }
                    }
                    if vertex_count > 0 {
                        self.batches.push(NovaUploadedBatch::PathRasterization {
                            first_vertex,
                            vertex_count,
                        });
                        summary.path_vertex_count =
                            summary.path_vertex_count.saturating_add(vertex_count);
                    }

                    let Some(first_path) = paths.first() else {
                        continue;
                    };
                    let first = (self.path_sprites.len() / PACKED_PATH_SPRITE_BYTES) as u32;
                    let mut count = 0_u32;
                    if paths
                        .last()
                        .is_some_and(|path| path.order == first_path.order)
                    {
                        for path in paths {
                            if self.path_sprites.len() / PACKED_PATH_SPRITE_BYTES
                                >= MAX_PATH_SPRITES
                            {
                                break;
                            }
                            write_path_sprite(&mut self.path_sprites, &path.clipped_bounds());
                            count = count.saturating_add(1);
                        }
                    } else {
                        let mut bounds = first_path.clipped_bounds();
                        for path in paths.iter().skip(1) {
                            bounds = bounds.union(&path.clipped_bounds());
                        }
                        if self.path_sprites.len() / PACKED_PATH_SPRITE_BYTES < MAX_PATH_SPRITES {
                            write_path_sprite(&mut self.path_sprites, &bounds);
                            count = 1;
                        }
                    }
                    if count > 0 {
                        self.batches.push(NovaUploadedBatch::Paths { first, count });
                        summary.path_sprite_count = summary.path_sprite_count.saturating_add(count);
                    }
                }
                PreparedSceneBatch::Surfaces(_) => {
                    summary.unsupported_batches.surfaces =
                        summary.unsupported_batches.surfaces.saturating_add(1);
                }
                PreparedSceneBatch::BackdropBlurs(group) => {
                    let first = (self.backdrop_blurs.len() / PACKED_BACKDROP_BLUR_BYTES) as u32;
                    let mut count = 0_u32;
                    for blur in &scene.backdrop_blurs[group.range.clone()] {
                        if self.backdrop_blurs.len() / PACKED_BACKDROP_BLUR_BYTES
                            >= MAX_BACKDROP_BLURS
                        {
                            break;
                        }
                        if count == 0 {
                            self.backdrop_blur_passes.clear();
                            self.backdrop_blur_downsample = blur.downsample.max(1);
                            self.backdrop_blur_levels =
                                blur.levels.clamp(1, MAX_BACKDROP_BLUR_LEVELS);
                            write_backdrop_blur_pass(
                                &mut self.backdrop_blur_passes,
                                backdrop_blur_offset(blur.radius.0, blur.downsample, blur.levels),
                            );
                        }
                        write_backdrop_blur(&mut self.backdrop_blurs, blur, drawable_size);
                        count = count.saturating_add(1);
                    }
                    if count > 0 {
                        self.batches
                            .push(NovaUploadedBatch::BackdropBlurs { first, count });
                    }
                }
                PreparedSceneBatch::GpuMeshes3d(_) => {
                    summary.unsupported_batches.gpu_meshes_3d =
                        summary.unsupported_batches.gpu_meshes_3d.saturating_add(1);
                }
            }
        }
        summary
    }

    fn backdrop_blur_downsample(&self) -> u8 {
        self.backdrop_blur_downsample.max(1)
    }

    fn backdrop_blur_levels(&self) -> usize {
        usize::from(self.backdrop_blur_levels.clamp(1, MAX_BACKDROP_BLUR_LEVELS))
    }

    fn uploaded_bytes(&self) -> usize {
        self.globals
            .len()
            .saturating_add(self.text_raster_params.len())
            .saturating_add(self.quads.len())
            .saturating_add(self.shadows.len())
            .saturating_add(self.path_rasterization_vertices.len())
            .saturating_add(self.path_sprites.len())
            .saturating_add(self.mono_sprites.len())
            .saturating_add(self.poly_sprites.len())
            .saturating_add(self.underlines.len())
            .saturating_add(self.backdrop_blur_passes.len())
            .saturating_add(self.backdrop_blurs.len())
    }
}

fn trim_upload_vec<T>(vec: &mut Vec<T>, floor: usize, multiplier: usize) {
    let target = floor.max(1);
    if vec.capacity() > target.saturating_mul(multiplier.max(1)) {
        vec.shrink_to(target);
    }
}

struct NovaRenderingParameters {
    gamma_ratios: [f32; 4],
    grayscale_enhanced_contrast: f32,
}

impl NovaRenderingParameters {
    fn from_env() -> Self {
        let gamma = std::env::var("ZED_FONTS_GAMMA")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(1.45_f32)
            .clamp(1.0, 2.2);
        let grayscale_enhanced_contrast = std::env::var("ZED_FONTS_GRAYSCALE_ENHANCED_CONTRAST")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0.35_f32)
            .max(0.0);
        Self {
            gamma_ratios: gamma_ratios(gamma),
            grayscale_enhanced_contrast,
        }
    }
}

fn gamma_ratios(gamma: f32) -> [f32; 4] {
    const GAMMA_INCORRECT_TARGET_RATIOS: [[f32; 4]; 13] = [
        [0.0000 / 4.0, 0.0000 / 4.0, 0.0000 / 4.0, 0.0000 / 4.0],
        [0.0166 / 4.0, -0.0807 / 4.0, 0.2227 / 4.0, -0.0751 / 4.0],
        [0.0350 / 4.0, -0.1760 / 4.0, 0.4325 / 4.0, -0.1370 / 4.0],
        [0.0543 / 4.0, -0.2821 / 4.0, 0.6302 / 4.0, -0.1876 / 4.0],
        [0.0739 / 4.0, -0.3963 / 4.0, 0.8167 / 4.0, -0.2287 / 4.0],
        [0.0933 / 4.0, -0.5161 / 4.0, 0.9926 / 4.0, -0.2616 / 4.0],
        [0.1121 / 4.0, -0.6395 / 4.0, 1.1588 / 4.0, -0.2877 / 4.0],
        [0.1300 / 4.0, -0.7649 / 4.0, 1.3159 / 4.0, -0.3080 / 4.0],
        [0.1469 / 4.0, -0.8911 / 4.0, 1.4644 / 4.0, -0.3234 / 4.0],
        [0.1627 / 4.0, -1.0170 / 4.0, 1.6051 / 4.0, -0.3347 / 4.0],
        [0.1773 / 4.0, -1.1420 / 4.0, 1.7385 / 4.0, -0.3426 / 4.0],
        [0.1908 / 4.0, -1.2652 / 4.0, 1.8650 / 4.0, -0.3476 / 4.0],
        [0.2031 / 4.0, -1.3864 / 4.0, 1.9851 / 4.0, -0.3501 / 4.0],
    ];
    const NORM13: f32 = ((0x10000 as f64) / (255.0 * 255.0) * 4.0) as f32;
    const NORM24: f32 = ((0x100 as f64) / 255.0 * 4.0) as f32;
    let index = ((gamma * 10.0).round() as usize).clamp(10, 22) - 10;
    let ratios = GAMMA_INCORRECT_TARGET_RATIOS[index];
    [
        ratios[0] * NORM13,
        ratios[1] * NORM24,
        ratios[2] * NORM13,
        ratios[3] * NORM24,
    ]
}

fn write_quad(bytes: &mut Vec<u8>, quad: &Quad) {
    write_u32_vec(bytes, quad.order);
    write_u32_vec(bytes, quad.border_style as u32);
    write_bounds_scaled(bytes, &quad.bounds);
    write_bounds_scaled(bytes, &quad.content_mask.bounds);
    write_background(bytes, &quad.background);
    write_hsla(bytes, quad.border_color);
    write_corners(bytes, &quad.corner_radii);
    write_edges(bytes, &quad.border_widths);
}

fn write_shadow(bytes: &mut Vec<u8>, shadow: &Shadow) {
    write_u32_vec(bytes, shadow.order);
    write_f32_vec(bytes, shadow.blur_radius.0);
    write_bounds_scaled(bytes, &shadow.bounds);
    write_corners(bytes, &shadow.corner_radii);
    write_bounds_scaled(bytes, &shadow.content_mask.bounds);
    write_hsla(bytes, shadow.color);
}

fn write_path_rasterization_vertex(
    bytes: &mut Vec<u8>,
    vertex: &crate::PathVertex_ScaledPixels,
    background: &crate::Background,
    bounds: &Bounds<crate::ScaledPixels>,
) {
    write_f32_vec(bytes, vertex.xy_position.x.0);
    write_f32_vec(bytes, vertex.xy_position.y.0);
    write_f32_vec(bytes, vertex.st_position.x);
    write_f32_vec(bytes, vertex.st_position.y);
    write_background(bytes, background);
    write_bounds_scaled(bytes, bounds);
}

fn write_path_sprite(bytes: &mut Vec<u8>, bounds: &Bounds<crate::ScaledPixels>) {
    write_bounds_scaled(bytes, bounds);
}

fn write_monochrome_sprite(bytes: &mut Vec<u8>, sprite: &MonochromeSprite) {
    write_u32_vec(bytes, sprite.order);
    write_u32_vec(bytes, sprite.pad);
    write_bounds_scaled(bytes, &sprite.bounds);
    write_bounds_scaled(bytes, &sprite.content_mask.bounds);
    write_hsla(bytes, sprite.color);
    write_atlas_tile(bytes, &sprite.tile);
    write_transformation(bytes, &sprite.transformation);
}

fn write_subpixel_sprite_as_mono(bytes: &mut Vec<u8>, sprite: &crate::SubpixelSprite) {
    write_u32_vec(bytes, sprite.order);
    write_u32_vec(bytes, sprite.pad);
    write_bounds_scaled(bytes, &sprite.bounds);
    write_bounds_scaled(bytes, &sprite.content_mask.bounds);
    write_hsla(bytes, sprite.color);
    write_atlas_tile(bytes, &sprite.tile);
    write_transformation(bytes, &sprite.transformation);
}

fn write_polychrome_sprite(bytes: &mut Vec<u8>, sprite: &PolychromeSprite) {
    write_u32_vec(bytes, sprite.order);
    write_u32_vec(bytes, sprite.pad);
    write_u32_vec(bytes, u32::from(sprite.grayscale));
    write_f32_vec(bytes, sprite.opacity);
    write_bounds_scaled(bytes, &sprite.bounds);
    write_bounds_scaled(bytes, &sprite.content_mask.bounds);
    write_corners(bytes, &sprite.corner_radii);
    write_atlas_tile(bytes, &sprite.tile);
}

fn write_underline(bytes: &mut Vec<u8>, underline: &Underline) {
    write_u32_vec(bytes, underline.order);
    write_u32_vec(bytes, underline.pad);
    write_bounds_scaled(bytes, &underline.bounds);
    write_bounds_scaled(bytes, &underline.content_mask.bounds);
    write_hsla(bytes, underline.color);
    write_f32_vec(bytes, underline.thickness.0);
    write_u32_vec(bytes, underline.wavy);
}

fn write_background(bytes: &mut Vec<u8>, background: &crate::Background) {
    write_u32_vec(bytes, background.tag as u32);
    write_u32_vec(bytes, background.color_space as u32);
    write_hsla(bytes, background.solid);
    write_f32_vec(bytes, background.gradient_angle_or_pattern_height);
    for stop in background.colors {
        write_hsla(bytes, stop.color);
        write_f32_vec(bytes, stop.percentage);
    }
    write_u32_vec(bytes, 0);
}

fn write_bounds_scaled(bytes: &mut Vec<u8>, bounds: &Bounds<crate::ScaledPixels>) {
    write_f32_vec(bytes, bounds.origin.x.0);
    write_f32_vec(bytes, bounds.origin.y.0);
    write_f32_vec(bytes, bounds.size.width.0);
    write_f32_vec(bytes, bounds.size.height.0);
}

fn write_bounds_device(bytes: &mut Vec<u8>, bounds: &Bounds<DevicePixels>) {
    write_i32_vec(bytes, bounds.origin.x.0);
    write_i32_vec(bytes, bounds.origin.y.0);
    write_i32_vec(bytes, bounds.size.width.0);
    write_i32_vec(bytes, bounds.size.height.0);
}

fn write_corners(bytes: &mut Vec<u8>, corners: &crate::Corners<crate::ScaledPixels>) {
    write_f32_vec(bytes, corners.top_left.0);
    write_f32_vec(bytes, corners.top_right.0);
    write_f32_vec(bytes, corners.bottom_right.0);
    write_f32_vec(bytes, corners.bottom_left.0);
}

fn write_edges(bytes: &mut Vec<u8>, edges: &crate::Edges<crate::ScaledPixels>) {
    write_f32_vec(bytes, edges.top.0);
    write_f32_vec(bytes, edges.right.0);
    write_f32_vec(bytes, edges.bottom.0);
    write_f32_vec(bytes, edges.left.0);
}

fn write_hsla(bytes: &mut Vec<u8>, color: crate::Hsla) {
    write_f32_vec(bytes, color.h);
    write_f32_vec(bytes, color.s);
    write_f32_vec(bytes, color.l);
    write_f32_vec(bytes, color.a);
}

fn write_atlas_tile(bytes: &mut Vec<u8>, tile: &AtlasTile) {
    write_u32_vec(bytes, tile.texture_id.index);
    write_u32_vec(bytes, tile.texture_id.kind as u32);
    write_u32_vec(bytes, tile.tile_id.0);
    write_u32_vec(bytes, tile.padding);
    write_bounds_device(bytes, &tile.bounds);
}

fn write_transformation(bytes: &mut Vec<u8>, transform: &crate::TransformationMatrix) {
    for row in transform.rotation_scale {
        for value in row {
            write_f32_vec(bytes, value);
        }
    }
    for value in transform.translation {
        write_f32_vec(bytes, value);
    }
}

fn write_u32_vec(bytes: &mut Vec<u8>, value: u32) {
    bytes.extend_from_slice(&value.to_ne_bytes());
}

fn write_i32_vec(bytes: &mut Vec<u8>, value: i32) {
    bytes.extend_from_slice(&value.to_ne_bytes());
}

fn write_f32_vec(bytes: &mut Vec<u8>, value: f32) {
    bytes.extend_from_slice(&value.to_ne_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        FontId, GlyphId, ImageId, RenderGlyphParams, RenderImageParams, RenderImagePixelFormat, px,
        size,
    };
    use std::cell::Cell;

    #[test]
    fn glyph_atlas_insert_update_remove() {
        let atlas = NovaAtlas::new();
        let key = AtlasKey::Glyph(RenderGlyphParams {
            font_id: FontId(1),
            glyph_id: GlyphId(2),
            font_size: px(14.0),
            subpixel_variant: Point { x: 0, y: 0 },
            scale_factor: 1.0,
            is_emoji: false,
            is_cjk: false,
            subpixel_rendering: false,
            dilation: 0,
        });
        let inserted = atlas
            .get_or_insert_with(&key, &mut || {
                Ok(Some((
                    size(DevicePixels(2), DevicePixels(2)),
                    Cow::Borrowed(&[255; 4]),
                )))
            })
            .expect("insert should succeed");
        assert!(inserted.is_some());
        assert_eq!(atlas.pending_upload_bytes_for_test()[3], 255);
        let updated = atlas
            .get_or_update_with(&key, &mut || {
                Ok(Some((
                    size(DevicePixels(4), DevicePixels(4)),
                    Cow::Borrowed(&[128; 16]),
                )))
            })
            .expect("update should succeed")
            .expect("update should return tile");
        assert_eq!(updated.bounds.size, size(DevicePixels(4), DevicePixels(4)));
        assert_eq!(atlas.pending_upload_count_for_test(), 2);
        atlas.remove(&key);
        let missing = atlas
            .get_or_insert_with(&key, &mut || Ok(None))
            .expect("lookup should succeed");
        assert!(missing.is_none());
    }

    #[test]
    fn glyph_atlas_resets_and_retries_when_full() {
        let atlas = NovaAtlas::new();
        let large_key = AtlasKey::Glyph(RenderGlyphParams {
            font_id: FontId(1),
            glyph_id: GlyphId(1),
            font_size: px(14.0),
            subpixel_variant: Point { x: 0, y: 0 },
            scale_factor: 1.0,
            is_emoji: false,
            is_cjk: false,
            subpixel_rendering: false,
            dilation: 0,
        });
        let large_size = size(
            DevicePixels(i32::try_from(NOVA_ATLAS_SIZE - 2).expect("atlas size fits i32")),
            DevicePixels(i32::try_from(NOVA_ATLAS_SIZE - 2).expect("atlas size fits i32")),
        );
        atlas
            .get_or_insert_with(&large_key, &mut || {
                Ok(Some((
                    large_size,
                    Cow::Owned(vec![
                        255;
                        large_size.width.0 as usize
                            * large_size.height.0 as usize
                    ]),
                )))
            })
            .expect("large glyph insert should not error")
            .expect("large glyph should fill atlas");

        let small_params = RenderGlyphParams {
            font_id: FontId(1),
            glyph_id: GlyphId(2),
            font_size: px(14.0),
            subpixel_variant: Point { x: 0, y: 0 },
            scale_factor: 1.0,
            is_emoji: false,
            is_cjk: false,
            subpixel_rendering: false,
            dilation: 0,
        };
        let tile = atlas
            .get_or_insert_glyph(&small_params, &mut || {
                Ok(GlyphRasterization::Bitmap {
                    size: size(DevicePixels(2), DevicePixels(2)),
                    bytes: vec![128; 4],
                })
            })
            .expect("retry should not error")
            .expect("small glyph should allocate after atlas reset");

        assert_eq!(
            tile.bounds.origin,
            Point {
                x: DevicePixels(1),
                y: DevicePixels(1)
            }
        );
    }

    #[test]
    fn monochrome_atlas_upload_uses_red_channel_coverage() {
        let mut pixels = [0_u8; 12];

        encode_rgba_upload(
            &mut pixels,
            size(DevicePixels(3), DevicePixels(1)),
            &[0, 128, 255],
            AtlasTextureKind::Monochrome,
        )
        .expect("monochrome upload should encode");

        assert_eq!(pixels, [0, 0, 0, 255, 128, 0, 0, 255, 255, 0, 0, 255]);
    }

    #[test]
    fn subpixel_atlas_fallback_upload_uses_grayscale_red_channel_coverage() {
        let mut pixels = [0_u8; 8];

        encode_rgba_upload(
            &mut pixels,
            size(DevicePixels(2), DevicePixels(1)),
            &[255, 0, 0, 255, 0, 255, 255, 128],
            AtlasTextureKind::Subpixel,
        )
        .expect("subpixel fallback upload should encode");

        assert_eq!(pixels, [85, 0, 0, 255, 85, 0, 0, 255]);
    }

    #[test]
    fn monochrome_atlas_upload_rejects_short_source_data() {
        let mut pixels = [0_u8; 8];

        let encoded = encode_rgba_upload(
            &mut pixels,
            size(DevicePixels(2), DevicePixels(1)),
            &[255],
            AtlasTextureKind::Monochrome,
        );

        assert!(encoded.is_none());
    }

    #[test]
    fn image_atlas_insert_returns_tile_for_rgba_and_bgra() {
        let atlas = NovaAtlas::new();
        let rgba_key = AtlasKey::Image(RenderImageParams {
            image_id: ImageId(1),
            frame_slot: 0,
            pixel_format: RenderImagePixelFormat::Rgba8,
        });
        let rgba_tile = atlas
            .get_or_insert_with(&rgba_key, &mut || {
                Ok(Some((
                    size(DevicePixels(1), DevicePixels(1)),
                    Cow::Borrowed(&[10, 20, 30, 40]),
                )))
            })
            .expect("rgba insert should succeed")
            .expect("rgba image should allocate a tile");
        assert_eq!(rgba_tile.texture_id.kind, AtlasTextureKind::Rgba);
        assert_eq!(rgba_tile.padding, NOVA_ATLAS_TILE_PADDING);
        assert_eq!(
            rgba_tile.bounds.origin,
            Point {
                x: DevicePixels(1),
                y: DevicePixels(1),
            }
        );
        assert_eq!(
            &atlas.pending_upload_bytes_for_test()[0..4],
            &[10, 20, 30, 40]
        );
        let rgba_center_offset = (NOVA_ATLAS_SIZE.min(3) as usize + 1) * NOVA_ATLAS_BYTES_PER_PIXEL;
        assert_eq!(
            &atlas.pending_upload_bytes_for_test()[rgba_center_offset..rgba_center_offset + 4],
            &[10, 20, 30, 40]
        );

        let bgra_key = AtlasKey::Image(RenderImageParams {
            image_id: ImageId(2),
            frame_slot: 0,
            pixel_format: RenderImagePixelFormat::Bgra8,
        });
        let bgra_tile = atlas
            .get_or_insert_with(&bgra_key, &mut || {
                Ok(Some((
                    size(DevicePixels(1), DevicePixels(1)),
                    Cow::Borrowed(&[1, 2, 3, 4]),
                )))
            })
            .expect("bgra insert should succeed")
            .expect("bgra image should allocate a tile");
        assert_eq!(bgra_tile.texture_id.kind, AtlasTextureKind::Bgra);

        let pixels = atlas.pending_upload_bytes_for_test();
        let padded_rgba_bytes = (rgba_tile.bounds.size.width.0.max(1) as usize
            + (rgba_tile.padding as usize * 2))
            * (rgba_tile.bounds.size.height.0.max(1) as usize + (rgba_tile.padding as usize * 2))
            * NOVA_ATLAS_BYTES_PER_PIXEL;
        let bgra_offset = padded_rgba_bytes;
        assert_eq!(&pixels[bgra_offset..bgra_offset + 4], &[3, 2, 1, 4]);
    }

    #[test]
    fn pending_atlas_upload_borrows_pixels_without_repeating_clean_upload() {
        let atlas = NovaAtlas::new();
        let uploads = Cell::new(0);
        let key = AtlasKey::Image(RenderImageParams {
            image_id: ImageId(3),
            frame_slot: 0,
            pixel_format: RenderImagePixelFormat::Rgba8,
        });
        let tile = atlas
            .get_or_insert_with(&key, &mut || {
                Ok(Some((
                    size(DevicePixels(2), DevicePixels(2)),
                    Cow::Borrowed(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]),
                )))
            })
            .expect("insert should succeed")
            .expect("image should allocate a tile");

        atlas
            .upload_pending_rgba_pixels(|origin, size, bytes_per_row, pixels| {
                assert_eq!(
                    origin.x,
                    tile.bounds.origin.x.0.saturating_sub(tile.padding as i32) as u32
                );
                assert_eq!(
                    origin.y,
                    tile.bounds.origin.y.0.saturating_sub(tile.padding as i32) as u32
                );
                assert_eq!(size.width(), 2 + tile.padding * 2);
                assert_eq!(size.height(), 2 + tile.padding * 2);
                assert_eq!(
                    bytes_per_row,
                    size.width() * NOVA_ATLAS_BYTES_PER_PIXEL as u32
                );
                assert_eq!(
                    pixels.len(),
                    bytes_per_row as usize * size.height() as usize
                );
                uploads.set(uploads.get() + 1);
                Ok(())
            })
            .expect("initial dirty upload should succeed");
        atlas
            .upload_pending_rgba_pixels(|_origin, _size, _bytes_per_row, _pixels| {
                uploads.set(uploads.get() + 1);
                Ok(())
            })
            .expect("clean upload should be skipped");

        assert_eq!(uploads.get(), 1);
    }

    #[test]
    fn quad_packer_matches_shader_storage_stride() {
        let mut bytes = Vec::new();

        write_quad(&mut bytes, &Quad::default());

        assert_eq!(bytes.len(), PACKED_QUAD_BYTES);
    }

    #[test]
    fn monochrome_sprite_packer_matches_shader_storage_stride() {
        let sprite = MonochromeSprite {
            order: 0,
            pad: 0,
            bounds: Bounds::default(),
            content_mask: Default::default(),
            color: Default::default(),
            tile: test_atlas_tile(),
            transformation: Default::default(),
        };
        let mut bytes = Vec::new();

        write_monochrome_sprite(&mut bytes, &sprite);

        assert_eq!(bytes.len(), PACKED_MONO_SPRITE_BYTES);
    }

    #[test]
    fn shadow_packer_matches_shader_storage_stride() {
        let mut bytes = Vec::new();

        write_shadow(
            &mut bytes,
            &Shadow {
                order: 0,
                blur_radius: crate::ScaledPixels(1.0),
                bounds: Bounds::default(),
                corner_radii: Default::default(),
                content_mask: Default::default(),
                color: Default::default(),
            },
        );

        assert_eq!(bytes.len(), PACKED_SHADOW_BYTES);
    }

    #[test]
    fn path_rasterization_vertex_packer_matches_shader_storage_stride() {
        let mut bytes = Vec::new();
        let vertex = crate::PathVertex_ScaledPixels {
            xy_position: Point {
                x: crate::ScaledPixels(1.0),
                y: crate::ScaledPixels(2.0),
            },
            st_position: Point { x: 0.25, y: 0.75 },
            content_mask: Default::default(),
        };

        write_path_rasterization_vertex(
            &mut bytes,
            &vertex,
            &crate::Background::default(),
            &Bounds::default(),
        );

        assert_eq!(bytes.len(), PACKED_PATH_RASTERIZATION_VERTEX_BYTES);
    }

    #[test]
    fn path_sprite_packer_matches_shader_storage_stride() {
        let mut bytes = Vec::new();

        write_path_sprite(&mut bytes, &Bounds::default());

        assert_eq!(bytes.len(), PACKED_PATH_SPRITE_BYTES);
    }

    #[test]
    fn polychrome_sprite_packer_matches_shader_storage_stride() {
        let sprite = PolychromeSprite {
            order: 0,
            pad: 0,
            grayscale: false,
            opacity: 1.0,
            bounds: Bounds::default(),
            content_mask: Default::default(),
            corner_radii: Default::default(),
            tile: test_atlas_tile(),
        };
        let mut bytes = Vec::new();

        write_polychrome_sprite(&mut bytes, &sprite);

        assert_eq!(bytes.len(), PACKED_POLY_SPRITE_BYTES);
    }

    #[test]
    fn underline_packer_matches_shader_storage_stride() {
        let mut bytes = Vec::new();

        write_underline(
            &mut bytes,
            &Underline {
                order: 0,
                pad: 0,
                bounds: Bounds::default(),
                content_mask: Default::default(),
                color: Default::default(),
                thickness: crate::ScaledPixels(1.0),
                wavy: 0,
            },
        );

        assert_eq!(bytes.len(), PACKED_UNDERLINE_BYTES);
    }

    #[test]
    fn unsupported_batch_summary_counts_each_advanced_batch_kind() {
        let summary = UnsupportedBatchSummary {
            paths: 1,
            surfaces: 2,
            backdrop_blurs: 3,
            backdrop_blur_tint_fallbacks: 4,
            gpu_meshes_3d: 5,
        };

        assert_eq!(summary.total(), 15);
    }

    #[test]
    fn frame_upload_globals_follow_surface_alpha_mode() {
        let scene = crate::Scene::default();
        let mut upload = NovaFrameUpload::default();
        let rendering_parameters = NovaRenderingParameters::from_env();
        let drawable_size = DrawableSize {
            width: 640,
            height: 480,
        };

        upload.encode(&scene, drawable_size, &rendering_parameters, false);
        assert_eq!(read_u32_at(&upload.globals, 8), 0);

        upload.encode(&scene, drawable_size, &rendering_parameters, true);
        assert_eq!(read_u32_at(&upload.globals, 8), 1);
    }

    #[test]
    fn transparent_window_uses_premultiplied_surface_alpha_like_wgpu() {
        let transparent = NovaSurfaceAlphaState::for_window_transparency(true);
        assert_eq!(
            transparent.swapchain_mode,
            CompositeAlphaMode::Premultiplied
        );
        assert!(transparent.outputs_premultiplied_alpha());

        let opaque = NovaSurfaceAlphaState::for_window_transparency(false);
        assert_eq!(opaque.swapchain_mode, CompositeAlphaMode::Opaque);
        assert!(!opaque.outputs_premultiplied_alpha());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn dx12_transparent_window_uses_premultiplied_swapchain_alpha() {
        let transparent = NovaRenderer::alpha_state_for_window_transparency_on_backend(
            RendererBackend::NovaDx12,
            true,
        );
        assert_eq!(
            transparent.swapchain_mode,
            CompositeAlphaMode::Premultiplied
        );
        assert_eq!(
            transparent.output_mode,
            NovaSurfaceOutputMode::Premultiplied
        );
        assert!(transparent.outputs_premultiplied_alpha());
    }

    #[test]
    fn auto_surface_alpha_uses_straight_output_like_wgpu() {
        let alpha = NovaSurfaceAlphaState::new(CompositeAlphaMode::Auto);
        assert_eq!(alpha.swapchain_mode, CompositeAlphaMode::Auto);
        assert_eq!(alpha.output_mode, NovaSurfaceOutputMode::Straight);
        assert!(!alpha.outputs_premultiplied_alpha());
    }

    #[test]
    fn backdrop_blur_encodes_real_batch_without_tint_fallback() {
        let mut scene = crate::Scene::default();
        let bounds = Bounds {
            origin: Point {
                x: crate::ScaledPixels(0.0),
                y: crate::ScaledPixels(0.0),
            },
            size: size(crate::ScaledPixels(64.0), crate::ScaledPixels(32.0)),
        };
        scene.insert_primitive(crate::PaintBackdropBlur {
            order: 0,
            bounds,
            content_mask: crate::ContentMask { bounds },
            corner_radii: Default::default(),
            radius: crate::ScaledPixels(12.0),
            downsample: 2,
            levels: 3,
            saturation: 1.0,
            tint: Some(crate::Hsla {
                h: 0.0,
                s: 0.0,
                l: 1.0,
                a: 0.5,
            }),
        });
        scene.finish();
        let mut upload = NovaFrameUpload::default();

        let summary = upload.encode(
            &scene,
            DrawableSize {
                width: 640,
                height: 480,
            },
            &NovaRenderingParameters::from_env(),
            true,
        );

        assert_eq!(summary.unsupported_batches.backdrop_blurs, 0);
        assert_eq!(summary.unsupported_batches.backdrop_blur_tint_fallbacks, 0);
        assert_eq!(summary.quad_count, 0);
        assert_eq!(upload.quads.len(), 0);
        assert_eq!(upload.backdrop_blur_passes.len(), BACKDROP_BLUR_PASS_BYTES);
        assert_eq!(upload.backdrop_blurs.len(), PACKED_BACKDROP_BLUR_BYTES);
        assert!(matches!(
            upload.batches.as_slice(),
            [NovaUploadedBatch::BackdropBlurs { first: 0, count: 1 }]
        ));
    }

    #[test]
    fn draw_steps_preserve_supported_batch_order_and_resources() {
        let mut upload = NovaFrameUpload::default();
        upload
            .batches
            .push(NovaUploadedBatch::SolidQuads { first: 0, count: 2 });
        upload
            .batches
            .push(NovaUploadedBatch::Quads { first: 2, count: 3 });
        upload
            .batches
            .push(NovaUploadedBatch::Shadows { first: 0, count: 6 });
        upload.batches.push(NovaUploadedBatch::PathRasterization {
            first_vertex: 9,
            vertex_count: 12,
        });
        upload
            .batches
            .push(NovaUploadedBatch::Paths { first: 1, count: 2 });
        upload
            .batches
            .push(NovaUploadedBatch::MonoSprites { first: 0, count: 4 });
        upload
            .batches
            .push(NovaUploadedBatch::PolySprites { first: 0, count: 5 });
        upload
            .batches
            .push(NovaUploadedBatch::Underlines { first: 0, count: 7 });
        upload
            .batches
            .push(NovaUploadedBatch::BackdropBlurs { first: 0, count: 1 });
        let pipelines = test_pipelines();
        let blend_pipelines = pipelines.alpha;
        let quad_set = test_resource_set_id(10);
        let shadow_set = test_resource_set_id(11);
        let path_set = test_resource_set_id(12);
        let mono_set = test_resource_set_id(13);
        let poly_set = test_resource_set_id(14);
        let underline_set = test_resource_set_id(15);
        let backdrop_blur_set = test_resource_set_id(16);

        let steps = draw_steps_for_upload(
            &upload,
            &pipelines,
            blend_pipelines,
            quad_set,
            shadow_set,
            path_set,
            mono_set,
            poly_set,
            underline_set,
            backdrop_blur_set,
            NovaDrawStepMode::Present,
        );

        assert_eq!(
            steps,
            vec![
                DrawStepDesc {
                    pipeline: blend_pipelines.solid_quads,
                    resource_sets: vec![quad_set],
                    vertex_count: 4,
                    first_vertex: 0,
                    instance_count: 2,
                    first_instance: 0,
                    scissor: None,
                },
                DrawStepDesc {
                    pipeline: blend_pipelines.quads,
                    resource_sets: vec![quad_set],
                    vertex_count: 4,
                    first_vertex: 0,
                    instance_count: 3,
                    first_instance: 2,
                    scissor: None,
                },
                DrawStepDesc {
                    pipeline: blend_pipelines.shadows,
                    resource_sets: vec![shadow_set],
                    vertex_count: 4,
                    first_vertex: 0,
                    instance_count: 6,
                    first_instance: 0,
                    scissor: None,
                },
                DrawStepDesc {
                    pipeline: pipelines.paths,
                    resource_sets: vec![path_set],
                    vertex_count: 4,
                    first_vertex: 0,
                    instance_count: 2,
                    first_instance: 1,
                    scissor: None,
                },
                DrawStepDesc {
                    pipeline: blend_pipelines.mono_sprites,
                    resource_sets: vec![mono_set],
                    vertex_count: 4,
                    first_vertex: 0,
                    instance_count: 4,
                    first_instance: 0,
                    scissor: None,
                },
                DrawStepDesc {
                    pipeline: blend_pipelines.poly_sprites,
                    resource_sets: vec![poly_set],
                    vertex_count: 4,
                    first_vertex: 0,
                    instance_count: 5,
                    first_instance: 0,
                    scissor: None,
                },
                DrawStepDesc {
                    pipeline: blend_pipelines.underlines,
                    resource_sets: vec![underline_set],
                    vertex_count: 4,
                    first_vertex: 0,
                    instance_count: 7,
                    first_instance: 0,
                    scissor: None,
                },
                DrawStepDesc {
                    pipeline: blend_pipelines.backdrop_blurs,
                    resource_sets: vec![backdrop_blur_set],
                    vertex_count: 4,
                    first_vertex: 0,
                    instance_count: 1,
                    first_instance: 0,
                    scissor: None,
                },
            ]
        );

        let mask_steps = path_mask_draw_steps_for_upload(&upload, &pipelines, path_set);
        assert_eq!(
            mask_steps,
            vec![DrawStepDesc {
                pipeline: pipelines.path_rasterization,
                resource_sets: vec![path_set],
                vertex_count: 12,
                first_vertex: 9,
                instance_count: 1,
                first_instance: 0,
                scissor: None,
            }]
        );
    }

    #[test]
    fn draw_steps_emit_zero_instance_clear_step_when_scene_is_empty() {
        let upload = NovaFrameUpload::default();
        let pipelines = test_pipelines();
        let blend_pipelines = pipelines.premultiplied;
        let quad_set = test_resource_set_id(10);

        let steps = draw_steps_for_upload(
            &upload,
            &pipelines,
            blend_pipelines,
            quad_set,
            test_resource_set_id(11),
            test_resource_set_id(12),
            test_resource_set_id(11),
            test_resource_set_id(12),
            test_resource_set_id(13),
            test_resource_set_id(14),
            NovaDrawStepMode::Present,
        );

        assert_eq!(
            steps,
            vec![DrawStepDesc {
                pipeline: blend_pipelines.solid_quads,
                resource_sets: vec![quad_set],
                vertex_count: 4,
                first_vertex: 0,
                instance_count: 0,
                first_instance: 0,
                scissor: None,
            }]
        );
    }

    #[test]
    fn backdrop_blur_source_steps_stop_at_first_blur_batch() {
        let mut upload = NovaFrameUpload::default();
        upload
            .batches
            .push(NovaUploadedBatch::Quads { first: 0, count: 1 });
        upload
            .batches
            .push(NovaUploadedBatch::BackdropBlurs { first: 0, count: 1 });
        upload
            .batches
            .push(NovaUploadedBatch::MonoSprites { first: 0, count: 1 });
        let pipelines = test_pipelines();
        let blend_pipelines = pipelines.alpha;
        let quad_set = test_resource_set_id(10);

        let steps = draw_steps_for_upload(
            &upload,
            &pipelines,
            blend_pipelines,
            quad_set,
            test_resource_set_id(11),
            test_resource_set_id(12),
            test_resource_set_id(13),
            test_resource_set_id(14),
            test_resource_set_id(15),
            test_resource_set_id(16),
            NovaDrawStepMode::BackdropSource,
        );

        assert_eq!(
            steps,
            vec![DrawStepDesc {
                pipeline: blend_pipelines.quads,
                resource_sets: vec![quad_set],
                vertex_count: 4,
                first_vertex: 0,
                instance_count: 1,
                first_instance: 0,
                scissor: None,
            }]
        );
    }

    #[test]
    fn present_draw_steps_continue_after_backdrop_blur_batch() {
        let mut upload = NovaFrameUpload::default();
        upload
            .batches
            .push(NovaUploadedBatch::Quads { first: 0, count: 1 });
        upload
            .batches
            .push(NovaUploadedBatch::BackdropBlurs { first: 0, count: 1 });
        upload
            .batches
            .push(NovaUploadedBatch::MonoSprites { first: 0, count: 1 });
        let pipelines = test_pipelines();
        let blend_pipelines = pipelines.alpha;

        let steps = draw_steps_for_upload(
            &upload,
            &pipelines,
            blend_pipelines,
            test_resource_set_id(10),
            test_resource_set_id(11),
            test_resource_set_id(12),
            test_resource_set_id(13),
            test_resource_set_id(14),
            test_resource_set_id(15),
            test_resource_set_id(16),
            NovaDrawStepMode::Present,
        );

        assert_eq!(
            steps.iter().map(|step| step.pipeline).collect::<Vec<_>>(),
            vec![
                blend_pipelines.quads,
                blend_pipelines.backdrop_blurs,
                blend_pipelines.mono_sprites
            ]
        );
    }

    #[test]
    fn partial_render_plan_produces_dirty_region_scissor() {
        let scene = crate::Scene::default();
        let mut dirty_region = crate::DirtyRegion::empty();
        dirty_region.push(crate::bounds(
            Point {
                x: crate::ScaledPixels(10.25),
                y: crate::ScaledPixels(20.75),
            },
            size(crate::ScaledPixels(30.1), crate::ScaledPixels(40.1)),
        ));
        dirty_region.push(crate::bounds(
            Point {
                x: crate::ScaledPixels(60.0),
                y: crate::ScaledPixels(70.0),
            },
            size(crate::ScaledPixels(10.0), crate::ScaledPixels(10.0)),
        ));
        let render_plan = FrameRenderPlan {
            scene: &scene,
            dirty_region: &dirty_region,
            partial_present_mode: PartialPresentMode::Partial,
            trim_policy: Default::default(),
        };

        assert_eq!(
            partial_scissor_for_plan(
                render_plan,
                DrawableSize {
                    width: 100,
                    height: 100,
                },
            ),
            Some(ScissorRect {
                x: 10,
                y: 20,
                width: 60,
                height: 60,
            })
        );
    }

    #[test]
    fn full_render_plan_does_not_produce_scissor() {
        let scene = crate::Scene::default();
        let dirty_region = crate::DirtyRegion::full(crate::bounds(
            Point {
                x: crate::ScaledPixels(0.0),
                y: crate::ScaledPixels(0.0),
            },
            size(crate::ScaledPixels(100.0), crate::ScaledPixels(100.0)),
        ));
        let render_plan = FrameRenderPlan::full_redraw(&scene, &dirty_region);

        assert_eq!(
            partial_scissor_for_plan(
                render_plan,
                DrawableSize {
                    width: 100,
                    height: 100,
                },
            ),
            None
        );
    }

    #[test]
    fn backdrop_blur_render_passes_downsample_then_upsample_levels() {
        let pipelines = test_pipelines();
        let targets = NovaBackdropBlurTargets {
            downsample: 2,
            source: NovaTextureTarget {
                texture: test_texture_id(1),
                texture_view: test_texture_view_id(1),
            },
            levels: vec![
                NovaBackdropBlurLevelTarget {
                    texture: test_texture_id(2),
                    texture_view: test_texture_view_id(2),
                    pass_resource_set: test_resource_set_id(12),
                },
                NovaBackdropBlurLevelTarget {
                    texture: test_texture_id(3),
                    texture_view: test_texture_view_id(3),
                    pass_resource_set: test_resource_set_id(13),
                },
                NovaBackdropBlurLevelTarget {
                    texture: test_texture_id(4),
                    texture_view: test_texture_view_id(4),
                    pass_resource_set: test_resource_set_id(14),
                },
            ],
            source_pass_resource_set: test_resource_set_id(11),
            target_resource_set: test_resource_set_id(15),
        };

        let passes = backdrop_blur_render_passes_for_targets(&pipelines, &targets, 3);

        assert_eq!(passes.len(), 5);
        assert_eq!(
            passes
                .iter()
                .map(|pass| pass.target_texture_view)
                .collect::<Vec<_>>(),
            vec![
                test_texture_view_id(2),
                test_texture_view_id(3),
                test_texture_view_id(4),
                test_texture_view_id(3),
                test_texture_view_id(2),
            ]
        );
        assert_eq!(
            passes
                .iter()
                .flat_map(|pass| pass.steps.iter().map(|step| step.pipeline))
                .collect::<Vec<_>>(),
            vec![
                pipelines.backdrop_blur_downsample,
                pipelines.backdrop_blur_downsample,
                pipelines.backdrop_blur_downsample,
                pipelines.backdrop_blur_upsample,
                pipelines.backdrop_blur_upsample,
            ]
        );
        assert_eq!(
            passes
                .iter()
                .flat_map(|pass| pass.steps.iter())
                .map(|step| step.resource_sets.as_slice())
                .collect::<Vec<_>>(),
            vec![
                [test_resource_set_id(11)].as_slice(),
                [test_resource_set_id(12)].as_slice(),
                [test_resource_set_id(13)].as_slice(),
                [test_resource_set_id(14)].as_slice(),
                [test_resource_set_id(13)].as_slice(),
            ]
        );
    }

    #[test]
    fn nova_production_shader_entries_compile_for_enabled_backends() {
        #[cfg(all(
            feature = "nova-gfx-vulkan",
            any(target_os = "windows", target_os = "linux", target_os = "freebsd")
        ))]
        compile_nova_shader_binaries(compile_wgsl_to_spirv)
            .expect("nova production shaders should compile to SPIR-V");

        #[cfg(target_os = "windows")]
        compile_nova_shader_binaries(compile_wgsl_to_hlsl)
            .expect("nova production shaders should compile to HLSL");

        #[cfg(target_os = "macos")]
        compile_nova_shader_binaries(compile_wgsl_to_msl)
            .expect("nova production shaders should compile to MSL");
    }

    #[test]
    fn nova_optional_shader_entries_compile_for_enabled_backends() {
        fn compile_optional_entries(
            mut compile: impl FnMut(
                &str,
                ShaderStage,
                &str,
            ) -> std::result::Result<
                gfx_core::ShaderBinary,
                gfx_shader::ShaderError,
            >,
            target: &str,
        ) {
            let entries = [
                (
                    NOVA_SURFACE_SHADER_SOURCE,
                    ShaderStage::Vertex,
                    "vs_surface",
                ),
                (
                    NOVA_SURFACE_SHADER_SOURCE,
                    ShaderStage::Fragment,
                    "fs_surface",
                ),
                (
                    NOVA_MESH_3D_SHADER_SOURCE,
                    ShaderStage::Vertex,
                    "vs_gpu_mesh_3d",
                ),
                (
                    NOVA_MESH_3D_SHADER_SOURCE,
                    ShaderStage::Fragment,
                    "fs_gpu_mesh_3d",
                ),
                (
                    NOVA_MESH_3D_SHADER_SOURCE,
                    ShaderStage::Vertex,
                    "vs_gpu_mesh_3d_composite",
                ),
                (
                    NOVA_MESH_3D_SHADER_SOURCE,
                    ShaderStage::Fragment,
                    "fs_gpu_mesh_3d_composite",
                ),
            ];

            for (source, stage, entry_point) in entries {
                compile(source, stage, entry_point).unwrap_or_else(|error| {
                    panic!("nova optional shader entry {entry_point} should compile to {target}: {error}");
                });
            }
        }

        #[cfg(all(
            feature = "nova-gfx-vulkan",
            any(target_os = "windows", target_os = "linux", target_os = "freebsd")
        ))]
        compile_optional_entries(compile_wgsl_to_spirv, "SPIR-V");

        #[cfg(target_os = "windows")]
        compile_optional_entries(compile_wgsl_to_hlsl, "HLSL");

        #[cfg(target_os = "macos")]
        compile_optional_entries(compile_wgsl_to_msl, "MSL");
    }

    #[test]
    fn nova_shaders_do_not_guard_division_with_select() {
        for (shader_name, source) in nova_shader_sources() {
            let source_without_comments = source
                .lines()
                .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
                .collect::<Vec<_>>()
                .join("\n");

            for statement in source_without_comments.split(';') {
                let compact_statement = statement.split_whitespace().collect::<String>();
                assert!(
                    !select_arguments_contain_division(&compact_statement),
                    "{shader_name} contains a select() guarded division; use an explicit branch instead: {statement}"
                );
            }
        }
    }

    fn select_arguments_contain_division(statement: &str) -> bool {
        let mut search_start = 0;
        while let Some(relative_start) = statement[search_start..].find("select(") {
            let select_start = search_start + relative_start;
            let mut depth = 0_i32;
            for (offset, character) in statement[select_start..].char_indices() {
                match character {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            let select_expression = &statement[select_start..select_start + offset];
                            if select_expression.contains('/') {
                                return true;
                            }
                            search_start = select_start + offset + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }

            if search_start == select_start {
                return false;
            }
        }

        false
    }

    #[test]
    fn nova_shaders_use_explicit_lod_for_texture_sampling() {
        for (shader_name, source) in nova_shader_sources() {
            assert!(
                !source.contains("textureSample("),
                "{shader_name} uses implicit texture sampling; use textureSampleLevel(..., 0.0)"
            );
        }
    }

    #[test]
    fn nova_fragment_shaders_clip_before_expensive_fragment_work() {
        for (shader_name, source) in nova_shader_sources() {
            let mut search_start = 0;
            while let Some(relative_start) = source[search_start..].find("@fragment") {
                let fragment_start = search_start + relative_start;
                let function_start = source[fragment_start..]
                    .find("fn ")
                    .map(|offset| fragment_start + offset)
                    .unwrap_or_else(|| panic!("{shader_name} fragment entry is missing fn"));
                let function_name_start = function_start + "fn ".len();
                let function_name_end = source[function_name_start..]
                    .find('(')
                    .map(|offset| function_name_start + offset)
                    .unwrap_or_else(|| {
                        panic!("{shader_name} fragment entry is missing argument list")
                    });
                let function_name = &source[function_name_start..function_name_end];
                let function_source = shader_function_source(shader_name, source, function_name);

                if let Some(clip_offset) =
                    function_source.find("if (any(input.clip_distances < vec4<f32>(0.0)))")
                {
                    for expensive_fragment in [
                        "dpdx(",
                        "dpdy(",
                        "textureSampleLevel(",
                        "sample_backdrop_blur_texture(",
                    ] {
                        if let Some(expensive_offset) = function_source.find(expensive_fragment) {
                            assert!(
                                clip_offset < expensive_offset,
                                "{shader_name}::{function_name} should clip before {expensive_fragment}"
                            );
                        }
                    }
                    for storage_buffer in storage_buffers_declared_by_shader(source) {
                        let storage_read = format!("{storage_buffer}[");
                        if let Some(storage_offset) = function_source.find(&storage_read) {
                            assert!(
                                clip_offset < storage_offset,
                                "{shader_name}::{function_name} should clip before reading {storage_buffer}"
                            );
                        }
                    }
                }

                search_start = function_start + function_source.len();
            }
        }
    }

    #[test]
    fn nova_fragment_shaders_skip_work_before_sampling_transparent_pixels() {
        assert_fragment_contains_in_order(
            "mono_sprite.wgsl",
            include_str!("nova/shaders/mono_sprite.wgsl"),
            "fs_mono_sprite",
            &[
                "if (any(input.clip_distances < vec4<f32>(0.0)))",
                "if (input.color.a <= 0.0)",
                "let sample = textureSampleLevel(t_sprite, s_sprite, input.tile_position, 0.0).r",
                "if (sample <= 0.0)",
                "let alpha_corrected = apply_contrast_and_gamma_correction(",
            ],
        );
        assert_fragment_contains_in_order(
            "poly_sprite.wgsl",
            include_str!("nova/shaders/poly_sprite.wgsl"),
            "fs_poly_sprite",
            &[
                "if (any(input.clip_distances < vec4<f32>(0.0)))",
                "if (input.opacity <= 0.0)",
                "quad_sdf_from_packed(input.position.xy, input.bounds, input.corner_radii)",
                "let coverage = saturate(SDF_ANTIALIAS_THRESHOLD - distance)",
                "if (coverage <= 0.0)",
                "let sample = textureSampleLevel(t_sprite, s_sprite, input.tile_position, 0.0)",
                "if (sample.a <= 0.0)",
                "let grayscale = dot(sample.rgb, GRAYSCALE_FACTORS)",
            ],
        );
        assert_fragment_contains_in_order(
            "backdrop_blur.wgsl",
            include_str!("nova/shaders/backdrop_blur.wgsl"),
            "fs_backdrop_blur",
            &[
                "if (any(input.clip_distances < vec4<f32>(0.0)))",
                "let alpha = saturate(SDF_ANTIALIAS_THRESHOLD - distance)",
                "if (alpha <= 0.0)",
                "var color = sample_backdrop_blur_texture(input.texture_coords)",
                "if (color.a <= 0.0 && input.tint.a <= 0.0)",
                "if (input.saturation != 1.0)",
                "color = vec4<f32>(saturate_color(color.rgb, input.saturation), color.a)",
            ],
        );
    }

    #[test]
    fn nova_fragment_shaders_skip_fully_transparent_instances() {
        assert_fragment_contains_in_order(
            "solid_quad.wgsl",
            include_str!("nova/shaders/solid_quad.wgsl"),
            "fs_solid_quad",
            &[
                "if (any(input.clip_distances < vec4<f32>(0.0)))",
                "if (input.color.a <= 0.0)",
                "return blend_color(input.color, 1.0)",
            ],
        );
        assert_fragment_contains_in_order(
            "quad.wgsl",
            include_str!("nova/shaders/quad.wgsl"),
            "fs_quad",
            &[
                "if (input.background_tag == 0u &&",
                "input.background_solid.a <= 0.0 &&",
                "input.border_color.a <= 0.0)",
                "let quad = b_quads[input.quad_id]",
                "var background_color = input.background_solid",
                "if (background_color.a <= 0.0 && input.border_color.a <= 0.0)",
            ],
        );
        assert_fragment_contains_in_order(
            "path.wgsl",
            include_str!("nova/shaders/path.wgsl"),
            "fs_path_rasterization",
            &[
                "var color = input.background_solid",
                "if (input.background_tag == 0u && color.a <= 0.0)",
                "if (input.background_tag != 0u)",
                "if (color.a <= 0.0)",
                "return vec4<f32>(color.rgb * color.a * alpha, color.a * alpha)",
            ],
        );
        assert_fragment_contains_in_order(
            "shadow.wgsl",
            include_str!("nova/shaders/shadow.wgsl"),
            "fs_shadow",
            &[
                "if (any(input.clip_distances < vec4<f32>(0.0)))",
                "if (input.color.a <= 0.0)",
                "if (input.blur_radius <= SHADER_EPSILON)",
                "let inverse_sigma = 1.0 / input.blur_radius",
            ],
        );
        assert_fragment_contains_in_order(
            "underline.wgsl",
            include_str!("nova/shaders/underline.wgsl"),
            "fs_underline",
            &[
                "if (any(input.clip_distances < vec4<f32>(0.0)))",
                "if (input.color.a <= 0.0)",
                "let underline_height = input.bounds.w",
                "if ((input.wavy & 0xFFu) == 0u)",
                "return blend_color(input.color, 1.0)",
            ],
        );
        assert_fragment_contains_in_order(
            "mesh_3d.wgsl",
            include_str!("nova/shaders/mesh_3d.wgsl"),
            "fs_gpu_mesh_3d",
            &[
                "if (any(input.clip_distances < vec4<f32>(0.0)))",
                "if (input.color.a <= 0.0)",
                "let edge_alpha = clamp(",
                "let alpha = input.color.a * edge_alpha",
            ],
        );
    }

    #[test]
    fn nova_shader_discard_usage_matches_clip_strategy() {
        assert_shader_contains(
            "core.wgsl",
            include_str!("nova/shaders/core.wgsl"),
            &[
                "Most Nova shaders pass software clip distances to the fragment stage",
                "return transparent outside the clip",
                "Mesh 3D uses `discard` for",
            ],
        );

        for (shader_name, source) in nova_shader_sources() {
            if shader_name == "mesh_3d.wgsl" {
                assert!(
                    source.contains("discard;"),
                    "mesh_3d.wgsl should document the reviewed discard exception in code"
                );
            } else {
                assert!(
                    !source.contains("discard;"),
                    "{shader_name} should return transparent for software clipping instead of discarding"
                );
            }
        }
    }

    #[test]
    fn nova_underline_alpha_is_applied_once() {
        let source = include_str!("nova/shaders/underline.wgsl");
        assert_fragment_contains_in_order(
            "underline.wgsl",
            source,
            "fs_underline",
            &[
                "if ((input.wavy & 0xFFu) == 0u)",
                "return blend_color(input.color, 1.0)",
                "let alpha = saturate(SDF_ANTIALIAS_THRESHOLD - stroke_distance)",
                "return blend_color(input.color, alpha)",
            ],
        );
        assert_fragment_function_omits("underline.wgsl", source, "fs_underline", "input.color.a)");
    }

    #[test]
    fn nova_quad_struct_definitions_stay_in_sync() {
        let quad_struct =
            shader_struct_source("quad.wgsl", include_str!("nova/shaders/quad.wgsl"), "Quad");
        let solid_quad_struct = shader_struct_source(
            "solid_quad.wgsl",
            include_str!("nova/shaders/solid_quad.wgsl"),
            "Quad",
        );

        assert_eq!(
            solid_quad_struct, quad_struct,
            "solid_quad.wgsl and quad.wgsl must agree on the packed Quad buffer layout"
        );
        assert_shader_contains(
            "quad.wgsl",
            include_str!("nova/shaders/quad.wgsl"),
            &["Keep in sync with solid_quad.wgsl"],
        );
        assert_shader_contains(
            "solid_quad.wgsl",
            include_str!("nova/shaders/solid_quad.wgsl"),
            &["Keep in sync with quad.wgsl"],
        );
    }

    #[test]
    fn nova_sdf_and_dash_thresholds_are_shared_constants() {
        assert_shader_contains(
            "core.wgsl",
            include_str!("nova/shaders/core.wgsl"),
            &["const SDF_ANTIALIAS_THRESHOLD: f32 = 0.5"],
        );
        assert_shader_contains(
            "quad.wgsl",
            include_str!("nova/shaders/quad.wgsl"),
            &[
                "const DASH_PERIOD_PER_WIDTH: f32 = DASH_LENGTH_PER_WIDTH + DASH_GAP_PER_WIDTH",
                "const DASH_LENGTH: f32 = DASH_LENGTH_PER_WIDTH / DASH_PERIOD_PER_WIDTH",
                "const DASH_VELOCITY_NUMERATOR: f32 = 1.0 / DASH_PERIOD_PER_WIDTH",
            ],
        );

        for (shader_name, source) in nova_shader_sources() {
            for forbidden_fragment in [
                "saturate(0.5 -",
                "let antialias_threshold = 0.5",
                "dash_period_per_width",
                "desired_dash_gap",
            ] {
                assert!(
                    !source.contains(forbidden_fragment),
                    "{shader_name} should use shared shader constants instead of {forbidden_fragment}"
                );
            }
        }
    }

    #[test]
    fn nova_shader_divisions_are_guarded_or_constant() {
        const ALLOWED_DIVISIONS: &[&str] = &[
            "/ vec3<f32>(1.055)",
            "/ vec3<f32>(12.92)",
            "/ 1.055",
            "/ 12.92",
            "1.0 / 2.4",
            "1.0 / 3.0",
            "* M_PI_F / 180.0",
            "safe_size.y / safe_size.x",
            "safe_size.x / safe_size.y",
            "/ max(length(direction), SHADER_EPSILON)",
            "/ max(length(scaled_direction), SHADER_EPSILON)",
            "/ safe_size.x",
            "/ safe_size.y",
            "/ stop_range",
            "/ 65535.0f",
            "/ 255.0f",
            "M_PI_F / 4.0",
            "/ max(length(gradient), SHADER_EPSILON)",
            "pattern_width / pattern_height",
            "1.0 / source_size",
            "/ max(blur.blurred_size, vec2<f32>(1.0))",
            "/ 2.0",
            "1.0 / input.blur_radius",
            "/ sqrt(2.0 * M_PI_F)",
            "/ (r2 * r2)",
            "/ 4.0",
            "1.0 / DASH_PERIOD_PER_WIDTH",
            "/ DASH_PERIOD_PER_WIDTH",
            "/ dash_count",
            "/ safe_border_width",
            "/ 2",
            "/ 2.0)",
            "/ dash_velocity",
            "/ safe_radii",
            "/ b)",
            "/ viewport_size",
            "/ atlas_size",
            "/ alpha",
            "/ max(clip_position.w, 0.0001)",
            "/ max(globals.viewport_size, vec2<f32>(1.0))",
            "/ max(safe_alpha * safe_k + 1.0, SHADER_EPSILON)",
            "/ underline_height",
            "/ sqrt(1.0 + dSine * dSine)",
        ];

        for (shader_name, source) in nova_shader_sources() {
            for (line_number, line) in source.lines().enumerate() {
                let code = line.split_once("//").map_or(line, |(code, _)| code);
                if code.contains('/') {
                    assert!(
                        ALLOWED_DIVISIONS
                            .iter()
                            .any(|division| code.contains(division)),
                        "{shader_name}:{} uses division without an explicit safety review: {line}",
                        line_number + 1
                    );
                }
            }
        }
    }

    #[test]
    fn nova_shader_modulo_uses_have_nonzero_divisors() {
        const ALLOWED_MODULO_DIVISORS: [&str; 4] =
            ["% 2.0", "% 360.0", "% 65535.0f", "% pattern_period"];

        assert_shader_contains(
            "quad_common.wgsl",
            include_str!("nova/shaders/quad_common.wgsl"),
            &[
                "if (pattern_width <= SHADER_EPSILON || pattern_height <= SHADER_EPSILON || pattern_period <= SHADER_EPSILON)",
                "rotated_point.x % pattern_period",
            ],
        );

        for (shader_name, source) in nova_shader_sources() {
            for (line_number, line) in source.lines().enumerate() {
                let code = line.split_once("//").map_or(line, |(code, _)| code);
                if code.contains('%') {
                    assert!(
                        ALLOWED_MODULO_DIVISORS
                            .iter()
                            .any(|divisor| code.contains(divisor)),
                        "{shader_name}:{} uses modulo without a known nonzero divisor: {line}",
                        line_number + 1
                    );
                }
            }
        }
    }

    #[test]
    fn nova_shader_edge_guards_cover_degenerate_inputs() {
        assert_shader_contains(
            "core.wgsl",
            include_str!("nova/shaders/core.wgsl"),
            &[
                "const SHADER_EPSILON",
                "const SDF_ANTIALIAS_THRESHOLD",
                "let viewport_size = max(globals.viewport_size, vec2<f32>(1.0))",
                "fn viewport_texture_coords",
                "let atlas_size = max(vec2<f32>(textureDimensions(t_sprite, 0)), vec2<f32>(1.0))",
                "fn over",
                "if (alpha <= SHADER_EPSILON)",
                "return vec4<f32>(0.0)",
            ],
        );
        assert_shader_contains(
            "quad.wgsl",
            include_str!("nova/shaders/quad.wgsl"),
            &[
                "dash_velocity_for_border_width(DASH_VELOCITY_NUMERATOR, border_width)",
                "fn dash_velocity_for_border_width",
                "let safe_border_width = max(border_width, SHADER_EPSILON)",
                "let velocity = dv_numerator / safe_border_width",
                "return select(0.0, velocity, has_border)",
                "const DASH_PERIOD_PER_WIDTH",
                "const DASH_LENGTH",
                "const DASH_VELOCITY_NUMERATOR",
                "let antialias_threshold = SDF_ANTIALIAS_THRESHOLD",
                "let dv1_or_min = select(min_nonzero_velocity, dv1, dv2 == 0.0)",
                "if (dash_velocity <= SHADER_EPSILON || period <= SHADER_EPSILON || length <= SHADER_EPSILON)",
                "let safe_radii = max(radii, vec2<f32>(SHADER_EPSILON))",
                "let outer_alpha = saturate(antialias_threshold - outer_sdf)",
                "if (outer_alpha <= 0.0)",
                "return blend_color(color, outer_alpha)",
            ],
        );
        assert_shader_contains(
            "quad_common.wgsl",
            include_str!("nova/shaders/quad_common.wgsl"),
            &[
                "let safe_size = max(bounds.size, vec2<f32>(SHADER_EPSILON))",
                "let x_over_y = safe_size.x / safe_size.y",
                "let y_over_x = safe_size.y / safe_size.x",
                "let scaled_direction = vec2<f32>",
                "if (abs(stop_range) <= SHADER_EPSILON)",
                "if (pattern_width <= SHADER_EPSILON || pattern_height <= SHADER_EPSILON || pattern_period <= SHADER_EPSILON)",
                "background_color.a *= saturate(SDF_ANTIALIAS_THRESHOLD - distance)",
            ],
        );
        assert_shader_contains(
            "shadow.wgsl",
            include_str!("nova/shaders/shadow.wgsl"),
            &[
                "if (input.blur_radius <= SHADER_EPSILON)",
                "pick_corner_radius_from_packed(center_to_point, input.corner_radii)",
                "if (end <= start)",
                "let inverse_sigma = 1.0 / input.blur_radius",
                "let gaussian_scale = inverse_sigma / sqrt(2.0 * M_PI_F)",
                "let gaussian_exponent_scale = 0.5 * inverse_sigma * inverse_sigma",
                "gaussian_weight(y, gaussian_scale, gaussian_exponent_scale)",
            ],
        );
        assert_fragment_function_omits(
            "shadow.wgsl",
            include_str!("nova/shaders/shadow.wgsl"),
            "fs_shadow",
            "gaussian(y, input.blur_radius)",
        );
        assert_shader_contains(
            "underline.wgsl",
            include_str!("nova/shaders/underline.wgsl"),
            &[
                "if (underline_height <= SHADER_EPSILON || input.thickness <= SHADER_EPSILON)",
                "let stroke_distance = max(-distance_from_bottom_border, distance_from_top_border)",
                "let alpha = saturate(SDF_ANTIALIAS_THRESHOLD - stroke_distance)",
            ],
        );
        assert_shader_contains(
            "backdrop_blur.wgsl",
            include_str!("nova/shaders/backdrop_blur.wgsl"),
            &[
                "let source_size = max(vec2<f32>(textureDimensions(t_sprite, 0)), vec2<f32>(1.0))",
                "screen_position / max(blur.blurred_size, vec2<f32>(1.0))",
                "quad_sdf_from_packed(input.position.xy, input.bounds, input.corner_radii)",
                "let alpha = saturate(SDF_ANTIALIAS_THRESHOLD - distance)",
                "if (alpha <= 0.0)",
                "textureSampleLevel(t_sprite, s_sprite, texture_coords, 0.0)",
            ],
        );
        assert_shader_contains(
            "path.wgsl",
            include_str!("nova/shaders/path.wgsl"),
            &[
                "let edge_gradient = vec2<f32>(dx.x, dy.x)",
                "if (length(edge_gradient) < 0.001)",
                "let distance = f / max(length(gradient), SHADER_EPSILON)",
                "alpha = saturate(SDF_ANTIALIAS_THRESHOLD - distance)",
                "if (alpha <= 0.0)",
                "let texture_coords = viewport_texture_coords(screen_position)",
            ],
        );
        assert_shader_contains(
            "mesh_3d.wgsl",
            include_str!("nova/shaders/mesh_3d.wgsl"),
            &["out.texture_coords = viewport_texture_coords(position)"],
        );
        assert_shader_contains(
            "text.wgsl",
            include_str!("nova/shaders/text.wgsl"),
            &[
                "let safe_alpha = saturate(alpha)",
                "return safe_alpha * (safe_k + 1.0) / max(safe_alpha * safe_k + 1.0, SHADER_EPSILON)",
            ],
        );
    }

    #[test]
    fn nova_fragment_shaders_avoid_redundant_instance_ssbo_reads() {
        // `fs_quad` still reads `b_quads` because it needs the full quad shape,
        // border, and background metadata. Small preconverted colors are already
        // passed as flat varyings to keep this reviewed exception narrow.
        let allowed_fragment_reads = [("quad.wgsl", "fs_quad", "b_quads")];
        assert_fragment_storage_reads_are_allowlisted(&allowed_fragment_reads);

        assert_fragment_function_omits(
            "backdrop_blur.wgsl",
            include_str!("nova/shaders/backdrop_blur.wgsl"),
            "fs_backdrop_blur",
            "b_backdrop_blurs",
        );
        assert_fragment_function_omits(
            "path.wgsl",
            include_str!("nova/shaders/path.wgsl"),
            "fs_path_rasterization",
            "b_path_vertices",
        );
        assert_fragment_function_omits(
            "poly_sprite.wgsl",
            include_str!("nova/shaders/poly_sprite.wgsl"),
            "fs_poly_sprite",
            "b_poly_sprites",
        );
        assert_fragment_function_omits(
            "shadow.wgsl",
            include_str!("nova/shaders/shadow.wgsl"),
            "fs_shadow",
            "b_shadows",
        );
        assert_fragment_function_omits(
            "underline.wgsl",
            include_str!("nova/shaders/underline.wgsl"),
            "fs_underline",
            "b_underlines",
        );
    }

    fn assert_fragment_storage_reads_are_allowlisted(allowed_reads: &[(&str, &str, &str)]) {
        for (shader_name, source) in nova_shader_sources() {
            let mut search_start = 0;
            while let Some(relative_start) = source[search_start..].find("@fragment") {
                let fragment_start = search_start + relative_start;
                let function_start = source[fragment_start..]
                    .find("fn ")
                    .map(|offset| fragment_start + offset)
                    .unwrap_or_else(|| panic!("{shader_name} fragment entry is missing fn"));
                let function_name_start = function_start + "fn ".len();
                let function_name_end = source[function_name_start..]
                    .find('(')
                    .map(|offset| function_name_start + offset)
                    .unwrap_or_else(|| {
                        panic!("{shader_name} fragment entry is missing argument list")
                    });
                let function_name = &source[function_name_start..function_name_end];
                let function_end = source[function_start + 1..]
                    .find("\n@")
                    .map(|offset| function_start + 1 + offset)
                    .unwrap_or(source.len());
                let function_source = &source[function_start..function_end];

                for storage_buffer in storage_buffers_declared_by_shader(source) {
                    if function_source.contains(&format!("{storage_buffer}[")) {
                        assert!(
                            allowed_reads.iter().any(
                                |(allowed_shader, allowed_function, allowed_buffer)| {
                                    *allowed_shader == shader_name
                                        && *allowed_function == function_name
                                        && *allowed_buffer == storage_buffer
                                }
                            ),
                            "{shader_name}::{function_name} reads {storage_buffer} in the fragment stage; pass small per-instance fields through flat varyings or add a reviewed exception"
                        );
                    }
                }

                search_start = function_end;
            }
        }
    }

    fn storage_buffers_declared_by_shader(source: &str) -> Vec<&str> {
        source
            .lines()
            .filter_map(|line| {
                let code = line.split_once("//").map_or(line, |(code, _)| code);
                if !code.contains("var<storage") {
                    return None;
                }
                code.split_once("var<storage")
                    .and_then(|(_, rest)| rest.split_once('>'))
                    .and_then(|(_, rest)| rest.trim_start().split_once(':'))
                    .map(|(name, _)| name.trim())
            })
            .collect()
    }

    #[test]
    fn nova_quad_has_solid_background_fast_path() {
        assert_fragment_contains_in_order(
            "quad.wgsl",
            include_str!("nova/shaders/quad.wgsl"),
            "fs_quad",
            &[
                "var background_color = input.background_solid",
                "if (input.background_tag != 0u)",
                "background_color = gradient_color(",
            ],
        );
        assert_shader_contains(
            "quad.wgsl",
            include_str!("nova/shaders/quad.wgsl"),
            &[
                "@location(6) @interpolate(flat) background_tag: u32",
                "out.background_solid = gradient.solid",
                "out.background_color0 = gradient.color0",
                "out.background_color1 = gradient.color1",
                "out.background_tag = quad.background.tag",
            ],
        );
    }

    #[test]
    fn nova_path_rasterization_has_solid_background_fast_path() {
        assert_shader_contains(
            "path.wgsl",
            include_str!("nova/shaders/path.wgsl"),
            &[
                "let prepared_color = prepare_gradient_color(",
                "var color = input.background_solid",
                "if (input.background_tag != 0u)",
            ],
        );
        assert_fragment_contains_in_order(
            "path.wgsl",
            include_str!("nova/shaders/path.wgsl"),
            "fs_path_rasterization",
            &[
                "alpha = saturate(SDF_ANTIALIAS_THRESHOLD - distance)",
                "if (alpha <= 0.0)",
                "var color = input.background_solid",
                "if (input.background_tag != 0u)",
                "let background = Background(",
            ],
        );
        assert_fragment_function_omits(
            "path.wgsl",
            include_str!("nova/shaders/path.wgsl"),
            "fs_path_rasterization",
            "prepare_gradient_color(",
        );
    }

    #[test]
    fn nova_vertex_only_storage_buffers_are_not_visible_to_fragment_stage() {
        let renderer_source = include_str!("nova_renderer.rs");
        for binding in [2_u32, 3, 6, 7, 8, 9, 15, 16] {
            let binding_marker = format!("binding: {binding},");
            let entry_source = renderer_source
                .match_indices(&binding_marker)
                .filter_map(|(binding_start, _)| {
                    let binding_source = &renderer_source[binding_start..];
                    let entry_end = binding_source
                        .find("},")
                        .map_or(binding_source.len(), |end| end + 2);
                    let entry_source = &binding_source[..entry_end];

                    entry_source
                        .contains("ResourceBindingType::StorageBuffer")
                        .then_some(entry_source)
                })
                .next()
                .unwrap_or_else(|| {
                    panic!("nova_renderer.rs is missing storage buffer layout binding {binding}")
                });

            assert!(
                entry_source.contains("stages: ShaderStages::VERTEX"),
                "binding {binding} should only be visible to the vertex stage"
            );
            assert!(
                !entry_source.contains("ShaderStages::FRAGMENT"),
                "binding {binding} should not be visible to the fragment stage"
            );
        }
    }

    #[test]
    fn nova_wgsl_files_are_covered_by_build_validation() {
        let shader_dir =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/platform/nova/shaders");
        let build_script = include_str!("../../build.rs");
        let renderer_source = include_str!("nova_renderer.rs");
        let mut shader_names = Vec::new();

        for shader_entry in std::fs::read_dir(&shader_dir)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", shader_dir.display()))
        {
            let shader_entry = shader_entry
                .unwrap_or_else(|error| panic!("failed to read shader dir entry: {error}"));
            let shader_path = shader_entry.path();
            if shader_path
                .extension()
                .and_then(|extension| extension.to_str())
                != Some("wgsl")
            {
                continue;
            }

            let shader_name = shader_path
                .file_name()
                .and_then(|file_name| file_name.to_str())
                .expect("WGSL shader file names should be valid UTF-8");

            assert_ne!(
                shader_name, "basic_quad.wgsl",
                "basic_quad.wgsl is deprecated and should not live in the production Nova shader directory"
            );

            shader_names.push(shader_name.to_string());

            let build_validation_path = format!("./src/platform/nova/shaders/{shader_name}");
            assert!(
                build_script.contains(&build_validation_path),
                "{shader_name} is not covered by build.rs Nova WGSL validation"
            );

            let runtime_bundle_path = format!("include_str!(\"nova/shaders/{shader_name}\")");
            assert!(
                renderer_source.contains(&runtime_bundle_path),
                "{shader_name} is not included by nova_renderer.rs runtime shader bundles"
            );
        }

        assert_shader_contains(
            "build.rs",
            build_script,
            &[
                "fn check_nova_wgsl_shader_coverage",
                "std::fs::read_dir(SHADER_DIR)",
                "basic_quad.wgsl is deprecated",
                "is not covered by build shader validation",
            ],
        );

        shader_names.sort_unstable();
        let mut tested_shader_names = nova_shader_sources()
            .into_iter()
            .map(|(shader_name, _)| shader_name.to_string())
            .collect::<Vec<_>>();
        tested_shader_names.sort_unstable();
        assert_eq!(
            shader_names, tested_shader_names,
            "nova_shader_sources() should cover every production Nova WGSL file"
        );
    }

    fn nova_shader_sources() -> [(&'static str, &'static str); 14] {
        [
            (
                "backdrop_blur.wgsl",
                include_str!("nova/shaders/backdrop_blur.wgsl"),
            ),
            ("core.wgsl", include_str!("nova/shaders/core.wgsl")),
            ("mesh_3d.wgsl", include_str!("nova/shaders/mesh_3d.wgsl")),
            (
                "mono_sprite.wgsl",
                include_str!("nova/shaders/mono_sprite.wgsl"),
            ),
            ("path.wgsl", include_str!("nova/shaders/path.wgsl")),
            (
                "poly_sprite.wgsl",
                include_str!("nova/shaders/poly_sprite.wgsl"),
            ),
            ("quad.wgsl", include_str!("nova/shaders/quad.wgsl")),
            (
                "quad_common.wgsl",
                include_str!("nova/shaders/quad_common.wgsl"),
            ),
            ("shadow.wgsl", include_str!("nova/shaders/shadow.wgsl")),
            ("shape.wgsl", include_str!("nova/shaders/shape.wgsl")),
            (
                "solid_quad.wgsl",
                include_str!("nova/shaders/solid_quad.wgsl"),
            ),
            ("surface.wgsl", include_str!("nova/shaders/surface.wgsl")),
            ("text.wgsl", include_str!("nova/shaders/text.wgsl")),
            (
                "underline.wgsl",
                include_str!("nova/shaders/underline.wgsl"),
            ),
        ]
    }

    fn assert_shader_contains(shader_name: &str, source: &str, expected_fragments: &[&str]) {
        for expected_fragment in expected_fragments {
            assert!(
                source.contains(expected_fragment),
                "{shader_name} is missing edge guard fragment: {expected_fragment}"
            );
        }
    }

    fn shader_struct_source<'a>(shader_name: &str, source: &'a str, struct_name: &str) -> &'a str {
        let struct_marker = format!("struct {struct_name} {{");
        let struct_start = source
            .find(&struct_marker)
            .unwrap_or_else(|| panic!("{shader_name} is missing struct {struct_name}"));
        let struct_source = &source[struct_start..];
        let struct_end = struct_source
            .find("\n}")
            .map(|offset| offset + "\n}".len())
            .unwrap_or_else(|| {
                panic!("{shader_name} struct {struct_name} is missing closing brace")
            });

        &struct_source[..struct_end]
    }

    fn assert_fragment_contains_in_order(
        shader_name: &str,
        source: &str,
        function_name: &str,
        expected_fragments: &[&str],
    ) {
        let function_source = shader_function_source(shader_name, source, function_name);
        let mut search_start = 0;
        for expected_fragment in expected_fragments {
            let fragment_offset = function_source[search_start..]
                .find(expected_fragment)
                .unwrap_or_else(|| {
                    panic!(
                        "{shader_name}::{function_name} is missing ordered fragment: {expected_fragment}"
                    )
                });
            search_start += fragment_offset + expected_fragment.len();
        }
    }

    fn assert_fragment_function_omits(
        shader_name: &str,
        source: &str,
        function_name: &str,
        forbidden_fragment: &str,
    ) {
        let function_source = shader_function_source(shader_name, source, function_name);

        assert!(
            !function_source.contains(forbidden_fragment),
            "{shader_name}::{function_name} should not read {forbidden_fragment} in fragment stage"
        );
    }

    fn shader_function_source<'a>(
        shader_name: &str,
        source: &'a str,
        function_name: &str,
    ) -> &'a str {
        let function_marker = format!("fn {function_name}(");
        let function_start = source
            .find(&function_marker)
            .unwrap_or_else(|| panic!("{shader_name} is missing {function_name}"));
        let function_source = &source[function_start..];
        let body_start = function_source
            .find('{')
            .unwrap_or_else(|| panic!("{shader_name}::{function_name} is missing a body"));
        let mut depth = 0_i32;
        for (offset, character) in function_source[body_start..].char_indices() {
            match character {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &function_source[..body_start + offset + 1];
                    }
                }
                _ => {}
            }
        }

        panic!("{shader_name}::{function_name} has an unterminated body");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn nova_sprite_hlsl_bindings_match_dx12_resource_sets() {
        let mono_source = concat!(
            include_str!("nova/shaders/core.wgsl"),
            include_str!("nova/shaders/text.wgsl"),
            include_str!("nova/shaders/mono_sprite.wgsl"),
        );
        let gfx_core::ShaderCode::Hlsl(mono_hlsl) =
            compile_wgsl_to_hlsl(mono_source, ShaderStage::Fragment, "fs_mono_sprite")
                .expect("mono sprite fragment shader should compile to HLSL")
                .code
        else {
            panic!("expected mono sprite HLSL");
        };
        assert!(
            mono_hlsl.contains("Texture2D<float4> t_sprite : register(t4)"),
            "unexpected mono sprite HLSL:\n{mono_hlsl}"
        );
        assert!(
            mono_hlsl.contains("SamplerState nagaSamplerHeap[2048]: register(s0, space0)"),
            "unexpected mono sprite HLSL:\n{mono_hlsl}"
        );
        assert!(
            mono_hlsl.contains("StructuredBuffer<uint> nagaGroup0SamplerIndexArray"),
            "unexpected mono sprite HLSL:\n{mono_hlsl}"
        );

        let gfx_core::ShaderCode::Hlsl(mono_vertex_hlsl) =
            compile_wgsl_to_hlsl(mono_source, ShaderStage::Vertex, "vs_mono_sprite")
                .expect("mono sprite vertex shader should compile to HLSL")
                .code
        else {
            panic!("expected mono sprite vertex HLSL");
        };
        assert!(
            mono_vertex_hlsl.contains("ByteAddressBuffer b_mono_sprites : register(t8)"),
            "unexpected mono sprite vertex HLSL:\n{mono_vertex_hlsl}"
        );
        assert!(
            mono_vertex_hlsl.contains("_NagaConstants.first_instance"),
            "DX12 sprite vertex shaders must offset instance_index by DrawStepDesc.first_instance:\n{mono_vertex_hlsl}"
        );

        let gfx_core::ShaderCode::Hlsl(poly_vertex_hlsl) = compile_wgsl_to_hlsl(
            NOVA_POLY_SPRITE_SHADER_SOURCE,
            ShaderStage::Vertex,
            "vs_poly_sprite",
        )
        .expect("poly sprite vertex shader should compile to HLSL")
        .code
        else {
            panic!("expected poly sprite vertex HLSL");
        };
        assert!(
            poly_vertex_hlsl.contains("ByteAddressBuffer b_poly_sprites : register(t9)"),
            "unexpected poly sprite vertex HLSL:\n{poly_vertex_hlsl}"
        );
    }

    fn test_atlas_tile() -> AtlasTile {
        AtlasTile {
            texture_id: AtlasTextureId {
                index: 0,
                kind: AtlasTextureKind::Rgba,
            },
            tile_id: TileId(1),
            padding: 0,
            bounds: Bounds {
                origin: Point {
                    x: DevicePixels(0),
                    y: DevicePixels(0),
                },
                size: size(DevicePixels(1), DevicePixels(1)),
            },
        }
    }

    fn test_render_pipeline_id(index: u32) -> RenderPipelineId {
        RenderPipelineId::from_parts(index, 1)
    }

    fn test_blend_pipelines(base: u32) -> NovaBlendPipelines {
        NovaBlendPipelines {
            solid_quads: test_render_pipeline_id(base + 1),
            quads: test_render_pipeline_id(base + 2),
            shadows: test_render_pipeline_id(base + 3),
            mono_sprites: test_render_pipeline_id(base + 6),
            poly_sprites: test_render_pipeline_id(base + 7),
            underlines: test_render_pipeline_id(base + 8),
            backdrop_blurs: test_render_pipeline_id(base + 9),
        }
    }

    fn test_pipelines() -> NovaPipelines {
        NovaPipelines {
            alpha: test_blend_pipelines(0),
            premultiplied: test_blend_pipelines(100),
            path_rasterization: test_render_pipeline_id(4),
            paths: test_render_pipeline_id(5),
            backdrop_blur_downsample: test_render_pipeline_id(6),
            backdrop_blur_upsample: test_render_pipeline_id(7),
        }
    }

    fn test_resource_set_id(index: u32) -> ResourceSetId {
        ResourceSetId::from_parts(index, 1)
    }

    fn test_texture_id(index: u32) -> TextureId {
        TextureId::from_parts(index, 1)
    }

    fn test_texture_view_id(index: u32) -> TextureViewId {
        TextureViewId::from_parts(index, 1)
    }

    fn read_u32_at(bytes: &[u8], offset: usize) -> u32 {
        let chunk = bytes
            .get(offset..offset + std::mem::size_of::<u32>())
            .expect("test offset should be in bounds");
        u32::from_ne_bytes(chunk.try_into().expect("u32 chunk should have exact size"))
    }
}
