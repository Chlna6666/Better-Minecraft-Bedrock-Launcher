pub(super) use std::{
    borrow::Cow,
    sync::{Arc, Mutex},
    time::Instant,
};

pub(super) use anyhow::{Context as _, Result};
pub(super) use collections::{FxHashMap, FxHashSet};

pub(super) use crate::{
    AtlasKey, AtlasTextureId, AtlasTextureKind, AtlasTile, Bounds, DevicePixels,
    GlyphRasterization, GpuSpecs, GpuSubmissionMode, GpuiMemoryTrimLevel, MonochromeSprite,
    PaintRendererExtension, PartialPresentMode, PlatformAtlas, Point, PolychromeSprite,
    PreparedSceneBatch, PresentModePreference, PresentationPacket, Quad, RenderGlyphParams,
    RendererBackend, RendererExtension, RendererExtensionContext, RendererExtensionRenderer,
    RendererOptions, RetainedChunkId, Shadow, Size, Underline,
};

pub(super) use gfx_core::{
    AddressMode, AsyncCapabilities, BackendKind, BindGroupDescriptor, BindGroupEntry, BindGroupId,
    BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindGroupLayoutId, BindingResource,
    BindingType, BlendMode, BufferBinding, BufferDescriptor, BufferId, BufferUsage, ClearColor,
    ColorAttachmentDescriptor, ColorAttachmentDescriptor as _ColorAttachment, CommandDevice,
    CompareFunction, CompositeAlphaMode, DepthAttachment, DepthAttachmentDescriptor, DepthState,
    DeviceDescriptor, DiagnosticsDevice, DrawCommand, DrawIndexedCommand, Extent2d, FilterMode,
    Format, IndexBufferBinding, IndexFormat, LoadOp, MemoryLocation, MemoryTrimLevel, Origin2d,
    PipelineDevice, PipelineLayoutDescriptor, PipelineLayoutId, PowerPreference,
    PresentFrameDescriptor, PresentTimings, PresentationDevice, PresentedFrame, PrimitiveTopology,
    RenderCommand, RenderPassCommands, RenderPassDescriptor, RenderPassId,
    RenderPipelineDescriptor, RenderPipelineId, ResourceDevice, SamplerBinding, SamplerDescriptor,
    SamplerId, ScissorRect, ShaderModuleDescriptor, ShaderStage, ShaderStages, SignedOrigin2d,
    SubmissionDevice, SubmissionId, SubmissionStatus, SurfaceCapabilities, SurfaceConfig,
    SurfaceDescriptor, SurfaceDevice, SurfaceId, SwapchainId, TextureBinding, TextureDataLayout,
    TextureDescriptor, TextureDimension, TextureId, TextureUsage, TextureViewDescriptor,
    TextureViewId, TextureWrite, TextureWriteDescriptor, Viewport, ViewportMapping,
    bind_group_list,
};

#[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
pub(super) use gfx_dx11::Dx11Device;
#[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
pub(super) use gfx_dx12::Dx12Device;
#[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
pub(super) use gfx_metal::MetalDevice;
#[cfg(all(
    feature = "nova-gfx-opengl",
    any(target_os = "windows", target_os = "linux")
))]
pub(super) use gfx_opengl::OpenGlDevice;
// Production shaders come from the artifacts `build.rs` embeds, so WGSL translation
// is only reachable from tests that exercise the translator directly.
#[cfg(all(test, feature = "nova-gfx-dx12", target_os = "windows"))]
pub(super) use gfx_shader::compile_wgsl_to_hlsl;
#[cfg(all(test, feature = "nova-gfx-metal", target_os = "macos"))]
pub(super) use gfx_shader::compile_wgsl_to_msl;
#[cfg(all(
    test,
    feature = "nova-gfx-vulkan",
    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
))]
pub(super) use gfx_shader::compile_wgsl_to_spirv;
#[cfg(all(
    feature = "nova-gfx-vulkan",
    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
))]
pub(super) use gfx_vulkan::VulkanDevice;
