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
    AddressMode, AsyncCapabilities, BackendPipelines, BackendPresentationCompat, BackendQueue,
    BackendResources, BackendSurface, BlendMode, BufferBinding, BufferDescriptor, BufferId,
    BufferUsage, ClearColor, ColorAttachmentDescriptor, CompareFunction, CompositeAlphaMode,
    DepthAttachmentDescriptor, DepthState, DeviceDescriptor, DrawIndexedStepDescriptor,
    DrawStepDescriptor, Extent2d, FilterMode, Format, IndexBufferBinding, IndexFormat, LoadOp,
    MemoryLocation, MemoryTrimLevel, Origin2d, PipelineLayoutDescriptor, PipelineLayoutId,
    PowerPreference, PresentationDevice, PrimitiveTopology, RenderPassDepthAttachment,
    RenderPassDescriptor, RenderPassId, RenderPipelineDescriptor, RenderPipelineId,
    RenderStepDescriptor, RenderStepList, ResourceBinding, BindingResource,
    ResourceBindingType, ResourceSetDescriptor, ResourceSetId, ResourceSetLayoutDescriptor,
    ResourceSetLayoutEntry, ResourceSetLayoutId, SamplerBinding, SamplerDescriptor, SamplerId,
    ScissorRect, ShaderModuleDescriptor, ShaderStage, ShaderStages, SubmissionId, SubmissionStatus,
    SurfaceConfig, SurfaceDescriptor, SurfaceId, SwapchainId, TextureBinding, TextureDataLayout,
    TextureDescriptor, TextureDimension, TextureId, TextureUsage, TextureViewDescriptor,
    TextureViewId, TextureWrite, TextureWriteDescriptor, resource_set_list,
};

#[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
pub(super) use gfx_dx12::Dx12Device;
#[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
pub(super) use gfx_metal::MetalDevice;
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
