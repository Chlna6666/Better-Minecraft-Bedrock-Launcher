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
    sync::Arc,
    time::Instant,
};

use crate::error::VulkanError;
use crate::registry::ResourceRegistry;
use ash::{Entry, Instance, khr, vk};
use gfx_core::{
    AdapterInfo, AddressMode, BackendCapabilities, BackendKind, BeginRenderPassDesc, BlendMode,
    BufferDesc, BufferId, BufferUsage, ClearColor, ColorAttachmentDesc, CommandEncoderDesc,
    CommandEncoderId, CompositeAlphaMode, DeviceDesc, DrawDesc, DrawStepDesc, DrawTriangleDesc,
    FilterMode, Format, GfxBackend, GfxCommandDevice, GfxDiagnosticsDevice, GfxError,
    GfxPipelineDevice, GfxPresentationDevice, GfxResourceDevice, GfxSubmissionDevice,
    GfxSurfaceDevice, GfxThreadingMode, IndexBufferBinding, IndexFormat, LoadOp, MemoryLocation,
    PipelineLayoutDesc, PipelineLayoutId, PresentMode, PrimitiveTopology,
    RenderPassDepthAttachment, RenderPassDesc, RenderPassId, RenderPipelineDesc, RenderPipelineId,
    RenderStepDescriptor, RenderStepList, RenderStepRef, RenderTarget, ResourceBindingResource,
    ResourceBindingType, ResourceSetDesc, ResourceSetId, ResourceSetLayoutDesc,
    ResourceSetLayoutId, ResourceStats, Result, SamplerDesc, SamplerId, ShaderBinary, ShaderCode,
    ShaderModuleDesc, ShaderModuleId, ShaderStage, ShaderStages, SubmissionId, SubmissionStatus,
    SurfaceConfig, SurfaceDesc, SurfaceId, TextureDataLayout, TextureDesc, TextureDimension,
    TextureId, TextureUsage, TextureViewDesc, TextureViewId, TextureWriteDesc, VertexFormat,
};
use gfx_memory::{
    DeferredFreeQueue, MemoryAllocation, MemoryAllocator, UploadAllocation, UploadRingAllocator,
    UploadRingAllocatorDesc, VulkanMemoryAllocatorDesc,
};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

const FRAMES_IN_FLIGHT: usize = 2;

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
/// Returns [`GfxError`] if the Vulkan loader, instance creation, or physical
/// device enumeration fails.
pub fn enumerate_adapter_info() -> Result<Vec<AdapterInfo>> {
    let entry = load_entry()?;
    let instance = create_instance(&entry, "nova-gfx adapter enumeration")?;
    let devices = unsafe { instance.enumerate_physical_devices() }.map_err(VulkanError::from)?;
    let mut adapters = Vec::with_capacity(devices.len());
    for physical_device in devices {
        // SAFETY: Physical device belongs to this instance.
        let properties = unsafe { instance.get_physical_device_properties(physical_device) };
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
    entry: Entry,
    instance: Instance,
    surface_loader: khr::surface::Instance,
    physical_device: vk::PhysicalDevice,
    device: Arc<ash::Device>,
    graphics_queue: vk::Queue,
    present_queue: vk::Queue,
    graphics_queue_family_index: u32,
    present_queue_family_index: u32,
    swapchain_loader: khr::swapchain::Device,
    allocator: MemoryAllocator,
    buffers: ResourceRegistry<VulkanBuffer>,
    textures: ResourceRegistry<VulkanTexture>,
    texture_views: ResourceRegistry<VulkanTextureView>,
    samplers: ResourceRegistry<VulkanSampler>,
    resource_set_layouts: ResourceRegistry<VulkanResourceSetLayout>,
    resource_sets: ResourceRegistry<VulkanResourceSet>,
    pipeline_layouts: ResourceRegistry<VulkanPipelineLayout>,
    shader_modules: ResourceRegistry<VulkanShaderModule>,
    render_passes: ResourceRegistry<VulkanRenderPass>,
    render_pipelines: ResourceRegistry<VulkanRenderPipeline>,
    command_encoders: ResourceRegistry<VulkanCommandEncoder>,
    submissions: ResourceRegistry<VulkanSubmission>,
    surfaces: ResourceRegistry<VulkanSurface>,
    swapchains: ResourceRegistry<VulkanSwapchain>,
    descriptor_pool: vk::DescriptorPool,
    upload_ring: UploadRingAllocator,
    upload_pages: Vec<Option<VulkanBuffer>>,
    deferred_destroys: DeferredFreeQueue<DeferredResource>,
    submitted_frames: u64,
}

impl VulkanDevice {
    /// Creates a Vulkan device.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] if Vulkan initialization fails.
    pub fn new(desc: &DeviceDesc) -> Result<Self> {
        let entry = load_entry()?;
        let instance = create_instance(&entry, &desc.application_name)?;
        let surface_loader = khr::surface::Instance::new(&entry, &instance);
        let physical_device = pick_physical_device_without_surface(&instance)?;
        let queue_families = queue_family_indices_without_surface(&instance, physical_device)?;
        let (device, graphics_queue, present_queue) =
            create_device(&instance, physical_device, queue_families)?;
        let device = Arc::new(device);
        let swapchain_loader = khr::swapchain::Device::new(&instance, &device);
        let allocator = MemoryAllocator::new_vulkan(VulkanMemoryAllocatorDesc {
            instance: instance.clone(),
            device: (*device).clone(),
            physical_device,
        })?;
        let upload_ring = UploadRingAllocator::new(UploadRingAllocatorDesc::default())?;
        let descriptor_pool = create_descriptor_pool(&device)?;

        Ok(Self {
            entry,
            instance,
            surface_loader,
            physical_device,
            device,
            graphics_queue,
            present_queue,
            graphics_queue_family_index: queue_families.graphics,
            present_queue_family_index: queue_families.present,
            swapchain_loader,
            allocator,
            buffers: ResourceRegistry::new("buffer"),
            textures: ResourceRegistry::new("texture"),
            texture_views: ResourceRegistry::new("texture view"),
            samplers: ResourceRegistry::new("sampler"),
            resource_set_layouts: ResourceRegistry::new("resource set layout"),
            resource_sets: ResourceRegistry::new("resource set"),
            pipeline_layouts: ResourceRegistry::new("pipeline layout"),
            shader_modules: ResourceRegistry::new("shader module"),
            render_passes: ResourceRegistry::new("render pass"),
            render_pipelines: ResourceRegistry::new("render pipeline"),
            command_encoders: ResourceRegistry::new("command encoder"),
            submissions: ResourceRegistry::new("submission"),
            surfaces: ResourceRegistry::new("surface"),
            swapchains: ResourceRegistry::new("swapchain"),
            descriptor_pool,
            upload_ring,
            upload_pages: Vec::new(),
            deferred_destroys: DeferredFreeQueue::new(),
            submitted_frames: 0,
        })
    }

    /// Creates a native Vulkan surface from raw-window-handle traits.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when surface creation or present queue selection fails.
    fn create_surface<W>(&mut self, window: &W, _desc: &SurfaceDesc) -> Result<SurfaceId>
    where
        W: HasDisplayHandle + HasWindowHandle + ?Sized,
    {
        let display_handle = window
            .display_handle()
            .map_err(|error| GfxError::Backend(error.to_string()))?
            .as_raw();
        let window_handle = window
            .window_handle()
            .map_err(|error| GfxError::Backend(error.to_string()))?
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
        .map_err(|error| GfxError::Backend(error.to_string()))?;
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
    /// Returns [`GfxError`] when swapchain resources cannot be created.
    pub fn configure_surface(
        &mut self,
        surface: SurfaceId,
        config: SurfaceConfig,
    ) -> Result<SurfaceId> {
        let _ = self.create_swapchain(surface, config)?;
        Ok(surface)
    }

    /// Creates or replaces a swapchain for a surface.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when swapchain resources cannot be created.
    fn create_swapchain(
        &mut self,
        surface_id: SurfaceId,
        config: SurfaceConfig,
    ) -> Result<gfx_core::SwapchainId> {
        let surface = self.surfaces.get(surface_id)?.surface;
        let swapchain = self.build_swapchain(surface, config)?;

        Ok(self.swapchains.insert(swapchain))
    }

    /// Recreates an existing swapchain.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when the existing swapchain is invalid or recreation fails.
    pub fn resize_swapchain(
        &mut self,
        swapchain_id: gfx_core::SwapchainId,
        width: u32,
        height: u32,
    ) -> Result<()> {
        if width == 0 || height == 0 {
            return Ok(());
        }
        // SAFETY: Device is valid and waiting before swapchain resource replacement is valid.
        unsafe { self.device.device_wait_idle() }.map_err(VulkanError::from)?;
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
    /// Returns [`GfxError`] when the existing swapchain is invalid or recreation fails.
    pub fn reconfigure_swapchain(
        &mut self,
        swapchain_id: gfx_core::SwapchainId,
        config: SurfaceConfig,
    ) -> Result<()> {
        // SAFETY: Device is valid and waiting before swapchain resource replacement is valid.
        unsafe { self.device.device_wait_idle() }.map_err(VulkanError::from)?;
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
    /// Returns [`GfxError`] when validation or Vulkan allocation fails.
    fn create_buffer(&mut self, desc: &BufferDesc) -> Result<BufferId> {
        desc.validate()?;
        let create_info = vk::BufferCreateInfo::default()
            .size(desc.size)
            .usage(buffer_usage_to_vk(desc.usage))
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
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
    /// Returns [`GfxError`] when the handle is invalid or upload fails.
    fn write_buffer(&mut self, buffer_id: BufferId, offset: u64, data: &[u8]) -> Result<()> {
        let memory_location = self.buffers.get(buffer_id)?.desc.memory_location;
        if memory_location == MemoryLocation::CpuToGpu {
            let buffer = self.buffers.get_mut(buffer_id)?;
            let mapped = buffer
                .allocation
                .mapped_slice_mut()
                .ok_or_else(|| GfxError::Backend("buffer memory is not CPU visible".to_string()))?;
            let offset = usize::try_from(offset)
                .map_err(|error| GfxError::InvalidInput(format!("offset overflow: {error}")))?;
            let end = offset
                .checked_add(data.len())
                .ok_or_else(|| GfxError::InvalidInput("buffer write range overflow".to_string()))?;
            let target = mapped.get_mut(offset..end).ok_or_else(|| {
                GfxError::InvalidInput("buffer write range is out of bounds".to_string())
            })?;
            target.copy_from_slice(data);
            return Ok(());
        }

        let upload = self.write_upload_data(data)?;
        let staging_buffer = self.upload_page_buffer(upload.page_index)?;
        let target_buffer = self.buffers.get(buffer_id)?.buffer;
        self.copy_buffer_once(
            staging_buffer,
            target_buffer,
            upload.offset,
            offset,
            upload.size,
        )?;
        self.complete_synchronous_upload();
        Ok(())
    }

    /// Creates a 2D texture.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when validation or Vulkan allocation fails.
    fn create_texture(&mut self, desc: &TextureDesc) -> Result<TextureId> {
        desc.validate()?;
        if desc.dimension != TextureDimension::D2 {
            return Err(GfxError::InvalidInput(
                "only 2D textures are supported in phase 1".to_string(),
            ));
        }
        let create_info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format_to_vk(desc.format))
            .extent(vk::Extent3D {
                width: desc.size.width(),
                height: desc.size.height(),
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(texture_usage_to_vk(desc.usage))
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: Device is valid and image create info is self-contained.
        let image =
            unsafe { self.device.create_image(&create_info, None) }.map_err(VulkanError::from)?;
        // SAFETY: Image was created from this device and is valid here.
        let requirements = unsafe { self.device.get_image_memory_requirements(image) };
        let name = desc.label.as_deref().unwrap_or("nova-gfx-texture");
        let allocation = self.allocator.allocate_vulkan_image(
            name,
            requirements,
            desc.memory_location,
            image,
        )?;
        let (memory, offset) = vulkan_memory(&allocation)?;
        // SAFETY: Allocation was created from this image's memory requirements and outlives it.
        unsafe { self.device.bind_image_memory(image, memory, offset) }
            .map_err(VulkanError::from)?;

        Ok(self.textures.insert(VulkanTexture {
            image,
            allocation,
            desc: desc.clone(),
            layout: vk::ImageLayout::UNDEFINED,
        }))
    }

    /// Writes data into a texture using a staging buffer.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when upload fails.
    fn write_texture(&mut self, desc: TextureWriteDesc, data: &[u8]) -> Result<()> {
        let upload = self.write_upload_data(data)?;
        let staging_buffer = self.upload_page_buffer(upload.page_index)?;
        let (image, old_layout) = {
            let texture = self.textures.get(desc.texture)?;
            (texture.image, texture.layout)
        };
        self.copy_buffer_to_texture_once(
            staging_buffer,
            image,
            old_layout,
            TextureDataLayout::new(
                upload.offset,
                desc.layout.bytes_per_row.get(),
                desc.layout.rows_per_image.get(),
            )?,
            desc.origin,
            desc.size,
        )?;
        self.complete_synchronous_upload();
        self.textures.get_mut(desc.texture)?.layout = vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL;
        let _ = self.textures.get(desc.texture)?.desc.format;
        Ok(())
    }

    /// Creates a texture view.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when the texture is invalid or view creation fails.
    fn create_texture_view(&mut self, desc: &TextureViewDesc) -> Result<TextureViewId> {
        let image = self.textures.get(desc.texture)?.image;
        let view = create_image_view(
            &self.device,
            image,
            format_to_vk(desc.format),
            image_aspect_for_format(desc.format),
        )?;
        Ok(self.texture_views.insert(VulkanTextureView {
            view,
            texture: desc.texture,
        }))
    }

    /// Creates a sampler.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when Vulkan sampler creation fails.
    fn create_sampler(&mut self, desc: &SamplerDesc) -> Result<SamplerId> {
        let create_info = vk::SamplerCreateInfo::default()
            .mag_filter(filter_to_vk(desc.mag_filter))
            .min_filter(filter_to_vk(desc.min_filter))
            .address_mode_u(address_mode_to_vk(desc.address_mode_u))
            .address_mode_v(address_mode_to_vk(desc.address_mode_v))
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .max_lod(1.0);
        // SAFETY: Device is valid and sampler create info is self-contained.
        let sampler =
            unsafe { self.device.create_sampler(&create_info, None) }.map_err(VulkanError::from)?;
        Ok(self.samplers.insert(VulkanSampler { sampler }))
    }

    /// Creates a resource set layout.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when validation or Vulkan descriptor set layout creation fails.
    fn create_resource_set_layout(
        &mut self,
        desc: &ResourceSetLayoutDesc,
    ) -> Result<ResourceSetLayoutId> {
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
        Ok(self.resource_set_layouts.insert(VulkanResourceSetLayout {
            layout,
            desc: desc.clone(),
        }))
    }

    /// Creates a pipeline layout from resource set layouts.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when any layout handle is invalid or Vulkan creation fails.
    fn create_pipeline_layout(&mut self, desc: &PipelineLayoutDesc) -> Result<PipelineLayoutId> {
        desc.validate()?;
        let layouts = desc
            .resource_set_layouts
            .iter()
            .copied()
            .map(|layout| Ok(self.resource_set_layouts.get(layout)?.layout))
            .collect::<Result<Vec<_>>>()?;
        let create_info = vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts);
        // SAFETY: Descriptor set layouts belong to this device and remain live via registry handles.
        let layout = unsafe { self.device.create_pipeline_layout(&create_info, None) }
            .map_err(VulkanError::from)?;
        Ok(self.pipeline_layouts.insert(VulkanPipelineLayout {
            layout,
            resource_set_layouts: desc.resource_set_layouts.clone(),
        }))
    }

    /// Creates a resource set and writes descriptor bindings.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when validation, handles, or descriptor writes fail.
    #[expect(
        clippy::too_many_lines,
        reason = "resource set creation keeps descriptor writes together for backend validation"
    )]
    fn create_resource_set(&mut self, desc: &ResourceSetDesc) -> Result<ResourceSetId> {
        let layout_desc = self.resource_set_layouts.get(desc.layout)?.desc.clone();
        desc.validate_against(&layout_desc)?;
        let layout = self.resource_set_layouts.get(desc.layout)?.layout;
        let set_layouts = [layout];
        let alloc_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(self.descriptor_pool)
            .set_layouts(&set_layouts);
        // SAFETY: Descriptor pool and set layout belong to this device.
        let descriptor_set = unsafe { self.device.allocate_descriptor_sets(&alloc_info) }
            .map_err(VulkanError::from)?
            .into_iter()
            .next()
            .ok_or_else(|| GfxError::Backend("failed to allocate descriptor set".to_string()))?;

        let mut buffer_infos = Vec::new();
        let mut image_infos = Vec::new();
        let mut pending_writes = Vec::new();
        for binding in &desc.bindings {
            match binding.resource {
                ResourceBindingResource::Buffer(buffer_binding) => {
                    let buffer = self.buffers.get(buffer_binding.buffer)?;
                    let binding_type = layout_desc
                        .entries
                        .iter()
                        .find(|entry| entry.binding == binding.binding)
                        .map(|entry| resource_binding_type_to_vk(entry.binding_type))
                        .ok_or_else(|| {
                            GfxError::InvalidInput(format!(
                                "resource set layout is missing binding {}",
                                binding.binding
                            ))
                        })?;
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
                ResourceBindingResource::Texture(texture_binding) => {
                    let view = self.texture_views.get(texture_binding.texture_view)?;
                    image_infos.push(
                        vk::DescriptorImageInfo::default()
                            .image_view(view.view)
                            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL),
                    );
                    pending_writes.push(PendingDescriptorWrite::Image {
                        binding: binding.binding,
                        descriptor_type: vk::DescriptorType::SAMPLED_IMAGE,
                        info_index: image_infos.len() - 1,
                    });
                }
                ResourceBindingResource::Sampler(sampler_binding) => {
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
        let writes = pending_writes
            .iter()
            .map(|write| match *write {
                PendingDescriptorWrite::Buffer {
                    binding,
                    descriptor_type,
                    info_index,
                } => {
                    let buffer_info = buffer_infos.get(info_index).ok_or_else(|| {
                        GfxError::Backend("descriptor buffer info index is invalid".to_string())
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
                        GfxError::Backend("descriptor image info index is invalid".to_string())
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
        Ok(self.resource_sets.insert(VulkanResourceSet {
            descriptor_set,
            layout: desc.layout,
        }))
    }

    /// Creates a shader module.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when validation or Vulkan module creation fails.
    fn create_shader_module(&mut self, desc: &ShaderModuleDesc) -> Result<ShaderModuleId> {
        desc.validate()?;
        let ShaderCode::Spirv(spirv) = &desc.binary.code else {
            return Err(GfxError::Shader(
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
    /// Returns [`GfxError`] when Vulkan render pass creation fails.
    fn create_render_pass(&mut self, desc: &RenderPassDesc) -> Result<RenderPassId> {
        let render_pass =
            create_render_pass(&self.device, format_to_vk(desc.color_attachment.format))?;
        Ok(self.render_passes.insert(VulkanRenderPass {
            render_pass,
            color_format: desc.color_attachment.format,
        }))
    }

    /// Creates a graphics render pipeline.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when validation or Vulkan pipeline creation fails.
    fn create_render_pipeline(
        &mut self,
        desc: &RenderPipelineDesc,
        _viewport_extent: gfx_core::Extent2d,
    ) -> Result<RenderPipelineId> {
        desc.validate()?;
        let render_pass = self.render_passes.get(desc.render_pass)?.render_pass;
        let vertex_shader = self.shader_modules.get(desc.vertex_shader)?;
        let fragment_shader = self.shader_modules.get(desc.fragment_shader)?;
        if vertex_shader.stage != ShaderStage::Vertex {
            return Err(GfxError::InvalidInput(
                "vertex shader module must use ShaderStage::Vertex".to_string(),
            ));
        }
        if fragment_shader.stage != ShaderStage::Fragment {
            return Err(GfxError::InvalidInput(
                "fragment shader module must use ShaderStage::Fragment".to_string(),
            ));
        }
        let (pipeline_layout, owns_pipeline_layout) =
            if let Some(pipeline_layout_id) = desc.pipeline_layout {
                (self.pipeline_layouts.get(pipeline_layout_id)?.layout, false)
            } else {
                (create_empty_pipeline_layout(&self.device)?, true)
            };
        let (pipeline_layout, pipeline) = create_graphics_pipeline(&GraphicsPipelineBuild {
            device: &self.device,
            render_pass,
            pipeline_layout,
            vertex_shader,
            vertex_entry_point: &desc.vertex_entry_point,
            fragment_shader,
            fragment_entry_point: &desc.fragment_entry_point,
            desc,
        })?;
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
    /// Returns [`GfxError`] when command pool or command buffer allocation fails.
    fn create_command_encoder(&mut self, _desc: &CommandEncoderDesc) -> Result<CommandEncoderId> {
        let command_pool = create_command_pool(&self.device, self.graphics_queue_family_index)?;
        let command_buffer = allocate_command_buffers(&self.device, command_pool, 1)?
            .into_iter()
            .next()
            .ok_or_else(|| GfxError::Backend("failed to allocate command buffer".to_string()))?;
        Ok(self.command_encoders.insert(VulkanCommandEncoder {
            command_pool,
            command_buffer,
            transient_framebuffers: Vec::new(),
            fence: vk::Fence::null(),
        }))
    }

    /// Records a render pass and draw call with optional resource sets.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when command recording or any handle lookup fails.
    fn record_draw_desc(&mut self, encoder_id: CommandEncoderId, draw: &DrawDesc) -> Result<()> {
        self.record_draw_steps_desc(
            encoder_id,
            draw.pass,
            &[DrawStepDesc {
                pipeline: draw.pipeline,
                resource_sets: draw.resource_sets.clone(),
                vertex_count: draw.vertex_count,
                first_vertex: draw.first_vertex,
                instance_count: draw.instance_count,
                first_instance: draw.first_instance,
                scissor: draw.scissor,
            }],
        )
    }

    /// Records a render pass with one or more draw steps.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when command recording or any handle lookup fails.
    fn record_draw_steps_desc(
        &mut self,
        encoder_id: CommandEncoderId,
        pass: BeginRenderPassDesc,
        steps: &[DrawStepDesc],
    ) -> Result<()> {
        self.record_render_step_list_desc(encoder_id, pass, RenderStepList::from_draw_steps(steps))
    }

    fn record_render_step_list_desc(
        &mut self,
        encoder_id: CommandEncoderId,
        pass: BeginRenderPassDesc,
        steps: RenderStepList<'_>,
    ) -> Result<()> {
        let command_buffer = self.command_encoders.get(encoder_id)?.command_buffer;
        let mut transient_framebuffer = None;
        let mut render_target_texture = None;
        let mut render_target_transition = None;
        let (framebuffer, extent) = match pass.target {
            RenderTarget::Swapchain {
                swapchain,
                image_index,
            } => {
                let swapchain_record = self.swapchains.get(swapchain)?;
                let image_index = usize::try_from(image_index).map_err(|error| {
                    GfxError::InvalidInput(format!("image index overflow: {error}"))
                })?;
                let framebuffer =
                    *swapchain_record
                        .framebuffers
                        .get(image_index)
                        .ok_or_else(|| {
                            GfxError::InvalidInput("swapchain image index out of range".to_string())
                        })?;
                (framebuffer, swapchain_record.extent)
            }
            RenderTarget::TextureView(texture_view_id) => {
                let texture_view = self.texture_views.get(texture_view_id)?;
                let texture = self.textures.get(texture_view.texture)?;
                let framebuffer = create_framebuffer(
                    &self.device,
                    self.render_passes.get(pass.render_pass)?.render_pass,
                    texture_view.view,
                    vk::Extent2D {
                        width: texture.desc.size.width(),
                        height: texture.desc.size.height(),
                    },
                )?;
                transient_framebuffer = Some(framebuffer);
                render_target_texture = Some(texture_view.texture);
                render_target_transition = Some(CommandRenderTargetTransition {
                    image: texture.image,
                    old_layout: texture.layout,
                });
                (
                    framebuffer,
                    vk::Extent2D {
                        width: texture.desc.size.width(),
                        height: texture.desc.size.height(),
                    },
                )
            }
        };
        let render_pass = self.render_passes.get(pass.render_pass)?.render_pass;
        let draw_steps = steps
            .iter()
            .map(|step| {
                let pipeline_record = self.render_pipelines.get(step.pipeline())?;
                let descriptor_sets = step
                    .resource_sets()
                    .iter()
                    .copied()
                    .map(|resource_set| Ok(self.resource_sets.get(resource_set)?.descriptor_set))
                    .collect::<Result<Vec<_>>>()?;
                let draw = match step {
                    RenderStepRef::Draw(step) => CommandDrawStepKind::NonIndexed {
                        vertex_count: step.vertex_count,
                        first_vertex: step.first_vertex,
                        instance_count: step.instance_count,
                        first_instance: step.first_instance,
                    },
                    RenderStepRef::DrawIndexed(step) => {
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
                };
                Ok(CommandDrawStepInfo {
                    pipeline: pipeline_record.pipeline,
                    pipeline_layout: pipeline_record.pipeline_layout,
                    descriptor_sets,
                    draw,
                    scissor: step.scissor(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let result = record_command_buffer(&CommandRecordInfo {
            device: &self.device,
            command_buffer,
            render_pass,
            framebuffer,
            steps: &draw_steps,
            extent,
            color_load_op: pass.color_load_op,
            render_target_transition,
        });
        if let Some(texture_id) = render_target_texture {
            self.textures.get_mut(texture_id)?.layout = vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL;
        }
        if let Some(framebuffer) = transient_framebuffer {
            self.command_encoders
                .get_mut(encoder_id)?
                .transient_framebuffers
                .push(framebuffer);
        }
        result
    }

    /// Submits a command buffer.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when submission fails.
    fn submit(&mut self, encoder_id: CommandEncoderId) -> Result<()> {
        let command_buffer = self.command_encoders.get(encoder_id)?.command_buffer;
        self.submit_command_buffer(command_buffer, &[], &[], vk::Fence::null())?;
        self.submitted_frames = self.submitted_frames.saturating_add(1);
        self.poll_cleanup();
        Ok(())
    }

    /// Acquires, submits, and presents a swapchain image with multiple draw steps.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when acquire, record, submit, or present fails.
    fn draw_steps_and_present(
        &mut self,
        swapchain_id: gfx_core::SwapchainId,
        render_pass_id: RenderPassId,
        steps: &[DrawStepDesc],
        clear_color: ClearColor,
    ) -> Result<()> {
        self.render_step_list_and_present(
            swapchain_id,
            render_pass_id,
            RenderStepList::from_draw_steps(steps),
            clear_color,
        )
    }

    fn render_steps_and_present(
        &mut self,
        swapchain_id: gfx_core::SwapchainId,
        render_pass_id: RenderPassId,
        steps: &[RenderStepDescriptor],
        clear_color: ClearColor,
    ) -> Result<()> {
        self.render_step_list_and_present(
            swapchain_id,
            render_pass_id,
            RenderStepList::from_render_steps(steps),
            clear_color,
        )
    }

    fn render_step_list_and_present(
        &mut self,
        swapchain_id: gfx_core::SwapchainId,
        render_pass_id: RenderPassId,
        steps: RenderStepList<'_>,
        clear_color: ClearColor,
    ) -> Result<()> {
        let (image_index, frame_index, image_available, render_finished, fence) = {
            let swapchain = self.swapchains.get_mut(swapchain_id)?;
            let fence = swapchain.in_flight_fences[swapchain.frame_index];
            // SAFETY: Fence belongs to this device and is not destroyed until swapchain destroy.
            unsafe { self.device.wait_for_fences(&[fence], true, u64::MAX) }
                .map_err(VulkanError::from)?;
            // SAFETY: Swapchain and semaphore belong to this device and are valid here.
            let acquire_result = unsafe {
                self.swapchain_loader.acquire_next_image(
                    swapchain.swapchain,
                    u64::MAX,
                    swapchain.image_available_semaphores[swapchain.frame_index],
                    vk::Fence::null(),
                )
            };
            let image_index = match acquire_result {
                Ok((image_index, suboptimal)) => {
                    if suboptimal {
                        return Err(VulkanError::Vk(vk::Result::SUBOPTIMAL_KHR).into());
                    }
                    image_index
                }
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => return Ok(()),
                Err(error) => return Err(VulkanError::from(error).into()),
            };
            // SAFETY: Fence belongs to this device and was waited above.
            unsafe { self.device.reset_fences(&[fence]) }.map_err(VulkanError::from)?;
            (
                image_index,
                swapchain.frame_index,
                swapchain.image_available_semaphores[swapchain.frame_index],
                swapchain.render_finished_semaphores[swapchain.frame_index],
                fence,
            )
        };

        let encoder = self.create_command_encoder(&CommandEncoderDesc { label: None })?;
        self.record_render_step_list_desc(
            encoder,
            BeginRenderPassDesc {
                render_pass: render_pass_id,
                target: RenderTarget::Swapchain {
                    swapchain: swapchain_id,
                    image_index,
                },
                color_load_op: LoadOp::Clear(clear_color),
            },
            steps,
        )?;
        let command_buffer = self.command_encoders.get(encoder)?.command_buffer;
        self.submit_command_buffer(
            command_buffer,
            &[image_available],
            &[render_finished],
            fence,
        )?;
        let present_result = self.present(swapchain_id, image_index, render_finished);
        // SAFETY: Fence was passed to queue_submit above and is valid until swapchain destruction.
        let wait_result = unsafe { self.device.wait_for_fences(&[fence], true, u64::MAX) }
            .map_err(VulkanError::from);
        let encoder_resource = self.command_encoders.take(encoder)?;
        self.destroy_command_encoder_now(&encoder_resource);
        present_result?;
        wait_result?;
        let swapchain = self.swapchains.get_mut(swapchain_id)?;
        swapchain.frame_index = (frame_index + 1) % FRAMES_IN_FLIGHT;
        self.submitted_frames = self.submitted_frames.saturating_add(1);
        self.poll_cleanup();
        Ok(())
    }

    /// Records and submits draw steps into a regular texture view.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when command recording, submission, or handle lookup fails.
    fn draw_steps_to_texture(
        &mut self,
        texture_view: TextureViewId,
        render_pass_id: RenderPassId,
        steps: &[DrawStepDesc],
        color_load_op: LoadOp<ClearColor>,
    ) -> Result<()> {
        self.render_step_list_to_texture(
            texture_view,
            render_pass_id,
            RenderStepList::from_draw_steps(steps),
            color_load_op,
        )
    }

    fn render_steps_to_texture(
        &mut self,
        texture_view: TextureViewId,
        render_pass_id: RenderPassId,
        steps: &[RenderStepDescriptor],
        color_load_op: LoadOp<ClearColor>,
    ) -> Result<()> {
        self.render_step_list_to_texture(
            texture_view,
            render_pass_id,
            RenderStepList::from_render_steps(steps),
            color_load_op,
        )
    }

    fn render_step_list_to_texture(
        &mut self,
        texture_view: TextureViewId,
        render_pass_id: RenderPassId,
        steps: RenderStepList<'_>,
        color_load_op: LoadOp<ClearColor>,
    ) -> Result<()> {
        let encoder = self.create_command_encoder(&CommandEncoderDesc { label: None })?;
        self.record_render_step_list_desc(
            encoder,
            BeginRenderPassDesc {
                render_pass: render_pass_id,
                target: RenderTarget::TextureView(texture_view),
                color_load_op,
            },
            steps,
        )?;
        self.submit_command_encoder_deferred(encoder)?;
        self.poll_cleanup();
        Ok(())
    }

    fn submit_command_encoder_deferred(&mut self, encoder: CommandEncoderId) -> Result<()> {
        self.submit_command_encoder_deferred_tracked(encoder)
            .map(|_| ())
    }

    fn submit_command_encoder_deferred_tracked(
        &mut self,
        encoder: CommandEncoderId,
    ) -> Result<SubmissionId> {
        let mut encoder_resource = self.command_encoders.take(encoder)?;
        let command_buffer = encoder_resource.command_buffer;
        let fence = create_fence(&self.device, false)?;
        self.submit_command_buffer(command_buffer, &[], &[], fence)?;
        encoder_resource.fence = fence;
        self.deferred_destroys
            .retire(0, DeferredResource::CommandEncoder(encoder_resource));
        self.submitted_frames = self.submitted_frames.saturating_add(1);
        Ok(self.submissions.insert(VulkanSubmission { fence }))
    }

    fn submit_deferred(&mut self, encoder: CommandEncoderId) -> Result<SubmissionId> {
        self.submit_command_encoder_deferred_tracked(encoder)
    }

    fn poll_submission(&mut self, submission: SubmissionId) -> Result<SubmissionStatus> {
        self.poll_cleanup();
        let submission_resource = self.submissions.get(submission)?;
        if fence_is_complete(&self.device, submission_resource.fence) {
            let _completed = self.submissions.take(submission)?;
            Ok(SubmissionStatus::Complete)
        } else {
            Ok(SubmissionStatus::Pending)
        }
    }

    fn wait_submission(&mut self, submission: SubmissionId) -> Result<()> {
        let fence = self.submissions.get(submission)?.fence;
        // SAFETY: Fence belongs to this device and is retained by the deferred command encoder.
        unsafe { self.device.wait_for_fences(&[fence], true, u64::MAX) }
            .map_err(VulkanError::from)?;
        let _completed = self.submissions.take(submission)?;
        self.poll_cleanup();
        Ok(())
    }

    /// Presents a swapchain image.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when presentation fails.
    fn present(
        &mut self,
        swapchain_id: gfx_core::SwapchainId,
        image_index: u32,
        wait_semaphore: vk::Semaphore,
    ) -> Result<()> {
        let swapchain = self.swapchains.get(swapchain_id)?.swapchain;
        let wait_semaphores = [wait_semaphore];
        let swapchains = [swapchain];
        let image_indices = [image_index];
        let present_info = vk::PresentInfoKHR::default()
            .wait_semaphores(&wait_semaphores)
            .swapchains(&swapchains)
            .image_indices(&image_indices);
        // SAFETY: Present queue and swapchain are valid and synchronized by wait_semaphore.
        let present_result = unsafe {
            self.swapchain_loader
                .queue_present(self.present_queue, &present_info)
        };
        match present_result {
            Ok(_) | Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => Ok(()),
            Err(error) => Err(VulkanError::from(error).into()),
        }
    }

    /// Destroys a buffer.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when the handle is invalid or memory free fails.
    fn destroy_buffer(&mut self, buffer_id: BufferId) -> Result<()> {
        let buffer = self.buffers.take(buffer_id)?;
        let fence = self.signal_cleanup_fence()?;
        self.deferred_destroys
            .retire(0, DeferredResource::Buffer { fence, buffer });
        self.poll_cleanup();
        Ok(())
    }

    /// Destroys a texture.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when the handle is invalid.
    fn destroy_texture(&mut self, texture_id: TextureId) -> Result<()> {
        let texture = self.textures.take(texture_id)?;
        let fence = self.signal_cleanup_fence()?;
        self.deferred_destroys
            .retire(0, DeferredResource::Texture { fence, texture });
        self.poll_cleanup();
        Ok(())
    }

    /// Destroys a texture view.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when the handle is invalid.
    fn destroy_texture_view(&mut self, view_id: TextureViewId) -> Result<()> {
        self.wait_for_pending_work()?;
        let view = self.texture_views.take(view_id)?;
        self.destroy_texture_view_now(&view);
        Ok(())
    }

    /// Destroys a sampler.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when the handle is invalid.
    fn destroy_sampler(&mut self, sampler_id: SamplerId) -> Result<()> {
        self.wait_for_pending_work()?;
        let sampler = self.samplers.take(sampler_id)?;
        self.destroy_sampler_now(sampler);
        Ok(())
    }

    /// Destroys a resource set layout.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when the handle is invalid.
    fn destroy_resource_set_layout(&mut self, layout_id: ResourceSetLayoutId) -> Result<()> {
        self.wait_for_pending_work()?;
        let layout = self.resource_set_layouts.take(layout_id)?;
        self.destroy_resource_set_layout_now(&layout);
        Ok(())
    }

    /// Destroys a resource set.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when the handle is invalid.
    fn destroy_resource_set(&mut self, set_id: ResourceSetId) -> Result<()> {
        self.wait_for_pending_work()?;
        let set = self.resource_sets.take(set_id)?;
        self.destroy_resource_set_now(&set);
        Ok(())
    }

    /// Destroys a pipeline layout.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when the handle is invalid.
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
    /// Returns [`GfxError`] when the handle is invalid.
    fn destroy_shader_module(&mut self, shader_id: ShaderModuleId) -> Result<()> {
        self.wait_for_pending_work()?;
        let shader = self.shader_modules.take(shader_id)?;
        self.destroy_shader_module_now(&shader);
        Ok(())
    }

    /// Destroys a render pass.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when the handle is invalid.
    fn destroy_render_pass(&mut self, render_pass_id: RenderPassId) -> Result<()> {
        self.wait_for_pending_work()?;
        let render_pass = self.render_passes.take(render_pass_id)?;
        self.destroy_render_pass_now(&render_pass);
        Ok(())
    }

    /// Destroys a render pipeline.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when the handle is invalid.
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
    /// Returns [`GfxError`] when the handle is invalid.
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
    /// Returns [`GfxError`] when the handle is invalid.
    fn destroy_swapchain(&mut self, swapchain_id: gfx_core::SwapchainId) -> Result<()> {
        // SAFETY: Device is valid and waiting before swapchain resource destruction is valid.
        unsafe { self.device.device_wait_idle() }.map_err(VulkanError::from)?;
        let swapchain = self.swapchains.take(swapchain_id)?;
        self.destroy_swapchain_now(swapchain);
        Ok(())
    }

    /// Destroys a surface.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when the handle is invalid.
    fn destroy_surface(&mut self, surface_id: SurfaceId) -> Result<()> {
        let surface = self.surfaces.take(surface_id)?;
        self.destroy_surface_now(surface);
        Ok(())
    }

    /// Polls and releases deferred resources that have aged past their retirement frame.
    pub fn poll_cleanup(&mut self) {
        let device = self.device.clone();
        for resource in self
            .deferred_destroys
            .collect_ready(|_, resource| deferred_resource_ready(&device, resource))
        {
            let _destroy_result = self.destroy_deferred_now(resource);
        }
    }

    /// Returns live resource and allocator statistics.
    #[must_use]
    fn resource_stats(&self) -> ResourceStats {
        let memory = self.allocator.stats();
        ResourceStats {
            buffers: self.buffers.live_len(),
            textures: self.textures.live_len(),
            texture_views: self.texture_views.live_len(),
            samplers: self.samplers.live_len(),
            resource_set_layouts: self.resource_set_layouts.live_len(),
            resource_sets: self.resource_sets.live_len(),
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
        let support = query_swapchain_support(self.physical_device, &self.surface_loader, surface)?;
        let surface_format = choose_surface_format(&support.formats, config.format);
        let present_mode = choose_present_mode(&support.present_modes, config.present_mode);
        let extent = choose_extent(
            &support.capabilities,
            config.size.width(),
            config.size.height(),
        );
        let mut image_count = support.capabilities.min_image_count.saturating_add(1);
        if support.capabilities.max_image_count > 0 {
            image_count = image_count.min(support.capabilities.max_image_count);
        }

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
        let alpha_mode = choose_composite_alpha(
            support.capabilities.supported_composite_alpha,
            config.alpha_mode,
        );
        let create_info = vk::SwapchainCreateInfoKHR::default()
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
            .composite_alpha(alpha_mode)
            .present_mode(present_mode)
            .clipped(true)
            .old_swapchain(old_swapchain);

        // SAFETY: All inputs are valid for the selected physical device and surface.
        let swapchain = unsafe { self.swapchain_loader.create_swapchain(&create_info, None) }
            .map_err(VulkanError::from)?;
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
        let render_pass = create_render_pass(&self.device, surface_format.format)?;
        let framebuffers = image_views
            .iter()
            .copied()
            .map(|image_view| create_framebuffer(&self.device, render_pass, image_view, extent))
            .collect::<Result<Vec<_>>>()?;
        let (image_available_semaphores, render_finished_semaphores, in_flight_fences) =
            create_sync_objects(&self.device)?;
        Ok(VulkanSwapchain {
            surface,
            swapchain,
            format: surface_format.format,
            extent,
            images,
            image_views,
            internal_render_pass: render_pass,
            framebuffers,
            image_available_semaphores,
            render_finished_semaphores,
            in_flight_fences,
            frame_index: 0,
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
        let wait_stages =
            vec![vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT; wait_semaphores.len()];
        let command_buffers = [command_buffer];
        let submit_info = vk::SubmitInfo::default()
            .wait_semaphores(wait_semaphores)
            .wait_dst_stage_mask(&wait_stages)
            .command_buffers(&command_buffers)
            .signal_semaphores(signal_semaphores);
        // SAFETY: Queue, command buffer, semaphores, and fence are owned by this device.
        unsafe {
            self.device
                .queue_submit(self.graphics_queue, &[submit_info], fence)
        }
        .map_err(VulkanError::from)?;
        Ok(())
    }

    fn complete_synchronous_upload(&mut self) {
        self.upload_ring.retire_used_pages(self.submitted_frames);
        self.upload_ring.complete_fence(self.submitted_frames);
        self.upload_ring.trim_idle_pages();
    }

    fn signal_cleanup_fence(&self) -> Result<vk::Fence> {
        let fence = create_fence(&self.device, false)?;
        let submit_info = vk::SubmitInfo::default();
        // SAFETY: Queue and fence are owned by this device. An empty submit is valid and
        // orders the fence after earlier submissions on the same queue.
        unsafe {
            self.device
                .queue_submit(self.graphics_queue, &[submit_info], fence)
        }
        .map_err(VulkanError::from)?;
        Ok(fence)
    }

    fn write_upload_data(&mut self, data: &[u8]) -> Result<UploadAllocation> {
        let size = u64::try_from(data.len())
            .map_err(|error| GfxError::InvalidInput(format!("upload size overflow: {error}")))?;
        let allocation = self.upload_ring.allocate(size)?;
        self.ensure_upload_page(allocation.page_index)?;
        let page = self
            .upload_pages
            .get_mut(allocation.page_index)
            .and_then(Option::as_mut)
            .ok_or_else(|| GfxError::Backend("missing Vulkan upload page".to_string()))?;
        let mapped = page.allocation.mapped_slice_mut().ok_or_else(|| {
            GfxError::Backend("upload page memory is not CPU visible".to_string())
        })?;
        let offset = usize::try_from(allocation.offset)
            .map_err(|error| GfxError::InvalidInput(format!("upload offset overflow: {error}")))?;
        let end = offset
            .checked_add(data.len())
            .ok_or_else(|| GfxError::InvalidInput("upload range overflow".to_string()))?;
        let target = mapped.get_mut(offset..end).ok_or_else(|| {
            GfxError::InvalidInput("upload range is out of upload page bounds".to_string())
        })?;
        target.copy_from_slice(data);
        Ok(allocation)
    }

    fn ensure_upload_page(&mut self, page_index: usize) -> Result<()> {
        if self
            .upload_pages
            .get(page_index)
            .is_some_and(Option::is_some)
        {
            return Ok(());
        }
        let size = self.upload_ring.page_size(page_index).ok_or_else(|| {
            GfxError::Backend(format!("upload ring page {page_index} has no size"))
        })?;
        while self.upload_pages.len() <= page_index {
            self.upload_pages.push(None);
        }
        let desc = BufferDesc {
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
            .ok_or_else(|| GfxError::Backend(format!("missing Vulkan upload page {page_index}")))
    }

    fn create_buffer_unregistered(&mut self, desc: &BufferDesc) -> Result<VulkanBuffer> {
        desc.validate()?;
        let create_info = vk::BufferCreateInfo::default()
            .size(desc.size)
            .usage(buffer_usage_to_vk(desc.usage))
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
            .ok_or_else(|| GfxError::Backend("failed to allocate command buffer".to_string()))?;
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
        )
    }

    fn copy_buffer_to_texture_once(
        &self,
        source: vk::Buffer,
        image: vk::Image,
        old_layout: vk::ImageLayout,
        layout: TextureDataLayout,
        origin: gfx_core::Origin2d,
        size: gfx_core::Extent2d,
    ) -> Result<()> {
        let command_pool = create_command_pool(&self.device, self.graphics_queue_family_index)?;
        let command_buffer = allocate_command_buffers(&self.device, command_pool, 1)?
            .into_iter()
            .next()
            .ok_or_else(|| GfxError::Backend("failed to allocate command buffer".to_string()))?;
        begin_one_time_commands(&self.device, command_buffer)?;
        transition_image_layout(
            &self.device,
            command_buffer,
            image,
            old_layout,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        );
        let region = vk::BufferImageCopy::default()
            .buffer_offset(layout.offset)
            .buffer_row_length(layout.bytes_per_row.get() / 4)
            .buffer_image_height(layout.rows_per_image.get())
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .mip_level(0)
                    .base_array_layer(0)
                    .layer_count(1),
            )
            .image_offset(vk::Offset3D {
                x: i32::try_from(origin.x).map_err(|error| {
                    GfxError::InvalidInput(format!("texture upload origin x overflow: {error}"))
                })?,
                y: i32::try_from(origin.y).map_err(|error| {
                    GfxError::InvalidInput(format!("texture upload origin y overflow: {error}"))
                })?,
                z: 0,
            })
            .image_extent(vk::Extent3D {
                width: size.width(),
                height: size.height(),
                depth: 1,
            });
        // SAFETY: Command buffer is recording, image is in transfer dst layout, and source
        // buffer is valid for the provided copy region.
        unsafe {
            self.device.cmd_copy_buffer_to_image(
                command_buffer,
                source,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[region],
            );
        }
        transition_image_layout(
            &self.device,
            command_buffer,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        );
        end_submit_wait_destroy(
            &self.device,
            self.graphics_queue,
            command_pool,
            command_buffer,
        )
    }

    fn destroy_deferred_now(&mut self, resource: DeferredResource) -> Result<()> {
        match resource {
            DeferredResource::Buffer { fence, buffer } => {
                destroy_fence_if_needed(&self.device, fence);
                self.destroy_buffer_now(buffer)
            }
            DeferredResource::Texture { fence, texture } => {
                destroy_fence_if_needed(&self.device, fence);
                self.destroy_texture_now(texture)
            }
            DeferredResource::CommandEncoder(encoder) => {
                self.destroy_command_encoder_now(&encoder);
                Ok(())
            }
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

    fn destroy_resource_set_now(&self, set: &VulkanResourceSet) {
        let _ = set.layout;
        // SAFETY: Descriptor set was allocated from descriptor_pool and may be freed once.
        let _ = unsafe {
            self.device
                .free_descriptor_sets(self.descriptor_pool, &[set.descriptor_set])
        };
    }

    fn destroy_pipeline_layout_now(&self, layout: &VulkanPipelineLayout) {
        let _ = layout.resource_set_layouts.len();
        // SAFETY: Pipeline layout was created by this device and is destroyed once here.
        unsafe {
            self.device.destroy_pipeline_layout(layout.layout, None);
        }
    }

    fn destroy_shader_module_now(&self, shader: &VulkanShaderModule) {
        // SAFETY: Shader module was created by this device and is destroyed once here.
        unsafe { self.device.destroy_shader_module(shader.module, None) };
    }

    fn destroy_render_pass_now(&self, render_pass: &VulkanRenderPass) {
        let _ = render_pass.color_format;
        // SAFETY: Render pass was created by this device and is destroyed once here.
        unsafe {
            self.device
                .destroy_render_pass(render_pass.render_pass, None);
        };
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

    fn destroy_command_encoder_now(&self, encoder: &VulkanCommandEncoder) {
        for framebuffer in &encoder.transient_framebuffers {
            // SAFETY: Framebuffer was created for this command encoder and is destroyed once here.
            unsafe { self.device.destroy_framebuffer(*framebuffer, None) };
        }
        if encoder.fence != vk::Fence::null() {
            // SAFETY: Fence was created for this command encoder and is destroyed once here.
            unsafe { self.device.destroy_fence(encoder.fence, None) };
        }
        // SAFETY: Command pool was created by this device and owns the command buffer.
        unsafe { self.device.destroy_command_pool(encoder.command_pool, None) };
    }

    fn destroy_swapchain_now(&self, swapchain: VulkanSwapchain) {
        let _ = (swapchain.format, swapchain.images.len());
        // SAFETY: Swapchain resources were created by this device and are destroyed once here.
        unsafe {
            for framebuffer in swapchain.framebuffers {
                self.device.destroy_framebuffer(framebuffer, None);
            }
            self.device
                .destroy_render_pass(swapchain.internal_render_pass, None);
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

impl GfxBackend for VulkanDevice {
    const BACKEND_KIND: BackendKind = BackendKind::Vulkan;
}

impl GfxSurfaceDevice for VulkanDevice {
    type SurfaceTarget = dyn VulkanSurfaceTarget;

    fn create_surface(
        &mut self,
        target: &Self::SurfaceTarget,
        desc: &SurfaceDesc,
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

    fn destroy_swapchain(&mut self, swapchain: gfx_core::SwapchainId) -> Result<()> {
        Self::destroy_swapchain(self, swapchain)
    }

    fn destroy_surface(&mut self, surface: SurfaceId) -> Result<()> {
        Self::destroy_surface(self, surface)
    }
}

impl GfxResourceDevice for VulkanDevice {
    fn create_buffer(&mut self, desc: &BufferDesc) -> Result<BufferId> {
        Self::create_buffer(self, desc)
    }

    fn write_buffer(&mut self, buffer: BufferId, offset: u64, data: &[u8]) -> Result<()> {
        Self::write_buffer(self, buffer, offset, data)
    }

    fn create_texture(&mut self, desc: &TextureDesc) -> Result<TextureId> {
        Self::create_texture(self, desc)
    }

    fn write_texture(&mut self, desc: TextureWriteDesc, data: &[u8]) -> Result<()> {
        Self::write_texture(self, desc, data)
    }

    fn create_texture_view(&mut self, desc: &TextureViewDesc) -> Result<TextureViewId> {
        Self::create_texture_view(self, desc)
    }

    fn create_sampler(&mut self, desc: &SamplerDesc) -> Result<SamplerId> {
        Self::create_sampler(self, desc)
    }

    fn create_resource_set_layout(
        &mut self,
        desc: &ResourceSetLayoutDesc,
    ) -> Result<ResourceSetLayoutId> {
        Self::create_resource_set_layout(self, desc)
    }

    fn create_resource_set(&mut self, desc: &ResourceSetDesc) -> Result<ResourceSetId> {
        Self::create_resource_set(self, desc)
    }

    fn destroy_buffer(&mut self, buffer: BufferId) -> Result<()> {
        Self::destroy_buffer(self, buffer)
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

    fn destroy_resource_set_layout(&mut self, layout: ResourceSetLayoutId) -> Result<()> {
        Self::destroy_resource_set_layout(self, layout)
    }

    fn destroy_resource_set(&mut self, resource_set: ResourceSetId) -> Result<()> {
        Self::destroy_resource_set(self, resource_set)
    }
}

impl GfxPipelineDevice for VulkanDevice {
    fn create_pipeline_layout(&mut self, desc: &PipelineLayoutDesc) -> Result<PipelineLayoutId> {
        Self::create_pipeline_layout(self, desc)
    }

    fn create_shader_module(&mut self, desc: &ShaderModuleDesc) -> Result<ShaderModuleId> {
        Self::create_shader_module(self, desc)
    }

    fn create_render_pass(&mut self, desc: &RenderPassDesc) -> Result<RenderPassId> {
        Self::create_render_pass(self, desc)
    }

    fn create_render_pipeline(
        &mut self,
        desc: &RenderPipelineDesc,
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

impl GfxCommandDevice for VulkanDevice {
    fn create_command_encoder(&mut self, desc: &CommandEncoderDesc) -> Result<CommandEncoderId> {
        Self::create_command_encoder(self, desc)
    }

    fn record_draw_desc(&mut self, encoder: CommandEncoderId, draw: DrawDesc) -> Result<()> {
        Self::record_draw_desc(self, encoder, &draw)
    }

    fn submit(&mut self, encoder: CommandEncoderId) -> Result<()> {
        Self::submit(self, encoder)
    }

    fn destroy_command_encoder(&mut self, encoder: CommandEncoderId) -> Result<()> {
        Self::destroy_command_encoder(self, encoder)
    }
}

impl GfxSubmissionDevice for VulkanDevice {
    fn async_capabilities(&self) -> gfx_core::GfxAsyncCapabilities {
        gfx_core::GfxAsyncCapabilities {
            threading_mode: GfxThreadingMode::MultiThreadDeviceProxy,
            async_submission: true,
            async_wait: true,
            async_presentation: false,
            partial_presentation: false,
        }
    }

    fn submit_deferred(&mut self, encoder: CommandEncoderId) -> Result<SubmissionId> {
        Self::submit_deferred(self, encoder)
    }

    fn poll_submission(&mut self, submission: SubmissionId) -> Result<SubmissionStatus> {
        Self::poll_submission(self, submission)
    }

    fn wait_submission(&mut self, submission: SubmissionId) -> Result<()> {
        Self::wait_submission(self, submission)
    }
}

impl GfxPresentationDevice for VulkanDevice {
    fn draw_steps_and_present(
        &mut self,
        swapchain: gfx_core::SwapchainId,
        render_pass: RenderPassId,
        steps: &[DrawStepDesc],
        clear_color: ClearColor,
    ) -> Result<()> {
        Self::draw_steps_and_present(self, swapchain, render_pass, steps, clear_color)
    }

    fn draw_steps_to_texture(
        &mut self,
        texture_view: TextureViewId,
        render_pass: RenderPassId,
        steps: &[DrawStepDesc],
        color_load_op: LoadOp<ClearColor>,
    ) -> Result<()> {
        Self::draw_steps_to_texture(self, texture_view, render_pass, steps, color_load_op)
    }

    fn render_steps_and_present_compat(
        &mut self,
        swapchain: gfx_core::SwapchainId,
        render_pass: RenderPassId,
        steps: &[RenderStepDescriptor],
        clear_color: ClearColor,
        _depth_attachment: Option<RenderPassDepthAttachment>,
    ) -> Result<()> {
        Self::render_steps_and_present(self, swapchain, render_pass, steps, clear_color)
    }

    fn render_steps_to_texture_compat(
        &mut self,
        texture_view: TextureViewId,
        render_pass: RenderPassId,
        steps: &[RenderStepDescriptor],
        color_load_op: LoadOp<ClearColor>,
        _depth_attachment: Option<RenderPassDepthAttachment>,
    ) -> Result<()> {
        Self::render_steps_to_texture(self, texture_view, render_pass, steps, color_load_op)
    }

    fn render_steps_and_present_deferred_compat(
        &mut self,
        swapchain: gfx_core::SwapchainId,
        render_pass: RenderPassId,
        steps: &[RenderStepDescriptor],
        clear_color: ClearColor,
        _depth_attachment: Option<RenderPassDepthAttachment>,
    ) -> Result<SubmissionId>
    where
        Self: GfxSubmissionDevice,
    {
        Self::render_steps_and_present(self, swapchain, render_pass, steps, clear_color)?;
        Ok(SubmissionId::from_parts(0, 0))
    }
}

impl GfxDiagnosticsDevice for VulkanDevice {
    fn resource_stats(&self) -> ResourceStats {
        Self::resource_stats(self)
    }
}

impl Drop for VulkanDevice {
    fn drop(&mut self) {
        // SAFETY: Device may still have in-flight work; waiting before destruction is valid.
        let _ = unsafe { self.device.device_wait_idle() };
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
        for (_, texture) in self.textures.drain_live() {
            let _ = self.destroy_texture_now(texture);
        }
        for (_, view) in self.texture_views.drain_live() {
            self.destroy_texture_view_now(&view);
        }
        for (_, sampler) in self.samplers.drain_live() {
            self.destroy_sampler_now(sampler);
        }
        for (_, set) in self.resource_sets.drain_live() {
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
        for (_, render_pass) in self.render_passes.drain_live() {
            self.destroy_render_pass_now(&render_pass);
        }
        for (_, layout) in self.resource_set_layouts.drain_live() {
            self.destroy_resource_set_layout_now(&layout);
        }
        for (_, encoder) in self.command_encoders.drain_live() {
            self.destroy_command_encoder_now(&encoder);
        }
        for (_, swapchain) in self.swapchains.drain_live() {
            self.destroy_swapchain_now(swapchain);
        }
        for (_, surface) in self.surfaces.drain_live() {
            self.destroy_surface_now(surface);
        }
        // SAFETY: Descriptor pool belongs to this device and no descriptor sets remain live.
        unsafe {
            self.device
                .destroy_descriptor_pool(self.descriptor_pool, None);
        }
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
    /// Returns [`GfxError`] if Vulkan initialization or resource creation fails.
    pub fn new<W>(window: &W, config: &VulkanTriangleConfig) -> Result<Self>
    where
        W: HasDisplayHandle + HasWindowHandle,
    {
        if config.vertex_shader.stage != ShaderStage::Vertex {
            return Err(GfxError::InvalidInput(
                "vertex_shader must use ShaderStage::Vertex".to_string(),
            ));
        }
        if config.fragment_shader.stage != ShaderStage::Fragment {
            return Err(GfxError::InvalidInput(
                "fragment_shader must use ShaderStage::Fragment".to_string(),
            ));
        }

        let metrics_started_at = Instant::now();
        let mut device = VulkanDevice::new(&DeviceDesc {
            application_name: config.application_name.clone(),
            ..DeviceDesc::default()
        })?;
        let surface = device.create_surface(window, &SurfaceDesc { label: None })?;
        let swapchain = device.create_swapchain(surface, config.surface_config)?;
        let vertex_shader = device.create_shader_module(&ShaderModuleDesc {
            label: Some("triangle vertex shader".to_string()),
            binary: config.vertex_shader.clone(),
        })?;
        let fragment_shader = device.create_shader_module(&ShaderModuleDesc {
            label: Some("triangle fragment shader".to_string()),
            binary: config.fragment_shader.clone(),
        })?;
        let render_pass = device.create_render_pass(&RenderPassDesc {
            label: Some("triangle render pass".to_string()),
            color_attachment: ColorAttachmentDesc {
                format: config.surface_config.format,
            },
            depth_attachment: None,
        })?;
        let pipeline = device.create_render_pipeline(
            &RenderPipelineDesc {
                label: Some("triangle pipeline".to_string()),
                vertex_shader,
                vertex_entry_point: config.vertex_shader.entry_point.clone(),
                fragment_shader,
                fragment_entry_point: config.fragment_shader.entry_point.clone(),
                vertex_buffers: Vec::new(),
                render_pass,
                pipeline_layout: None,
                color_format: config.surface_config.format,
                blend_mode: BlendMode::Replace,
                primitive_topology: PrimitiveTopology::TriangleList,
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
    /// Returns [`GfxError`] if surface recreation fails.
    pub fn resize(&mut self, width: u32, height: u32) -> Result<()> {
        self.device.resize_swapchain(self.swapchain, width, height)
    }

    /// Draws and presents one triangle frame.
    ///
    /// # Errors
    ///
    /// Returns [`GfxError`] when acquire, submit, or present fails.
    pub fn draw_triangle(&mut self, draw: DrawTriangleDesc) -> Result<()> {
        self.device.draw_and_present(
            self.swapchain,
            self.render_pass,
            self.pipeline,
            draw.clear_color,
        )?;
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
}

struct SwapchainSupport {
    capabilities: vk::SurfaceCapabilitiesKHR,
    formats: Vec<vk::SurfaceFormatKHR>,
    present_modes: Vec<vk::PresentModeKHR>,
}

struct VulkanBuffer {
    buffer: vk::Buffer,
    allocation: MemoryAllocation,
    desc: BufferDesc,
}

struct VulkanTexture {
    image: vk::Image,
    allocation: MemoryAllocation,
    desc: TextureDesc,
    layout: vk::ImageLayout,
}

#[derive(Clone, Copy)]
struct VulkanTextureView {
    view: vk::ImageView,
    texture: TextureId,
}

#[derive(Clone, Copy)]
struct VulkanSampler {
    sampler: vk::Sampler,
}

#[derive(Clone)]
struct VulkanResourceSetLayout {
    layout: vk::DescriptorSetLayout,
    desc: ResourceSetLayoutDesc,
}

#[derive(Clone, Copy)]
struct VulkanResourceSet {
    descriptor_set: vk::DescriptorSet,
    layout: ResourceSetLayoutId,
}

#[derive(Clone)]
struct VulkanPipelineLayout {
    layout: vk::PipelineLayout,
    resource_set_layouts: Vec<ResourceSetLayoutId>,
}

#[derive(Clone)]
struct VulkanShaderModule {
    module: vk::ShaderModule,
    stage: ShaderStage,
    entry_point: String,
}

#[derive(Clone, Copy)]
struct VulkanRenderPass {
    render_pass: vk::RenderPass,
    color_format: Format,
}

#[derive(Clone, Copy)]
struct VulkanRenderPipeline {
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    owns_pipeline_layout: bool,
}

#[derive(Clone)]
struct VulkanCommandEncoder {
    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    transient_framebuffers: Vec<vk::Framebuffer>,
    fence: vk::Fence,
}

#[derive(Clone, Copy)]
struct VulkanSubmission {
    fence: vk::Fence,
}

#[derive(Clone, Copy)]
struct VulkanSurface {
    surface: vk::SurfaceKHR,
}

struct VulkanSwapchain {
    surface: vk::SurfaceKHR,
    swapchain: vk::SwapchainKHR,
    format: vk::Format,
    extent: vk::Extent2D,
    images: Vec<vk::Image>,
    image_views: Vec<vk::ImageView>,
    internal_render_pass: vk::RenderPass,
    framebuffers: Vec<vk::Framebuffer>,
    image_available_semaphores: [vk::Semaphore; FRAMES_IN_FLIGHT],
    render_finished_semaphores: [vk::Semaphore; FRAMES_IN_FLIGHT],
    in_flight_fences: [vk::Fence; FRAMES_IN_FLIGHT],
    frame_index: usize,
    config: SurfaceConfig,
}

enum DeferredResource {
    Buffer {
        fence: vk::Fence,
        buffer: VulkanBuffer,
    },
    Texture {
        fence: vk::Fence,
        texture: VulkanTexture,
    },
    CommandEncoder(VulkanCommandEncoder),
}

fn deferred_resource_ready(device: &ash::Device, resource: &DeferredResource) -> bool {
    let fence = match resource {
        DeferredResource::Buffer { fence, .. }
        | DeferredResource::Texture { fence, .. }
        | DeferredResource::CommandEncoder(VulkanCommandEncoder { fence, .. }) => *fence,
    };
    fence == vk::Fence::null() || fence_is_complete(device, fence)
}

fn fence_is_complete(device: &ash::Device, fence: vk::Fence) -> bool {
    // SAFETY: Fence belongs to this device and remains live until deferred resource destroy.
    unsafe { device.get_fence_status(fence) }.unwrap_or(true)
}

fn destroy_fence_if_needed(device: &ash::Device, fence: vk::Fence) {
    if fence != vk::Fence::null() {
        // SAFETY: Fence was created by this device and is destroyed once here.
        unsafe { device.destroy_fence(fence, None) };
    }
}

fn vulkan_memory(allocation: &MemoryAllocation) -> Result<(vk::DeviceMemory, u64)> {
    // SAFETY: The caller immediately binds this allocation to the resource that produced
    // the memory requirements used for the allocation.
    unsafe { allocation.vulkan_memory() }
        .ok_or_else(|| GfxError::Backend("allocation is not a Vulkan allocation".to_string()))
}

fn load_entry() -> Result<Entry> {
    // SAFETY: Loading the Vulkan loader is safe if the process has a valid Vulkan loader.
    unsafe { Entry::load() }.map_err(|error| VulkanError::Loader(error.to_string()).into())
}

fn create_instance(entry: &Entry, application_name: &str) -> Result<Instance> {
    let app_name = CString::new(application_name)
        .map_err(|error| GfxError::InvalidInput(error.to_string()))?;
    let engine_name =
        CString::new("nova-gfx").map_err(|error| GfxError::InvalidInput(error.to_string()))?;
    let app_info = vk::ApplicationInfo::default()
        .application_name(&app_name)
        .application_version(vk::make_api_version(0, 0, 1, 0))
        .engine_name(&engine_name)
        .engine_version(vk::make_api_version(0, 0, 1, 0))
        .api_version(vk::API_VERSION_1_0);
    let extension_names = instance_extension_names();
    let create_info = vk::InstanceCreateInfo::default()
        .application_info(&app_info)
        .enabled_extension_names(&extension_names);
    // SAFETY: Create info references live CStrings and static extension names.
    unsafe { entry.create_instance(&create_info, None) }
        .map_err(|error| VulkanError::from(error).into())
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

fn pick_physical_device_without_surface(instance: &Instance) -> Result<vk::PhysicalDevice> {
    // SAFETY: Instance is valid.
    let devices = unsafe { instance.enumerate_physical_devices() }.map_err(VulkanError::from)?;
    devices
        .into_iter()
        .find(|device| queue_family_indices_without_surface(instance, *device).is_ok())
        .ok_or_else(|| {
            VulkanError::Unavailable("no suitable Vulkan physical device".to_string()).into()
        })
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
                .contains(vk::QueueFlags::GRAPHICS)
                .then_some(index)
        })
        .ok_or_else(|| VulkanError::Unavailable("no graphics queue family".to_string()))?;
    let graphics = u32::try_from(graphics)
        .map_err(|error| GfxError::Backend(format!("queue family index overflow: {error}")))?;
    Ok(QueueFamilyIndices {
        graphics,
        present: graphics,
    })
}

fn create_device(
    instance: &Instance,
    physical_device: vk::PhysicalDevice,
    indices: QueueFamilyIndices,
) -> Result<(ash::Device, vk::Queue, vk::Queue)> {
    let priorities = [1.0_f32];
    let mut unique_families = vec![indices.graphics];
    if indices.present != indices.graphics {
        unique_families.push(indices.present);
    }
    let queue_infos = unique_families
        .iter()
        .copied()
        .map(|queue_family_index| {
            vk::DeviceQueueCreateInfo::default()
                .queue_family_index(queue_family_index)
                .queue_priorities(&priorities)
        })
        .collect::<Vec<_>>();
    let device_extensions = [khr::swapchain::NAME.as_ptr()];
    let create_info = vk::DeviceCreateInfo::default()
        .queue_create_infos(&queue_infos)
        .enabled_extension_names(&device_extensions);
    // SAFETY: Physical device and queue info were selected from this instance.
    let device = unsafe { instance.create_device(physical_device, &create_info, None) }
        .map_err(VulkanError::from)?;
    // SAFETY: Queue family index and queue index are valid per device creation.
    let graphics_queue = unsafe { device.get_device_queue(indices.graphics, 0) };
    // SAFETY: Queue family index and queue index are valid per device creation.
    let present_queue = unsafe { device.get_device_queue(indices.present, 0) };
    Ok((device, graphics_queue, present_queue))
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

fn choose_composite_alpha(
    supported: vk::CompositeAlphaFlagsKHR,
    preferred: CompositeAlphaMode,
) -> vk::CompositeAlphaFlagsKHR {
    let candidates: &[vk::CompositeAlphaFlagsKHR] = match preferred {
        CompositeAlphaMode::Auto => &[
            vk::CompositeAlphaFlagsKHR::OPAQUE,
            vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED,
            vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED,
            vk::CompositeAlphaFlagsKHR::INHERIT,
        ],
        CompositeAlphaMode::Opaque => &[vk::CompositeAlphaFlagsKHR::OPAQUE],
        CompositeAlphaMode::Premultiplied => &[vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED],
    };
    candidates
        .iter()
        .copied()
        .find(|candidate| supported.contains(*candidate))
        .unwrap_or(vk::CompositeAlphaFlagsKHR::OPAQUE)
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
    let subresource_range = vk::ImageSubresourceRange::default()
        .aspect_mask(aspect_mask)
        .base_mip_level(0)
        .level_count(1)
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

fn create_render_pass(device: &ash::Device, format: vk::Format) -> Result<vk::RenderPass> {
    let color_attachment = vk::AttachmentDescription::default()
        .format(format)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(vk::ImageLayout::PRESENT_SRC_KHR);
    let color_attachment_ref = vk::AttachmentReference::default()
        .attachment(0)
        .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL);
    let color_attachments = [color_attachment_ref];
    let subpass = vk::SubpassDescription::default()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(&color_attachments);
    let dependency = vk::SubpassDependency::default()
        .src_subpass(vk::SUBPASS_EXTERNAL)
        .dst_subpass(0)
        .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
        .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
        .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE);
    let attachments = [color_attachment];
    let subpasses = [subpass];
    let dependencies = [dependency];
    let render_pass_info = vk::RenderPassCreateInfo::default()
        .attachments(&attachments)
        .subpasses(&subpasses)
        .dependencies(&dependencies);
    // SAFETY: Render pass create info is self-contained and valid.
    unsafe { device.create_render_pass(&render_pass_info, None) }
        .map_err(|error| VulkanError::from(error).into())
}

fn create_descriptor_pool(device: &ash::Device) -> Result<vk::DescriptorPool> {
    let pool_sizes = [
        vk::DescriptorPoolSize {
            ty: vk::DescriptorType::UNIFORM_BUFFER,
            descriptor_count: 256,
        },
        vk::DescriptorPoolSize {
            ty: vk::DescriptorType::STORAGE_BUFFER,
            descriptor_count: 256,
        },
        vk::DescriptorPoolSize {
            ty: vk::DescriptorType::SAMPLED_IMAGE,
            descriptor_count: 256,
        },
        vk::DescriptorPoolSize {
            ty: vk::DescriptorType::SAMPLER,
            descriptor_count: 256,
        },
    ];
    let create_info = vk::DescriptorPoolCreateInfo::default()
        .flags(vk::DescriptorPoolCreateFlags::FREE_DESCRIPTOR_SET)
        .max_sets(256)
        .pool_sizes(&pool_sizes);
    // SAFETY: Descriptor pool create info is self-contained and device is valid.
    unsafe { device.create_descriptor_pool(&create_info, None) }
        .map_err(|error| VulkanError::from(error).into())
}

fn create_empty_pipeline_layout(device: &ash::Device) -> Result<vk::PipelineLayout> {
    let pipeline_layout_info = vk::PipelineLayoutCreateInfo::default();
    // SAFETY: Pipeline layout has no descriptors or push constants and is valid.
    unsafe { device.create_pipeline_layout(&pipeline_layout_info, None) }
        .map_err(|error| VulkanError::from(error).into())
}

struct GraphicsPipelineBuild<'a> {
    device: &'a ash::Device,
    render_pass: vk::RenderPass,
    pipeline_layout: vk::PipelineLayout,
    vertex_shader: &'a VulkanShaderModule,
    vertex_entry_point: &'a str,
    fragment_shader: &'a VulkanShaderModule,
    fragment_entry_point: &'a str,
    desc: &'a RenderPipelineDesc,
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
        .map_err(|error| GfxError::InvalidInput(error.to_string()))?;
    let fragment_entry = CString::new(build.fragment_entry_point)
        .map_err(|error| GfxError::InvalidInput(error.to_string()))?;
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
        .cull_mode(vk::CullModeFlags::NONE)
        .front_face(vk::FrontFace::CLOCKWISE)
        .depth_bias_enable(false);
    let multisampling = vk::PipelineMultisampleStateCreateInfo::default()
        .sample_shading_enable(false)
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);
    let mut color_blend_attachment = vk::PipelineColorBlendAttachmentState::default()
        .color_write_mask(
            vk::ColorComponentFlags::R
                | vk::ColorComponentFlags::G
                | vk::ColorComponentFlags::B
                | vk::ColorComponentFlags::A,
        );
    match build.desc.blend_mode {
        BlendMode::Replace => {}
        BlendMode::Alpha => {
            color_blend_attachment = color_blend_attachment
                .blend_enable(true)
                .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
                .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .color_blend_op(vk::BlendOp::ADD)
                .src_alpha_blend_factor(vk::BlendFactor::ONE)
                .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .alpha_blend_op(vk::BlendOp::ADD);
        }
        BlendMode::PremultipliedAlpha => {
            color_blend_attachment = color_blend_attachment
                .blend_enable(true)
                .src_color_blend_factor(vk::BlendFactor::ONE)
                .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .color_blend_op(vk::BlendOp::ADD)
                .src_alpha_blend_factor(vk::BlendFactor::ONE)
                .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .alpha_blend_op(vk::BlendOp::ADD);
        }
        BlendMode::AdditiveAlpha => {
            color_blend_attachment = color_blend_attachment
                .blend_enable(true)
                .src_color_blend_factor(vk::BlendFactor::ONE)
                .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
                .color_blend_op(vk::BlendOp::ADD)
                .src_alpha_blend_factor(vk::BlendFactor::ONE)
                .dst_alpha_blend_factor(vk::BlendFactor::ONE)
                .alpha_blend_op(vk::BlendOp::ADD);
        }
    }
    let color_blend_attachments = [color_blend_attachment];
    let color_blending = vk::PipelineColorBlendStateCreateInfo::default()
        .logic_op_enable(false)
        .attachments(&color_blend_attachments);
    let pipeline_info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&shader_stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_assembly)
        .viewport_state(&viewport_state)
        .rasterization_state(&rasterizer)
        .multisample_state(&multisampling)
        .color_blend_state(&color_blending)
        .dynamic_state(&dynamic_state)
        .layout(build.pipeline_layout)
        .render_pass(build.render_pass)
        .subpass(0);
    // SAFETY: Pipeline create info references valid shader modules and render pass.
    let pipeline = unsafe {
        build
            .device
            .create_graphics_pipelines(vk::PipelineCache::null(), &[pipeline_info], None)
    }
    .map_err(|(_, error)| VulkanError::from(error))?[0];
    Ok((build.pipeline_layout, pipeline))
}

fn create_framebuffer(
    device: &ash::Device,
    render_pass: vk::RenderPass,
    image_view: vk::ImageView,
    extent: vk::Extent2D,
) -> Result<vk::Framebuffer> {
    let attachments = [image_view];
    let framebuffer_info = vk::FramebufferCreateInfo::default()
        .render_pass(render_pass)
        .attachments(&attachments)
        .width(extent.width)
        .height(extent.height)
        .layers(1);
    // SAFETY: Framebuffer create info references a valid render pass and image view.
    unsafe { device.create_framebuffer(&framebuffer_info, None) }
        .map_err(|error| VulkanError::from(error).into())
}

fn create_command_pool(device: &ash::Device, queue_family_index: u32) -> Result<vk::CommandPool> {
    let pool_info = vk::CommandPoolCreateInfo::default()
        .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
        .queue_family_index(queue_family_index);
    // SAFETY: Queue family index was selected from this physical device.
    unsafe { device.create_command_pool(&pool_info, None) }
        .map_err(|error| VulkanError::from(error).into())
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
) -> Result<(
    [vk::Semaphore; FRAMES_IN_FLIGHT],
    [vk::Semaphore; FRAMES_IN_FLIGHT],
    [vk::Fence; FRAMES_IN_FLIGHT],
)> {
    let mut image_available_semaphores = [vk::Semaphore::null(); FRAMES_IN_FLIGHT];
    let mut render_finished_semaphores = [vk::Semaphore::null(); FRAMES_IN_FLIGHT];
    let mut in_flight_fences = [vk::Fence::null(); FRAMES_IN_FLIGHT];
    let semaphore_info = vk::SemaphoreCreateInfo::default();
    for index in 0..FRAMES_IN_FLIGHT {
        // SAFETY: Device is valid and creation info is well-formed.
        image_available_semaphores[index] =
            unsafe { device.create_semaphore(&semaphore_info, None) }.map_err(VulkanError::from)?;
        // SAFETY: Device is valid and creation info is well-formed.
        render_finished_semaphores[index] =
            unsafe { device.create_semaphore(&semaphore_info, None) }.map_err(VulkanError::from)?;
        // SAFETY: Device is valid and creation info is well-formed.
        in_flight_fences[index] = create_fence(device, true)?;
    }
    Ok((
        image_available_semaphores,
        render_finished_semaphores,
        in_flight_fences,
    ))
}

struct CommandRecordInfo<'a> {
    device: &'a ash::Device,
    command_buffer: vk::CommandBuffer,
    render_pass: vk::RenderPass,
    framebuffer: vk::Framebuffer,
    steps: &'a [CommandDrawStepInfo],
    extent: vk::Extent2D,
    color_load_op: LoadOp<ClearColor>,
    render_target_transition: Option<CommandRenderTargetTransition>,
}

#[derive(Clone, Copy)]
struct CommandRenderTargetTransition {
    image: vk::Image,
    old_layout: vk::ImageLayout,
}

struct CommandDrawStepInfo {
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    descriptor_sets: Vec<vk::DescriptorSet>,
    draw: CommandDrawStepKind,
    scissor: Option<gfx_core::ScissorRect>,
}

enum CommandDrawStepKind {
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
        return Err(GfxError::InvalidInput(
            "index buffer must include INDEX usage".to_string(),
        ));
    }
    let stride = index_format_size(binding.format);
    if binding.offset % stride != 0 {
        return Err(GfxError::InvalidInput(
            "index buffer offset must be aligned to the index format size".to_string(),
        ));
    }
    let first_index_byte = u64::from(first_index)
        .checked_mul(stride)
        .ok_or_else(|| GfxError::InvalidInput("first index byte offset overflow".to_string()))?;
    let index_bytes = u64::from(index_count)
        .checked_mul(stride)
        .ok_or_else(|| GfxError::InvalidInput("index buffer range overflow".to_string()))?;
    let byte_end = binding
        .offset
        .checked_add(first_index_byte)
        .and_then(|start| start.checked_add(index_bytes))
        .ok_or_else(|| GfxError::InvalidInput("index buffer range overflow".to_string()))?;
    if byte_end > buffer_size {
        return Err(GfxError::InvalidInput(
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

#[expect(
    clippy::too_many_lines,
    reason = "Vulkan command recording keeps render-pass state transitions adjacent for auditability"
)]
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
    if let Some(transition) = info.render_target_transition {
        transition_image_layout(
            info.device,
            info.command_buffer,
            transition.image,
            transition.old_layout,
            vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
        );
    }

    let clear_color = match info.color_load_op {
        LoadOp::Clear(color) => color,
        LoadOp::Load => ClearColor::default(),
    };
    let clear_values = [vk::ClearValue {
        color: vk::ClearColorValue {
            float32: [
                clear_color.red,
                clear_color.green,
                clear_color.blue,
                clear_color.alpha,
            ],
        },
    }];
    let render_area = info
        .steps
        .iter()
        .filter_map(|step| step.scissor)
        .find(|scissor| !scissor.is_empty())
        .and_then(|scissor| vk_rect_for_scissor(scissor, info.extent).ok())
        .unwrap_or(vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent: info.extent,
        });
    let render_pass_info = vk::RenderPassBeginInfo::default()
        .render_pass(info.render_pass)
        .framebuffer(info.framebuffer)
        .render_area(render_area)
        .clear_values(&clear_values);
    #[expect(
        clippy::cast_precision_loss,
        reason = "Vulkan viewport dimensions are f32 by API contract"
    )]
    let viewport = vk::Viewport {
        x: 0.0,
        y: 0.0,
        width: info.extent.width as f32,
        height: info.extent.height as f32,
        min_depth: 0.0,
        max_depth: 1.0,
    };
    let full_scissor = vk::Rect2D {
        offset: vk::Offset2D { x: 0, y: 0 },
        extent: info.extent,
    };
    // SAFETY: Render pass, framebuffer, pipeline, and command buffer belong to this device.
    unsafe {
        info.device.cmd_begin_render_pass(
            info.command_buffer,
            &render_pass_info,
            vk::SubpassContents::INLINE,
        );
        info.device
            .cmd_set_viewport(info.command_buffer, 0, &[viewport]);
        for step in info.steps {
            let scissor = step
                .scissor
                .and_then(|scissor| vk_rect_for_scissor(scissor, info.extent).ok())
                .unwrap_or(full_scissor);
            info.device
                .cmd_set_scissor(info.command_buffer, 0, &[scissor]);
            info.device.cmd_bind_pipeline(
                info.command_buffer,
                vk::PipelineBindPoint::GRAPHICS,
                step.pipeline,
            );
            if !step.descriptor_sets.is_empty() {
                info.device.cmd_bind_descriptor_sets(
                    info.command_buffer,
                    vk::PipelineBindPoint::GRAPHICS,
                    step.pipeline_layout,
                    0,
                    &step.descriptor_sets,
                    &[],
                );
            }
            match step.draw {
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
                    info.device.cmd_bind_index_buffer(
                        info.command_buffer,
                        buffer,
                        offset,
                        index_type,
                    );
                    info.device.cmd_draw_indexed(
                        info.command_buffer,
                        index_count,
                        instance_count,
                        first_index,
                        base_vertex,
                        first_instance,
                    );
                }
            }
        }
        info.device.cmd_end_render_pass(info.command_buffer);
        if let Some(transition) = info.render_target_transition {
            transition_image_layout(
                info.device,
                info.command_buffer,
                transition.image,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            );
        }
        info.device.end_command_buffer(info.command_buffer)
    }
    .map_err(VulkanError::from)?;
    Ok(())
}

fn vk_rect_for_scissor(scissor: gfx_core::ScissorRect, extent: vk::Extent2D) -> Result<vk::Rect2D> {
    let x = scissor.x.min(extent.width);
    let y = scissor.y.min(extent.height);
    let right = scissor.x.saturating_add(scissor.width).min(extent.width);
    let bottom = scissor.y.saturating_add(scissor.height).min(extent.height);
    Ok(vk::Rect2D {
        offset: vk::Offset2D {
            x: i32::try_from(x)
                .map_err(|error| GfxError::InvalidInput(format!("scissor x overflow: {error}")))?,
            y: i32::try_from(y)
                .map_err(|error| GfxError::InvalidInput(format!("scissor y overflow: {error}")))?,
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
) -> Result<()> {
    // SAFETY: Command buffer is recording and can be ended.
    unsafe { device.end_command_buffer(command_buffer) }.map_err(VulkanError::from)?;
    let command_buffers = [command_buffer];
    let submit_info = vk::SubmitInfo::default().command_buffers(&command_buffers);
    // SAFETY: Queue and command buffer belong to the same device.
    unsafe { device.queue_submit(queue, &[submit_info], vk::Fence::null()) }
        .map_err(VulkanError::from)?;
    // SAFETY: Queue belongs to this device; waiting before destroying temporary resources is valid.
    unsafe { device.queue_wait_idle(queue) }.map_err(VulkanError::from)?;
    // SAFETY: Command pool belongs to this device and owns command_buffer.
    unsafe { device.destroy_command_pool(command_pool, None) };
    Ok(())
}

fn transition_image_layout(
    device: &ash::Device,
    command_buffer: vk::CommandBuffer,
    image: vk::Image,
    old_layout: vk::ImageLayout,
    new_layout: vk::ImageLayout,
) {
    let (source_stage, destination_stage, source_access_mask, destination_access_mask) =
        match (old_layout, new_layout) {
            (vk::ImageLayout::UNDEFINED, vk::ImageLayout::TRANSFER_DST_OPTIMAL) => (
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::AccessFlags::empty(),
                vk::AccessFlags::TRANSFER_WRITE,
            ),
            (vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL, vk::ImageLayout::TRANSFER_DST_OPTIMAL) => (
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::PipelineStageFlags::TRANSFER,
                vk::AccessFlags::SHADER_READ,
                vk::AccessFlags::TRANSFER_WRITE,
            ),
            (vk::ImageLayout::TRANSFER_DST_OPTIMAL, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL) => (
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::AccessFlags::TRANSFER_WRITE,
                vk::AccessFlags::SHADER_READ,
            ),
            (vk::ImageLayout::UNDEFINED, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL) => (
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags::empty(),
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            ),
            (
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            ) => (
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::AccessFlags::SHADER_READ,
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            ),
            (
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            ) => (
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                vk::AccessFlags::SHADER_READ,
            ),
            _ => (
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                vk::AccessFlags::empty(),
                vk::AccessFlags::empty(),
            ),
        };
    let barrier = vk::ImageMemoryBarrier::default()
        .old_layout(old_layout)
        .new_layout(new_layout)
        .src_access_mask(source_access_mask)
        .dst_access_mask(destination_access_mask)
        .image(image)
        .subresource_range(
            vk::ImageSubresourceRange::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .base_mip_level(0)
                .level_count(1)
                .base_array_layer(0)
                .layer_count(1),
        );
    // SAFETY: Command buffer is recording and image barrier targets the whole image subresource.
    unsafe {
        device.cmd_pipeline_barrier(
            command_buffer,
            source_stage,
            destination_stage,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        );
    }
}

fn format_to_vk(format: Format) -> vk::Format {
    match format {
        Format::Bgra8Unorm => vk::Format::B8G8R8A8_UNORM,
        Format::Bgra8UnormSrgb => vk::Format::B8G8R8A8_SRGB,
        Format::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
        Format::Rgba8UnormSrgb => vk::Format::R8G8B8A8_SRGB,
        Format::Depth32Float => vk::Format::D32_SFLOAT,
    }
}

fn image_aspect_for_format(format: Format) -> vk::ImageAspectFlags {
    match format {
        Format::Depth32Float => vk::ImageAspectFlags::DEPTH,
        Format::Bgra8Unorm
        | Format::Bgra8UnormSrgb
        | Format::Rgba8Unorm
        | Format::Rgba8UnormSrgb => vk::ImageAspectFlags::COLOR,
    }
}

fn buffer_usage_to_vk(usage: BufferUsage) -> vk::BufferUsageFlags {
    let mut flags = vk::BufferUsageFlags::empty();
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

fn address_mode_to_vk(mode: AddressMode) -> vk::SamplerAddressMode {
    match mode {
        AddressMode::ClampToEdge => vk::SamplerAddressMode::CLAMP_TO_EDGE,
        AddressMode::Repeat => vk::SamplerAddressMode::REPEAT,
    }
}

fn resource_binding_type_to_vk(binding_type: ResourceBindingType) -> vk::DescriptorType {
    match binding_type {
        ResourceBindingType::UniformBuffer => vk::DescriptorType::UNIFORM_BUFFER,
        ResourceBindingType::StorageBuffer => vk::DescriptorType::STORAGE_BUFFER,
        ResourceBindingType::SampledTexture => vk::DescriptorType::SAMPLED_IMAGE,
        ResourceBindingType::Sampler => vk::DescriptorType::SAMPLER,
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
    flags
}

fn vertex_format_to_vk(format: VertexFormat) -> vk::Format {
    match format {
        VertexFormat::Float32x2 => vk::Format::R32G32_SFLOAT,
        VertexFormat::Float32x3 => vk::Format::R32G32B32_SFLOAT,
        VertexFormat::Float32x4 => vk::Format::R32G32B32A32_SFLOAT,
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
    fn present_mode_falls_back_to_fifo_when_missing() {
        let mode = choose_present_mode(&[vk::PresentModeKHR::FIFO], PresentMode::Mailbox);

        assert_eq!(mode, vk::PresentModeKHR::FIFO);
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
}
