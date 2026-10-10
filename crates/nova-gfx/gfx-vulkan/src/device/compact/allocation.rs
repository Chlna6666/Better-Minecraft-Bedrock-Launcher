use super::*;

pub(super) fn allocate_buffer(
    device: &ash::Device,
    allocator: &mut MemoryAllocator,
    desc: &BufferDescriptor,
    source: vk::DeviceMemory,
) -> Result<Option<VulkanBuffer>> {
    let info = vk::BufferCreateInfo::default()
        .size(desc.size)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .usage(
            buffer_usage_to_vk(desc.usage)
                | vk::BufferUsageFlags::TRANSFER_SRC
                | vk::BufferUsageFlags::TRANSFER_DST,
        );
    // SAFETY: Descriptor copied from a live supported buffer, same native device.
    let buffer = unsafe { device.create_buffer(&info, None) }.map_err(VulkanError::from)?;
    let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
    let allocation = match allocator.allocate_vulkan_relocation(
        source,
        gfx_memory::VulkanRelocationDescriptor {
            name: desc.label.as_deref().unwrap_or("compact buffer"),
            requirements,
            location: desc.memory_location,
            linear: true,
        },
    ) {
        Ok(Some(allocation)) => allocation,
        Ok(None) => {
            unsafe {
                device.destroy_buffer(buffer, None);
            }
            return Ok(None);
        }
        Err(error) => {
            unsafe {
                device.destroy_buffer(buffer, None);
            }
            return Err(error);
        }
    };
    let (memory, offset) = vulkan_memory(&allocation)?;
    if let Err(error) = unsafe { device.bind_buffer_memory(buffer, memory, offset) } {
        unsafe {
            device.destroy_buffer(buffer, None);
        }
        allocator.free(allocation)?;
        return Err(VulkanError::from(error).into());
    }
    Ok(Some(VulkanBuffer {
        buffer,
        allocation,
        desc: desc.clone(),
    }))
}

pub(super) fn allocate_texture(
    device: &ash::Device,
    allocator: &mut MemoryAllocator,
    desc: &TextureDescriptor,
    source: vk::DeviceMemory,
) -> Result<Option<VulkanTexture>> {
    let info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(format_to_vk(desc.format))
        .extent(vk::Extent3D {
            width: desc.size.width(),
            height: desc.size.height(),
            depth: 1,
        })
        .mip_levels(desc.mip_level_count)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .usage(
            texture_usage_to_vk(desc.usage)
                | vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::TRANSFER_DST,
        );
    // SAFETY: Descriptor copied from a live supported texture, same native device.
    let image = unsafe { device.create_image(&info, None) }.map_err(VulkanError::from)?;
    let requirements = unsafe { device.get_image_memory_requirements(image) };
    let allocation = match allocator.allocate_vulkan_relocation(
        source,
        gfx_memory::VulkanRelocationDescriptor {
            name: desc.label.as_deref().unwrap_or("compact image"),
            requirements,
            location: desc.memory_location,
            linear: false,
        },
    ) {
        Ok(Some(allocation)) => allocation,
        Ok(None) => {
            unsafe {
                device.destroy_image(image, None);
            }
            return Ok(None);
        }
        Err(error) => {
            unsafe {
                device.destroy_image(image, None);
            }
            return Err(error);
        }
    };
    let (memory, offset) = vulkan_memory(&allocation)?;
    if let Err(error) = unsafe { device.bind_image_memory(image, memory, offset) } {
        unsafe {
            device.destroy_image(image, None);
        }
        allocator.free(allocation)?;
        return Err(VulkanError::from(error).into());
    }
    Ok(Some(VulkanTexture {
        image,
        allocation,
        desc: desc.clone(),
        layout: vk::ImageLayout::UNDEFINED,
    }))
}

pub(super) fn allocation_memory(allocation: &MemoryAllocation) -> Result<vk::DeviceMemory> {
    // SAFETY: Only compares a live handle; ownership remains with the allocator.
    unsafe { allocation.vulkan_memory() }
        .map(|(memory, _)| memory)
        .ok_or_else(|| Error::InvalidInput("non-Vulkan allocation in Vulkan registry".into()))
}
