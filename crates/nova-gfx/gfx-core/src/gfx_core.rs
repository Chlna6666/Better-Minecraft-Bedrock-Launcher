//! Backend-neutral graphics API types for nova-gfx.
//!
//! `gfx-core` intentionally contains no GPUI, Vulkan, DX12, Metal, or windowing
//! code. It provides shared descriptors, errors, and typed generational handles
//! used by backend implementations.
//!
//! The canonical device contract is exposed through the `Gfx*Device` traits.
//! Backend crates implement those traits; callers should import only the narrow
//! traits they need.
//!
//! Chinese documentation is available in `README.zh-CN.md` in the crate source
//! package.
//!
//! # Examples
//!
//! ```no_run
//! use gfx_core::{BufferDesc, BufferId, GfxResourceDevice, Result};
//!
//! fn create_buffer<D>(device: &mut D, desc: &BufferDesc) -> Result<BufferId>
//! where
//!     D: GfxResourceDevice,
//! {
//!     device.create_buffer(desc)
//! }
//! ```

mod backend;

use std::{
    fmt,
    future::Future,
    hash::{Hash, Hasher},
    marker::PhantomData,
    num::NonZeroU32,
    pin::Pin,
};

use bitflags::bitflags;
use thiserror::Error;

pub use backend::{
    GfxAsyncCommandDevice, GfxAsyncDevice, GfxAsyncDiagnosticsDevice, GfxAsyncPipelineDevice,
    GfxAsyncPresentationDevice, GfxAsyncResourceDevice, GfxAsyncSurfaceDevice, GfxBackend,
    GfxCommandDevice, GfxDevice, GfxDiagnosticsDevice, GfxPipelineDevice, GfxPresentationDevice,
    GfxResourceDevice, GfxSubmissionDevice, GfxSurfaceDevice, SharedGfxDevice,
};

/// Convenience result type used by nova-gfx crates.
pub type Result<T> = std::result::Result<T, GfxError>;

/// Runtime-neutral boxed future returned by nova-gfx async interfaces.
pub type GfxFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

/// Error type for backend-neutral validation and backend-provided failures.
#[derive(Debug, Error)]
pub enum GfxError {
    /// A required graphics capability or resource was not available.
    #[error("graphics resource is unavailable: {0}")]
    Unavailable(String),
    /// A descriptor, handle, or command was invalid.
    #[error("invalid graphics input: {0}")]
    InvalidInput(String),
    /// Shader parsing, validation, or translation failed.
    #[error("shader error: {0}")]
    Shader(String),
    /// A backend operation failed.
    #[error("backend error: {0}")]
    Backend(String),
}

/// Opaque typed generational resource identifier.
#[repr(transparent)]
pub struct ResourceId<T> {
    raw: u64,
    marker: PhantomData<fn() -> T>,
}

impl<T> ResourceId<T> {
    const INDEX_BITS: u64 = 32;
    const INDEX_MASK: u64 = u32::MAX as u64;

    /// Creates a resource identifier from a backend-owned raw value.
    #[must_use]
    pub const fn new(raw: u64) -> Self {
        Self {
            raw,
            marker: PhantomData,
        }
    }

    /// Creates a generational resource identifier.
    #[must_use]
    pub const fn from_parts(index: u32, generation: u32) -> Self {
        let raw = (generation as u64) << Self::INDEX_BITS | index as u64;
        Self::new(raw)
    }

    /// Returns the backend-owned raw value.
    #[must_use]
    pub const fn raw(self) -> u64 {
        self.raw
    }

    /// Returns the resource slot index.
    #[must_use]
    pub const fn index(self) -> u32 {
        (self.raw & Self::INDEX_MASK) as u32
    }

    /// Returns the resource generation.
    #[must_use]
    pub const fn generation(self) -> u32 {
        (self.raw >> Self::INDEX_BITS) as u32
    }
}

impl<T> fmt::Debug for ResourceId<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ResourceId")
            .field("index", &self.index())
            .field("generation", &self.generation())
            .field("raw", &self.raw)
            .finish()
    }
}

impl<T> Clone for ResourceId<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for ResourceId<T> {}

impl<T> PartialEq for ResourceId<T> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}

impl<T> Eq for ResourceId<T> {}

impl<T> Hash for ResourceId<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.raw.hash(state);
    }
}

/// Logical GPU instance handle.
#[derive(Debug)]
pub enum InstanceResource {}
/// Physical adapter handle.
#[derive(Debug)]
pub enum AdapterResource {}
/// Logical device handle.
#[derive(Debug)]
pub enum DeviceResource {}
/// Submission queue handle.
#[derive(Debug)]
pub enum QueueResource {}
/// Buffer handle.
#[derive(Debug)]
pub enum BufferResource {}
/// Texture handle.
#[derive(Debug)]
pub enum TextureResource {}
/// Texture view handle.
#[derive(Debug)]
pub enum TextureViewResource {}
/// Sampler handle.
#[derive(Debug)]
pub enum SamplerResource {}
/// Resource set layout handle.
#[derive(Debug)]
pub enum ResourceSetLayoutResource {}
/// Resource set handle.
#[derive(Debug)]
pub enum ResourceSetResource {}
/// Pipeline layout handle.
#[derive(Debug)]
pub enum PipelineLayoutResource {}
/// Shader module handle.
#[derive(Debug)]
pub enum ShaderModuleResource {}
/// Render pass handle.
#[derive(Debug)]
pub enum RenderPassResource {}
/// Render pipeline handle.
#[derive(Debug)]
pub enum RenderPipelineResource {}
/// Command encoder handle.
#[derive(Debug)]
pub enum CommandEncoderResource {}
/// Surface handle.
#[derive(Debug)]
pub enum SurfaceResource {}
/// Swapchain handle.
#[derive(Debug)]
pub enum SwapchainResource {}
/// GPU submission handle.
#[derive(Debug)]
pub enum SubmissionResource {}

/// Instance resource identifier.
pub type InstanceId = ResourceId<InstanceResource>;
/// Adapter resource identifier.
pub type AdapterId = ResourceId<AdapterResource>;
/// Device resource identifier.
pub type DeviceId = ResourceId<DeviceResource>;
/// Queue resource identifier.
pub type QueueId = ResourceId<QueueResource>;
/// Buffer resource identifier.
pub type BufferId = ResourceId<BufferResource>;
/// Texture resource identifier.
pub type TextureId = ResourceId<TextureResource>;
/// Texture view resource identifier.
pub type TextureViewId = ResourceId<TextureViewResource>;
/// Sampler resource identifier.
pub type SamplerId = ResourceId<SamplerResource>;
/// Resource set layout identifier.
pub type ResourceSetLayoutId = ResourceId<ResourceSetLayoutResource>;
/// Resource set identifier.
pub type ResourceSetId = ResourceId<ResourceSetResource>;
/// Pipeline layout identifier.
pub type PipelineLayoutId = ResourceId<PipelineLayoutResource>;
/// Shader module resource identifier.
pub type ShaderModuleId = ResourceId<ShaderModuleResource>;
/// Render pass resource identifier.
pub type RenderPassId = ResourceId<RenderPassResource>;
/// Render pipeline resource identifier.
pub type RenderPipelineId = ResourceId<RenderPipelineResource>;
/// Command encoder resource identifier.
pub type CommandEncoderId = ResourceId<CommandEncoderResource>;
/// Surface resource identifier.
pub type SurfaceId = ResourceId<SurfaceResource>;
/// Swapchain resource identifier.
pub type SwapchainId = ResourceId<SwapchainResource>;
/// Submission resource identifier.
pub type SubmissionId = ResourceId<SubmissionResource>;

/// Completion state for a GPU submission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SubmissionStatus {
    /// The backend still reports work in flight.
    Pending,
    /// The submission completed successfully.
    Complete,
    /// The backend reported failure while tracking or waiting for the submission.
    Failed(String),
}

impl SubmissionStatus {
    /// Returns whether the submission is no longer pending.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        !matches!(self, Self::Pending)
    }
}

/// Threading support exposed by a backend or device wrapper.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum GfxThreadingMode {
    /// Calls must be made on the owning thread.
    #[default]
    OwnerThreadOnly,
    /// Submission waits may be performed from another thread.
    MultiThreadWait,
    /// A serializing device proxy supports cross-thread device calls.
    MultiThreadDeviceProxy,
}

/// Async and threading capabilities for a nova-gfx device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "backend capabilities are independent feature flags exposed as a stable public API"
)]
pub struct GfxAsyncCapabilities {
    /// How the device can be accessed from multiple threads.
    pub threading_mode: GfxThreadingMode,
    /// Device can return submission handles without blocking for completion.
    pub async_submission: bool,
    /// Device can wait for submission completion asynchronously.
    pub async_wait: bool,
    /// Presentation helper can be submitted through an async/deferred path.
    pub async_presentation: bool,
    /// Swapchain presentation preserves or restricts unchanged regions safely.
    pub partial_presentation: bool,
}

impl Default for GfxAsyncCapabilities {
    fn default() -> Self {
        Self {
            threading_mode: GfxThreadingMode::OwnerThreadOnly,
            async_submission: false,
            async_wait: false,
            async_presentation: false,
            partial_presentation: false,
        }
    }
}

/// Width and height in pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Extent2d {
    /// Width in pixels.
    pub width: NonZeroU32,
    /// Height in pixels.
    pub height: NonZeroU32,
}

impl Extent2d {
    /// Builds a non-zero extent.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError::InvalidInput`] when either dimension is zero.
    pub fn new(width: u32, height: u32) -> Result<Self> {
        let width = NonZeroU32::new(width)
            .ok_or_else(|| GfxError::InvalidInput("width must be greater than zero".to_string()))?;
        let height = NonZeroU32::new(height).ok_or_else(|| {
            GfxError::InvalidInput("height must be greater than zero".to_string())
        })?;
        Ok(Self { width, height })
    }

    /// Returns width as `u32`.
    #[must_use]
    pub const fn width(self) -> u32 {
        self.width.get()
    }

    /// Returns height as `u32`.
    #[must_use]
    pub const fn height(self) -> u32 {
        self.height.get()
    }
}

/// Origin in a 2D texture.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Origin2d {
    /// X coordinate in pixels.
    pub x: u32,
    /// Y coordinate in pixels.
    pub y: u32,
}

impl Origin2d {
    /// Zero origin.
    pub const ZERO: Self = Self { x: 0, y: 0 };
}

/// Debug label shared by resource descriptors.
pub type ResourceLabel = Option<String>;

/// Graphics backend kind.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum BackendKind {
    /// Vulkan backend.
    Vulkan,
    /// Direct3D 12 backend.
    Dx12,
    /// Metal backend.
    Metal,
}

/// Backend feature limits used for adapter selection and diagnostics.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BackendCapabilities {
    /// Backend can create a presentable native surface.
    pub surface: bool,
    /// Backend supports CPU-visible upload resources.
    pub cpu_visible_memory: bool,
    /// Backend supports GPU-only resources with staging uploads.
    pub gpu_only_memory: bool,
}

/// Logical device creation descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceDesc {
    /// Application name reported to backends.
    pub application_name: String,
}

impl Default for DeviceDesc {
    fn default() -> Self {
        Self {
            application_name: "nova-gfx".to_string(),
        }
    }
}

/// Backend adapter information.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdapterInfo {
    /// Backend that exposes this adapter.
    pub backend: BackendKind,
    /// Adapter name.
    pub name: String,
    /// Vendor ID when the backend exposes one.
    pub vendor_id: u32,
    /// Device ID when the backend exposes one.
    pub device_id: u32,
    /// Adapter capabilities known to nova-gfx.
    pub capabilities: BackendCapabilities,
}

/// Queue role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueueKind {
    /// Graphics queue.
    Graphics,
    /// Presentation queue.
    Present,
}

/// Queue descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueueDesc {
    /// Queue role.
    pub kind: QueueKind,
    /// Backend queue family index.
    pub family_index: u32,
}

/// Color formats supported by the phase-1 API.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Format {
    /// 8-bit BGRA unsigned normalized format.
    Bgra8Unorm,
    /// 8-bit BGRA unsigned normalized sRGB format.
    Bgra8UnormSrgb,
    /// 8-bit RGBA unsigned normalized format.
    Rgba8Unorm,
    /// 8-bit RGBA unsigned normalized sRGB format.
    Rgba8UnormSrgb,
}

impl Format {
    /// Returns whether this format uses sRGB conversion.
    #[must_use]
    pub const fn is_srgb(self) -> bool {
        matches!(self, Self::Bgra8UnormSrgb | Self::Rgba8UnormSrgb)
    }
}

/// Surface presentation mode preference.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PresentMode {
    /// FIFO/vsync presentation.
    #[default]
    Fifo,
    /// Mailbox presentation when supported.
    Mailbox,
    /// Immediate presentation when supported.
    Immediate,
}

/// Alpha compositing behavior for a surface.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CompositeAlphaMode {
    /// Let the backend select the best alpha mode.
    #[default]
    Auto,
    /// Opaque surface.
    Opaque,
    /// Premultiplied alpha.
    Premultiplied,
}

/// Native surface descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SurfaceDesc {
    /// Debug label.
    pub label: ResourceLabel,
}

/// Surface configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SurfaceConfig {
    /// Surface size.
    pub size: Extent2d,
    /// Color format.
    pub format: Format,
    /// Present mode.
    pub present_mode: PresentMode,
    /// Alpha behavior.
    pub alpha_mode: CompositeAlphaMode,
}

impl SurfaceConfig {
    /// Creates a validated swapchain descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError::InvalidInput`] when width or height is zero.
    pub fn new(width: u32, height: u32, format: Format) -> Result<Self> {
        Ok(Self {
            size: Extent2d::new(width, height)?,
            format,
            present_mode: PresentMode::Fifo,
            alpha_mode: CompositeAlphaMode::Auto,
        })
    }
}

/// Compatibility alias for the swapchain descriptor name.
pub type SwapchainDesc = SurfaceConfig;

bitflags! {
    /// Buffer usage flags.
    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    pub struct BufferUsage: u32 {
        /// Transfer source.
        const COPY_SRC = 1 << 0;
        /// Transfer destination.
        const COPY_DST = 1 << 1;
        /// Vertex buffer.
        const VERTEX = 1 << 2;
        /// Index buffer.
        const INDEX = 1 << 3;
        /// Uniform buffer.
        const UNIFORM = 1 << 4;
        /// Storage buffer.
        const STORAGE = 1 << 5;
    }

    /// Texture usage flags.
    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    pub struct TextureUsage: u32 {
        /// Transfer source.
        const COPY_SRC = 1 << 0;
        /// Transfer destination.
        const COPY_DST = 1 << 1;
        /// Sampled texture.
        const SAMPLED = 1 << 2;
        /// Color attachment.
        const COLOR_ATTACHMENT = 1 << 3;
        /// Depth attachment.
        const DEPTH_ATTACHMENT = 1 << 4;
    }
}

bitflags! {
    /// Shader stage visibility for resources and pipeline layouts.
    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    pub struct ShaderStages: u32 {
        /// Vertex stage visibility.
        const VERTEX = 1 << 0;
        /// Fragment stage visibility.
        const FRAGMENT = 1 << 1;
    }
}

/// A typed buffer binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BufferBinding {
    /// Buffer resource.
    pub buffer: BufferId,
    /// Byte offset into the buffer.
    pub offset: u64,
    /// Binding size in bytes.
    pub size: u64,
    /// Structured element stride for storage-buffer views.
    pub stride: Option<u32>,
}

/// A typed texture binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextureBinding {
    /// Texture view resource.
    pub texture_view: TextureViewId,
}

/// A typed sampler binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SamplerBinding {
    /// Sampler resource.
    pub sampler: SamplerId,
}

/// A resource binding entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceBinding {
    /// Binding slot.
    pub binding: u32,
    /// Resource payload.
    pub resource: ResourceBindingResource,
}

/// A resource binding payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceBindingResource {
    /// Uniform buffer binding.
    Buffer(BufferBinding),
    /// Sampled texture binding.
    Texture(TextureBinding),
    /// Sampler binding.
    Sampler(SamplerBinding),
}

/// Resource binding kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceBindingType {
    /// Uniform buffer.
    UniformBuffer,
    /// Read-only storage buffer.
    StorageBuffer,
    /// Sampled texture or texture view.
    SampledTexture,
    /// Sampler.
    Sampler,
}

impl ResourceBindingType {
    /// Returns the expected payload type for this binding.
    #[must_use]
    pub const fn matches(self, binding: &ResourceBindingResource) -> bool {
        matches!(
            (self, binding),
            (
                Self::UniformBuffer | Self::StorageBuffer,
                ResourceBindingResource::Buffer(_)
            ) | (Self::SampledTexture, ResourceBindingResource::Texture(_))
                | (Self::Sampler, ResourceBindingResource::Sampler(_))
        )
    }
}

/// Resource layout entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceSetLayoutEntry {
    /// Binding slot.
    pub binding: u32,
    /// Resource kind.
    pub binding_type: ResourceBindingType,
    /// Shader visibility.
    pub stages: ShaderStages,
}

/// Resource set layout descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceSetLayoutDesc {
    /// Debug label.
    pub label: ResourceLabel,
    /// Layout entries.
    pub entries: Vec<ResourceSetLayoutEntry>,
}

impl ResourceSetLayoutDesc {
    /// Validates the descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError::InvalidInput`] when the layout is empty or contains
    /// duplicate bindings.
    pub fn validate(&self) -> Result<()> {
        if self.entries.is_empty() {
            return Err(GfxError::InvalidInput(
                "resource set layout must contain at least one entry".to_string(),
            ));
        }
        let mut bindings = std::collections::BTreeSet::new();
        for entry in &self.entries {
            if entry.stages.is_empty() {
                return Err(GfxError::InvalidInput(
                    "resource set layout entry must be visible to at least one shader stage"
                        .to_string(),
                ));
            }
            if !bindings.insert(entry.binding) {
                return Err(GfxError::InvalidInput(format!(
                    "duplicate resource binding slot {}",
                    entry.binding
                )));
            }
        }
        Ok(())
    }
}

/// Resource set descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceSetDesc {
    /// Debug label.
    pub label: ResourceLabel,
    /// Layout this resource set conforms to.
    pub layout: ResourceSetLayoutId,
    /// Concrete bindings.
    pub bindings: Vec<ResourceBinding>,
}

impl ResourceSetDesc {
    /// Validates the descriptor against a layout.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError::InvalidInput`] when a binding is missing or invalid.
    pub fn validate_against(&self, layout: &ResourceSetLayoutDesc) -> Result<()> {
        layout.validate()?;
        if self.bindings.len() != layout.entries.len() {
            return Err(GfxError::InvalidInput(format!(
                "resource set binding count {} does not match layout entry count {}",
                self.bindings.len(),
                layout.entries.len()
            )));
        }
        let mut bindings_by_slot = std::collections::BTreeMap::new();
        for binding in &self.bindings {
            if bindings_by_slot
                .insert(binding.binding, binding.resource)
                .is_some()
            {
                return Err(GfxError::InvalidInput(format!(
                    "duplicate resource binding slot {}",
                    binding.binding
                )));
            }
        }
        for entry in &layout.entries {
            let Some(binding) = bindings_by_slot.get(&entry.binding) else {
                return Err(GfxError::InvalidInput(format!(
                    "missing resource binding slot {}",
                    entry.binding
                )));
            };
            if !entry.binding_type.matches(binding) {
                return Err(GfxError::InvalidInput(format!(
                    "binding {} has incompatible resource type",
                    entry.binding
                )));
            }
        }
        Ok(())
    }
}

/// Pipeline layout descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PipelineLayoutDesc {
    /// Debug label.
    pub label: ResourceLabel,
    /// Resource set layouts used by this pipeline.
    pub resource_set_layouts: Vec<ResourceSetLayoutId>,
}

impl PipelineLayoutDesc {
    /// Validates the descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError::InvalidInput`] when the layout is empty.
    pub fn validate(&self) -> Result<()> {
        if self.resource_set_layouts.is_empty() {
            return Err(GfxError::InvalidInput(
                "pipeline layout must contain at least one resource set layout".to_string(),
            ));
        }
        Ok(())
    }
}

/// Resource memory placement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryLocation {
    /// CPU-visible memory intended for uploads.
    CpuToGpu,
    /// Device-local memory intended for GPU-only use.
    GpuOnly,
}

/// Buffer creation descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BufferDesc {
    /// Debug label.
    pub label: ResourceLabel,
    /// Size in bytes.
    pub size: u64,
    /// Usage flags.
    pub usage: BufferUsage,
    /// Memory placement.
    pub memory_location: MemoryLocation,
}

impl BufferDesc {
    /// Validates the descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError::InvalidInput`] when size is zero or no usage is set.
    pub fn validate(&self) -> Result<()> {
        if self.size == 0 {
            return Err(GfxError::InvalidInput(
                "buffer size must be greater than zero".to_string(),
            ));
        }
        if self.usage.is_empty() {
            return Err(GfxError::InvalidInput(
                "buffer usage must not be empty".to_string(),
            ));
        }
        Ok(())
    }
}

/// Texture dimensionality.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextureDimension {
    /// 2D texture.
    D2,
}

/// Texture creation descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextureDesc {
    /// Debug label.
    pub label: ResourceLabel,
    /// Width and height.
    pub size: Extent2d,
    /// Texture format.
    pub format: Format,
    /// Texture usage.
    pub usage: TextureUsage,
    /// Memory placement.
    pub memory_location: MemoryLocation,
    /// Texture dimension.
    pub dimension: TextureDimension,
}

impl TextureDesc {
    /// Validates the descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError::InvalidInput`] when no usage is set.
    pub fn validate(&self) -> Result<()> {
        if self.usage.is_empty() {
            return Err(GfxError::InvalidInput(
                "texture usage must not be empty".to_string(),
            ));
        }
        Ok(())
    }
}

/// Texture view descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextureViewDesc {
    /// Debug label.
    pub label: ResourceLabel,
    /// Source texture.
    pub texture: TextureId,
    /// View format.
    pub format: Format,
}

/// Texture data layout for uploads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextureDataLayout {
    /// Byte offset from the start of the source data.
    pub offset: u64,
    /// Bytes per image row.
    pub bytes_per_row: NonZeroU32,
    /// Number of rows in the source image.
    pub rows_per_image: NonZeroU32,
}

impl TextureDataLayout {
    /// Creates a validated data layout.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError::InvalidInput`] when row values are zero.
    pub fn new(offset: u64, bytes_per_row: u32, rows_per_image: u32) -> Result<Self> {
        let bytes_per_row = NonZeroU32::new(bytes_per_row).ok_or_else(|| {
            GfxError::InvalidInput("bytes_per_row must be greater than zero".to_string())
        })?;
        let rows_per_image = NonZeroU32::new(rows_per_image).ok_or_else(|| {
            GfxError::InvalidInput("rows_per_image must be greater than zero".to_string())
        })?;
        Ok(Self {
            offset,
            bytes_per_row,
            rows_per_image,
        })
    }
}

/// Sampler filtering mode.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FilterMode {
    /// Nearest neighbor filtering.
    #[default]
    Nearest,
    /// Linear filtering.
    Linear,
}

/// Sampler address mode.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AddressMode {
    /// Clamp coordinates to texture edges.
    #[default]
    ClampToEdge,
    /// Repeat texture coordinates.
    Repeat,
}

/// Sampler descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SamplerDesc {
    /// Debug label.
    pub label: ResourceLabel,
    /// Magnification filter.
    pub mag_filter: FilterMode,
    /// Minification filter.
    pub min_filter: FilterMode,
    /// U address mode.
    pub address_mode_u: AddressMode,
    /// V address mode.
    pub address_mode_v: AddressMode,
}

impl Default for SamplerDesc {
    fn default() -> Self {
        Self {
            label: None,
            mag_filter: FilterMode::Nearest,
            min_filter: FilterMode::Nearest,
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
        }
    }
}

/// Shader stage.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ShaderStage {
    /// Vertex stage.
    Vertex,
    /// Fragment stage.
    Fragment,
}

impl fmt::Display for ShaderStage {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Vertex => output.write_str("vertex"),
            Self::Fragment => output.write_str("fragment"),
        }
    }
}

/// Compiled shader bytes and entry point metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShaderBinary {
    /// Stage represented by this binary.
    pub stage: ShaderStage,
    /// Entry point name.
    pub entry_point: String,
    /// Backend shader code.
    pub code: ShaderCode,
}

impl ShaderBinary {
    /// Creates a SPIR-V shader binary.
    #[must_use]
    pub fn spirv(stage: ShaderStage, entry_point: impl Into<String>, spirv: Vec<u32>) -> Self {
        Self {
            stage,
            entry_point: entry_point.into(),
            code: ShaderCode::Spirv(spirv),
        }
    }

    /// Creates an HLSL shader binary.
    #[must_use]
    pub fn hlsl(stage: ShaderStage, entry_point: impl Into<String>, source: String) -> Self {
        Self {
            stage,
            entry_point: entry_point.into(),
            code: ShaderCode::Hlsl(source),
        }
    }

    /// Creates a D3D bytecode shader binary.
    #[must_use]
    pub fn dx_bytecode(
        stage: ShaderStage,
        entry_point: impl Into<String>,
        bytecode: Vec<u8>,
    ) -> Self {
        Self {
            stage,
            entry_point: entry_point.into(),
            code: ShaderCode::DxBytecode(bytecode),
        }
    }

    /// Creates an MSL shader binary.
    #[must_use]
    pub fn msl(stage: ShaderStage, entry_point: impl Into<String>, source: String) -> Self {
        Self {
            stage,
            entry_point: entry_point.into(),
            code: ShaderCode::Msl(source),
        }
    }

    /// Returns SPIR-V words when this binary targets Vulkan.
    #[must_use]
    pub fn spirv_words(&self) -> Option<&[u32]> {
        match &self.code {
            ShaderCode::Spirv(words) => Some(words),
            ShaderCode::Hlsl(_) | ShaderCode::DxBytecode(_) | ShaderCode::Msl(_) => None,
        }
    }

    /// Returns true when the backend code payload is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match &self.code {
            ShaderCode::Spirv(words) => words.is_empty(),
            ShaderCode::Hlsl(source) | ShaderCode::Msl(source) => source.is_empty(),
            ShaderCode::DxBytecode(bytecode) => bytecode.is_empty(),
        }
    }
}

/// Backend-specific shader code.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShaderCode {
    /// SPIR-V words for Vulkan.
    Spirv(Vec<u32>),
    /// HLSL source for DX12 compilation.
    Hlsl(String),
    /// D3D compiled shader bytecode.
    DxBytecode(Vec<u8>),
    /// Metal Shading Language source.
    Msl(String),
}

/// Shader module descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShaderModuleDesc {
    /// Debug label.
    pub label: ResourceLabel,
    /// Compiled shader data.
    pub binary: ShaderBinary,
}

impl ShaderModuleDesc {
    /// Validates the descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError::Shader`] when the shader code payload is empty.
    pub fn validate(&self) -> Result<()> {
        if self.binary.is_empty() {
            return Err(GfxError::Shader(
                "shader code must not be empty".to_string(),
            ));
        }
        Ok(())
    }
}

/// Vertex attribute format.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VertexFormat {
    /// Two 32-bit floats.
    Float32x2,
    /// Three 32-bit floats.
    Float32x3,
    /// Four 32-bit floats.
    Float32x4,
}

impl VertexFormat {
    /// Returns the format size in bytes.
    #[must_use]
    pub const fn size(self) -> u32 {
        match self {
            Self::Float32x2 => 8,
            Self::Float32x3 => 12,
            Self::Float32x4 => 16,
        }
    }
}

/// Single vertex attribute description.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VertexAttributeDesc {
    /// Shader location.
    pub location: u32,
    /// Byte offset within the vertex.
    pub offset: u32,
    /// Attribute format.
    pub format: VertexFormat,
}

/// Vertex buffer layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VertexBufferLayoutDesc {
    /// Vertex stride in bytes.
    pub stride: u32,
    /// Attributes in the vertex.
    pub attributes: Vec<VertexAttributeDesc>,
}

/// Color blend mode.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum BlendMode {
    /// No blending.
    #[default]
    Replace,
    /// Straight alpha blending.
    Alpha,
    /// Premultiplied alpha blending.
    PremultipliedAlpha,
    /// Preserve color-over behavior while accumulating alpha.
    AdditiveAlpha,
}

/// Primitive topology used by a graphics pipeline.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PrimitiveTopology {
    /// Independent triangles.
    #[default]
    TriangleList,
    /// Connected triangle strip.
    TriangleStrip,
}

/// Color attachment descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ColorAttachmentDesc {
    /// Attachment format.
    pub format: Format,
}

/// Render pass descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderPassDesc {
    /// Debug label.
    pub label: ResourceLabel,
    /// Color attachment.
    pub color_attachment: ColorAttachmentDesc,
}

/// Render pipeline descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderPipelineDesc {
    /// Debug label.
    pub label: ResourceLabel,
    /// Vertex shader module.
    pub vertex_shader: ShaderModuleId,
    /// Vertex entry point.
    pub vertex_entry_point: String,
    /// Fragment shader module.
    pub fragment_shader: ShaderModuleId,
    /// Fragment entry point.
    pub fragment_entry_point: String,
    /// Vertex layouts.
    pub vertex_buffers: Vec<VertexBufferLayoutDesc>,
    /// Render pass.
    pub render_pass: RenderPassId,
    /// Optional pipeline layout.
    pub pipeline_layout: Option<PipelineLayoutId>,
    /// Output color format.
    pub color_format: Format,
    /// Blend mode.
    pub blend_mode: BlendMode,
    /// Primitive topology.
    pub primitive_topology: PrimitiveTopology,
}

impl RenderPipelineDesc {
    /// Validates a render pipeline descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError::InvalidInput`] when an entry point is empty.
    pub fn validate(&self) -> Result<()> {
        if self.vertex_entry_point.is_empty() {
            return Err(GfxError::InvalidInput(
                "vertex_entry_point must not be empty".to_string(),
            ));
        }
        if self.fragment_entry_point.is_empty() {
            return Err(GfxError::InvalidInput(
                "fragment_entry_point must not be empty".to_string(),
            ));
        }
        Ok(())
    }
}

/// Render pass load operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoadOp<T> {
    /// Clear to a value.
    Clear(T),
    /// Preserve existing content.
    Load,
}

/// Clear color for a render pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClearColor {
    /// Red channel.
    pub red: f32,
    /// Green channel.
    pub green: f32,
    /// Blue channel.
    pub blue: f32,
    /// Alpha channel.
    pub alpha: f32,
}

impl Default for ClearColor {
    fn default() -> Self {
        Self {
            red: 0.0,
            green: 0.0,
            blue: 0.0,
            alpha: 1.0,
        }
    }
}

/// Per-frame render instructions for the compatibility triangle path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrawTriangleDesc {
    /// Clear color.
    pub clear_color: ClearColor,
}

/// Command encoder descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandEncoderDesc {
    /// Debug label.
    pub label: ResourceLabel,
}

/// Render pass begin descriptor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeginRenderPassDesc {
    /// Render pass.
    pub render_pass: RenderPassId,
    /// Color target.
    pub target: RenderTarget,
    /// Clear behavior.
    pub color_load_op: LoadOp<ClearColor>,
}

/// A color target for a render pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RenderTarget {
    /// A swapchain image.
    Swapchain {
        /// Swapchain image target.
        swapchain: SwapchainId,
        /// Image index in the swapchain.
        image_index: u32,
    },
    /// A regular texture view.
    TextureView(TextureViewId),
}

/// Draw call descriptor.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawDesc {
    /// Render pass begin parameters.
    pub pass: BeginRenderPassDesc,
    /// Render pipeline.
    pub pipeline: RenderPipelineId,
    /// Resource sets bound before drawing.
    pub resource_sets: Vec<ResourceSetId>,
    /// Vertex count.
    pub vertex_count: u32,
    /// First vertex.
    pub first_vertex: u32,
    /// Instance count.
    pub instance_count: u32,
    /// First instance.
    pub first_instance: u32,
    /// Optional scissor rectangle in target pixels.
    pub scissor: Option<ScissorRect>,
}

/// One draw step inside a render pass.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawStepDesc {
    /// Render pipeline.
    pub pipeline: RenderPipelineId,
    /// Resource sets bound before drawing.
    pub resource_sets: Vec<ResourceSetId>,
    /// Vertex count.
    pub vertex_count: u32,
    /// First vertex.
    pub first_vertex: u32,
    /// Instance count.
    pub instance_count: u32,
    /// First instance.
    pub first_instance: u32,
    /// Optional scissor rectangle in target pixels.
    pub scissor: Option<ScissorRect>,
}

/// Integer scissor rectangle in render target coordinates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScissorRect {
    /// Left edge in pixels.
    pub x: u32,
    /// Top edge in pixels.
    pub y: u32,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
}

impl ScissorRect {
    /// Returns whether the scissor covers at least one pixel.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// Buffer write descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BufferWriteDesc {
    /// Target buffer.
    pub buffer: BufferId,
    /// Destination byte offset.
    pub offset: u64,
}

/// Texture write descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextureWriteDesc {
    /// Target texture.
    pub texture: TextureId,
    /// Data layout.
    pub layout: TextureDataLayout,
    /// Target origin.
    pub origin: Origin2d,
    /// Target size.
    pub size: Extent2d,
}

/// Backend resource statistics.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResourceStats {
    /// Live buffers.
    pub buffers: usize,
    /// Live textures.
    pub textures: usize,
    /// Live texture views.
    pub texture_views: usize,
    /// Live samplers.
    pub samplers: usize,
    /// Live resource set layouts.
    pub resource_set_layouts: usize,
    /// Live resource sets.
    pub resource_sets: usize,
    /// Live pipeline layouts.
    pub pipeline_layouts: usize,
    /// Live shader modules.
    pub shader_modules: usize,
    /// Live render passes.
    pub render_passes: usize,
    /// Live render pipelines.
    pub render_pipelines: usize,
    /// Live command encoders.
    pub command_encoders: usize,
    /// Live tracked submissions.
    pub submissions: usize,
    /// Live surfaces.
    pub surfaces: usize,
    /// Live swapchains.
    pub swapchains: usize,
    /// Allocated GPU bytes known to the backend.
    pub allocated_bytes: u64,
    /// Reserved GPU bytes known to the backend.
    pub reserved_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extent_rejects_zero_dimensions() {
        assert!(Extent2d::new(0, 1).is_err());
        assert!(Extent2d::new(1, 0).is_err());
        assert!(Extent2d::new(1, 1).is_ok());
    }

    #[test]
    fn resource_id_encodes_generation_and_index() {
        let id = BufferId::from_parts(7, 9);

        assert_eq!(id.index(), 7);
        assert_eq!(id.generation(), 9);
    }

    #[test]
    fn submission_id_encodes_generation_and_index() {
        let id = SubmissionId::from_parts(11, 13);

        assert_eq!(id.index(), 11);
        assert_eq!(id.generation(), 13);
    }

    #[test]
    fn submission_status_reports_finished_state() {
        assert!(!SubmissionStatus::Pending.is_finished());
        assert!(SubmissionStatus::Complete.is_finished());
        assert!(SubmissionStatus::Failed("device lost".to_string()).is_finished());
    }

    #[test]
    fn async_capabilities_default_to_owner_thread_sync() {
        assert_eq!(
            GfxAsyncCapabilities::default(),
            GfxAsyncCapabilities {
                threading_mode: GfxThreadingMode::OwnerThreadOnly,
                async_submission: false,
                async_wait: false,
                async_presentation: false,
                partial_presentation: false,
            }
        );
    }

    #[test]
    fn buffer_desc_rejects_zero_size() {
        let descriptor = BufferDesc {
            label: None,
            size: 0,
            usage: BufferUsage::VERTEX,
            memory_location: MemoryLocation::CpuToGpu,
        };

        assert!(descriptor.validate().is_err());
    }

    #[test]
    fn texture_desc_rejects_empty_usage() {
        let descriptor = TextureDesc {
            label: None,
            size: Extent2d::new(1, 1).expect("test dimensions are non-zero"),
            format: Format::Rgba8Unorm,
            usage: TextureUsage::empty(),
            memory_location: MemoryLocation::GpuOnly,
            dimension: TextureDimension::D2,
        };

        assert!(descriptor.validate().is_err());
    }

    #[test]
    fn pipeline_desc_rejects_empty_entry_points() {
        let descriptor = RenderPipelineDesc {
            label: None,
            vertex_shader: ShaderModuleId::from_parts(1, 1),
            vertex_entry_point: String::new(),
            fragment_shader: ShaderModuleId::from_parts(2, 1),
            fragment_entry_point: "fs_main".to_string(),
            vertex_buffers: Vec::new(),
            render_pass: RenderPassId::from_parts(3, 1),
            pipeline_layout: None,
            color_format: Format::Bgra8Unorm,
            blend_mode: BlendMode::Replace,
            primitive_topology: PrimitiveTopology::TriangleList,
        };

        assert!(descriptor.validate().is_err());
    }

    #[test]
    fn resource_set_layout_rejects_duplicate_bindings() {
        let descriptor = ResourceSetLayoutDesc {
            label: None,
            entries: vec![
                ResourceSetLayoutEntry {
                    binding: 0,
                    binding_type: ResourceBindingType::UniformBuffer,
                    stages: ShaderStages::VERTEX,
                },
                ResourceSetLayoutEntry {
                    binding: 0,
                    binding_type: ResourceBindingType::Sampler,
                    stages: ShaderStages::FRAGMENT,
                },
            ],
        };

        assert!(descriptor.validate().is_err());
    }

    #[test]
    fn resource_set_rejects_mismatched_binding_type() {
        let layout = ResourceSetLayoutDesc {
            label: None,
            entries: vec![ResourceSetLayoutEntry {
                binding: 0,
                binding_type: ResourceBindingType::SampledTexture,
                stages: ShaderStages::FRAGMENT,
            }],
        };
        let descriptor = ResourceSetDesc {
            label: None,
            layout: ResourceSetLayoutId::from_parts(1, 1),
            bindings: vec![ResourceBinding {
                binding: 0,
                resource: ResourceBindingResource::Sampler(SamplerBinding {
                    sampler: SamplerId::from_parts(2, 1),
                }),
            }],
        };

        assert!(descriptor.validate_against(&layout).is_err());
    }

    #[test]
    fn resource_set_accepts_unordered_bindings() {
        let layout = ResourceSetLayoutDesc {
            label: None,
            entries: vec![
                ResourceSetLayoutEntry {
                    binding: 0,
                    binding_type: ResourceBindingType::UniformBuffer,
                    stages: ShaderStages::VERTEX,
                },
                ResourceSetLayoutEntry {
                    binding: 2,
                    binding_type: ResourceBindingType::Sampler,
                    stages: ShaderStages::FRAGMENT,
                },
            ],
        };
        let descriptor = ResourceSetDesc {
            label: None,
            layout: ResourceSetLayoutId::from_parts(1, 1),
            bindings: vec![
                ResourceBinding {
                    binding: 2,
                    resource: ResourceBindingResource::Sampler(SamplerBinding {
                        sampler: SamplerId::from_parts(2, 1),
                    }),
                },
                ResourceBinding {
                    binding: 0,
                    resource: ResourceBindingResource::Buffer(BufferBinding {
                        buffer: BufferId::from_parts(3, 1),
                        offset: 0,
                        size: 64,
                        stride: None,
                    }),
                },
            ],
        };

        assert!(descriptor.validate_against(&layout).is_ok());
    }
}
