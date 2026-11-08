//! Backend-neutral graphics API types for nova-gfx.
//!
//! `gfx-core` intentionally contains no UI framework, Vulkan, DX12, Metal, or
//! windowing code. It provides shared descriptors, errors, and typed generational
//! handles used by backend implementations.
//!
//! The canonical device contract is exposed through the device capability traits.
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
mod buffer_texture_copy;
mod buffer_upload;
mod compute;
mod gpu_timestamps;
#[cfg(feature = "conformance")]
pub mod conformance;
mod memory;
mod texture_array;
mod texture_copy;
pub use buffer_texture_copy::{
    CopyDirection, TextureBufferCopy, TextureCopyLimits, texture_copy_buffer_bytes,
    texture_copy_row_pitch,
};
pub use compute::{
    BufferBindingLimits, ComputeBindGroupDispatch, ComputeBufferAccess, ComputeBufferBinding,
    ComputeCapabilities, ComputeDispatch, ComputeKernelProfile, VoxelExecutionPath,
};
pub use texture_array::{
    TextureArrayCapabilities, TextureArrayLayerUpload, TextureArrayLayerWriteDescriptor,
    TextureArrayPlan,
};
pub use texture_copy::TextureCopy;

pub use buffer_upload::{BufferUploadBatch, BufferUploadStats, BufferWrite};
pub use gpu_timestamps::{GpuTimestampSample, GpuTimestampStage, GpuTimestampTag};
pub use memory::{
    DeviceMemoryBudget, MemoryAccounting, MemoryArchitecture, MemoryBudget, MemoryCompactReport,
};

use std::{
    borrow::Cow,
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    num::NonZeroU32,
    path::PathBuf,
};

use bitflags::bitflags;
use smallvec::SmallVec;
use thiserror::Error as ThisError;

pub use backend::{
    Backend, CommandDevice, Device, DiagnosticsDevice, FrameGpuTimestamps, PipelineDevice,
    PresentTimings, PresentationDevice, PresentedFrame, ResourceDevice, SubmissionDevice,
    SurfaceDevice, TextureTransferDevice,
};

/// Convenience result type used by nova-gfx crates.
pub type Result<T> = std::result::Result<T, Error>;

/// Error type for backend-neutral validation and backend-provided failures.
#[derive(Debug, ThisError)]
pub enum Error {
    /// A required graphics capability or resource was not available.
    #[error("graphics resource is unavailable: {0}")]
    Unavailable(String),
    /// The native presentation surface changed and the current frame must be retried.
    #[error("graphics presentation surface is outdated")]
    SurfaceOutdated,
    /// The GPU device was removed or reset; all GPU resources and submissions
    /// are invalid and the device must be recreated. The payload carries the
    /// backend's removal reason for diagnostics. This matches wgpu's
    /// lost-device semantics.
    #[error("graphics device lost: {0}")]
    DeviceLost(String),
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
/// Bind group layout handle.
#[derive(Debug)]
pub enum BindGroupLayoutResource {}
/// Bind group handle.
#[derive(Debug)]
pub enum BindGroupResource {}
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
/// Bind group layout identifier.
pub type BindGroupLayoutId = ResourceId<BindGroupLayoutResource>;
/// Bind group identifier.
pub type BindGroupId = ResourceId<BindGroupResource>;
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
pub struct SurfaceCapabilities {
    /// Native presentation consumes a present region while keeping every rotating backbuffer coherent.
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

/// Backend pool, descriptor-heap, and transfer tuning.
///
/// Every field uses zero to select the backend's built-in default. These knobs
/// only affect backend-internal allocations and pacing; they never change the
/// public resource semantics. Values outside a backend's native limits are
/// rejected or clamped by that backend at device creation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DeviceTuning {
    /// Retained command encoders before excess encoders are destroyed.
    ///
    /// Used by DX12 and Vulkan. Zero selects 16.
    pub command_encoder_pool_capacity: u32,
    /// Retained upload command encoders/pools before excess ones are destroyed.
    ///
    /// Used by DX12 and Vulkan. Zero selects 4.
    pub upload_command_pool_capacity: u32,
    /// Shader-visible resource descriptor heap capacity.
    ///
    /// Used by DX12 for CBV/SRV/UAV descriptors. Zero selects 65536; the D3D12
    /// limit of 1,000,000 descriptors is enforced. The sampler heap stays at
    /// the D3D12 limit of 2048.
    pub resource_descriptor_capacity: u32,
    /// Non-shader-visible render-target-view descriptor heap capacity.
    ///
    /// Used by DX12. Zero selects 256.
    pub render_target_descriptor_capacity: u32,
    /// Non-shader-visible depth-stencil-view descriptor heap capacity.
    ///
    /// Used by DX12. Zero selects 256.
    pub depth_stencil_descriptor_capacity: u32,
    /// Upper bound for a persisted pipeline-cache blob.
    ///
    /// Used by DX12 and Vulkan. Zero selects 64 MiB.
    pub pipeline_cache_limit_bytes: u64,
    /// Bytes reserved by one transient upload-ring page.
    ///
    /// Used by DX12 and Vulkan. Zero selects 4 MiB.
    pub upload_page_bytes: u64,
    /// Idle upload-ring pages retained for reuse.
    ///
    /// Used by DX12 and Vulkan. Zero selects 2.
    pub upload_retained_idle_pages: u32,
    /// Bounded wait applied when a native frame slot is still busy.
    ///
    /// Used by Vulkan. Zero selects 8 ms.
    pub present_timeout_micros: u64,
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
    /// Backend pool, descriptor-heap, and transfer tuning.
    pub tuning: DeviceTuning,
}

impl Default for DeviceDescriptor {
    fn default() -> Self {
        Self {
            application_name: "nova-gfx".to_string(),
            adapter_name: None,
            power_preference: PowerPreference::LowPower,
            pipeline_cache_dir: None,
            tuning: DeviceTuning::default(),
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
    /// 16-bit half-float RGBA format with linear blending and filtering.
    ///
    /// This is the standard interchange format for linear HDR render targets and sampled HDR
    /// textures; values are not clamped to 0..=1.
    Rgba16Float,
    /// 32-bit floating-point depth format.
    Depth32Float,
    /// 8-bit unsigned normalized two-channel format for paired masks.
    Rg8Unorm,
    /// 16-bit half-float red channel; half the storage of [`Self::R32Float`].
    R16Float,
    /// 16-bit half-float red/green channels.
    Rg16Float,
    /// 32-bit floating-point red channel for depth-like or high-precision masks.
    R32Float,
    /// 32-bit floating-point RGBA for full-precision interchange.
    Rgba32Float,
    /// 8-bit unsigned integer red channel for identifiers.
    ///
    /// Sampling requires an integer-typed shader binding (`texture_2d<u32>`).
    R8Uint,
    /// 8-bit unsigned integer red/green channels for packed identifiers.
    Rg8Uint,
    /// 8-bit unsigned integer RGBA for identifiers and bit fields.
    Rgba8Uint,
}

impl Format {
    /// Returns whether this format uses sRGB conversion.
    #[must_use]
    pub const fn is_srgb(self) -> bool {
        matches!(self, Self::Bgra8UnormSrgb | Self::Rgba8UnormSrgb)
    }

    /// Returns whether this format stores any depth component.
    #[must_use]
    pub const fn is_depth(self) -> bool {
        matches!(self, Self::Depth32Float)
    }

    /// Returns whether this format stores integers instead of normalized/floating values.
    ///
    /// Integer formats cannot be blended or filtered, and shaders must declare an
    /// integer-typed binding to read them.
    #[must_use]
    pub const fn is_integer(self) -> bool {
        matches!(self, Self::R8Uint | Self::Rg8Uint | Self::Rgba8Uint)
    }

    /// Returns whether this format can back a sampled or storage texture.
    #[must_use]
    pub const fn is_texture(self) -> bool {
        !self.is_depth()
    }

    /// Returns the number of bytes occupied by one pixel.
    #[must_use]
    pub const fn bytes_per_pixel(self) -> u32 {
        match self {
            Self::R8Unorm | Self::R8Uint => 1,
            Self::Rg8Unorm | Self::R16Float | Self::Rg8Uint => 2,
            Self::Bgra8Unorm
            | Self::Bgra8UnormSrgb
            | Self::Rgba8Unorm
            | Self::Rgba8UnormSrgb
            | Self::Rgba8Uint
            | Self::Depth32Float
            | Self::Rg16Float
            | Self::R32Float => 4,
            Self::Rgba16Float => 8,
            Self::Rgba32Float => 16,
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
    /// Requested number of rotating swapchain images.
    ///
    /// Zero selects the backend default (DX11: 2, DX12: 3, Vulkan:
    /// `min_image_count + 1`). Other backends may ignore the request. The value
    /// is clamped to each backend's supported range.
    pub desired_image_count: u32,
    /// Requested maximum number of frames queued ahead of presentation.
    ///
    /// Zero selects the backend default (DX11: 1, DX12: 2, Vulkan: 2 frame
    /// slots). Vulkan applies this to its frame-slot ring, not to swapchain
    /// image count. Other backends may ignore the request.
    pub desired_maximum_frame_latency: u32,
}

impl SurfaceConfig {
    /// Creates a validated swapchain descriptor with backend-default pacing.
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
            desired_image_count: 0,
            desired_maximum_frame_latency: 0,
        })
    }

    /// Resolves [`Self::desired_image_count`] against a backend range.
    ///
    /// `backend_default` applies when the request is zero. The result is always
    /// within `minimum..=maximum`; an inverted range is treated as `minimum`.
    #[must_use]
    pub fn resolve_image_count(self, backend_default: u32, minimum: u32, maximum: u32) -> u32 {
        let requested = backend_default_when_zero(self.desired_image_count, backend_default);
        clamp_request(requested, minimum, maximum)
    }

    /// Resolves [`Self::desired_maximum_frame_latency`] against a backend range.
    ///
    /// `backend_default` applies when the request is zero. The result is always
    /// within `minimum..=maximum`; an inverted range is treated as `minimum`.
    #[must_use]
    pub fn resolve_frame_latency(self, backend_default: u32, minimum: u32, maximum: u32) -> u32 {
        let requested =
            backend_default_when_zero(self.desired_maximum_frame_latency, backend_default);
        clamp_request(requested, minimum, maximum)
    }
}

fn backend_default_when_zero(requested: u32, backend_default: u32) -> u32 {
    if requested == 0 {
        backend_default
    } else {
        requested
    }
}

fn clamp_request(requested: u32, minimum: u32, maximum: u32) -> u32 {
    requested.max(minimum).min(maximum.max(minimum))
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
        /// Writable storage, in addition to `STORAGE`.
        const STORAGE_WRITE = 1 << 6;
        /// GPU-generated indirect draw arguments.
        const INDIRECT = 1 << 7;
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
        /// Shader storage image: read or written by a shader through a UAV / image unit.
        const STORAGE = 1 << 5;
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
pub struct BindGroupEntry {
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
pub enum BindingType {
    /// Uniform buffer.
    UniformBuffer,
    /// Read-only storage buffer.
    StorageBuffer,
    /// Sampled texture or texture view.
    SampledTexture,
    /// Storage texture view bound as a UAV / image unit.
    ///
    /// `format` must equal the bound view's format; `read_only` bindings reject writes.
    StorageTexture {
        /// Texel format the shader reads or writes.
        format: Format,
        /// `true` when the shader only reads the image.
        read_only: bool,
    },
    /// Sampler.
    Sampler,
}

impl BindingType {
    /// Returns the expected payload type for this binding.
    #[must_use]
    pub const fn matches(self, binding: &BindingResource) -> bool {
        matches!(
            (self, binding),
            (
                Self::UniformBuffer | Self::StorageBuffer,
                BindingResource::Buffer(_)
            ) | (
                Self::SampledTexture | Self::StorageTexture { .. },
                BindingResource::Texture(_)
            ) | (Self::Sampler, BindingResource::Sampler(_))
        )
    }
}

/// Resource layout entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BindGroupLayoutEntry {
    /// Binding slot.
    pub binding: u32,
    /// Resource kind.
    pub binding_type: BindingType,
    /// Shader visibility.
    pub stages: ShaderStages,
}

/// Bind group layout descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BindGroupLayoutDescriptor {
    /// Debug label.
    pub label: ResourceLabel,
    /// Layout entries.
    pub entries: Vec<BindGroupLayoutEntry>,
}

impl BindGroupLayoutDescriptor {
    /// Validates the descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] when the layout is empty or contains
    /// duplicate bindings.
    pub fn validate(&self) -> Result<()> {
        if self.entries.is_empty() {
            return Err(Error::InvalidInput(
                "bind group layout must contain at least one entry".to_string(),
            ));
        }
        let mut bindings = std::collections::BTreeSet::new();
        for entry in &self.entries {
            if entry.stages.is_empty() {
                return Err(Error::InvalidInput(
                    "bind group layout entry must be visible to at least one shader stage"
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

/// Bind group descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BindGroupDescriptor {
    /// Debug label.
    pub label: ResourceLabel,
    /// Layout this bind group conforms to.
    pub layout: BindGroupLayoutId,
    /// Concrete bind group entries.
    pub entries: Vec<BindGroupEntry>,
}

impl BindGroupDescriptor {
    /// Validates the descriptor against a layout.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] when a binding is missing or invalid.
    pub fn validate_against(&self, layout: &BindGroupLayoutDescriptor) -> Result<()> {
        layout.validate()?;
        if self.entries.len() != layout.entries.len() {
            return Err(Error::InvalidInput(format!(
                "bind group binding count {} does not match layout entry count {}",
                self.entries.len(),
                layout.entries.len()
            )));
        }
        let mut bindings_by_slot = std::collections::BTreeMap::new();
        for binding in &self.entries {
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
    /// Bind group layouts used by this pipeline.
    pub bind_group_layouts: Vec<BindGroupLayoutId>,
}

impl PipelineLayoutDescriptor {
    /// Validates the descriptor.
    ///
    /// An empty layout is valid: pipelines without resource bindings (for
    /// example vertex-color passes) must not be forced to declare a dummy
    /// bind group layout.
    pub fn validate(&self) -> Result<()> {
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
        if self.usage.contains(BufferUsage::STORAGE_WRITE)
            && (!self.usage.contains(BufferUsage::STORAGE)
                || self.usage.contains(BufferUsage::UNIFORM))
        {
            return Err(Error::InvalidInput(
                "writable storage requires STORAGE without UNIFORM".into(),
            ));
        }
        Ok(())
    }
}

/// Texture dimensionality.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextureDimension {
    /// Single-layer 2D texture.
    D2,
    /// Array of identical 2D slices, with independent mip chains.
    D2Array {
        /// Number of independently addressed slices.
        layers: u32,
    },
}

impl TextureDimension {
    /// Number of array layers per mip level.
    #[must_use]
    pub const fn layers(self) -> u32 {
        match self {
            Self::D2 => 1,
            Self::D2Array { layers } => layers,
        }
    }
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
    /// Sample count for multisampled textures. `1` is single-sample.
    pub sample_count: u32,
}

impl TextureDescriptor {
    /// Logical texel bytes across all mip levels, excluding native tiling,
    /// alignment, metadata and driver overhead. Use only with a valid descriptor.
    #[must_use]
    pub fn byte_size(&self) -> u64 {
        (0..self.mip_level_count)
            .fold(0_u64, |total, mip| {
                total.saturating_add(
                    u64::from((self.size.width() >> mip).max(1))
                        .saturating_mul(u64::from((self.size.height() >> mip).max(1)))
                        .saturating_mul(u64::from(self.format.bytes_per_pixel())),
                )
            })
            .saturating_mul(u64::from(self.dimension.layers()))
            .saturating_mul(u64::from(self.sample_count))
    }
    /// Validates the descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] when no usage is set or the mip count exceeds the
    /// Returns whether the texture uses a depth format.
    #[must_use]
    pub const fn is_format_depth(&self) -> bool {
        self.format.is_depth()
    }

    /// dimensions' complete mip chain.
    pub fn validate(&self) -> Result<()> {
        if self.usage.is_empty() {
            return Err(Error::InvalidInput(
                "texture usage must not be empty".to_string(),
            ));
        }
        if self.sample_count == 0 || !self.sample_count.is_power_of_two() || self.sample_count > 64 {
            return Err(Error::InvalidInput(format!(
                "texture sample_count {} must be a power of two in 1..=64",
                self.sample_count
            )));
        }
        // D3D11/D3D12, Vulkan and OpenGL all require a multisampled
        // 2D texture to have a single mip level and no array layers in
        // the currently exposed portable texture-view ABI.
        if self.sample_count > 1
            && (self.mip_level_count != 1 || self.dimension != TextureDimension::D2)
        {
            return Err(Error::InvalidInput(
                "multisampled textures require one mip level and a 2D dimension".into(),
            ));
        }
        if let TextureDimension::D2Array { layers } = self.dimension {
            if layers == 0 {
                return Err(Error::InvalidInput(
                    "texture arrays require nonzero layers".into(),
                ));
            }
            if self.format == Format::Depth32Float {
                return Err(Error::Unavailable(
                    "depth texture arrays need a separate depth-view ABI".into(),
                ));
            }
            // Array render targets/readback need their own layer-aware ABI.
            if self.usage.intersects(
                TextureUsage::COLOR_ATTACHMENT
                    | TextureUsage::DEPTH_ATTACHMENT
                    | TextureUsage::COPY_SRC,
            ) {
                return Err(Error::Unavailable(
                    "layered attachments and readback are not implemented".into(),
                ));
            }
        }
        if self.usage.contains(TextureUsage::STORAGE) {
            // Storage images are single-sampled, single-level and never depth.
            if self.sample_count > 1 {
                return Err(Error::InvalidInput(
                    "storage textures must be single-sampled".into(),
                ));
            }
            if self.is_format_depth() {
                return Err(Error::InvalidInput(
                    "storage textures cannot use a depth format".into(),
                ));
            }
            if self.mip_level_count != 1 {
                return Err(Error::InvalidInput(
                    "storage textures must have exactly one mip level".into(),
                ));
            }
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
    /// Mirror the texture before repeating.
    MirrorRepeat,
    /// Clamp to the border color.
    ClampToBorder,
}

/// Sampler descriptor.
#[derive(Clone, Debug, PartialEq)]
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
    /// W address mode.
    pub address_mode_w: AddressMode,
    /// Comparison function for shadow sampling. `None` uses ordinary (non-compare) sampling.
    pub compare: Option<CompareFunction>,
    /// Border color used with [`AddressMode::ClampToBorder`].
    pub border_color: Option<[f32; 4]>,
    /// Minimum level-of-detail clamp.
    pub lod_min_clamp: f32,
    /// Maximum level-of-detail clamp.
    pub lod_max_clamp: f32,
    /// Level-of-detail bias in mip units.
    pub lod_bias: f32,
}

impl SamplerDescriptor {
    /// Validates sampler fields.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] when the LOD range is inverted.
    pub fn validate(&self) -> Result<()> {
        if self.lod_min_clamp > self.lod_max_clamp {
            return Err(Error::InvalidInput(
                "sampler lod_min_clamp must not exceed lod_max_clamp".to_string(),
            ));
        }
        Ok(())
    }
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
            address_mode_w: AddressMode::ClampToEdge,
            compare: None,
            border_color: None,
            lod_min_clamp: 0.0,
            lod_max_clamp: f32::MAX,
            lod_bias: 0.0,
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
    /// Compute stage.
    Compute,
}

impl fmt::Display for ShaderStage {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Vertex => output.write_str("vertex"),
            Self::Fragment => output.write_str("fragment"),
            Self::Compute => output.write_str("compute"),
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
        /// GLSL image unit names and the WGSL storage slots they came from.
        images: &'static [GlslImageBinding],
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
                images,
            } => ShaderBinary {
                stage,
                entry_point: entry_point.into(),
                code: ShaderCode::Glsl(GlslShader {
                    source: Cow::Borrowed(source),
                    buffers: buffers.to_vec(),
                    textures: textures.to_vec(),
                    images: images.to_vec(),
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
    /// Storage image (image unit) reflection required by desktop GLSL.
    pub images: Vec<GlslImageBinding>,
}

/// One GLSL storage image uniform and the WGSL slot it came from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GlslImageBinding {
    /// Linked image uniform name generated by Naga.
    pub name: Cow<'static, str>,
    /// WGSL group-zero binding number.
    pub binding: u32,
    /// Texel format declared by the WGSL storage texture.
    pub format: Format,
    /// `true` when the shader only reads the image.
    pub read_only: bool,
}

/// Original group-zero resource slot for a GLSL buffer block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GlslBufferBinding {
    /// Linked block name generated by Naga; never inferred from a user variable name.
    pub name: Cow<'static, str>,
    /// WGSL group-zero binding number.
    pub binding: u32,
    /// Whether this block is uniform or read-only storage.
    pub kind: BindingType,
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
    /// One 32-bit float.
    Float32,
    /// Two 32-bit floats.
    Float32x2,
    /// Three 32-bit floats.
    Float32x3,
    /// Four 32-bit floats.
    Float32x4,
    /// One 32-bit unsigned integer.
    Uint32,
    /// Two 32-bit unsigned integers.
    Uint32x2,
    /// Three 32-bit unsigned integers.
    Uint32x3,
    /// Four 32-bit unsigned integers.
    Uint32x4,
    /// Four 8-bit unsigned normalized values packed from one `u32`.
    Unorm8x4,
    /// Four 8-bit unsigned integers packed from one `u32`.
    Uint8x4,
    /// Two 16-bit unsigned normalized values packed from one `u32`.
    Unorm16x2,
    /// Four 16-bit unsigned normalized values packed from two `u32`s.
    Unorm16x4,
}

impl VertexFormat {
    /// Returns the format size in bytes, matching how a native input layout declares it.
    ///
    /// `Unorm8x4`/`Uint8x4` occupy four bytes, and `Unorm16x2` four; `Unorm16x4` eight.
    #[must_use]
    pub const fn size(self) -> u32 {
        match self {
            Self::Float32 | Self::Uint32 | Self::Unorm8x4 | Self::Uint8x4 | Self::Unorm16x2 => 4,
            Self::Float32x2 | Self::Unorm16x4 => 8,
            Self::Float32x3 => 12,
            Self::Float32x4 | Self::Uint32x4 => 16,
            Self::Uint32x2 => 8,
            Self::Uint32x3 => 12,
        }
    }
}

/// How often a vertex buffer advances while a draw runs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum VertexStepMode {
    /// One element per vertex.
    #[default]
    Vertex,
    /// One element per instance.
    Instance,
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
    /// Element advance rate. Metadata for the shader-pulled vertex path; backends that
    /// build native input layouts map it to their per-instance input slot class.
    pub step_mode: VertexStepMode,
    /// Attributes in the vertex.
    pub attributes: Vec<VertexAttributeDescriptor>,
}

/// Portable upper bound for color attachments in one render pass.
///
/// D3D11 and D3D12 guarantee eight; Vulkan and OpenGL devices may report
/// fewer through [`ResourceDevice::max_color_attachments`]. Callers must not
/// request more than the smaller of this constant and the device limit.
pub const MAX_COLOR_ATTACHMENTS: usize = 8;

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

/// Triangle winding considered front-facing after projection, in a Y-up coordinate system.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FrontFace {
    /// Counter-clockwise winding.
    #[default]
    Ccw,
    /// Clockwise winding.
    Cw,
}

/// Triangle face rejected by the rasterizer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Face {
    /// Front-facing triangles.
    Front,
    /// Back-facing triangles.
    Back,
}

/// Rasterizer winding and face culling. Defaults to drawing both faces.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RasterState {
    /// Winding considered front-facing.
    pub front_face: FrontFace,
    /// Faces to reject; `None` draws both sides.
    pub cull_mode: Option<Face>,
}

/// Color attachment descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ColorAttachmentDescriptor {
    /// Attachment format.
    pub format: Format,
}

bitflags! {
    /// Per-attachment color write mask.
    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    pub struct ColorWrites: u8 {
        /// Write the red channel.
        const RED = 1 << 0;
        /// Write the green channel.
        const GREEN = 1 << 1;
        /// Write the blue channel.
        const BLUE = 1 << 2;
        /// Write the alpha channel.
        const ALPHA = 1 << 3;
        /// Write every RGB channel.
        const COLOR = Self::RED.bits() | Self::GREEN.bits() | Self::BLUE.bits();
        /// Write every channel.
        const ALL = Self::COLOR.bits() | Self::ALPHA.bits();
    }
}

impl Default for ColorWrites {
    fn default() -> Self {
        Self::ALL
    }
}

/// Multisampling and stencil capabilities reported by a device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MultisampleCapabilities {
    /// Maximum multisample sample count the device supports. `1` means no MSAA.
    pub max_sample_count: u32,
    /// Whether the device supports a stencil buffer.
    pub stencil_supported: bool,
}

impl Default for MultisampleCapabilities {
    fn default() -> Self {
        Self {
            max_sample_count: 1,
            stencil_supported: true,
        }
    }
}

/// One color output of a graphics pipeline.
///
/// The format must match the corresponding [`RenderPassDescriptor`] color
/// attachment. Blend mode and write mask apply independently per attachment.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ColorTargetState {
    /// Attachment format.
    pub format: Format,
    /// Blend mode for this attachment.
    pub blend: BlendMode,
    /// Channels this attachment writes.
    pub write_mask: ColorWrites,
}

impl ColorTargetState {
    /// Creates a target with full color writes.
    #[must_use]
    pub const fn new(format: Format, blend: BlendMode) -> Self {
        Self {
            format,
            blend,
            write_mask: ColorWrites::ALL,
        }
    }
}

/// Depth attachment format for a render pass layout descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DepthAttachmentDescriptor {
    /// Attachment format.
    pub format: Format,
    /// Optional stencil format when the attachment carries a stencil plane.
    pub stencil_format: Option<Format>,
}

/// Render pass descriptor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderPassDescriptor {
    /// Debug label.
    pub label: ResourceLabel,
    /// Color attachments in pipeline output order.
    pub color_attachments: Vec<ColorAttachmentDescriptor>,
    /// Optional depth attachment.
    pub depth_attachment: Option<DepthAttachmentDescriptor>,
    /// Sample count for a multisampled pass. `1` is single-sample.
    pub sample_count: u32,
    /// Optional single-sample resolve target for the first color attachment.
    /// Valid only when [`Self::sample_count`] is greater than one.
    pub resolve_target: Option<ColorAttachmentDescriptor>,
}

impl RenderPassDescriptor {
    /// Validates the attachment layout.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] when the pass has no color attachment or
    /// more than [`MAX_COLOR_ATTACHMENTS`], or an invalid sample count.
    pub fn validate(&self) -> Result<()> {
        if self.color_attachments.is_empty() {
            return Err(Error::InvalidInput(
                "render pass must contain at least one color attachment".to_string(),
            ));
        }
        if self.color_attachments.len() > MAX_COLOR_ATTACHMENTS {
            return Err(Error::InvalidInput(format!(
                "render pass has {} color attachments; the maximum is {MAX_COLOR_ATTACHMENTS}",
                self.color_attachments.len()
            )));
        }
        if self.sample_count == 0 || !self.sample_count.is_power_of_two() || self.sample_count > 64 {
            return Err(Error::InvalidInput(format!(
                "render pass sample_count {} must be a power of two in 1..=64",
                self.sample_count
            )));
        }
        if self.sample_count == 1 && self.resolve_target.is_some() {
            return Err(Error::InvalidInput(
                "a single-sampled render pass cannot declare a resolve target".to_string(),
            ));
        }
        if let Some(resolve) = &self.resolve_target
            && let Some(first) = self.color_attachments.first()
            && resolve.format != first.format
        {
            return Err(Error::InvalidInput(
                "render pass resolve target format must match the first color attachment".into(),
            ));
        }
        Ok(())
    }
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

/// Stencil operation applied when the stencil test passes or fails.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StencilOp {
    /// Keep the current value.
    #[default]
    Keep,
    /// Write zero.
    Zero,
    /// Write the reference value.
    Replace,
    /// Increment, clamped.
    IncrementClamp,
    /// Decrement, clamped.
    DecrementClamp,
    /// Bitwise invert.
    Invert,
    /// Increment with wrap-around.
    IncrementWrap,
    /// Decrement with wrap-around.
    DecrementWrap,
}

/// Per-face stencil operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StencilFaceState {
    /// Comparison applied to the stencil value.
    pub compare: CompareFunction,
    /// Operation when the stencil test fails.
    pub fail_op: StencilOp,
    /// Operation when the depth test fails.
    pub depth_fail_op: StencilOp,
    /// Operation when the stencil and depth tests pass.
    pub pass_op: StencilOp,
}

impl Default for StencilFaceState {
    fn default() -> Self {
        Self {
            compare: CompareFunction::Always,
            fail_op: StencilOp::Keep,
            depth_fail_op: StencilOp::Keep,
            pass_op: StencilOp::Keep,
        }
    }
}

/// Stencil test configuration for a render pipeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StencilState {
    /// Front-face operations.
    pub front: StencilFaceState,
    /// Back-face operations.
    pub back: StencilFaceState,
    /// Bitmask of the readable stencil bits (compared against the reference).
    pub read_mask: u32,
    /// Bitmask of the writable stencil bits.
    pub write_mask: u32,
}

impl Default for StencilState {
    fn default() -> Self {
        Self {
            front: StencilFaceState::default(),
            back: StencilFaceState::default(),
            read_mask: u32::MAX,
            write_mask: u32::MAX,
        }
    }
}

/// Depth test and write behavior for a render pipeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DepthState {
    /// Comparison applied to fragment depth.
    pub compare: CompareFunction,
    /// Whether passing fragments update the depth attachment.
    pub write_enabled: bool,
    /// Optional stencil test; `None` disables stencil.
    pub stencil: Option<StencilState>,
}

impl Default for DepthState {
    fn default() -> Self {
        Self {
            compare: CompareFunction::default(),
            write_enabled: true,
            stencil: None,
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
    /// Color outputs in render-pass attachment order.
    pub color_targets: Vec<ColorTargetState>,
    /// Primitive topology.
    pub primitive_topology: PrimitiveTopology,
    /// Rasterizer winding and face culling.
    pub raster: RasterState,
    /// Optional depth state.
    pub depth_state: Option<DepthState>,
}

impl RenderPipelineDescriptor {
    /// Validates a render pipeline descriptor.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] when an entry point is empty or the
    /// color target count is outside `1..=MAX_COLOR_ATTACHMENTS`.
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
        if self.color_targets.is_empty() {
            return Err(Error::InvalidInput(
                "render pipeline must contain at least one color target".to_string(),
            ));
        }
        if self.color_targets.len() > MAX_COLOR_ATTACHMENTS {
            return Err(Error::InvalidInput(format!(
                "render pipeline has {} color targets; the maximum is {MAX_COLOR_ATTACHMENTS}",
                self.color_targets.len()
            )));
        }
        // Integer render targets cannot blend on any backend; rejecting the state once
        // keeps every backend from having to guard its native blend state.
        for (index, target) in self.color_targets.iter().enumerate() {
            if target.format.is_integer() && target.blend != BlendMode::Replace {
                return Err(Error::InvalidInput(format!(
                    "color target {index} uses integer format {:?}, which cannot blend",
                    target.format
                )));
            }
        }
        // Vertex layouts must describe attributes that fit inside one element, so a
        // native input layout never reads past the buffer element (or repeats a slot).
        let mut locations = Vec::new();
        for (buffer_index, layout) in self.vertex_buffers.iter().enumerate() {
            if layout.stride == 0 {
                return Err(Error::InvalidInput(format!(
                    "vertex buffer {buffer_index} declares a zero stride"
                )));
            }
            for attribute in &layout.attributes {
                let end = attribute.offset + attribute.format.size();
                if end > layout.stride {
                    return Err(Error::InvalidInput(format!(
                        "vertex buffer {buffer_index} attribute at location {} ends at byte {end}, past the stride {}",
                        attribute.location, layout.stride
                    )));
                }
                if locations.contains(&attribute.location) {
                    return Err(Error::InvalidInput(format!(
                        "shader location {} is declared by more than one vertex attribute",
                        attribute.location
                    )));
                }
                locations.push(attribute.location);
            }
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

/// Draw call descriptor used by the explicit command-encoder path.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawDescriptor {
    /// Render pass begin parameters.
    pub pass: BeginRenderPassDescriptor,
    /// Render pipeline.
    pub pipeline: RenderPipelineId,
    /// Bind groups bound before drawing.
    pub bind_groups: BindGroupList,
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

/// Bind groups bound by one render command.
///
/// Four inline slots cover the common case without a heap allocation on the
/// draw path while still supporting larger binding groups.
pub type BindGroupList = SmallVec<[BindGroupId; 4]>;

/// Builds a bind-group list without heap allocation for up to four entries.
#[must_use]
pub fn bind_group_list(bind_groups: impl IntoIterator<Item = BindGroupId>) -> BindGroupList {
    bind_groups.into_iter().collect()
}

/// Builds an empty bind-group list.
#[must_use]
pub fn bind_group_list_empty() -> BindGroupList {
    BindGroupList::new()
}

/// One non-indexed draw command recorded inside a render pass.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawCommand {
    /// Render pipeline.
    pub pipeline: RenderPipelineId,
    /// Bind groups bound before drawing.
    pub bind_groups: BindGroupList,
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

/// Index buffer binding for an indexed draw command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndexBufferBinding {
    /// Index buffer resource.
    pub buffer: BufferId,
    /// Index element format.
    pub format: IndexFormat,
    /// Byte offset into the index buffer.
    pub offset: u64,
}

/// One indexed draw command recorded inside a render pass.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawIndexedCommand {
    /// Render pipeline.
    pub pipeline: RenderPipelineId,
    /// Bind groups bound before drawing.
    pub bind_groups: BindGroupList,
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

/// A non-indexed draw whose four little-endian `u32` arguments are GPU-resident:
/// vertex count, instance count, first vertex, first instance (16 bytes total).
/// GPU-written counts and offsets are consumed without CPU readback.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawIndirectCommand {
    /// Render pipeline.
    pub pipeline: RenderPipelineId,
    /// Bind groups bound before drawing.
    pub bind_groups: BindGroupList,
    /// Buffer created with `INDIRECT` usage.
    pub arguments: BufferId,
    /// Four-byte aligned byte offset of the argument record.
    pub offset: u64,
    /// Optional scissor rectangle in target pixels.
    pub scissor: Option<ScissorRect>,
}

/// GPU-generated indexed draw; requires advertised indirect-draw support.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawIndexedIndirectCommand {
    /// Render pipeline.
    pub pipeline: RenderPipelineId,
    /// Bind groups bound before drawing.
    pub bind_groups: BindGroupList,
    /// Buffer created with `INDIRECT` usage holding indexed argument records.
    pub arguments: BufferId,
    /// Four-byte aligned byte offset of the argument record.
    pub offset: u64,
    /// Index buffer bound for this draw. Part of pipeline state, not the argument record.
    pub index_buffer: IndexBufferBinding,
    /// Optional scissor rectangle in target pixels.
    pub scissor: Option<ScissorRect>,
}

/// One recorded render command.
///
/// Commands execute in recording order inside their render pass.
#[derive(Clone, Debug, PartialEq)]
pub enum RenderCommand {
    /// Non-indexed draw.
    Draw(DrawCommand),
    /// Indexed draw.
    DrawIndexed(DrawIndexedCommand),
    /// GPU-generated non-indexed draw; requires advertised indirect-draw support.
    DrawIndirect(DrawIndirectCommand),
    /// GPU-generated indexed draw; requires advertised indirect-draw support.
    DrawIndexedIndirect(DrawIndexedIndirectCommand),
}

impl RenderCommand {
    /// Returns the pipeline used by this command.
    #[must_use]
    pub fn pipeline(&self) -> RenderPipelineId {
        match self {
            Self::Draw(command) => command.pipeline,
            Self::DrawIndexed(command) => command.pipeline,
            Self::DrawIndirect(command) => command.pipeline,
            Self::DrawIndexedIndirect(command) => command.pipeline,
        }
    }

    /// Returns the bind groups bound before this command.
    #[must_use]
    pub fn bind_groups(&self) -> &[BindGroupId] {
        match self {
            Self::Draw(command) => command.bind_groups.as_slice(),
            Self::DrawIndexed(command) => command.bind_groups.as_slice(),
            Self::DrawIndirect(command) => command.bind_groups.as_slice(),
            Self::DrawIndexedIndirect(command) => command.bind_groups.as_slice(),
        }
    }

    /// Returns the optional scissor rectangle for this command.
    #[must_use]
    pub fn scissor(&self) -> Option<ScissorRect> {
        match self {
            Self::Draw(command) => command.scissor,
            Self::DrawIndexed(command) => command.scissor,
            Self::DrawIndirect(command) => command.scissor,
            Self::DrawIndexedIndirect(command) => command.scissor,
        }
    }

    /// Replaces the scissor rectangle for this command.
    pub fn set_scissor(&mut self, scissor: Option<ScissorRect>) {
        match self {
            Self::Draw(command) => command.scissor = scissor,
            Self::DrawIndexed(command) => command.scissor = scissor,
            Self::DrawIndirect(command) => command.scissor = scissor,
            Self::DrawIndexedIndirect(command) => command.scissor = scissor,
        }
    }

    /// Returns the CPU-known instance count, or `None` for GPU-generated indirect arguments.
    #[must_use]
    pub fn instance_count(&self) -> Option<u32> {
        match self {
            Self::Draw(command) => Some(command.instance_count),
            Self::DrawIndexed(command) => Some(command.instance_count),
            Self::DrawIndirect(_) | Self::DrawIndexedIndirect(_) => None,
        }
    }
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
        if texture.dimension != TextureDimension::D2 {
            return Err(Error::InvalidInput(
                "use TextureArrayLayerWriteDescriptor for array-layer writes".into(),
            ));
        }
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

/// Depth attachment for a render pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DepthAttachment {
    /// Depth texture view target.
    pub target: TextureViewId,
    /// Depth load operation.
    pub depth_load_op: LoadOp<f32>,
    /// Optional stencil load operation.
    pub stencil_load_op: Option<LoadOp<u32>>,
}

/// Signed viewport origin used when a render region starts outside its attachment.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SignedOrigin2d {
    /// Horizontal viewport origin in pixels.
    pub x: i32,
    /// Vertical viewport origin in pixels.
    pub y: i32,
}

impl SignedOrigin2d {
    /// Zero origin.
    pub const ZERO: Self = Self { x: 0, y: 0 };
}

/// Controls how a viewport maps to its target allocation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ViewportMapping {
    /// Scale the viewport proportionally to the target allocation.
    #[default]
    Scaled,
    /// Keep source-pixel dimensions and origin, allowing the viewport to extend beyond a tile.
    PixelExact,
}

/// A viewport mapped onto a render target allocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Viewport {
    /// Viewport origin in source-pixel coordinates. A pixel-exact mapping may use a negative
    /// origin to render one content-space tile of a larger target.
    pub origin: SignedOrigin2d,
    /// Source extent in full-resolution pixels.
    pub extent: Extent2d,
    /// Reference allocation that `extent` is relative to.
    pub reference_extent: Extent2d,
    /// Mapping policy for the render-target viewport.
    pub mapping: ViewportMapping,
}

impl Viewport {
    /// Maps the source extent onto a full-resolution or downsampled render target.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] if the source extent exceeds its reference allocation.
    pub fn for_target(self, target_extent: Extent2d) -> Result<Extent2d> {
        if self.extent.width() > self.reference_extent.width()
            || self.extent.height() > self.reference_extent.height()
        {
            return Err(Error::InvalidInput(
                "render viewport exceeds its reference extent".into(),
            ));
        }
        if self.mapping == ViewportMapping::PixelExact {
            return Ok(self.extent);
        }
        let map_axis = |active: u32, reference: u32, target: u32| -> Result<u32> {
            let numerator = u64::from(active) * u64::from(target);
            let rounded_up = numerator.saturating_add(u64::from(reference - 1));
            u32::try_from((rounded_up / u64::from(reference)).max(1))
                .map_err(|error| Error::InvalidInput(format!("render viewport overflow: {error}")))
        };
        Extent2d::new(
            map_axis(
                self.extent.width(),
                self.reference_extent.width(),
                target_extent.width(),
            )?,
            map_axis(
                self.extent.height(),
                self.reference_extent.height(),
                target_extent.height(),
            )?,
        )
    }

    /// Maps the viewport origin to the target's pixel coordinate system.
    ///
    /// Pixel-exact mappings preserve signed offsets so a tile can rasterize a stable region of a
    /// larger content-space target without changing primitive coordinates.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] if the mapped origin overflows `i32`.
    pub fn mapped_origin(self, target_extent: Extent2d) -> Result<SignedOrigin2d> {
        if self.mapping == ViewportMapping::PixelExact {
            return Ok(self.origin);
        }
        let map_axis = |origin: i32, target: u32, reference: u32| -> Result<i32> {
            let scaled = i64::from(origin) * i64::from(target) / i64::from(reference);
            i32::try_from(scaled).map_err(|error| {
                Error::InvalidInput(format!("render viewport origin overflow: {error}"))
            })
        };
        Ok(SignedOrigin2d {
            x: map_axis(
                self.origin.x,
                target_extent.width(),
                self.reference_extent.width(),
            )?,
            y: map_axis(
                self.origin.y,
                target_extent.height(),
                self.reference_extent.height(),
            )?,
        })
    }

    /// Translates a source-space scissor into this pixel-exact viewport's target coordinates.
    ///
    /// Scaled viewports retain the existing scissor coordinates. Pixel-exact viewports translate
    /// them by the viewport origin and clip the result to the physical attachment, which lets a
    /// tile render the matching part of a larger target.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidInput`] if this viewport is invalid for `target_extent`.
    pub fn map_scissor(self, scissor: ScissorRect, target_extent: Extent2d) -> Result<ScissorRect> {
        if self.mapping != ViewportMapping::PixelExact {
            return Ok(scissor);
        }

        self.for_target(target_extent)?;

        fn clip_axis(start: u32, length: u32, origin: i32, limit: u32) -> (u32, u32) {
            let start = i64::from(start) + i64::from(origin);
            let end = start + i64::from(length);
            let clipped_start = start.clamp(0, i64::from(limit));
            let clipped_end = end.clamp(clipped_start, i64::from(limit));
            (
                u32::try_from(clipped_start)
                    .expect("clipped scissor origin is within the target extent"),
                u32::try_from(clipped_end - clipped_start)
                    .expect("clipped scissor length is within the target extent"),
            )
        }

        let (x, width) = clip_axis(
            scissor.x,
            scissor.width,
            self.origin.x,
            target_extent.width(),
        );
        let (y, height) = clip_axis(
            scissor.y,
            scissor.height,
            self.origin.y,
            target_extent.height(),
        );
        Ok(ScissorRect {
            x,
            y,
            width,
            height,
        })
    }

    /// Returns the target-local scissor extent for this mapping.
    ///
    /// Scaled viewports keep the source extent's scissor behavior. Pixel-exact viewports may
    /// span beyond a tile, so their scissor is clipped to the physical attachment.
    pub fn mapped_scissor_extent(self, target_extent: Extent2d) -> Result<Extent2d> {
        let viewport_extent = self.for_target(target_extent)?;
        if self.mapping == ViewportMapping::PixelExact {
            Extent2d::new(
                viewport_extent.width().min(target_extent.width()),
                viewport_extent.height().min(target_extent.height()),
            )
        } else {
            Ok(viewport_extent)
        }
    }
}

/// One ordered render command list targeting an offscreen texture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderPassCommands<'a> {
    /// Color texture views receiving this pass, in render-pass attachment order.
    pub color_attachments: &'a [TextureViewId],
    /// Render pass used by this operation.
    pub render_pass: RenderPassId,
    /// Borrowed commands recorded into the pass.
    pub commands: &'a [RenderCommand],
    /// Load behavior applied to every color attachment.
    pub color_load_op: LoadOp<ClearColor>,
    /// Optional rectangle limiting attachment Clear operations. A backend may
    /// conservatively clear the entire target when the optimization is unsupported.
    /// The caller must guarantee that pixels outside this region are not read
    /// from the cleared attachment before being initialized.
    pub clear_region: Option<ScissorRect>,
    /// Optional depth target and load behavior.
    pub depth_attachment: Option<DepthAttachment>,
    /// Optional viewport mapped onto the target. `None` renders across the entire target.
    pub viewport: Option<Viewport>,
    /// Single-sample texture view that receives the resolved result of a
    /// multisampled pass. `Some` requires [`RenderPassDescriptor::sample_count`]
    /// greater than one; `None` means single-sampled or no resolve.
    pub resolve_target: Option<TextureViewId>,
}

/// Validates the attachment sample counts of one recorded pass against its render pass.
///
/// A multisampled pass requires every attachment to share the pass sample count, and a
/// resolve destination must be single-sampled. Backends call this once per pass so the
/// invariant (and its error text) lives in one place instead of being re-derived, or
/// silently violated, per backend.
///
/// # Errors
/// Returns [`Error::InvalidInput`] naming the offending attachment role.
pub fn validate_pass_attachments(
    pass_sample_count: u32,
    color_samples: impl Iterator<Item = u32>,
    depth_samples: Option<u32>,
    resolve_samples: Option<u32>,
) -> Result<()> {
    for (index, samples) in color_samples.enumerate() {
        if samples != pass_sample_count {
            return Err(Error::InvalidInput(format!(
                "color attachment {index} has {samples} samples but the render pass uses {pass_sample_count}"
            )));
        }
    }
    if let Some(samples) = depth_samples
        && samples != pass_sample_count
    {
        return Err(Error::InvalidInput(format!(
            "depth attachment has {samples} samples but the render pass uses {pass_sample_count}"
        )));
    }
    if resolve_samples.is_some() && pass_sample_count == 1 {
        return Err(Error::InvalidInput(
            "resolve targets require a multisampled render pass".into(),
        ));
    }
    if let Some(samples) = resolve_samples
        && samples != 1
    {
        return Err(Error::InvalidInput(format!(
            "resolve target must be single-sampled but has {samples} samples"
        )));
    }
    Ok(())
}

/// One swapchain frame to record and present.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PresentFrameDescriptor<'a> {
    /// Swapchain receiving the frame.
    pub swapchain: SwapchainId,
    /// Render pass layout used to record the frame.
    pub render_pass: RenderPassId,
    /// Commands recorded into the swapchain image.
    pub commands: &'a [RenderCommand],
    /// Color load operation for the swapchain image.
    pub color_load_op: LoadOp<ClearColor>,
    /// Optional depth target and load behavior.
    pub depth_attachment: Option<DepthAttachment>,
    /// Changed region passed to native incremental presentation when supported.
    pub present_region: Option<ScissorRect>,
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
    /// Live bind group layouts.
    pub bind_group_layouts: usize,
    /// Live bind groups.
    pub bind_groups: usize,
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
    fn depth_state_defaults_to_less_equal_with_depth_writes() {
        assert_eq!(
            DepthState::default(),
            DepthState {
                compare: CompareFunction::LessEqual,
                write_enabled: true,
                stencil: None,
            }
        );
    }

    #[test]
    fn stencil_state_defaults_to_always_pass_keep() {
        let stencil = StencilState::default();
        assert_eq!(stencil.front, StencilFaceState::default());
        assert_eq!(stencil.back, StencilFaceState::default());
        assert_eq!(stencil.read_mask, u32::MAX);
        assert_eq!(stencil.write_mask, u32::MAX);
        assert_eq!(stencil.front.compare, CompareFunction::Always);
        assert_eq!(stencil.front.fail_op, StencilOp::Keep);
        assert_eq!(stencil.front.pass_op, StencilOp::Keep);
    }

    #[test]
    fn texture_sample_count_must_be_power_of_two() {
        let mut descriptor = texture_desc(16, 16);
        descriptor.sample_count = 0;
        assert!(descriptor.validate().is_err());
        descriptor.sample_count = 3;
        assert!(descriptor.validate().is_err());
        descriptor.sample_count = 128;
        assert!(descriptor.validate().is_err());
        descriptor.sample_count = 4;
        assert!(descriptor.validate().is_ok());
    }

    #[test]
    fn msaa_texture_size_accounts_for_all_samples() {
        let mut descriptor = texture_desc(16, 16);
        descriptor.sample_count = 4;
        assert_eq!(descriptor.byte_size(), 16 * 16 * 4 * 4);
    }

    #[test]
    fn multisampled_texture_rejects_mip_chain_and_array_layers() {
        let mut desc = texture_desc(16, 16);
        desc.sample_count = 4;
        desc.mip_level_count = 2;
        assert!(desc.validate().is_err());
        desc.mip_level_count = 1;
        assert!(desc.validate().is_ok());
        desc.dimension = TextureDimension::D2Array { layers: 2 };
        assert!(desc.validate().is_err());
    }

    #[test]
    fn multisample_capabilities_defaults_to_single_sample() {
        let caps = MultisampleCapabilities::default();
        assert_eq!(caps.max_sample_count, 1);
        assert!(caps.stencil_supported);
    }

    #[test]
    fn extent_rejects_zero_dimensions() {
        assert!(Extent2d::new(0, 1).is_err());
        assert!(Extent2d::new(1, 0).is_err());
        assert!(Extent2d::new(1, 1).is_ok());
    }

    #[test]
    fn surface_config_uses_backend_defaults_when_pacing_is_unset() {
        let config = SurfaceConfig::new(640, 480, Format::Bgra8Unorm).expect("valid config");
        assert_eq!(config.desired_image_count, 0);
        assert_eq!(config.desired_maximum_frame_latency, 0);
        assert_eq!(config.resolve_image_count(3, 2, 16), 3);
        assert_eq!(config.resolve_frame_latency(2, 1, 15), 2);
    }

    #[test]
    fn surface_config_clamps_requested_pacing_to_backend_range() {
        let mut config = SurfaceConfig::new(640, 480, Format::Bgra8Unorm).expect("valid config");
        config.desired_image_count = 1;
        config.desired_maximum_frame_latency = 32;
        assert_eq!(config.resolve_image_count(3, 2, 16), 2);
        assert_eq!(config.resolve_frame_latency(2, 1, 15), 15);
        config.desired_image_count = 24;
        assert_eq!(config.resolve_image_count(3, 2, 16), 16);
    }

    #[test]
    fn surface_config_keeps_explicit_pacing_inside_range() {
        let mut config = SurfaceConfig::new(640, 480, Format::Bgra8Unorm).expect("valid config");
        config.desired_image_count = 4;
        config.desired_maximum_frame_latency = 3;
        assert_eq!(config.resolve_image_count(3, 2, 16), 4);
        assert_eq!(config.resolve_frame_latency(2, 1, 15), 3);
    }

    #[test]
    fn surface_config_treats_inverted_range_as_minimum() {
        let config = SurfaceConfig::new(640, 480, Format::Bgra8Unorm).expect("valid config");
        assert_eq!(config.resolve_image_count(0, 4, 2), 4);
        assert_eq!(config.resolve_frame_latency(0, 4, 2), 4);
    }

    #[test]
    fn sampler_descriptor_rejects_inverted_lod_range() {
        let mut sampler = SamplerDescriptor::default();
        sampler.lod_min_clamp = 2.0;
        sampler.lod_max_clamp = 1.0;
        assert!(sampler.validate().is_err());

        let mut sampler = SamplerDescriptor::default();
        sampler.lod_min_clamp = 1.0;
        sampler.lod_max_clamp = 2.0;
        assert!(sampler.validate().is_ok());
    }

    #[test]
    fn sampler_descriptor_defaults_to_opaque_sampling() {
        let sampler = SamplerDescriptor::default();
        assert_eq!(sampler.compare, None);
        assert_eq!(sampler.border_color, None);
        assert_eq!(sampler.address_mode_w, AddressMode::ClampToEdge);
        assert_eq!(sampler.lod_min_clamp, 0.0);
        assert_eq!(sampler.lod_max_clamp, f32::MAX);
        assert_eq!(sampler.lod_bias, 0.0);
    }

    #[test]
    fn device_tuning_defaults_to_backend_values() {
        let tuning = DeviceTuning::default();
        assert_eq!(tuning.command_encoder_pool_capacity, 0);
        assert_eq!(tuning.upload_command_pool_capacity, 0);
        assert_eq!(tuning.resource_descriptor_capacity, 0);
        assert_eq!(tuning.render_target_descriptor_capacity, 0);
        assert_eq!(tuning.depth_stencil_descriptor_capacity, 0);
        assert_eq!(tuning.pipeline_cache_limit_bytes, 0);
        assert_eq!(tuning.upload_page_bytes, 0);
        assert_eq!(tuning.upload_retained_idle_pages, 0);
        assert_eq!(tuning.present_timeout_micros, 0);
        assert_eq!(DeviceDescriptor::default().tuning, tuning);
    }

    #[test]
    fn viewport_scales_to_target_capacity() {
        let viewport = Viewport {
            origin: SignedOrigin2d::ZERO,
            extent: Extent2d::new(520, 522).expect("client extent"),
            reference_extent: Extent2d::new(768, 768).expect("allocation extent"),
            mapping: ViewportMapping::Scaled,
        };

        assert_eq!(
            viewport
                .for_target(Extent2d::new(768, 768).expect("full target"))
                .expect("full target viewport"),
            viewport.extent,
        );
        assert_eq!(
            viewport
                .for_target(Extent2d::new(384, 384).expect("half-resolution target"))
                .expect("scaled viewport"),
            Extent2d::new(260, 261).expect("scaled viewport extent"),
        );
    }

    #[test]
    fn viewport_rejects_extent_outside_reference() {
        let viewport = Viewport {
            origin: SignedOrigin2d::ZERO,
            extent: Extent2d::new(769, 768).expect("client extent"),
            reference_extent: Extent2d::new(768, 768).expect("allocation extent"),
            mapping: ViewportMapping::Scaled,
        };

        assert!(
            viewport
                .for_target(Extent2d::new(768, 768).expect("target extent"))
                .is_err()
        );
    }

    #[test]
    fn pixel_exact_viewport_translates_and_clips_scissors_to_tile() {
        let viewport = Viewport {
            origin: SignedOrigin2d { x: -12, y: 8 },
            extent: Extent2d::new(64, 48).expect("client extent"),
            reference_extent: Extent2d::new(64, 48).expect("allocation extent"),
            mapping: ViewportMapping::PixelExact,
        };
        let target_extent = Extent2d::new(64, 48).expect("target extent");
        let scissor = ScissorRect {
            x: 0,
            y: 0,
            width: 32,
            height: 32,
        };

        assert_eq!(
            viewport
                .map_scissor(scissor, target_extent)
                .expect("clipped scissor"),
            ScissorRect {
                x: 0,
                y: 8,
                width: 20,
                height: 32,
            },
        );
    }

    #[test]
    fn scaled_viewport_keeps_existing_scissor_coordinates() {
        let viewport = Viewport {
            origin: SignedOrigin2d { x: 10, y: 20 },
            extent: Extent2d::new(100, 100).expect("client extent"),
            reference_extent: Extent2d::new(100, 100).expect("allocation extent"),
            mapping: ViewportMapping::Scaled,
        };
        let scissor = ScissorRect {
            x: 5,
            y: 6,
            width: 30,
            height: 40,
        };

        assert_eq!(
            viewport
                .map_scissor(scissor, Extent2d::new(50, 50).expect("target extent"))
                .expect("scaled scissor"),
            scissor,
        );
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
            sample_count: 1,
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
            sample_count: 1,
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
            color_targets: vec![ColorTargetState::new(
                Format::Bgra8Unorm,
                BlendMode::Replace,
            )],
            primitive_topology: PrimitiveTopology::TriangleList,
            raster: Default::default(),
            depth_state: None,
        };

        assert!(descriptor.validate().is_err());
    }

    #[test]
    fn color_target_state_defaults_to_full_write_mask() {
        let target = ColorTargetState::new(Format::Rgba16Float, BlendMode::Alpha);
        assert_eq!(target.format, Format::Rgba16Float);
        assert_eq!(target.blend, BlendMode::Alpha);
        assert_eq!(target.write_mask, ColorWrites::ALL);
        assert_eq!(ColorWrites::default(), ColorWrites::ALL);
    }

    #[test]
    fn color_writes_compose_color_and_alpha_channels() {
        assert_eq!(
            ColorWrites::COLOR,
            ColorWrites::RED | ColorWrites::GREEN | ColorWrites::BLUE
        );
        assert!(ColorWrites::ALL.contains(ColorWrites::COLOR));
        assert!(ColorWrites::ALL.contains(ColorWrites::ALPHA));
        assert!(!ColorWrites::COLOR.contains(ColorWrites::ALPHA));
    }

    #[test]
    fn render_pass_validation_requires_one_to_max_color_attachments() {
        let attachment = ColorAttachmentDescriptor {
            format: Format::Rgba16Float,
        };
        let descriptor = RenderPassDescriptor {
            label: None,
            color_attachments: Vec::new(),
            depth_attachment: None,
            sample_count: 1,

            resolve_target: None,
        };
        assert!(descriptor.validate().is_err());

        let descriptor = RenderPassDescriptor {
            label: None,
            color_attachments: vec![attachment; MAX_COLOR_ATTACHMENTS],
            depth_attachment: None,
            sample_count: 1,

            resolve_target: None,
        };
        assert!(descriptor.validate().is_ok());

        let descriptor = RenderPassDescriptor {
            label: None,
            color_attachments: vec![attachment; MAX_COLOR_ATTACHMENTS + 1],
            depth_attachment: None,
            sample_count: 1,

            resolve_target: None,
        };
        assert!(descriptor.validate().is_err());
    }

    #[test]
    fn storage_textures_are_single_sampled_single_level_and_color() {
        let base = TextureDescriptor {
            label: None,
            size: Extent2d::new(4, 4).unwrap(),
            mip_level_count: 1,
            format: Format::Rgba8Unorm,
            usage: TextureUsage::STORAGE | TextureUsage::COPY_SRC,
            memory_location: MemoryLocation::GpuOnly,
            dimension: TextureDimension::D2,
            sample_count: 1,
        };
        assert!(base.validate().is_ok());
        // Multisampled storage images are not addressable by a shader.
        let mut multisampled = base.clone();
        multisampled.sample_count = 4;
        assert!(multisampled.validate().is_err());
        // Storage images have no mip chain.
        let mut mips = base.clone();
        mips.mip_level_count = 2;
        assert!(mips.validate().is_err());
        // Depth formats cannot be storage images.
        let mut depth = base.clone();
        depth.format = Format::Depth32Float;
        assert!(depth.validate().is_err());
        // A storage-texture binding only accepts a texture resource.
        assert!(
            BindingType::StorageTexture {
                format: Format::Rgba8Unorm,
                read_only: false,
            }
            .matches(&BindingResource::Texture(TextureBinding {
                texture_view: ResourceId::new(1),
            }))
        );
    }

    #[test]
    fn indexed_indirect_command_exposes_its_draw_state() {
        let command = RenderCommand::DrawIndexedIndirect(DrawIndexedIndirectCommand {
            pipeline: ResourceId::new(7),
            bind_groups: bind_group_list([ResourceId::new(9)]),
            arguments: ResourceId::new(11),
            offset: 32,
            index_buffer: IndexBufferBinding {
                buffer: ResourceId::new(13),
                format: IndexFormat::Uint16,
                offset: 4,
            },
            scissor: Some(ScissorRect {
                x: 1,
                y: 2,
                width: 3,
                height: 4,
            }),
        });
        assert_eq!(command.pipeline(), ResourceId::new(7));
        assert_eq!(command.bind_groups(), &[ResourceId::new(9)]);
        assert_eq!(
            command.scissor(),
            Some(ScissorRect {
                x: 1,
                y: 2,
                width: 3,
                height: 4,
            })
        );
        // Indexed indirect counts live in the GPU argument record, not on the CPU.
        assert_eq!(command.instance_count(), None);
    }

    #[test]
    fn integer_color_targets_reject_blending() {
        let base = RenderPipelineDescriptor {
            label: None,
            vertex_shader: ResourceId::new(1),
            vertex_entry_point: "vs".into(),
            fragment_shader: ResourceId::new(2),
            fragment_entry_point: "fs".into(),
            vertex_buffers: Vec::new(),
            render_pass: ResourceId::new(3),
            pipeline_layout: None,
            color_targets: vec![ColorTargetState::new(Format::Rgba8Uint, BlendMode::Replace)],
            primitive_topology: PrimitiveTopology::TriangleList,
            raster: RasterState::default(),
            depth_state: None,
        };
        assert!(base.validate().is_ok());
        let mut blended = base.clone();
        blended.color_targets[0].blend = BlendMode::Alpha;
        assert!(blended.validate().is_err());
    }

    #[test]
    fn vertex_layouts_must_fit_the_stride_and_unique_locations() {
        let descriptor = |layout: VertexBufferLayoutDescriptor| RenderPipelineDescriptor {
            label: None,
            vertex_shader: ResourceId::new(1),
            vertex_entry_point: "vs".into(),
            fragment_shader: ResourceId::new(2),
            fragment_entry_point: "fs".into(),
            vertex_buffers: vec![layout],
            render_pass: ResourceId::new(3),
            pipeline_layout: None,
            color_targets: vec![ColorTargetState::new(
                Format::Rgba8Unorm,
                BlendMode::Replace,
            )],
            primitive_topology: PrimitiveTopology::TriangleList,
            raster: RasterState::default(),
            depth_state: None,
        };
        let attribute = |location, offset, format| VertexAttributeDescriptor {
            location,
            offset,
            format,
        };
        // Position + normal inside a 24-byte element is valid.
        assert!(
            descriptor(VertexBufferLayoutDescriptor {
                stride: 24,
                step_mode: VertexStepMode::Vertex,
                attributes: vec![
                    attribute(0, 0, VertexFormat::Float32x3),
                    attribute(1, 12, VertexFormat::Float32x3),
                ],
            })
            .validate()
            .is_ok()
        );
        // An attribute past the stride is rejected.
        assert!(
            descriptor(VertexBufferLayoutDescriptor {
                stride: 12,
                step_mode: VertexStepMode::Instance,
                attributes: vec![attribute(0, 8, VertexFormat::Float32x3)],
            })
            .validate()
            .is_err()
        );
        // Repeating a shader location is rejected.
        assert!(
            descriptor(VertexBufferLayoutDescriptor {
                stride: 32,
                step_mode: VertexStepMode::Vertex,
                attributes: vec![
                    attribute(0, 0, VertexFormat::Float32x4),
                    attribute(0, 16, VertexFormat::Float32x4),
                ],
            })
            .validate()
            .is_err()
        );
    }

    #[test]
    fn pass_attachment_samples_must_match_the_render_pass() {
        // Matching color/depth samples with a single-sampled resolve destination pass.
        assert!(validate_pass_attachments(4, [4, 4].into_iter(), Some(4), Some(1)).is_ok());
        assert!(validate_pass_attachments(1, [1].into_iter(), Some(1), None).is_ok());
        // A color attachment with another sample count is rejected, naming its index.
        assert!(validate_pass_attachments(4, [4, 1].into_iter(), None, None).is_err());
        // The depth attachment must match too: a 4x pass cannot use a 1x depth map.
        assert!(validate_pass_attachments(4, [4].into_iter(), Some(1), None).is_err());
        // A resolve destination must be single-sampled.
        assert!(validate_pass_attachments(4, [4].into_iter(), None, Some(4)).is_err());
        // A single-sampled command pass may not request a native resolve at all.
        assert!(validate_pass_attachments(1, [1].into_iter(), None, Some(1)).is_err());
    }

    #[test]
    fn render_pass_validation_requires_resolve_only_with_multisampling() {
        let attachment = ColorAttachmentDescriptor {
            format: Format::Rgba16Float,
        };
        // Single-sampled pass with a resolve target is rejected.
        let descriptor = RenderPassDescriptor {
            label: None,
            color_attachments: vec![attachment],
            depth_attachment: None,
            sample_count: 1,
            resolve_target: Some(attachment),
        };
        assert!(descriptor.validate().is_err());

        // Multisampled pass with a default single-sample resolve is valid.
        let descriptor = RenderPassDescriptor {
            label: None,
            color_attachments: vec![attachment],
            depth_attachment: None,
            sample_count: 4,
            resolve_target: Some(attachment),
        };
        assert!(descriptor.validate().is_ok());

        // Multisampled pass without a resolve target is still valid.
        let descriptor = RenderPassDescriptor {
            label: None,
            color_attachments: vec![attachment],
            depth_attachment: None,
            sample_count: 4,
            resolve_target: None,
        };
        assert!(descriptor.validate().is_ok());

        // Resolve format mismatch is rejected.
        let descriptor = RenderPassDescriptor {
            label: None,
            color_attachments: vec![attachment],
            depth_attachment: None,
            sample_count: 4,
            resolve_target: Some(ColorAttachmentDescriptor {
                format: Format::Rgba8Unorm,
            }),
        };
        assert!(descriptor.validate().is_err());
    }

    #[test]
    fn pipeline_validation_rejects_empty_color_targets() {
        let descriptor = RenderPipelineDescriptor {
            label: None,
            vertex_shader: ShaderModuleId::from_parts(1, 1),
            vertex_entry_point: "vs_main".to_string(),
            fragment_shader: ShaderModuleId::from_parts(2, 1),
            fragment_entry_point: "fs_main".to_string(),
            vertex_buffers: Vec::new(),
            render_pass: RenderPassId::from_parts(3, 1),
            pipeline_layout: None,
            color_targets: Vec::new(),
            primitive_topology: PrimitiveTopology::TriangleList,
            raster: Default::default(),
            depth_state: None,
        };

        assert!(descriptor.validate().is_err());
    }

    #[test]
    fn resource_set_layout_rejects_duplicate_bindings() {
        let descriptor = BindGroupLayoutDescriptor {
            label: None,
            entries: vec![
                BindGroupLayoutEntry {
                    binding: 0,
                    binding_type: BindingType::UniformBuffer,
                    stages: ShaderStages::VERTEX,
                },
                BindGroupLayoutEntry {
                    binding: 0,
                    binding_type: BindingType::Sampler,
                    stages: ShaderStages::FRAGMENT,
                },
            ],
        };

        assert!(descriptor.validate().is_err());
    }

    #[test]
    fn resource_set_rejects_mismatched_binding_type() {
        let layout = BindGroupLayoutDescriptor {
            label: None,
            entries: vec![BindGroupLayoutEntry {
                binding: 0,
                binding_type: BindingType::SampledTexture,
                stages: ShaderStages::FRAGMENT,
            }],
        };
        let descriptor = BindGroupDescriptor {
            label: None,
            layout: BindGroupLayoutId::from_parts(1, 1),
            entries: vec![BindGroupEntry {
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
        let layout = BindGroupLayoutDescriptor {
            label: None,
            entries: vec![
                BindGroupLayoutEntry {
                    binding: 0,
                    binding_type: BindingType::UniformBuffer,
                    stages: ShaderStages::VERTEX,
                },
                BindGroupLayoutEntry {
                    binding: 2,
                    binding_type: BindingType::Sampler,
                    stages: ShaderStages::FRAGMENT,
                },
            ],
        };
        let descriptor = BindGroupDescriptor {
            label: None,
            layout: BindGroupLayoutId::from_parts(1, 1),
            entries: vec![
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::Sampler(SamplerBinding {
                        sampler: SamplerId::from_parts(2, 1),
                    }),
                },
                BindGroupEntry {
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
            sample_count: 1,
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
