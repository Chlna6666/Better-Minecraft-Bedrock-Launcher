//! Vulkan backend for nova-gfx.
//!
//! This crate implements the `gfx-core` device traits for Vulkan. Native
//! raw-window-handle integration is kept in this backend crate, not in
//! `gfx-core`.
//!
//! Chinese documentation is available in `README.zh-CN.md` in the crate source
//! package.

#![expect(
    unsafe_code,
    reason = "Vulkan FFI requires unsafe calls; each unsafe block documents its safety invariant"
)]

use std::{
    ffi::{CStr, CString},
    fs,
    mem::ManuallyDrop,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use crate::error::VulkanError;
use crate::registry::ResourceRegistry;
use ash::{Entry, Instance, khr, vk};
use gfx_core::{
    AdapterInfo, AddressMode, Backend, BackendCapabilities, BackendKind, BeginRenderPassDescriptor,
    BindGroupDescriptor, BindGroupId, BindGroupLayoutDescriptor, BindGroupLayoutId,
    BindingResource, BindingType, BlendMode, BufferDescriptor, BufferId, BufferUploadStats,
    BufferUsage, BufferWrite, ClearColor, ColorAttachmentDescriptor, ColorTargetState, ColorWrites,
    CommandDevice, CommandEncoderDescriptor, CommandEncoderId, CompareFunction, CompositeAlphaMode,
    DepthAttachment, DeviceDescriptor, DeviceMemoryBudget, DiagnosticsDevice, DrawCommand,
    DrawDescriptor, Error, FilterMode, Format, FrameGpuTimestamps, IndexBufferBinding, IndexFormat,
    LoadOp, MemoryAccounting, MemoryBudget, MemoryLocation, MemoryTrimLevel, PipelineDevice,
    PipelineLayoutDescriptor, PipelineLayoutId, PowerPreference, PresentFrameDescriptor,
    PresentMode, PresentTimings, PresentationDevice, PresentedFrame, PrimitiveTopology,
    RenderCommand, RenderPassCommands, RenderPassDescriptor, RenderPassId,
    RenderPipelineDescriptor, RenderPipelineId, RenderTarget, ResourceDevice, ResourceStats,
    Result, SamplerDescriptor, SamplerId, ScissorRect, ShaderBinary, ShaderCode,
    ShaderModuleDescriptor, ShaderModuleId, ShaderStage, ShaderStages, SubmissionDevice,
    SubmissionId, SubmissionStatus, SurfaceConfig, SurfaceDescriptor, SurfaceDevice, SurfaceId,
    TextureDataLayout, TextureDescriptor, TextureDimension, TextureId, TextureReadback,
    TextureTransferDevice, TextureUsage, TextureViewDescriptor, TextureViewId, TextureWrite,
    TextureWriteDescriptor, ThreadingMode, VertexFormat, Viewport,
};
use gfx_memory::{
    DeferredFreeQueue, MemoryAllocation, MemoryAllocator, UploadAllocation, UploadRingAllocator,
    UploadRingAllocatorDesc, VulkanMemoryAllocatorDesc,
};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
mod buffer_upload;
mod compact;
mod compute;
mod depth;
mod descriptors;
mod presentation;
mod relocation;
mod transfer_queue;
mod submission_wake;
mod timeline;
mod submit;

// Desktop has alternate renderers; Android 15 does not guarantee Vulkan 1.4.
// ash 0.38 lacks the named 1.4 constant, but the API version encoding is stable.
// Use only bound core 1.3 commands until the whole ash dependency graph is updated.
const PREFERRED_API_VERSION: u32 = vk::make_api_version(0, 1, 4, 0);
const REQUIRED_API_VERSION: u32 = if cfg!(target_os = "android") {
    vk::API_VERSION_1_3
} else {
    PREFERRED_API_VERSION
};
const DEFAULT_FRAME_SLOTS: u32 = 2;
// A transient WSI stall must not discard a prepared compositor frame after only 1 ms.
// Bound the wait so a hidden/occluded window cannot block the GPU owner indefinitely.
const DEFAULT_PRESENT_TIMEOUT_NS: u64 = 8_000_000;
const DEFAULT_UPLOAD_COMMAND_POOL_CAPACITY: usize = 4;
// GPU-completed command pools are recycled instead of recreated for each blur/frame pass.
const DEFAULT_COMMAND_ENCODER_POOL_CAPACITY: usize = 16;
const DEFAULT_PIPELINE_CACHE_BYTES: u64 = 64 * 1024 * 1024;

/// Native presentation target accepted by the Vulkan backend.
pub trait VulkanSurfaceTarget: HasDisplayHandle + HasWindowHandle {}

impl<T> VulkanSurfaceTarget for T where T: HasDisplayHandle + HasWindowHandle + ?Sized {}

/// Metrics captured by the triangle baseline.
#[derive(Clone, Debug, Default)]
pub struct BaselineMetrics {
    /// Time spent creating Vulkan instance, device, surface, swapchain, and pipeline.
    pub startup_time: std::time::Duration,
    /// Time from context creation start until the first submitted frame.
    pub first_frame_time: Option<std::time::Duration>,
    /// Number of submitted frames.
    pub submitted_frames: u64,
}

/// Enumerates Vulkan physical devices visible through the loader.
///
/// # Errors
///
/// Returns [`Error`] if the Vulkan loader, instance creation, or physical
/// device enumeration fails.
pub fn enumerate_adapter_info() -> Result<Vec<AdapterInfo>> {
    let entry = load_entry()?;
    let (instance, _) = create_instance(&entry, "nova-gfx adapter enumeration")?;
    let devices = match unsafe { instance.enumerate_physical_devices() } {
        Ok(devices) => devices,
        Err(error) => {
            // SAFETY: no device resources exist yet.
            unsafe { instance.destroy_instance(None) };
            return Err(VulkanError::from(error).into());
        }
    };
    let mut adapters = Vec::with_capacity(devices.len());
    for physical_device in devices {
        // SAFETY: Physical device belongs to this instance.
        let properties = unsafe { instance.get_physical_device_properties(physical_device) };
        if !supports_modern_device(&instance, physical_device, &properties)
            || queue_family_indices_without_surface(&instance, physical_device).is_err()
        {
            continue;
        }
        // SAFETY: Vulkan guarantees device_name is a null-terminated C string.
        let name = unsafe { CStr::from_ptr(properties.device_name.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        adapters.push(AdapterInfo {
            backend: BackendKind::Vulkan,
            name,
            vendor_id: properties.vendor_id,
            device_id: properties.device_id,
            capabilities: BackendCapabilities {
                surface: true,
                cpu_visible_memory: true,
                gpu_only_memory: true,
            },
        });
    }
    // SAFETY: Instance is no longer used after enumeration.
    unsafe {
        instance.destroy_instance(None);
    }
    Ok(adapters)
}

/// Generic Vulkan device and resource owner.
pub struct VulkanDevice {
    swapchain_maintenance: bool,
    native_pixel_presentation: Option<khr::get_surface_capabilities2::Instance>,
    memory_budget_supported: bool,
    host_query_reset: bool,
    draw_indirect_count: bool,
    entry: Entry,
    instance: Instance,
    surface_loader: khr::surface::Instance,
    physical_device: vk::PhysicalDevice,
    adapter_name: String,
    max_sampler_anisotropy: f32,
    max_color_attachment_count: Option<u32>,
    max_sample_count: u32,
    stencil_supported: bool,
    multi_draw_indirect: bool,
    max_draw_indirect_count: u32,
    binding_limits: gfx_core::BufferBindingLimits,
    texture_array_limits: gfx_core::TextureArrayCapabilities,
    copy_row_pitch_alignment: u64,
    copy_offset_alignment: u64,
    device: Arc<ash::Device>,
    graphics_queue: vk::Queue,
    /// Secondary graphics-family queue can execute all stages with no
    /// family ownership transfer.
    sidecar_queue: Option<vk::Queue>,
    /// Compute-only family, when present. Only concurrent-shared buffer
    /// dispatches are permitted; images remain graphics-family owned.
    dedicated_compute_queue: Option<vk::Queue>,
    dedicated_compute_family_index: Option<u32>,
    present_queue: vk::Queue,
    graphics_queue_family_index: u32,
    present_queue_family_index: u32,
    swapchain_loader: khr::swapchain::Device,
    display_timing: Option<ash::google::display_timing::Device>,
    present_wait: Option<khr::present_wait::Device>,
    allocator: ManuallyDrop<MemoryAllocator>,
    buffers: ResourceRegistry<VulkanBuffer>,
    textures: ResourceRegistry<VulkanTexture>,
    texture_views: ResourceRegistry<VulkanTextureView>,
    samplers: ResourceRegistry<VulkanSampler>,
    bind_group_layouts: ResourceRegistry<VulkanResourceSetLayout>,
    bind_groups: ResourceRegistry<VulkanResourceSet>,
    pipeline_layouts: ResourceRegistry<VulkanPipelineLayout>,
    shader_modules: ResourceRegistry<VulkanShaderModule>,
    compute: compute::State,
    render_passes: ResourceRegistry<VulkanRenderPass>,
    render_pipelines: ResourceRegistry<VulkanRenderPipeline>,
    command_encoders: ResourceRegistry<VulkanCommandEncoder>,
    free_command_encoders: Vec<VulkanCommandEncoder>,
    submissions: ResourceRegistry<VulkanSubmission>,
    submission_wakes: submission_wake::SubmissionWakes,
    primary_timeline: Option<timeline::QueueTimeline>,
    secondary_timeline: Option<timeline::QueueTimeline>,
    /// Completed via a native fence on the waiting graphics-family queue.
    /// Binary semaphores must remain alive until that GPU wait executes.
    native_queue_waits: Vec<VulkanQueueWait>,
    surfaces: ResourceRegistry<VulkanSurface>,
    swapchains: ResourceRegistry<VulkanSwapchain>,
    descriptor_pools: descriptors::Pools,
    pipeline_cache: vk::PipelineCache,
    pipeline_cache_path: Option<PathBuf>,
    upload_ring: UploadRingAllocator,
    upload_pages: Vec<Option<VulkanBuffer>>,
    upload_command_pool: Vec<VulkanUploadCommands>,
    timestamp_clock: Option<VulkanTimestampClock>,
    /// Native Compute Queue Families may report different timestampValidBits
    /// or zero. The graphics clock must never decode an unrelated family.
    dedicated_compute_timestamp_clock: Option<VulkanTimestampClock>,
    /// Opt-in hardware timestamps for main/offscreen draw, not CPU command encoding.
    gpu_trace_enabled: bool,
    gpu_trace_totals: VulkanGpuTraceTotals,
    last_frame_gpu_timestamps: Option<FrameGpuTimestamps>,
    completed_frame_probes: std::collections::VecDeque<(SubmissionId, FrameGpuTimestamps)>,
    gpu_work_samples: Vec<gfx_core::GpuTimestampSample>,
    last_texture_transfer_time: Option<Duration>,
    deferred_destroys: DeferredFreeQueue<DeferredResource>,
    next_upload_fence_value: u64,
    submitted_frames: u64,
    incremental_presentation: bool,
    command_encoder_pool_capacity: usize,
    upload_command_pool_capacity: usize,
    pipeline_cache_limit_bytes: u64,
    present_timeout_ns: u64,
}

fn vulkan_pipeline_cache_path(
    root: Option<&Path>,
    properties: &vk::PhysicalDeviceProperties,
) -> Option<PathBuf> {
    let root = root?;
    let dir = root.join("vulkan");
    if let Err(error) = fs::create_dir_all(&dir) {
        log::warn!(
            "nova-gfx Vulkan pipeline cache disabled: path={} error={error}",
            dir.display()
        );
        return None;
    }

    let mut uuid = String::with_capacity(properties.pipeline_cache_uuid.len() * 2);
    for byte in properties.pipeline_cache_uuid {
        use core::fmt::Write as _;
        let _ = write!(&mut uuid, "{byte:02x}");
    }

    Some(dir.join(format!(
        "{:04x}-{:04x}-{uuid}.bin",
        properties.vendor_id, properties.device_id
    )))
}

fn read_pipeline_cache_data(path: &Path, limit_bytes: u64) -> Option<Vec<u8>> {
    let bytes = fs::read(path).ok()?;
    let size = u64::try_from(bytes.len()).ok()?;
    if bytes.is_empty() || size > limit_bytes {
        return None;
    }
    Some(bytes)
}

fn create_pipeline_cache(
    device: &ash::Device,
    initial_data: Option<&[u8]>,
    cache_path: Option<&Path>,
) -> Result<vk::PipelineCache> {
    let info = initial_data.map_or_else(vk::PipelineCacheCreateInfo::default, |data| {
        vk::PipelineCacheCreateInfo::default().initial_data(data)
    });
    // SAFETY: Initial data is borrowed for this call and the logical device is live.
    match unsafe { device.create_pipeline_cache(&info, None) } {
        Ok(cache) => Ok(cache),
        Err(error) if initial_data.is_some() => {
            log::debug!(
                "nova-gfx Vulkan rejected pipeline cache; retrying empty: path={} error={error:?}",
                cache_path.map_or_else(|| "<none>".to_string(), |path| path.display().to_string())
            );
            // SAFETY: Empty create info has no borrowed pointers and the device is live.
            let cache = unsafe {
                device.create_pipeline_cache(&vk::PipelineCacheCreateInfo::default(), None)
            }
            .map_err(VulkanError::from)?;
            Ok(cache)
        }
        Err(error) => Err(VulkanError::from(error).into()),
    }
}

fn persist_pipeline_cache(
    device: &ash::Device,
    cache: vk::PipelineCache,
    path: Option<&Path>,
    limit_bytes: u64,
) {
    let Some(path) = path else {
        return;
    };
    // SAFETY: Pipeline cache belongs to this live device and device work is idle at drop.
    let Ok(bytes) = (unsafe { device.get_pipeline_cache_data(cache) }) else {
        return;
    };
    let Ok(size) = u64::try_from(bytes.len()) else {
        return;
    };
    if bytes.is_empty() || size > limit_bytes {
        return;
    }
    if let Err(error) = fs::write(path, bytes) {
        log::debug!(
            "nova-gfx Vulkan pipeline cache write skipped: path={} error={error}",
            path.display()
        );
    }
}

impl VulkanDevice {
    /// Creates a Vulkan device.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if Vulkan initialization fails.
    pub fn new(desc: &DeviceDescriptor) -> Result<Self> {
        let entry = load_entry()?;
        let (instance, instance_api) = create_instance(&entry, &desc.application_name)?;
        let physical_device = match pick_physical_device_without_surface(&instance, desc) {
            Ok(device) => device,
            Err(error) => {
                // SAFETY: the failed candidate owns no logical device or surface.
                unsafe { instance.destroy_instance(None) };
                return Err(error);
            }
        };
        let surface_loader = khr::surface::Instance::new(&entry, &instance);
        // SAFETY: The selected physical device belongs to this live instance.
        let adapter_properties =
            unsafe { instance.get_physical_device_properties(physical_device) };
        let adapter_name = physical_device_name(&adapter_properties);
        log::info!("nova-gfx Vulkan API instance={} adapter={} driver_api={} effective_api={}",
            api_version_string(instance_api), adapter_name,
            api_version_string(adapter_properties.api_version),
            api_version_string(instance_api.min(adapter_properties.api_version)));
        let native_limits = adapter_properties.limits;
        let binding_limits = gfx_core::BufferBindingLimits {
            storage_offset_alignment: native_limits.min_storage_buffer_offset_alignment.max(1),
            max_storage_binding_bytes: u64::from(native_limits.max_storage_buffer_range),
            uniform_offset_alignment: native_limits.min_uniform_buffer_offset_alignment.max(1),
            max_uniform_binding_bytes: u64::from(native_limits.max_uniform_buffer_range),
        };
        // Driver-reported optimal alignments; never a fixed constant.
        let copy_row_pitch_alignment = native_limits.optimal_buffer_copy_row_pitch_alignment.max(1);
        let copy_offset_alignment = native_limits.optimal_buffer_copy_offset_alignment.max(1);
        let texture_array_limits = gfx_core::TextureArrayCapabilities {
            max_dimension_2d: native_limits.max_image_dimension2_d,
            max_array_layers: native_limits.max_image_array_layers,
            ..Default::default()
        };
        let queue_families = queue_family_indices_without_surface(&instance, physical_device)?;
        // SAFETY: The selected physical device belongs to this instance.
        let queue_properties =
            unsafe { instance.get_physical_device_queue_family_properties(physical_device) };
        let timestamp_clock = queue_properties
            .get(queue_families.graphics as usize)
            .filter(|properties| properties.timestamp_valid_bits > 0)
            .map(|properties| VulkanTimestampClock {
                period_ns: f64::from(adapter_properties.limits.timestamp_period),
                valid_bits: properties.timestamp_valid_bits,
            });
        // SAFETY: feature and queue information belong to the selected physical device.
        let native_features = unsafe { instance.get_physical_device_features(physical_device) };
        let compute_capabilities = gfx_core::ComputeCapabilities {
            max_workgroups: if queue_properties[queue_families.graphics as usize]
                .queue_flags
                .contains(vk::QueueFlags::COMPUTE)
            {
                adapter_properties.limits.max_compute_work_group_count
            } else {
                [0; 3]
            },
            max_invocations_per_workgroup: adapter_properties
                .limits
                .max_compute_work_group_invocations,
            max_workgroup_size: adapter_properties.limits.max_compute_work_group_size,
            max_workgroup_shared_memory_bytes: adapter_properties
                .limits
                .max_compute_shared_memory_size,
            indirect_draw: native_features.draw_indirect_first_instance == vk::TRUE,
        };
        let (
            device,
            graphics_queue,
            sidecar_queue,
            dedicated_compute_queue,
            present_queue,
            incremental_presentation,
            supports_sampler_anisotropy,
            memory_budget_supported,
            native_pixel_presentation,
            display_timing_supported,
            present_wait_supported,
            host_query_reset,
            draw_indirect_count,
        ) = match create_device(&instance, physical_device, queue_families, &entry) {
            Ok(result) => result,
            Err(error) => {
                // SAFETY: logical device creation failed; no child device exists.
                unsafe { instance.destroy_instance(None) };
                return Err(error);
            }
        };
        let dedicated_compute_timestamp_clock = if dedicated_compute_queue.is_some() {
            queue_families
                .compute
                .and_then(|family| queue_properties.get(family as usize))
                .filter(|properties| properties.timestamp_valid_bits > 0)
                .map(|properties| VulkanTimestampClock {
                    period_ns: f64::from(adapter_properties.limits.timestamp_period),
                    valid_bits: properties.timestamp_valid_bits,
                })
        } else {
            None
        };
        let swapchain_maintenance = native_pixel_presentation;
        let native_pixel_presentation = native_pixel_presentation
            .then(|| khr::get_surface_capabilities2::Instance::new(&entry, &instance));
        let max_sampler_anisotropy = if supports_sampler_anisotropy {
            adapter_properties.limits.max_sampler_anisotropy.max(1.0)
        } else {
            1.0
        };
        let max_color_attachment_count = adapter_properties
            .limits
            .max_color_attachments
            .gt(&0)
            .then_some(adapter_properties.limits.max_color_attachments);
        let max_sample_count =
            sample_count_of(adapter_properties.limits.framebuffer_color_sample_counts);
        // Stencil is a core Vulkan feature; support is determined by the
        // depth-stencil format chosen later.
        let stencil_supported = true;
        let device = Arc::new(device);
        let swapchain_loader = khr::swapchain::Device::new(&instance, &device);
        let display_timing = display_timing_supported
            .then(|| ash::google::display_timing::Device::new(&instance, &device));
        let present_wait = present_wait_supported
            .then(|| khr::present_wait::Device::new(&instance, &device));
        let allocator = MemoryAllocator::new_vulkan(VulkanMemoryAllocatorDesc {
            instance: instance.clone(),
            device: (*device).clone(),
            physical_device,
        })?;
        let tuning = desc.tuning;
        let upload_defaults = UploadRingAllocatorDesc::default();
        let upload_ring = UploadRingAllocator::new(UploadRingAllocatorDesc {
            page_size: if tuning.upload_page_bytes == 0 {
                upload_defaults.page_size
            } else {
                tuning.upload_page_bytes
            },
            alignment: upload_defaults.alignment,
            max_retained_idle_pages: if tuning.upload_retained_idle_pages == 0 {
                upload_defaults.max_retained_idle_pages
            } else {
                tuning.upload_retained_idle_pages as usize
            },
        })?;
        let command_encoder_pool_capacity = if tuning.command_encoder_pool_capacity == 0 {
            DEFAULT_COMMAND_ENCODER_POOL_CAPACITY
        } else {
            tuning.command_encoder_pool_capacity as usize
        };
        let upload_command_pool_capacity = if tuning.upload_command_pool_capacity == 0 {
            DEFAULT_UPLOAD_COMMAND_POOL_CAPACITY
        } else {
            tuning.upload_command_pool_capacity as usize
        };
        let pipeline_cache_limit_bytes = if tuning.pipeline_cache_limit_bytes == 0 {
            DEFAULT_PIPELINE_CACHE_BYTES
        } else {
            tuning.pipeline_cache_limit_bytes
        };
        let present_timeout_ns = tuning
            .present_timeout_micros
            .checked_mul(1_000)
            .filter(|timeout| *timeout > 0)
            .unwrap_or(DEFAULT_PRESENT_TIMEOUT_NS);
        let pipeline_cache_path =
            vulkan_pipeline_cache_path(desc.pipeline_cache_dir.as_deref(), &adapter_properties);
        let pipeline_cache_data = pipeline_cache_path
            .as_deref()
            .and_then(|path| read_pipeline_cache_data(path, pipeline_cache_limit_bytes));
        let pipeline_cache = create_pipeline_cache(
            &device,
            pipeline_cache_data.as_deref(),
            pipeline_cache_path.as_deref(),
        )?;

        // Timeline is a required enabled core feature, not an extension
        // negotiation. Keep the private fence wake only for allocation failure
        // and submissions not yet migrated to timeline signaling.
        let primary_timeline = match timeline::QueueTimeline::new(&device) {
            Ok(value) => Some(value),
            Err(error) => { log::warn!("Vulkan timeline allocation failed: {error}"); None }
        };
        let secondary_timeline = if sidecar_queue.is_some() || dedicated_compute_queue.is_some() {
            match timeline::QueueTimeline::new(&device) {
                Ok(value) => Some(value),
                Err(error) => { log::warn!("Vulkan secondary timeline allocation failed: {error}"); None }
            }
        } else { None };
        Ok(Self {
            swapchain_maintenance,
            native_pixel_presentation,
            memory_budget_supported,
            host_query_reset,
            draw_indirect_count,
            entry,
            instance,
            surface_loader,
            physical_device,
            adapter_name,
            max_sampler_anisotropy,
            max_color_attachment_count,
            max_sample_count,
            stencil_supported,
            multi_draw_indirect: native_features.multi_draw_indirect == vk::TRUE,
            max_draw_indirect_count: native_limits.max_draw_indirect_count.max(1),
            binding_limits,
            texture_array_limits,
            copy_row_pitch_alignment,
            copy_offset_alignment,
            device,
            graphics_queue,
            sidecar_queue,
            dedicated_compute_queue,
            // Prefer the second queue in the same graphics family. The
            // compute-only family is requested only if the graphics family
            // has no spare queue, so do not advertise an unused queue.
            dedicated_compute_family_index: queue_families
                .compute
                .filter(|_| sidecar_queue.is_none()),
            present_queue,
            graphics_queue_family_index: queue_families.graphics,
            present_queue_family_index: queue_families.present,
            swapchain_loader,
            display_timing,
            present_wait,
            allocator: ManuallyDrop::new(allocator),
            buffers: ResourceRegistry::new("buffer"),
            textures: ResourceRegistry::new("texture"),
            texture_views: ResourceRegistry::new("texture view"),
            samplers: ResourceRegistry::new("sampler"),
            bind_group_layouts: ResourceRegistry::new("bind group layout"),
            bind_groups: ResourceRegistry::new("bind group"),
            pipeline_layouts: ResourceRegistry::new("pipeline layout"),
            shader_modules: ResourceRegistry::new("shader module"),
            compute: compute::State::new(compute_capabilities),
            render_passes: ResourceRegistry::new("render pass"),
            render_pipelines: ResourceRegistry::new("render pipeline"),
            command_encoders: ResourceRegistry::new("command encoder"),
            free_command_encoders: Vec::with_capacity(command_encoder_pool_capacity),
            submissions: ResourceRegistry::new("submission"),
            submission_wakes: submission_wake::SubmissionWakes::default(),
            primary_timeline,
            secondary_timeline,
            native_queue_waits: Vec::new(),
            surfaces: ResourceRegistry::new("surface"),
            swapchains: ResourceRegistry::new("swapchain"),
            descriptor_pools: descriptors::Pools::default(),
            pipeline_cache,
            pipeline_cache_path,
            upload_ring,
            upload_pages: Vec::new(),
            upload_command_pool: Vec::with_capacity(upload_command_pool_capacity),
            timestamp_clock,
            dedicated_compute_timestamp_clock,
            gpu_trace_enabled: timestamp_clock.is_some()
                && log::log_enabled!(target: "gfx_vulkan::gpu", log::Level::Trace),
            gpu_trace_totals: VulkanGpuTraceTotals::default(),
            last_frame_gpu_timestamps: None,
            completed_frame_probes: std::collections::VecDeque::new(),
            gpu_work_samples: Vec::new(),
            last_texture_transfer_time: None,
            deferred_destroys: DeferredFreeQueue::new(),
            next_upload_fence_value: 1,
            submitted_frames: 0,
            incremental_presentation,
            command_encoder_pool_capacity,
            upload_command_pool_capacity,
            pipeline_cache_limit_bytes,
            present_timeout_ns,
        })
    }

    /// Returns the physical adapter selected when this logical device was created.
    #[must_use]
    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    /// Returns the extent of the allocated WSI images, after native surface capability resolution.
    /// This may differ from the requested client size; attachments must cover this extent.
    /// No GPU work or resize is performed.
    ///
    /// # Errors
    /// Returns an invalid-handle error or a surface error for a suspended zero-sized swapchain.
    pub fn swapchain_size(&self, swapchain: gfx_core::SwapchainId) -> Result<gfx_core::Extent2d> {
        let extent = self.swapchains.get(swapchain)?.extent;
        gfx_core::Extent2d::new(extent.width, extent.height)
    }

    /// Whether this swapchain can retain the given image extent and present it at native
    /// pixel scale, anchored to the client's top-left. Unsupported maintenance1 features,
    /// gravity or image extent limits return `false`; no resize or GPU work is performed.
    ///
    /// # Errors
    /// Returns an invalid-handle error for an unknown or retired swapchain.
    pub fn supports_native_pixel_presentation(
        &self,
        swapchain: gfx_core::SwapchainId,
        extent: gfx_core::Extent2d,
    ) -> Result<bool> {
        let swapchain = self.swapchains.get(swapchain)?;
        Ok(swapchain
            .scaled_image_extent_range
            .is_some_and(|(minimum, maximum)| {
                extent.width() >= minimum.width
                    && extent.height() >= minimum.height
                    && extent.width() <= maximum.width
                    && extent.height() <= maximum.height
            }))
    }

    /// Creates a native Vulkan surface from raw-window-handle traits.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when surface creation or present queue selection fails.
    fn create_surface<W>(&mut self, window: &W, _desc: &SurfaceDescriptor) -> Result<SurfaceId>
    where
        W: HasDisplayHandle + HasWindowHandle + ?Sized,
    {
        let display_handle = window
            .display_handle()
            .map_err(|error| Error::Backend(error.to_string()))?
            .as_raw();
        let window_handle = window
            .window_handle()
            .map_err(|error| Error::Backend(error.to_string()))?
            .as_raw();
        // SAFETY: The raw display and window handles come from a live native window
        // borrowed for this call. The created VkSurfaceKHR is owned by this device and
        // destroyed before instance shutdown.
        let surface = unsafe {
            ash_window::create_surface(
                &self.entry,
                &self.instance,
                display_handle,
                window_handle,
                None,
            )
        }
        .map_err(|error| Error::Backend(error.to_string()))?;
        // SAFETY: Surface, physical device, and queue family index are valid for this instance.
        let graphics_queue_supports_present = unsafe {
            self.surface_loader.get_physical_device_surface_support(
                self.physical_device,
                self.graphics_queue_family_index,
                surface,
            )
        }
        .map_err(VulkanError::from)?;
        if !graphics_queue_supports_present {
            return Err(VulkanError::Unavailable(
                "selected graphics queue family cannot present to this surface".to_string(),
            )
            .into());
        }
        self.present_queue_family_index = self.graphics_queue_family_index;
        self.present_queue = self.graphics_queue;

        Ok(self.surfaces.insert(VulkanSurface { surface }))
    }

    /// Configures a surface swapchain.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when swapchain resources cannot be created.
    pub fn configure_surface(
        &mut self,
        surface: SurfaceId,
        config: SurfaceConfig,
    ) -> Result<SurfaceId> {
        let _ = self.create_swapchain(surface, config)?;
        Ok(surface)
    }

    /// Resolves a requested surface alpha mode against Vulkan surface capabilities.
    ///
    /// A premultiplied request represents a transparent surface and may resolve to
    /// postmultiplied alpha when that is the compositor's only transparent mode.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the surface handle is invalid, capabilities cannot be
    /// queried, or no compatible alpha mode is supported.
    pub fn resolve_surface_alpha_mode(
        &self,
        surface_id: SurfaceId,
        requested: CompositeAlphaMode,
    ) -> Result<CompositeAlphaMode> {
        let surface = self.surfaces.get(surface_id)?.surface;
        // SAFETY: The physical device and surface were created from this Vulkan instance.
        let capabilities = unsafe {
            self.surface_loader
                .get_physical_device_surface_capabilities(self.physical_device, surface)
        }
        .map_err(VulkanError::from)?;
        choose_composite_alpha_mode(capabilities.supported_composite_alpha, requested)
    }

    /// Creates or replaces a swapchain for a surface.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when swapchain resources cannot be created.
    fn create_swapchain(
        &mut self,
        surface_id: SurfaceId,
        config: SurfaceConfig,
    ) -> Result<gfx_core::SwapchainId> {
        let surface = self.surfaces.get(surface_id)?.surface;
        let swapchain = self.build_swapchain(surface, config)?;

        Ok(self.swapchains.insert(swapchain))
    }

    /// Reports whether any tracked frame submission or in-flight swapchain frame
    /// is still executing on the GPU.
    ///
    /// Mirrors the DX12 query so callers gate interactive swapchain resizes on the
    /// same backend-independent semantic instead of assuming Vulkan is always idle.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when a swapchain handle is invalid.
    pub fn poll_gpu_work(&self) -> Result<bool> {
        let mut pending = false;
        let mut failure = None;
        self.submissions.for_each_live(|submission| {
            if failure.is_none() {
                match self.submission_is_complete(submission) {
                    Ok(complete) => pending |= !complete,
                    Err(error) => failure = Some(error),
                }
            }
        });
        if let Some(error) = failure {
            return Err(error);
        }
        if !pending {
            self.swapchains.for_each_live(|swapchain| {
                for &fence in &swapchain.in_flight_fences {
                    pending |=
                        fence != vk::Fence::null() && !fence_is_complete(&self.device, fence);
                }
            });
        }
        Ok(pending)
    }

    /// Recreates an existing swapchain.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the existing swapchain is invalid or recreation fails.
    pub fn resize_swapchain(
        &mut self,
        swapchain_id: gfx_core::SwapchainId,
        width: u32,
        height: u32,
    ) -> Result<()> {
        if width == 0 || height == 0 {
            return Ok(());
        }
        let previous = self.swapchains.get(swapchain_id)?;
        let support =
            query_swapchain_support(self.physical_device, &self.surface_loader, previous.surface)?;
        let desired = if previous.native_pixel_scaling {
            vk::Extent2D { width, height }
        } else {
            choose_extent(&support.capabilities, width, height)
        };
        if desired == previous.extent {
            self.swapchains.get_mut(swapchain_id)?.config.size =
                gfx_core::Extent2d::new(width, height)?;
            return Ok(());
        }
        self.retire_swapchain(swapchain_id)?;
        let old_swapchain = self.swapchains.get(swapchain_id)?;
        let mut config = old_swapchain.config;
        config.size = gfx_core::Extent2d::new(width, height)?;
        let surface = old_swapchain.surface;
        let old_native_swapchain = old_swapchain.swapchain;
        let new_swapchain =
            self.build_swapchain_replacing(surface, config, old_native_swapchain)?;
        let old_swapchain = self.swapchains.replace_live(swapchain_id, new_swapchain)?;
        self.destroy_swapchain_now(old_swapchain);
        Ok(())
    }

    /// Recreates an existing swapchain with a full surface configuration.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the existing swapchain is invalid or recreation fails.
    pub fn reconfigure_swapchain(
        &mut self,
        swapchain_id: gfx_core::SwapchainId,
        config: SurfaceConfig,
    ) -> Result<()> {
        self.retire_swapchain(swapchain_id)?;
        // Retire queued encoders while their old swapchain fences are still live.
        self.poll_cleanup();
        let old_swapchain = self.swapchains.get(swapchain_id)?;
        let surface = old_swapchain.surface;
        let old_native_swapchain = old_swapchain.swapchain;
        let new_swapchain =
            self.build_swapchain_replacing(surface, config, old_native_swapchain)?;
        let old_swapchain = self.swapchains.replace_live(swapchain_id, new_swapchain)?;
        self.destroy_swapchain_now(old_swapchain);
        Ok(())
    }

    /// Creates a buffer.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when validation or Vulkan allocation fails.
    fn create_buffer(&mut self, desc: &BufferDescriptor) -> Result<BufferId> {
        desc.validate()?;
        // A compute-only family may access generic storage/uniform buffers
        // concurrently with the graphics family. Such resources MUST be
        // CONCURRENT; EXCLUSIVE resources would need matched native
        // release/acquire Queue Family Ownership Transfer barriers.
        //
        // Restrict sharing to Compute-compatible buffers. Render targets,
        // images and unrelated upload/transfer allocations remain EXCLUSIVE.
        // This avoids per-Section cross-family ownership ping-pong and its
        // extra command buffers when the camera streams new chunks.
        // Avoid CONCURRENT's implementation-defined cache/ownership cost
        // when a cheaper second graphics-family queue is already usable.
        // The dedicated compute family is selected only when no such queue
        // exists, and only native buffer compute may execute there.
        let share_compute = concurrent_compute_sharing_required(
            self.sidecar_queue.is_some(),
            self.dedicated_compute_family_index,
            desc.usage,
        );
        let families = [
            self.graphics_queue_family_index,
            self.dedicated_compute_family_index
                .unwrap_or(self.graphics_queue_family_index),
        ];
        let mut create_info = vk::BufferCreateInfo::default().size(desc.size).usage(
            buffer_usage_to_vk(desc.usage)
                | vk::BufferUsageFlags::TRANSFER_SRC
                | vk::BufferUsageFlags::TRANSFER_DST,
        );
        if share_compute {
            create_info = create_info
                .sharing_mode(vk::SharingMode::CONCURRENT)
                .queue_family_indices(&families);
        } else {
            create_info = create_info.sharing_mode(vk::SharingMode::EXCLUSIVE);
        }
        // SAFETY: Device is valid and create info is self-contained.
        let buffer =
            unsafe { self.device.create_buffer(&create_info, None) }.map_err(VulkanError::from)?;
        // SAFETY: Buffer was created from this device and is valid here.
        let requirements = unsafe { self.device.get_buffer_memory_requirements(buffer) };
        let name = desc.label.as_deref().unwrap_or("nova-gfx-buffer");
        let allocation = self.allocator.allocate_vulkan_buffer(
            name,
            requirements,
            desc.memory_location,
            buffer,
        )?;
        let (memory, offset) = vulkan_memory(&allocation)?;
        // SAFETY: Allocation was created from this buffer's memory requirements and outlives it.
        unsafe { self.device.bind_buffer_memory(buffer, memory, offset) }
            .map_err(VulkanError::from)?;

        Ok(self.buffers.insert(VulkanBuffer {
            buffer,
            allocation,
            desc: desc.clone(),
        }))
    }

    /// Writes data into a buffer.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the handle is invalid or upload fails.
    fn write_buffer(&mut self, buffer_id: BufferId, offset: u64, data: &[u8]) -> Result<()> {
        let memory_location = self.buffers.get(buffer_id)?.desc.memory_location;
        if memory_location == MemoryLocation::CpuToGpu {
            let buffer = self.buffers.get_mut(buffer_id)?;
            let mapped = buffer
                .allocation
                .mapped_slice_mut()
                .ok_or_else(|| Error::Backend("buffer memory is not CPU visible".to_string()))?;
            let offset = usize::try_from(offset)
                .map_err(|error| Error::InvalidInput(format!("offset overflow: {error}")))?;
            let end = offset
                .checked_add(data.len())
                .ok_or_else(|| Error::InvalidInput("buffer write range overflow".to_string()))?;
            let target = mapped.get_mut(offset..end).ok_or_else(|| {
                Error::InvalidInput("buffer write range is out of bounds".to_string())
            })?;
            target.copy_from_slice(data);
            return Ok(());
        }

        self.upload_buffer_batch([BufferWrite {
            descriptor: gfx_core::BufferWriteDescriptor {
                buffer: buffer_id,
                offset,
            },
            data,
        }])
        .map(|_| ())
    }

    /// Creates a 2D texture.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when validation or Vulkan allocation fails.
    fn create_texture(&mut self, desc: &TextureDescriptor) -> Result<TextureId> {
        desc.validate()?;
        if desc.sample_count > self.max_sample_count {
            return Err(Error::Unavailable(format!(
                "Vulkan texture requests {} samples, device supports at most {}",
                desc.sample_count, self.max_sample_count
            )));
        }
        let layers = desc.dimension.layers();
        if matches!(desc.dimension, TextureDimension::D3 { .. }) {
            self.texture_volume_capabilities().validate(desc)?;
            // Vulkan storage-image support varies per format, even when 3D
            // image dimensions are supported by the device.
            let features = unsafe {
                self.instance.get_physical_device_format_properties(
                    self.physical_device,
                    format_to_vk(desc.format),
                )
            }
            .optimal_tiling_features;
            if desc.usage.contains(TextureUsage::STORAGE)
                && !features.contains(vk::FormatFeatureFlags::STORAGE_IMAGE)
            {
                return Err(Error::Unavailable(
                    "Vulkan 3D storage image format unsupported".into(),
                ));
            }
            if desc.usage.contains(TextureUsage::SAMPLED)
                && !features.contains(vk::FormatFeatureFlags::SAMPLED_IMAGE)
            {
                return Err(Error::Unavailable(
                    "Vulkan 3D sampled image format unsupported".into(),
                ));
            }
        }
        if let TextureDimension::D2Array { .. } = desc.dimension {
            self.texture_array_capabilities().plan(
                desc.size.width(),
                desc.size.height(),
                layers,
                desc.mip_level_count,
                desc.format,
            )?;
        }
        let transient = desc.usage.contains(TextureUsage::TRANSIENT_ATTACHMENT);
        let create_info = vk::ImageCreateInfo::default()
            .image_type(if matches!(desc.dimension, TextureDimension::D3 { .. }) {
                vk::ImageType::TYPE_3D
            } else {
                vk::ImageType::TYPE_2D
            })
            .format(format_to_vk(desc.format))
            .extent(vk::Extent3D {
                width: desc.size.width(),
                height: desc.size.height(),
                depth: desc.dimension.mip_depth(0),
            })
            .mip_levels(desc.mip_level_count)
            .array_layers(layers)
            .samples(sample_flag_for(desc.sample_count).unwrap_or(vk::SampleCountFlags::TYPE_1))
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(if transient {
                vk::ImageUsageFlags::TRANSIENT_ATTACHMENT | vk::ImageUsageFlags::COLOR_ATTACHMENT
            } else {
                texture_usage_to_vk(desc.usage)
                    | vk::ImageUsageFlags::TRANSFER_SRC
                    | vk::ImageUsageFlags::TRANSFER_DST
            })
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: Device is valid and image create info is self-contained.
        let image =
            unsafe { self.device.create_image(&create_info, None) }.map_err(VulkanError::from)?;
        // SAFETY: Image was created from this device and is valid here.
        let requirements = unsafe { self.device.get_image_memory_requirements(image) };
        let name = desc.label.as_deref().unwrap_or("nova-gfx-texture");
        let allocate = |allocator: &mut gfx_memory::MemoryAllocator, requirements| {
            allocator.allocate_vulkan_image(name, requirements, desc.memory_location, image)
        };
        let allocation = if transient {
            // Lazy heaps are optional (typically tile-based GPUs). Mask the
            // actual VkImage requirements rather than guessing a heap index.
            let properties = unsafe { self.instance.get_physical_device_memory_properties(self.physical_device) };
            let mut lazy_bits = 0u32;
            for (index, memory_type) in properties.memory_types[..properties.memory_type_count as usize].iter().enumerate() {
                if memory_type.property_flags.contains(vk::MemoryPropertyFlags::LAZILY_ALLOCATED | vk::MemoryPropertyFlags::DEVICE_LOCAL) {
                    lazy_bits |= 1u32 << index;
                }
            }
            let lazy = requirements.memory_type_bits & lazy_bits;
            let ordinary = requirements.memory_type_bits & !lazy_bits;
            let mut preferred = requirements;
            preferred.memory_type_bits = lazy;
            let mut fallback = requirements;
            fallback.memory_type_bits = ordinary;
            if lazy != 0 {
                allocate(&mut self.allocator, preferred).or_else(|error| {
                    if ordinary != 0 { allocate(&mut self.allocator, fallback) } else { Err(error) }
                })
            } else {
                allocate(&mut self.allocator, requirements)
            }
        } else {
            allocate(&mut self.allocator, requirements)
        };
        let allocation = match allocation {
            Ok(allocation) => allocation,
            Err(error) => {
                unsafe { self.device.destroy_image(image, None); }
                return Err(error);
            }
        };
        let binding = vulkan_memory(&allocation).and_then(|(memory, offset)| {
            // SAFETY: image and allocation are live and compatible.
            unsafe { self.device.bind_image_memory(image, memory, offset) }
                .map_err(|error| VulkanError::from(error).into())
        });
        if let Err(error) = binding {
            unsafe { self.device.destroy_image(image, None); }
            let _ = self.allocator.free(allocation);
            return Err(error);
        }

        Ok(self.textures.insert(VulkanTexture {
            image,
            allocation,
            desc: desc.clone(),
            // VkImageCreateInfo starts in UNDEFINED. A storage image must be
            // transitioned to GENERAL by a GPU image barrier before its first
            // shader write; reporting GENERAL here without a barrier was invalid.
            layout: vk::ImageLayout::UNDEFINED,
        }))
    }

    /// Writes data into a texture using a staging buffer.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when upload fails.
    fn write_texture(&mut self, desc: TextureWriteDescriptor, data: &[u8]) -> Result<()> {
        let (image, old_layout, mip_level_count) = {
            let texture = self.textures.get(desc.texture)?;
            if texture.desc.dimension != TextureDimension::D2 {
                return Err(Error::InvalidInput(
                    "use write_texture_array_layer for D2Array".into(),
                ));
            }
            desc.validate_against(&texture.desc, data.len())?;
            (texture.image, texture.layout, texture.desc.mip_level_count)
        };
        validate_texture_staging_layout(desc, self.textures.get(desc.texture)?.desc.format)?;
        let upload = self.write_upload_data(data)?;
        let staging_buffer = self.upload_page_buffer(upload.page_index)?;
        self.copy_buffer_to_texture(
            desc.texture,
            staging_buffer,
            image,
            old_layout,
            desc.mip_level,
            mip_level_count,
            texture_staging_layout(upload, desc)?,
            desc.origin,
            desc.size,
        )?;
        self.textures.get_mut(desc.texture)?.layout = vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL;
        let _ = self.textures.get(desc.texture)?.desc.format;
        Ok(())
    }

    fn write_texture_batch<'a>(
        &mut self,
        writes: impl IntoIterator<Item = TextureWrite<'a>>,
    ) -> Result<()> {
        let writes = writes.into_iter();
        let mut plans = Vec::with_capacity(writes.size_hint().0);
        for write in writes {
            let descriptor = write.descriptor;
            let (image, old_layout, mip_level_count) = {
                let texture = self.textures.get(descriptor.texture)?;
                if texture.desc.dimension != TextureDimension::D2 {
                    return Err(Error::InvalidInput(
                        "batch 2D upload cannot write a D2Array".into(),
                    ));
                }
                descriptor.validate_against(&texture.desc, write.data.len())?;
                (texture.image, texture.layout, texture.desc.mip_level_count)
            };
            validate_texture_staging_layout(
                descriptor,
                self.textures.get(descriptor.texture)?.desc.format,
            )?;
            plans.push(VulkanTextureWritePlan {
                write,
                image,
                old_layout,
                mip_level_count,
            });
        }
        if plans.is_empty() {
            return Ok(());
        }
        let upload_sizes = plans
            .iter()
            .map(|plan| {
                u64::try_from(plan.write.data.len())
                    .map_err(|error| Error::InvalidInput(format!("upload size overflow: {error}")))
            })
            .collect::<Result<Vec<_>>>()?;
        let allocations = self.upload_ring.allocate_batch(&upload_sizes)?;
        let mut uploads = Vec::with_capacity(plans.len());
        for (plan, allocation) in plans.into_iter().zip(allocations) {
            let descriptor = plan.write.descriptor;
            let upload = self.stage_upload_data(plan.write.data, allocation)?;
            let staging_buffer = self.upload_page_buffer(upload.page_index)?;
            let layout = texture_staging_layout(upload, descriptor)?;
            uploads.push(VulkanTextureUpload {
                texture: descriptor.texture,
                source: staging_buffer,
                image: plan.image,
                old_layout: plan.old_layout,
                mip_level_count: plan.mip_level_count,
                layout,
                mip_level: descriptor.mip_level,
                layer: 0,
                origin: descriptor.origin,
                size: descriptor.size,
            });
        }
        self.copy_buffers_to_textures(&uploads)?;
        for upload in &uploads {
            self.textures.get_mut(upload.texture)?.layout =
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL;
        }
        Ok(())
    }

    fn read_texture_pixels(&mut self, texture_id: TextureId) -> Result<TextureReadback> {
        self.wait_for_pending_work()?;
        let (image, desc, layout) = {
            let texture = self.textures.get(texture_id)?;
            if !texture.desc.usage.contains(TextureUsage::COPY_SRC) {
                return Err(Error::InvalidInput(
                    "texture readback requires COPY_SRC usage".to_string(),
                ));
            }
            (texture.image, texture.desc.clone(), texture.layout)
        };
        let bytes_per_row = desc
            .size
            .width()
            .checked_mul(desc.format.bytes_per_pixel())
            .ok_or_else(|| Error::InvalidInput("texture row size overflow".to_string()))?;
        let byte_len = u64::from(bytes_per_row)
            .checked_mul(u64::from(desc.size.height()))
            .ok_or_else(|| Error::InvalidInput("texture readback size overflow".to_string()))?;
        let readback_desc = BufferDescriptor {
            label: Some("nova-gfx Vulkan texture readback".to_string()),
            size: byte_len,
            usage: BufferUsage::COPY_DST,
            memory_location: MemoryLocation::GpuToCpu,
        };
        let readback = self.create_buffer_unregistered(&readback_desc)?;
        if let Err(error) =
            self.copy_texture_to_buffer_once(image, layout, desc.format, desc.size, readback.buffer)
        {
            self.destroy_buffer_now(readback)?;
            return Err(error);
        }
        let byte_len = usize::try_from(byte_len).map_err(|error| {
            Error::InvalidInput(format!("texture readback length overflow: {error}"))
        })?;
        let bytes = readback
            .allocation
            .mapped_slice()
            .and_then(|mapped| mapped.get(..byte_len))
            .map(<[u8]>::to_vec)
            .ok_or_else(|| Error::Backend("texture readback memory is not mapped".to_string()));
        let release = self.destroy_buffer_now(readback);
        let bytes = bytes?;
        release?;
        Ok(TextureReadback {
            format: desc.format,
            size: desc.size,
            bytes_per_row,
            bytes,
        })
    }

    fn copy_texture_to_buffer_once(
        &self,
        image: vk::Image,
        old_layout: vk::ImageLayout,
        format: Format,
        size: gfx_core::Extent2d,
        destination: vk::Buffer,
    ) -> Result<()> {
        let command_pool = create_command_pool(&self.device, self.graphics_queue_family_index)?;
        let command_buffer = allocate_command_buffers(&self.device, command_pool, 1)?
            .into_iter()
            .next()
            .ok_or_else(|| Error::Backend("failed to allocate readback command buffer".into()))?;
        begin_one_time_commands(&self.device, command_buffer)?;
        transition_image_layout_levels(
            &self.device,
            command_buffer,
            image,
            old_layout,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            1,
            1,
            image_aspect_from_format(format),
        );
        let region = vk::BufferImageCopy::default()
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(image_aspect_from_format(format))
                    .mip_level(0)
                    .base_array_layer(0)
                    .layer_count(1),
            )
            .image_extent(vk::Extent3D {
                width: size.width(),
                height: size.height(),
                depth: 1,
            });
        // SAFETY: The image is in transfer-source layout and the destination buffer is large
        // enough for a tightly packed copy of the complete subresource.
        unsafe {
            self.device.cmd_copy_image_to_buffer(
                command_buffer,
                image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                destination,
                &[region],
            );
        }
        transition_image_layout_levels(
            &self.device,
            command_buffer,
            image,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            old_layout,
            1,
            1,
            image_aspect_from_format(format),
        );
        end_submit_wait_destroy(
            &self.device,
            self.graphics_queue,
            command_pool,
            command_buffer,
            self.primary_timeline.as_ref(),
        )
    }

    /// Creates a texture view.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the texture is invalid or view creation fails.
    fn create_texture_view(&mut self, desc: &TextureViewDescriptor) -> Result<TextureViewId> {
        let texture = self.textures.get(desc.texture)?;
        desc.validate_against(&texture.desc)?;
        // Sampled mip-chain views and attachment views share image storage.
        // The sampled view may span every level, while a render attachment
        // selects exactly one mip in the ordered render-pass submission.
        // Do not restrict the sampled range based on the texture's usage.
        let view = if matches!(texture.desc.dimension, TextureDimension::D3 { .. }) {
            let range = vk::ImageSubresourceRange::default()
                .aspect_mask(image_aspect_from_format(desc.format))
                .base_mip_level(desc.base_mip_level)
                .level_count(desc.mip_level_count)
                .base_array_layer(0)
                .layer_count(1);
            let info = vk::ImageViewCreateInfo::default()
                .image(texture.image)
                .view_type(vk::ImageViewType::TYPE_3D)
                .format(format_to_vk(desc.format))
                .subresource_range(range);
            unsafe { self.device.create_image_view(&info, None) }.map_err(VulkanError::from)?
        } else if let TextureDimension::D2Array { layers } = texture.desc.dimension {
            create_image_view_array(
                &self.device,
                texture.image,
                format_to_vk(desc.format),
                image_aspect_from_format(desc.format),
                desc.base_mip_level,
                desc.mip_level_count,
                layers,
            )?
        } else {
            create_image_view_2d(
                &self.device,
                texture.image,
                format_to_vk(desc.format),
                image_aspect_from_format(desc.format),
                desc.base_mip_level,
                desc.mip_level_count,
            )?
        };
        Ok(self.texture_views.insert(VulkanTextureView {
            view,
            texture: desc.texture,
            desc: desc.clone(),
        }))
    }

    /// Creates a sampler.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when Vulkan sampler creation fails.
    fn create_sampler(&mut self, desc: &SamplerDescriptor) -> Result<SamplerId> {
        desc.validate()?;
        let create_info = sampler_create_info(desc, self.max_sampler_anisotropy);
        // SAFETY: Device is valid and sampler create info is self-contained.
        let sampler =
            unsafe { self.device.create_sampler(&create_info, None) }.map_err(VulkanError::from)?;
        Ok(self.samplers.insert(VulkanSampler { sampler }))
    }

    /// Creates a bind group layout.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when validation or Vulkan descriptor set layout creation fails.
    fn create_bind_group_layout(
        &mut self,
        desc: &BindGroupLayoutDescriptor,
    ) -> Result<BindGroupLayoutId> {
        desc.validate()?;

        let bindings = desc
            .entries
            .iter()
            .map(|entry| {
                vk::DescriptorSetLayoutBinding::default()
                    .binding(entry.binding)
                    .descriptor_type(resource_binding_type_to_vk(entry.binding_type))
                    .descriptor_count(1)
                    .stage_flags(shader_stages_to_vk(entry.stages))
            })
            .collect::<Vec<_>>();
        let create_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        // SAFETY: Descriptor set layout create info references local binding metadata only.
        let layout = unsafe { self.device.create_descriptor_set_layout(&create_info, None) }
            .map_err(VulkanError::from)?;
        Ok(self.bind_group_layouts.insert(VulkanResourceSetLayout {
            layout,
            desc: desc.clone(),
        }))
    }

    /// Creates a pipeline layout from bind group layouts.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when any layout handle is invalid or Vulkan creation fails.
    fn create_pipeline_layout(
        &mut self,
        desc: &PipelineLayoutDescriptor,
    ) -> Result<PipelineLayoutId> {
        desc.validate()?;
        let layouts = desc
            .bind_group_layouts
            .iter()
            .copied()
            .map(|layout| Ok(self.bind_group_layouts.get(layout)?.layout))
            .collect::<Result<Vec<_>>>()?;
        let create_info = vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts);
        // SAFETY: Descriptor set layouts belong to this device and remain live via registry handles.
        let layout = unsafe { self.device.create_pipeline_layout(&create_info, None) }
            .map_err(VulkanError::from)?;
        Ok(self.pipeline_layouts.insert(VulkanPipelineLayout {
            layout,
            bind_group_layouts: desc.bind_group_layouts.clone(),
        }))
    }

    /// Creates a bind group and writes descriptor bindings.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when validation, handles, or descriptor writes fail.
    #[expect(
        clippy::too_many_lines,
        reason = "bind group creation keeps descriptor writes together for backend validation"
    )]
    fn create_bind_group(&mut self, desc: &BindGroupDescriptor) -> Result<BindGroupId> {
        // Reclaim completed descriptor retirements before consuming the shared pool budget.
        self.poll_cleanup();
        let layout_desc = self.bind_group_layouts.get(desc.layout)?.desc.clone();
        desc.validate_against(&layout_desc)?;
        let layout = self.bind_group_layouts.get(desc.layout)?.layout;
        let mut buffer_infos = Vec::new();
        let mut image_infos = Vec::new();
        let mut sampled_depth = Vec::new();
        let mut pending_writes = Vec::new();
        for binding in &desc.entries {
            match binding.resource {
                BindingResource::Buffer(buffer_binding) => {
                    let buffer = self.buffers.get(buffer_binding.buffer)?;
                    buffer_binding.validate_against(buffer.desc.size)?;
                    let kind = layout_desc
                        .entries
                        .iter()
                        .find(|entry| entry.binding == binding.binding)
                        .map(|entry| entry.binding_type)
                        .ok_or_else(|| {
                            Error::InvalidInput(format!(
                                "bind group layout is missing binding {}",
                                binding.binding
                            ))
                        })?;
                    let uniform = kind == BindingType::UniformBuffer;
                    self.buffer_binding_limits().validate_range(
                        buffer_binding.offset,
                        buffer_binding.size,
                        uniform,
                    )?;
                    if !buffer.desc.usage.contains(if uniform {
                        BufferUsage::UNIFORM
                    } else {
                        BufferUsage::STORAGE
                    }) || (kind == BindingType::StorageBufferReadWrite
                        && !buffer.desc.usage.contains(BufferUsage::STORAGE_WRITE))
                    {
                        return Err(Error::InvalidInput(
                            "Vulkan compute buffer binding lacks required usage".into(),
                        ));
                    }
                    let binding_type = resource_binding_type_to_vk(kind);
                    buffer_infos.push(
                        vk::DescriptorBufferInfo::default()
                            .buffer(buffer.buffer)
                            .offset(buffer_binding.offset)
                            .range(buffer_binding.size),
                    );
                    pending_writes.push(PendingDescriptorWrite::Buffer {
                        binding: binding.binding,
                        descriptor_type: binding_type,
                        info_index: buffer_infos.len() - 1,
                    });
                }
                BindingResource::Texture(texture_binding) => {
                    let kind = layout_desc
                        .entries
                        .iter()
                        .find(|entry| entry.binding == binding.binding)
                        .map(|entry| entry.binding_type)
                        .ok_or_else(|| {
                            Error::InvalidInput("missing image layout binding".into())
                        })?;
                    let view = self.texture_views.get(texture_binding.texture_view)?;
                    let texture = self.textures.get(view.texture)?;
                    let (descriptor_type, image_layout) = match kind {
                        BindingType::StorageTexture { format, .. } => {
                            if !texture.desc.usage.contains(TextureUsage::STORAGE)
                                || view.desc.format != format
                                || !matches!(
                                    texture.desc.dimension,
                                    TextureDimension::D2 | TextureDimension::D3 { .. }
                                )
                                || texture.desc.sample_count != 1
                                || view.desc.mip_level_count != 1
                                || format == Format::Depth32Float
                            {
                                return Err(Error::InvalidInput(
                                    "Vulkan storage image requires matching single-sample color D2 view".into(),
                                ));
                            }
                            (vk::DescriptorType::STORAGE_IMAGE, vk::ImageLayout::GENERAL)
                        }
                        BindingType::SampledTexture => {
                            if !texture.desc.usage.contains(TextureUsage::SAMPLED) {
                                return Err(Error::InvalidInput(
                                    "Vulkan sampled image lacks SAMPLED usage".into(),
                                ));
                            }
                            if texture.desc.format == Format::Depth32Float {
                                sampled_depth.push(view.texture);
                            }
                            (
                                vk::DescriptorType::SAMPLED_IMAGE,
                                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                            )
                        }
                        _ => {
                            return Err(Error::InvalidInput(
                                "invalid Vulkan image binding kind".into(),
                            ));
                        }
                    };
                    image_infos.push(
                        vk::DescriptorImageInfo::default()
                            .image_view(view.view)
                            .image_layout(image_layout),
                    );
                    pending_writes.push(PendingDescriptorWrite::Image {
                        binding: binding.binding,
                        descriptor_type,
                        info_index: image_infos.len() - 1,
                    });
                }
                BindingResource::Sampler(sampler_binding) => {
                    let sampler = self.samplers.get(sampler_binding.sampler)?;
                    image_infos.push(vk::DescriptorImageInfo::default().sampler(sampler.sampler));
                    pending_writes.push(PendingDescriptorWrite::Image {
                        binding: binding.binding,
                        descriptor_type: vk::DescriptorType::SAMPLER,
                        info_index: image_infos.len() - 1,
                    });
                }
            }
        }
        // Validate resource handles before consuming pool capacity.
        let descriptor_demand = descriptors::Demand::from_layout(&layout_desc);
        let (descriptor_set, descriptor_pool) =
            self.descriptor_pools.allocate(&self.device, layout, descriptor_demand)?;
        let writes = pending_writes
            .iter()
            .map(|write| match *write {
                PendingDescriptorWrite::Buffer {
                    binding,
                    descriptor_type,
                    info_index,
                } => {
                    let buffer_info = buffer_infos.get(info_index).ok_or_else(|| {
                        Error::Backend("descriptor buffer info index is invalid".to_string())
                    })?;
                    Ok(vk::WriteDescriptorSet::default()
                        .dst_set(descriptor_set)
                        .dst_binding(binding)
                        .descriptor_type(descriptor_type)
                        .buffer_info(std::slice::from_ref(buffer_info)))
                }
                PendingDescriptorWrite::Image {
                    binding,
                    descriptor_type,
                    info_index,
                } => {
                    let image_info = image_infos.get(info_index).ok_or_else(|| {
                        Error::Backend("descriptor image info index is invalid".to_string())
                    })?;
                    Ok(vk::WriteDescriptorSet::default()
                        .dst_set(descriptor_set)
                        .dst_binding(binding)
                        .descriptor_type(descriptor_type)
                        .image_info(std::slice::from_ref(image_info)))
                }
            })
            .collect::<Result<Vec<_>>>()?;
        // SAFETY: Descriptor set and resources referenced by descriptor infos belong to this device.
        unsafe {
            self.device.update_descriptor_sets(&writes, &[]);
        }
        Ok(self.bind_groups.insert(VulkanResourceSet {
            sampled_depth,
            descriptor_set,
            descriptor_pool,
            descriptor_demand,
            layout: desc.layout,
            desc: desc.clone(),
        }))
    }

    /// Creates a shader module.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when validation or Vulkan module creation fails.
    fn create_shader_module(&mut self, desc: &ShaderModuleDescriptor) -> Result<ShaderModuleId> {
        desc.validate()?;
        let ShaderCode::Spirv(spirv) = &desc.binary.code else {
            return Err(Error::Shader(
                "Vulkan shader module requires SPIR-V code".to_string(),
            ));
        };
        let create_info = vk::ShaderModuleCreateInfo::default().code(spirv);
        // SAFETY: SPIR-V words are passed as immutable code slice to Vulkan.
        let module = unsafe { self.device.create_shader_module(&create_info, None) }
            .map_err(VulkanError::from)?;
        Ok(self.shader_modules.insert(VulkanShaderModule {
            module,
            stage: desc.binary.stage,
            entry_point: desc.binary.entry_point.clone(),
        }))
    }

    /// Creates a render pass.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when Vulkan render pass creation fails.
    fn create_render_pass(&mut self, desc: &RenderPassDescriptor) -> Result<RenderPassId> {
        desc.validate()?;
        if desc.sample_count > self.max_sample_count {
            return Err(Error::Unavailable(format!(
                "Vulkan render pass requests {} samples, device supports at most {}",
                desc.sample_count, self.max_sample_count
            )));
        }
        let depth_format = desc
            .depth_attachment
            .as_ref()
            .map(|attachment| attachment.format);
        Ok(self.render_passes.insert(VulkanRenderPass {
            color_formats: desc.color_attachments.iter().map(|attachment| attachment.format).collect(),
            depth_format,
            sample_count: desc.sample_count,
        }))
    }

    /// Creates a graphics render pipeline.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when validation or Vulkan pipeline creation fails.
    fn create_render_pipeline(
        &mut self,
        desc: &RenderPipelineDescriptor,
        _viewport_extent: gfx_core::Extent2d,
    ) -> Result<RenderPipelineId> {
        desc.validate()?;
        let render_pass_record = self.render_passes.get(desc.render_pass)?;
        if desc.depth_state.is_some() && render_pass_record.depth_format.is_none() {
            return Err(Error::InvalidInput(
                "Vulkan depth pipeline requires a render pass depth attachment".to_string(),
            ));
        }
        if desc.color_targets.len() != render_pass_record.color_formats.len()
            || desc
                .color_targets
                .iter()
                .zip(render_pass_record.color_formats.iter())
                .any(|(target, format)| target.format != *format)
        {
            return Err(Error::InvalidInput(
                "Vulkan pipeline color targets and render pass attachments differ".to_string(),
            ));
        }
        let vertex_shader = self.shader_modules.get(desc.vertex_shader)?;
        let fragment_shader = self.shader_modules.get(desc.fragment_shader)?;
        if vertex_shader.stage != ShaderStage::Vertex {
            return Err(Error::InvalidInput(
                "vertex shader module must use ShaderStage::Vertex".to_string(),
            ));
        }
        if fragment_shader.stage != ShaderStage::Fragment {
            return Err(Error::InvalidInput(
                "fragment shader module must use ShaderStage::Fragment".to_string(),
            ));
        }
        let (pipeline_layout, owns_pipeline_layout) =
            if let Some(pipeline_layout_id) = desc.pipeline_layout {
                (self.pipeline_layouts.get(pipeline_layout_id)?.layout, false)
            } else {
                (create_empty_pipeline_layout(&self.device)?, true)
            };
        let pipeline_result = create_graphics_pipeline(&GraphicsPipelineBuild {
            device: &self.device,
            pipeline_cache: self.pipeline_cache,
            depth_format: render_pass_record.depth_format,
            pipeline_layout,
            vertex_shader,
            vertex_entry_point: &desc.vertex_entry_point,
            fragment_shader,
            fragment_entry_point: &desc.fragment_entry_point,
            sample_count: render_pass_record.sample_count,
            desc,
        });
        let (pipeline_layout, pipeline) = match pipeline_result {
            Ok(result) => result,
            Err(error) => {
                if owns_pipeline_layout {
                    // SAFETY: the failed pipeline did not retain this private layout.
                    unsafe { self.device.destroy_pipeline_layout(pipeline_layout, None) };
                }
                return Err(error);
            }
        };
        Ok(self.render_pipelines.insert(VulkanRenderPipeline {
            pipeline,
            pipeline_layout,
            owns_pipeline_layout,
        }))
    }

    /// Creates a command encoder.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when command pool or command buffer allocation fails.
    fn create_command_encoder(
        &mut self,
        _desc: &CommandEncoderDescriptor,
    ) -> Result<CommandEncoderId> {
        self.create_encoder(1, self.graphics_queue_family_index)
    }

    /// Command pools are queue-family typed. A buffer allocated from the
    /// graphics family CANNOT be submitted on a dedicated Compute family.
    fn create_encoder(&mut self, buffer_count: usize, family: u32) -> Result<CommandEncoderId> {
        self.create_encoder_with_queries(buffer_count, family, buffer_count)
    }

    fn create_encoder_with_queries(&mut self, buffer_count: usize, family: u32, query_pairs: usize) -> Result<CommandEncoderId> {
        let query_pairs = query_pairs.max(buffer_count);
        let query_count = u32::try_from(query_pairs).ok().and_then(|pairs| pairs.checked_mul(2))
            .ok_or_else(|| Error::InvalidInput("Vulkan GPU trace query count overflow".into()))?;
        let buffer_count = u32::try_from(buffer_count).map_err(|error| {
            Error::InvalidInput(format!("command buffer count overflow: {error}"))
        })?;
        if buffer_count == 0 {
            return Err(Error::InvalidInput(
                "command encoder must allocate at least one command buffer".to_string(),
            ));
        }
        // Recycle only fully completed encoders with exactly the same command-buffer count.
        // In particular, a multipass blur submission must never submit stale extra buffers.
        self.poll_cleanup();
        if let Some(index) = self.free_command_encoders.iter().position(|encoder| {
            encoder.command_buffers.len() == buffer_count as usize
                && encoder.queue_family_index == family
                && encoder.gpu_draw_tags.len() >= query_pairs
        }) {
            return Ok(self
                .command_encoders
                .insert(self.free_command_encoders.swap_remove(index)));
        }

        let command_pool = create_command_pool(&self.device, family)?;
        let command_buffers =
            match allocate_command_buffers(&self.device, command_pool, buffer_count) {
                Ok(command_buffers) => command_buffers,
                Err(error) => {
                    // SAFETY: The pool was created above and no command buffers were allocated.
                    unsafe { self.device.destroy_command_pool(command_pool, None) };
                    return Err(error);
                }
            };
        // Timestamp query support belongs to the executing physical family.
        // No valid bits means no queries, not a fabricated zero-duration sample.
        let gpu_work_clock = if family == self.graphics_queue_family_index {
            self.timestamp_clock
        } else if Some(family) == self.dedicated_compute_family_index {
            self.dedicated_compute_timestamp_clock
        } else {
            None
        };
        let gpu_trace_query_pool = if gpu_work_clock.is_some() {
            let info = vk::QueryPoolCreateInfo::default()
                .query_type(vk::QueryType::TIMESTAMP)
                .query_count(query_count);
            // SAFETY: Query pool is owned by this encoder and released only after GPU completion.
            match unsafe { self.device.create_query_pool(&info, None) } {
                Ok(pool) => Some(pool),
                Err(error) => {
                    // SAFETY: No command buffers in this pool have been submitted.
                    unsafe { self.device.destroy_command_pool(command_pool, None) };
                    return Err(VulkanError::from(error).into());
                }
            }
        } else {
            None
        };
        Ok(self.command_encoders.insert(VulkanCommandEncoder {
            command_pool,
            queue_family_index: family,
            gpu_trace_passes: vec![VulkanGpuPass::Unused; command_buffers.len()],
            sample_main_pass: true,
            frame_probe_surface: None,
            gpu_draw_tags: vec![None; query_pairs],
            gpu_work_tag: None,
            gpu_work_clock,
            frame_submission: None,
            command_buffers,
            fence: vk::Fence::null(),
            owns_fence: false,
            cross_queue_semaphores: None,
            retire_primary_value: None,
            gpu_trace_query_pool,
            compute_resources: Vec::new(),
        }))
    }

    /// Records a render pass and draw call with optional bind groups.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when command recording or any handle lookup fails.
    fn record_draw_desc(
        &mut self,
        encoder_id: CommandEncoderId,
        draw: &DrawDescriptor,
    ) -> Result<()> {
        self.record_draw_steps_desc(
            encoder_id,
            draw.pass,
            &[DrawCommand {
                pipeline: draw.pipeline,
                bind_groups: draw.bind_groups.clone(),
                vertex_count: draw.vertex_count,
                first_vertex: draw.first_vertex,
                instance_count: draw.instance_count,
                first_instance: draw.first_instance,
                scissor: draw.scissor,
            }],
        )
    }

    /// Records a render pass with one or more render commands.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when command recording or any handle lookup fails.
    fn record_draw_steps_desc(
        &mut self,
        encoder_id: CommandEncoderId,
        pass: BeginRenderPassDescriptor,
        steps: &[DrawCommand],
    ) -> Result<()> {
        let commands: Vec<RenderCommand> = steps.iter().cloned().map(RenderCommand::Draw).collect();
        self.record_render_step_list_desc(encoder_id, pass, &commands, None)
    }

    fn record_render_step_list_desc(
        &mut self,
        encoder_id: CommandEncoderId,
        pass: BeginRenderPassDescriptor,
        steps: &[RenderCommand],
        depth_attachment: Option<DepthAttachment>,
    ) -> Result<()> {
        let command_buffer = self
            .command_encoders
            .get(encoder_id)?
            .command_buffers
            .first()
            .copied()
            .ok_or_else(|| Error::Backend("command encoder has no command buffer".to_string()))?;
        self.record_render_step_list_desc_into(
            encoder_id,
            command_buffer,
            pass,
            steps,
            depth_attachment,
            None,
            None,
            false,
        )
    }

    fn record_render_step_list_desc_into(
        &mut self,
        encoder_id: CommandEncoderId,
        command_buffer: vk::CommandBuffer,
        pass: BeginRenderPassDescriptor,
        steps: &[RenderCommand],
        depth_attachment: Option<DepthAttachment>,
        clear_region: Option<ScissorRect>,
        viewport: Option<Viewport>,
        graph_write_hazard: bool,
    ) -> Result<()> {
        let mut present_image = None;
        let mut render_target_texture = None;
        let mut render_target_transition = None;
        let (gpu_trace, gpu_trace_slot) = {
            let encoder = self.command_encoders.get(encoder_id)?;
            let probe = match pass.target {
                RenderTarget::Swapchain { .. } => encoder.sample_main_pass,
                RenderTarget::TextureView(_) => self.gpu_trace_enabled,
            };
            let slot = encoder
                .command_buffers
                .iter()
                .position(|&buffer| buffer == command_buffer);
            let trace = encoder.gpu_trace_query_pool.filter(|_| probe).and_then(|pool| {
                let index = u32::try_from(slot?).ok()?;
                Some((pool, index.checked_mul(2)?))
            });
            (trace, slot)
        };
        let render_pass_record = self.render_passes.get(pass.render_pass)?;
        if render_pass_record.color_formats.len() != 1 {
            return Err(Error::InvalidInput("Vulkan single-target render pass requires one color attachment".into()));
        }
        let render_pass_record = render_pass_record.clone();
        let depth_view =
            self.depth_attachment_view_from_pass(&render_pass_record, depth_attachment)?;
        let target_extent = match pass.target {
            RenderTarget::Swapchain { swapchain, .. } => {
                let extent = self.swapchains.get(swapchain)?.extent;
                gfx_core::Extent2d::new(extent.width, extent.height)?
            }
            RenderTarget::TextureView(texture_view_id) => {
                let texture_view = self.texture_views.get(texture_view_id)?;
                self.textures.get(texture_view.texture)?.desc.size
            }
        };
        let clear_region = match (viewport, clear_region) {
            (Some(viewport), Some(region)) => Some(viewport.map_scissor(region, target_extent)?),
            (_, clear_region) => clear_region,
        };
        let empty_clear_region = clear_region.is_some_and(ScissorRect::is_empty);
        let color_load_op = if empty_clear_region {
            LoadOp::Load
        } else {
            pass.color_load_op
        };
        let depth_load_op = depth_view.map(|(_, load_op)| {
            if empty_clear_region {
                LoadOp::Load
            } else {
                load_op
            }
        });
        let (image_view, extent, viewport_extent, viewport_origin, scissor_extent) = match pass.target {
            RenderTarget::Swapchain { swapchain, image_index } => {
                if matches!(color_load_op, LoadOp::Load) {
                    return Err(Error::InvalidInput("Vulkan swapchain render passes require color LoadOp::Clear".into()));
                }
                let chain = self.swapchains.get(swapchain)?;
                let image_index = usize::try_from(image_index)
                    .map_err(|error| Error::InvalidInput(format!("image index overflow: {error}")))?;
                if render_pass_record.sample_count != 1
                    || format_to_vk(render_pass_record.color_formats[0]) != chain.format
                {
                    return Err(Error::InvalidInput("Vulkan swapchain and render pass formats or sample counts differ".into()));
                }
                let view = *chain.image_views.get(image_index).ok_or_else(|| {
                    Error::InvalidInput("swapchain image index out of range".into())
                })?;
                present_image = Some(*chain.images.get(image_index).ok_or_else(|| {
                    Error::InvalidInput("swapchain image index out of range".into())
                })?);
                (view, chain.extent, chain.source_extent, gfx_core::SignedOrigin2d::ZERO, chain.source_extent)
            }
            RenderTarget::TextureView(texture_view_id) => {
                let texture_view = self.texture_views.get(texture_view_id)?;
                let texture = self.textures.get(texture_view.texture)?;
                if texture_view.desc.format != render_pass_record.color_formats[0]
                    || texture.desc.sample_count != render_pass_record.sample_count
                {
                    return Err(Error::InvalidInput("Vulkan texture and render pass formats or sample counts differ".into()));
                }
                if texture.desc.usage.contains(TextureUsage::TRANSIENT_ATTACHMENT) {
                    return Err(Error::InvalidInput("transient MSAA attachment requires a resolving offscreen pass".into()));
                }
                render_target_texture = Some(texture_view.texture);
                render_target_transition = Some(CommandRenderTargetTransition {
                    image: texture.image, old_layout: texture.layout, transient: false,
                });
                let extent = vk::Extent2D { width: texture.desc.size.width(), height: texture.desc.size.height() };
                let viewport_extent = viewport.map(|viewport| {
                    viewport.for_target(texture.desc.size).map(|extent| vk::Extent2D {
                        width: extent.width(), height: extent.height(),
                    })
                }).transpose()?.unwrap_or(extent);
                let viewport_origin = viewport.map(|viewport| viewport.mapped_origin(texture.desc.size))
                    .transpose()?.unwrap_or(gfx_core::SignedOrigin2d::ZERO);
                let scissor_extent = viewport.map(|viewport| viewport.mapped_scissor_extent(texture.desc.size))
                    .transpose()?.unwrap_or(texture.desc.size);
                (texture_view.view, extent, viewport_extent, viewport_origin,
                    vk::Extent2D { width: scissor_extent.width(), height: scissor_extent.height() })
            }
        };
        if let Some(attachment) = depth_attachment {
            let view = self.texture_views.get(attachment.target)?;
            let depth = self.textures.get(view.texture)?;
            if depth.desc.size.width() < extent.width || depth.desc.size.height() < extent.height {
                return Err(Error::InvalidInput("Vulkan depth attachment is smaller than the rendering area".into()));
            }
        }
        let mut draw_steps = Vec::with_capacity(steps.len());
        let mut descriptor_sets = Vec::new();
        // Reuse the resolved native descriptor range for consecutive draws with
        // identical portable bindings. No repeated registry lookups or copies.
        let mut previous_sets: Option<(&[BindGroupId], std::ops::Range<usize>)> = None;
        for step in steps.iter() {
            let scissor = match (viewport, step.scissor()) {
                (Some(viewport), Some(scissor)) => {
                    Some(viewport.map_scissor(scissor, target_extent)?)
                }
                (_, scissor) => scissor,
            };
            if scissor.is_some_and(ScissorRect::is_empty) {
                continue;
            }
            let pipeline_record = self.render_pipelines.get(step.pipeline())?;
            let groups = step.bind_groups();
            let set_range = if let Some((previous, range)) = &previous_sets
                && *previous == groups
            {
                range.clone()
            } else {
                let first_set = descriptor_sets.len();
                for &bind_group in groups {
                    descriptor_sets.push(self.bind_groups.get(bind_group)?.descriptor_set);
                }
                let range = first_set..descriptor_sets.len();
                previous_sets = Some((groups, range.clone()));
                range
            };
            let draw = match step {
                RenderCommand::DrawIndirect(step) => {
                    let buffer = self.buffers.get(step.arguments)?;
                    if !self.compute.capabilities.indirect_draw {
                        return Err(Error::Unavailable(
                            "Vulkan indirect first-instance is unsupported".into(),
                        ));
                    }
                    if !buffer.desc.usage.contains(BufferUsage::INDIRECT)
                        || step.offset % 4 != 0
                        || step
                            .offset
                            .checked_add(16)
                            .is_none_or(|end| end > buffer.desc.size)
                    {
                        return Err(Error::InvalidInput(
                            "invalid Vulkan indirect argument range or usage".into(),
                        ));
                    }
                    CommandDrawStepKind::Indirect {
                        buffer: buffer.buffer,
                        offset: step.offset,
                    }
                }
                RenderCommand::Draw(step) => CommandDrawStepKind::NonIndexed {
                    vertex_count: step.vertex_count,
                    first_vertex: step.first_vertex,
                    instance_count: step.instance_count,
                    first_instance: step.first_instance,
                },
                RenderCommand::DrawIndexed(step) => {
                    let index_buffer = self.buffers.get(step.index_buffer.buffer)?;
                    validate_index_buffer_range(
                        index_buffer.desc.usage,
                        index_buffer.desc.size,
                        step.index_buffer,
                        step.first_index,
                        step.index_count,
                    )?;
                    CommandDrawStepKind::Indexed {
                        buffer: index_buffer.buffer,
                        offset: step.index_buffer.offset,
                        index_type: index_format_to_vk(step.index_buffer.format),
                        index_count: step.index_count,
                        first_index: step.first_index,
                        base_vertex: step.base_vertex,
                        instance_count: step.instance_count,
                        first_instance: step.first_instance,
                    }
                }
                RenderCommand::DrawIndexedIndirect(step) => {
                    let arguments = self.buffers.get(step.arguments)?;
                    if !self.compute.capabilities.indirect_draw {
                        return Err(Error::Unavailable(
                            "Vulkan indirect first-instance is unsupported".into(),
                        ));
                    }
                    if !arguments.desc.usage.contains(BufferUsage::INDIRECT)
                        || step.offset % 4 != 0
                        || step
                            .offset
                            .checked_add(20)
                            .is_none_or(|end| end > arguments.desc.size)
                    {
                        return Err(Error::InvalidInput(
                            "invalid Vulkan indexed indirect argument range or usage".into(),
                        ));
                    }
                    let index = self.buffers.get(step.index_buffer.buffer)?;
                    // The index range itself comes from the argument record, so only the
                    // binding and one element at the offset can be validated here.
                    validate_index_buffer_range(
                        index.desc.usage,
                        index.desc.size,
                        step.index_buffer,
                        0,
                        1,
                    )?;
                    CommandDrawStepKind::IndexedIndirect {
                        buffer: arguments.buffer,
                        offset: step.offset,
                        index_buffer: index.buffer,
                        index_offset: step.index_buffer.offset,
                        index_type: index_format_to_vk(step.index_buffer.format),
                    }
                }
            };
            draw_steps.push(CommandDrawStepInfo {
                count: None,
                pipeline: pipeline_record.pipeline,
                pipeline_layout: pipeline_record.pipeline_layout,
                descriptor_sets: set_range,
                draw,
                scissor,
            });
        }
        let depth_accesses = self.depth_accesses(steps, depth_attachment)?;
        let transitions = render_target_transition
            .as_ref()
            .map(std::slice::from_ref)
            .unwrap_or(&[]);
        let result = record_command_buffer(&CommandRecordInfo {
            depth_accesses: &depth_accesses,
            device: &self.device,
            command_buffer,
            color_views: std::slice::from_ref(&image_view),
            depth_view: depth_view.map(|(view, _)| view),
            present_image,
            steps: &draw_steps,
            descriptor_sets: &descriptor_sets,
            multi_draw_indirect: self.multi_draw_indirect,
            max_draw_indirect_count: self.max_draw_indirect_count,
            extent,
            viewport_extent,
            viewport_origin,
            scissor_extent,
            color_load_op,
            depth_load_op,
            render_target_transitions: transitions,
            clear_region,
            resolve: None,
            gpu_trace,
            gpu_indirect_trace: None,
            host_query_reset: self.host_query_reset,
            graph_write_hazard,
        });
        if result.is_ok() {
            for access in depth_accesses {
                self.textures.get_mut(access.texture)?.layout = access.new_layout;
            }
            if let Some(slot) = gpu_trace_slot {
                self.command_encoders.get_mut(encoder_id)?.gpu_trace_passes[slot] =
                    if gpu_trace.is_none() {
                        VulkanGpuPass::Unused
                    } else if render_target_transition.is_some() {
                        VulkanGpuPass::Offscreen
                    } else {
                        VulkanGpuPass::Main
                    };
            }
            if let Some(texture_id) = render_target_texture {
                self.textures.get_mut(texture_id)?.layout =
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL;
            }
        }
        result
    }

    /// Records one offscreen pass writing `color_views.len()` color attachments
    /// plus an optional depth attachment. Used for MRT (multi-color) targets.
    fn record_render_step_list_views(
        &mut self,
        encoder_id: CommandEncoderId,
        command_buffer: vk::CommandBuffer,
        render_pass_id: RenderPassId,
        color_views: &[TextureViewId],
        resolve_view: Option<TextureViewId>,
        steps: &[RenderCommand],
        color_load_op: LoadOp<ClearColor>,
        depth_attachment: Option<DepthAttachment>,
        clear_region: Option<ScissorRect>,
        viewport: Option<Viewport>,
        graph_write_hazard: bool,
        gpu_draw_tag: Option<gfx_core::GpuTimestampTag>,
        counts: &[gfx_core::IndirectCountBinding],
    ) -> Result<()> {
        if color_views.is_empty() {
            return Err(Error::InvalidInput(
                "Vulkan offscreen pass must have at least one color attachment".into(),
            ));
        }
        let gpu_query = if gpu_draw_tag.is_some() {
            let encoder = self.command_encoders.get(encoder_id)?;
            let slot = encoder
                .command_buffers
                .iter()
                .position(|&cmd| cmd == command_buffer);
            slot.and_then(|index| {
                let pool = encoder.gpu_trace_query_pool?;
                let start = u32::try_from(index).ok()?.checked_mul(2)?;
                Some((pool, start, index))
            })
        } else {
            None
        };
        let render_pass_record = self.render_passes.get(render_pass_id)?.clone();
        if color_views.len() != render_pass_record.color_formats.len() {
            return Err(Error::InvalidInput(
                "Vulkan offscreen attachment and render pass counts differ".into(),
            ));
        }
        let depth_view =
            self.depth_attachment_view_from_pass(&render_pass_record, depth_attachment)?;
        let mut transitions = Vec::with_capacity(color_views.len());
        let mut render_target_textures = Vec::with_capacity(color_views.len());
        let mut extent = vk::Extent2D {
            width: 1,
            height: 1,
        };
        // Shared invariant: attachments match the pass sample count and a resolve
        // destination is single-sampled.
        let color_samples = color_views
            .iter()
            .map(|view| {
                let view = self.texture_views.get(*view)?;
                Ok(self.textures.get(view.texture)?.desc.sample_count)
            })
            .collect::<Result<Vec<_>>>()?;
        let depth_samples = match depth_attachment {
            Some(attachment) => {
                let view = self.texture_views.get(attachment.target)?;
                Some(self.textures.get(view.texture)?.desc.sample_count)
            }
            None => None,
        };
        let resolve_samples = match resolve_view {
            Some(view) => {
                let view = self.texture_views.get(view)?;
                Some(self.textures.get(view.texture)?.desc.sample_count)
            }
            None => None,
        };
        gfx_core::validate_pass_attachments(
            render_pass_record.sample_count,
            color_samples.into_iter(),
            depth_samples,
            resolve_samples,
        )?;
        let mut image_views = Vec::with_capacity(color_views.len());
        for (index, texture_view_id) in color_views.iter().enumerate() {
            let texture_view = self.texture_views.get(*texture_view_id)?;
            if texture_view.desc.format != render_pass_record.color_formats[index] {
                return Err(Error::InvalidInput(
                    "Vulkan texture view and render pass color formats differ".into(),
                ));
            }
            let texture = self.textures.get(texture_view.texture)?;
            let texture_extent = vk::Extent2D {
                width: texture.desc.size.width(),
                height: texture.desc.size.height(),
            };
            if image_views.is_empty() {
                extent = texture_extent;
            } else if extent != texture_extent {
                return Err(Error::InvalidInput(
                    "Vulkan color attachments must share one extent".into(),
                ));
            }
            if transitions
                .iter()
                .any(|transition: &CommandRenderTargetTransition| transition.image == texture.image)
            {
                return Err(Error::InvalidInput(
                    "Vulkan color attachments must not repeat a texture".into(),
                ));
            }
            let transient = texture.desc.usage.contains(TextureUsage::TRANSIENT_ATTACHMENT);
            if transient && (index != 0 || resolve_view.is_none()
                || !matches!(color_load_op, LoadOp::Clear(_)) || clear_region.is_some())
            {
                return Err(Error::InvalidInput("transient MSAA requires a full clear and first-attachment resolve".into()));
            }
            transitions.push(CommandRenderTargetTransition {
                image: texture.image,
                old_layout: texture.layout,
                transient,
            });
            image_views.push(texture_view.view);
            render_target_textures.push(texture_view.texture);
        }
        let depth_load_op = depth_view.map(|(_, load_op)| {
            if clear_region.is_some_and(ScissorRect::is_empty) {
                LoadOp::Load
            } else {
                load_op
            }
        });
        let target_size = gfx_core::Extent2d::new(extent.width, extent.height)?;
        let viewport_extent = viewport
            .map(|viewport| {
                viewport.for_target(target_size).map(|extent| vk::Extent2D {
                    width: extent.width(),
                    height: extent.height(),
                })
            })
            .transpose()?
            .unwrap_or(extent);
        let viewport_origin = viewport
            .map(|viewport| viewport.mapped_origin(target_size))
            .transpose()?
            .unwrap_or(gfx_core::SignedOrigin2d::ZERO);
        let scissor_extent = viewport
            .map(|viewport| viewport.mapped_scissor_extent(target_size))
            .transpose()?
            .map(|extent| vk::Extent2D {
                width: extent.width(),
                height: extent.height(),
            })
            .unwrap_or(extent);
        let target_size = gfx_core::Extent2d::new(extent.width, extent.height)?;
        let clear_region = match (viewport, clear_region) {
            (Some(viewport), Some(region)) => Some(viewport.map_scissor(region, target_size)?),
            (_, clear_region) => clear_region,
        };
        let color_load_op = if clear_region.is_some_and(ScissorRect::is_empty) {
            LoadOp::Load
        } else {
            color_load_op
        };
        if let Some(attachment) = depth_attachment {
            let view = self.texture_views.get(attachment.target)?;
            let depth = self.textures.get(view.texture)?;
            if depth.desc.size.width() < extent.width || depth.desc.size.height() < extent.height {
                return Err(Error::InvalidInput("Vulkan depth attachment is smaller than the rendering area".into()));
            }
        }
        let mut draw_steps = Vec::with_capacity(steps.len());
        let mut descriptor_sets = Vec::new();
        // Reuse the resolved native descriptor range for consecutive draws with
        // identical portable bindings. No repeated registry lookups or copies.
        let mut previous_sets: Option<(&[BindGroupId], std::ops::Range<usize>)> = None;
        let mut counts = counts.iter().peekable();
        for (command_index, step) in steps.iter().enumerate() {
            let count = if counts.peek().is_some_and(|count| count.command_index == command_index) {
                counts.next().copied()
            } else { None };
            let scissor = match (viewport, step.scissor()) {
                (Some(viewport), Some(scissor)) => {
                    Some(viewport.map_scissor(scissor, target_size)?)
                }
                (_, scissor) => scissor,
            };
            if scissor.is_some_and(ScissorRect::is_empty) {
                continue;
            }
            let pipeline_record = self.render_pipelines.get(step.pipeline())?;
            let groups = step.bind_groups();
            let set_range = if let Some((previous, range)) = &previous_sets
                && *previous == groups
            {
                range.clone()
            } else {
                let first_set = descriptor_sets.len();
                for &bind_group in groups {
                    descriptor_sets.push(self.bind_groups.get(bind_group)?.descriptor_set);
                }
                let range = first_set..descriptor_sets.len();
                previous_sets = Some((groups, range.clone()));
                range
            };
            let draw = match step {
                RenderCommand::DrawIndirect(step) => {
                    let buffer = self.buffers.get(step.arguments)?;
                    if !self.compute.capabilities.indirect_draw {
                        return Err(Error::Unavailable(
                            "Vulkan indirect first-instance is unsupported".into(),
                        ));
                    }
                    if !buffer.desc.usage.contains(BufferUsage::INDIRECT)
                        || step.offset % 4 != 0
                        || step
                            .offset
                            .checked_add(16)
                            .is_none_or(|end| end > buffer.desc.size)
                    {
                        return Err(Error::InvalidInput(
                            "invalid Vulkan indirect argument range or usage".into(),
                        ));
                    }
                    CommandDrawStepKind::Indirect {
                        buffer: buffer.buffer,
                        offset: step.offset,
                    }
                }
                RenderCommand::Draw(step) => CommandDrawStepKind::NonIndexed {
                    vertex_count: step.vertex_count,
                    first_vertex: step.first_vertex,
                    instance_count: step.instance_count,
                    first_instance: step.first_instance,
                },
                RenderCommand::DrawIndexed(step) => {
                    let index_buffer = self.buffers.get(step.index_buffer.buffer)?;
                    validate_index_buffer_range(
                        index_buffer.desc.usage,
                        index_buffer.desc.size,
                        step.index_buffer,
                        step.first_index,
                        step.index_count,
                    )?;
                    CommandDrawStepKind::Indexed {
                        buffer: index_buffer.buffer,
                        offset: step.index_buffer.offset,
                        index_type: index_format_to_vk(step.index_buffer.format),
                        index_count: step.index_count,
                        first_index: step.first_index,
                        base_vertex: step.base_vertex,
                        instance_count: step.instance_count,
                        first_instance: step.first_instance,
                    }
                }
                RenderCommand::DrawIndexedIndirect(step) => {
                    let arguments = self.buffers.get(step.arguments)?;
                    if !self.compute.capabilities.indirect_draw {
                        return Err(Error::Unavailable(
                            "Vulkan indirect first-instance is unsupported".into(),
                        ));
                    }
                    if !arguments.desc.usage.contains(BufferUsage::INDIRECT)
                        || step.offset % 4 != 0
                        || step
                            .offset
                            .checked_add(20)
                            .is_none_or(|end| end > arguments.desc.size)
                    {
                        return Err(Error::InvalidInput(
                            "invalid Vulkan indexed indirect argument range or usage".into(),
                        ));
                    }
                    let index = self.buffers.get(step.index_buffer.buffer)?;
                    // The index range itself comes from the argument record, so only the
                    // binding and one element at the offset can be validated here.
                    validate_index_buffer_range(
                        index.desc.usage,
                        index.desc.size,
                        step.index_buffer,
                        0,
                        1,
                    )?;
                    CommandDrawStepKind::IndexedIndirect {
                        buffer: arguments.buffer,
                        offset: step.offset,
                        index_buffer: index.buffer,
                        index_offset: step.index_buffer.offset,
                        index_type: index_format_to_vk(step.index_buffer.format),
                    }
                }
            };
            let count = if let Some(count) = count {
                let buffer = self.buffers.get(count.count_buffer)?;
                Some((buffer.buffer, count.count_offset, count.max_draw_count))
            } else { None };
            draw_steps.push(CommandDrawStepInfo {
                count,
                pipeline: pipeline_record.pipeline,
                pipeline_layout: pipeline_record.pipeline_layout,
                descriptor_sets: set_range,
                draw,
                scissor,
            });
        }
        let depth_accesses = self.depth_accesses(steps, depth_attachment)?;
        let resolve = match resolve_view {
            Some(resolve_view_id) => {
                let source = self.textures.get(render_target_textures[0])?;
                let dst_texture_view = self.texture_views.get(resolve_view_id)?;
                let destination = self.textures.get(dst_texture_view.texture)?;
                if render_target_textures.contains(&dst_texture_view.texture)
                    || destination.desc.size != source.desc.size
                    || destination.desc.format != source.desc.format
                {
                    return Err(Error::InvalidInput("Vulkan resolve target must be a distinct texture of the source format and extent".into()));
                }
                // Dynamic rendering resolves only the render area, whereas the
                // explicit legacy resolve covers the complete mip-zero image.
                // Restrict the native fast path to matching simple 2D views.
                let src_view = self.texture_views.get(color_views[0])?;
                let native_view = (destination.desc.usage.contains(TextureUsage::COLOR_ATTACHMENT)
                    && source.desc.dimension == TextureDimension::D2
                    && destination.desc.dimension == TextureDimension::D2
                    && src_view.desc.base_mip_level == 0
                    && src_view.desc.mip_level_count == 1
                    && dst_texture_view.desc.base_mip_level == 0
                    && dst_texture_view.desc.mip_level_count == 1
                    && src_view.desc.format == dst_texture_view.desc.format
                    && !matches!(src_view.desc.format, Format::R8Uint | Format::Rg8Uint | Format::Rgba8Uint))
                    .then_some(dst_texture_view.view);
                if source.desc.usage.contains(TextureUsage::TRANSIENT_ATTACHMENT) && native_view.is_none() {
                    return Err(Error::InvalidInput("transient MSAA requires native attachment resolve".into()));
                }
                Some(CommandResolve {
                    source: source.image,
                    destination: destination.image,
                    extent,
                    old_layout: destination.layout,
                    native_view,
                    discard_source: source.desc.usage.contains(TextureUsage::DISCARD_AFTER_RESOLVE),
                })
            }
            None => None,
        };
        let result = record_command_buffer(&CommandRecordInfo {
            depth_accesses: &depth_accesses,
            device: &self.device,
            command_buffer,
            color_views: &image_views,
            depth_view: depth_view.map(|(view, _)| view),
            present_image: None,
            steps: &draw_steps,
            descriptor_sets: &descriptor_sets,
            multi_draw_indirect: self.multi_draw_indirect,
            max_draw_indirect_count: self.max_draw_indirect_count,
            extent,
            viewport_extent,
            viewport_origin,
            scissor_extent,
            color_load_op,
            depth_load_op,
            render_target_transitions: &transitions,
            clear_region,
            resolve,
            gpu_trace: None,
            gpu_indirect_trace: gpu_query.map(|(pool, first, _)| (pool, first)),
            host_query_reset: self.host_query_reset,
            graph_write_hazard,
        });
        if result.is_ok() {
            if let (Some(tag), Some((_, _, index))) = (gpu_draw_tag, gpu_query) {
                self.command_encoders.get_mut(encoder_id)?.gpu_draw_tags[index] = Some(tag);
            }
            for access in depth_accesses {
                self.textures.get_mut(access.texture)?.layout = access.new_layout;
            }
            for (texture_id, transition) in render_target_textures.into_iter().zip(&transitions) {
                self.textures.get_mut(texture_id)?.layout = if transition.transient {
                    vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
                } else {
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
                };
            }
            if let Some(view) = resolve_view {
                let texture = self.texture_views.get(view)?.texture;
                self.textures.get_mut(texture)?.layout = vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL;
            }
        }
        result
    }

    fn depth_attachment_view_from_pass(
        &self,
        render_pass: &VulkanRenderPass,
        depth_attachment: Option<DepthAttachment>,
    ) -> Result<Option<(vk::ImageView, LoadOp<f32>)>> {
        let Some(depth_attachment) = depth_attachment else {
            if render_pass.depth_format.is_some() {
                return Err(Error::InvalidInput(
                    "Vulkan render pass expects a depth attachment".to_string(),
                ));
            }
            return Ok(None);
        };
        let Some(depth_format) = render_pass.depth_format else {
            return Err(Error::InvalidInput(
                "Vulkan depth attachment was provided for a color-only render pass".to_string(),
            ));
        };
        let texture_view = self.texture_views.get(depth_attachment.target)?;
        let texture = self.textures.get(texture_view.texture)?;
        if texture.desc.format != depth_format || texture.desc.sample_count != render_pass.sample_count {
            return Err(Error::InvalidInput(
                "Vulkan depth attachment format or sample count does not match render pass".to_string(),
            ));
        }
        if !texture.desc.usage.contains(TextureUsage::DEPTH_ATTACHMENT) {
            return Err(Error::InvalidInput(
                "Vulkan depth attachment texture must include DEPTH_ATTACHMENT usage".to_string(),
            ));
        }
        Ok(Some((texture_view.view, depth_attachment.depth_load_op)))
    }

    /// Submits a command buffer.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when submission fails.
    fn submit(&mut self, encoder_id: CommandEncoderId) -> Result<()> {
        let command_buffer = self.command_encoders.get(encoder_id)?.command_buffers[0];
        self.submit_command_buffer(command_buffer, &[], &[], vk::Fence::null())?;
        self.submitted_frames = self.submitted_frames.saturating_add(1);
        self.poll_cleanup();
        Ok(())
    }

    fn record_and_present_timed(
        &mut self,
        swapchain_id: gfx_core::SwapchainId,
        render_pass_id: RenderPassId,
        steps: &[RenderCommand],
        clear_color: ClearColor,
        depth_attachment: Option<DepthAttachment>,
        damage: Option<ScissorRect>,
    ) -> Result<Option<PresentedFrame>> {
        let reserved = self.swapchains.get(swapchain_id)?.reserved_frame;
        let Some(present_frame) = (match reserved {
            Some(frame) => {
                if !self.reserved_frame_ready(frame)? {
                    return Ok(None);
                }
                Some(frame)
            },
            None => self.acquire_present_frame(swapchain_id, false)?,
        }) else {
            return Ok(None);
        };
        // The late-acquire API owns a real image too. Keep it reserved
        // across encoder allocation/recording failures, just like preflight.
        self.swapchains.get_mut(swapchain_id)?.reserved_frame = Some(present_frame);
        let mut timings = PresentTimings {
            acquire_fence_wait: present_frame.acquire_fence_wait,
            image_acquire: present_frame.image_acquire,
            ..PresentTimings::default()
        };
        let encoder_create_started = Instant::now();
        let encoder = self.create_command_encoder(&CommandEncoderDescriptor { label: None })?;
        let chain = self.swapchains.get_mut(swapchain_id)?;
        let sample = chain.frame_probe.should_sample(Instant::now(), self.gpu_trace_enabled);
        let native_chain = chain.swapchain;
        let encoder_record = self.command_encoders.get_mut(encoder)?;
        encoder_record.sample_main_pass = sample;
        encoder_record.frame_probe_surface = sample.then_some((swapchain_id, native_chain));
        timings.command_encoder_create = encoder_create_started.elapsed();
        let command_record_started = Instant::now();
        let record_result = self.record_render_step_list_desc(
            encoder,
            BeginRenderPassDescriptor {
                render_pass: render_pass_id,
                target: RenderTarget::Swapchain {
                    swapchain: swapchain_id,
                    image_index: present_frame.image_index,
                },
                color_load_op: LoadOp::Clear(clear_color),
            },
            steps,
            depth_attachment,
        );
        timings.command_record = command_record_started.elapsed();
        if let Err(error) = record_result {
            let _destroy_result = self.destroy_temporary_command_encoder_now(encoder);
            return Err(error);
        }

        let command_buffer = self.command_encoders.get(encoder)?.command_buffers[0];
        // SAFETY: The frame fence was waited before acquire and is about to be used by queue_submit.
        let fence_reset_started = Instant::now();
        if let Err(error) = unsafe { self.device.reset_fences(&[present_frame.fence]) } {
            let _ = self.destroy_temporary_command_encoder_now(encoder);
            return Err(VulkanError::from(error).into());
        }
        timings.fence_reset = fence_reset_started.elapsed();
        let submit_result = self.submit_command_buffer_timed(
            command_buffer,
            &[present_frame.image_available],
            &[present_frame.render_finished],
            present_frame.fence,
        );
        timings.queue_submit = match submit_result {
            Ok(elapsed) => elapsed,
            Err(error) => {
                // Keep the acquired image, but do not reset the same fence
                // again until the recovery submit has actually completed.
                if let Some(frame) = self.swapchains.get_mut(swapchain_id)?.reserved_frame.as_mut() {
                    frame.recovery_pending = true;
                }
                let recovery = self.signal_frame_fence_after_submit_failure(present_frame.fence);
                let _destroy_result = self.destroy_temporary_command_encoder_now(encoder);
                recovery?;
                return Err(error);
            }
        };

        // Successful submission consumed the acquire semaphore. Do not
        // reuse the reservation even if later WSI presentation fails.
        self.swapchains.get_mut(swapchain_id)?.reserved_frame = None;
        let mut encoder_resource = self.command_encoders.take(encoder)?;
        encoder_resource.fence = present_frame.fence;
        encoder_resource.owns_fence = false;
        // Publish the encoder's retire record BEFORE WSI present: an outdated
        // surface may synchronously retire and destroy the old native fences.
        let submission = self.submissions.insert(VulkanSubmission {
            fence: present_frame.fence,
            queue: gfx_core::GpuQueueId::PRIMARY,
            owned_by_submission: false,
            timeline_value: self.primary_timeline.as_ref().map(|timeline| timeline.last()),
        });
        encoder_resource.frame_submission = Some(submission);
        self.swapchains.get_mut(swapchain_id)?.frame_submissions[present_frame.frame_index] =
            Some(submission);
        self.deferred_destroys
            .retire(0, DeferredResource::CommandEncoder(encoder_resource));
        let present_started = Instant::now();
        if let Err(error) = self.present(
            swapchain_id,
            present_frame.image_index,
            present_frame.render_finished,
            damage,
        ) {
            // No submission ID escaped. The deferred encoder still retires.
            let _ = self.submissions.take(submission);
            self.completed_frame_probes.retain(|(id, _)| *id != submission);
            return Err(error);
        }
        timings.queue_present = present_started.elapsed();
        let post_present_cleanup_started = Instant::now();
        let swapchain = self.swapchains.get_mut(swapchain_id)?;
        let frame_slots = swapchain.in_flight_fences.len().max(1);
        swapchain.frame_index = (present_frame.frame_index + 1) % frame_slots;
        self.submitted_frames = self.submitted_frames.saturating_add(1);
        self.poll_cleanup();
        timings.post_present_cleanup = post_present_cleanup_started.elapsed();
        Ok(Some(PresentedFrame {
            submission: Some(submission),
            timings: Some(timings),
        }))
    }

    fn record_texture_passes(
        &mut self,
        passes: &[RenderPassCommands<'_>],
        tags: Option<&[Option<gfx_core::GpuTimestampTag>]>,
        independent: bool,
        counts: &[&[gfx_core::IndirectCountBinding]],
    ) -> Result<Option<SubmissionId>> {
        if !counts.is_empty() && counts.len() != passes.len() {
            return Err(Error::InvalidInput("offscreen count table length mismatch".into()));
        }
        if tags.is_some_and(|t| t.len() != passes.len()) {
            return Err(Error::InvalidInput(
                "offscreen GPU tag count mismatch".into(),
            ));
        }
        if passes.is_empty() {
            return Ok(None);
        }

        let mut target_layouts = Vec::<(TextureId, vk::ImageLayout)>::new();
        for pass in passes {
            let depths = self.depth_accesses(pass.commands, pass.depth_attachment)?;
            let color_ids = pass
                .color_attachments
                .iter()
                .chain(pass.resolve_target.iter())
                .map(|view| self.texture_views.get(*view).map(|v| v.texture))
                .collect::<Result<Vec<_>>>()?;
            for texture_id in color_ids
                .into_iter()
                .chain(depths.iter().map(|access| access.texture))
            {
                if target_layouts.iter().all(|(known, _)| *known != texture_id) {
                    target_layouts.push((texture_id, self.textures.get(texture_id)?.layout));
                }
            }
        }

        let encoder = self.create_encoder(passes.len(), self.graphics_queue_family_index)?;
        // The real target TextureId, not the logical variant index, is the
        // resource identity. Multiple horizontal Gaussian passes may share
        // one physical image. The graph marks WAW handoffs when a later pass
        // reuses its attachment; the native command buffer executes that
        // access barrier before entering the render pass.
        let mut previous_writers = Vec::<TextureId>::new();
        for (index, pass) in passes.iter().enumerate() {
            let graph_write_hazard = pass.color_attachments.iter().any(|view| {
                let texture_id = self.texture_views.get(*view).map(|v| v.texture);
                match texture_id {
                    Ok(texture_id) => {
                        graph_color_write_requires_barrier(&mut previous_writers, texture_id)
                    }
                    Err(_) => false,
                }
            });
            let command_buffer = self.command_encoders.get(encoder)?.command_buffers[index];
            let result = self.record_render_step_list_views(
                encoder,
                command_buffer,
                pass.render_pass,
                pass.color_attachments,
                pass.resolve_target,
                pass.commands,
                pass.color_load_op,
                pass.depth_attachment,
                pass.clear_region,
                pass.viewport,
                graph_write_hazard,
                tags.and_then(|tags| tags[index]).filter(|tag| {
                    tag.stage == gfx_core::GpuTimestampStage::DrawIndirect
                        && !pass.commands.is_empty()
                        && pass.commands.iter().all(|command| {
                            matches!(
                                command,
                                RenderCommand::DrawIndirect(_)
                                    | RenderCommand::DrawIndexedIndirect(_)
                            )
                        })
                }),
                counts.get(index).copied().unwrap_or(&[]),
            );
            if let Err(error) = result {
                self.destroy_temporary_command_encoder_now(encoder)?;
                for (texture_id, layout) in &target_layouts {
                    self.textures.get_mut(*texture_id)?.layout = *layout;
                }
                return Err(error);
            }
        }

        let submission = if independent {
            // The native graphics queue already waits for the producer
            // semaphore. Retire this encoder by its consumer-queue fence,
            // as the legacy Vulkan offscreen path does, without returning
            // a duplicate SubmissionId that could outlive its VkFence.
            self.submit_command_encoder_independent(encoder, false)
                .map(|_| None)
        } else {
            self.submit_command_encoder_deferred(encoder).map(|_| None)
        };
        let submission = match submission {
            Ok(submission) => submission,
            Err(error) => {
                self.destroy_temporary_command_encoder_now(encoder)?;
                for (texture_id, layout) in &target_layouts {
                    self.textures.get_mut(*texture_id)?.layout = *layout;
                }
                return Err(error);
            }
        };
        self.poll_cleanup();
        Ok(submission)
    }

    fn submit_command_encoder_deferred(&mut self, encoder: CommandEncoderId) -> Result<()> {
        self.submit_command_encoder_tracked(encoder, false).map(|_| ())
    }

    fn submit_encoder(&mut self, encoder: CommandEncoderId) -> Result<SubmissionId> {
        self.submit_command_encoder_tracked(encoder, true)?
            .ok_or_else(|| Error::Backend("Vulkan tracked encoder has no submission".into()))
    }

    fn submit_command_encoder_tracked(
        &mut self,
        encoder: CommandEncoderId,
        tracked: bool,
    ) -> Result<Option<SubmissionId>> {
        // Timeline-backed encoders need no private per-submit fence.
        let _ = self.command_encoders.get(encoder)?;
        let fence = if self.primary_timeline.is_some() {
            vk::Fence::null()
        } else {
            create_fence(&self.device, false)?
        };
        if let Err(error) = self.submit_command_buffers(
            &self.command_encoders.get(encoder)?.command_buffers,
            &[],
            &[],
            fence,
        ) {
            // SAFETY: Submission failed, so the fence is not pending on the queue.
            unsafe { self.device.destroy_fence(fence, None) };
            return Err(error);
        }
        let mut encoder_resource = self.command_encoders.take(encoder)?;
        encoder_resource.fence = fence;
        encoder_resource.owns_fence = fence != vk::Fence::null();
        let timeline_value = self.primary_timeline.as_ref().map(|timeline| timeline.last());
        encoder_resource.retire_primary_value = timeline_value;
        let submission = tracked.then(|| self.submissions.insert(VulkanSubmission {
            fence,
            queue: gfx_core::GpuQueueId::PRIMARY,
            owned_by_submission: false,
            timeline_value,
        }));
        // Publish the association before the encoder can be recycled.
        // recycle_command_encoder records completion before destroying the fence.
        encoder_resource.frame_submission = submission;
        self.deferred_destroys
            .retire(0, DeferredResource::CommandEncoder(encoder_resource));
        self.submitted_frames = self.submitted_frames.saturating_add(1);
        Ok(submission)
    }

    /// True second Graphics queue submission. Both queues share one family,
    /// so image queue-family ownership never changes. Three native submits:
    /// graphics release -> offscreen producer -> graphics acquire, joined by
    /// two binary GPU semaphores. No CPU fence wait occurs on the render path.
    fn submit_command_encoder_independent(
        &mut self,
        encoder: CommandEncoderId,
        track_native_producer: bool,
    ) -> Result<Option<SubmissionId>> {
        let Some(queue) = self.sidecar_queue else {
            if track_native_producer {
                return Err(Error::Unavailable(
                    "independent Vulkan graphics-family queue is unavailable".into(),
                ));
            }
            return self.submit_command_encoder_deferred(encoder).map(|_| None);
        };
        self.submit_command_encoder_native_secondary(
            encoder,
            track_native_producer,
            queue,
            self.graphics_queue_family_index,
            gfx_core::GpuQueueId {
                family: self.graphics_queue_family_index,
                index: 1,
            },
        )
    }

    /// Dedicated Compute-only families execute only buffer-based dispatch.
    /// Every bound Storage/Uniform buffer uses CONCURRENT sharing between
    /// Graphics and this Compute family. EXCLUSIVE textures are rejected.
    fn submit_command_encoder_dedicated_compute(
        &mut self,
        encoder: CommandEncoderId,
    ) -> Result<Option<SubmissionId>> {
        let queue = self.dedicated_compute_queue.ok_or_else(|| {
            Error::Unavailable("Vulkan dedicated compute queue is unavailable".into())
        })?;
        let family = self.dedicated_compute_family_index.ok_or_else(|| {
            Error::Unavailable("Vulkan dedicated compute family is unavailable".into())
        })?;
        let queue_id = self.dedicated_compute_queue_id().ok_or_else(|| {
            Error::Unavailable("Vulkan dedicated compute queue ID is unavailable".into())
        })?;
        self.submit_command_encoder_native_secondary(encoder, true, queue, family, queue_id)
    }

    /// One native queue handoff implementation for either secondary graphics
    /// or a buffer-only separate Compute family. GPU-native semaphores order
    /// uploads -> producer -> graphics consumer without any host Fence wait.
    fn submit_command_encoder_native_secondary(
        &mut self,
        encoder: CommandEncoderId,
        track_native_producer: bool,
        queue: vk::Queue,
        family: u32,
        queue_id: gfx_core::GpuQueueId,
    ) -> Result<Option<SubmissionId>> {
        if self.command_encoders.get(encoder)?.queue_family_index != family {
            return Err(Error::InvalidInput(
                "Vulkan native command pool family does not match executing queue".into(),
            ));
        }
        if let (Some(primary), Some(secondary)) = (&self.primary_timeline, &self.secondary_timeline) {
            let result = (|| -> Result<(u64, u64)> {
                // The last resource-bearing primary submission already
                // signals ALL_COMMANDS. Bootstrap only the first handoff.
                if primary.last() == 0 {
                    primary.submit(&self.device, self.graphics_queue, &[], &[], &[],
                        vk::PipelineStageFlags2::ALL_COMMANDS, vk::Fence::null())?;
                }
                let buffers = &self.command_encoders.get(encoder)?.command_buffers;
                let produced = secondary.submit_after(
                    &self.device, queue, buffers, primary, primary.last(),
                )?;
                let joined = primary.submit_after(
                    &self.device, self.graphics_queue, &[], secondary, produced,
                )?;
                Ok((produced, joined))
            })();
            let (produced, joined) = match result {
                Ok(values) => values,
                Err(error) => {
                    // A partial handoff may have submitted command buffers.
                    // Preserve the exceptional failure retirement contract.
                    let _ = unsafe { self.device.device_wait_idle() };
                    return Err(error);
                }
            };
            let mut resource = self.command_encoders.take(encoder)?;
            resource.retire_primary_value = Some(joined);
            self.deferred_destroys.retire(0, DeferredResource::CommandEncoder(resource));
            self.submitted_frames = self.submitted_frames.saturating_add(1);
            return Ok(track_native_producer.then(|| self.submissions.insert(VulkanSubmission {
                fence: vk::Fence::null(),
                queue: queue_id,
                owned_by_submission: false,
                timeline_value: Some(produced),
            })));
        }
        let semaphore_info = vk::SemaphoreCreateInfo::default();
        let first = unsafe { self.device.create_semaphore(&semaphore_info, None) }
            .map_err(VulkanError::from)?;
        let second = match unsafe { self.device.create_semaphore(&semaphore_info, None) } {
            Ok(value) => value,
            Err(error) => {
                unsafe { self.device.destroy_semaphore(first, None) };
                return Err(VulkanError::from(error).into());
            }
        };
        let fence = match create_fence(&self.device, false) {
            Ok(value) => value,
            Err(error) => {
                unsafe {
                    self.device.destroy_semaphore(second, None);
                    self.device.destroy_semaphore(first, None);
                }
                return Err(error);
            }
        };
        // The consumer join fence protects command-encoder destruction. A
        // separate producer fence, only when requested, certifies the actual
        // physical queue on which Compute/3D command buffers ran.
        let producer_fence = if track_native_producer {
            match create_fence(&self.device, false) {
                Ok(fence) => Some(fence),
                Err(error) => {
                    // SAFETY: Neither semaphore nor consumer fence has yet
                    // been submitted to any native queue.
                    unsafe {
                        self.device.destroy_fence(fence, None);
                        self.device.destroy_semaphore(second, None);
                        self.device.destroy_semaphore(first, None);
                    }
                    return Err(error);
                }
            }
        } else {
            None
        };
        let result = (|| -> Result<()> {
            // Earlier GPU uploads and main GUI work must precede 3D.
            submit::binary(&self.device, self.graphics_queue, &[], &[], &[first],
                vk::PipelineStageFlags2::ALL_COMMANDS, vk::Fence::null())?;

            // Submit 3D command buffers to queue index 1 in the same family.
            let buffers = &self.command_encoders.get(encoder)?.command_buffers;
            let producer_fence = producer_fence.unwrap_or(vk::Fence::null());
            if let Some(timeline) = &self.secondary_timeline {
                timeline.submit(&self.device, queue, buffers, &[first], &[second],
                    vk::PipelineStageFlags2::ALL_COMMANDS, producer_fence)?;
            } else {
                submit::binary(&self.device, queue, buffers, &[first], &[second],
                    vk::PipelineStageFlags2::ALL_COMMANDS, producer_fence)?;
            }

            // Keep the sampled attachment unavailable to the GUI graphics
            // queue until all sidecar writes and layout transitions finish.
            // The fence is on the consumer queue so it protects resource
            // retirement after both queues' GPU work completes.
            if let Some(timeline) = &self.primary_timeline {
                timeline.submit(&self.device, self.graphics_queue, &[], &[second], &[],
                    vk::PipelineStageFlags2::ALL_COMMANDS, fence)?;
            } else {
                submit::binary(&self.device, self.graphics_queue, &[], &[second], &[],
                    vk::PipelineStageFlags2::ALL_COMMANDS, fence)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            // Rare submission failure: one of the above queues may already
            // own the semaphore or command buffers. Drain the device before
            // the caller disposes of the failed encoder.
            let _ = unsafe { self.device.device_wait_idle() };
            unsafe {
                self.device.destroy_fence(fence, None);
                if let Some(producer_fence) = producer_fence {
                    self.device.destroy_fence(producer_fence, None);
                }
                self.device.destroy_semaphore(second, None);
                self.device.destroy_semaphore(first, None);
            }
            return Err(error);
        }
        let mut encoder_resource = self.command_encoders.take(encoder)?;
        encoder_resource.fence = fence;
        encoder_resource.owns_fence = true;
        encoder_resource.cross_queue_semaphores = Some([first, second]);
        self.deferred_destroys
            .retire(0, DeferredResource::CommandEncoder(encoder_resource));
        self.submitted_frames = self.submitted_frames.saturating_add(1);
        // This fence is on the actual SECONDARY GPU queue, not the primary
        // consumer join. The native submission can be waited on without a
        // host stall; the deferred encoder retains the separate join fence.
        Ok(producer_fence.map(|native_fence| {
            self.submissions.insert(VulkanSubmission {
                fence: native_fence,
                queue: queue_id,
                owned_by_submission: true,
                timeline_value: self.secondary_timeline.as_ref().map(|timeline| timeline.last()),
            })
        }))
    }

    fn submit_deferred(&mut self, encoder: CommandEncoderId) -> Result<SubmissionId> {
        self.submit_encoder(encoder)
    }

    fn submission_is_complete(&self, record: &VulkanSubmission) -> Result<bool> {
        if record.fence != vk::Fence::null() {
            // SAFETY: the owner holds the live fence, including any WSI slot.
            return unsafe { self.device.get_fence_status(record.fence) }
                .map_err(|error| VulkanError::from(error).into());
        }
        if let Some(value) = record.timeline_value {
            let timeline = self.timeline_for_queue(record.queue).ok_or_else(|| {
                Error::Backend("Vulkan submission lost its queue timeline".into())
            })?;
            return timeline.is_complete(value);
        }
        // A recycled, successful encoder has neither a fence nor a pending
        // timeline point. Device-loss retirement never creates this state.
        Ok(true)
    }

    fn poll_submission(&mut self, submission: SubmissionId) -> Result<SubmissionStatus> {
        self.poll_cleanup();
        let submission_resource = self.submissions.get(submission)?;
        let complete = self.submission_is_complete(submission_resource)?;
        if complete {
            // The first cleanup snapshot may precede the completion query.
            // Collect encoder timestamps while the public ID is still live.
            self.poll_cleanup();
            self.submission_wakes.cancel(submission);
            let completed = self.submissions.take(submission)?;
            if completed.owned_by_submission {
                // SAFETY: A checkpoint owns its own fence and the GPU has
                // signaled it; no command encoder or WSI image references it.
                unsafe { self.device.destroy_fence(completed.fence, None) };
            }
            Ok(SubmissionStatus::Complete)
        } else {
            Ok(SubmissionStatus::Pending)
        }
    }

    fn wait_submission(&mut self, submission: SubmissionId) -> Result<()> {
        let record = self.submissions.get(submission)?;
        let fence = record.fence;
        // SAFETY: Fence belongs to this device and remains live until its owning resource is destroyed.
        if fence != vk::Fence::null() {
            unsafe { self.device.wait_for_fences(&[fence], true, u64::MAX) }
                .map_err(VulkanError::from)?;
        } else if let Some(value) = record.timeline_value {
            let timeline = self.timeline_for_queue(record.queue).ok_or_else(|| {
                Error::Backend("Vulkan submission lost its queue timeline".into())
            })?;
            timeline.wait_point(value).wait().map_err(VulkanError::from)?;
        }
        self.submission_wakes.cancel(submission);
        let completed = self.submissions.take(submission)?;
        if completed.owned_by_submission {
            // SAFETY: wait_for_fences returned success for this owned fence.
            unsafe { self.device.destroy_fence(completed.fence, None) };
        }
        self.poll_cleanup();
        Ok(())
    }

    // Offscreen blur/path passes may already have been submitted before WSI acquisition.
    // A skipped present has no swapchain fence in the caller's in-flight slot table, so let
    // those submissions finish before the caller may reuse or overwrite their buffers.
    // This slow path is only used after bounded acquisition timeouts.
    fn drain_unpresented_graphics_work(&mut self) -> Result<()> {
        // The legacy caller has already issued offscreen work but has no
        // presented frame ID to retain its buffers. Wait for the latest
        // resource-bearing epoch, not WSI scanout or unrelated queue work.
        if let Some(timeline) = &self.primary_timeline {
            let value = timeline.last();
            if !timeline.is_complete(value)? {
                timeline.wait_point(value).wait().map_err(VulkanError::from)?;
            }
        } else {
            // SAFETY: compatibility path without the primary timeline.
            unsafe { self.device.queue_wait_idle(self.graphics_queue) }.map_err(VulkanError::from)?;
        }
        self.poll_cleanup();
        Ok(())
    }

    /// Reserve one drawable before the caller packs or submits offscreen work.
    /// False never drains the graphics queue. The successful reservation is
    /// consumed by render_and_present, or retired with this swapchain.
    ///
    /// # Errors
    /// Returns an error if WSI or the device rejects the acquisition.
    pub fn reserve_present_frame(&mut self, id: gfx_core::SwapchainId) -> Result<bool> {
        if let Some(frame) = self.swapchains.get(id)?.reserved_frame {
            return self.reserved_frame_ready(frame);
        }
        // Enforce the current native WSI credit even when callers bypass
        // the renderer's earlier readiness probe.
        if !PresentationDevice::poll_present_readiness(self, id)? {
            return Ok(false);
        }
        let Some(frame) = self.acquire_present_frame(id, true)? else {
            return Ok(false);
        };
        self.swapchains.get_mut(id)?.reserved_frame = Some(frame);
        Ok(true)
    }

    fn reserved_frame_ready(&self, frame: VulkanPresentFrame) -> Result<bool> {
        if !frame.recovery_pending {
            return Ok(true);
        }
        // SAFETY: the swapchain owns the fence; no reset occurs here.
        unsafe { self.device.get_fence_status(frame.fence) }
            .map_err(|error| VulkanError::from(error).into())
    }

    fn acquire_present_frame(
        &mut self,
        swapchain_id: gfx_core::SwapchainId,
        preflight: bool,
    ) -> Result<Option<VulkanPresentFrame>> {
        let (swapchain, frame_index, image_available, fence) = {
            let swapchain = self.swapchains.get(swapchain_id)?;
            // A zero-sized native surface cannot acquire drawable Vulkan images.
            if swapchain.extent.width == 0 || swapchain.extent.height == 0 {
                // Visibility can change without another logical window resize.
                // Probe only while suspended, then rebuild when WSI becomes drawable.
                let surface = swapchain.surface;
                let config = swapchain.config;
                // SAFETY: This surface belongs to our live Vulkan instance and device.
                let capabilities = unsafe {
                    self.surface_loader
                        .get_physical_device_surface_capabilities(self.physical_device, surface)
                }
                .map_err(VulkanError::from)?;
                let extent =
                    choose_extent(&capabilities, config.size.width(), config.size.height());
                if !preflight {
                    self.drain_unpresented_graphics_work()?;
                }
                if extent.width != 0 && extent.height != 0 {
                    self.reconfigure_swapchain(swapchain_id, config)?;
                }
                return Ok(None);
            }
            let frame_index = swapchain.frame_index;
            (
                swapchain.swapchain,
                frame_index,
                swapchain.image_available_semaphores[frame_index],
                swapchain.in_flight_fences[frame_index],
            )
        };
        // SAFETY: Fence belongs to this device and is not destroyed until swapchain destroy.
        let fence_wait_started = Instant::now();
        if !unsafe { self.device.get_fence_status(fence) }.map_err(VulkanError::from)? {
            if preflight {
                return Ok(None);
            }
            // Do not discard a complete compositor frame on a transient frame-slot
            // fence miss. A bounded wait absorbs a normal display refresh interval.
            match unsafe {
                self.device
                    .wait_for_fences(&[fence], true, self.present_timeout_ns)
            } {
                Ok(()) => {}
                Err(vk::Result::TIMEOUT) => {
                    self.drain_unpresented_graphics_work()?;
                    return Ok(None);
                }
                Err(error) => return Err(VulkanError::from(error).into()),
            }
        }
        let acquire_fence_wait = fence_wait_started.elapsed();
        // Keep the logical completion handle valid across native fence reuse.
        // Encoder recycling detaches completed frame IDs from their fence before
        // create_command_encoder returns and record_and_present resets the slot.
        // An early acquisition needs its own completion proof: an image may
        // have been acquired but never submitted if scene preparation fails.
        // Allocate once per swapchain, not once per frame.
        let acquire_fence = {
            let chain = self.swapchains.get_mut(swapchain_id)?;
            // An acquisition belongs to the physical frame slot whose
            // binary semaphore it signals, not to the entire swapchain.
            let slot = &mut chain.acquire_slots[frame_index];
            if slot.fence == vk::Fence::null() {
                slot.fence = create_fence(&self.device, false)?;
            }
            if slot.pending {
                if !unsafe { self.device.get_fence_status(slot.fence) }
                    .map_err(VulkanError::from)?
                {
                    if !preflight {
                        self.drain_unpresented_graphics_work()?;
                    }
                    return Ok(None);
                }
                unsafe { self.device.reset_fences(&[slot.fence]) }
                    .map_err(VulkanError::from)?;
                slot.pending = false;
            }
            slot.fence
        };
        // SAFETY: Swapchain and semaphore belong to this device and are valid here.
        let image_acquire_started = Instant::now();
        let acquire_result = unsafe {
            self.swapchain_loader.acquire_next_image(
                swapchain,
                if preflight { 0 } else { self.present_timeout_ns },
                image_available,
                acquire_fence,
            )
        };
        if acquire_result.is_ok() {
            self.swapchains.get_mut(swapchain_id)?.acquire_slots[frame_index].pending = true;
        }
        let image_acquire = image_acquire_started.elapsed();
        let image_index = match acquire_result {
            Ok((image_index, false)) => image_index,
            // SUBOPTIMAL still returns an acquired image and signals the
            // semaphore. Consume it normally; present handles recreation.
            Ok((image_index, true)) => image_index,
            // No image was acquired and the semaphore is unchanged. Keep this frame slot for
            // the next native presentation request; only submission resets its fence.
            Err(vk::Result::NOT_READY | vk::Result::TIMEOUT) => {
                // Only the legacy late-acquire contract can have untracked
                // offscreen work at this point. Preflight has not submitted it.
                if !preflight {
                    self.drain_unpresented_graphics_work()?;
                }
                return Ok(None);
            }
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                if !preflight {
                    self.drain_unpresented_graphics_work()?;
                }
                self.reconfigure_outdated_swapchain(swapchain_id)?;
                return Err(Error::SurfaceOutdated);
            }
            Err(error) => return Err(VulkanError::from(error).into()),
        };
        // Reacquiring this image and waiting on image_available in the submit orders the
        // next signal after presentation has consumed this image's previous wait semaphore.
        // A graphics frame fence alone does not establish that presentation dependency.
        let render_finished = self
            .swapchains
            .get(swapchain_id)?
            .render_finished_semaphores[image_index as usize];
        Ok(Some(VulkanPresentFrame {
            image_index,
            frame_index,
            image_available,
            render_finished,
            fence,
            recovery_pending: false,
            acquire_fence_wait,
            image_acquire,
        }))
    }

    fn destroy_temporary_command_encoder_now(&mut self, encoder: CommandEncoderId) -> Result<()> {
        let encoder = self.command_encoders.take(encoder)?;
        self.destroy_command_encoder_now(&encoder);
        Ok(())
    }

    fn signal_frame_fence_after_submit_failure(&self, fence: vk::Fence) -> Result<()> {
        submit::binary(&self.device, self.graphics_queue, &[], &[], &[],
            vk::PipelineStageFlags2::ALL_COMMANDS, fence)
    }

    /// Presents a swapchain image.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when presentation fails.
    fn present(
        &mut self,
        swapchain_id: gfx_core::SwapchainId,
        image_index: u32,
        wait_semaphore: vk::Semaphore,
        damage: Option<ScissorRect>,
    ) -> Result<()> {
        let swapchain = self.swapchains.get(swapchain_id)?.swapchain;
        let wait_semaphores = [wait_semaphore];
        let swapchains = [swapchain];
        let image_indices = [image_index];
        let mut present_info = vk::PresentInfoKHR::default()
            .wait_semaphores(&wait_semaphores)
            .swapchains(&swapchains)
            .image_indices(&image_indices);
        // Identify actual display samples without requesting a second pacing deadline.
        let chain = self.swapchains.get_mut(swapchain_id)?;
        let present_id = chain.present_credit.as_mut().map(PresentCredit::next_id);
        let ids = [present_id.unwrap_or(0)];
        let mut present_id_info = vk::PresentIdKHR::default().present_ids(&ids);
        if present_id.is_some() {
            present_info = present_info.push_next(&mut present_id_info);
        }
        let present_times = chain.display_timing_enabled.then(|| {
            let id = chain.next_present_id;
            chain.next_present_id = id.wrapping_add(1).max(1);
            [vk::PresentTimeGOOGLE::default().present_id(id).desired_present_time(0)]
        });
        let mut present_timing = present_times.as_ref()
            .map(|times| vk::PresentTimesInfoGOOGLE::default().times(times));
        if let Some(timing) = present_timing.as_mut() {
            present_info = present_info.push_next(timing);
        }
        let rectangles = if let Some(damage) = damage.filter(|_| self.incremental_presentation) {
            Some([Self::damage_to_present_rect(damage)?])
        } else {
            None
        };
        let regions = rectangles
            .as_ref()
            .map(|rectangles| [vk::PresentRegionKHR::default().rectangles(rectangles)]);
        let mut present_regions = regions
            .as_ref()
            .map(|regions| vk::PresentRegionsKHR::default().regions(regions));
        if let Some(present_regions) = present_regions.as_mut() {
            present_info = present_info.push_next(present_regions);
        }
        let present_fence = if self.swapchain_maintenance {
            Some(
                self.swapchains
                    .get_mut(swapchain_id)?
                    .presentation
                    .acquire(&self.device)?,
            )
        } else {
            None
        };
        let fences = [present_fence.unwrap_or(vk::Fence::null())];
        let mut fence_info = vk::SwapchainPresentFenceInfoEXT::default().fences(&fences);
        if present_fence.is_some() {
            present_info = present_info.push_next(&mut fence_info);
        }
        // SAFETY: Present queue and swapchain are valid and synchronized by wait_semaphore.
        let present_result = unsafe {
            self.swapchain_loader
                .queue_present(self.present_queue, &present_info)
        };
        if let Some(fence) = present_fence {
            self.swapchains
                .get_mut(swapchain_id)?
                .presentation
                .record(fence, present_result);
        }
        if present_result.is_ok()
            && let Some(id) = present_id
            && let Some(credit) = self.swapchains.get_mut(swapchain_id)?.present_credit.as_mut()
        {
            credit.submitted(id);
        }
        match present_result {
            Ok(false) => Ok(()),
            Ok(true) | Err(vk::Result::SUBOPTIMAL_KHR)
                if self.swapchains.get(swapchain_id)?.native_pixel_scaling =>
            {
                Ok(())
            }
            Ok(true) | Err(vk::Result::ERROR_OUT_OF_DATE_KHR | vk::Result::SUBOPTIMAL_KHR) => {
                self.reconfigure_outdated_swapchain(swapchain_id)?;
                Err(Error::SurfaceOutdated)
            }
            Err(error) => Err(VulkanError::from(error).into()),
        }
    }

    fn reconfigure_outdated_swapchain(
        &mut self,
        swapchain_id: gfx_core::SwapchainId,
    ) -> Result<()> {
        // Reconfiguration owns the idle barrier and old-fence retirement. Do not
        // wait for device idle a second time in outdated-surface recovery.
        let config = self.swapchains.get(swapchain_id)?.config;
        self.reconfigure_swapchain(swapchain_id, config)
    }

    fn damage_to_present_rect(damage: ScissorRect) -> Result<vk::RectLayerKHR> {
        let x = i32::try_from(damage.x)
            .map_err(|error| Error::InvalidInput(format!("present damage x overflow: {error}")))?;
        let y = i32::try_from(damage.y)
            .map_err(|error| Error::InvalidInput(format!("present damage y overflow: {error}")))?;
        Ok(vk::RectLayerKHR::default()
            .offset(vk::Offset2D { x, y })
            .extent(vk::Extent2D {
                width: damage.width,
                height: damage.height,
            })
            .layer(0))
    }

    /// Destroys a buffer.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the handle is invalid or memory free fails.
    fn destroy_buffer(&mut self, buffer_id: BufferId) -> Result<()> {
        self.buffers.get(buffer_id)?;
        let retirement = self.signal_cleanup_point()?;
        let buffer = self.buffers.take(buffer_id)?;
        self.deferred_destroys
            .retire(0, DeferredResource::Buffer { retirement, buffer });
        self.poll_cleanup();
        Ok(())
    }

    fn destroy_buffer_batch(&mut self, ids: &[BufferId]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        if ids.len() == 1 {
            return self.destroy_buffer(ids[0]);
        }
        let mut unique = std::collections::HashSet::with_capacity(ids.len());
        for &id in ids {
            self.buffers.get(id)?;
            if !unique.insert(id) {
                return Err(Error::InvalidInput(
                    "duplicate buffer retirement handle".into(),
                ));
            }
        }
        let retirement = self.signal_cleanup_point()?;
        let buffers = ids
            .iter()
            .map(|&id| self.buffers.take(id))
            .collect::<Result<Vec<_>>>()?;
        self.deferred_destroys
            .retire(0, DeferredResource::Buffers { retirement, buffers });
        self.poll_cleanup();
        Ok(())
    }

    /// Destroys a texture.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the handle is invalid.
    fn destroy_texture(&mut self, texture_id: TextureId) -> Result<()> {
        self.textures.get(texture_id)?;
        let retirement = self.signal_cleanup_point()?;
        let texture = self.textures.take(texture_id)?;
        self.deferred_destroys
            .retire(0, DeferredResource::Texture { retirement, texture });
        self.poll_cleanup();
        Ok(())
    }

    /// Destroys a texture view.
    ///
    /// Invalidates the handle immediately. The native view is retained until prior graphics
    /// submissions complete; this does not wait for work from other windows on the device.
    /// Recorded command encoders referencing this view must be submitted before destruction.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the handle is invalid or the cleanup fence cannot be queued.
    fn destroy_texture_view(&mut self, view_id: TextureViewId) -> Result<()> {
        self.texture_views.get(view_id)?;
        let retirement = self.signal_cleanup_point()?;
        let view = self.texture_views.take(view_id)?;
        self.deferred_destroys
            .retire(0, DeferredResource::TextureView { retirement, view });
        self.poll_cleanup();
        Ok(())
    }

    /// Destroys a sampler.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the handle is invalid.
    fn destroy_sampler(&mut self, sampler_id: SamplerId) -> Result<()> {
        self.wait_for_pending_work()?;
        let sampler = self.samplers.take(sampler_id)?;
        self.destroy_sampler_now(sampler);
        Ok(())
    }

    /// Destroys a bind group layout.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the handle is invalid.
    fn destroy_bind_group_layout(&mut self, layout_id: BindGroupLayoutId) -> Result<()> {
        self.wait_for_pending_work()?;
        let layout = self.bind_group_layouts.take(layout_id)?;
        self.destroy_resource_set_layout_now(&layout);
        Ok(())
    }

    /// Destroys a bind group.
    ///
    /// Invalidates the handle immediately and frees its native descriptor set only after prior
    /// graphics submissions complete. The shared descriptor pool remains owned by this device.
    /// Recorded command encoders referencing this set must be submitted before destruction.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the handle is invalid or the cleanup fence cannot be queued.
    fn destroy_bind_group(&mut self, set_id: BindGroupId) -> Result<()> {
        self.bind_groups.get(set_id)?;
        let retirement = self.signal_cleanup_point()?;
        let set = self.bind_groups.take(set_id)?;
        self.deferred_destroys
            .retire(0, DeferredResource::BindGroup { retirement, set });
        self.poll_cleanup();
        Ok(())
    }

    /// Destroys a pipeline layout.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the handle is invalid.
    fn destroy_pipeline_layout(&mut self, layout_id: PipelineLayoutId) -> Result<()> {
        self.wait_for_pending_work()?;
        let layout = self.pipeline_layouts.take(layout_id)?;
        self.destroy_pipeline_layout_now(&layout);
        Ok(())
    }

    /// Destroys a shader module.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the handle is invalid.
    fn destroy_shader_module(&mut self, shader_id: ShaderModuleId) -> Result<()> {
        self.wait_for_pending_work()?;
        let shader = self.shader_modules.take(shader_id)?;
        self.compute.remove_shader(shader_id);
        self.destroy_shader_module_now(&shader);
        Ok(())
    }

    /// Destroys a render pass.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the handle is invalid.
    fn destroy_render_pass(&mut self, render_pass_id: RenderPassId) -> Result<()> {
        // Dynamic rendering pass IDs hold only immutable CPU metadata.
        self.render_passes.take(render_pass_id)?;
        Ok(())
    }

    /// Destroys a render pipeline.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the handle is invalid.
    fn destroy_render_pipeline(&mut self, pipeline_id: RenderPipelineId) -> Result<()> {
        self.wait_for_pending_work()?;
        let pipeline = self.render_pipelines.take(pipeline_id)?;
        self.destroy_render_pipeline_now(&pipeline);
        Ok(())
    }

    /// Destroys a command encoder.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the handle is invalid.
    fn destroy_command_encoder(&mut self, encoder_id: CommandEncoderId) -> Result<()> {
        self.wait_for_pending_work()?;
        let encoder = self.command_encoders.take(encoder_id)?;
        self.destroy_command_encoder_now(&encoder);
        Ok(())
    }

    /// Destroys a swapchain.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the handle is invalid.
    fn destroy_swapchain(&mut self, swapchain_id: gfx_core::SwapchainId) -> Result<()> {
        self.retire_swapchain(swapchain_id)?;
        let swapchain = self.swapchains.take(swapchain_id)?;
        self.destroy_swapchain_now(swapchain);
        Ok(())
    }

    /// Destroys a surface.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when the handle is invalid.
    fn destroy_surface(&mut self, surface_id: SurfaceId) -> Result<()> {
        let surface = self.surfaces.take(surface_id)?;
        self.destroy_surface_now(surface);
        Ok(())
    }

    /// Polls and releases deferred resources that have aged past their retirement frame.
    pub fn poll_cleanup(&mut self) {
        let device = self.device.clone();
        self.submission_wakes.collect(&device);
        // The secondary queue's native fence is the only proof that the
        // binary wait semaphore has been consumed. Do not free it when the
        // producing graphics queue signals, or when a host submit returns.
        self.native_queue_waits.retain(|bridge| {
            if fence_is_complete(&device, bridge.wait_fence) {
                // SAFETY: The wait submission signaled this fence, therefore
                // both the queued semaphore wait and its source signal are
                // finished. No other queue references these objects.
                unsafe {
                    device.destroy_fence(bridge.wait_fence, None);
                    device.destroy_semaphore(bridge.semaphore, None);
                }
                false
            } else {
                true
            }
        });
        // One counter snapshot per retirement pass, not one driver query
        // per outstanding encoder. Failed queries never prove retirement.
        let primary = self.primary_timeline.as_ref();
        let mut completed = None;
        for resource in self
            .deferred_destroys
            .collect_ready(|_, resource| {
                if let Some(value) = resource.primary_retirement_value() {
                    let snapshot = completed.get_or_insert_with(|| {
                        primary.and_then(|timeline| timeline.completed_value().ok())
                    });
                    return snapshot.is_some_and(|completed| completed >= value);
                }
                deferred_resource_ready(&device, resource)
            })
        {
            let _destroy_result = self.destroy_deferred_now(resource);
        }
    }

    /// Returns live resource and allocator statistics.
    #[must_use]
    fn resource_stats(&self) -> ResourceStats {
        let memory = self.allocator.detailed_report();
        ResourceStats {
            memory_accounting: MemoryAccounting::Allocator,
            upload_used_bytes: self.upload_ring.stats().used_bytes,
            upload_capacity_bytes: self.upload_ring.stats().reserved_bytes,
            buffers: self.buffers.live_len(),
            textures: self.textures.live_len(),
            texture_views: self.texture_views.live_len(),
            samplers: self.samplers.live_len(),
            bind_group_layouts: self.bind_group_layouts.live_len(),
            bind_groups: self.bind_groups.live_len(),
            pipeline_layouts: self.pipeline_layouts.live_len(),
            shader_modules: self.shader_modules.live_len(),
            render_passes: self.render_passes.live_len(),
            render_pipelines: self.render_pipelines.live_len(),
            command_encoders: self.command_encoders.live_len(),
            submissions: self.submissions.live_len(),
            surfaces: self.surfaces.live_len(),
            swapchains: self.swapchains.live_len(),
            allocated_bytes: memory.allocated_bytes,
            reserved_bytes: memory.reserved_bytes,
            ..ResourceStats::default()
        }
    }

    fn build_swapchain(
        &mut self,
        surface: vk::SurfaceKHR,
        config: SurfaceConfig,
    ) -> Result<VulkanSwapchain> {
        self.build_swapchain_replacing(surface, config, vk::SwapchainKHR::null())
    }

    fn build_swapchain_replacing(
        &mut self,
        surface: vk::SurfaceKHR,
        config: SurfaceConfig,
        old_swapchain: vk::SwapchainKHR,
    ) -> Result<VulkanSwapchain> {
        let started_at = Instant::now();
        let support = query_swapchain_support(self.physical_device, &self.surface_loader, surface)?;
        let surface_format = choose_surface_format(&support.formats, config.format);
        let present_mode = choose_present_mode(&support.present_modes, config.present_mode);
        let requested_extent = vk::Extent2D {
            width: config.size.width(),
            height: config.size.height(),
        };
        let mut scaling = self
            .native_pixel_presentation
            .as_ref()
            .map(|loader| {
                presentation::scaling(
                    loader,
                    self.physical_device,
                    surface,
                    present_mode,
                    requested_extent,
                )
            })
            .transpose()?
            .flatten();
        let native_pixel_scaling = scaling.is_some();
        let scaled_image_extent_range = scaling
            .as_ref()
            .map(|scaling| (scaling.minimum, scaling.maximum));
        let extent = if native_pixel_scaling {
            requested_extent
        } else {
            choose_extent(
                &support.capabilities,
                config.size.width(),
                config.size.height(),
            )
        };
        let default_image_count = support.capabilities.min_image_count.saturating_add(1);
        let maximum_image_count = if support.capabilities.max_image_count == 0 {
            u32::MAX
        } else {
            support.capabilities.max_image_count
        };
        let image_count = config.resolve_image_count(
            default_image_count,
            support.capabilities.min_image_count.max(1),
            maximum_image_count,
        );
        let frame_slots =
            config.resolve_frame_latency(DEFAULT_FRAME_SLOTS, 1, image_count.clamp(1, 8)) as usize;

        let queue_family_indices = [
            self.graphics_queue_family_index,
            self.present_queue_family_index,
        ];
        let (sharing_mode, queue_family_indices) =
            if self.graphics_queue_family_index == self.present_queue_family_index {
                (vk::SharingMode::EXCLUSIVE, &[][..])
            } else {
                (vk::SharingMode::CONCURRENT, &queue_family_indices[..])
            };
        let alpha_mode = choose_composite_alpha_mode(
            support.capabilities.supported_composite_alpha,
            config.alpha_mode,
        )?;
        let mut create_info = vk::SwapchainCreateInfoKHR::default()
            .surface(surface)
            .min_image_count(image_count)
            .image_format(surface_format.format)
            .image_color_space(surface_format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            .image_sharing_mode(sharing_mode)
            .queue_family_indices(queue_family_indices)
            .pre_transform(support.capabilities.current_transform)
            .composite_alpha(composite_alpha_to_vulkan(alpha_mode))
            .present_mode(present_mode)
            .clipped(true)
            .old_swapchain(old_swapchain);
        if let Some(scaling) = &mut scaling {
            create_info = create_info.push_next(&mut scaling.info);
        }

        let support_elapsed = started_at.elapsed();
        let driver_started_at = Instant::now();
        // SAFETY: All inputs are valid for the selected physical device and surface.
        let swapchain = unsafe { self.swapchain_loader.create_swapchain(&create_info, None) }
            .map_err(VulkanError::from)?;
        let driver_elapsed = driver_started_at.elapsed();
        let resources_started_at = Instant::now();
        // SAFETY: Swapchain belongs to this device and is valid.
        let images = unsafe { self.swapchain_loader.get_swapchain_images(swapchain) }
            .map_err(VulkanError::from)?;
        let image_views = images
            .iter()
            .copied()
            .map(|image| {
                create_image_view(
                    &self.device,
                    image,
                    surface_format.format,
                    vk::ImageAspectFlags::COLOR,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        let (image_available_semaphores, render_finished_semaphores, in_flight_fences) =
            create_sync_objects(&self.device, images.len(), frame_slots)?;
        log::debug!(
            "nova-gfx Vulkan swapchain initialization: total_ms={} support_ms={} native_swapchain_ms={} image_resources_ms={} requested_images={} images={} format={:?} present_mode={:?} alpha={:?} extent={}x{}",
            started_at.elapsed().as_millis(),
            support_elapsed.as_millis(),
            driver_elapsed.as_millis(),
            resources_started_at.elapsed().as_millis(),
            image_count,
            images.len(),
            surface_format.format,
            present_mode,
            alpha_mode,
            extent.width,
            extent.height,
        );
        Ok(VulkanSwapchain {
            native_pixel_scaling,
            scaled_image_extent_range,
            surface,
            swapchain,
            format: surface_format.format,
            extent,
            source_extent: extent,
            images,
            image_views,
            image_available_semaphores,
            render_finished_semaphores,
            frame_submissions: vec![None; in_flight_fences.len()],
            acquire_slots: vec![presentation::AcquireSlot::default(); in_flight_fences.len()],
            frame_probe: FrameProbeCadence::default(),
            in_flight_fences,
            presentation: presentation::Retirement::default(),
            present_wake: None,
            next_present_id: 1,
            display_timing_enabled: self.display_timing.is_some()
                && present_mode == vk::PresentModeKHR::FIFO,
            present_credit: (self.present_wait.is_some() && present_mode == vk::PresentModeKHR::FIFO)
                .then(|| PresentCredit::new(frame_slots)),
            frame_index: 0,
            reserved_frame: None,
            config,
        })
    }

    fn submit_command_buffer(
        &self,
        command_buffer: vk::CommandBuffer,
        wait_semaphores: &[vk::Semaphore],
        signal_semaphores: &[vk::Semaphore],
        fence: vk::Fence,
    ) -> Result<()> {
        self.submit_command_buffer_timed(command_buffer, wait_semaphores, signal_semaphores, fence)
            .map(|_| ())
    }

    fn submit_command_buffers(
        &self,
        command_buffers: &[vk::CommandBuffer],
        wait_semaphores: &[vk::Semaphore],
        signal_semaphores: &[vk::Semaphore],
        fence: vk::Fence,
    ) -> Result<()> {
        if let Some(timeline) = &self.primary_timeline {
            return timeline.submit(&self.device, self.graphics_queue, command_buffers,
                wait_semaphores, signal_semaphores, vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT, fence);
        }
        submit::binary(&self.device, self.graphics_queue, command_buffers, wait_semaphores,
            signal_semaphores, vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT, fence)
    }

    fn submit_command_buffer_timed(
        &self,
        command_buffer: vk::CommandBuffer,
        wait_semaphores: &[vk::Semaphore],
        signal_semaphores: &[vk::Semaphore],
        fence: vk::Fence,
    ) -> Result<Duration> {
        let submit_started = Instant::now();
        self.submit_command_buffers(&[command_buffer], wait_semaphores, signal_semaphores, fence)?;
        Ok(submit_started.elapsed())
    }

    fn trim_upload_pages(&mut self, retained_page_count: usize) -> Result<()> {
        let released_pages = self.upload_pages.split_off(retained_page_count);
        let mut first_error = None;
        for page in released_pages.into_iter().flatten() {
            if let Err(error) = self.destroy_buffer_now(page) {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    fn trim_upload_command_pool(&mut self, retained_count: usize) {
        if self.upload_command_pool.len() <= retained_count {
            return;
        }
        let released = self.upload_command_pool.split_off(retained_count);
        for commands in released {
            destroy_upload_commands(&self.device, &commands);
        }
    }

    /// Releases completed staging/upload caches without touching live Vulkan resources.
    /// Moderate and aggressive pressure release fully empty allocator blocks;
    /// live allocations stay put, including the moderate trim's retained upload page.
    pub fn trim_memory(&mut self, level: MemoryTrimLevel) -> Result<()> {
        if matches!(level, MemoryTrimLevel::Light) {
            self.poll_cleanup();
            return Ok(());
        }

        self.wait_for_pending_work()?;
        let target_idle_pages = if matches!(level, MemoryTrimLevel::Moderate) {
            1
        } else {
            0
        };
        let mut retained_page_count = self.upload_ring.trim_idle_pages_to(target_idle_pages);
        if retained_page_count == 1
            && self
                .upload_ring
                .page_size(0)
                .is_some_and(|size| size > self.upload_ring.configured_page_size())
        {
            retained_page_count = self.upload_ring.trim_idle_pages_to(0);
        }
        self.trim_upload_pages(retained_page_count)?;

        let retained_upload_commands = if matches!(level, MemoryTrimLevel::Moderate) {
            1
        } else {
            0
        };
        self.trim_upload_command_pool(retained_upload_commands);
        if matches!(level, MemoryTrimLevel::Aggressive) {
            self.upload_pages.shrink_to_fit();
            self.upload_command_pool.shrink_to_fit();
        }
        self.allocator.trim();
        Ok(())
    }

    fn retire_deferred_upload(&mut self, commands: VulkanUploadCommands, retirement: CleanupPoint) {
        self.enqueue_deferred_upload(commands, retirement);
        self.poll_cleanup();
    }

    fn enqueue_deferred_upload(&mut self, commands: VulkanUploadCommands, retirement: CleanupPoint) {
        let fence_value = self.next_upload_fence_value;
        self.next_upload_fence_value = self.next_upload_fence_value.saturating_add(1);
        self.upload_ring.retire_used_pages(fence_value);
        self.deferred_destroys.retire(
            0,
            DeferredResource::Upload {
                retirement,
                fence_value,
                commands,
            },
        );
    }

    // Uploads and relocation copies are resource-bearing graphics work.
    // Their submission itself signals the queue timeline, not a later checkpoint.
    fn end_submit_upload(&self, command_buffer: vk::CommandBuffer) -> Result<CleanupPoint> {
        if let Some(timeline) = &self.primary_timeline {
            // SAFETY: this owner holds the recording command buffer.
            unsafe { self.device.end_command_buffer(command_buffer) }.map_err(VulkanError::from)?;
            timeline.submit(&self.device, self.graphics_queue, &[command_buffer], &[], &[],
                vk::PipelineStageFlags2::ALL_COMMANDS, vk::Fence::null())?;
            return Ok(CleanupPoint::Timeline(timeline.last()));
        }
        end_submit_deferred(&self.device, self.graphics_queue, command_buffer, None)
            .map(CleanupPoint::Fence)
    }

    fn signal_cleanup_point(&self) -> Result<CleanupPoint> {
        if let Some(timeline) = &self.primary_timeline {
            // Every resource-bearing primary submit (including uploads,
            // readback, relocation and secondary consumer joins) now signals
            // this timeline. No new empty submit is needed for each resource.
            // Zero is the initial completed epoch before any GPU work.
            return Ok(CleanupPoint::Timeline(timeline.last()));
        }
        let fence = create_fence(&self.device, false)?;
        if let Err(error) = submit::binary(&self.device, self.graphics_queue, &[], &[], &[],
            vk::PipelineStageFlags2::ALL_COMMANDS, fence) {
            // SAFETY: the failed submission did not associate the fence with GPU work.
            unsafe { self.device.destroy_fence(fence, None) };
            return Err(error);
        }
        Ok(CleanupPoint::Fence(fence))
    }

    fn write_upload_data(&mut self, data: &[u8]) -> Result<UploadAllocation> {
        let size = u64::try_from(data.len())
            .map_err(|error| Error::InvalidInput(format!("upload size overflow: {error}")))?;
        let allocation = self.upload_ring.allocate(size)?;
        self.stage_upload_data(data, allocation)
    }

    fn stage_upload_data(
        &mut self,
        data: &[u8],
        allocation: UploadAllocation,
    ) -> Result<UploadAllocation> {
        self.ensure_upload_page(allocation.page_index)?;
        let page = self
            .upload_pages
            .get_mut(allocation.page_index)
            .and_then(Option::as_mut)
            .ok_or_else(|| Error::Backend("missing Vulkan upload page".to_string()))?;
        let mapped = page
            .allocation
            .mapped_slice_mut()
            .ok_or_else(|| Error::Backend("upload page memory is not CPU visible".to_string()))?;
        let offset = usize::try_from(allocation.offset)
            .map_err(|error| Error::InvalidInput(format!("upload offset overflow: {error}")))?;
        let end = offset
            .checked_add(data.len())
            .ok_or_else(|| Error::InvalidInput("upload range overflow".to_string()))?;
        let target = mapped.get_mut(offset..end).ok_or_else(|| {
            Error::InvalidInput("upload range is out of upload page bounds".to_string())
        })?;
        target.copy_from_slice(data);
        Ok(allocation)
    }

    fn ensure_upload_page(&mut self, page_index: usize) -> Result<()> {
        let size = self
            .upload_ring
            .page_size(page_index)
            .ok_or_else(|| Error::Backend(format!("upload ring page {page_index} has no size")))?;
        while self.upload_pages.len() <= page_index {
            self.upload_pages.push(None);
        }
        if let Some(page) = self.upload_pages[page_index].as_ref() {
            if page.desc.size >= size {
                return Ok(());
            }
        }
        if let Some(page) = self.upload_pages[page_index].take() {
            self.destroy_buffer_now(page)?;
        }
        let desc = BufferDescriptor {
            label: Some(format!("nova-gfx vulkan upload page {page_index}")),
            size,
            usage: BufferUsage::COPY_SRC,
            memory_location: MemoryLocation::CpuToGpu,
        };
        let buffer = self.create_buffer_unregistered(&desc)?;
        self.upload_pages[page_index] = Some(buffer);
        Ok(())
    }

    fn upload_page_buffer(&self, page_index: usize) -> Result<vk::Buffer> {
        self.upload_pages
            .get(page_index)
            .and_then(Option::as_ref)
            .map(|page| page.buffer)
            .ok_or_else(|| Error::Backend(format!("missing Vulkan upload page {page_index}")))
    }

    fn create_buffer_unregistered(&mut self, desc: &BufferDescriptor) -> Result<VulkanBuffer> {
        desc.validate()?;
        let create_info = vk::BufferCreateInfo::default()
            .size(desc.size)
            .usage(
                buffer_usage_to_vk(desc.usage)
                    | vk::BufferUsageFlags::TRANSFER_SRC
                    | vk::BufferUsageFlags::TRANSFER_DST,
            )
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: Device is valid and buffer create info is self-contained.
        let buffer =
            unsafe { self.device.create_buffer(&create_info, None) }.map_err(VulkanError::from)?;
        // SAFETY: Buffer was created by this device and is valid for requirements query.
        let requirements = unsafe { self.device.get_buffer_memory_requirements(buffer) };
        let name = desc.label.as_deref().unwrap_or("nova-gfx-upload-buffer");
        let allocation = self.allocator.allocate_vulkan_buffer(
            name,
            requirements,
            desc.memory_location,
            buffer,
        )?;
        let (memory, offset) = vulkan_memory(&allocation)?;
        // SAFETY: Allocation was created from this buffer's memory requirements and outlives it.
        unsafe { self.device.bind_buffer_memory(buffer, memory, offset) }
            .map_err(VulkanError::from)?;

        Ok(VulkanBuffer {
            buffer,
            allocation,
            desc: desc.clone(),
        })
    }

    #[cfg(test)]
    fn copy_buffer_once(
        &self,
        source: vk::Buffer,
        destination: vk::Buffer,
        source_offset: u64,
        destination_offset: u64,
        size: u64,
    ) -> Result<()> {
        let command_pool = create_command_pool(&self.device, self.graphics_queue_family_index)?;
        let command_buffer = allocate_command_buffers(&self.device, command_pool, 1)?
            .into_iter()
            .next()
            .ok_or_else(|| Error::Backend("failed to allocate command buffer".to_string()))?;
        begin_one_time_commands(&self.device, command_buffer)?;
        let region = vk::BufferCopy::default()
            .src_offset(source_offset)
            .dst_offset(destination_offset)
            .size(size);
        // SAFETY: Command buffer is recording and buffers support copy usage by descriptor.
        unsafe {
            self.device
                .cmd_copy_buffer(command_buffer, source, destination, &[region]);
        }
        end_submit_wait_destroy(
            &self.device,
            self.graphics_queue,
            command_pool,
            command_buffer,
            self.primary_timeline.as_ref(),
        )
    }

    fn copy_buffer_to_texture(
        &mut self,
        texture: TextureId,
        source: vk::Buffer,
        image: vk::Image,
        old_layout: vk::ImageLayout,
        mip_level: u32,
        mip_level_count: u32,
        layout: TextureDataLayout,
        origin: gfx_core::Origin2d,
        size: gfx_core::Extent2d,
    ) -> Result<()> {
        self.copy_buffers_to_textures(&[VulkanTextureUpload {
            texture,
            source,
            image,
            old_layout,
            mip_level_count,
            layout,
            mip_level,
            layer: 0,
            origin,
            size,
        }])
    }

    fn copy_buffers_to_textures(&mut self, uploads: &[VulkanTextureUpload]) -> Result<()> {
        if uploads.is_empty() {
            return Ok(());
        }
        let commands = match self.upload_command_pool.pop() {
            Some(commands) => {
                // SAFETY: Commands return to this pool only after their submission fence completes.
                unsafe {
                    self.device.reset_command_pool(
                        commands.command_pool,
                        vk::CommandPoolResetFlags::empty(),
                    )
                }
                .map_err(VulkanError::from)?;
                commands
            }
            None => create_upload_commands(
                &self.device,
                self.graphics_queue_family_index,
                self.timestamp_clock.is_some(),
            )?,
        };
        let command_buffer = commands.command_buffer;
        begin_one_time_commands(&self.device, command_buffer)?;
        if let Some(query_pool) = commands.timestamp_queries {
            // SAFETY: The command buffer is recording and the query pool has exactly two slots.
            unsafe {
                if self.host_query_reset {
                    // Upload pools are new or retired before being reused.
                    self.device.reset_query_pool(query_pool, 0, 2);
                } else {
                    self.device.cmd_reset_query_pool(command_buffer, query_pool, 0, 2);
                }
                self.device.cmd_write_timestamp2(
                    command_buffer,
                    vk::PipelineStageFlags2::TOP_OF_PIPE,
                    query_pool,
                    0,
                );
            }
        }
        let groups = texture_upload_groups(uploads);
        for group in groups {
            if let Err(error) = self.record_texture_upload_group(command_buffer, &group) {
                destroy_upload_commands(&self.device, &commands);
                return Err(error);
            }
        }
        if let Some(query_pool) = commands.timestamp_queries {
            // SAFETY: The command buffer is recording and query slot one is valid.
            unsafe {
                self.device.cmd_write_timestamp2(
                    command_buffer,
                    vk::PipelineStageFlags2::BOTTOM_OF_PIPE,
                    query_pool,
                    1,
                );
            }
        }
        let retirement = match self.end_submit_upload(command_buffer) {
            Ok(retirement) => retirement,
            Err(error) => {
                destroy_upload_commands(&self.device, &commands);
                return Err(error);
            }
        };
        self.retire_deferred_upload(commands, retirement);
        Ok(())
    }

    fn record_texture_upload_group(
        &self,
        command_buffer: vk::CommandBuffer,
        group: &VulkanTextureUploadGroup<'_>,
    ) -> Result<()> {
        let layers = self.textures.get(group.texture)?.desc.dimension.layers();
        transition_image_layout_levels(
            &self.device,
            command_buffer,
            group.image,
            group.old_layout,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            group.mip_level_count,
            layers,
            vk::ImageAspectFlags::COLOR,
        );
        let mut synchronized = 0;
        for (index, upload) in group.uploads.iter().enumerate() {
            if group.uploads[synchronized..index]
                .iter()
                .any(|previous| texture_uploads_overlap(previous, upload))
            {
                let barrier = vk::MemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
                    .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                    .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE);
                // SAFETY: Recording on the owning queue. Order overlapping copies without a host wait.
                unsafe {
                    self.device.cmd_pipeline_barrier2(
                command_buffer,
                &vk::DependencyInfo::default().dependency_flags(vk::DependencyFlags::empty())
                    .memory_barriers(&[barrier]).buffer_memory_barriers(&[]).image_memory_barriers(&[]),
            );
                }
                synchronized = index;
            }
            self.record_buffer_to_texture_copy(command_buffer, upload)?;
        }
        transition_image_layout_levels(
            &self.device,
            command_buffer,
            group.image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            group.mip_level_count,
            layers,
            vk::ImageAspectFlags::COLOR,
        );
        Ok(())
    }

    fn record_buffer_to_texture_copy(
        &self,
        command_buffer: vk::CommandBuffer,
        upload: &VulkanTextureUpload,
    ) -> Result<()> {
        let bytes_per_pixel = self
            .textures
            .get(upload.texture)?
            .desc
            .format
            .bytes_per_pixel();
        let region = vk::BufferImageCopy::default()
            .buffer_offset(upload.layout.offset)
            .buffer_row_length(upload.layout.bytes_per_row.get() / bytes_per_pixel)
            .buffer_image_height(upload.layout.rows_per_image.get())
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .mip_level(upload.mip_level)
                    .base_array_layer(upload.layer)
                    .layer_count(1),
            )
            .image_offset(vk::Offset3D {
                x: i32::try_from(upload.origin.x).map_err(|error| {
                    Error::InvalidInput(format!("texture upload origin x overflow: {error}"))
                })?,
                y: i32::try_from(upload.origin.y).map_err(|error| {
                    Error::InvalidInput(format!("texture upload origin y overflow: {error}"))
                })?,
                z: 0,
            })
            .image_extent(vk::Extent3D {
                width: upload.size.width(),
                height: upload.size.height(),
                depth: 1,
            });
        // SAFETY: Command buffer is recording, image is in transfer dst layout, and source
        // buffer is valid for the provided copy region.
        unsafe {
            self.device.cmd_copy_buffer_to_image(
                command_buffer,
                upload.source,
                upload.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[region],
            );
        }
        Ok(())
    }

    fn destroy_deferred_now(&mut self, resource: DeferredResource) -> Result<()> {
        match resource {
            DeferredResource::HeapReplacement {
                fence,
                commands,
                mut replacement,
            } => {
                destroy_fence(&self.device, fence);
                destroy_upload_commands(&self.device, &commands);
                self.discard_heap_replacement(&mut replacement)
            }
            DeferredResource::Buffer { retirement, buffer } => {
                retirement.destroy(&self.device);
                self.destroy_buffer_now(buffer)
            }
            DeferredResource::Buffers { retirement, buffers } => {
                retirement.destroy(&self.device);
                let mut result = Ok(());
                for buffer in buffers {
                    if let Err(error) = self.destroy_buffer_now(buffer) {
                        result = Err(error);
                    }
                }
                result
            }
            DeferredResource::Texture { retirement, texture } => {
                retirement.destroy(&self.device);
                self.destroy_texture_now(texture)
            }
            DeferredResource::TextureView { retirement, view } => {
                retirement.destroy(&self.device);
                self.destroy_texture_view_now(&view);
                Ok(())
            }
            DeferredResource::BindGroup { retirement, set } => {
                retirement.destroy(&self.device);
                self.destroy_resource_set_now(&set);
                Ok(())
            }
            DeferredResource::Upload {
                retirement,
                fence_value,
                commands,
            } => {
                if let (Some(query_pool), Some(timestamp_clock)) =
                    (commands.timestamp_queries, self.timestamp_clock)
                {
                    let mut ticks = [0_u64; 2];
                    // SAFETY: The owning submission fence completed before this deferred resource
                    // became ready, and both query slots belong to this device.
                    if unsafe {
                        self.device.get_query_pool_results(
                            query_pool,
                            0,
                            &mut ticks,
                            vk::QueryResultFlags::TYPE_64,
                        )
                    }
                    .is_ok()
                    {
                        if let Some(tag) = commands.gpu_work_tag {
                            self.gpu_work_samples.push(gfx_core::GpuTimestampSample {
                                tag,
                                duration_ticks: timestamp_clock.duration_ticks(ticks[0], ticks[1]),
                                frequency_hz: timestamp_clock.frequency_hz(),
                            });
                        } else {
                            let elapsed_ns =
                                timestamp_clock.elapsed_nanoseconds(ticks[0], ticks[1]);
                            self.last_texture_transfer_time =
                                Some(Duration::from_secs_f64(elapsed_ns / 1_000_000_000.0));
                        }
                    }
                }
                retirement.destroy(&self.device);
                let mut commands = commands;
                commands.gpu_work_tag = None;
                if self.upload_command_pool.len() < self.upload_command_pool_capacity {
                    self.upload_command_pool.push(commands);
                } else {
                    destroy_upload_commands(&self.device, &commands);
                }
                self.upload_ring.complete_fence(fence_value);
                let retained_page_count = self.upload_ring.trim_idle_pages();
                self.trim_upload_pages(retained_page_count)
            }
            DeferredResource::CommandEncoder(encoder) => self.recycle_command_encoder(encoder),
        }
    }

    fn wait_for_pending_work(&mut self) -> Result<()> {
        // SAFETY: Waiting for the device before explicit resource destruction keeps the safe
        // handle API sound even when previous submissions are still in flight.
        unsafe { self.device.device_wait_idle() }.map_err(VulkanError::from)?;
        self.poll_cleanup();
        Ok(())
    }

    fn destroy_buffer_now(&mut self, buffer: VulkanBuffer) -> Result<()> {
        // SAFETY: Buffer was created by this device and is destroyed once here.
        unsafe { self.device.destroy_buffer(buffer.buffer, None) };
        self.allocator.free(buffer.allocation)
    }

    fn destroy_texture_now(&mut self, texture: VulkanTexture) -> Result<()> {
        // SAFETY: Image was created by this device and is destroyed once here.
        unsafe { self.device.destroy_image(texture.image, None) };
        self.allocator.free(texture.allocation)
    }

    fn destroy_texture_view_now(&self, view: &VulkanTextureView) {
        let _ = view.texture;
        // SAFETY: Image view was created by this device and is destroyed once here.
        unsafe { self.device.destroy_image_view(view.view, None) };
    }

    fn destroy_sampler_now(&self, sampler: VulkanSampler) {
        // SAFETY: Sampler was created by this device and is destroyed once here.
        unsafe { self.device.destroy_sampler(sampler.sampler, None) };
    }

    fn destroy_resource_set_layout_now(&self, layout: &VulkanResourceSetLayout) {
        // SAFETY: Descriptor set layout was created by this device and is destroyed once here.
        unsafe {
            self.device
                .destroy_descriptor_set_layout(layout.layout, None);
        }
    }

    fn destroy_resource_set_now(&mut self, set: &VulkanResourceSet) {
        let _ = set.layout;
        // Restore capacity only when native destruction succeeds.
        if let Err(error) = self.descriptor_pools.free(
            &self.device, set.descriptor_pool, set.descriptor_set, set.descriptor_demand,
        ) {
            log::warn!("nova-gfx Vulkan descriptor free failed: {error}");
        }
    }

    fn destroy_pipeline_layout_now(&self, layout: &VulkanPipelineLayout) {
        let _ = layout.bind_group_layouts.len();
        // SAFETY: Pipeline layout was created by this device and is destroyed once here.
        unsafe {
            self.device.destroy_pipeline_layout(layout.layout, None);
        }
    }

    fn destroy_shader_module_now(&self, shader: &VulkanShaderModule) {
        // SAFETY: Shader module was created by this device and is destroyed once here.
        unsafe { self.device.destroy_shader_module(shader.module, None) };
    }

    fn destroy_render_pipeline_now(&self, pipeline: &VulkanRenderPipeline) {
        // SAFETY: Pipeline objects were created by this device and are destroyed once here.
        unsafe {
            self.device.destroy_pipeline(pipeline.pipeline, None);
            if pipeline.owns_pipeline_layout {
                self.device
                    .destroy_pipeline_layout(pipeline.pipeline_layout, None);
            }
        }
    }

    fn recycle_command_encoder(&mut self, mut encoder: VulkanCommandEncoder) -> Result<()> {
        encoder.compute_resources.clear();
        let frame_probe_surface = encoder.frame_probe_surface.take();
        encoder.sample_main_pass = true;
        encoder.retire_primary_value = None;
        // The consumer retirement predicate covers both binary operations.
        // Do not retain or overwrite old semaphore handles in the pool.
        if let Some(semaphores) = encoder.cross_queue_semaphores.take() {
            for semaphore in semaphores {
                // SAFETY: the consumer-side fence has completed.
                unsafe { self.device.destroy_semaphore(semaphore, None) };
            }
        }
        let frame_submission = encoder.frame_submission.take();
        // The retirement predicate proved this frame complete. Retain its
        // public ID until the caller consumes it; the WSI fence is reusable.
        let frame_submission = frame_submission.filter(|id| {
            if let Ok(record) = self.submissions.get_mut(*id) {
                record.fence = vk::Fence::null();
                true
            } else {
                false
            }
        });
        if let (Some(tag), Some(pool), Some(clock)) = (
            encoder.gpu_work_tag.take(),
            encoder.gpu_trace_query_pool,
            encoder.gpu_work_clock,
        ) {
            let mut ticks = [0_u64; 2];
            // SAFETY: The encoder's submission fence completed, and these two
            // query slots are private to the submitted compute command buffer.
            if unsafe {
                self.device.get_query_pool_results(
                    pool,
                    0,
                    &mut ticks,
                    vk::QueryResultFlags::TYPE_64,
                )
            }
            .is_ok()
            {
                self.gpu_work_samples.push(gfx_core::GpuTimestampSample {
                    tag,
                    duration_ticks: clock.duration_ticks(ticks[0], ticks[1]),
                    frequency_hz: clock.frequency_hz(),
                });
            }
        }
        // The deferred-queue completion predicate verified the submission fence.
        // Read native GPU timestamps only AFTER completion; never stall the rendering lane.
        if let (Some(pool), Some(clock)) = (encoder.gpu_trace_query_pool, self.timestamp_clock) {
            for (index, entry) in encoder.gpu_draw_tags.iter_mut().enumerate() {
                let Some(tag) = entry.take() else {
                    continue;
                };
                let Some(start) = u32::try_from(index).ok().and_then(|i| i.checked_mul(2)) else {
                    continue;
                };
                let mut ticks = [0_u64; 2];
                // SAFETY: Encoder's fence has completed. Do not use WAIT.
                if unsafe {
                    self.device.get_query_pool_results(
                        pool,
                        start,
                        &mut ticks,
                        vk::QueryResultFlags::TYPE_64,
                    )
                }
                .is_ok()
                {
                    self.gpu_work_samples.push(gfx_core::GpuTimestampSample {
                        tag,
                        duration_ticks: clock.duration_ticks(ticks[0], ticks[1]),
                        frequency_hz: clock.frequency_hz(),
                    });
                }
            }
        }
        if let (Some(pool), Some(clock)) = (encoder.gpu_trace_query_pool, self.timestamp_clock) {
            for (index, pass) in encoder.gpu_trace_passes.iter_mut().enumerate() {
                if *pass == VulkanGpuPass::Unused {
                    continue;
                }
                let Some(first_query) = u32::try_from(index)
                    .ok()
                    .and_then(|index| index.checked_mul(2))
                else {
                    continue;
                };
                let mut ticks = [0_u64; 2];
                // SAFETY: This encoder's fence completed and the query slots are exclusive.
                let result = unsafe {
                    self.device.get_query_pool_results(
                        pool,
                        first_query,
                        &mut ticks,
                        vk::QueryResultFlags::TYPE_64,
                    )
                };
                if result.is_ok() {
                    if *pass == VulkanGpuPass::Main {
                        let sample = FrameGpuTimestamps {
                            duration_ticks: clock.duration_ticks(ticks[0], ticks[1]),
                            frequency: clock.frequency_hz(),
                        };
                        self.last_frame_gpu_timestamps = Some(sample);
                        if let Some((id, native)) = frame_probe_surface {
                            if let Ok(chain) = self.swapchains.get_mut(id) {
                                // Resize recreates the native chain; never train its
                                // cadence on completed probes from the previous extent.
                                if chain.swapchain == native {
                                    chain.frame_probe.observe(sample.duration_ticks);
                                }
                            }
                        }
                        if let Some(id) = frame_submission {
                            if self.completed_frame_probes.len() >= 128 {
                                self.completed_frame_probes.pop_front();
                            }
                            self.completed_frame_probes.push_back((id, sample));
                        }
                    }
                    if self.gpu_trace_enabled {
                        let gpu_us = clock.elapsed_nanoseconds(ticks[0], ticks[1]) / 1000.0;
                        self.gpu_trace_totals.record(*pass, gpu_us);
                    }
                }
                *pass = VulkanGpuPass::Unused;
            }
            if self.gpu_trace_enabled {
                self.gpu_trace_totals.log_if_due();
            }
        }
        // Keep only bounded, fully idle native pools for subsequent submissions.
        if encoder.owns_fence && encoder.fence != vk::Fence::null() {
            // SAFETY: the completed submission owns this fence and it is destroyed once.
            unsafe { self.device.destroy_fence(encoder.fence, None) };
        }
        encoder.fence = vk::Fence::null();
        encoder.owns_fence = false;

        if self.free_command_encoders.len() >= self.command_encoder_pool_capacity {
            // SAFETY: all command buffers and their timestamp queries have finished executing.
            if let Some(query_pool) = encoder.gpu_trace_query_pool {
                unsafe { self.device.destroy_query_pool(query_pool, None) };
            }
            unsafe { self.device.destroy_command_pool(encoder.command_pool, None) };
            return Ok(());
        }
        // SAFETY: no pending GPU work references the command pool or any of its buffers.
        if let Err(error) = unsafe {
            self.device
                .reset_command_pool(encoder.command_pool, vk::CommandPoolResetFlags::empty())
        } {
            // Do not cache a pool that could not be reset to the initial state.
            if let Some(query_pool) = encoder.gpu_trace_query_pool {
                unsafe { self.device.destroy_query_pool(query_pool, None) };
            }
            unsafe { self.device.destroy_command_pool(encoder.command_pool, None) };
            return Err(VulkanError::from(error).into());
        }
        self.free_command_encoders.push(encoder);
        Ok(())
    }

    fn destroy_command_encoder_now(&self, encoder: &VulkanCommandEncoder) {
        if let Some(semaphores) = encoder.cross_queue_semaphores {
            // Consumer-side fence completed (or device-wide idle on
            // teardown): both the producer signal and GUI wait are finished.
            for semaphore in semaphores {
                unsafe { self.device.destroy_semaphore(semaphore, None) };
            }
        }
        if encoder.owns_fence && encoder.fence != vk::Fence::null() {
            // SAFETY: Fence was created for this command encoder and is destroyed once here.
            unsafe { self.device.destroy_fence(encoder.fence, None) };
        }
        // SAFETY: The GPU is idle or this encoder never submitted its timestamp queries.
        if let Some(query_pool) = encoder.gpu_trace_query_pool {
            unsafe { self.device.destroy_query_pool(query_pool, None) };
        }
        // SAFETY: Command pool was created by this device and owns the command buffer.
        unsafe { self.device.destroy_command_pool(encoder.command_pool, None) };
    }

    fn destroy_swapchain_now(&self, mut swapchain: VulkanSwapchain) {
        // Join the host present wait before destroying its native swapchain.
        drop(swapchain.present_wake.take());
        let _ = (swapchain.format, swapchain.images.len());
        // SAFETY: Swapchain resources were created by this device and are destroyed once here.
        unsafe {
            // Acquisition can outlive a CPU-side failed/abandoned frame.
            // The ordinary resize path already waits this fence; device drop
            // also needs the explicit proof before destroying the semaphore.
            for slot in swapchain.acquire_slots {
                if slot.pending {
                    let _ = self.device.wait_for_fences(&[slot.fence], true, u64::MAX);
                }
                if slot.fence != vk::Fence::null() {
                    self.device.destroy_fence(slot.fence, None);
                }
            }
            swapchain.presentation.destroy(&self.device);
            for image_view in swapchain.image_views {
                self.device.destroy_image_view(image_view, None);
            }
            for semaphore in swapchain.image_available_semaphores {
                self.device.destroy_semaphore(semaphore, None);
            }
            for semaphore in swapchain.render_finished_semaphores {
                self.device.destroy_semaphore(semaphore, None);
            }
            for fence in swapchain.in_flight_fences {
                self.device.destroy_fence(fence, None);
            }
            self.swapchain_loader
                .destroy_swapchain(swapchain.swapchain, None);
        }
    }

    fn destroy_surface_now(&self, surface: VulkanSurface) {
        // SAFETY: Surface was created for this instance and is destroyed once here.
        unsafe { self.surface_loader.destroy_surface(surface.surface, None) };
    }
}

impl Backend for VulkanDevice {
    const BACKEND_KIND: BackendKind = BackendKind::Vulkan;

    fn adapter_name(&self) -> &str {
        &self.adapter_name
    }
}

impl SurfaceDevice for VulkanDevice {
    type SurfaceTarget = dyn VulkanSurfaceTarget;

    fn prepare_swapchain_resize(&mut self, swapchain: gfx_core::SwapchainId) -> Result<bool> {
        if !self.swapchain_maintenance {
            // The native fallback still establishes the presentation-engine boundary.
            return Ok(true);
        }
        let chain = self.swapchains.get_mut(swapchain)?;
        for slot in &chain.acquire_slots {
            if slot.pending
                && !unsafe { self.device.get_fence_status(slot.fence) }.map_err(VulkanError::from)?
            {
                return Ok(false);
            }
        }
        if !chain.presentation.poll(&self.device)? {
            return Ok(false);
        }
        for &fence in &chain.in_flight_fences {
            // SAFETY: these fences remain live until this swapchain is retired.
            if !unsafe { self.device.get_fence_status(fence) }.map_err(VulkanError::from)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn create_surface(
        &mut self,
        target: &Self::SurfaceTarget,
        desc: &SurfaceDescriptor,
    ) -> Result<SurfaceId> {
        Self::create_surface(self, target, desc)
    }

    fn create_swapchain(
        &mut self,
        surface: SurfaceId,
        config: SurfaceConfig,
    ) -> Result<gfx_core::SwapchainId> {
        Self::create_swapchain(self, surface, config)
    }

    fn set_swapchain_source_extent(
        &mut self,
        swapchain: gfx_core::SwapchainId,
        extent: gfx_core::Extent2d,
    ) -> Result<()> {
        let swapchain = self.swapchains.get_mut(swapchain)?;
        if extent.width() > swapchain.extent.width || extent.height() > swapchain.extent.height {
            return Err(Error::InvalidInput(
                "Vulkan source extent exceeds swapchain extent".into(),
            ));
        }
        swapchain.source_extent = vk::Extent2D {
            width: extent.width(),
            height: extent.height(),
        };
        Ok(())
    }

    fn resize_swapchain(
        &mut self,
        swapchain: gfx_core::SwapchainId,
        size: gfx_core::Extent2d,
    ) -> Result<()> {
        Self::resize_swapchain(self, swapchain, size.width(), size.height())
    }

    fn recreate_swapchain(
        &mut self,
        swapchain: gfx_core::SwapchainId,
        config: SurfaceConfig,
    ) -> Result<gfx_core::SwapchainId> {
        Self::reconfigure_swapchain(self, swapchain, config)?;
        Ok(swapchain)
    }

    fn swapchain_extent(&self, swapchain: gfx_core::SwapchainId) -> Result<gfx_core::Extent2d> {
        self.swapchain_size(swapchain)
    }

    fn resolve_composite_alpha(
        &self,
        surface: SurfaceId,
        requested: CompositeAlphaMode,
    ) -> Result<CompositeAlphaMode> {
        self.resolve_surface_alpha_mode(surface, requested)
    }

    fn destroy_swapchain(&mut self, swapchain: gfx_core::SwapchainId) -> Result<()> {
        Self::destroy_swapchain(self, swapchain)
    }

    fn destroy_surface(&mut self, surface: SurfaceId) -> Result<()> {
        Self::destroy_surface(self, surface)
    }
}

impl ResourceDevice for VulkanDevice {
    fn max_color_attachments(&self) -> u32 {
        let limit = self
            .max_color_attachment_count
            .unwrap_or(gfx_core::MAX_COLOR_ATTACHMENTS as u32);
        limit.min(gfx_core::MAX_COLOR_ATTACHMENTS as u32)
    }

    fn texture_volume_capabilities(&self) -> gfx_core::TextureVolumeCapabilities {
        let properties = unsafe {
            self.instance
                .get_physical_device_properties(self.physical_device)
        };
        let format_features = unsafe {
            self.instance.get_physical_device_format_properties(
                self.physical_device,
                vk::Format::R16G16B16A16_SFLOAT,
            )
        }
        .optimal_tiling_features;
        let maximum = properties.limits.max_image_dimension3_d;
        gfx_core::TextureVolumeCapabilities {
            max_dimension_3d: maximum,
            sampled: maximum != 0
                && format_features.contains(vk::FormatFeatureFlags::SAMPLED_IMAGE),
            storage: maximum != 0
                && self.compute.capabilities.supports_compute()
                && format_features.contains(vk::FormatFeatureFlags::STORAGE_IMAGE),
        }
    }

    fn texture_array_capabilities(&self) -> gfx_core::TextureArrayCapabilities {
        gfx_core::TextureArrayCapabilities {
            array_resources: true,
            layer_uploads: true,
            indexed_sampling: false,
            ..self.texture_array_limits
        }
    }

    fn multisample_capabilities(&self) -> gfx_core::MultisampleCapabilities {
        gfx_core::MultisampleCapabilities {
            max_sample_count: self.max_sample_count,
            stencil_supported: self.stencil_supported,
        }
    }

    fn buffer_binding_limits(&self) -> gfx_core::BufferBindingLimits {
        self.binding_limits
    }

    fn compute_capabilities(&self) -> gfx_core::ComputeCapabilities {
        self.compute.capabilities
    }
    fn compute_bind_group_capabilities(&self) -> gfx_core::ComputeBindGroupCapabilities {
        gfx_core::ComputeBindGroupCapabilities {
            buffers: true,
            sampled_textures: true,
            samplers: true,
            storage_textures: true,
        }
    }
    fn dispatch_compute(&mut self, dispatch: &gfx_core::ComputeDispatch<'_>) -> Result<()> {
        self.dispatch_buffers(dispatch)
    }

    fn dispatch_compute_bind_group(
        &mut self,
        dispatch: &gfx_core::ComputeBindGroupDispatch,
    ) -> Result<()> {
        // Full native descriptor sets; the buffer-only batch still takes its
        // specialized zero-allocation path in dispatch_buffer_batch.
        self.dispatch_resource_set_profiled(dispatch, dispatch.gpu_work_tag)
    }
    fn dispatch_compute_batch(
        &mut self,
        dispatches: &[gfx_core::ComputeDispatch<'_>],
    ) -> Result<()> {
        self.dispatch_buffer_batch(dispatches)
    }
    fn gpu_work_timestamps_supported(&self) -> bool {
        self.timestamp_clock.is_some()
    }
    fn dispatch_compute_batch_profiled(
        &mut self,
        dispatches: &[gfx_core::ComputeDispatch<'_>],
        tag: gfx_core::GpuTimestampTag,
    ) -> Result<()> {
        self.dispatch_buffer_batch_profiled(dispatches, Some(tag))
    }
    fn dispatch_compute_batch_tagged(
        &mut self, dispatches: &[gfx_core::ComputeDispatch<'_>],
        tags: &[Option<gfx_core::GpuTimestampTag>],
    ) -> Result<()> {
        if dispatches.len() != tags.len() {
            return Err(Error::InvalidInput("compute tag count mismatch".into()));
        }
        if tags.iter().all(Option::is_none) {
            return self.dispatch_buffer_batch(dispatches);
        }
        self.dispatch_buffer_batch_impl(dispatches, None, None, tags).map(|_| ())
    }
    fn write_buffer_batch_profiled<'a>(
        &mut self,
        writes: impl IntoIterator<Item = BufferWrite<'a>>,
        tag: gfx_core::GpuTimestampTag,
    ) -> Result<BufferUploadStats> {
        self.upload_buffer_batch_profiled(writes, Some(tag))
    }
    fn drain_gpu_work_timestamps(
        &mut self,
        output: &mut Vec<gfx_core::GpuTimestampSample>,
    ) -> Result<()> {
        self.poll_cleanup();
        output.append(&mut self.gpu_work_samples);
        Ok(())
    }
    fn texture_copy_limits(&self) -> gfx_core::TextureCopyLimits {
        gfx_core::TextureCopyLimits {
            row_pitch_alignment: self.copy_row_pitch_alignment,
            buffer_offset_alignment: self.copy_offset_alignment,
        }
    }

    fn copy_texture_buffer_batch(&mut self, copies: &[gfx_core::TextureBufferCopy]) -> Result<()> {
        self.copy_texture_buffer_regions(copies)
    }

    fn copy_texture_batch(&mut self, copies: &[gfx_core::TextureCopy]) -> Result<()> {
        self.copy_texture_regions(copies)
    }
    fn create_buffer(&mut self, desc: &BufferDescriptor) -> Result<BufferId> {
        Self::create_buffer(self, desc)
    }

    fn write_buffer(&mut self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<()> {
        Self::write_buffer(self, buffer, offset, data)
    }

    fn write_buffer_batch<'a>(
        &mut self,
        writes: impl IntoIterator<Item = BufferWrite<'a>>,
    ) -> Result<BufferUploadStats> {
        self.upload_buffer_batch(writes)
    }

    fn create_texture(&mut self, desc: &TextureDescriptor) -> Result<TextureId> {
        Self::create_texture(self, desc)
    }

    fn write_texture(&mut self, desc: TextureWriteDescriptor, data: &[u8]) -> Result<()> {
        Self::write_texture(self, desc, data)
    }

    fn write_texture_array_layer(
        &mut self,
        desc: gfx_core::TextureArrayLayerWriteDescriptor,
        data: &[u8],
    ) -> Result<()> {
        let (image, old_layout, mip_level_count, format) = {
            let texture = self.textures.get(desc.texture)?;
            desc.validate_against(&texture.desc, self.texture_array_capabilities(), data.len())?;
            (
                texture.image,
                texture.layout,
                texture.desc.mip_level_count,
                texture.desc.format,
            )
        };
        let slice = desc.upload;
        let staging_descriptor = TextureWriteDescriptor {
            texture: desc.texture,
            mip_level: slice.mip_level,
            layout: slice.layout,
            origin: slice.origin,
            size: slice.extent,
        };
        validate_texture_staging_layout(staging_descriptor, format)?;
        let allocation = self.write_upload_data(data)?;
        let staging_buffer = self.upload_page_buffer(allocation.page_index)?;
        let layout = texture_staging_layout(allocation, staging_descriptor)?;
        self.copy_buffers_to_textures(&[VulkanTextureUpload {
            texture: desc.texture,
            source: staging_buffer,
            image,
            old_layout,
            mip_level_count,
            layout,
            mip_level: slice.mip_level,
            layer: slice.layer,
            origin: slice.origin,
            size: slice.extent,
        }])?;
        self.textures.get_mut(desc.texture)?.layout = vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL;
        Ok(())
    }

    fn write_texture_batch<'a>(
        &mut self,
        writes: impl IntoIterator<Item = TextureWrite<'a>>,
    ) -> Result<()> {
        Self::write_texture_batch(self, writes)
    }

    fn create_texture_view(&mut self, desc: &TextureViewDescriptor) -> Result<TextureViewId> {
        Self::create_texture_view(self, desc)
    }

    fn create_sampler(&mut self, desc: &SamplerDescriptor) -> Result<SamplerId> {
        Self::create_sampler(self, desc)
    }

    fn create_bind_group_layout(
        &mut self,
        desc: &BindGroupLayoutDescriptor,
    ) -> Result<BindGroupLayoutId> {
        Self::create_bind_group_layout(self, desc)
    }

    fn create_bind_group(&mut self, desc: &BindGroupDescriptor) -> Result<BindGroupId> {
        Self::create_bind_group(self, desc)
    }

    fn destroy_buffer(&mut self, buffer: BufferId) -> Result<()> {
        Self::destroy_buffer(self, buffer)
    }

    fn destroy_buffer_batch(&mut self, buffers: &[BufferId]) -> Result<()> {
        Self::destroy_buffer_batch(self, buffers)
    }

    fn destroy_texture(&mut self, texture: TextureId) -> Result<()> {
        Self::destroy_texture(self, texture)
    }

    fn destroy_texture_view(&mut self, view: TextureViewId) -> Result<()> {
        Self::destroy_texture_view(self, view)
    }

    fn destroy_sampler(&mut self, sampler: SamplerId) -> Result<()> {
        Self::destroy_sampler(self, sampler)
    }

    fn destroy_bind_group_layout(&mut self, layout: BindGroupLayoutId) -> Result<()> {
        Self::destroy_bind_group_layout(self, layout)
    }

    fn destroy_bind_group(&mut self, bind_group: BindGroupId) -> Result<()> {
        Self::destroy_bind_group(self, bind_group)
    }
}

impl PipelineDevice for VulkanDevice {
    fn create_pipeline_layout(
        &mut self,
        desc: &PipelineLayoutDescriptor,
    ) -> Result<PipelineLayoutId> {
        Self::create_pipeline_layout(self, desc)
    }

    fn create_shader_module(&mut self, desc: &ShaderModuleDescriptor) -> Result<ShaderModuleId> {
        Self::create_shader_module(self, desc)
    }

    fn create_render_pass(&mut self, desc: &RenderPassDescriptor) -> Result<RenderPassId> {
        Self::create_render_pass(self, desc)
    }

    fn create_render_pipeline(
        &mut self,
        desc: &RenderPipelineDescriptor,
        viewport_extent: gfx_core::Extent2d,
    ) -> Result<RenderPipelineId> {
        Self::create_render_pipeline(self, desc, viewport_extent)
    }

    fn destroy_pipeline_layout(&mut self, layout: PipelineLayoutId) -> Result<()> {
        Self::destroy_pipeline_layout(self, layout)
    }

    fn destroy_shader_module(&mut self, shader: ShaderModuleId) -> Result<()> {
        Self::destroy_shader_module(self, shader)
    }

    fn destroy_render_pass(&mut self, render_pass: RenderPassId) -> Result<()> {
        Self::destroy_render_pass(self, render_pass)
    }

    fn destroy_render_pipeline(&mut self, pipeline: RenderPipelineId) -> Result<()> {
        Self::destroy_render_pipeline(self, pipeline)
    }
}

impl CommandDevice for VulkanDevice {
    fn create_command_encoder(
        &mut self,
        desc: &CommandEncoderDescriptor,
    ) -> Result<CommandEncoderId> {
        Self::create_command_encoder(self, desc)
    }

    fn record_draw_desc(&mut self, encoder: CommandEncoderId, draw: DrawDescriptor) -> Result<()> {
        Self::record_draw_desc(self, encoder, &draw)
    }

    fn submit(&mut self, encoder: CommandEncoderId) -> Result<()> {
        Self::submit(self, encoder)
    }

    fn destroy_command_encoder(&mut self, encoder: CommandEncoderId) -> Result<()> {
        Self::destroy_command_encoder(self, encoder)
    }
}

impl VulkanDevice {
    /// GpuQueueId::PRIMARY reserves (0,0) even when the real Vulkan graphics
    /// family isn't zero. Namespace dedicated-family IDs by adding one to
    /// the physical family index, avoiding accidental alias with PRIMARY.
    fn checkpoint_native_queue(
        &mut self, native: vk::Queue, queue: gfx_core::GpuQueueId,
    ) -> Result<SubmissionId> {
        // A checkpoint needs no command-pool or WSI retirement proof. The
        // device-owned timeline is sufficient for the public completion ID.
        if let Some(timeline) = self.timeline_for_queue(queue) {
            timeline.submit(&self.device, native, &[], &[], &[],
                vk::PipelineStageFlags2::ALL_COMMANDS, vk::Fence::null())?;
            let value = timeline.last();
            return Ok(self.submissions.insert(VulkanSubmission {
                fence: vk::Fence::null(),
                queue,
                owned_by_submission: false,
                timeline_value: Some(value),
            }));
        }
        let fence = create_fence(&self.device, false)?;
        if let Err(error) = submit::binary(&self.device, native, &[], &[], &[],
            vk::PipelineStageFlags2::ALL_COMMANDS, fence) {
            // SAFETY: submission failed and no queued operation owns the fence.
            unsafe { self.device.destroy_fence(fence, None) };
            return Err(error);
        }
        Ok(self.submissions.insert(VulkanSubmission {
            fence,
            queue,
            owned_by_submission: true,
            timeline_value: None,
        }))
    }

    fn timeline_for_queue(&self, queue: gfx_core::GpuQueueId) -> Option<&timeline::QueueTimeline> {
        if queue == gfx_core::GpuQueueId::PRIMARY {
            self.primary_timeline.as_ref()
        } else if self.independent_queue(gfx_core::GpuWorkStage::Graphics) == Some(queue)
            || self.dedicated_compute_queue_id() == Some(queue)
        {
            self.secondary_timeline.as_ref()
        } else {
            None
        }
    }

    fn dedicated_compute_queue_id(&self) -> Option<gfx_core::GpuQueueId> {
        self.dedicated_compute_queue?;
        Some(gfx_core::GpuQueueId {
            family: self.dedicated_compute_family_index?.checked_add(1)?,
            index: 0,
        })
    }
}

impl SubmissionDevice for VulkanDevice {
    fn independent_queue(&self, stage: gfx_core::GpuWorkStage) -> Option<gfx_core::GpuQueueId> {
        let same_family = self.sidecar_queue.map(|_| gfx_core::GpuQueueId {
            family: self.graphics_queue_family_index,
            index: 1,
        });
        match stage {
            gfx_core::GpuWorkStage::Graphics | gfx_core::GpuWorkStage::Transfer => same_family,
            // Same-family queue is cheaper: no cross-family ownership or
            // concurrent-sharing cost and no additional pipeline barriers.
            // With only one graphics queue, use the dedicated compute family
            // for buffer-only submissions. Textures/images stay on graphics.
            gfx_core::GpuWorkStage::Compute => {
                same_family.or_else(|| self.dedicated_compute_queue_id())
            }
        }
    }

    fn checkpoint_gpu_queue(&mut self, queue: gfx_core::GpuQueueId) -> Result<SubmissionId> {
        if queue == gfx_core::GpuQueueId::PRIMARY {
            return self.checkpoint_ordered_queue(gfx_core::GpuWorkStage::Graphics);
        }
        let secondary = if self.independent_queue(gfx_core::GpuWorkStage::Graphics) == Some(queue) {
            self.sidecar_queue
                .expect("matched available graphics-family queue")
        } else if self.dedicated_compute_queue_id() == Some(queue) {
            self.dedicated_compute_queue
                .expect("matched dedicated compute queue")
        } else {
            return Err(Error::Unavailable(
                "Vulkan checkpoint target is not an active native queue".into(),
            ));
        };
        self.checkpoint_native_queue(secondary, queue)
    }

    fn submit_compute(&mut self, submit: gfx_core::ComputeSubmit<'_>) -> Result<SubmissionId> {
        let gfx_core::ComputeSubmit {
            queue,
            dispatches,
            timestamp_tag,
        } = submit;
        // Same-family compute uses the secondary graphics queue. Dedicated
        // compute uses queue-family-typed command pools and CONCURRENT-shared
        // storage/uniform buffers, so no EXCLUSIVE ownership transfer occurs.
        // Image compute is never submitted to this buffer-only path.
        if queue != gfx_core::GpuQueueId::PRIMARY
            && self.independent_queue(gfx_core::GpuWorkStage::Compute) != Some(queue)
        {
            return Err(Error::Unavailable(
                "Vulkan native Compute queue is not available".into(),
            ));
        }
        self.dispatch_buffer_batch_on_queue(dispatches, timestamp_tag, Some(queue))?
            .ok_or_else(|| {
                Error::Unavailable("native Compute submission did not return a GPU fence".into())
            })
    }

    fn submit_buffer_copies_on_queue(
        &mut self,
        queue: gfx_core::GpuQueueId,
        copies: &[gfx_core::GpuBufferCopy],
    ) -> Result<SubmissionId> {
        self.submit_native_buffer_copies(queue, copies)
    }

    fn enqueue_gpu_queue_wait(
        &mut self,
        target_queue: gfx_core::GpuQueueId,
        producer: SubmissionId,
    ) -> Result<gfx_core::GpuNativeWaitProof> {
        // A native semaphore provides queue execution/memory ordering.
        // Dedicated-family Compute is legal only for CONCURRENT-shared
        // buffers; EXCLUSIVE textures/images are not admitted here.
        let secondary = self.sidecar_queue.map(|queue| {
            (
                gfx_core::GpuQueueId {
                    family: self.graphics_queue_family_index,
                    index: 1,
                },
                queue,
            )
        });
        let dedicated = self
            .dedicated_compute_queue_id()
            .zip(self.dedicated_compute_queue);
        let lookup = |id| {
            if id == gfx_core::GpuQueueId::PRIMARY {
                Some(self.graphics_queue)
            } else if secondary.is_some_and(|(q, _)| q == id) {
                secondary.map(|(_, queue)| queue)
            } else if dedicated.is_some_and(|(q, _)| q == id) {
                dedicated.map(|(_, queue)| queue)
            } else {
                None
            }
        };
        let target = lookup(target_queue).ok_or_else(|| {
            Error::Unavailable("Vulkan requested GPU wait target queue is unavailable".into())
        })?;
        let source = self.submissions.get(producer)?;
        if source.queue == target_queue {
            return Err(Error::InvalidInput(
                "Vulkan cross-queue GPU wait cannot target the producer queue".into(),
            ));
        }
        let source_queue = lookup(source.queue).ok_or_else(|| {
            Error::Unavailable("Vulkan producer has no matching native queue".into())
        })?;

        // Exact producer value: no extra producer submit, transient semaphore,
        // or wait-fence allocation. The device owns the timeline until idle.
        if let Some((value, timeline)) = source.timeline_value.zip(self.timeline_for_queue(source.queue)) {
            if let Some(target_timeline) = self.timeline_for_queue(target_queue) {
                target_timeline.submit_after(&self.device, target, &[], timeline, value)?;
            } else {
                timeline.enqueue_wait(&self.device, target, value)?;
            }
            // SAFETY: a native timeline wait covering the producer is enqueued.
            return Ok(unsafe { gfx_core::GpuNativeWaitProof::after_native_wait(producer, target_queue) });
        }

        let info = vk::SemaphoreCreateInfo::default();
        // SAFETY: Binary semaphore is allocated on this live logical device.
        let semaphore =
            unsafe { self.device.create_semaphore(&info, None) }.map_err(VulkanError::from)?;
        let fence = match create_fence(&self.device, false) {
            Ok(fence) => fence,
            Err(error) => {
                // SAFETY: The semaphore was never submitted to a queue.
                unsafe { self.device.destroy_semaphore(semaphore, None) };
                return Err(error);
            }
        };
        let result = (|| -> Result<()> {
            submit::binary(&self.device, source_queue, &[], &[], &[semaphore],
                vk::PipelineStageFlags2::ALL_COMMANDS, vk::Fence::null())?;
            if let Some(timeline) = self.timeline_for_queue(target_queue) {
                timeline.submit(&self.device, target, &[], &[semaphore], &[],
                    vk::PipelineStageFlags2::ALL_COMMANDS, fence)?;
            } else {
                submit::binary(&self.device, target, &[], &[semaphore], &[],
                    vk::PipelineStageFlags2::ALL_COMMANDS, fence)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            // A partial queue-submit failure can leave the binary semaphore
            // in use. This exceptional error path drains the device before
            // destroying native sync objects; normal frames never host-wait.
            // SAFETY: Queue ownership remains with this device.
            let _ = unsafe { self.device.device_wait_idle() };
            // SAFETY: Queued references are no longer in flight.
            unsafe {
                self.device.destroy_fence(fence, None);
                self.device.destroy_semaphore(semaphore, None);
            }
            return Err(error);
        }
        self.native_queue_waits.push(VulkanQueueWait {
            semaphore,
            wait_fence: fence,
        });
        // SAFETY: A Vulkan binary semaphore signal and matching GPU wait
        // have both been submitted successfully to these physical queues.
        Ok(unsafe { gfx_core::GpuNativeWaitProof::after_native_wait(producer, target_queue) })
    }

    fn checkpoint_ordered_queue(&mut self, _stage: gfx_core::GpuWorkStage) -> Result<SubmissionId> {
        self.checkpoint_native_queue(self.graphics_queue, gfx_core::GpuQueueId::PRIMARY)
    }

    fn wait_queue_idle(&mut self) -> Result<()> {
        // Preserve the public queue-idle contract, including native
        // operations that are not represented by a resource epoch.
        unsafe { self.device.queue_wait_idle(self.graphics_queue) }.map_err(VulkanError::from)?;
        self.poll_cleanup();
        Ok(())
    }

    fn async_capabilities(&self) -> gfx_core::AsyncCapabilities {
        gfx_core::AsyncCapabilities {
            threading_mode: ThreadingMode::MultiThreadDeviceProxy,
            async_submission: true,
            async_wait: true,
            async_presentation: true,
        }
    }

    fn submit_deferred(&mut self, encoder: CommandEncoderId) -> Result<SubmissionId> {
        Self::submit_deferred(self, encoder)
    }

    fn poll_submission(&mut self, submission: SubmissionId) -> Result<SubmissionStatus> {
        Self::poll_submission(self, submission)
    }

    fn request_submission_ready(
        &mut self,
        submission: SubmissionId,
        callback: Box<dyn FnOnce() + Send + 'static>,
    ) -> Result<bool> {
        self.poll_cleanup();
        let record = self.submissions.get(submission)?;
        if self.submission_is_complete(record)? {
            self.submission_wakes.cancel(submission);
            callback();
            return Ok(true);
        }
        let queue_id = record.queue;
        let queue = if queue_id == gfx_core::GpuQueueId::PRIMARY {
            self.graphics_queue
        } else if self.independent_queue(gfx_core::GpuWorkStage::Graphics) == Some(queue_id) {
            self.sidecar_queue.expect("matched graphics queue")
        } else if self.dedicated_compute_queue_id() == Some(queue_id) {
            self.dedicated_compute_queue.expect("matched compute queue")
        } else {
            return Ok(false);
        };
        let timeline = record.timeline_value.zip(self.timeline_for_queue(queue_id))
            .map(|(value, timeline)| timeline.wait_point(value));
        self.submission_wakes.register(&self.device, queue, submission, timeline, callback)
    }

    fn poll_submission_with_frame_timestamps(&mut self, submission: SubmissionId)
        -> Result<(SubmissionStatus, Option<FrameGpuTimestamps>)> {
        let status = Self::poll_submission(self, submission)?;
        if status != SubmissionStatus::Complete {
            return Ok((status, None));
        }
        let sample = self.completed_frame_probes.iter().position(|(id, _)| *id == submission)
            .and_then(|index| self.completed_frame_probes.remove(index)).map(|(_, sample)| sample);
        Ok((status, sample))
    }

    fn wait_submission(&mut self, submission: SubmissionId) -> Result<()> {
        Self::wait_submission(self, submission)
    }
}

impl PresentationDevice for VulkanDevice {
    fn presentation_feedback(&self, id: gfx_core::SwapchainId) -> Option<gfx_core::PresentationFeedback> {
        let chain = self.swapchains.get(id).ok()?;
        let loader = self.display_timing.as_ref()?;
        if !chain.display_timing_enabled {
            return None;
        }
        // SAFETY: extension enabled at device creation; this swapchain is live.
        // These queries return past presentation records, not a wait for scanout.
        let samples = unsafe { loader.get_past_presentation_timing(chain.swapchain) }.ok()?;
        let sample = samples.iter().filter(|s| s.actual_present_time != 0)
            .max_by_key(|s| s.actual_present_time)?;
        let refresh = unsafe { loader.get_refresh_cycle_duration(chain.swapchain) }.ok();
        Some(gfx_core::PresentationFeedback::Vulkan {
            present_id: sample.present_id,
            actual_present_ns: sample.actual_present_time,
            earliest_present_ns: sample.earliest_present_time,
            present_margin_ns: sample.present_margin,
            refresh_duration_ns: refresh.map(|r| r.refresh_duration).filter(|ns| *ns != 0),
        })
    }

    fn set_frame_latency_limit(
        &mut self, id: gfx_core::SwapchainId, requested: u32,
    ) -> Result<Option<u32>> {
        let chain = self.swapchains.get_mut(id)?;
        let maximum = chain.in_flight_fences.len().max(1);
        let Some(credit) = chain.present_credit.as_mut() else {
            return Ok(None);
        };
        let limit = (requested as usize).clamp(1, maximum);
        credit.set_limit(limit);
        Ok(Some(limit as u32))
    }

    fn request_frame_ready(
        &mut self, id: gfx_core::SwapchainId, callback: Box<dyn FnOnce() + Send + 'static>,
    ) -> Result<bool> {
        // GPU and WSI readiness have independent completion identities.
        // Never wait on a reusable GPU frame fence from a second thread.
        if PresentationDevice::poll_present_readiness(self, id)? {
            callback();
            return Ok(true);
        }
        let chain = self.swapchains.get(id)?;
        if chain.reserved_frame.is_some() {
            // A recovery-only fence has no public submission ID. Never
            // arm a stale slot or WSI wait for an already acquired image.
            return Ok(false);
        }
        if chain.extent.width == 0 || chain.extent.height == 0 {
            return Ok(false);
        }
        let slot = chain.frame_index;
        let fence = chain.in_flight_fences[slot];
        // SAFETY: swapchain owns this fence and this call does not reset it.
        if unsafe { self.device.get_fence_status(fence) }.map_err(VulkanError::from)? {
            // GPU slot is ready; wait for the actual WSI present ID on a
            // sleeping worker. The owner still verifies readiness on wake.
            let Some(loader) = self.present_wait.as_ref() else {
                return Ok(false);
            };
            let chain = self.swapchains.get_mut(id)?;
            let Some(target) = chain.present_credit.as_ref().and_then(PresentCredit::wait_target)
            else {
                return Ok(false);
            };
            if chain.present_wake.is_none() {
                chain.present_wake = Some(presentation::PresentWake::new(
                    loader.clone(), chain.swapchain,
                )?);
            }
            return Ok(chain.present_wake.as_ref().expect("present wake initialized")
                .request(target, callback));
        }
        let Some(submission) = chain.frame_submissions[slot] else { return Ok(false); };
        if self.submissions.get(submission).is_err() {
            return Ok(false);
        }
        SubmissionDevice::request_submission_ready(self, submission, callback)
    }

    fn poll_present_readiness(&mut self, id: gfx_core::SwapchainId) -> Result<bool> {
        let chain = self.swapchains.get(id)?;
        if let Some(frame) = chain.reserved_frame {
            // This frame already owns an acquired WSI image and its credit.
            // Reapplying a reduced present limit could strand the reservation.
            return self.reserved_frame_ready(frame);
        }
        if chain.extent.width == 0 || chain.extent.height == 0 {
            // The acquire path owns suspended-surface recovery.
            return Ok(true);
        }
        let fence = chain.in_flight_fences[chain.frame_index];
        // SAFETY: this swapchain owns the frame slot's live fence. Querying it
        // neither consumes an acquire semaphore nor waits for the GPU.
        if !unsafe { self.device.get_fence_status(fence) }.map_err(VulkanError::from)? {
            return Ok(false);
        }
        let acquire = &chain.acquire_slots[chain.frame_index];
        if acquire.pending && !unsafe { self.device.get_fence_status(acquire.fence) }.map_err(VulkanError::from)? {
            return Ok(false);
        }
        let Some(loader) = self.present_wait.as_ref() else {
            return Ok(true);
        };
        let chain = self.swapchains.get_mut(id)?;
        let Some(credit) = chain.present_credit.as_mut() else {
            return Ok(true);
        };
        let Some(oldest) = credit.wait_target() else {
            return Ok(true);
        };
        // The queue owner serializes this zero-timeout WSI query with present
        // and swapchain recreation. It never waits on a display interval.
        match unsafe { loader.wait_for_present(chain.swapchain, oldest, 0) } {
            Ok(()) => {
                credit.completed(oldest);
                Ok(true)
            }
            Err(vk::Result::TIMEOUT) => {
                if credit.stalled(Instant::now()) {
                    // An occluded surface may stop presenting altogether.
                    // Never strand swapchain recovery behind this optional gate.
                    // GPU fences and WSI acquire still enforce resource safety.
                    log::debug!("Vulkan present-wait stalled; restoring acquire/fence pacing");
                    chain.present_credit = None;
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            Err(vk::Result::ERROR_DEVICE_LOST) => Err(VulkanError::from(vk::Result::ERROR_DEVICE_LOST).into()),
            Err(error) => {
                // Some WSI implementations cannot report completion for a
                // particular surface. Fall back to acquire/fence pacing and
                // let the existing acquire path handle outdated surfaces.
                log::debug!("Vulkan present-wait unavailable for swapchain: {error:?}");
                chain.present_credit = None;
                Ok(true)
            }
        }
    }

    fn ordered_mixed_graphics_compute_supported(&self) -> bool {
        // Normal offscreen passes and resource-set compute dispatches both
        // submit to graphics_queue. The independent sidecar queue is never
        // selected for mixed work; image layouts are tracked per resource.
        true
    }

    fn surface_capabilities(
        &self,
        swapchain: gfx_core::SwapchainId,
    ) -> gfx_core::SurfaceCapabilities {
        // The renderer submits a complete frame, but Vulkan swapchain images rotate between
        // presents. Without per-image damage accumulation, VK_KHR_incremental_present can leave
        // unchanged pixels sourced from an uninitialized image, which appears as intermittent
        // black corners or rectangular stale regions on Linux/X11 and Wayland.
        gfx_core::SurfaceCapabilities {
            partial_presentation: false,
            // GPUI gates native readiness registration on this capability.
            // GPU-slot wakes work even without present_wait; unsupported
            // WSI-only waits return false and keep the owner's bounded fallback.
            frame_ready_notification: self.swapchains.get(swapchain).is_ok(),
        }
    }

    fn render_and_present(
        &mut self,
        frame: &PresentFrameDescriptor<'_>,
    ) -> Result<Option<PresentedFrame>> {
        let LoadOp::Clear(clear_color) = frame.color_load_op else {
            return Err(Error::Unavailable(
                "Vulkan swapchain LoadOp::Load is not implemented".to_string(),
            ));
        };
        self.record_and_present_timed(
            frame.swapchain,
            frame.render_pass,
            frame.commands,
            clear_color,
            frame.depth_attachment,
            frame.present_region,
        )
    }

    fn indirect_draw_count_limit(&self) -> Option<u32> {
        self.draw_indirect_count.then_some(self.max_draw_indirect_count)
    }

    fn render_to_texture_counted(
        &mut self, pass: &RenderPassCommands<'_>, counts: &[gfx_core::IndirectCountBinding],
    ) -> Result<()> {
        self.render_to_texture_counted_profiled(pass, counts, None)
    }

    fn render_to_texture_counted_profiled(
        &mut self, pass: &RenderPassCommands<'_>, counts: &[gfx_core::IndirectCountBinding],
        tag: Option<gfx_core::GpuTimestampTag>,
    ) -> Result<()> {
        if counts.is_empty() {
            return self.record_texture_passes(std::slice::from_ref(pass),
                Some(std::slice::from_ref(&tag)), false, &[]).map(|_| ());
        }
        self.render_to_textures_counted_profiled(
            std::slice::from_ref(pass), std::slice::from_ref(&counts), std::slice::from_ref(&tag),
        ).map(|_| ())
    }

    fn render_to_textures_counted_profiled(
        &mut self, passes: &[RenderPassCommands<'_>], counts: &[&[gfx_core::IndirectCountBinding]],
        tags: &[Option<gfx_core::GpuTimestampTag>],
    ) -> Result<Option<SubmissionId>> {
        if passes.len() != counts.len() || passes.len() != tags.len() {
            return Err(Error::InvalidInput("counted pass metadata length mismatch".into()));
        }
        for (pass, &counts) in passes.iter().zip(counts) {
            if counts.is_empty() { continue; }
            let limit = self.indirect_draw_count_limit().ok_or_else(|| {
                Error::Unavailable("Vulkan drawIndirectCount is unavailable".into())
            })?;
            let mut previous = None;
            for &count in counts {
                if previous.is_some_and(|index| index >= count.command_index) {
                    return Err(Error::InvalidInput("indirect count bindings must be strictly ordered".into()));
                }
                previous = Some(count.command_index);
                let (arguments, offset, stride) = match pass.commands.get(count.command_index) {
                    Some(RenderCommand::DrawIndirect(draw)) => (draw.arguments, draw.offset, 16),
                    Some(RenderCommand::DrawIndexedIndirect(draw)) => (draw.arguments, draw.offset, 20),
                    _ => return Err(Error::InvalidInput("count binding requires an indirect draw".into())),
                };
                let args = self.buffers.get(arguments)?;
                let counter = self.buffers.get(count.count_buffer)?;
                if !args.desc.usage.contains(BufferUsage::INDIRECT)
                    || !counter.desc.usage.contains(BufferUsage::INDIRECT)
                {
                    return Err(Error::InvalidInput("counted draw buffers require INDIRECT usage".into()));
                }
                count.validate_ranges(offset, args.desc.size, counter.desc.size, stride, limit)?;
            }
        }
        self.record_texture_passes(passes, Some(tags), false, counts)
    }

    fn render_to_texture(&mut self, pass: &RenderPassCommands<'_>) -> Result<()> {
        self.record_texture_passes(std::slice::from_ref(pass), None, false, &[])
            .map(|_| ())
    }

    fn render_to_textures(
        &mut self,
        passes: &[RenderPassCommands<'_>],
    ) -> Result<Option<SubmissionId>> {
        self.record_texture_passes(passes, None, false, &[])?;
        Ok(None)
    }

    fn render_to_textures_on_queue(
        &mut self,
        passes: &[RenderPassCommands<'_>],
        tags: &[Option<gfx_core::GpuTimestampTag>],
        _queue: gfx_core::OffscreenQueuePreference,
    ) -> Result<Option<SubmissionId>> {
        self.record_texture_passes(passes, Some(tags), false, &[])
    }

    fn independent_graphics_queue_available(&self) -> bool {
        self.sidecar_queue.is_some()
    }

    fn render_to_textures_independent(
        &mut self,
        passes: &[RenderPassCommands<'_>],
        tags: &[Option<gfx_core::GpuTimestampTag>],
        _queue: gfx_core::OffscreenQueuePreference,
    ) -> Result<Option<SubmissionId>> {
        self.record_texture_passes(passes, Some(tags), self.sidecar_queue.is_some(), &[])
    }
}

impl DiagnosticsDevice for VulkanDevice {
    fn compact_memory(&mut self) -> Result<gfx_core::MemoryCompactReport> {
        self.compact_heap()
    }

    fn resource_stats(&self) -> ResourceStats {
        Self::resource_stats(self)
    }

    fn memory_budget(&self) -> Result<Option<DeviceMemoryBudget>> {
        if !self.memory_budget_supported {
            return Ok(None);
        }
        let mut budget = vk::PhysicalDeviceMemoryBudgetPropertiesEXT::default();
        let mut properties = vk::PhysicalDeviceMemoryProperties2::default().push_next(&mut budget);
        // SAFETY: the instance targets Vulkan 1.3, the optional budget
        // extension is enabled, and the physical device/output chain are live.
        unsafe {
            self.instance.get_physical_device_memory_properties2(self.physical_device, &mut properties)
        };
        let heaps = properties.memory_properties;
        let mut result = DeviceMemoryBudget::default();
        for index in 0..heaps.memory_heap_count as usize {
            let segment = if heaps.memory_heaps[index]
                .flags
                .contains(vk::MemoryHeapFlags::DEVICE_LOCAL)
            {
                &mut result.local
            } else {
                &mut result.non_local
            };
            let total = segment.get_or_insert_with(MemoryBudget::default);
            total.usage_bytes = total.usage_bytes.saturating_add(budget.heap_usage[index]);
            total.budget_bytes = total.budget_bytes.saturating_add(budget.heap_budget[index]);
        }
        Ok(Some(result))
    }

    fn frame_gpu_timestamps_supported(&self) -> bool {
        self.timestamp_clock.is_some()
    }

    fn last_frame_gpu_timestamps(&mut self) -> Result<Option<FrameGpuTimestamps>> {
        // Recycle fence-complete encoders so their Main-pass queries are read
        // without waiting for in-flight GPU work.
        self.poll_cleanup();
        Ok(self.last_frame_gpu_timestamps)
    }
}

impl TextureTransferDevice for VulkanDevice {
    fn supports_transfer_timestamps(&self) -> bool {
        self.timestamp_clock.is_some()
    }

    fn wait_texture_transfers(&mut self) -> Result<()> {
        self.wait_for_pending_work()
    }

    fn last_texture_transfer_time(&self) -> Option<Duration> {
        self.last_texture_transfer_time
    }

    fn read_texture(&mut self, texture: TextureId) -> Result<TextureReadback> {
        self.read_texture_pixels(texture)
    }
}

impl Drop for VulkanDevice {
    fn drop(&mut self) {
        // SAFETY: Device may still have in-flight work; waiting before destruction is valid.
        let _ = unsafe { self.device.device_wait_idle() };
        // Drain native wait threads before destroying any private fences or VkDevice.
        self.submission_wakes.shutdown(&self.device);
        if let Some(timeline) = self.primary_timeline.take() {
            timeline.destroy(&self.device);
        }
        if let Some(timeline) = self.secondary_timeline.take() {
            timeline.destroy(&self.device);
        }
        for bridge in self.native_queue_waits.drain(..) {
            // SAFETY: device_wait_idle above returned after all GPU-side waits;
            // neither queue can reference this semaphore or fence again.
            unsafe {
                self.device.destroy_fence(bridge.wait_fence, None);
                self.device.destroy_semaphore(bridge.semaphore, None);
            }
        }
        // Checkpoints have no encoder/swapchain owner. Destroy any unpolled
        // native fences before the logical device itself is released.
        for (_, submission) in self.submissions.drain_live() {
            if submission.owned_by_submission {
                // SAFETY: device_wait_idle completed above and these checkpoint
                // fences have no encoder/swapchain owner left to destroy them.
                unsafe { self.device.destroy_fence(submission.fence, None) };
            }
        }
        persist_pipeline_cache(
            &self.device,
            self.pipeline_cache,
            self.pipeline_cache_path.as_deref(),
            self.pipeline_cache_limit_bytes,
        );
        for resource in self.deferred_destroys.drain_all() {
            let _ = self.destroy_deferred_now(resource);
        }
        for (_, buffer) in self.buffers.drain_live() {
            let _ = self.destroy_buffer_now(buffer);
        }
        let upload_pages = std::mem::take(&mut self.upload_pages);
        for buffer in upload_pages.into_iter().flatten() {
            let _ = self.destroy_buffer_now(buffer);
        }
        for commands in std::mem::take(&mut self.upload_command_pool) {
            destroy_upload_commands(&self.device, &commands);
        }
        for (_, set) in self.bind_groups.drain_live() {
            self.destroy_resource_set_now(&set);
        }
        for (_, shader) in self.shader_modules.drain_live() {
            self.destroy_shader_module_now(&shader);
        }
        for (_, pipeline) in self.render_pipelines.drain_live() {
            self.destroy_render_pipeline_now(&pipeline);
        }
        for (_, layout) in self.pipeline_layouts.drain_live() {
            self.destroy_pipeline_layout_now(&layout);
        }
        drop(self.render_passes.drain_live());
        for (_, layout) in self.bind_group_layouts.drain_live() {
            self.destroy_resource_set_layout_now(&layout);
        }
        for (_, sampler) in self.samplers.drain_live() {
            self.destroy_sampler_now(sampler);
        }
        for (_, view) in self.texture_views.drain_live() {
            self.destroy_texture_view_now(&view);
        }
        for (_, texture) in self.textures.drain_live() {
            let _ = self.destroy_texture_now(texture);
        }
        for (_, encoder) in self.command_encoders.drain_live() {
            self.destroy_command_encoder_now(&encoder);
        }
        for encoder in std::mem::take(&mut self.free_command_encoders) {
            self.destroy_command_encoder_now(&encoder);
        }
        for (_, swapchain) in self.swapchains.drain_live() {
            self.destroy_swapchain_now(swapchain);
        }
        for (_, surface) in self.surfaces.drain_live() {
            self.destroy_surface_now(surface);
        }
        self.descriptor_pools.destroy(&self.device);
        self.compute.clear();
        // SAFETY: Pipeline cache belongs to this device and no pipelines remain live.
        unsafe {
            self.device
                .destroy_pipeline_cache(self.pipeline_cache, None);
        }
        // SAFETY: Every allocation-backed resource was destroyed above. The allocator must free
        // retained suballocation blocks while VkDevice is still alive.
        unsafe { ManuallyDrop::drop(&mut self.allocator) };
        // SAFETY: All resources using this device and instance have been destroyed above.
        unsafe {
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

/// Configuration for a Vulkan triangle context.
#[derive(Clone, Debug)]
pub struct VulkanTriangleConfig {
    /// Application name reported to Vulkan.
    pub application_name: String,
    /// Initial surface configuration.
    pub surface_config: SurfaceConfig,
    /// Vertex shader.
    pub vertex_shader: ShaderBinary,
    /// Fragment shader.
    pub fragment_shader: ShaderBinary,
}

/// Compatibility triangle context implemented on top of the resource layer.
pub struct VulkanTriangle {
    device: VulkanDevice,
    surface: SurfaceId,
    swapchain: gfx_core::SwapchainId,
    render_pass: RenderPassId,
    pipeline: RenderPipelineId,
    metrics_started_at: Instant,
    metrics: BaselineMetrics,
}

impl VulkanTriangle {
    /// Creates a Vulkan triangle context for a native window.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if Vulkan initialization or resource creation fails.
    pub fn new<W>(window: &W, config: &VulkanTriangleConfig) -> Result<Self>
    where
        W: HasDisplayHandle + HasWindowHandle,
    {
        if config.vertex_shader.stage != ShaderStage::Vertex {
            return Err(Error::InvalidInput(
                "vertex_shader must use ShaderStage::Vertex".to_string(),
            ));
        }
        if config.fragment_shader.stage != ShaderStage::Fragment {
            return Err(Error::InvalidInput(
                "fragment_shader must use ShaderStage::Fragment".to_string(),
            ));
        }

        let metrics_started_at = Instant::now();
        let mut device = VulkanDevice::new(&DeviceDescriptor {
            application_name: config.application_name.clone(),
            ..DeviceDescriptor::default()
        })?;
        let surface = device.create_surface(window, &SurfaceDescriptor { label: None })?;
        let swapchain = device.create_swapchain(surface, config.surface_config)?;
        let vertex_shader = device.create_shader_module(&ShaderModuleDescriptor {
            label: Some("triangle vertex shader".to_string()),
            binary: config.vertex_shader.clone(),
        })?;
        let fragment_shader = device.create_shader_module(&ShaderModuleDescriptor {
            label: Some("triangle fragment shader".to_string()),
            binary: config.fragment_shader.clone(),
        })?;
        let render_pass = device.create_render_pass(&RenderPassDescriptor {
            label: Some("triangle render pass".to_string()),
            color_attachments: vec![ColorAttachmentDescriptor {
                format: config.surface_config.format,
            }],
            depth_attachment: None,

            sample_count: 1,
            resolve_target: None,
        })?;
        let pipeline = device.create_render_pipeline(
            &RenderPipelineDescriptor {
                label: Some("triangle pipeline".to_string()),
                vertex_shader,
                vertex_entry_point: config.vertex_shader.entry_point.clone(),
                fragment_shader,
                fragment_entry_point: config.fragment_shader.entry_point.clone(),
                vertex_buffers: Vec::new(),
                render_pass,
                pipeline_layout: None,
                color_targets: vec![ColorTargetState::new(
                    config.surface_config.format,
                    BlendMode::Replace,
                )],
                primitive_topology: PrimitiveTopology::TriangleList,
                raster: Default::default(),
                depth_state: None,
            },
            config.surface_config.size,
        )?;
        let metrics = BaselineMetrics {
            startup_time: metrics_started_at.elapsed(),
            ..BaselineMetrics::default()
        };
        Ok(Self {
            device,
            surface,
            swapchain,
            render_pass,
            pipeline,
            metrics_started_at,
            metrics,
        })
    }

    /// Returns current baseline metrics.
    #[must_use]
    pub fn metrics(&self) -> &BaselineMetrics {
        &self.metrics
    }

    /// Recreates size-dependent surface resources.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] if surface recreation fails.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        self.device.resize_swapchain(self.swapchain, width, height)
    }

    /// Draws and presents one triangle frame.
    ///
    /// # Errors
    ///
    /// Returns [`Error`] when acquire, submit, or present fails.
    pub fn draw_triangle(&mut self, clear_color: ClearColor) -> Result<()> {
        let command = RenderCommand::Draw(DrawCommand {
            pipeline: self.pipeline,
            bind_groups: gfx_core::bind_group_list_empty(),
            vertex_count: 3,
            first_vertex: 0,
            instance_count: 1,
            first_instance: 0,
            scissor: None,
        });
        let frame = PresentFrameDescriptor {
            swapchain: self.swapchain,
            render_pass: self.render_pass,
            commands: std::slice::from_ref(&command),
            color_load_op: LoadOp::Clear(clear_color),
            depth_attachment: None,
            present_region: None,
        };
        self.device.render_and_present(&frame)?;
        self.metrics.submitted_frames = self.metrics.submitted_frames.saturating_add(1);
        if self.metrics.first_frame_time.is_none() {
            self.metrics.first_frame_time = Some(self.metrics_started_at.elapsed());
        }
        Ok(())
    }

    /// Returns backend resource statistics.
    #[must_use]
    #[expect(
        dead_code,
        reason = "kept for compatibility diagnostics in triangle smoke helpers"
    )]
    fn resource_stats(&self) -> ResourceStats {
        let _ = self.surface;
        self.device.resource_stats()
    }
}

#[derive(Clone, Copy)]
struct QueueFamilyIndices {
    graphics: u32,
    present: u32,
    /// A distinct COMPUTE-capable, non-GRAPHICS family. Never used for
    /// resource access unless buffer sharing and producer/consumer sync
    /// are explicitly established.
    compute: Option<u32>,
}

struct SwapchainSupport {
    capabilities: vk::SurfaceCapabilitiesKHR,
    formats: Vec<vk::SurfaceFormatKHR>,
    present_modes: Vec<vk::PresentModeKHR>,
}

struct VulkanBuffer {
    buffer: vk::Buffer,
    allocation: MemoryAllocation,
    desc: BufferDescriptor,
}

struct VulkanTexture {
    image: vk::Image,
    allocation: MemoryAllocation,
    desc: TextureDescriptor,
    layout: vk::ImageLayout,
}

struct VulkanTextureUpload {
    texture: TextureId,
    source: vk::Buffer,
    image: vk::Image,
    old_layout: vk::ImageLayout,
    mip_level_count: u32,
    layout: TextureDataLayout,
    mip_level: u32,
    layer: u32,
    origin: gfx_core::Origin2d,
    size: gfx_core::Extent2d,
}

struct VulkanTextureWritePlan<'a> {
    write: gfx_core::TextureWrite<'a>,
    image: vk::Image,
    old_layout: vk::ImageLayout,
    mip_level_count: u32,
}

/// Native Render Graph access tracking for ordered physical color targets.
/// A logical Blur variant is not a native allocation: different variants may
/// resolve to the same TextureId after transient scratch aliasing.
fn graph_color_write_requires_barrier(
    previous_writers: &mut Vec<TextureId>,
    texture: TextureId,
) -> bool {
    if previous_writers.contains(&texture) {
        true
    } else {
        previous_writers.push(texture);
        false
    }
}

fn texture_uploads_overlap(first: &VulkanTextureUpload, second: &VulkanTextureUpload) -> bool {
    first.mip_level == second.mip_level
        && first.layer == second.layer
        && u64::from(first.origin.x) < u64::from(second.origin.x) + u64::from(second.size.width())
        && u64::from(second.origin.x) < u64::from(first.origin.x) + u64::from(first.size.width())
        && u64::from(first.origin.y) < u64::from(second.origin.y) + u64::from(second.size.height())
        && u64::from(second.origin.y) < u64::from(first.origin.y) + u64::from(first.size.height())
}

struct VulkanTextureUploadGroup<'a> {
    texture: TextureId,
    image: vk::Image,
    old_layout: vk::ImageLayout,
    mip_level_count: u32,
    uploads: Vec<&'a VulkanTextureUpload>,
}

fn texture_upload_groups(uploads: &[VulkanTextureUpload]) -> Vec<VulkanTextureUploadGroup<'_>> {
    let mut groups: Vec<VulkanTextureUploadGroup<'_>> = Vec::new();
    for upload in uploads {
        let mut group_index = None;
        for (index, group) in groups.iter().enumerate() {
            if group.texture == upload.texture {
                group_index = Some(index);
                break;
            }
        }
        if let Some(index) = group_index {
            groups[index].uploads.push(upload);
        } else {
            groups.push(VulkanTextureUploadGroup {
                texture: upload.texture,
                image: upload.image,
                old_layout: upload.old_layout,
                mip_level_count: upload.mip_level_count,
                uploads: vec![upload],
            });
        }
    }
    groups
}

fn texture_staging_layout(
    allocation: UploadAllocation,
    descriptor: TextureWriteDescriptor,
) -> Result<TextureDataLayout> {
    TextureDataLayout::new(
        allocation
            .offset
            .checked_add(descriptor.layout.offset)
            .ok_or_else(|| Error::InvalidInput("texture upload offset overflow".to_string()))?,
        descriptor.layout.bytes_per_row.get(),
        descriptor.layout.rows_per_image.get(),
    )
}

fn validate_texture_staging_layout(
    descriptor: TextureWriteDescriptor,
    format: Format,
) -> Result<()> {
    if descriptor.layout.offset % 4 != 0 {
        return Err(Error::InvalidInput(
            "Vulkan texture upload offset must be a multiple of 4 bytes".to_string(),
        ));
    }
    if descriptor.layout.bytes_per_row.get() % format.bytes_per_pixel() != 0 {
        return Err(Error::InvalidInput(
            "Vulkan texture upload bytes_per_row must contain whole texels".to_string(),
        ));
    }
    Ok(())
}

#[derive(Clone)]
struct VulkanTextureView {
    view: vk::ImageView,
    texture: TextureId,
    desc: TextureViewDescriptor,
}

#[derive(Clone, Copy)]
struct VulkanSampler {
    sampler: vk::Sampler,
}

#[derive(Clone)]
struct VulkanResourceSetLayout {
    layout: vk::DescriptorSetLayout,
    desc: BindGroupLayoutDescriptor,
}

#[derive(Clone)]
struct VulkanResourceSet {
    sampled_depth: Vec<TextureId>,
    descriptor_set: vk::DescriptorSet,
    descriptor_pool: vk::DescriptorPool,
    descriptor_demand: descriptors::Demand,
    layout: BindGroupLayoutId,
    desc: BindGroupDescriptor,
}

#[derive(Clone)]
struct VulkanPipelineLayout {
    layout: vk::PipelineLayout,
    bind_group_layouts: Vec<BindGroupLayoutId>,
}

#[derive(Clone)]
struct VulkanShaderModule {
    module: vk::ShaderModule,
    stage: ShaderStage,
    entry_point: String,
}

#[derive(Clone)]
struct VulkanRenderPass {
    color_formats: Vec<Format>,
    depth_format: Option<Format>,
    sample_count: u32,
}

fn rendering_attachment(
    view: vk::ImageView, layout: vk::ImageLayout, load: bool, clear: vk::ClearValue,
) -> vk::RenderingAttachmentInfo<'static> {
    vk::RenderingAttachmentInfo::default().image_view(view).image_layout(layout)
        .load_op(if load { vk::AttachmentLoadOp::LOAD } else { vk::AttachmentLoadOp::CLEAR })
        .store_op(vk::AttachmentStoreOp::STORE).clear_value(clear)
}

#[derive(Clone, Copy)]
struct VulkanRenderPipeline {
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    owns_pipeline_layout: bool,
}

#[derive(Clone)]
struct VulkanCommandEncoder {
    compute_resources: Vec<Arc<compute::Call>>,
    command_pool: vk::CommandPool,
    queue_family_index: u32,
    command_buffers: Vec<vk::CommandBuffer>,
    fence: vk::Fence,
    owns_fence: bool,
    /// Both binary semaphores belong to this command encoder's native fence.
    /// They are never destroyed until the consumer-side graphics wait ends.
    cross_queue_semaphores: Option<[vk::Semaphore; 2]>,
    /// Exact primary queue completion value; never a WSI retirement proof.
    retire_primary_value: Option<u64>,
    /// One timestamp pair per command buffer; retained until its fence completes.
    gpu_trace_query_pool: Option<vk::QueryPool>,
    gpu_trace_passes: Vec<VulkanGpuPass>,
    sample_main_pass: bool,
    frame_probe_surface: Option<(gfx_core::SwapchainId, vk::SwapchainKHR)>,
    /// Pure indirect-only GPU draw windows recorded in the offscreen pair
    /// formerly unused by frame tracing. One tag per native command buffer.
    gpu_draw_tags: Vec<Option<gfx_core::GpuTimestampTag>>,
    gpu_work_tag: Option<gfx_core::GpuTimestampTag>,
    gpu_work_clock: Option<VulkanTimestampClock>,
    frame_submission: Option<SubmissionId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum VulkanGpuPass {
    Unused,
    Main,
    Offscreen,
}

/// Per-swapchain bounded sampling. Always probe new/unstable surfaces,
/// then keep at least four real samples per sixteen presented frames.
#[derive(Default)]
struct FrameProbeCadence {
    previous_ticks: Option<u64>,
    stable_samples: u8,
    skipped: u8,
    last_attempt: Option<Instant>,
}

impl FrameProbeCadence {
    fn should_sample(&mut self, now: Instant, trace: bool) -> bool {
        if self.last_attempt.is_some_and(|last| now.saturating_duration_since(last) > Duration::from_millis(250)) {
            *self = Self::default();
        }
        self.last_attempt = Some(now);
        if trace || self.stable_samples < 8 || self.skipped >= 3 {
            self.skipped = 0;
            true
        } else {
            self.skipped += 1;
            false
        }
    }

    fn observe(&mut self, ticks: u64) {
        if ticks == 0 {
            self.stable_samples = 0;
            self.previous_ticks = None;
            return;
        }
        let stable = self.previous_ticks.is_some_and(|old| {
            old.abs_diff(ticks) <= old.max(ticks) / 8
        });
        self.stable_samples = if stable { self.stable_samples.saturating_add(1) } else { 0 };
        self.previous_ticks = Some(ticks);
    }
}

#[cfg(test)]
mod frame_probe_cadence_tests {
    use super::*;

    #[test]
    fn stable_sampling_is_bounded_and_cost_change_restores_full_rate() {
        let mut cadence = FrameProbeCadence::default();
        let now = Instant::now();
        for _ in 0..9 {
            assert!(cadence.should_sample(now, false));
            cadence.observe(1000);
        }
        for _ in 0..3 {
            assert!(!cadence.should_sample(now, false));
        }
        assert!(cadence.should_sample(now, false));
        cadence.observe(2000);
        assert!(cadence.should_sample(now, false));
        assert!(cadence.should_sample(now + Duration::from_secs(1), false));
        assert_eq!(cadence.stable_samples, 0);
    }

    #[test]
    fn trace_mode_never_skips_a_probe() {
        let mut cadence = FrameProbeCadence::default();
        let now = Instant::now();
        for _ in 0..32 {
            assert!(cadence.should_sample(now, true));
            cadence.observe(1000);
        }
    }
}

#[derive(Default)]
struct VulkanGpuTraceTotals {
    started: Option<Instant>,
    main_count: u32,
    offscreen_count: u32,
    main_total_us: f64,
    offscreen_total_us: f64,
    main_max_us: f64,
    offscreen_max_us: f64,
}

impl VulkanGpuTraceTotals {
    fn record(&mut self, pass: VulkanGpuPass, microseconds: f64) {
        let (count, total, max) = match pass {
            VulkanGpuPass::Main => (
                &mut self.main_count,
                &mut self.main_total_us,
                &mut self.main_max_us,
            ),
            VulkanGpuPass::Offscreen => (
                &mut self.offscreen_count,
                &mut self.offscreen_total_us,
                &mut self.offscreen_max_us,
            ),
            VulkanGpuPass::Unused => return,
        };
        *count = count.saturating_add(1);
        *total += microseconds;
        *max = (*max).max(microseconds);
    }

    fn log_if_due(&mut self) {
        let now = Instant::now();
        let started = self.started.get_or_insert(now);
        if now.duration_since(*started) < Duration::from_secs(5) {
            return;
        }
        log::trace!(target: "gfx_vulkan::gpu",
            "nova Vulkan GPU timestamp trace (5s, completed submissions): main_passes={} main_gpu_us_avg={:.1} main_gpu_us_max={:.1} offscreen_passes={} offscreen_gpu_us_avg={:.1} offscreen_gpu_us_max={:.1} note=GPU_command_execution_excludes_WSI_present_wait",
            self.main_count,
            self.main_total_us / f64::from(self.main_count.max(1)),
            self.main_max_us,
            self.offscreen_count,
            self.offscreen_total_us / f64::from(self.offscreen_count.max(1)),
            self.offscreen_max_us,
        );
        *self = Self {
            started: Some(now),
            ..Self::default()
        };
    }
}

struct VulkanUploadCommands {
    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    timestamp_queries: Option<vk::QueryPool>,
    gpu_work_tag: Option<gfx_core::GpuTimestampTag>,
}

#[derive(Clone, Copy)]
struct VulkanTimestampClock {
    period_ns: f64,
    valid_bits: u32,
}

impl VulkanTimestampClock {
    fn duration_ticks(self, start: u64, end: u64) -> u64 {
        let mask = if self.valid_bits >= u64::BITS {
            u64::MAX
        } else {
            (1_u64 << self.valid_bits) - 1
        };
        end.wrapping_sub(start) & mask
    }

    #[allow(clippy::cast_precision_loss)]
    fn elapsed_nanoseconds(self, start: u64, end: u64) -> f64 {
        self.duration_ticks(start, end) as f64 * self.period_ns
    }

    /// Converts the nanosecond period to whole ticks per second. Zero means the
    /// driver did not report a usable period.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    fn frequency_hz(self) -> u64 {
        if !self.period_ns.is_finite() || self.period_ns <= 0.0 {
            return 0;
        }
        let frequency = (1_000_000_000.0 / self.period_ns).round();
        if frequency.is_finite() && frequency >= 1.0 {
            frequency as u64
        } else {
            0
        }
    }
}

/// A completed secondary-queue wait owns its binary handshake semaphore.
struct VulkanQueueWait {
    semaphore: vk::Semaphore,
    wait_fence: vk::Fence,
}

#[derive(Clone, Copy)]
struct VulkanSubmission {
    fence: vk::Fence,
    /// Actual native queue that signaled this fence. Never infer this from
    /// the shader stage or SubmissionId arena index.
    queue: gfx_core::GpuQueueId,
    /// A pure queue checkpoint owns its fence directly; command encoders and
    /// swapchains retire their own fences through existing GPU owners.
    owned_by_submission: bool,
    /// Stable primary timeline signal, independent of reusable fences.
    timeline_value: Option<u64>,
}

#[derive(Clone, Copy)]
struct VulkanPresentFrame {
    image_index: u32,
    frame_index: usize,
    image_available: vk::Semaphore,
    render_finished: vk::Semaphore,
    fence: vk::Fence,
    /// A failed queue submit can leave an empty fence-recovery submit pending.
    recovery_pending: bool,
    acquire_fence_wait: Duration,
    image_acquire: Duration,
}

#[derive(Clone, Copy)]
struct VulkanSurface {
    surface: vk::SurfaceKHR,
}

/// A bounded history of successful WSI submissions. IDs are generated
/// independently of GPU fence slots; a graphics fence is not a present credit.
struct PresentCredit {
    next: u64,
    limit: usize,
    history_capacity: usize,
    pending: std::collections::VecDeque<u64>,
    timeout_since: Option<Instant>,
}

impl PresentCredit {
    fn new(limit: usize) -> Self {
        Self {
            next: 1,
            limit: limit.max(1),
            history_capacity: limit.max(1),
            pending: std::collections::VecDeque::with_capacity(limit.max(1)),
            timeout_since: None,
        }
    }

    fn next_id(&mut self) -> u64 {
        let id = self.next;
        self.next = self.next.checked_add(1).expect("Vulkan present ID exhausted");
        id
    }

    fn set_limit(&mut self, limit: usize) {
        // Reducing latency never fabricates completion or drops the already
        // submitted credits. Drain them through native present feedback.
        let limit = limit.max(1);
        if self.limit != limit {
            self.limit = limit;
            self.timeout_since = None;
        }
    }

    fn submitted(&mut self, id: u64) {
        // A direct gfx client may bypass preflight. Keep a bounded history
        // at the configured physical capacity, not the adaptive limit.
        // Waiting for a later ID also proves every earlier presentation.
        if self.pending.len() >= self.history_capacity {
            self.pending.pop_front();
        }
        self.pending.push_back(id);
    }

    fn wait_target(&self) -> Option<u64> {
        // If the limit was reduced with multiple outstanding presents,
        // wait for enough of them to restore a real admission credit.
        self.pending.len().checked_sub(self.limit)
            .and_then(|index| self.pending.get(index).copied())
    }

    fn stalled(&mut self, now: Instant) -> bool {
        let first = *self.timeout_since.get_or_insert(now);
        now.saturating_duration_since(first) >= Duration::from_millis(250)
    }

    fn completed(&mut self, id: u64) {
        self.timeout_since = None;
        while self.pending.front().is_some_and(|pending| *pending <= id) {
            self.pending.pop_front();
        }
    }
}

#[cfg(test)]
mod present_credit_tests {
    use super::{Duration, Instant, PresentCredit};

    #[test]
    fn completed_present_resets_stall_recovery() {
        let mut credit = PresentCredit::new(1);
        let id = credit.next_id();
        credit.submitted(id);
        let now = Instant::now();
        assert!(!credit.stalled(now));
        assert!(!credit.stalled(now + Duration::from_millis(249)));
        assert!(credit.stalled(now + Duration::from_millis(250)));
        credit.completed(id);
        assert!(!credit.stalled(now + Duration::from_secs(1)));
    }

    #[test]
    fn unchanged_present_limit_does_not_restart_stall_deadline() {
        let mut credit = PresentCredit::new(1);
        let id = credit.next_id();
        credit.submitted(id);
        let now = Instant::now();
        assert!(!credit.stalled(now));
        credit.set_limit(1);
        assert!(credit.stalled(now + Duration::from_millis(250)));
    }

    #[test]
    fn shrinking_present_limit_preserves_outstanding_native_credits() {
        let mut credit = PresentCredit::new(2);
        let first = credit.next_id();
        credit.submitted(first);
        let second = credit.next_id();
        credit.submitted(second);
        credit.set_limit(1);
        assert_eq!(credit.wait_target(), Some(second));
        credit.completed(first);
        assert_eq!(credit.wait_target(), Some(second));
        credit.completed(second);
        assert_eq!(credit.wait_target(), None);
        credit.set_limit(2);
        let third = credit.next_id();
        credit.submitted(third);
        assert_eq!(credit.wait_target(), None);
    }

    #[test]
    fn present_credit_limits_successful_submissions_and_ignores_failed_ids() {
        let mut credit = PresentCredit::new(2);
        let first = credit.next_id();
        credit.submitted(first);
        assert_eq!(credit.wait_target(), None);
        let _failed = credit.next_id();
        let third = credit.next_id();
        credit.submitted(third);
        assert_eq!(credit.wait_target(), Some(first));
        credit.completed(first);
        assert_eq!(credit.wait_target(), None);
        let fourth = credit.next_id();
        credit.submitted(fourth);
        assert_eq!(credit.wait_target(), Some(third));
        let fifth = credit.next_id();
        credit.submitted(fifth);
        assert_eq!(credit.wait_target(), Some(fourth));
    }
}

struct VulkanSwapchain {
    presentation: presentation::Retirement,
    present_wake: Option<presentation::PresentWake>,
    next_present_id: u32,
    display_timing_enabled: bool,
    present_credit: Option<PresentCredit>,
    native_pixel_scaling: bool,
    scaled_image_extent_range: Option<(vk::Extent2D, vk::Extent2D)>,
    surface: vk::SurfaceKHR,
    swapchain: vk::SwapchainKHR,
    format: vk::Format,
    extent: vk::Extent2D,
    source_extent: vk::Extent2D,
    images: Vec<vk::Image>,
    image_views: Vec<vk::ImageView>,
    image_available_semaphores: Vec<vk::Semaphore>,
    render_finished_semaphores: Vec<vk::Semaphore>,
    in_flight_fences: Vec<vk::Fence>,
    // Logical completion identity of the last GPU reader of each slot.
    frame_submissions: Vec<Option<SubmissionId>>,
    frame_probe: FrameProbeCadence,
    frame_index: usize,
    reserved_frame: Option<VulkanPresentFrame>,
    acquire_slots: Vec<presentation::AcquireSlot>,
    config: SurfaceConfig,
}

#[derive(Clone, Copy)]
enum CleanupPoint {
    Timeline(u64),
    Fence(vk::Fence),
}

impl CleanupPoint {
    fn destroy(self, device: &ash::Device) {
        if let Self::Fence(fence) = self {
            destroy_fence(device, fence);
        }
    }
}

enum DeferredResource {
    HeapReplacement {
        fence: vk::Fence,
        commands: VulkanUploadCommands,
        replacement: compact::HeapReplacement,
    },
    Buffer {
        retirement: CleanupPoint,
        buffer: VulkanBuffer,
    },
    Buffers {
        retirement: CleanupPoint,
        buffers: Vec<VulkanBuffer>,
    },
    Texture {
        retirement: CleanupPoint,
        texture: VulkanTexture,
    },
    TextureView {
        retirement: CleanupPoint,
        view: VulkanTextureView,
    },
    BindGroup {
        retirement: CleanupPoint,
        set: VulkanResourceSet,
    },
    Upload {
        retirement: CleanupPoint,
        fence_value: u64,
        commands: VulkanUploadCommands,
    },
    CommandEncoder(VulkanCommandEncoder),
}

impl DeferredResource {
    fn primary_retirement_value(&self) -> Option<u64> {
        match self {
            Self::Buffer { retirement: CleanupPoint::Timeline(value), .. }
            | Self::Buffers { retirement: CleanupPoint::Timeline(value), .. }
            | Self::Texture { retirement: CleanupPoint::Timeline(value), .. }
            | Self::TextureView { retirement: CleanupPoint::Timeline(value), .. }
            | Self::BindGroup { retirement: CleanupPoint::Timeline(value), .. }
            | Self::Upload { retirement: CleanupPoint::Timeline(value), .. } => Some(*value),
            Self::CommandEncoder(encoder) => encoder.retire_primary_value,
            _ => None,
        }
    }
}

fn deferred_resource_ready(device: &ash::Device, resource: &DeferredResource) -> bool {
    let fence = match resource {
        DeferredResource::HeapReplacement { fence, .. }
        | DeferredResource::CommandEncoder(VulkanCommandEncoder { fence, .. }) => *fence,
        DeferredResource::Buffer { retirement, .. }
        | DeferredResource::Buffers { retirement, .. }
        | DeferredResource::Texture { retirement, .. }
        | DeferredResource::TextureView { retirement, .. }
        | DeferredResource::BindGroup { retirement, .. }
        | DeferredResource::Upload { retirement, .. } => match retirement {
            CleanupPoint::Fence(fence) => *fence,
            // Only the timeline-aware owner predicate can retire this.
            CleanupPoint::Timeline(_) => return false,
        },
    };
    // Resource retirement publishes logical completion for tracked encoders.
    // Device loss is not successful execution; leave these records for
    // teardown instead of rewriting their submission fence to null.
    fence == vk::Fence::null()
        || matches!(unsafe { device.get_fence_status(fence) }, Ok(true))
}

fn fence_is_complete(device: &ash::Device, fence: vk::Fence) -> bool {
    if fence == vk::Fence::null() {
        return true;
    }
    // SAFETY: Fence belongs to this device and remains live until deferred resource destroy.
    match unsafe { device.get_fence_status(fence) } {
        Ok(complete) => complete,
        Err(vk::Result::ERROR_DEVICE_LOST) => true,
        Err(_) => false,
    }
}

fn destroy_fence(device: &ash::Device, fence: vk::Fence) {
    if fence != vk::Fence::null() {
        // SAFETY: Fence was created by this device and is destroyed once here.
        unsafe { device.destroy_fence(fence, None) };
    }
}

fn vulkan_memory(allocation: &MemoryAllocation) -> Result<(vk::DeviceMemory, u64)> {
    // SAFETY: The caller immediately binds this allocation to the resource that produced
    // the memory requirements used for the allocation.
    unsafe { allocation.vulkan_memory() }
        .ok_or_else(|| Error::Backend("allocation is not a Vulkan allocation".to_string()))
}

fn load_entry() -> Result<Entry> {
    // SAFETY: Loading the Vulkan loader is safe if the process has a valid Vulkan loader.
    unsafe { Entry::load() }.map_err(|error| VulkanError::Loader(error.to_string()).into())
}

fn supports_api_version(version: u32) -> bool {
    vk::api_version_variant(version) == 0
        && version >= REQUIRED_API_VERSION
}

fn negotiate_instance_api(loader_version: u32) -> Option<u32> {
    supports_api_version(loader_version)
        .then(|| loader_version.min(PREFERRED_API_VERSION))
}

fn api_version_string(version: u32) -> String {
    format!("{}.{}.{}", vk::api_version_major(version),
        vk::api_version_minor(version), vk::api_version_patch(version))
}

fn create_instance(entry: &Entry, application_name: &str) -> Result<(Instance, u32)> {
    // SAFETY: the loaded entry remains alive and this query needs no instance.
    let loader_version = unsafe { entry.try_enumerate_instance_version() }
        .map_err(VulkanError::from)?.unwrap_or(vk::API_VERSION_1_0);
    let instance_api = negotiate_instance_api(loader_version).ok_or_else(|| {
        VulkanError::Unavailable(format!(
            "nova-gfx requires Vulkan {} or newer; loader reports {}",
            api_version_string(REQUIRED_API_VERSION), api_version_string(loader_version)
        ))
    })?;
    let app_name =
        CString::new(application_name).map_err(|error| Error::InvalidInput(error.to_string()))?;
    let engine_name =
        CString::new("nova-gfx").map_err(|error| Error::InvalidInput(error.to_string()))?;
    let app_info = vk::ApplicationInfo::default()
        .application_name(&app_name)
        .application_version(vk::make_api_version(0, 0, 1, 0))
        .engine_name(&engine_name)
        .engine_version(vk::make_api_version(0, 0, 1, 0))
        .api_version(instance_api);
    let mut extension_names = instance_extension_names();
    if let Some(maintenance) = presentation::instance_extension(entry)? {
        extension_names.push(khr::get_surface_capabilities2::NAME.as_ptr());
        extension_names.push(maintenance.as_ptr());
    }
    let create_info = vk::InstanceCreateInfo::default()
        .application_info(&app_info)
        .enabled_extension_names(&extension_names);
    // SAFETY: Create info references live CStrings and static extension names.
    let instance = unsafe { entry.create_instance(&create_info, None) }
        .map_err(VulkanError::from)?;
    Ok((instance, instance_api))
}

/// Required core features are queried, not inferred solely from apiVersion.
fn supports_modern_device(
    instance: &Instance, device: vk::PhysicalDevice, properties: &vk::PhysicalDeviceProperties,
) -> bool {
    if !supports_api_version(properties.api_version) {
        return false;
    }
    let mut features12 = vk::PhysicalDeviceVulkan12Features::default();
    let mut features13 = vk::PhysicalDeviceVulkan13Features::default();
    let mut features = vk::PhysicalDeviceFeatures2::default()
        .push_next(&mut features12).push_next(&mut features13);
    // SAFETY: both the physical device and live instance support at least Vulkan 1.3.
    unsafe { instance.get_physical_device_features2(device, &mut features) };
    features12.timeline_semaphore == vk::TRUE
        && features13.synchronization2 == vk::TRUE
        && features13.dynamic_rendering == vk::TRUE
}

fn instance_extension_names() -> Vec<*const i8> {
    let mut names = vec![khr::surface::NAME.as_ptr()];
    #[cfg(windows)]
    names.push(khr::win32_surface::NAME.as_ptr());
    #[cfg(all(unix, not(target_vendor = "apple"), not(target_os = "android")))]
    {
        names.push(khr::xlib_surface::NAME.as_ptr());
        names.push(khr::xcb_surface::NAME.as_ptr());
        names.push(khr::wayland_surface::NAME.as_ptr());
    }
    #[cfg(target_os = "android")]
    names.push(khr::android_surface::NAME.as_ptr());
    #[cfg(target_vendor = "apple")]
    names.push(ash::ext::metal_surface::NAME.as_ptr());
    names
}

fn pick_physical_device_without_surface(
    instance: &Instance,
    desc: &DeviceDescriptor,
) -> Result<vk::PhysicalDevice> {
    // SAFETY: Instance is valid.
    let devices = unsafe { instance.enumerate_physical_devices() }.map_err(VulkanError::from)?;
    let requested_name = desc
        .adapter_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty());
    let mut candidates = devices
        .into_iter()
        .filter_map(|device| {
            // SAFETY: physical device belongs to this instance.
            let properties = unsafe { instance.get_physical_device_properties(device) };
            if !supports_modern_device(instance, device, &properties) {
                log::debug!("skipping Vulkan adapter {} (API {}, requires {} and core features)",
                    physical_device_name(&properties), api_version_string(properties.api_version),
                    api_version_string(REQUIRED_API_VERSION));
                return None;
            }
            queue_family_indices_without_surface(instance, device).is_ok()
                .then_some((device, properties))
        })
        .collect::<Vec<_>>();

    if let Some(requested_name) = requested_name {
        return candidates
            .into_iter()
            .find_map(|(device, properties)| {
                physical_device_name(&properties)
                    .eq_ignore_ascii_case(requested_name)
                    .then_some(device)
            })
            .ok_or_else(|| {
                VulkanError::Unavailable(format!(
                    "requested Vulkan adapter is unavailable or lacks the Vulkan {} baseline: {requested_name}",
                    api_version_string(REQUIRED_API_VERSION)
                ))
                .into()
            });
    }

    candidates.sort_by_key(|(_, properties)| {
        physical_device_preference_rank(properties.device_type, desc.power_preference)
    });
    candidates
        .into_iter()
        .next()
        .map(|(device, _)| device)
        .ok_or_else(|| {
            VulkanError::Unavailable(format!(
                "no suitable Vulkan {} physical device with timeline semaphore, synchronization2 and dynamic rendering",
                api_version_string(REQUIRED_API_VERSION)
            )).into()
        })
}

fn physical_device_name(properties: &vk::PhysicalDeviceProperties) -> String {
    // SAFETY: Vulkan guarantees a null-terminated device name in this fixed-size field.
    unsafe { CStr::from_ptr(properties.device_name.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

fn physical_device_preference_rank(
    device_type: vk::PhysicalDeviceType,
    power_preference: PowerPreference,
) -> u8 {
    match power_preference {
        PowerPreference::LowPower => match device_type {
            vk::PhysicalDeviceType::INTEGRATED_GPU => 0,
            vk::PhysicalDeviceType::VIRTUAL_GPU => 1,
            vk::PhysicalDeviceType::DISCRETE_GPU => 2,
            vk::PhysicalDeviceType::CPU => 4,
            _ => 3,
        },
        PowerPreference::HighPerformance => match device_type {
            vk::PhysicalDeviceType::DISCRETE_GPU => 0,
            vk::PhysicalDeviceType::INTEGRATED_GPU => 1,
            vk::PhysicalDeviceType::VIRTUAL_GPU => 2,
            vk::PhysicalDeviceType::CPU => 4,
            _ => 3,
        },
    }
}

/// Select a real non-graphics COMPUTE family, never a family advertising
/// COMPUTE without any allocatable queues. Do not silently assume every
/// physical adapter has a dedicated compute queue.
fn select_compute_only_family(families: &[vk::QueueFamilyProperties]) -> Option<u32> {
    families.iter().enumerate().find_map(|(index, family)| {
        (family.queue_count > 0
            && family.queue_flags.contains(vk::QueueFlags::COMPUTE)
            && !family.queue_flags.contains(vk::QueueFlags::GRAPHICS))
        .then(|| u32::try_from(index).ok())
        .flatten()
    })
}

/// Concurrent sharing is a correctness requirement when two queue families
/// access the same buffer. Avoid its potential cache overhead when a second
/// same-family queue suffices. EXCLUSIVE images are never handed to compute.
fn concurrent_compute_sharing_required(
    sidecar_graphics_queue: bool,
    compute_family: Option<u32>,
    usage: BufferUsage,
) -> bool {
    !sidecar_graphics_queue
        && compute_family.is_some()
        && usage.intersects(BufferUsage::STORAGE | BufferUsage::UNIFORM)
}

fn queue_family_indices_without_surface(
    instance: &Instance,
    physical_device: vk::PhysicalDevice,
) -> Result<QueueFamilyIndices> {
    // SAFETY: Physical device belongs to this instance.
    let families = unsafe { instance.get_physical_device_queue_family_properties(physical_device) };
    let graphics = families
        .iter()
        .enumerate()
        .find_map(|(index, family)| {
            family
                .queue_flags
                .contains(vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE)
                .then_some(index)
        })
        .or_else(|| {
            families
                .iter()
                .position(|family| family.queue_flags.contains(vk::QueueFlags::GRAPHICS))
        })
        .ok_or_else(|| VulkanError::Unavailable("no graphics queue family".to_string()))?;
    let graphics = u32::try_from(graphics)
        .map_err(|error| Error::Backend(format!("queue family index overflow: {error}")))?;
    let compute = select_compute_only_family(&families);
    Ok(QueueFamilyIndices {
        graphics,
        present: graphics,
        compute,
    })
}

/// The second graphics-family queue is only requested if the physical
/// adapter exposes it. Distinct compute-family queues are requested
/// separately; their shared storage buffers use CONCURRENT mode and native
/// semaphores, never undefined EXCLUSIVE ownership transitions.
fn independent_graphics_queue_enabled(available: u32) -> bool {
    available >= 2
}

fn create_device(
    instance: &Instance,
    physical_device: vk::PhysicalDevice,
    indices: QueueFamilyIndices,
    entry: &Entry,
) -> Result<(
    ash::Device,
    vk::Queue,
    Option<vk::Queue>,
    Option<vk::Queue>,
    vk::Queue,
    bool,
    bool,
    bool,
    bool,
    bool,
    bool,
    bool,
    bool,
)> {
    let priorities = [1.0_f32, 0.75_f32];
    let single_priority = [1.0_f32];
    // Never request index 1 on a one-queue adapter. A separate Compute
    // family has its own index-0 queue and uses concurrent-shared buffers.
    let queue_families =
        unsafe { instance.get_physical_device_queue_family_properties(physical_device) };
    let graphics_queue_count = queue_families
        .get(indices.graphics as usize)
        .map_or(0, |properties| properties.queue_count);
    // The secondary native graphics queue is used automatically on a
    // capable adapter. One-queue devices retain the ordered submission
    // path without requiring a flag or separate queue-family transfers.
    let dedicated = independent_graphics_queue_enabled(graphics_queue_count);
    // On adapters providing both, avoid requesting and maintaining a third
    // queue needlessly. Same-family execution has lower synchronization
    // overhead and does not require CONCURRENT buffer allocations.
    let selected_compute = if dedicated { None } else { indices.compute };
    let mut unique_families = vec![indices.graphics];
    if indices.present != indices.graphics {
        unique_families.push(indices.present);
    }
    if let Some(compute) = selected_compute
        && !unique_families.contains(&compute)
    {
        unique_families.push(compute);
    }
    let queue_infos = unique_families
        .iter()
        .copied()
        .map(|queue_family_index| {
            let priorities_for_family = if dedicated && queue_family_index == indices.graphics {
                &priorities[..]
            } else {
                &single_priority[..]
            };
            vk::DeviceQueueCreateInfo::default()
                .queue_family_index(queue_family_index)
                .queue_priorities(priorities_for_family)
        })
        .collect::<Vec<_>>();
    // SAFETY: Extension properties are owned values returned for this physical device.
    let available_extensions =
        unsafe { instance.enumerate_device_extension_properties(physical_device) }
            .map_err(VulkanError::from)?;
    let incremental_presentation = available_extensions.iter().any(|extension| {
        // SAFETY: Vulkan guarantees a null-terminated extension name in this fixed-size field.
        let name = unsafe { CStr::from_ptr(extension.extension_name.as_ptr()) };
        name == khr::incremental_present::NAME
    });
    let mut device_extensions = vec![khr::swapchain::NAME.as_ptr()];
    let display_timing_supported = available_extensions.iter().any(|extension| {
        // SAFETY: Vulkan extension names are null-terminated within the fixed array.
        (unsafe { CStr::from_ptr(extension.extension_name.as_ptr()) })
            == ash::google::display_timing::NAME
    });
    if display_timing_supported {
        device_extensions.push(ash::google::display_timing::NAME.as_ptr());
    }
    let memory_budget_supported = available_extensions.iter().any(|extension| {
            // SAFETY: Vulkan extension names are null-terminated within this fixed array.
            (unsafe { CStr::from_ptr(extension.extension_name.as_ptr()) })
                == ash::ext::memory_budget::NAME
        });
    if memory_budget_supported {
        device_extensions.push(ash::ext::memory_budget::NAME.as_ptr());
    }
    if incremental_presentation {
        device_extensions.push(khr::incremental_present::NAME.as_ptr());
    }
    let supported_features = unsafe { instance.get_physical_device_features(physical_device) };
    let mut enabled_features = vk::PhysicalDeviceFeatures::default();
    enabled_features.draw_indirect_first_instance = supported_features.draw_indirect_first_instance;
    enabled_features.multi_draw_indirect = supported_features.multi_draw_indirect;
    let supports_sampler_anisotropy = supported_features.sampler_anisotropy == vk::TRUE;
    enabled_features.sampler_anisotropy = supported_features.sampler_anisotropy;
    #[cfg(target_os = "windows")]
    {
        // Windows Nova has a single strict text contract: RGB subpixel composition requires
        // independent destination attenuation for R/G/B, which Vulkan exposes through dualSrcBlend.
        if supported_features.dual_src_blend != vk::TRUE {
            return Err(VulkanError::Unavailable(
                "Windows Nova Vulkan requires dualSrcBlend for RGB subpixel text".to_string(),
            )
            .into());
        }
        enabled_features.dual_src_blend = vk::TRUE;
    }
    let maintenance = presentation::device_extension(
        presentation::instance_extension(entry)?,
        &available_extensions,
    );
    let mut maintenance_features = vk::PhysicalDeviceSwapchainMaintenance1FeaturesEXT::default();
    if maintenance.is_some() {
        let mut features =
            vk::PhysicalDeviceFeatures2::default().push_next(&mut maintenance_features);
        // SAFETY: the Vulkan 1.3 core query accepts this advertised extension feature.
        unsafe { instance.get_physical_device_features2(physical_device, &mut features) };
    }
    let native_pixel_presentation = maintenance_features.swapchain_maintenance1 == vk::TRUE;
    if native_pixel_presentation {
        device_extensions.push(
            maintenance
                .expect("queried maintenance feature has an extension")
                .as_ptr(),
        );
    }
    let has_extension = |name: &CStr| available_extensions.iter().any(|extension| {
        (unsafe { CStr::from_ptr(extension.extension_name.as_ptr()) }) == name
    });
    // The adapter filter verified these promoted core features. Enable them
    // explicitly; no KHR timeline extension name or suffixed command is needed.
    let mut available12 = vk::PhysicalDeviceVulkan12Features::default();
    let mut available = vk::PhysicalDeviceFeatures2::default().push_next(&mut available12);
    // SAFETY: Vulkan 1.2 is core on the selected physical device.
    unsafe { instance.get_physical_device_features2(physical_device, &mut available) };
    let host_query_reset = available12.host_query_reset == vk::TRUE;
    let draw_indirect_count = available12.draw_indirect_count == vk::TRUE
        && supported_features.multi_draw_indirect == vk::TRUE
        && supported_features.draw_indirect_first_instance == vk::TRUE;
    let mut features12 = vk::PhysicalDeviceVulkan12Features::default()
        .timeline_semaphore(true).host_query_reset(host_query_reset)
        .draw_indirect_count(draw_indirect_count);
    let mut features13 = vk::PhysicalDeviceVulkan13Features::default()
        .synchronization2(true).dynamic_rendering(true);
    let mut present_id_features = vk::PhysicalDevicePresentIdFeaturesKHR::default();
    let mut present_wait_features = vk::PhysicalDevicePresentWaitFeaturesKHR::default();
    if has_extension(khr::present_id::NAME)
        && has_extension(khr::present_wait::NAME)
    {
        let mut features = vk::PhysicalDeviceFeatures2::default()
            .push_next(&mut present_id_features)
            .push_next(&mut present_wait_features);
        // SAFETY: both feature structures correspond to advertised extensions.
        unsafe { instance.get_physical_device_features2(physical_device, &mut features) };
    }
    let present_wait_supported = present_id_features.present_id == vk::TRUE
        && present_wait_features.present_wait == vk::TRUE;
    if present_wait_supported {
        device_extensions.push(khr::present_id::NAME.as_ptr());
        device_extensions.push(khr::present_wait::NAME.as_ptr());
    }
    let mut create_info = vk::DeviceCreateInfo::default()
        .queue_create_infos(&queue_infos)
        .enabled_extension_names(&device_extensions)
        .enabled_features(&enabled_features);
    if native_pixel_presentation {
        create_info = create_info.push_next(&mut maintenance_features);
    }
    if present_wait_supported {
        create_info = create_info.push_next(&mut present_id_features)
            .push_next(&mut present_wait_features);
    }
    create_info = create_info.push_next(&mut features12).push_next(&mut features13);
    // SAFETY: Physical device and queue info were selected from this instance.
    let device = unsafe { instance.create_device(physical_device, &create_info, None) }
        .map_err(VulkanError::from)?;
    // SAFETY: Queue family index and queue index are valid per device creation.
    let graphics_queue = unsafe { device.get_device_queue(indices.graphics, 0) };
    // Queue index 1 is requested only when the physical family exposes it.
    let sidecar_queue = dedicated.then(|| unsafe { device.get_device_queue(indices.graphics, 1) });
    if dedicated {
        log::info!(
            "nova-gfx Vulkan second graphics queue enabled in family {}",
            indices.graphics
        );
    }
    let dedicated_compute_queue = selected_compute.map(|family| {
        // SAFETY: The compute-family index-zero queue was requested above.
        unsafe { device.get_device_queue(family, 0) }
    });
    if let Some(family) = selected_compute {
        log::info!(
            "nova-gfx Vulkan separate compute queue enabled family={} graphics_family={}",
            family,
            indices.graphics
        );
    }
    // SAFETY: Queue family index and queue index are valid per device creation.
    let present_queue = unsafe { device.get_device_queue(indices.present, 0) };
    Ok((
        device,
        graphics_queue,
        sidecar_queue,
        dedicated_compute_queue,
        present_queue,
        incremental_presentation,
        supports_sampler_anisotropy,
        memory_budget_supported,
        native_pixel_presentation,
        display_timing_supported,
        present_wait_supported,
        host_query_reset,
        draw_indirect_count,
    ))
}

fn query_swapchain_support(
    physical_device: vk::PhysicalDevice,
    surface_loader: &khr::surface::Instance,
    surface: vk::SurfaceKHR,
) -> Result<SwapchainSupport> {
    // SAFETY: Physical device and surface are valid for this surface loader.
    let capabilities = unsafe {
        surface_loader.get_physical_device_surface_capabilities(physical_device, surface)
    }
    .map_err(VulkanError::from)?;
    // SAFETY: Physical device and surface are valid for this surface loader.
    let formats =
        unsafe { surface_loader.get_physical_device_surface_formats(physical_device, surface) }
            .map_err(VulkanError::from)?;
    // SAFETY: Physical device and surface are valid for this surface loader.
    let present_modes = unsafe {
        surface_loader.get_physical_device_surface_present_modes(physical_device, surface)
    }
    .map_err(VulkanError::from)?;
    Ok(SwapchainSupport {
        capabilities,
        formats,
        present_modes,
    })
}

fn choose_surface_format(
    formats: &[vk::SurfaceFormatKHR],
    preferred: Format,
) -> vk::SurfaceFormatKHR {
    let preferred_format = format_to_vk(preferred);
    formats
        .iter()
        .copied()
        .find(|format| format.format == preferred_format)
        .or_else(|| {
            formats.iter().copied().find(|format| {
                format.format == vk::Format::B8G8R8A8_UNORM
                    || format.format == vk::Format::B8G8R8A8_SRGB
            })
        })
        .unwrap_or(vk::SurfaceFormatKHR {
            format: vk::Format::B8G8R8A8_UNORM,
            color_space: vk::ColorSpaceKHR::SRGB_NONLINEAR,
        })
}

fn choose_present_mode(modes: &[vk::PresentModeKHR], preferred: PresentMode) -> vk::PresentModeKHR {
    let preferred = match preferred {
        PresentMode::Fifo => vk::PresentModeKHR::FIFO,
        PresentMode::Mailbox => vk::PresentModeKHR::MAILBOX,
        PresentMode::Immediate => vk::PresentModeKHR::IMMEDIATE,
    };
    if modes.contains(&preferred) {
        preferred
    } else {
        vk::PresentModeKHR::FIFO
    }
}

fn choose_composite_alpha_mode(
    supported: vk::CompositeAlphaFlagsKHR,
    preferred: CompositeAlphaMode,
) -> Result<CompositeAlphaMode> {
    let candidates: &[CompositeAlphaMode] = match preferred {
        CompositeAlphaMode::Auto => &[
            CompositeAlphaMode::Opaque,
            CompositeAlphaMode::Premultiplied,
            CompositeAlphaMode::Postmultiplied,
            CompositeAlphaMode::Inherit,
        ],
        CompositeAlphaMode::Opaque => &[CompositeAlphaMode::Opaque],
        CompositeAlphaMode::Premultiplied => &[
            CompositeAlphaMode::Premultiplied,
            CompositeAlphaMode::Postmultiplied,
            CompositeAlphaMode::Inherit,
        ],
        CompositeAlphaMode::Postmultiplied => &[CompositeAlphaMode::Postmultiplied],
        CompositeAlphaMode::Inherit => &[CompositeAlphaMode::Inherit],
    };
    candidates
        .iter()
        .copied()
        .find(|candidate| supported.contains(composite_alpha_to_vulkan(*candidate)))
        .ok_or_else(|| {
            Error::Unavailable(format!(
                "Vulkan surface does not support requested composite alpha mode {preferred:?}; supported flags: {supported:?}"
            ))
        })
}

fn composite_alpha_to_vulkan(mode: CompositeAlphaMode) -> vk::CompositeAlphaFlagsKHR {
    match mode {
        CompositeAlphaMode::Auto | CompositeAlphaMode::Opaque => vk::CompositeAlphaFlagsKHR::OPAQUE,
        CompositeAlphaMode::Premultiplied => vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
        CompositeAlphaMode::Postmultiplied => vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
        CompositeAlphaMode::Inherit => vk::CompositeAlphaFlagsKHR::INHERIT,
    }
}

fn choose_extent(
    capabilities: &vk::SurfaceCapabilitiesKHR,
    width: u32,
    height: u32,
) -> vk::Extent2D {
    if capabilities.current_extent.width == u32::MAX {
        vk::Extent2D {
            width: width.clamp(
                capabilities.min_image_extent.width,
                capabilities.max_image_extent.width,
            ),
            height: height.clamp(
                capabilities.min_image_extent.height,
                capabilities.max_image_extent.height,
            ),
        }
    } else {
        capabilities.current_extent
    }
}

fn create_image_view(
    device: &ash::Device,
    image: vk::Image,
    format: vk::Format,
    aspect_mask: vk::ImageAspectFlags,
) -> Result<vk::ImageView> {
    create_image_view_2d(device, image, format, aspect_mask, 0, 1)
}

fn create_image_view_array(
    device: &ash::Device,
    image: vk::Image,
    format: vk::Format,
    aspect_mask: vk::ImageAspectFlags,
    base_mip_level: u32,
    mip_level_count: u32,
    layers: u32,
) -> Result<vk::ImageView> {
    let range = vk::ImageSubresourceRange::default()
        .aspect_mask(aspect_mask)
        .base_mip_level(base_mip_level)
        .level_count(mip_level_count)
        .base_array_layer(0)
        .layer_count(layers);
    let create_info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D_ARRAY)
        .format(format)
        .subresource_range(range);
    unsafe { device.create_image_view(&create_info, None) }
        .map_err(|error| VulkanError::from(error).into())
}

fn create_image_view_2d(
    device: &ash::Device,
    image: vk::Image,
    format: vk::Format,
    aspect_mask: vk::ImageAspectFlags,
    base_mip_level: u32,
    mip_level_count: u32,
) -> Result<vk::ImageView> {
    let subresource_range = vk::ImageSubresourceRange::default()
        .aspect_mask(aspect_mask)
        .base_mip_level(base_mip_level)
        .level_count(mip_level_count)
        .base_array_layer(0)
        .layer_count(1);
    let create_info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(format)
        .subresource_range(subresource_range);
    // SAFETY: Image belongs to this device and view create info matches it.
    unsafe { device.create_image_view(&create_info, None) }
        .map_err(|error| VulkanError::from(error).into())
}

fn create_empty_pipeline_layout(device: &ash::Device) -> Result<vk::PipelineLayout> {
    let info = vk::PipelineLayoutCreateInfo::default();
    // SAFETY: an empty layout has no descriptor sets or push constant ranges.
    unsafe { device.create_pipeline_layout(&info, None) }
        .map_err(|error| VulkanError::from(error).into())
}

struct GraphicsPipelineBuild<'a> {
    device: &'a ash::Device,
    pipeline_cache: vk::PipelineCache,
    depth_format: Option<Format>,
    pipeline_layout: vk::PipelineLayout,
    vertex_shader: &'a VulkanShaderModule,
    vertex_entry_point: &'a str,
    fragment_shader: &'a VulkanShaderModule,
    fragment_entry_point: &'a str,
    sample_count: u32,
    desc: &'a RenderPipelineDescriptor,
}

#[expect(
    clippy::too_many_lines,
    reason = "Vulkan graphics pipeline construction is kept together at the FFI boundary"
)]
fn create_graphics_pipeline(
    build: &GraphicsPipelineBuild<'_>,
) -> Result<(vk::PipelineLayout, vk::Pipeline)> {
    let _ = (
        &build.vertex_shader.entry_point,
        &build.fragment_shader.entry_point,
    );
    let vertex_entry = CString::new(build.vertex_entry_point)
        .map_err(|error| Error::InvalidInput(error.to_string()))?;
    let fragment_entry = CString::new(build.fragment_entry_point)
        .map_err(|error| Error::InvalidInput(error.to_string()))?;
    let shader_stages = [
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::VERTEX)
            .module(build.vertex_shader.module)
            .name(&vertex_entry),
        vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::FRAGMENT)
            .module(build.fragment_shader.module)
            .name(&fragment_entry),
    ];
    let vertex_bindings = build
        .desc
        .vertex_buffers
        .iter()
        .enumerate()
        .map(|(binding, layout)| {
            let binding = u32::try_from(binding).unwrap_or(u32::MAX);
            vk::VertexInputBindingDescription::default()
                .binding(binding)
                .stride(layout.stride)
                .input_rate(vk::VertexInputRate::VERTEX)
        })
        .collect::<Vec<_>>();
    let vertex_attributes = build
        .desc
        .vertex_buffers
        .iter()
        .enumerate()
        .flat_map(|(binding, layout)| {
            let binding = u32::try_from(binding).unwrap_or(u32::MAX);
            layout.attributes.iter().map(move |attribute| {
                vk::VertexInputAttributeDescription::default()
                    .binding(binding)
                    .location(attribute.location)
                    .format(vertex_format_to_vk(attribute.format))
                    .offset(attribute.offset)
            })
        })
        .collect::<Vec<_>>();
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
        .vertex_binding_descriptions(&vertex_bindings)
        .vertex_attribute_descriptions(&vertex_attributes);
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
        .topology(primitive_topology_to_vk(build.desc.primitive_topology))
        .primitive_restart_enable(false);
    let viewport_state = vk::PipelineViewportStateCreateInfo::default()
        .viewport_count(1)
        .scissor_count(1);
    let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic_state =
        vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
    let rasterizer = vk::PipelineRasterizationStateCreateInfo::default()
        .depth_clamp_enable(false)
        .rasterizer_discard_enable(false)
        .polygon_mode(vk::PolygonMode::FILL)
        .line_width(1.0)
        .cull_mode(match build.desc.raster.cull_mode {
            None => vk::CullModeFlags::NONE,
            Some(gfx_core::Face::Front) => vk::CullModeFlags::FRONT,
            Some(gfx_core::Face::Back) => vk::CullModeFlags::BACK,
        })
        .front_face(match build.desc.raster.front_face {
            gfx_core::FrontFace::Ccw => vk::FrontFace::COUNTER_CLOCKWISE,
            gfx_core::FrontFace::Cw => vk::FrontFace::CLOCKWISE,
        })
        .depth_bias_enable(false);
    let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
        .sample_shading_enable(false)
        .rasterization_samples(
            sample_flag_for(build.sample_count).unwrap_or(vk::SampleCountFlags::TYPE_1),
        );
    let color_blend_attachments = build
        .desc
        .color_targets
        .iter()
        .map(|target| {
            let mut attachment = vk::PipelineColorBlendAttachmentState::default()
                .color_write_mask(color_write_flags(target.write_mask));
            let blend_mode = target.blend;
            match blend_mode {
                BlendMode::Replace => {}
                BlendMode::Alpha => {
                    attachment = attachment
                        .blend_enable(true)
                        .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
                        .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                        .color_blend_op(vk::BlendOp::ADD)
                        .src_alpha_blend_factor(vk::BlendFactor::ONE)
                        .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                        .alpha_blend_op(vk::BlendOp::ADD);
                }
                BlendMode::PremultipliedAlpha => {
                    attachment = attachment
                        .blend_enable(true)
                        .src_color_blend_factor(vk::BlendFactor::ONE)
                        .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                        .color_blend_op(vk::BlendOp::ADD)
                        .src_alpha_blend_factor(vk::BlendFactor::ONE)
                        .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                        .alpha_blend_op(vk::BlendOp::ADD);
                }
                BlendMode::AdditiveAlpha => {
                    attachment = attachment
                        .blend_enable(true)
                        .src_color_blend_factor(vk::BlendFactor::ONE)
                        .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                        .color_blend_op(vk::BlendOp::ADD)
                        .src_alpha_blend_factor(vk::BlendFactor::ONE)
                        .dst_alpha_blend_factor(vk::BlendFactor::ONE)
                        .alpha_blend_op(vk::BlendOp::ADD);
                }
                #[cfg(target_os = "windows")]
                BlendMode::SubpixelDualSource => {
                    attachment = attachment
                        .blend_enable(true)
                        .src_color_blend_factor(vk::BlendFactor::SRC1_COLOR)
                        .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC1_COLOR)
                        .color_blend_op(vk::BlendOp::ADD)
                        .src_alpha_blend_factor(vk::BlendFactor::ONE)
                        .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
                        .alpha_blend_op(vk::BlendOp::ADD);
                }
            }
            attachment
        })
        .collect::<Vec<_>>();
    let color_blending = vk::PipelineColorBlendStateCreateInfo::default()
        .logic_op_enable(false)
        .attachments(&color_blend_attachments);
    let depth_state = build.desc.depth_state;
    let stencil = depth_state.and_then(|state| state.stencil);
    let stencil_op_state = |face: gfx_core::StencilFaceState| {
        vk::StencilOpState::default()
            .fail_op(stencil_op_to_vk(face.fail_op))
            .pass_op(stencil_op_to_vk(face.pass_op))
            .depth_fail_op(stencil_op_to_vk(face.depth_fail_op))
            .compare_op(depth_compare_function(face.compare))
    };
    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default()
        .depth_test_enable(depth_state.is_some())
        .depth_write_enable(depth_state.is_some_and(|state| state.write_enabled))
        .depth_compare_op(depth_state.map_or(vk::CompareOp::ALWAYS, |state| {
            depth_compare_function(state.compare)
        }))
        .depth_bounds_test_enable(false)
        .stencil_test_enable(stencil.is_some())
        .front(stencil.map_or(vk::StencilOpState::default(), |s| stencil_op_state(s.front)))
        .back(stencil.map_or(vk::StencilOpState::default(), |s| stencil_op_state(s.back)));
    let color_formats = build.desc.color_targets.iter().map(|target| format_to_vk(target.format))
        .collect::<Vec<_>>();
    let mut rendering = vk::PipelineRenderingCreateInfo::default()
        .color_attachment_formats(&color_formats)
        .depth_attachment_format(build.depth_format.map_or(vk::Format::UNDEFINED, format_to_vk));
    let pipeline_info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&shader_stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&rasterizer)
        .multisample_state(&multisampling)
        .depth_stencil_state(&depth_stencil)
        .color_blend_state(&color_blending)
        .dynamic_state(&dynamic_state)
        .layout(build.pipeline_layout)
        .push_next(&mut rendering);
    // SAFETY: dynamic rendering formats and shader modules remain live through creation.
    let pipeline = unsafe {
        build
            .device
            .create_graphics_pipelines(build.pipeline_cache, &[pipeline_info], None)
    }
    .map_err(|(pipelines, error)| {
        for pipeline in pipelines {
            if pipeline != vk::Pipeline::null() {
                // SAFETY: partial results of the failed creation belong to this call.
                unsafe { build.device.destroy_pipeline(pipeline, None) };
            }
        }
        VulkanError::from(error)
    })?[0];
    Ok((build.pipeline_layout, pipeline))
}

fn sample_count_of(flags: vk::SampleCountFlags) -> u32 {
    for count in [64u32, 32, 16, 8, 4, 2, 1] {
        if let Some(flag) = sample_flag_for(count)
            && flags.contains(flag)
        {
            return count;
        }
    }
    1
}

fn sample_flag_for(count: u32) -> Option<vk::SampleCountFlags> {
    Some(match count {
        1 => vk::SampleCountFlags::TYPE_1,
        2 => vk::SampleCountFlags::TYPE_2,
        4 => vk::SampleCountFlags::TYPE_4,
        8 => vk::SampleCountFlags::TYPE_8,
        16 => vk::SampleCountFlags::TYPE_16,
        32 => vk::SampleCountFlags::TYPE_32,
        64 => vk::SampleCountFlags::TYPE_64,
        _ => return None,
    })
}

fn color_write_flags(write_mask: ColorWrites) -> vk::ColorComponentFlags {
    let mut flags = vk::ColorComponentFlags::empty();
    if write_mask.contains(ColorWrites::RED) {
        flags |= vk::ColorComponentFlags::R;
    }
    if write_mask.contains(ColorWrites::GREEN) {
        flags |= vk::ColorComponentFlags::G;
    }
    if write_mask.contains(ColorWrites::BLUE) {
        flags |= vk::ColorComponentFlags::B;
    }
    if write_mask.contains(ColorWrites::ALPHA) {
        flags |= vk::ColorComponentFlags::A;
    }
    flags
}

fn depth_compare_function(compare: CompareFunction) -> vk::CompareOp {
    match compare {
        CompareFunction::Never => vk::CompareOp::NEVER,
        CompareFunction::Less => vk::CompareOp::LESS,
        CompareFunction::Equal => vk::CompareOp::EQUAL,
        CompareFunction::LessEqual => vk::CompareOp::LESS_OR_EQUAL,
        CompareFunction::Greater => vk::CompareOp::GREATER,
        CompareFunction::NotEqual => vk::CompareOp::NOT_EQUAL,
        CompareFunction::GreaterEqual => vk::CompareOp::GREATER_OR_EQUAL,
        CompareFunction::Always => vk::CompareOp::ALWAYS,
    }
}

fn stencil_op_to_vk(op: gfx_core::StencilOp) -> vk::StencilOp {
    use gfx_core::StencilOp;
    match op {
        StencilOp::Keep => vk::StencilOp::KEEP,
        StencilOp::Zero => vk::StencilOp::ZERO,
        StencilOp::Replace => vk::StencilOp::REPLACE,
        StencilOp::IncrementClamp => vk::StencilOp::INCREMENT_AND_CLAMP,
        StencilOp::DecrementClamp => vk::StencilOp::DECREMENT_AND_CLAMP,
        StencilOp::Invert => vk::StencilOp::INVERT,
        StencilOp::IncrementWrap => vk::StencilOp::INCREMENT_AND_WRAP,
        StencilOp::DecrementWrap => vk::StencilOp::DECREMENT_AND_WRAP,
    }
}

fn create_command_pool(device: &ash::Device, queue_family_index: u32) -> Result<vk::CommandPool> {
    let pool_info = vk::CommandPoolCreateInfo::default()
        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
        .queue_family_index(queue_family_index);
    // SAFETY: Queue family index was selected from this physical device.
    unsafe { device.create_command_pool(&pool_info, None) }
        .map_err(|error| VulkanError::from(error).into())
}

fn create_upload_commands(
    device: &ash::Device,
    queue_family_index: u32,
    timestamps_enabled: bool,
) -> Result<VulkanUploadCommands> {
    let command_pool = create_command_pool(device, queue_family_index)?;
    let command_buffers = match allocate_command_buffers(device, command_pool, 1) {
        Ok(command_buffers) => command_buffers,
        Err(error) => {
            // SAFETY: The pool was created immediately above and has no submitted work.
            unsafe { device.destroy_command_pool(command_pool, None) };
            return Err(error);
        }
    };
    let Some(command_buffer) = command_buffers.into_iter().next() else {
        // SAFETY: The pool was created immediately above and owns no submitted work.
        unsafe { device.destroy_command_pool(command_pool, None) };
        return Err(Error::Backend(
            "failed to allocate Vulkan upload command buffer".to_string(),
        ));
    };
    let timestamp_queries = if timestamps_enabled {
        let create_info = vk::QueryPoolCreateInfo::default()
            .query_type(vk::QueryType::TIMESTAMP)
            .query_count(2);
        // SAFETY: The descriptor contains no borrowed pointers and the device is live.
        match unsafe { device.create_query_pool(&create_info, None) } {
            Ok(query_pool) => Some(query_pool),
            Err(error) => {
                // SAFETY: The command pool has no submitted work.
                unsafe { device.destroy_command_pool(command_pool, None) };
                return Err(VulkanError::from(error).into());
            }
        }
    } else {
        None
    };
    Ok(VulkanUploadCommands {
        command_pool,
        command_buffer,
        timestamp_queries,
        gpu_work_tag: None,
    })
}

fn destroy_upload_commands(device: &ash::Device, commands: &VulkanUploadCommands) {
    if let Some(query_pool) = commands.timestamp_queries {
        // SAFETY: The query pool belongs to this device and no submitted work references it.
        unsafe { device.destroy_query_pool(query_pool, None) };
    }
    // SAFETY: The pool owns the command buffer and is destroyed only after submitted work retires.
    unsafe { device.destroy_command_pool(commands.command_pool, None) };
}

fn allocate_command_buffers(
    device: &ash::Device,
    command_pool: vk::CommandPool,
    count: u32,
) -> Result<Vec<vk::CommandBuffer>> {
    let alloc_info = vk::CommandBufferAllocateInfo::default()
        .command_pool(command_pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(count);
    // SAFETY: Command pool is valid and belongs to this device.
    unsafe { device.allocate_command_buffers(&alloc_info) }
        .map_err(|error| VulkanError::from(error).into())
}

fn create_fence(device: &ash::Device, signaled: bool) -> Result<vk::Fence> {
    let flags = if signaled {
        vk::FenceCreateFlags::SIGNALED
    } else {
        vk::FenceCreateFlags::empty()
    };
    let fence_info = vk::FenceCreateInfo::default().flags(flags);
    // SAFETY: Device is valid and fence creation info is self-contained.
    unsafe { device.create_fence(&fence_info, None) }
        .map_err(|error| VulkanError::from(error).into())
}

fn create_sync_objects(
    device: &ash::Device,
    image_count: usize,
    frame_slots: usize,
) -> Result<(Vec<vk::Semaphore>, Vec<vk::Semaphore>, Vec<vk::Fence>)> {
    let mut image_available_semaphores = Vec::with_capacity(frame_slots);
    let mut render_finished_semaphores = Vec::with_capacity(image_count);
    let mut in_flight_fences = Vec::with_capacity(frame_slots);
    let semaphore_info = vk::SemaphoreCreateInfo::default();
    for _ in 0..frame_slots {
        // SAFETY: Device is valid and creation info is well-formed.
        image_available_semaphores.push(
            unsafe { device.create_semaphore(&semaphore_info, None) }.map_err(VulkanError::from)?,
        );
        // SAFETY: Device is valid and creation info is well-formed.
        in_flight_fences.push(create_fence(device, true)?);
    }
    for _ in 0..image_count {
        // SAFETY: Device is valid and creation info is well-formed. Each swapchain image
        // owns its present-wait semaphore independently of the graphics frame slots.
        render_finished_semaphores.push(
            unsafe { device.create_semaphore(&semaphore_info, None) }.map_err(VulkanError::from)?,
        );
    }
    Ok((
        image_available_semaphores,
        render_finished_semaphores,
        in_flight_fences,
    ))
}

struct CommandRecordInfo<'a> {
    depth_accesses: &'a [depth::DepthAccess],
    device: &'a ash::Device,
    command_buffer: vk::CommandBuffer,
    color_views: &'a [vk::ImageView],
    depth_view: Option<vk::ImageView>,
    present_image: Option<vk::Image>,
    steps: &'a [CommandDrawStepInfo],
    descriptor_sets: &'a [vk::DescriptorSet],
    multi_draw_indirect: bool,
    max_draw_indirect_count: u32,
    extent: vk::Extent2D,
    viewport_extent: vk::Extent2D,
    viewport_origin: gfx_core::SignedOrigin2d,
    scissor_extent: vk::Extent2D,
    color_load_op: LoadOp<ClearColor>,
    depth_load_op: Option<LoadOp<f32>>,
    render_target_transitions: &'a [CommandRenderTargetTransition],
    clear_region: Option<ScissorRect>,
    /// Prefer attachment resolve when it preserves the full-image contract.
    resolve: Option<CommandResolve>,
    /// Vulkan timestamps enclose all command-buffer GPU work including layout transitions.
    gpu_trace: Option<(vk::QueryPool, u32)>,
    /// Timestamp pair reserved for the native indirect draw loop ONLY.
    gpu_indirect_trace: Option<(vk::QueryPool, u32)>,
    host_query_reset: bool,
    /// A later native attachment write reuses physical texture storage that
    /// appeared earlier in this ordered Render Graph batch.
    graph_write_hazard: bool,
}

#[derive(Clone, Copy)]
struct CommandResolve {
    source: vk::Image,
    destination: vk::Image,
    extent: vk::Extent2D,
    old_layout: vk::ImageLayout,
    native_view: Option<vk::ImageView>,
    discard_source: bool,
}

#[derive(Clone, Copy)]
struct CommandRenderTargetTransition {
    image: vk::Image,
    old_layout: vk::ImageLayout,
    transient: bool,
}

struct CommandDrawStepInfo {
    count: Option<(vk::Buffer, u64, u32)>,
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    descriptor_sets: std::ops::Range<usize>,
    draw: CommandDrawStepKind,
    scissor: Option<gfx_core::ScissorRect>,
}

enum CommandDrawStepKind {
    Indirect {
        buffer: vk::Buffer,
        offset: u64,
    },
    /// Indexed indirect: the index range lives in the argument record, the buffer is bound.
    IndexedIndirect {
        buffer: vk::Buffer,
        offset: u64,
        index_buffer: vk::Buffer,
        index_offset: u64,
        index_type: vk::IndexType,
    },
    NonIndexed {
        vertex_count: u32,
        first_vertex: u32,
        instance_count: u32,
        first_instance: u32,
    },
    Indexed {
        buffer: vk::Buffer,
        offset: u64,
        index_type: vk::IndexType,
        index_count: u32,
        first_index: u32,
        base_vertex: i32,
        instance_count: u32,
        first_instance: u32,
    },
}

fn validate_index_buffer_range(
    usage: BufferUsage,
    buffer_size: u64,
    binding: IndexBufferBinding,
    first_index: u32,
    index_count: u32,
) -> Result<()> {
    if !usage.contains(BufferUsage::INDEX) {
        return Err(Error::InvalidInput(
            "index buffer must include INDEX usage".to_string(),
        ));
    }
    let stride = index_format_size(binding.format);
    if binding.offset % stride != 0 {
        return Err(Error::InvalidInput(
            "index buffer offset must be aligned to the index format size".to_string(),
        ));
    }
    let first_index_byte = u64::from(first_index)
        .checked_mul(stride)
        .ok_or_else(|| Error::InvalidInput("first index byte offset overflow".to_string()))?;
    let index_bytes = u64::from(index_count)
        .checked_mul(stride)
        .ok_or_else(|| Error::InvalidInput("index buffer range overflow".to_string()))?;
    let byte_end = binding
        .offset
        .checked_add(first_index_byte)
        .and_then(|start| start.checked_add(index_bytes))
        .ok_or_else(|| Error::InvalidInput("index buffer range overflow".to_string()))?;
    if byte_end > buffer_size {
        return Err(Error::InvalidInput(
            "index buffer range is out of bounds".to_string(),
        ));
    }
    Ok(())
}

fn index_format_size(format: IndexFormat) -> u64 {
    match format {
        IndexFormat::Uint16 => 2,
        IndexFormat::Uint32 => 4,
    }
}

fn index_format_to_vk(format: IndexFormat) -> vk::IndexType {
    match format {
        IndexFormat::Uint16 => vk::IndexType::UINT16,
        IndexFormat::Uint32 => vk::IndexType::UINT32,
    }
}

fn full_render_area(extent: vk::Extent2D) -> vk::Rect2D {
    vk::Rect2D {
        offset: vk::Offset2D { x: 0, y: 0 },
        extent,
    }
}

/// Restrict Vulkan attachment LOAD/STORE traffic to the actual dirty drawing
/// footprint, not the full backing texture. Offscreen targets can
/// be much larger than the scissored portion being updated. A draw scissor
/// alone does not limit the render pass attachment renderArea.
///
/// Never shrink a swapchain pass, a color/depth Clear, or a pass with even one
/// unbounded draw. Those cases can legitimately affect pixels outside the
/// known scissors, including depth relied on by a later pass.
fn retained_offscreen_render_area(
    extent: vk::Extent2D,
    offscreen: bool,
    color_load: LoadOp<ClearColor>,
    depth_load: Option<LoadOp<f32>>,
    scissors: impl IntoIterator<Item = Option<ScissorRect>>,
) -> vk::Rect2D {
    let full = full_render_area(extent);
    if !offscreen
        || !matches!(color_load, LoadOp::Load)
        || matches!(depth_load, Some(LoadOp::Clear(_)))
    {
        return full;
    }

    let mut union: Option<(u32, u32, u32, u32)> = None;
    for scissor in scissors {
        let Some(scissor) = scissor else {
            return full;
        };
        // The draw recorder falls back to a full scissor if native Vulkan
        // coordinate conversion fails. Match that behavior here, otherwise
        // a narrow renderArea could exclude pixels the draw will touch.
        if vk_rect_from_scissor(scissor, extent).is_err() {
            return full;
        }
        let left = scissor.x.min(extent.width);
        let top = scissor.y.min(extent.height);
        let right = scissor.x.saturating_add(scissor.width).min(extent.width);
        let bottom = scissor.y.saturating_add(scissor.height).min(extent.height);
        if right <= left || bottom <= top {
            continue;
        }
        union = Some(match union {
            Some((min_x, min_y, max_x, max_y)) => (
                min_x.min(left),
                min_y.min(top),
                max_x.max(right),
                max_y.max(bottom),
            ),
            None => (left, top, right, bottom),
        });
    }

    let Some((left, top, right, bottom)) = union else {
        // An empty or fully clipped sequence does not prove that the attachment
        // may be discarded. Keep the established behavior.
        return full;
    };
    let (Ok(x), Ok(y)) = (i32::try_from(left), i32::try_from(top)) else {
        return full;
    };
    vk::Rect2D {
        offset: vk::Offset2D { x, y },
        extent: vk::Extent2D {
            width: right - left,
            height: bottom - top,
        },
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "Vulkan command recording keeps render-pass state transitions adjacent for auditability"
)]
/// A bounded source capture clears only the requested color/depth attachments.
/// The caller clips every draw to that footprint. If any step could write
/// outside it, use a full attachment clear instead.
fn scoped_offscreen_clear_area(
    extent: vk::Extent2D,
    offscreen: bool,
    color_load_op: LoadOp<ClearColor>,
    depth_load_op: Option<LoadOp<f32>>,
    clear_region: Option<ScissorRect>,
    scissors: impl IntoIterator<Item = Option<ScissorRect>>,
) -> Option<vk::Rect2D> {
    let region = clear_region?;
    if !offscreen
        || (!matches!(color_load_op, LoadOp::Clear(_))
            && !matches!(depth_load_op, Some(LoadOp::Clear(_))))
    {
        return None;
    }
    let target = vk_rect_from_scissor(region, extent).ok()?;
    if target.extent.width == 0 || target.extent.height == 0 {
        return None;
    }
    let end_x = u32::try_from(target.offset.x)
        .ok()?
        .checked_add(target.extent.width)?;
    let end_y = u32::try_from(target.offset.y)
        .ok()?
        .checked_add(target.extent.height)?;
    for scissor in scissors {
        let scissor = scissor?;
        let clipped = vk_rect_from_scissor(scissor, extent).ok()?;
        let left = u32::try_from(clipped.offset.x).ok()?;
        let top = u32::try_from(clipped.offset.y).ok()?;
        if left < u32::try_from(target.offset.x).ok()?
            || top < u32::try_from(target.offset.y).ok()?
            || left.saturating_add(clipped.extent.width) > end_x
            || top.saturating_add(clipped.extent.height) > end_y
        {
            return None;
        }
    }
    Some(target)
}

// Match the WSI acquire wait stage to the first image transition.
fn record_present_layout(device: &ash::Device, command: vk::CommandBuffer, image: vk::Image, present: bool) {
    let color = vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT;
    let (old, new, src_stage, dst_stage, src_access, dst_access) = if present {
        (vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL, vk::ImageLayout::PRESENT_SRC_KHR,
            color, vk::PipelineStageFlags2::NONE, vk::AccessFlags2::COLOR_ATTACHMENT_WRITE,
            vk::AccessFlags2::NONE)
    } else {
        (vk::ImageLayout::UNDEFINED, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            color, color, vk::AccessFlags2::NONE,
            vk::AccessFlags2::COLOR_ATTACHMENT_READ | vk::AccessFlags2::COLOR_ATTACHMENT_WRITE)
    };
    let barrier = vk::ImageMemoryBarrier2::default().image(image)
        .old_layout(old).new_layout(new).src_stage_mask(src_stage).dst_stage_mask(dst_stage)
        .src_access_mask(src_access).dst_access_mask(dst_access)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED).dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .subresource_range(vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1));
    // SAFETY: the acquired image belongs to this swapchain; its binary
    // semaphores order transitions against the presentation engine.
    unsafe { device.cmd_pipeline_barrier2(command,
        &vk::DependencyInfo::default().image_memory_barriers(&[barrier])) };
}

fn record_command_buffer(info: &CommandRecordInfo<'_>) -> Result<()> {
    let begin_info = vk::CommandBufferBeginInfo::default();
    // SAFETY: Command buffer is allocated from this context's command pool and resettable.
    unsafe {
        info.device
            .reset_command_buffer(info.command_buffer, vk::CommandBufferResetFlags::empty())
    }
    .map_err(VulkanError::from)?;
    // SAFETY: Command buffer is valid and currently in initial state after reset.
    unsafe {
        info.device
            .begin_command_buffer(info.command_buffer, &begin_info)
    }
    .map_err(VulkanError::from)?;
    if let Some((query_pool, first_query)) = info.gpu_trace {
        // SAFETY: This command buffer is recording and owns this unique timestamp pair.
        unsafe {
            if info.host_query_reset {
                // Encoders are exclusively owner-recorded and recycled only after retirement.
                info.device.reset_query_pool(query_pool, first_query, 2);
            } else {
                info.device.cmd_reset_query_pool(info.command_buffer, query_pool, first_query, 2);
            }
            info.device.cmd_write_timestamp2(
                    info.command_buffer,
                    vk::PipelineStageFlags2::TOP_OF_PIPE,
                query_pool,
                first_query,
            );
        }
    }
    if let Some((pool, slot)) = info.gpu_indirect_trace {
        // SAFETY: This dedicated command buffer owns this probe pair and is
        // reset only before entering its render pass.
        unsafe {
            if info.host_query_reset {
                info.device.reset_query_pool(pool, slot, 2);
            } else {
                info.device.cmd_reset_query_pool(info.command_buffer, pool, slot, 2);
            }
        };
    }
    depth::record_depth_accesses(info.device, info.command_buffer, info.depth_accesses);
    if info.graph_write_hazard || !info.render_target_transitions.is_empty() || info.depth_view.is_some() {
        // Preserve the legacy external subpass dependency, including
        // repeated depth writes for an unchanged attachment layout.
        let mut stages = vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT;
        let mut writes = vk::AccessFlags2::COLOR_ATTACHMENT_WRITE;
        let mut accesses = vk::AccessFlags2::COLOR_ATTACHMENT_READ | writes;
        if info.depth_view.is_some() {
            stages |= vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS;
            writes |= vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE;
            accesses |= vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE;
        }
        let barrier = vk::MemoryBarrier2::default()
            .src_stage_mask(stages).dst_stage_mask(stages)
            .src_access_mask(writes).dst_access_mask(accesses);
        // SAFETY: Command buffer recording is active; the target image and
        // previous writer belong to this same ordered queue submission.
        unsafe {
            info.device.cmd_pipeline_barrier2(
                info.command_buffer,
                &vk::DependencyInfo::default().dependency_flags(vk::DependencyFlags::empty())
                    .memory_barriers(&[barrier]).buffer_memory_barriers(&[]).image_memory_barriers(&[]),
            );
        }
    }
    for transition in info.render_target_transitions {
        transition_image_layout(
            info.device,
            info.command_buffer,
            transition.image,
            transition.old_layout,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        );
    }

    if let Some(image) = info.present_image {
        record_present_layout(info.device, info.command_buffer, image, false);
    }
    let clear_color = match info.color_load_op {
        LoadOp::Clear(color) => color,
        LoadOp::Load => ClearColor::default(),
    };
    let color_clear = vk::ClearValue {
        color: vk::ClearColorValue {
            float32: [
                clear_color.red,
                clear_color.green,
                clear_color.blue,
                clear_color.alpha,
            ],
        },
    };
    let color_load = matches!(info.color_load_op, LoadOp::Load);
    let mut attachment_stack = [vk::RenderingAttachmentInfo::default(); 4];
    let mut attachment_heap;
    let attachments = if info.color_views.len() <= attachment_stack.len() {
        &mut attachment_stack[..info.color_views.len()]
    } else {
        attachment_heap = vec![vk::RenderingAttachmentInfo::default(); info.color_views.len()];
        attachment_heap.as_mut_slice()
    };
    for (attachment, view) in attachments.iter_mut().zip(info.color_views) {
        *attachment = rendering_attachment(*view, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            color_load, color_clear);
    }
    let depth = info.depth_view.zip(info.depth_load_op).map(|(view, load)| {
        let clear_depth = match load { LoadOp::Clear(depth) => depth, LoadOp::Load => 1.0 };
        rendering_attachment(view, vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
            matches!(load, LoadOp::Load),
            vk::ClearValue { depth_stencil: vk::ClearDepthStencilValue { depth: clear_depth, stencil: 0 } })
    });
    let has_transition = !info.render_target_transitions.is_empty();
    let render_area = if info.render_target_transitions.iter().any(|transition| transition.transient) {
        full_render_area(info.extent)
    } else { scoped_offscreen_clear_area(
        info.extent,
        has_transition,
        info.color_load_op,
        info.depth_load_op,
        info.clear_region,
        info.steps.iter().map(|step| step.scissor),
    )
    .unwrap_or_else(|| {
        retained_offscreen_render_area(
            info.extent,
            has_transition,
            info.color_load_op,
            info.depth_load_op,
            info.steps.iter().map(|step| step.scissor),
        )
    }) };
    let native_resolve = info.resolve.and_then(|resolve| {
        (render_area == full_render_area(resolve.extent))
            .then_some(resolve.native_view).flatten()
    });
    if let (Some(resolve), Some(view)) = (info.resolve, native_resolve) {
        transition_image_layout(
            info.device, info.command_buffer, resolve.destination,
            resolve.old_layout, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        );
        attachments[0] = attachments[0]
            .resolve_mode(vk::ResolveModeFlags::AVERAGE)
            .resolve_image_view(view)
            .resolve_image_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
        // Only the resolved source is disposable. Other MRT attachments and
        // depth may still be sampled; the explicit resolve fallback needs STORE.
        if resolve.discard_source && !color_load {
            attachments[0] = attachments[0].store_op(vk::AttachmentStoreOp::DONT_CARE);
        }
    }
    let mut rendering_info = vk::RenderingInfo::default()
        .render_area(render_area).layer_count(1).color_attachments(attachments);
    if let Some(depth) = depth.as_ref() {
        rendering_info = rendering_info.depth_attachment(depth);
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "Vulkan viewport coordinates and dimensions are f32 by API contract"
    )]
    let viewport = vk::Viewport {
        x: info.viewport_origin.x as f32,
        y: info.viewport_origin.y as f32,
        width: info.viewport_extent.width as f32,
        height: info.viewport_extent.height as f32,
        min_depth: 0.0,
        max_depth: 1.0,
    };
    let full_scissor = full_render_area(info.scissor_extent);
    // SAFETY: attachment views, layouts, pipeline formats and command buffer are valid.
    unsafe {
        info.device.cmd_begin_rendering(info.command_buffer, &rendering_info);
        info.device
            .cmd_set_viewport(info.command_buffer, 0, &[viewport]);
        if let Some((pool, slot)) = info.gpu_indirect_trace {
            // The clear/attachment setup has completed: start at native draw.
            info.device.cmd_write_timestamp2(
                    info.command_buffer,
                    vk::PipelineStageFlags2::TOP_OF_PIPE,
                pool,
                slot,
            );
        }
        let mut previous_pipeline = None;
        let mut previous_scissor = None;
        let mut previous_bindings = None;
        let mut previous_index = None;
        let mut step_iter = info.steps.iter().peekable();
        while let Some(step) = step_iter.next() {
            let scissor = step
                .scissor
                .and_then(|scissor| vk_rect_from_scissor(scissor, info.scissor_extent).ok())
                .unwrap_or(full_scissor);
            if previous_scissor != Some(scissor) {
                info.device
                    .cmd_set_scissor(info.command_buffer, 0, &[scissor]);
                previous_scissor = Some(scissor);
            }
            if previous_pipeline != Some(step.pipeline) {
                info.device.cmd_bind_pipeline(
                    info.command_buffer,
                    vk::PipelineBindPoint::GRAPHICS,
                    step.pipeline,
                );
                previous_pipeline = Some(step.pipeline);
            }
            let sets = &info.descriptor_sets[step.descriptor_sets.clone()];
            if !sets.is_empty() && previous_bindings != Some((step.pipeline_layout, sets)) {
                info.device.cmd_bind_descriptor_sets(
                    info.command_buffer,
                    vk::PipelineBindPoint::GRAPHICS,
                    step.pipeline_layout,
                    0,
                    sets,
                    &[],
                );
            }
            previous_bindings = Some((step.pipeline_layout, sets));
            match step.draw {
                CommandDrawStepKind::Indirect { buffer, offset } => {
                    if let Some((count_buffer, count_offset, max_count)) = step.count {
                        info.device.cmd_draw_indirect_count(
                            info.command_buffer, buffer, offset, count_buffer, count_offset,
                            max_count, 16,
                        );
                        continue;
                    }
                    let mut draw_count = 1u32;
                    if info.multi_draw_indirect {
                        while draw_count < info.max_draw_indirect_count {
                            let Some(&next) = step_iter.peek() else { break };
                            let CommandDrawStepKind::Indirect {
                                buffer: next_buffer,
                                offset: next_offset,
                            } = next.draw
                            else {
                                break;
                            };
                            if next.count.is_some() || next_buffer != buffer
                                || next.pipeline != step.pipeline
                                || next.pipeline_layout != step.pipeline_layout
                                || next.scissor != step.scissor
                                || &info.descriptor_sets[next.descriptor_sets.clone()] != sets
                                || offset.checked_add(u64::from(draw_count) * 16)
                                    != Some(next_offset)
                            {
                                break;
                            }
                            // Skipped steps have identical native graphics state.
                            // Their GPU records are still executed in original order.
                            step_iter.next();
                            draw_count += 1;
                        }
                    }
                    info.device.cmd_draw_indirect(
                        info.command_buffer,
                        buffer,
                        offset,
                        draw_count,
                        16,
                    );
                }
                CommandDrawStepKind::NonIndexed {
                    vertex_count,
                    first_vertex,
                    instance_count,
                    first_instance,
                } => {
                    info.device.cmd_draw(
                        info.command_buffer,
                        vertex_count,
                        instance_count,
                        first_vertex,
                        first_instance,
                    );
                }
                CommandDrawStepKind::Indexed {
                    buffer,
                    offset,
                    index_type,
                    index_count,
                    first_index,
                    base_vertex,
                    instance_count,
                    first_instance,
                } => {
                    if previous_index != Some((buffer, offset, index_type)) {
                        info.device.cmd_bind_index_buffer(info.command_buffer, buffer, offset, index_type);
                        previous_index = Some((buffer, offset, index_type));
                    }
                    info.device.cmd_draw_indexed(
                        info.command_buffer,
                        index_count,
                        instance_count,
                        first_index,
                        base_vertex,
                        first_instance,
                    );
                }
                CommandDrawStepKind::IndexedIndirect {
                    buffer,
                    offset,
                    index_buffer,
                    index_offset,
                    index_type,
                } => {
                    if previous_index != Some((index_buffer, index_offset, index_type)) {
                        info.device.cmd_bind_index_buffer(info.command_buffer, index_buffer, index_offset, index_type);
                        previous_index = Some((index_buffer, index_offset, index_type));
                    }
                    if let Some((count_buffer, count_offset, max_count)) = step.count {
                        info.device.cmd_draw_indexed_indirect_count(
                            info.command_buffer, buffer, offset, count_buffer, count_offset,
                            max_count, 20,
                        );
                        continue;
                    }
                    let mut draw_count = 1u32;
                    if info.multi_draw_indirect {
                        while draw_count < info.max_draw_indirect_count {
                            let Some(&next) = step_iter.peek() else { break };
                            let CommandDrawStepKind::IndexedIndirect {
                                buffer: next_buffer, offset: next_offset,
                                index_buffer: next_index, index_offset: next_index_offset,
                                index_type: next_index_type,
                            } = next.draw else { break };
                            if next.count.is_some() || next_buffer != buffer
                                || next_index != index_buffer || next_index_offset != index_offset
                                || next_index_type != index_type || next.pipeline != step.pipeline
                                || next.pipeline_layout != step.pipeline_layout || next.scissor != step.scissor
                                || &info.descriptor_sets[next.descriptor_sets.clone()] != sets
                                || offset.checked_add(u64::from(draw_count) * 20) != Some(next_offset)
                            { break; }
                            step_iter.next();
                            draw_count += 1;
                        }
                    }
                    info.device.cmd_draw_indexed_indirect(
                        info.command_buffer, buffer, offset, draw_count, 20,
                    );
                }
            }
        }
        if let Some((pool, slot)) = info.gpu_indirect_trace {
            // Ends after the last indirect command, before MSAA resolve,
            // attachment layout changes or GUI composition.
            info.device.cmd_write_timestamp2(
                    info.command_buffer,
                    vk::PipelineStageFlags2::BOTTOM_OF_PIPE,
                pool,
                slot + 1,
            );
        }
        info.device.cmd_end_rendering(info.command_buffer);
        if let Some(image) = info.present_image {
            record_present_layout(info.device, info.command_buffer, image, true);
        }
        if let Some(resolve) = info.resolve {
            let CommandResolve { source: src, destination: dst, extent, old_layout: dst_old_layout, .. } = resolve;
            if native_resolve.is_some() {
                transition_image_layout(
                    info.device, info.command_buffer, dst,
                    vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                );
            } else {
            // Explicit resolve preserves the existing full-image contract.
            // Both layout transitions must participate in the tracked state.
            transition_image_layout(
                info.device,
                info.command_buffer,
                src,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            );
            transition_image_layout(
                info.device,
                info.command_buffer,
                dst,
                dst_old_layout,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            );
            let region = vk::ImageResolve::default()
                .src_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .src_offset(vk::Offset3D::default())
                .dst_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .dst_offset(vk::Offset3D::default())
                .extent(vk::Extent3D {
                    width: extent.width,
                    height: extent.height,
                    depth: 1,
                });
            // SAFETY: Both images are live and in transfer layouts; region is within bounds.
            info.device.cmd_resolve_image(
                info.command_buffer,
                src,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                dst,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[region],
            );
            transition_image_layout(
                info.device, info.command_buffer, dst,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            );
            }
        }
        for transition in info.render_target_transitions {
            // Attachment-only images must never enter a shader-read or transfer layout.
            if transition.transient { continue; }
            let old_layout = if native_resolve.is_none()
                && info.resolve.is_some_and(|resolve| resolve.source == transition.image) {
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL
            } else {
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL
            };
            transition_image_layout(
                info.device, info.command_buffer, transition.image,
                old_layout, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            );
        }
        if let Some((query_pool, first_query)) = info.gpu_trace {
            info.device.cmd_write_timestamp2(
                    info.command_buffer,
                    vk::PipelineStageFlags2::BOTTOM_OF_PIPE,
                query_pool,
                first_query + 1,
            );
        }
        info.device.end_command_buffer(info.command_buffer)
    }
    .map_err(VulkanError::from)?;
    Ok(())
}

fn vk_rect_from_scissor(
    scissor: gfx_core::ScissorRect,
    extent: vk::Extent2D,
) -> Result<vk::Rect2D> {
    let x = scissor.x.min(extent.width);
    let y = scissor.y.min(extent.height);
    let right = scissor.x.saturating_add(scissor.width).min(extent.width);
    let bottom = scissor.y.saturating_add(scissor.height).min(extent.height);
    Ok(vk::Rect2D {
        offset: vk::Offset2D {
            x: i32::try_from(x)
                .map_err(|error| Error::InvalidInput(format!("scissor x overflow: {error}")))?,
            y: i32::try_from(y)
                .map_err(|error| Error::InvalidInput(format!("scissor y overflow: {error}")))?,
        },
        extent: vk::Extent2D {
            width: right.saturating_sub(x),
            height: bottom.saturating_sub(y),
        },
    })
}

fn begin_one_time_commands(device: &ash::Device, command_buffer: vk::CommandBuffer) -> Result<()> {
    let begin_info =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    // SAFETY: Command buffer is valid and currently initial.
    unsafe { device.begin_command_buffer(command_buffer, &begin_info) }
        .map_err(VulkanError::from)?;
    Ok(())
}

fn end_submit_wait_destroy(
    device: &ash::Device,
    queue: vk::Queue,
    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    timeline: Option<&timeline::QueueTimeline>,
) -> Result<()> {
    // SAFETY: Command buffer is recording and can be ended.
    unsafe { device.end_command_buffer(command_buffer) }.map_err(VulkanError::from)?;
    if let Some(timeline) = timeline {
        timeline.submit(device, queue, &[command_buffer], &[], &[],
            vk::PipelineStageFlags2::ALL_COMMANDS, vk::Fence::null())?;
        timeline.wait_point(timeline.last()).wait().map_err(VulkanError::from)?;
    } else {
        submit::binary(device, queue, &[command_buffer], &[], &[],
            vk::PipelineStageFlags2::ALL_COMMANDS, vk::Fence::null())?;
        // SAFETY: this queue owns the temporary commands.
        unsafe { device.queue_wait_idle(queue) }.map_err(VulkanError::from)?;
    }
    // SAFETY: Command pool belongs to this device and owns command_buffer.
    unsafe { device.destroy_command_pool(command_pool, None) };
    Ok(())
}

fn end_submit_deferred(
    device: &ash::Device,
    queue: vk::Queue,
    command_buffer: vk::CommandBuffer,
    timeline: Option<&timeline::QueueTimeline>,
) -> Result<vk::Fence> {
    // SAFETY: Command buffer is recording and can be ended.
    unsafe { device.end_command_buffer(command_buffer) }.map_err(VulkanError::from)?;
    let fence = create_fence(device, false)?;
    let submitted = match timeline {
        Some(timeline) => timeline.submit(device, queue, &[command_buffer], &[], &[],
            vk::PipelineStageFlags2::ALL_COMMANDS, fence),
        None => submit::binary(device, queue, &[command_buffer], &[], &[],
            vk::PipelineStageFlags2::ALL_COMMANDS, fence),
    };
    if let Err(error) = submitted {
        destroy_fence(device, fence);
        return Err(error);
    }
    Ok(fence)
}

fn transition_image_layout(
    device: &ash::Device,
    command_buffer: vk::CommandBuffer,
    image: vk::Image,
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
) {
    transition_image_layout_levels(
        device,
        command_buffer,
        image,
        old_layout,
        new_layout,
        1,
        1,
        vk::ImageAspectFlags::COLOR,
    );
}

fn transition_image_layout_levels(
    device: &ash::Device,
    command_buffer: vk::CommandBuffer,
    image: vk::Image,
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
    mip_level_count: u32,
    layers: u32,
    aspect: vk::ImageAspectFlags,
) {
    let color_attachment_access =
        vk::AccessFlags2::COLOR_ATTACHMENT_READ | vk::AccessFlags2::COLOR_ATTACHMENT_WRITE;
    let (source_stage, destination_stage, source_access_mask, destination_access_mask) =
        match (old_layout, new_layout) {
            (vk::ImageLayout::UNDEFINED, vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL) => (
                vk::PipelineStageFlags2::TOP_OF_PIPE,
                vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                    | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
                vk::AccessFlags2::empty(),
                vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ
                    | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE,
            ),
            (
                vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            ) => (
                vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                    | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
                vk::PipelineStageFlags2::FRAGMENT_SHADER,
                vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE,
                vk::AccessFlags2::SHADER_READ,
            ),
            (
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
            ) => (
                vk::PipelineStageFlags2::FRAGMENT_SHADER,
                vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                    | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
                vk::AccessFlags2::SHADER_READ,
                vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ
                    | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE,
            ),
            (
                vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            ) => (
                vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                    | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
                vk::PipelineStageFlags2::TRANSFER,
                vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE,
                vk::AccessFlags2::TRANSFER_READ,
            ),
            (
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
            ) => (
                vk::PipelineStageFlags2::TRANSFER,
                vk::PipelineStageFlags2::EARLY_FRAGMENT_TESTS
                    | vk::PipelineStageFlags2::LATE_FRAGMENT_TESTS,
                vk::AccessFlags2::TRANSFER_READ,
                vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_READ
                    | vk::AccessFlags2::DEPTH_STENCIL_ATTACHMENT_WRITE,
            ),
            (vk::ImageLayout::UNDEFINED, vk::ImageLayout::TRANSFER_DST_OPTIMAL) => (
                vk::PipelineStageFlags2::TOP_OF_PIPE,
                vk::PipelineStageFlags2::TRANSFER,
                vk::AccessFlags2::empty(),
                vk::AccessFlags2::TRANSFER_WRITE,
            ),
            (vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL, vk::ImageLayout::TRANSFER_DST_OPTIMAL) => (
                vk::PipelineStageFlags2::FRAGMENT_SHADER,
                vk::PipelineStageFlags2::TRANSFER,
                vk::AccessFlags2::SHADER_READ,
                vk::AccessFlags2::TRANSFER_WRITE,
            ),
            (vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL) => (
                vk::PipelineStageFlags2::TRANSFER,
                vk::PipelineStageFlags2::FRAGMENT_SHADER,
                vk::AccessFlags2::TRANSFER_WRITE,
                vk::AccessFlags2::SHADER_READ,
            ),
            (vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL, vk::ImageLayout::TRANSFER_SRC_OPTIMAL) => (
                vk::PipelineStageFlags2::FRAGMENT_SHADER,
                vk::PipelineStageFlags2::TRANSFER,
                vk::AccessFlags2::SHADER_READ,
                vk::AccessFlags2::TRANSFER_READ,
            ),
            (vk::ImageLayout::TRANSFER_SRC_OPTIMAL, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL) => (
                vk::PipelineStageFlags2::TRANSFER,
                vk::PipelineStageFlags2::FRAGMENT_SHADER,
                vk::AccessFlags2::TRANSFER_READ,
                vk::AccessFlags2::SHADER_READ,
            ),
            (vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL, vk::ImageLayout::TRANSFER_SRC_OPTIMAL) => (
                vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                vk::PipelineStageFlags2::TRANSFER,
                color_attachment_access,
                vk::AccessFlags2::TRANSFER_READ,
            ),
            (vk::ImageLayout::TRANSFER_SRC_OPTIMAL, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL) => (
                vk::PipelineStageFlags2::TRANSFER,
                vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags2::TRANSFER_READ,
                color_attachment_access,
            ),
            (vk::ImageLayout::UNDEFINED, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL) => (
                vk::PipelineStageFlags2::TOP_OF_PIPE,
                vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags2::empty(),
                color_attachment_access,
            ),
            (
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            ) => (
                vk::PipelineStageFlags2::FRAGMENT_SHADER,
                vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags2::SHADER_READ,
                color_attachment_access,
            ),
            (
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            ) => (
                vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
                vk::PipelineStageFlags2::FRAGMENT_SHADER,
                color_attachment_access,
                vk::AccessFlags2::SHADER_READ,
            ),
            // Storage images may stay in GENERAL between compute dispatches.
            // Transfers, attachments and future sampled consumers must observe
            // their shader writes. Unlike TOP_OF_PIPE -> BOTTOM_OF_PIPE with
            // empty access masks, this fallback establishes a real dependency
            // for uncommon layout pairs without involving the CPU.
            _ if old_layout == vk::ImageLayout::UNDEFINED => (
                vk::PipelineStageFlags2::TOP_OF_PIPE,
                vk::PipelineStageFlags2::ALL_COMMANDS,
                vk::AccessFlags2::empty(),
                vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE,
            ),
            _ => (
                vk::PipelineStageFlags2::ALL_COMMANDS,
                vk::PipelineStageFlags2::ALL_COMMANDS,
                vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE,
                vk::AccessFlags2::MEMORY_READ | vk::AccessFlags2::MEMORY_WRITE,
            ),
        };
    let barrier = vk::ImageMemoryBarrier2::default()
                .src_stage_mask(source_stage)
                .dst_stage_mask(destination_stage)
        .old_layout(old_layout)
        .new_layout(new_layout)
        .src_access_mask(source_access_mask)
        .dst_access_mask(destination_access_mask)
        .image(image)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .subresource_range(
            vk::ImageSubresourceRange::default()
                .aspect_mask(aspect)
                .base_mip_level(0)
                .level_count(mip_level_count)
                .base_array_layer(0)
                .layer_count(layers),
        );
    // SAFETY: Command buffer is recording and image barrier targets the whole image subresource.
    unsafe {
        device.cmd_pipeline_barrier2(
                command_buffer,
                &vk::DependencyInfo::default().dependency_flags(vk::DependencyFlags::empty())
                    .memory_barriers(&[]).buffer_memory_barriers(&[]).image_memory_barriers(&[barrier]),
            );
    }
}

fn format_to_vk(format: Format) -> vk::Format {
    match format {
        Format::R8Unorm => vk::Format::R8_UNORM,
        Format::Bgra8Unorm => vk::Format::B8G8R8A8_UNORM,
        Format::Bgra8UnormSrgb => vk::Format::B8G8R8A8_SRGB,
        Format::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
        Format::Rgba8UnormSrgb => vk::Format::R8G8B8A8_SRGB,
        Format::Rgba16Float => vk::Format::R16G16B16A16_SFLOAT,
        Format::Depth32Float => vk::Format::D32_SFLOAT,
        Format::Rg8Unorm => vk::Format::R8G8_UNORM,
        Format::R16Float => vk::Format::R16_SFLOAT,
        Format::Rg16Float => vk::Format::R16G16_SFLOAT,
        Format::R32Float => vk::Format::R32_SFLOAT,
        Format::Rgba32Float => vk::Format::R32G32B32A32_SFLOAT,
        Format::R8Uint => vk::Format::R8_UINT,
        Format::Rg8Uint => vk::Format::R8G8_UINT,
        Format::Rgba8Uint => vk::Format::R8G8B8A8_UINT,
    }
}

fn image_aspect_from_format(format: Format) -> vk::ImageAspectFlags {
    match format {
        Format::Depth32Float => vk::ImageAspectFlags::DEPTH,
        Format::R8Unorm
        | Format::Bgra8Unorm
        | Format::Bgra8UnormSrgb
        | Format::Rgba8Unorm
        | Format::Rgba8UnormSrgb
        | Format::Rgba16Float
        | Format::Rg8Unorm
        | Format::R16Float
        | Format::Rg16Float
        | Format::R32Float
        | Format::Rgba32Float
        | Format::R8Uint
        | Format::Rg8Uint
        | Format::Rgba8Uint => vk::ImageAspectFlags::COLOR,
    }
}

fn buffer_usage_to_vk(usage: BufferUsage) -> vk::BufferUsageFlags {
    let mut flags = vk::BufferUsageFlags::empty();
    if usage.contains(BufferUsage::INDIRECT) {
        flags |= vk::BufferUsageFlags::INDIRECT_BUFFER;
    }
    if usage.contains(BufferUsage::COPY_SRC) {
        flags |= vk::BufferUsageFlags::TRANSFER_SRC;
    }
    if usage.contains(BufferUsage::COPY_DST) {
        flags |= vk::BufferUsageFlags::TRANSFER_DST;
    }
    if usage.contains(BufferUsage::VERTEX) {
        flags |= vk::BufferUsageFlags::VERTEX_BUFFER;
    }
    if usage.contains(BufferUsage::INDEX) {
        flags |= vk::BufferUsageFlags::INDEX_BUFFER;
    }
    if usage.contains(BufferUsage::UNIFORM) {
        flags |= vk::BufferUsageFlags::UNIFORM_BUFFER;
    }
    if usage.contains(BufferUsage::STORAGE) {
        flags |= vk::BufferUsageFlags::STORAGE_BUFFER;
    }
    flags
}

fn texture_usage_to_vk(usage: TextureUsage) -> vk::ImageUsageFlags {
    let mut flags = vk::ImageUsageFlags::empty();
    if usage.contains(TextureUsage::COPY_SRC) {
        flags |= vk::ImageUsageFlags::TRANSFER_SRC;
    }
    if usage.contains(TextureUsage::COPY_DST) {
        flags |= vk::ImageUsageFlags::TRANSFER_DST;
    }
    if usage.contains(TextureUsage::SAMPLED) {
        flags |= vk::ImageUsageFlags::SAMPLED;
    }
    if usage.contains(TextureUsage::COLOR_ATTACHMENT) {
        flags |= vk::ImageUsageFlags::COLOR_ATTACHMENT;
    }
    if usage.contains(TextureUsage::DEPTH_ATTACHMENT) {
        flags |= vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT;
    }
    if usage.contains(TextureUsage::STORAGE) {
        flags |= vk::ImageUsageFlags::STORAGE;
    }
    flags
}

fn primitive_topology_to_vk(topology: PrimitiveTopology) -> vk::PrimitiveTopology {
    match topology {
        PrimitiveTopology::TriangleList => vk::PrimitiveTopology::TRIANGLE_LIST,
        PrimitiveTopology::TriangleStrip => vk::PrimitiveTopology::TRIANGLE_STRIP,
    }
}

fn filter_to_vk(filter: FilterMode) -> vk::Filter {
    match filter {
        FilterMode::Nearest => vk::Filter::NEAREST,
        FilterMode::Linear => vk::Filter::LINEAR,
    }
}

fn sampler_create_info(
    desc: &SamplerDescriptor,
    max_anisotropy: f32,
) -> vk::SamplerCreateInfo<'static> {
    let mut create_info = vk::SamplerCreateInfo::default()
        .mag_filter(filter_to_vk(desc.mag_filter))
        .min_filter(filter_to_vk(desc.min_filter))
        .address_mode_u(address_mode_to_vk(desc.address_mode_u))
        .address_mode_v(address_mode_to_vk(desc.address_mode_v))
        .address_mode_w(address_mode_to_vk(desc.address_mode_w))
        .mipmap_mode(match desc.mipmap_filter {
            FilterMode::Nearest => vk::SamplerMipmapMode::NEAREST,
            FilterMode::Linear => vk::SamplerMipmapMode::LINEAR,
        })
        .min_lod(desc.lod_min_clamp)
        .max_lod(desc.lod_max_clamp)
        .mip_lod_bias(desc.lod_bias);
    if let Some(compare) = desc.compare {
        create_info = create_info
            .compare_enable(true)
            .compare_op(depth_compare_op(compare));
    }
    if let Some(color) = desc.border_color {
        create_info = create_info.border_color(border_color_to_vk(color));
    }
    if desc.anisotropic && max_anisotropy > 1.0 {
        create_info = create_info
            .anisotropy_enable(true)
            .max_anisotropy(max_anisotropy);
    }
    create_info
}

fn address_mode_to_vk(mode: AddressMode) -> vk::SamplerAddressMode {
    match mode {
        AddressMode::ClampToEdge => vk::SamplerAddressMode::CLAMP_TO_EDGE,
        AddressMode::Repeat => vk::SamplerAddressMode::REPEAT,
        AddressMode::MirrorRepeat => vk::SamplerAddressMode::MIRRORED_REPEAT,
        AddressMode::ClampToBorder => vk::SamplerAddressMode::CLAMP_TO_BORDER,
    }
}

/// Maps an RGBA border color to the closest Vulkan [`vk::BorderColor`] enum.
/// wgpu exposes a fixed set; arbitrary float borders are approximated.
fn border_color_to_vk(color: [f32; 4]) -> vk::BorderColor {
    let [r, g, b, a] = color;
    if a == 0.0 {
        vk::BorderColor::FLOAT_TRANSPARENT_BLACK
    } else if r == 1.0 && g == 1.0 && b == 1.0 {
        vk::BorderColor::FLOAT_OPAQUE_WHITE
    } else {
        vk::BorderColor::FLOAT_OPAQUE_BLACK
    }
}

fn depth_compare_op(compare: CompareFunction) -> vk::CompareOp {
    match compare {
        CompareFunction::Never => vk::CompareOp::NEVER,
        CompareFunction::Less => vk::CompareOp::LESS,
        CompareFunction::Equal => vk::CompareOp::EQUAL,
        CompareFunction::LessEqual => vk::CompareOp::LESS_OR_EQUAL,
        CompareFunction::Greater => vk::CompareOp::GREATER,
        CompareFunction::NotEqual => vk::CompareOp::NOT_EQUAL,
        CompareFunction::GreaterEqual => vk::CompareOp::GREATER_OR_EQUAL,
        CompareFunction::Always => vk::CompareOp::ALWAYS,
    }
}

fn resource_binding_type_to_vk(binding_type: BindingType) -> vk::DescriptorType {
    match binding_type {
        BindingType::UniformBuffer => vk::DescriptorType::UNIFORM_BUFFER,
        BindingType::StorageBuffer | BindingType::StorageBufferReadWrite => {
            vk::DescriptorType::STORAGE_BUFFER
        }
        BindingType::SampledTexture => vk::DescriptorType::SAMPLED_IMAGE,
        BindingType::StorageTexture { .. } => vk::DescriptorType::STORAGE_IMAGE,
        BindingType::Sampler => vk::DescriptorType::SAMPLER,
    }
}

fn shader_stages_to_vk(stages: ShaderStages) -> vk::ShaderStageFlags {
    let mut flags = vk::ShaderStageFlags::empty();
    if stages.contains(ShaderStages::VERTEX) {
        flags |= vk::ShaderStageFlags::VERTEX;
    }
    if stages.contains(ShaderStages::FRAGMENT) {
        flags |= vk::ShaderStageFlags::FRAGMENT;
    }
    if stages.contains(ShaderStages::COMPUTE) {
        flags |= vk::ShaderStageFlags::COMPUTE;
    }
    flags
}

fn vertex_format_to_vk(format: VertexFormat) -> vk::Format {
    match format {
        VertexFormat::Float32 => vk::Format::R32_SFLOAT,
        VertexFormat::Float32x2 => vk::Format::R32G32_SFLOAT,
        VertexFormat::Float32x3 => vk::Format::R32G32B32_SFLOAT,
        VertexFormat::Float32x4 => vk::Format::R32G32B32A32_SFLOAT,
        VertexFormat::Uint32 => vk::Format::R32_UINT,
        VertexFormat::Uint32x2 => vk::Format::R32G32_UINT,
        VertexFormat::Uint32x3 => vk::Format::R32G32B32_UINT,
        VertexFormat::Uint32x4 => vk::Format::R32G32B32A32_UINT,
        VertexFormat::Unorm8x4 => vk::Format::R8G8B8A8_UNORM,
        VertexFormat::Uint8x4 => vk::Format::R8G8B8A8_UINT,
        VertexFormat::Unorm16x2 => vk::Format::R16G16_UNORM,
        VertexFormat::Unorm16x4 => vk::Format::R16G16B16A16_UNORM,
    }
}

enum PendingDescriptorWrite {
    Buffer {
        binding: u32,
        descriptor_type: vk::DescriptorType,
        info_index: usize,
    },
    Image {
        binding: u32,
        descriptor_type: vk::DescriptorType,
        info_index: usize,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_compute_family_selection_skips_zero_queue_and_graphics_families() {
        let with = |queue_count, queue_flags| vk::QueueFamilyProperties {
            queue_count,
            queue_flags,
            ..Default::default()
        };
        let families = [
            with(1, vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE),
            with(0, vk::QueueFlags::COMPUTE),
            with(1, vk::QueueFlags::TRANSFER),
            with(1, vk::QueueFlags::COMPUTE | vk::QueueFlags::TRANSFER),
        ];
        assert_eq!(select_compute_only_family(&families), Some(3));
        assert_eq!(select_compute_only_family(&families[..3]), None);
    }

    #[test]
    fn cross_family_buffer_sharing_is_restricted_to_compute_buffer_abi() {
        let compute = Some(2);
        assert!(concurrent_compute_sharing_required(
            false,
            compute,
            BufferUsage::STORAGE | BufferUsage::COPY_DST,
        ));
        assert!(concurrent_compute_sharing_required(
            false,
            compute,
            BufferUsage::UNIFORM | BufferUsage::COPY_DST,
        ));
        assert!(!concurrent_compute_sharing_required(
            false,
            compute,
            BufferUsage::COPY_DST,
        ));
        assert!(!concurrent_compute_sharing_required(
            true,
            compute,
            BufferUsage::STORAGE,
        ));
        assert!(!concurrent_compute_sharing_required(
            false,
            None,
            BufferUsage::STORAGE,
        ));
    }

    #[test]
    #[ignore = "requires a Vulkan hardware adapter"]
    fn frame_gpu_timestamps_follow_the_queue_clock() {
        let mut device =
            VulkanDevice::new(&gfx_core::DeviceDescriptor::default()).expect("Vulkan device");
        assert_eq!(
            device.frame_gpu_timestamps_supported(),
            device.timestamp_clock.is_some(),
            "timestamp support must follow the graphics queue clock"
        );
        assert_eq!(
            device
                .last_frame_gpu_timestamps()
                .expect("non-blocking timestamp read"),
            None,
            "no swapchain frame has been probed yet"
        );
    }

    #[test]
    fn native_sidecar_requires_second_queue_in_same_graphics_family() {
        assert!(!independent_graphics_queue_enabled(0));
        assert!(!independent_graphics_queue_enabled(1));
        assert!(independent_graphics_queue_enabled(2));
        assert!(independent_graphics_queue_enabled(4));
        assert!(independent_graphics_queue_enabled(8));
    }

    #[test]
    fn native_graph_barrier_tracks_physical_images_not_logical_variants() {
        let x = TextureId::from_parts(10, 1);
        let y1 = TextureId::from_parts(11, 1);
        let y2 = TextureId::from_parts(12, 1);
        let mut written = Vec::new();
        assert!(!graph_color_write_requires_barrier(&mut written, x));
        assert!(!graph_color_write_requires_barrier(&mut written, y1));
        assert!(graph_color_write_requires_barrier(&mut written, x));
        assert!(!graph_color_write_requires_barrier(&mut written, y2));
        assert_eq!(written, vec![x, y1, y2]);
    }

    #[test]
    fn depth_comparison_maps_to_vulkan_operations() {
        let comparisons = [
            (CompareFunction::Never, vk::CompareOp::NEVER),
            (CompareFunction::Less, vk::CompareOp::LESS),
            (CompareFunction::Equal, vk::CompareOp::EQUAL),
            (CompareFunction::LessEqual, vk::CompareOp::LESS_OR_EQUAL),
            (CompareFunction::Greater, vk::CompareOp::GREATER),
            (CompareFunction::NotEqual, vk::CompareOp::NOT_EQUAL),
            (
                CompareFunction::GreaterEqual,
                vk::CompareOp::GREATER_OR_EQUAL,
            ),
            (CompareFunction::Always, vk::CompareOp::ALWAYS),
        ];

        for (comparison, expected) in comparisons {
            assert_eq!(depth_compare_function(comparison), expected);
        }
    }

    #[test]
    fn anisotropic_sampler_uses_device_limit() {
        let desc = SamplerDescriptor {
            anisotropic: true,
            ..SamplerDescriptor::default()
        };
        let info = sampler_create_info(&desc, 16.0);

        assert_eq!(info.anisotropy_enable, vk::TRUE);
        assert_eq!(info.max_anisotropy, 16.0);
    }

    #[test]
    fn unsupported_anisotropy_keeps_configured_filter_modes() {
        let desc = SamplerDescriptor {
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Nearest,
            mipmap_filter: FilterMode::Linear,
            anisotropic: true,
            ..SamplerDescriptor::default()
        };
        let info = sampler_create_info(&desc, 1.0);

        assert_eq!(info.anisotropy_enable, vk::FALSE);
        assert_eq!(info.mag_filter, vk::Filter::LINEAR);
        assert_eq!(info.min_filter, vk::Filter::NEAREST);
        assert_eq!(info.mipmap_mode, vk::SamplerMipmapMode::LINEAR);
    }

    #[test]
    fn present_mode_falls_back_to_fifo_when_missing() {
        let mode = choose_present_mode(&[vk::PresentModeKHR::FIFO], PresentMode::Mailbox);

        assert_eq!(mode, vk::PresentModeKHR::FIFO);
    }

    #[test]
    fn present_damage_maps_to_vulkan_region() {
        let region = VulkanDevice::damage_to_present_rect(ScissorRect {
            x: 10,
            y: 20,
            width: 30,
            height: 40,
        })
        .expect("damage should fit Vulkan coordinates");

        assert_eq!(region.offset, vk::Offset2D { x: 10, y: 20 });
        assert_eq!(
            region.extent,
            vk::Extent2D {
                width: 30,
                height: 40
            }
        );
        assert_eq!(region.layer, 0);
    }

    #[test]
    fn clear_only_offscreen_pass_limits_color_and_depth_clear_to_source_halo() {
        let extent = vk::Extent2D {
            width: 1500,
            height: 900,
        };
        let region = ScissorRect {
            x: 4,
            y: 20,
            width: 900,
            height: 65,
        };
        let area = scoped_offscreen_clear_area(
            extent,
            true,
            LoadOp::Clear(ClearColor::default()),
            None,
            Some(region),
            [Some(region)],
        )
        .expect("known filter halo must limit attachment clear");
        assert_eq!(area.offset, vk::Offset2D { x: 4, y: 20 });
        assert_eq!(
            area.extent,
            vk::Extent2D {
                width: 900,
                height: 65
            }
        );
        assert_eq!(
            scoped_offscreen_clear_area(
                extent,
                true,
                LoadOp::Clear(ClearColor::default()),
                None,
                Some(region),
                std::iter::empty(),
            ),
            Some(area),
            "a transparent source with no draw still needs the filter halo cleared"
        );
    }

    #[test]
    fn retained_color_pass_scopes_depth_clear_without_clearing_color() {
        let extent = vk::Extent2D {
            width: 1500,
            height: 900,
        };
        let region = ScissorRect {
            x: 40,
            y: 20,
            width: 80,
            height: 65,
        };
        let area = scoped_offscreen_clear_area(
            extent,
            true,
            LoadOp::Load,
            Some(LoadOp::Clear(1.)),
            Some(region),
            [Some(region)],
        )
        .expect("preserved color source must clear only the dirty depth region");
        assert_eq!(area.offset, vk::Offset2D { x: 40, y: 20 });
        assert_eq!(
            area.extent,
            vk::Extent2D {
                width: 80,
                height: 65
            }
        );
        for scissors in [
            [None],
            [Some(ScissorRect {
                x: 0,
                y: 0,
                width: 1500,
                height: 900,
            })],
        ] {
            assert!(
                scoped_offscreen_clear_area(
                    extent,
                    true,
                    LoadOp::Load,
                    Some(LoadOp::Clear(1.)),
                    Some(region),
                    scissors,
                )
                .is_none()
            );
        }
        assert!(
            scoped_offscreen_clear_area(
                extent,
                true,
                LoadOp::Load,
                Some(LoadOp::Load),
                Some(region),
                [Some(region)],
            )
            .is_none()
        );
        assert!(
            scoped_offscreen_clear_area(
                extent,
                false,
                LoadOp::Load,
                Some(LoadOp::Clear(1.)),
                Some(region),
                [Some(region)],
            )
            .is_none()
        );
    }

    #[test]
    fn unknown_or_outside_draw_does_not_shrink_vulkan_clear() {
        let extent = vk::Extent2D {
            width: 1500,
            height: 900,
        };
        let region = ScissorRect {
            x: 4,
            y: 20,
            width: 900,
            height: 65,
        };
        assert!(
            scoped_offscreen_clear_area(
                extent,
                true,
                LoadOp::Clear(ClearColor::default()),
                None,
                Some(region),
                [None],
            )
            .is_none()
        );
        assert!(
            scoped_offscreen_clear_area(
                extent,
                true,
                LoadOp::Clear(ClearColor::default()),
                None,
                Some(region),
                [Some(ScissorRect {
                    x: 0,
                    y: 0,
                    width: 1500,
                    height: 900
                })],
            )
            .is_none()
        );
        assert!(
            scoped_offscreen_clear_area(
                extent,
                false,
                LoadOp::Clear(ClearColor::default()),
                None,
                Some(region),
                [Some(region)],
            )
            .is_none()
        );
    }

    #[test]
    fn retained_offscreen_render_area_limits_attachment_load_store_to_damage() {
        let extent = vk::Extent2D {
            width: 1458,
            height: 900,
        };
        let rect = retained_offscreen_render_area(
            extent,
            true,
            LoadOp::Load,
            Some(LoadOp::Load),
            [
                Some(ScissorRect {
                    x: 10,
                    y: 20,
                    width: 30,
                    height: 40,
                }),
                Some(ScissorRect {
                    x: 80,
                    y: 50,
                    width: 10,
                    height: 20,
                }),
            ],
        );
        assert_eq!(rect.offset, vk::Offset2D { x: 10, y: 20 });
        assert_eq!(
            rect.extent,
            vk::Extent2D {
                width: 80,
                height: 50
            }
        );
    }

    #[test]
    fn retained_offscreen_render_area_preserves_full_clear_and_unknown_coverage() {
        let extent = vk::Extent2D {
            width: 1458,
            height: 900,
        };
        let scissor = Some(ScissorRect {
            x: 4,
            y: 5,
            width: 8,
            height: 9,
        });
        let full = full_render_area(extent);
        assert_eq!(
            retained_offscreen_render_area(
                extent,
                true,
                LoadOp::Clear(ClearColor::default()),
                None,
                [scissor],
            ),
            full,
        );
        assert_eq!(
            retained_offscreen_render_area(
                extent,
                true,
                LoadOp::Load,
                Some(LoadOp::Clear(1.0)),
                [scissor],
            ),
            full,
        );
        assert_eq!(
            retained_offscreen_render_area(extent, false, LoadOp::Load, None, [scissor]),
            full,
        );
        assert_eq!(
            retained_offscreen_render_area(extent, true, LoadOp::Load, None, [scissor, None],),
            full,
        );
    }

    #[test]
    fn retained_offscreen_render_area_clamps_damage_to_texture() {
        let extent = vk::Extent2D {
            width: 100,
            height: 80,
        };
        let rect = retained_offscreen_render_area(
            extent,
            true,
            LoadOp::Load,
            None,
            [
                Some(ScissorRect {
                    x: 90,
                    y: 70,
                    width: 40,
                    height: 40,
                }),
                Some(ScissorRect {
                    x: 200,
                    y: 200,
                    width: 10,
                    height: 10,
                }),
            ],
        );
        assert_eq!(rect.offset, vk::Offset2D { x: 90, y: 70 });
        assert_eq!(
            rect.extent,
            vk::Extent2D {
                width: 10,
                height: 10
            }
        );
    }

    #[test]
    fn dynamic_rendering_attachment_preserves_color_and_depth_for_load() {
        for layout in [vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL] {
            let clear = rendering_attachment(vk::ImageView::null(), layout, false, vk::ClearValue::default());
            let load = rendering_attachment(vk::ImageView::null(), layout, true, vk::ClearValue::default());
            assert_eq!(clear.load_op, vk::AttachmentLoadOp::CLEAR);
            assert_eq!(load.load_op, vk::AttachmentLoadOp::LOAD);
            assert_eq!(load.store_op, vk::AttachmentStoreOp::STORE);
            assert_eq!(clear.image_layout, layout);
            assert_eq!(load.image_layout, layout);
        }
    }

    #[test]
    fn vulkan_gpu_trace_separates_main_from_offscreen_and_ignores_unused() {
        let mut totals = VulkanGpuTraceTotals::default();
        totals.record(VulkanGpuPass::Main, 100.0);
        totals.record(VulkanGpuPass::Offscreen, 250.0);
        totals.record(VulkanGpuPass::Main, 400.0);
        totals.record(VulkanGpuPass::Unused, 999.0);
        assert_eq!(totals.main_count, 2);
        assert_eq!(totals.offscreen_count, 1);
        assert_eq!(totals.main_total_us, 500.0);
        assert_eq!(totals.main_max_us, 400.0);
        assert_eq!(totals.offscreen_total_us, 250.0);
        assert_eq!(totals.offscreen_max_us, 250.0);
    }

    #[test]
    fn vulkan_gpu_timestamp_clock_wraps_valid_bits() {
        let clock = VulkanTimestampClock {
            period_ns: 2.0,
            valid_bits: 8,
        };
        assert_eq!(clock.elapsed_nanoseconds(250, 5), 22.0);
    }

    #[test]
    fn render_area_covers_complete_attachment() {
        let extent = vk::Extent2D {
            width: 1920,
            height: 1080,
        };
        let area = full_render_area(extent);

        assert_eq!(area.offset, vk::Offset2D { x: 0, y: 0 });
        assert_eq!(area.extent, extent);
    }

    #[test]
    fn physical_device_preference_rank_selects_requested_power_class() {
        assert!(
            physical_device_preference_rank(
                vk::PhysicalDeviceType::INTEGRATED_GPU,
                PowerPreference::LowPower,
            ) < physical_device_preference_rank(
                vk::PhysicalDeviceType::DISCRETE_GPU,
                PowerPreference::LowPower,
            )
        );
        assert!(
            physical_device_preference_rank(
                vk::PhysicalDeviceType::DISCRETE_GPU,
                PowerPreference::HighPerformance,
            ) < physical_device_preference_rank(
                vk::PhysicalDeviceType::INTEGRATED_GPU,
                PowerPreference::HighPerformance,
            )
        );
    }

    #[test]
    fn transparent_alpha_prefers_premultiplied() {
        let supported = vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED
            | vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED;

        assert_eq!(
            choose_composite_alpha_mode(supported, CompositeAlphaMode::Premultiplied)
                .expect("a transparent alpha mode should be selected"),
            CompositeAlphaMode::Premultiplied
        );
    }

    #[test]
    fn transparent_alpha_falls_back_to_postmultiplied() {
        assert_eq!(
            choose_composite_alpha_mode(
                vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
                CompositeAlphaMode::Premultiplied,
            )
            .expect("postmultiplied alpha should preserve transparency"),
            CompositeAlphaMode::Postmultiplied
        );
    }

    #[test]
    fn transparent_alpha_does_not_fall_back_to_opaque() {
        let error = choose_composite_alpha_mode(
            vk::CompositeAlphaFlagsKHR::OPAQUE,
            CompositeAlphaMode::Premultiplied,
        )
        .expect_err("opaque alpha must not satisfy a transparent request");

        assert!(error.to_string().contains("does not support requested"));
    }

    #[test]
    fn transparent_alpha_accepts_native_window_system_compositing() {
        assert_eq!(
            choose_composite_alpha_mode(
                vk::CompositeAlphaFlagsKHR::INHERIT,
                CompositeAlphaMode::Premultiplied,
            )
            .expect("native compositing should preserve transparency"),
            CompositeAlphaMode::Inherit
        );
    }

    #[test]
    fn format_mapping_preserves_srgb() {
        assert_eq!(
            format_to_vk(Format::Bgra8UnormSrgb),
            vk::Format::B8G8R8A8_SRGB
        );
    }

    #[test]
    fn registry_rejects_stale_handles() {
        let mut registry = ResourceRegistry::new("buffer");
        let id: BufferId = registry.insert(1_u32);
        assert_eq!(*registry.get(id).expect("resource exists"), 1);
        let _removed = registry.take(id).expect("resource can be taken");

        let error = registry.get(id).expect_err("stale handle should fail");
        assert!(error.to_string().contains("stale or invalid buffer handle"));
    }

    #[test]
    fn registry_replace_live_preserves_handle_generation() {
        let mut registry = ResourceRegistry::new("swapchain");
        let id: gfx_core::SwapchainId = registry.insert(1_u32);

        let old = registry
            .replace_live(id, 2_u32)
            .expect("live resource should be replaced");

        assert_eq!(old, 1);
        assert_eq!(
            *registry.get(id).expect("same handle should remain live"),
            2
        );
        assert_eq!(id.generation(), 1);
    }

    #[test]
    fn buffer_usage_mapping_preserves_copy_dst() {
        let usage = buffer_usage_to_vk(BufferUsage::COPY_DST | BufferUsage::VERTEX);

        assert!(usage.contains(vk::BufferUsageFlags::TRANSFER_DST));
        assert!(usage.contains(vk::BufferUsageFlags::VERTEX_BUFFER));
    }

    #[test]
    fn texture_usage_mapping_preserves_sampled() {
        let usage = texture_usage_to_vk(TextureUsage::COPY_DST | TextureUsage::SAMPLED);

        assert!(usage.contains(vk::ImageUsageFlags::TRANSFER_DST));
        assert!(usage.contains(vk::ImageUsageFlags::SAMPLED));
    }

    #[test]
    fn texture_upload_overlap_requires_shared_mip_and_intersecting_rectangles() {
        let first = VulkanTextureUpload {
            texture: TextureId::from_parts(0, 0),
            source: vk::Buffer::null(),
            image: vk::Image::null(),
            old_layout: vk::ImageLayout::UNDEFINED,
            mip_level_count: 2,
            layout: TextureDataLayout::new(0, 7, 5).unwrap(),
            mip_level: 0,
            layer: 0,
            origin: gfx_core::Origin2d::ZERO,
            size: gfx_core::Extent2d::new(7, 5).unwrap(),
        };
        let mut second = VulkanTextureUpload {
            origin: gfx_core::Origin2d { x: 2, y: 1 },
            size: gfx_core::Extent2d::new(3, 3).unwrap(),
            ..first
        };
        assert!(texture_uploads_overlap(&first, &second));
        second.origin.x = 7;
        assert!(!texture_uploads_overlap(&first, &second));
        second.origin.x = 2;
        second.mip_level = 1;
        assert!(!texture_uploads_overlap(&first, &second));
        second.mip_level = 0;
        second.layer = 1;
        assert!(
            !texture_uploads_overlap(&first, &second),
            "copies to different array slices must not alias"
        );
        second.layer = 0;
        assert!(texture_uploads_overlap(&first, &second));
    }

    #[test]
    fn r8_staging_accepts_odd_pitch_but_requires_aligned_source_offset() {
        let mut descriptor = TextureWriteDescriptor {
            texture: TextureId::from_parts(0, 0),
            mip_level: 0,
            layout: TextureDataLayout::new(4, 9, 5).unwrap(),
            origin: gfx_core::Origin2d::ZERO,
            size: gfx_core::Extent2d::new(7, 5).unwrap(),
        };
        assert!(validate_texture_staging_layout(descriptor, Format::R8Unorm).is_ok());
        descriptor.layout.offset = 1;
        assert!(validate_texture_staging_layout(descriptor, Format::R8Unorm).is_err());
    }

    #[test]
    fn texture_staging_layout_preserves_source_offset() {
        let descriptor = TextureWriteDescriptor {
            texture: TextureId::from_parts(0, 0),
            mip_level: 0,
            layout: TextureDataLayout::new(12, 16, 2).expect("layout should be valid"),
            origin: gfx_core::Origin2d::ZERO,
            size: gfx_core::Extent2d::new(4, 2).expect("extent should be valid"),
        };
        let allocation = UploadAllocation {
            page_index: 0,
            offset: 512,
            size: 44,
            end_offset: 768,
        };

        let layout =
            texture_staging_layout(allocation, descriptor).expect("staging layout should be valid");

        assert_eq!(layout.offset, 524);
        assert_eq!(layout.bytes_per_row.get(), 16);
        assert_eq!(layout.rows_per_image.get(), 2);
    }

    #[test]
    fn texture_staging_layout_rejects_partial_texel_stride() {
        let descriptor = TextureWriteDescriptor {
            texture: TextureId::from_parts(0, 0),
            mip_level: 0,
            layout: TextureDataLayout::new(4, 10, 2).expect("layout should be valid"),
            origin: gfx_core::Origin2d::ZERO,
            size: gfx_core::Extent2d::new(2, 2).expect("extent should be valid"),
        };

        let error = validate_texture_staging_layout(descriptor, Format::Bgra8Unorm)
            .expect_err("partial texel stride should be rejected");

        assert!(matches!(error, Error::InvalidInput(_)));
    }

    #[test]
    fn texture_staging_layout_rejects_partial_texel_offset() {
        let descriptor = TextureWriteDescriptor {
            texture: TextureId::from_parts(0, 0),
            mip_level: 0,
            layout: TextureDataLayout::new(2, 8, 2).expect("layout should be valid"),
            origin: gfx_core::Origin2d::ZERO,
            size: gfx_core::Extent2d::new(2, 2).expect("extent should be valid"),
        };

        let error = validate_texture_staging_layout(descriptor, Format::Bgra8Unorm)
            .expect_err("partial texel offset should be rejected");

        assert!(matches!(error, Error::InvalidInput(_)));
    }

    #[test]
    fn timestamp_clock_handles_wrapping_valid_bits() {
        let clock = VulkanTimestampClock {
            period_ns: 2.0,
            valid_bits: 8,
        };

        assert!((clock.elapsed_nanoseconds(250, 5) - 22.0).abs() < f64::EPSILON);
    }

    #[test]
    fn timestamp_clock_preserves_full_width_delta() {
        let clock = VulkanTimestampClock {
            period_ns: 1.0,
            valid_bits: 64,
        };

        assert!((clock.elapsed_nanoseconds(10, 15) - 5.0).abs() < f64::EPSILON);
        assert_eq!(clock.duration_ticks(10, 15), 5);
        assert_eq!(clock.frequency_hz(), 1_000_000_000);
    }

    #[test]
    fn color_write_flags_maps_channels_to_vk_components() {
        use vk::ColorComponentFlags;
        assert_eq!(
            color_write_flags(ColorWrites::ALL),
            ColorComponentFlags::R
                | ColorComponentFlags::G
                | ColorComponentFlags::B
                | ColorComponentFlags::A
        );
        assert_eq!(
            color_write_flags(ColorWrites::COLOR),
            ColorComponentFlags::R | ColorComponentFlags::G | ColorComponentFlags::B
        );
        assert_eq!(color_write_flags(ColorWrites::RED), ColorComponentFlags::R);
        assert_eq!(color_write_flags(ColorWrites::ALPHA), ColorComponentFlags::A);
        assert_eq!(
            color_write_flags(ColorWrites::RED | ColorWrites::ALPHA),
            ColorComponentFlags::R | ColorComponentFlags::A
        );
        assert!(color_write_flags(ColorWrites::empty()).is_empty());
    }

    #[test]
    fn sampler_compare_and_address_modes_map_to_vk() {
        assert_eq!(
            address_mode_to_vk(AddressMode::ClampToEdge),
            vk::SamplerAddressMode::CLAMP_TO_EDGE
        );
        assert_eq!(
            address_mode_to_vk(AddressMode::Repeat),
            vk::SamplerAddressMode::REPEAT
        );
        assert_eq!(
            address_mode_to_vk(AddressMode::MirrorRepeat),
            vk::SamplerAddressMode::MIRRORED_REPEAT
        );
        assert_eq!(
            address_mode_to_vk(AddressMode::ClampToBorder),
            vk::SamplerAddressMode::CLAMP_TO_BORDER
        );
        assert_eq!(depth_compare_op(CompareFunction::Less), vk::CompareOp::LESS);
        assert_eq!(
            depth_compare_op(CompareFunction::GreaterEqual),
            vk::CompareOp::GREATER_OR_EQUAL
        );
    }

    #[test]
    fn sampler_border_color_maps_to_nearest_vk_enum() {
        assert_eq!(
            border_color_to_vk([0.0, 0.0, 0.0, 0.0]),
            vk::BorderColor::FLOAT_TRANSPARENT_BLACK
        );
        assert_eq!(
            border_color_to_vk([1.0, 1.0, 1.0, 1.0]),
            vk::BorderColor::FLOAT_OPAQUE_WHITE
        );
        assert_eq!(
            border_color_to_vk([0.0, 0.0, 0.0, 1.0]),
            vk::BorderColor::FLOAT_OPAQUE_BLACK
        );
    }

    #[test]
    fn timestamp_clock_reports_wrapped_duration_and_frequency() {
        let clock = VulkanTimestampClock {
            period_ns: 2.0,
            valid_bits: 8,
        };

        assert_eq!(clock.duration_ticks(250, 5), 11);
        assert_eq!(clock.frequency_hz(), 500_000_000);
    }

    #[test]
    fn timestamp_clock_rejects_unusable_period() {
        let clock = VulkanTimestampClock {
            period_ns: 0.0,
            valid_bits: 64,
        };

        assert_eq!(clock.frequency_hz(), 0);
    }
}

#[cfg(test)]
mod conformance_tests {
    use super::*;

    #[test]
    #[ignore = "requires Vulkan hardware"]
    fn msaa_resolve_roundtrip() {
        let mut device =
            VulkanDevice::new(&gfx_core::DeviceDescriptor::default()).expect("hardware device");
        gfx_core::conformance::msaa_resolve_roundtrip(&mut device).expect("MSAA resolve roundtrip");
    }

    #[test]
    #[ignore = "requires GPU hardware"]
    fn format_allocation_roundtrip() {
        let mut device =
            VulkanDevice::new(&gfx_core::DeviceDescriptor::default()).expect("hardware device");
        gfx_core::conformance::format_allocation_roundtrip(&mut device)
            .expect("format allocation roundtrip");
    }

    #[test]
    #[ignore = "requires GPU hardware"]
    fn texture_buffer_roundtrip() {
        let mut device =
            VulkanDevice::new(&gfx_core::DeviceDescriptor::default()).expect("hardware device");
        gfx_core::conformance::texture_buffer_roundtrip(&mut device)
            .expect("texture buffer roundtrip");
    }
}
