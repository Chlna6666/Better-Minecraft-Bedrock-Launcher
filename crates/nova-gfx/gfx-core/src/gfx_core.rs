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
//! use gfx_core::{BufferDescriptor, BufferId, ResourceDevice, Result};
//!
//! fn create_buffer<D>(device: &mut D, desc: &BufferDescriptor) -> Result<BufferId>
//! where
//!     D: ResourceDevice,
//! {
//!     device.create_buffer(desc)
//! }
//! ```

mod backend;
mod buffer_upload;
mod memory;
mod texture_copy;
pub use texture_copy::TextureCopy;

pub use buffer_upload::{BufferUploadBatch, BufferUploadStats, BufferWrite};
pub use memory::{
    DeviceMemoryBudget, MemoryAccounting, MemoryArchitecture, MemoryBudget, MemoryCompactReport,
};

use std::{
    borrow::Cow,
    fmt,
    future::Future,
    hash::{Hash, Hasher},
    marker::PhantomData,
    num::NonZeroU32,
    path::PathBuf,
    pin::Pin,
};

use bitflags::bitflags;
use smallvec::SmallVec;
use thiserror::Error as ThisError;

pub use backend::{
    AsyncCommandDevice, AsyncDevice, AsyncDiagnosticsDevice, AsyncPipelineDevice,
    AsyncPresentationDevice, AsyncResourceDevice, AsyncSurfaceDevice, Backend, BackendDiagnostics,
    BackendPipelines, BackendPresentationCompat, BackendQueue, BackendResources, BackendSurface,
    CommandDevice, Device, DiagnosticsDevice, ExtensionDevice, PipelineDevice, PresentationDevice,
    PresentationFrame, PresentationTimings, ResourceDevice, SharedDevice, SubmissionDevice,
    SurfaceDevice, TextureTransferDevice,
};

/// Convenience result type used by nova-gfx crates.
pub type Result<T> = std::result::Result<T, Error>;

/// Runtime-neutral boxed future returned by nova-gfx async interfaces.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

/// Error type for backend-neutral validation and backend-provided failures.
#[derive(Debug, ThisError)]
pub enum Error {
    /// A required graphics capability or resource was not available.
    #[error("graphics resource is unavailable: {0}")]
    Unavailable(String),
    /// The native presentation surface changed and the current frame must be retried.
    #[error("graphics presentation surface is outdated")]
    SurfaceOutdated,
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
pub enum ThreadingMode {
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
pub struct AsyncCapabilities {
    /// How the device can be accessed from multiple threads.
    pub threading_mode: ThreadingMode,
    /// Device can return submission handles without blocking for completion.
    pub async_submission: bool,
    /// Device can wait for submission completion asynchronously.
    pub async_wait: bool,
    /// Presentation helper can be submitted through an async/deferred path.
    pub async_presentation: bool,
}

/// Actual presentation facilities available for one live swapchain.
///
/// Query these after surface creation and after swapchain recreation. They are distinct from
/// adapter [`BackendCapabilities`], device [`AsyncCapabilities`] and requested [`SurfaceConfig`]
/// policy. Default capabilities select portable full presentation without a native ready wake.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PresentationCapabilities {
    /// Native presentation consumes damage while keeping every rotating backbuffer coherent.
    pub partial_presentation: bool,
    /// A one-shot ready callback can wake the owner when this swapchain accepts another frame.
    pub frame_ready_notification: bool,
}

impl Default for AsyncCapabilities {
    fn default() -> Self {
        Self {
            threading_mode: ThreadingMode::OwnerThreadOnly,
            async_submission: false,
            async_wait: false,
            async_presentation: false,
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
    /// Returns [`Error::InvalidInput`] when either dimension is zero.
    pub fn new(width: u32, height: u32) -> Result<Self> {
        let width = NonZeroU32::new(width)
            .ok_or_else(|| Error::InvalidInput("width must be greater than zero".to_string()))?;
        let height = NonZeroU32::new(height)
            .ok_or_else(|| Error::InvalidInput("height must be greater than zero".to_string()))?;
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
    /// Direct3D 11 backend (feature level 11.0 or newer).
    Dx11,
    /// Direct3D 12 backend.
    Dx12,
    /// Metal backend.
    Metal,
    /// OpenGL backend.
    OpenGl,
    /// WebGL backend.
    WebGl,
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

/// Preferred GPU power class when a backend can choose between adapters.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum PowerPreference {
    /// Prefer integrated or low-power adapters.
    #[default]
    LowPower,
    /// Prefer the highest-performance adapter.
    HighPerformance,
}

/// Logical device creation descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceDescriptor {
    /// Application name reported to backends.
    pub application_name: String,
    /// Optional adapter name requested by the platform layer.
    pub adapter_name: Option<String>,
    /// Preferred GPU power class.
    pub power_preference: PowerPreference,
    /// Optional persistent backend pipeline-cache root.
    ///
    /// Backends place adapter-specific cache files below this directory. `None` keeps
    /// pipeline caches process-local only.
    pub pipeline_cache_dir: Option<PathBuf>,
}

impl Default for DeviceDescriptor {
    fn default() -> Self {
        Self {
            application_name: "nova-gfx".to_string(),
            adapter_name: None,
            power_preference: PowerPreference::LowPower,
            pipeline_cache_dir: None,
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
pub struct QueueDescriptor {
    /// Queue role.
    pub kind: QueueKind,
    /// Backend queue family index.
    pub family_index: u32,
}

/// Color formats supported by the phase-1 API.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Format {
    /// Single 8-bit unsigned normalized red channel, sampled in the range 0..=1.
    /// Suitable for coverage masks without allocating unused color channels.
    R8Unorm,
    /// 8-bit BGRA unsigned normalized format.
    Bgra8Unorm,
    /// 8-bit BGRA unsigned normalized sRGB format.
    Bgra8UnormSrgb,
    /// 8-bit RGBA unsigned normalized format.
    Rgba8Unorm,
    /// 8-bit RGBA unsigned normalized sRGB format.
    Rgba8UnormSrgb,
    /// 32-bit floating-point depth format.
    Depth32Float,
}

impl Format {
    /// Returns whether this format uses sRGB conversion.
    #[must_use]
    pub const fn is_srgb(self) -> bool {
        matches!(self, Self::Bgra8UnormSrgb | Self::Rgba8UnormSrgb)
    }

    /// Returns the number of bytes occupied by one pixel.
    #[must_use]
    pub const fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::R8Unorm => 1,
            Self::Bgra8Unorm
            | Self::Bgra8UnormSrgb
            | Self::Rgba8Unorm
            | Self::Rgba8UnormSrgb
            | Self::Depth32Float => 4,
        }
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
    /// Straight alpha, multiplied by the compositor after presentation.
    Postmultiplied,
    /// Inherit alpha compositing behavior from the native window system.
    Inherit,
}

/// Native surface descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SurfaceDescriptor {
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
    /// Returns [`Error::InvalidInput`] when width or height is zero.
    pub fn new(width: u32, height: u32, format: Format) -> Result<Self> {
        Ok(Self {
            size: Extent2d::new(width, height)?,
            format,
            present_mode: PresentMode::Fifo,
            alpha_mode: CompositeAlphaMode::Auto,
        })
    }
}

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

impl BufferBinding {
    /// Validates that the binding range is contained within a buffer.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] when the range is empty, overflows, or
    /// exceeds the target buffer size.
    pub fn validate_against(self, buffer_size: u64) -> Result<()> {
        if self.size == 0 {
            return Err(Error::InvalidInput(
                "buffer binding size must be non-zero".to_string(),
            ));
        }
        let end = self
            .offset
            .checked_add(self.size)
            .ok_or_else(|| Error::InvalidInput("buffer binding range overflow".to_string()))?;
        if end > buffer_size {
            return Err(Error::InvalidInput(format!(
                "buffer binding range {}..{} exceeds buffer size {}",
                self.offset, end, buffer_size
            )));
        }
        Ok(())
    }
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
    pub resource: BindingResource,
}

/// A resource binding payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BindingResource {
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
    pub const fn matches(self, binding: &BindingResource) -> bool {
        matches!(
            (self, binding),
            (
                Self::UniformBuffer | Self::StorageBuffer,
                BindingResource::Buffer(_)
            ) | (Self::SampledTexture, BindingResource::Texture(_))
                | (Self::Sampler, BindingResource::Sampler(_))
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
pub struct ResourceSetLayoutDescriptor {
    /// Debug label.
    pub label: ResourceLabel,
    /// Layout entries.
    pub entries: Vec<ResourceSetLayoutEntry>,
}

impl ResourceSetLayoutDescriptor {
    /// Validates the descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] when the layout is empty or contains
    /// duplicate bindings.
    pub fn validate(&self) -> Result<()> {
        if self.entries.is_empty() {
            return Err(Error::InvalidInput(
                "resource set layout must contain at least one entry".to_string(),
            ));
        }
        let mut bindings = std::collections::BTreeSet::new();
        for entry in &self.entries {
            if entry.stages.is_empty() {
                return Err(Error::InvalidInput(
                    "resource set layout entry must be visible to at least one shader stage"
                        .to_string(),
                ));
            }
            if !bindings.insert(entry.binding) {
                return Err(Error::InvalidInput(format!(
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
pub struct ResourceSetDescriptor {
    /// Debug label.
    pub label: ResourceLabel,
    /// Layout this resource set conforms to.
    pub layout: ResourceSetLayoutId,
    /// Concrete bindings.
    pub bindings: Vec<ResourceBinding>,
}

impl ResourceSetDescriptor {
    /// Validates the descriptor against a layout.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] when a binding is missing or invalid.
    pub fn validate_against(&self, layout: &ResourceSetLayoutDescriptor) -> Result<()> {
        layout.validate()?;
        if self.bindings.len() != layout.entries.len() {
            return Err(Error::InvalidInput(format!(
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
                return Err(Error::InvalidInput(format!(
                    "duplicate resource binding slot {}",
                    binding.binding
                )));
            }
        }
        for entry in &layout.entries {
            let Some(binding) = bindings_by_slot.get(&entry.binding) else {
                return Err(Error::InvalidInput(format!(
                    "missing resource binding slot {}",
                    entry.binding
                )));
            };
            if !entry.binding_type.matches(binding) {
                return Err(Error::InvalidInput(format!(
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
pub struct PipelineLayoutDescriptor {
    /// Debug label.
    pub label: ResourceLabel,
    /// Resource set layouts used by this pipeline.
    pub resource_set_layouts: Vec<ResourceSetLayoutId>,
}

impl PipelineLayoutDescriptor {
    /// Validates the descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] when the layout is empty.
    pub fn validate(&self) -> Result<()> {
        if self.resource_set_layouts.is_empty() {
            return Err(Error::InvalidInput(
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
    /// CPU-visible memory intended for GPU readback.
    GpuToCpu,
    /// Device-local memory intended for GPU-only use.
    GpuOnly,
}

/// Buffer creation descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BufferDescriptor {
    /// Debug label.
    pub label: ResourceLabel,
    /// Size in bytes.
    pub size: u64,
    /// Usage flags.
    pub usage: BufferUsage,
    /// Memory placement.
    pub memory_location: MemoryLocation,
}

impl BufferDescriptor {
    /// Validates the descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] when size is zero or no usage is set.
    pub fn validate(&self) -> Result<()> {
        if self.size == 0 {
            return Err(Error::InvalidInput(
                "buffer size must be greater than zero".to_string(),
            ));
        }
        if self.usage.is_empty() {
            return Err(Error::InvalidInput(
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
pub struct TextureDescriptor {
    /// Debug label.
    pub label: ResourceLabel,
    /// Width and height.
    pub size: Extent2d,
    /// Number of mip levels, including level zero.
    pub mip_level_count: u32,
    /// Texture format.
    pub format: Format,
    /// Texture usage.
    pub usage: TextureUsage,
    /// Memory placement.
    pub memory_location: MemoryLocation,
    /// Texture dimension.
    pub dimension: TextureDimension,
}

impl TextureDescriptor {
    /// Logical texel bytes across all mip levels, excluding native tiling,
    /// alignment, metadata and driver overhead. Use only with a valid descriptor.
    #[must_use]
    pub fn byte_size(&self) -> u64 {
        (0..self.mip_level_count).fold(0_u64, |total, mip| {
            total.saturating_add(
                u64::from((self.size.width() >> mip).max(1))
                    .saturating_mul(u64::from((self.size.height() >> mip).max(1)))
                    .saturating_mul(u64::from(self.format.bytes_per_pixel())),
            )
        })
    }
    /// Validates the descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] when no usage is set or the mip count exceeds the
    /// dimensions' complete mip chain.
    pub fn validate(&self) -> Result<()> {
        if self.usage.is_empty() {
            return Err(Error::InvalidInput(
                "texture usage must not be empty".to_string(),
            ));
        }
        let largest_dimension = self.size.width().max(self.size.height());
        let max_mip_level_count = u32::BITS - largest_dimension.leading_zeros();
        if self.mip_level_count == 0 || self.mip_level_count > max_mip_level_count {
            return Err(Error::InvalidInput(format!(
                "texture mip level count {} is outside 1..={max_mip_level_count}",
                self.mip_level_count
            )));
        }
        Ok(())
    }

    /// Returns the extent of a mip level.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] when the descriptor is invalid or `mip_level` is out
    /// of range.
    pub fn mip_extent(&self, mip_level: u32) -> Result<Extent2d> {
        self.validate()?;
        if mip_level >= self.mip_level_count {
            return Err(Error::InvalidInput(format!(
                "texture mip level {mip_level} exceeds level count {}",
                self.mip_level_count
            )));
        }
        Extent2d::new(
            (self.size.width() >> mip_level).max(1),
            (self.size.height() >> mip_level).max(1),
        )
    }
}

/// Texture view descriptor.
///
/// Sampled views may cover any non-empty mip range. Current backend attachment views select only
/// mip level zero.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextureViewDescriptor {
    /// Debug label.
    pub label: ResourceLabel,
    /// Source texture.
    pub texture: TextureId,
    /// First mip level in the view.
    pub base_mip_level: u32,
    /// Number of mip levels in the view.
    pub mip_level_count: u32,
    /// View format.
    pub format: Format,
}

impl TextureViewDescriptor {
    /// Validates this view against its texture descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] when the texture descriptor is invalid or the view range
    /// is empty or exceeds the texture's mip range.
    pub fn validate_against(&self, texture: &TextureDescriptor) -> Result<()> {
        texture.validate()?;
        let end = self
            .base_mip_level
            .checked_add(self.mip_level_count)
            .ok_or_else(|| Error::InvalidInput("texture view mip range overflow".to_string()))?;
        if self.mip_level_count == 0 || end > texture.mip_level_count {
            return Err(Error::InvalidInput(format!(
                "texture view mip range {}..{end} exceeds texture level count {}",
                self.base_mip_level, texture.mip_level_count
            )));
        }
        Ok(())
    }
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
    /// Returns [`Error::InvalidInput`] when row values are zero.
    pub fn new(offset: u64, bytes_per_row: u32, rows_per_image: u32) -> Result<Self> {
        let bytes_per_row = NonZeroU32::new(bytes_per_row).ok_or_else(|| {
            Error::InvalidInput("bytes_per_row must be greater than zero".to_string())
        })?;
        let rows_per_image = NonZeroU32::new(rows_per_image).ok_or_else(|| {
            Error::InvalidInput("rows_per_image must be greater than zero".to_string())
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
pub struct SamplerDescriptor {
    /// Debug label.
    pub label: ResourceLabel,
    /// Magnification filter.
    pub mag_filter: FilterMode,
    /// Minification filter.
    pub min_filter: FilterMode,
    /// Filter between mip levels.
    pub mipmap_filter: FilterMode,
    /// Request anisotropic filtering when supported; otherwise use the configured filter modes.
    pub anisotropic: bool,
    /// U address mode.
    pub address_mode_u: AddressMode,
    /// V address mode.
    pub address_mode_v: AddressMode,
}

impl Default for SamplerDescriptor {
    fn default() -> Self {
        Self {
            label: None,
            mag_filter: FilterMode::Nearest,
            min_filter: FilterMode::Nearest,
            mipmap_filter: FilterMode::Nearest,
            anisotropic: false,
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

    /// Creates a borrowed static D3D bytecode shader binary without copying the embedded bytes.
    #[must_use]
    pub fn dx_bytecode_static(
        stage: ShaderStage,
        entry_point: impl Into<String>,
        bytecode: &'static [u8],
    ) -> Self {
        Self {
            stage,
            entry_point: entry_point.into(),
            code: ShaderCode::DxBytecodeStatic(bytecode),
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

    /// Creates a borrowed precompiled Metal library shader binary.
    #[must_use]
    pub fn metallib_static(
        stage: ShaderStage,
        entry_point: impl Into<String>,
        library: &'static [u8],
    ) -> Self {
        Self {
            stage,
            entry_point: entry_point.into(),
            code: ShaderCode::MetallibStatic(library),
        }
    }

    /// Returns SPIR-V words when this binary targets Vulkan.
    #[must_use]
    pub fn spirv_words(&self) -> Option<&[u32]> {
        match &self.code {
            ShaderCode::Spirv(words) => Some(words),
            ShaderCode::Hlsl(_)
            | ShaderCode::Glsl(_)
            | ShaderCode::DxBytecode(_)
            | ShaderCode::DxBytecodeStatic(_)
            | ShaderCode::Msl(_)
            | ShaderCode::MetallibStatic(_) => None,
        }
    }

    /// Returns true when the backend code payload is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match &self.code {
            ShaderCode::Spirv(words) => words.is_empty(),
            ShaderCode::Hlsl(source) | ShaderCode::Msl(source) => source.is_empty(),
            ShaderCode::Glsl(shader) => shader.source.is_empty(),
            ShaderCode::DxBytecode(bytecode) => bytecode.is_empty(),
            ShaderCode::DxBytecodeStatic(bytecode) => bytecode.is_empty(),
            ShaderCode::MetallibStatic(library) => library.is_empty(),
        }
    }
}

/// Backend-specific shader code.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ShaderCode {
    /// Desktop GLSL and its WGSL resource reflection for native OpenGL.
    Glsl(GlslShader),
    /// SPIR-V words for Vulkan.
    Spirv(Vec<u32>),
    /// HLSL source for DX12 compilation.
    Hlsl(String),
    /// Owned D3D compiled shader bytecode.
    DxBytecode(Vec<u8>),
    /// Borrowed D3D bytecode embedded in the executable.
    DxBytecodeStatic(&'static [u8]),
    /// Metal Shading Language source.
    Msl(String),
    /// Borrowed precompiled Metal library embedded in the executable.
    MetallibStatic(&'static [u8]),
}

/// Backend shader code produced ahead of time by a build script.
///
/// A build script translates WGSL and, when the build host can run the platform
/// shader compiler, compiles it all the way to backend bytecode. Embedding the
/// result lets a renderer create shader modules without running a shader compiler
/// while the application starts.
///
/// The variant a build emits depends on the target backend and on whether the
/// build host was able to precompile:
///
/// - [`Self::DxBytecode`] for DX12 when the Direct3D compiler ran at build time.
/// - [`Self::Hlsl`] for DX12 otherwise; the DX12 backend then compiles it when a
///   renderer is created, because that compiler only exists on Windows.
/// - [`Self::SpirvBytes`] for Vulkan.
/// - [`Self::Metallib`] for Metal production builds.
#[derive(Clone, Copy, Debug)]
pub enum EmbeddedShader {
    /// Build-translated desktop GLSL. The native driver compiles and links this source.
    Glsl {
        /// Desktop GLSL source with WGSL coordinate conventions preserved.
        source: &'static str,
        /// Uniform/storage block names and their original group-zero binding slots.
        buffers: &'static [GlslBufferBinding],
        /// Combined GLSL sampler names and original texture/sampler slots.
        textures: &'static [GlslTextureBinding],
    },
    /// DX12 HLSL source, compiled by the backend when a renderer is created.
    Hlsl(&'static str),
    /// DX12 bytecode already compiled by `D3DCompile` at build time.
    DxBytecode(&'static [u8]),
    /// Vulkan SPIR-V words encoded as little-endian bytes.
    SpirvBytes(&'static [u8]),
    /// Metal Shading Language source for explicit runtime compilation tools.
    Msl(&'static str),
    /// Precompiled Metal library produced by the offline Metal toolchain.
    Metallib(&'static [u8]),
}

impl EmbeddedShader {
    /// Builds the [`ShaderBinary`] this embedded artifact describes.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Shader`] when an encoded SPIR-V payload is not a whole
    /// number of 32-bit words.
    pub fn to_binary(self, stage: ShaderStage, entry_point: &str) -> Result<ShaderBinary> {
        Ok(match self {
            Self::Glsl {
                source,
                buffers,
                textures,
            } => ShaderBinary {
                stage,
                entry_point: entry_point.into(),
                code: ShaderCode::Glsl(GlslShader {
                    source: Cow::Borrowed(source),
                    buffers: buffers.to_vec(),
                    textures: textures.to_vec(),
                }),
            },
            Self::Hlsl(source) => ShaderBinary::hlsl(stage, entry_point, source.to_string()),
            Self::DxBytecode(bytecode) => {
                ShaderBinary::dx_bytecode_static(stage, entry_point, bytecode)
            }
            Self::SpirvBytes(bytes) => ShaderBinary::spirv(
                stage,
                entry_point,
                decode_spirv_words(bytes).ok_or_else(|| {
                    Error::Shader(format!(
                        "embedded SPIR-V for `{entry_point}` is not a whole number of words"
                    ))
                })?,
            ),
            Self::Msl(source) => ShaderBinary::msl(stage, entry_point, source.to_string()),
            Self::Metallib(library) => ShaderBinary::metallib_static(stage, entry_point, library),
        })
    }
}

/// Desktop GLSL produced from one WGSL entry point.
///
/// Native OpenGL links reflected resource names to compact per-program slots. This keeps
/// WGSL binding numbers independent of a driver's uniform/storage binding limits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GlslShader {
    /// GLSL source compiled by the installed native OpenGL driver.
    pub source: Cow<'static, str>,
    /// Active uniform and read-only storage block reflection.
    pub buffers: Vec<GlslBufferBinding>,
    /// Combined texture/sampler reflection required by desktop GLSL.
    pub textures: Vec<GlslTextureBinding>,
}

/// Original group-zero resource slot for a GLSL buffer block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GlslBufferBinding {
    /// Linked block name generated by Naga; never inferred from a user variable name.
    pub name: Cow<'static, str>,
    /// WGSL group-zero binding number.
    pub binding: u32,
    /// Whether this block is uniform or read-only storage.
    pub kind: ResourceBindingType,
}

/// Original resources combined into one desktop GLSL sampler uniform.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GlslTextureBinding {
    /// Linked sampler uniform name generated by Naga.
    pub name: Cow<'static, str>,
    /// WGSL group-zero texture binding number.
    pub texture: u32,
    /// WGSL sampler binding number, absent for texture-load-only access.
    pub sampler: Option<u32>,
}

/// Decodes little-endian SPIR-V bytes into words, or `None` when the payload is
/// not word aligned.
#[cfg(test)]
mod embedded_shader_tests {
    use super::*;

    #[test]
    fn embedded_dxbc_keeps_the_static_payload_borrowed() {
        static DXBC: &[u8] = b"DXBC-test";
        let binary = EmbeddedShader::DxBytecode(DXBC)
            .to_binary(ShaderStage::Vertex, "vs_main")
            .expect("embedded DXBC should build a shader binary");
        let ShaderCode::DxBytecodeStatic(actual) = binary.code else {
            panic!("embedded DXBC should stay borrowed");
        };
        assert!(core::ptr::eq(actual.as_ptr(), DXBC.as_ptr()));
        assert_eq!(actual, DXBC);
    }
}

fn decode_spirv_words(bytes: &[u8]) -> Option<Vec<u32>> {
    let word_size = core::mem::size_of::<u32>();
    if bytes.len() % word_size != 0 {
        return None;
    }

    Some(
        bytes
            .chunks_exact(word_size)
            .map(|word| u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
            .collect(),
    )
}

/// Shader module descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShaderModuleDescriptor {
    /// Debug label.
    pub label: ResourceLabel,
    /// Compiled shader data.
    pub binary: ShaderBinary,
}

impl ShaderModuleDescriptor {
    /// Validates the descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Shader`] when the shader code payload is empty.
    pub fn validate(&self) -> Result<()> {
        if self.binary.is_empty() {
            return Err(Error::Shader("shader code must not be empty".to_string()));
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
pub struct VertexAttributeDescriptor {
    /// Shader location.
    pub location: u32,
    /// Byte offset within the vertex.
    pub offset: u32,
    /// Attribute format.
    pub format: VertexFormat,
}

/// Vertex buffer layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VertexBufferLayoutDescriptor {
    /// Vertex stride in bytes.
    pub stride: u32,
    /// Attributes in the vertex.
    pub attributes: Vec<VertexAttributeDescriptor>,
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
    /// Windows RGB subpixel text using the second fragment output as per-channel coverage.
    #[cfg(target_os = "windows")]
    SubpixelDualSource,
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
pub struct ColorAttachmentDescriptor {
    /// Attachment format.
    pub format: Format,
}

/// Depth attachment format for a render pass compatibility descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DepthAttachmentDescriptor {
    /// Attachment format.
    pub format: Format,
}

/// Render pass descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderPassDescriptor {
    /// Debug label.
    pub label: ResourceLabel,
    /// Color attachment.
    pub color_attachment: ColorAttachmentDescriptor,
    /// Optional depth attachment.
    pub depth_attachment: Option<DepthAttachmentDescriptor>,
}

/// Depth comparison used by a graphics pipeline.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CompareFunction {
    /// Never pass the depth test.
    Never,
    /// Pass when the incoming depth is less than the stored depth.
    Less,
    /// Pass when the incoming depth equals the stored depth.
    Equal,
    /// Pass when the incoming depth is less than or equal to the stored depth.
    #[default]
    LessEqual,
    /// Pass when the incoming depth is greater than the stored depth.
    Greater,
    /// Pass when the incoming depth differs from the stored depth.
    NotEqual,
    /// Pass when the incoming depth is greater than or equal to the stored depth.
    GreaterEqual,
    /// Always pass the depth test.
    Always,
}

/// Depth test and write behavior for a render pipeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DepthState {
    /// Comparison applied to fragment depth.
    pub compare: CompareFunction,
    /// Whether passing fragments update the depth attachment.
    pub write_enabled: bool,
}

impl Default for DepthState {
    fn default() -> Self {
        Self {
            compare: CompareFunction::default(),
            write_enabled: true,
        }
    }
}

/// Render pipeline descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderPipelineDescriptor {
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
    pub vertex_buffers: Vec<VertexBufferLayoutDescriptor>,
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
    /// Optional depth state.
    pub depth_state: Option<DepthState>,
}

impl RenderPipelineDescriptor {
    /// Validates a render pipeline descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] when an entry point is empty.
    pub fn validate(&self) -> Result<()> {
        if self.vertex_entry_point.is_empty() {
            return Err(Error::InvalidInput(
                "vertex_entry_point must not be empty".to_string(),
            ));
        }
        if self.fragment_entry_point.is_empty() {
            return Err(Error::InvalidInput(
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
pub struct DrawTriangleDescriptor {
    /// Clear color.
    pub clear_color: ClearColor,
}

/// Command encoder descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandEncoderDescriptor {
    /// Debug label.
    pub label: ResourceLabel,
}

/// Render pass begin descriptor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BeginRenderPassDescriptor {
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
pub struct DrawDescriptor {
    /// Render pass begin parameters.
    pub pass: BeginRenderPassDescriptor,
    /// Render pipeline.
    pub pipeline: RenderPipelineId,
    /// Resource sets bound before drawing.
    pub resource_sets: ResourceSetList,
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

/// Resource sets bound by one draw step.
///
/// Most GPUI nova draw steps bind zero or one resource set. Keeping four inline
/// slots avoids heap allocation on the hot path while still supporting larger
/// binding groups.
pub type ResourceSetList = SmallVec<[ResourceSetId; 4]>;

/// Builds a resource-set list without heap allocation for up to four entries.
#[must_use]
pub fn resource_set_list(
    resource_sets: impl IntoIterator<Item = ResourceSetId>,
) -> ResourceSetList {
    resource_sets.into_iter().collect()
}

/// Builds an empty resource-set list.
#[must_use]
pub fn resource_set_list_empty() -> ResourceSetList {
    ResourceSetList::new()
}

/// One draw step inside a render pass.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawStepDescriptor {
    /// Render pipeline.
    pub pipeline: RenderPipelineId,
    /// Resource sets bound before drawing.
    pub resource_sets: ResourceSetList,
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

/// Index buffer element format.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndexFormat {
    /// 16-bit unsigned integer indices.
    Uint16,
    /// 32-bit unsigned integer indices.
    Uint32,
}

/// Index buffer binding for indexed render steps.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndexBufferBinding {
    /// Index buffer resource.
    pub buffer: BufferId,
    /// Index element format.
    pub format: IndexFormat,
    /// Byte offset into the index buffer.
    pub offset: u64,
}

/// One indexed draw step inside a render pass.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawIndexedStepDescriptor {
    /// Render pipeline.
    pub pipeline: RenderPipelineId,
    /// Resource sets bound before drawing.
    pub resource_sets: ResourceSetList,
    /// Index buffer binding.
    pub index_buffer: IndexBufferBinding,
    /// Index count.
    pub index_count: u32,
    /// First index.
    pub first_index: u32,
    /// Base vertex offset.
    pub base_vertex: i32,
    /// Instance count.
    pub instance_count: u32,
    /// First instance.
    pub first_instance: u32,
    /// Optional scissor rectangle in target pixels.
    pub scissor: Option<ScissorRect>,
}

/// Render step accepted by the GPUI nova compatibility layer.
#[derive(Clone, Debug, PartialEq)]
pub enum RenderStepDescriptor {
    /// Non-indexed draw.
    Draw(DrawStepDescriptor),
    /// Indexed draw.
    DrawIndexed(DrawIndexedStepDescriptor),
}

/// Borrowed render step accepted by backend hot paths.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RenderStepRef<'a> {
    /// Non-indexed draw.
    Draw(&'a DrawStepDescriptor),
    /// Indexed draw.
    DrawIndexed(&'a DrawIndexedStepDescriptor),
}

impl<'a> RenderStepRef<'a> {
    /// Returns the pipeline used by this step.
    #[must_use]
    pub fn pipeline(self) -> RenderPipelineId {
        match self {
            Self::Draw(step) => step.pipeline,
            Self::DrawIndexed(step) => step.pipeline,
        }
    }

    /// Returns the resource sets bound by this step.
    #[must_use]
    pub fn resource_sets(self) -> &'a [ResourceSetId] {
        match self {
            Self::Draw(step) => step.resource_sets.as_slice(),
            Self::DrawIndexed(step) => step.resource_sets.as_slice(),
        }
    }

    /// Returns the optional scissor rectangle.
    #[must_use]
    pub fn scissor(self) -> Option<ScissorRect> {
        match self {
            Self::Draw(step) => step.scissor,
            Self::DrawIndexed(step) => step.scissor,
        }
    }
}

impl<'a> From<&'a DrawStepDescriptor> for RenderStepRef<'a> {
    fn from(step: &'a DrawStepDescriptor) -> Self {
        Self::Draw(step)
    }
}

impl<'a> From<&'a RenderStepDescriptor> for RenderStepRef<'a> {
    fn from(step: &'a RenderStepDescriptor) -> Self {
        match step {
            RenderStepDescriptor::Draw(step) => Self::Draw(step),
            RenderStepDescriptor::DrawIndexed(step) => Self::DrawIndexed(step),
        }
    }
}

/// Borrowed list of render steps used to avoid compatibility allocations.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RenderStepList<'a> {
    /// Non-indexed draw descriptors.
    Draw(&'a [DrawStepDescriptor]),
    /// Full render-step descriptors.
    Render(&'a [RenderStepDescriptor]),
}

impl<'a> RenderStepList<'a> {
    /// Creates a borrowed list from legacy draw steps.
    #[must_use]
    pub const fn from_draw_steps(steps: &'a [DrawStepDescriptor]) -> Self {
        Self::Draw(steps)
    }

    /// Creates a borrowed list from render steps.
    #[must_use]
    pub const fn from_render_steps(steps: &'a [RenderStepDescriptor]) -> Self {
        Self::Render(steps)
    }

    /// Returns true when the list is empty.
    #[must_use]
    pub fn is_empty(self) -> bool {
        match self {
            Self::Draw(steps) => steps.is_empty(),
            Self::Render(steps) => steps.is_empty(),
        }
    }

    /// Returns the first step.
    #[must_use]
    pub fn first(self) -> Option<RenderStepRef<'a>> {
        match self {
            Self::Draw(steps) => steps.first().map(RenderStepRef::from),
            Self::Render(steps) => steps.first().map(RenderStepRef::from),
        }
    }

    /// Iterates over borrowed render steps.
    #[must_use]
    pub fn iter(self) -> RenderStepListIter<'a> {
        match self {
            Self::Draw(steps) => RenderStepListIter::Draw(steps.iter()),
            Self::Render(steps) => RenderStepListIter::Render(steps.iter()),
        }
    }
}

/// Iterator over a borrowed render-step list.
#[derive(Clone, Debug)]
pub enum RenderStepListIter<'a> {
    /// Non-indexed draw descriptor iterator.
    Draw(std::slice::Iter<'a, DrawStepDescriptor>),
    /// Full render-step descriptor iterator.
    Render(std::slice::Iter<'a, RenderStepDescriptor>),
}

impl<'a> Iterator for RenderStepListIter<'a> {
    type Item = RenderStepRef<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Draw(steps) => steps.next().map(RenderStepRef::from),
            Self::Render(steps) => steps.next().map(RenderStepRef::from),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Draw(steps) => steps.size_hint(),
            Self::Render(steps) => steps.size_hint(),
        }
    }
}

impl ExactSizeIterator for RenderStepListIter<'_> {}

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
pub struct BufferWriteDescriptor {
    /// Target buffer.
    pub buffer: BufferId,
    /// Destination byte offset.
    pub offset: u64,
}

/// Texture write descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextureWriteDescriptor {
    /// Target texture.
    pub texture: TextureId,
    /// Destination mip level.
    pub mip_level: u32,
    /// Data layout.
    pub layout: TextureDataLayout,
    /// Target origin.
    pub origin: Origin2d,
    /// Target size.
    pub size: Extent2d,
}

impl TextureWriteDescriptor {
    /// Validates this write against a texture descriptor and source byte length.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] when the mip level or write rectangle exceeds the
    /// texture bounds, or the source layout cannot cover the requested rows.
    pub fn validate_against(&self, texture: &TextureDescriptor, data_len: usize) -> Result<()> {
        texture.validate()?;
        let mip_extent = texture.mip_extent(self.mip_level)?;
        let end_x = self
            .origin
            .x
            .checked_add(self.size.width())
            .ok_or_else(|| Error::InvalidInput("texture write x range overflow".to_string()))?;
        let end_y = self
            .origin
            .y
            .checked_add(self.size.height())
            .ok_or_else(|| Error::InvalidInput("texture write y range overflow".to_string()))?;
        if end_x > mip_extent.width() || end_y > mip_extent.height() {
            return Err(Error::InvalidInput(format!(
                "texture write rectangle {}x{} at {},{} exceeds mip {} bounds {}x{}",
                self.size.width(),
                self.size.height(),
                self.origin.x,
                self.origin.y,
                self.mip_level,
                mip_extent.width(),
                mip_extent.height()
            )));
        }
        let row_bytes = self
            .size
            .width()
            .checked_mul(texture.format.bytes_per_pixel())
            .ok_or_else(|| Error::InvalidInput("texture write row size overflow".to_string()))?;
        let bytes_per_row = self.layout.bytes_per_row.get();
        if bytes_per_row < row_bytes {
            return Err(Error::InvalidInput(format!(
                "texture write bytes_per_row ({bytes_per_row}) is smaller than row data ({row_bytes})"
            )));
        }
        if self.layout.rows_per_image.get() < self.size.height() {
            return Err(Error::InvalidInput(format!(
                "texture write rows_per_image ({}) is smaller than upload height ({})",
                self.layout.rows_per_image.get(),
                self.size.height()
            )));
        }
        let source_offset = usize::try_from(self.layout.offset).map_err(|error| {
            Error::InvalidInput(format!("texture write offset overflow: {error}"))
        })?;
        let source_row_pitch = usize::try_from(bytes_per_row).map_err(|error| {
            Error::InvalidInput(format!("texture write row pitch overflow: {error}"))
        })?;
        let row_bytes = usize::try_from(row_bytes).map_err(|error| {
            Error::InvalidInput(format!("texture write row size overflow: {error}"))
        })?;
        let height = usize::try_from(self.size.height()).map_err(|error| {
            Error::InvalidInput(format!("texture write height overflow: {error}"))
        })?;
        let required_len =
            required_texture_write_len(source_offset, source_row_pitch, row_bytes, height)?;
        if data_len < required_len {
            return Err(Error::InvalidInput(format!(
                "texture write data is smaller than layout: required {required_len} bytes, got {data_len}"
            )));
        }
        Ok(())
    }
}

fn required_texture_write_len(
    offset: usize,
    source_row_pitch: usize,
    row_bytes: usize,
    height: usize,
) -> Result<usize> {
    if height == 0 {
        return Ok(offset);
    }
    offset
        .checked_add(
            height
                .saturating_sub(1)
                .checked_mul(source_row_pitch)
                .ok_or_else(|| {
                    Error::InvalidInput("texture write required size overflow".to_string())
                })?,
        )
        .and_then(|value| value.checked_add(row_bytes))
        .ok_or_else(|| Error::InvalidInput("texture write required size overflow".to_string()))
}

/// Borrowed texture upload in a backend batch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextureWrite<'a> {
    /// Texture upload descriptor.
    pub descriptor: TextureWriteDescriptor,
    /// Pixel bytes for this upload.
    pub data: &'a [u8],
}

/// Tightly packed pixels copied from a GPU texture.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextureReadback {
    /// Texture format used to interpret the bytes.
    pub format: Format,
    /// Width and height of the copied texture.
    pub size: Extent2d,
    /// Number of bytes in each tightly packed row.
    pub bytes_per_row: u32,
    /// Pixel bytes in row-major order without backend row padding.
    pub bytes: Vec<u8>,
}

/// Memory pressure level reported by upper layers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryTrimLevel {
    /// Light memory pressure.
    Light,
    /// Moderate memory pressure.
    Moderate,
    /// Aggressive memory pressure.
    Aggressive,
}

/// Depth attachment used for render submission compatibility methods.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderPassDepthAttachment {
    /// Depth texture view target.
    pub target: TextureViewId,
    /// Depth load operation.
    pub depth_load_op: LoadOp<f32>,
}

/// One ordered render-step list targeting an offscreen texture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextureRenderStepList<'a> {
    /// Color texture view receiving this pass.
    pub texture_view: TextureViewId,
    /// Render pass used by this operation.
    pub render_pass: RenderPassId,
    /// Borrowed draw steps recorded into the pass.
    pub steps: RenderStepList<'a>,
    /// Color target load behavior.
    pub color_load_op: LoadOp<ClearColor>,
    /// Optional depth target and load behavior.
    pub depth_attachment: Option<RenderPassDepthAttachment>,
}

/// Backend resource statistics.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResourceStats {
    /// Native buffer copies allocated to satisfy binding alignment, included
    /// in GPU allocation bytes. This does not count copy commands.
    pub binding_mirror_bytes: u64,
    /// Origin and scope of the GPU byte counts; these are not driver residency.
    pub memory_accounting: MemoryAccounting,
    /// CPU capacity retained by backend buffer mirrors, separately from GPU bytes.
    pub cpu_shadow_bytes: u64,
    /// Upload ring bytes currently occupied by pending data.
    pub upload_used_bytes: u64,
    /// Capacity of allocated upload pages, already included in GPU byte counts.
    pub upload_capacity_bytes: u64,
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

impl ResourceStats {
    /// Bytes reserved by the backend but not occupied by live allocations.
    #[must_use]
    pub const fn unused_reserved_bytes(self) -> u64 {
        self.reserved_bytes.saturating_sub(self.allocated_bytes)
    }

    /// Whole percentage of reserved memory occupied by live allocations.
    #[must_use]
    pub fn reserved_memory_utilization(self) -> Option<u64> {
        if self.reserved_bytes == 0 {
            return None;
        }
        let percentage = u128::from(self.allocated_bytes)
            .saturating_mul(100)
            .checked_div(u128::from(self.reserved_bytes))
            .unwrap_or(0)
            .min(100);
        Some(u64::try_from(percentage).unwrap_or(100))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renderer_extension_device_is_object_safe() {
        let _: Option<&mut dyn ExtensionDevice> = None;
    }

    #[test]
    fn depth_state_defaults_to_less_equal_with_depth_writes() {
        assert_eq!(
            DepthState::default(),
            DepthState {
                compare: CompareFunction::LessEqual,
                write_enabled: true,
            }
        );
    }

    #[test]
    fn extent_rejects_zero_dimensions() {
        assert!(Extent2d::new(0, 1).is_err());
        assert!(Extent2d::new(1, 0).is_err());
        assert!(Extent2d::new(1, 1).is_ok());
    }

    #[test]
    fn resource_stats_report_unused_reservation_and_utilization() {
        let stats = ResourceStats {
            allocated_bytes: 3,
            reserved_bytes: 4,
            ..ResourceStats::default()
        };

        assert_eq!(stats.unused_reserved_bytes(), 1);
        assert_eq!(stats.reserved_memory_utilization(), Some(75));
        assert_eq!(ResourceStats::default().reserved_memory_utilization(), None);
    }

    #[test]
    fn texture_byte_size_counts_all_mips() {
        let descriptor = TextureDescriptor {
            label: None,
            size: Extent2d::new(4, 2).expect("fixture size"),
            mip_level_count: 3,
            format: Format::Rgba8Unorm,
            usage: TextureUsage::SAMPLED,
            memory_location: MemoryLocation::GpuOnly,
            dimension: TextureDimension::D2,
        };
        assert_eq!(descriptor.byte_size(), (4 * 2 + 2 + 1) * 4);
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
            AsyncCapabilities::default(),
            AsyncCapabilities {
                threading_mode: ThreadingMode::OwnerThreadOnly,
                async_submission: false,
                async_wait: false,
                async_presentation: false,
            }
        );
    }

    #[test]
    fn buffer_desc_rejects_zero_size() {
        let descriptor = BufferDescriptor {
            label: None,
            size: 0,
            usage: BufferUsage::VERTEX,
            memory_location: MemoryLocation::CpuToGpu,
        };

        assert!(descriptor.validate().is_err());
    }

    #[test]
    fn texture_desc_rejects_empty_usage() {
        let descriptor = TextureDescriptor {
            label: None,
            size: Extent2d::new(1, 1).expect("test dimensions are non-zero"),
            mip_level_count: 1,
            format: Format::Rgba8Unorm,
            usage: TextureUsage::empty(),
            memory_location: MemoryLocation::GpuOnly,
            dimension: TextureDimension::D2,
        };

        assert!(descriptor.validate().is_err());
    }

    #[test]
    fn texture_desc_validates_mip_levels_and_reports_mip_extent() {
        let mut texture = texture_desc(9, 4);
        texture.mip_level_count = 4;
        assert!(texture.validate().is_ok());
        assert_eq!(
            texture.mip_extent(3).expect("last mip exists"),
            Extent2d::new(1, 1).expect("test dimensions are non-zero")
        );
        assert!(texture.mip_extent(4).is_err());

        texture.mip_level_count = 5;
        assert!(texture.validate().is_err());
        texture.mip_level_count = 0;
        assert!(texture.validate().is_err());
    }

    #[test]
    fn texture_view_validates_mip_range() {
        let mut texture = texture_desc(8, 8);
        texture.mip_level_count = 4;
        let view = TextureViewDescriptor {
            label: None,
            texture: TextureId::from_parts(1, 1),
            base_mip_level: 1,
            mip_level_count: 3,
            format: Format::Rgba8Unorm,
        };

        assert!(view.validate_against(&texture).is_ok());
        assert!(
            TextureViewDescriptor {
                mip_level_count: 4,
                ..view.clone()
            }
            .validate_against(&texture)
            .is_err()
        );
        assert!(
            TextureViewDescriptor {
                mip_level_count: 0,
                ..view
            }
            .validate_against(&texture)
            .is_err()
        );
    }

    #[test]
    fn r8_texture_write_uses_single_byte_texels_and_odd_row_pitch() {
        let mut texture = texture_desc(7, 5);
        texture.format = Format::R8Unorm;
        let descriptor = texture_write_desc(0, 0, 7, 5, 9, 5, 4);
        assert_eq!(texture.format.bytes_per_pixel(), 1);
        assert_eq!(texture.byte_size(), 35);
        assert!(descriptor.validate_against(&texture, 47).is_ok());
        assert!(descriptor.validate_against(&texture, 46).is_err());
        texture.format = Format::Bgra8Unorm;
        assert!(descriptor.validate_against(&texture, 47).is_err());
    }

    #[test]
    fn texture_write_rejects_out_of_bounds_rectangle() {
        let texture = texture_desc(16, 16);
        let descriptor = texture_write_desc(12, 0, 8, 8, 32, 8, 0);

        assert!(descriptor.validate_against(&texture, 256).is_err());
    }

    #[test]
    fn texture_write_uses_selected_mip_extent() {
        let mut texture = texture_desc(16, 16);
        texture.mip_level_count = 5;
        let mut descriptor = texture_write_desc(0, 0, 8, 8, 32, 8, 0);
        descriptor.mip_level = 1;

        assert!(descriptor.validate_against(&texture, 256).is_ok());
        descriptor.origin = Origin2d { x: 7, y: 0 };
        assert!(descriptor.validate_against(&texture, 256).is_err());
        descriptor.mip_level = 5;
        assert!(descriptor.validate_against(&texture, 256).is_err());
    }

    #[test]
    fn texture_write_rejects_short_source_data() {
        let texture = texture_desc(16, 16);
        let descriptor = texture_write_desc(0, 0, 4, 4, 32, 4, 8);

        assert!(descriptor.validate_against(&texture, 119).is_err());
        assert!(descriptor.validate_against(&texture, 120).is_ok());
    }

    #[test]
    fn texture_write_rejects_short_row_layout() {
        let texture = texture_desc(16, 16);
        let descriptor = texture_write_desc(0, 0, 4, 4, 15, 4, 0);

        assert!(descriptor.validate_against(&texture, 64).is_err());
    }

    #[test]
    fn texture_write_rejects_short_rows_per_image() {
        let texture = texture_desc(16, 16);
        let descriptor = texture_write_desc(0, 0, 4, 4, 16, 3, 0);

        assert!(descriptor.validate_against(&texture, 64).is_err());
    }

    #[test]
    fn pipeline_desc_rejects_empty_entry_points() {
        let descriptor = RenderPipelineDescriptor {
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
            depth_state: None,
        };

        assert!(descriptor.validate().is_err());
    }

    #[test]
    fn resource_set_layout_rejects_duplicate_bindings() {
        let descriptor = ResourceSetLayoutDescriptor {
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
        let layout = ResourceSetLayoutDescriptor {
            label: None,
            entries: vec![ResourceSetLayoutEntry {
                binding: 0,
                binding_type: ResourceBindingType::SampledTexture,
                stages: ShaderStages::FRAGMENT,
            }],
        };
        let descriptor = ResourceSetDescriptor {
            label: None,
            layout: ResourceSetLayoutId::from_parts(1, 1),
            bindings: vec![ResourceBinding {
                binding: 0,
                resource: BindingResource::Sampler(SamplerBinding {
                    sampler: SamplerId::from_parts(2, 1),
                }),
            }],
        };

        assert!(descriptor.validate_against(&layout).is_err());
    }

    #[test]
    fn resource_set_accepts_unordered_bindings() {
        let layout = ResourceSetLayoutDescriptor {
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
        let descriptor = ResourceSetDescriptor {
            label: None,
            layout: ResourceSetLayoutId::from_parts(1, 1),
            bindings: vec![
                ResourceBinding {
                    binding: 2,
                    resource: BindingResource::Sampler(SamplerBinding {
                        sampler: SamplerId::from_parts(2, 1),
                    }),
                },
                ResourceBinding {
                    binding: 0,
                    resource: BindingResource::Buffer(BufferBinding {
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

    #[test]
    fn buffer_binding_accepts_range_inside_buffer() {
        let binding = BufferBinding {
            buffer: BufferId::from_parts(1, 1),
            offset: 16,
            size: 48,
            stride: None,
        };

        assert!(binding.validate_against(64).is_ok());
    }

    #[test]
    fn buffer_binding_rejects_empty_or_out_of_bounds_range() {
        let buffer = BufferId::from_parts(1, 1);
        assert!(
            BufferBinding {
                buffer,
                offset: 0,
                size: 0,
                stride: None,
            }
            .validate_against(64)
            .is_err()
        );
        assert!(
            BufferBinding {
                buffer,
                offset: 32,
                size: 64,
                stride: None,
            }
            .validate_against(64)
            .is_err()
        );
        assert!(
            BufferBinding {
                buffer,
                offset: u64::MAX,
                size: 1,
                stride: None,
            }
            .validate_against(u64::MAX)
            .is_err()
        );
    }

    fn texture_desc(width: u32, height: u32) -> TextureDescriptor {
        TextureDescriptor {
            label: None,
            size: Extent2d::new(width, height).expect("test dimensions are non-zero"),
            mip_level_count: 1,
            format: Format::Rgba8Unorm,
            usage: TextureUsage::SAMPLED,
            memory_location: MemoryLocation::GpuOnly,
            dimension: TextureDimension::D2,
        }
    }

    fn texture_write_desc(
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        bytes_per_row: u32,
        rows_per_image: u32,
        offset: u64,
    ) -> TextureWriteDescriptor {
        TextureWriteDescriptor {
            texture: TextureId::from_parts(1, 1),
            mip_level: 0,
            layout: TextureDataLayout::new(offset, bytes_per_row, rows_per_image)
                .expect("test layout should be valid"),
            origin: Origin2d { x, y },
            size: Extent2d::new(width, height).expect("test dimensions are non-zero"),
        }
    }
}
