pub(super) use std::{
    borrow::Cow,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub(super) use anyhow::{Context as _, Result};
pub(super) use collections::{FxHashMap, FxHashSet};

pub(super) use crate::{
    AtlasKey, AtlasTextureId, AtlasTextureKind, AtlasTile, Bounds, DevicePixels, FrameRenderPlan,
    FrameVisualEffectQuality, GlyphRasterization, GpuMesh3d, GpuMesh3dId, GpuMesh3dRange,
    GpuMesh3dShader, GpuMesh3dShaderId, GpuSpecs, GpuSubmissionMode, GpuiMemoryTrimLevel,
    MonochromeSprite, PartialPresentMode, PlatformAtlas, Point, PolychromeSprite,
    PreparedSceneBatch, PresentModePreference, Quad, RenderGlyphParams, RendererBackend,
    RendererOptions, Shadow, Size, Underline,
};

pub(super) use gfx_core::{
    AddressMode, BackendAsyncCapabilities, BackendDiagnostics, BackendPipelines,
    BackendPresentationCompat, BackendQueue, BackendResources, BackendSurface, BlendMode,
    BufferBinding, BufferDescriptor, BufferId, BufferUsage, ClearColor, ColorAttachmentDescriptor,
    CompositeAlphaMode, DepthAttachmentDescriptor, DepthState, DeviceDescriptor,
    DrawIndexedStepDescriptor, DrawStepDescriptor, Extent2d, FilterMode, Format,
    GfxMemoryTrimLevel, IndexBufferBinding, IndexFormat, LoadOp, MemoryLocation, Origin2d,
    PipelineLayoutId, PipelineLayoutResourceDescriptor, PowerPreference, PrimitiveTopology,
    RenderPassCompatibilityDescriptor, RenderPassDepthAttachment, RenderPassId,
    RenderPipelineDescriptor, RenderPipelineId, RenderStepDescriptor, RenderStepList,
    ResourceBinding, ResourceBindingResource, ResourceBindingType, ResourceSetDescriptor,
    ResourceSetId, ResourceSetLayoutDescriptor, ResourceSetLayoutEntry, ResourceSetLayoutId,
    SamplerBinding, SamplerDescriptor, SamplerId, ScissorRect, ShaderModuleDescriptor, ShaderStage,
    ShaderStages, SubmissionId, SubmissionStatus, SurfaceConfig, SurfaceDescriptor, SurfaceId,
    SwapchainId, TextureBinding, TextureDataLayout, TextureDescriptor, TextureDimension, TextureId,
    TextureUsage, TextureViewDescriptor, TextureViewId, TextureWrite, TextureWriteDescriptor,
    resource_set_list,
};

#[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
pub(super) use gfx_dx12::Dx12Device;
#[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
pub(super) use gfx_metal::MetalDevice;
#[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
pub(super) use gfx_shader::compile_wgsl_to_hlsl;
#[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
pub(super) use gfx_shader::compile_wgsl_to_msl;
#[cfg(all(
    feature = "nova-gfx-vulkan",
    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
))]
pub(super) use gfx_shader::compile_wgsl_to_spirv;
#[cfg(all(
    feature = "nova-gfx-vulkan",
    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
))]
pub(super) use gfx_vulkan::VulkanDevice;
