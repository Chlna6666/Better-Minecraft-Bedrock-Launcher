//! Vulkan 1.4 host initialization of small, never-submitted sampled textures.
//! Existing GPU-visible textures retain the asynchronous staging path.
use super::*;

// Bound synchronous CPU work and keep large transfers on the GPU upload ring.
const MAX_HOST_BYTES: u64 = 256 * 1024;

pub(super) struct HostCopyBudget { remaining: u64 }
impl Default for HostCopyBudget {
    fn default() -> Self { Self { remaining: MAX_HOST_BYTES } }
}
impl HostCopyBudget {
    pub(super) fn allows(&self, bytes: usize) -> bool { bytes as u64 <= self.remaining }
    pub(super) fn consume(&mut self, bytes: usize) { self.remaining = self.remaining.saturating_sub(bytes as u64); }
}

type ImageFormatKey = (i32, i32, i32, u32, u32);

pub(super) struct HostImageCopy {
    transition: vk::PFN_vkTransitionImageLayoutEXT,
    copy: vk::PFN_vkCopyMemoryToImageEXT,
    formats: std::collections::HashMap<ImageFormatKey, Option<vk::ImageFormatProperties>>,
}

impl HostImageCopy {
    pub(super) fn load(instance: &Instance, physical: vk::PhysicalDevice, device: &ash::Device, api: u32) -> Option<Self> {
        if api < PREFERRED_API_VERSION { return None; }
        let mut props = vk::PhysicalDeviceHostImageCopyPropertiesEXT::default();
        let mut query = vk::PhysicalDeviceProperties2::default().push_next(&mut props);
        // SAFETY: feature was enabled and the device belongs to this instance.
        unsafe { instance.get_physical_device_properties2(physical, &mut query) };
        let mut dst = vec![vk::ImageLayout::UNDEFINED; props.copy_dst_layout_count as usize];
        let mut src = vec![vk::ImageLayout::UNDEFINED; props.copy_src_layout_count as usize];
        let mut props = vk::PhysicalDeviceHostImageCopyPropertiesEXT::default()
            .copy_dst_layouts(&mut dst).copy_src_layouts(&mut src);
        let mut query = vk::PhysicalDeviceProperties2::default().push_next(&mut props);
        unsafe { instance.get_physical_device_properties2(physical, &mut query) };
        if !dst.contains(&vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL) { return None; }
        // SAFETY: core 1.4 functions have the same ABI as the promoted EXT
        // definitions in Ash 0.38. Never call an absent entry point.
        let transition = unsafe { instance.get_device_proc_addr(device.handle(), c"vkTransitionImageLayout".as_ptr()) }?;
        let copy = unsafe { instance.get_device_proc_addr(device.handle(), c"vkCopyMemoryToImage".as_ptr()) }?;
        Some(Self {
            transition: unsafe { std::mem::transmute::<unsafe extern "system" fn(), vk::PFN_vkTransitionImageLayoutEXT>(transition) },
            copy: unsafe { std::mem::transmute::<unsafe extern "system" fn(), vk::PFN_vkCopyMemoryToImageEXT>(copy) },
            formats: Default::default(),
        })
    }

    pub(super) fn supports_image(&mut self, instance: &Instance, physical: vk::PhysicalDevice,
        desc: &TextureDescriptor, create: &vk::ImageCreateInfo<'_>) -> bool {
        if desc.dimension != TextureDimension::D2 || desc.sample_count != 1 || desc.mip_level_count != 1
            || desc.format.is_depth() || desc.byte_size() > MAX_HOST_BYTES
            || !desc.usage.contains(TextureUsage::SAMPLED | TextureUsage::COPY_DST)
            || desc.usage.intersects(TextureUsage::COLOR_ATTACHMENT | TextureUsage::DEPTH_ATTACHMENT | TextureUsage::STORAGE)
        { return false; }
        let usage = create.usage | vk::ImageUsageFlags::HOST_TRANSFER_EXT;
        let key = (create.format.as_raw(), create.image_type.as_raw(), create.tiling.as_raw(),
            usage.as_raw(), create.flags.as_raw());
        // Both supported and unsupported format/usage combinations are cached.
        let cached = self.formats.entry(key).or_insert_with(|| {
            let mut perf = vk::HostImageCopyDevicePerformanceQueryEXT::default();
            let info = vk::PhysicalDeviceImageFormatInfo2::default()
                .format(create.format).ty(create.image_type).tiling(create.tiling)
                .usage(usage).flags(create.flags);
            let mut props = vk::ImageFormatProperties2::default().push_next(&mut perf);
            // SAFETY: valid image creation parameters, feature enabled.
            let supported = unsafe { instance.get_physical_device_image_format_properties2(physical, &info, &mut props) }.is_ok();
            let limits = props.image_format_properties;
            (supported && perf.optimal_device_access == vk::TRUE).then_some(limits)
        });
        cached.as_ref().is_some_and(|limits|
            limits.max_extent.width >= create.extent.width
            && limits.max_extent.height >= create.extent.height
            && limits.max_extent.depth >= create.extent.depth
            && limits.max_array_layers >= create.array_layers
            && limits.max_mip_levels >= create.mip_levels
            && limits.sample_counts.contains(create.samples))
    }
}

impl VulkanDevice {
    pub(super) fn try_host_texture_write(&mut self, desc: TextureWriteDescriptor, data: &[u8]) -> Result<bool> {
        let Some(copy) = &self.host_image_copy else { return Ok(false); };
        let texture = self.textures.get(desc.texture)?;
        // Only a full first initialization is eligible. No device access
        // can legally precede this UNDEFINED -> shader-readable transition.
        if !texture.host_copy || texture.layout != vk::ImageLayout::UNDEFINED
            || desc.mip_level != 0 || desc.origin != gfx_core::Origin2d::ZERO
            || desc.size != texture.desc.size { return Ok(false); }
        desc.validate_against(&texture.desc, data.len())?;
        let row = desc.layout.bytes_per_row.get() / texture.desc.format.bytes_per_pixel();
        let offset = usize::try_from(desc.layout.offset).map_err(|_| Error::InvalidInput("host image offset overflow".into()))?;
        let image = texture.image;
        let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1).layer_count(1);
        let transition = vk::HostImageLayoutTransitionInfoEXT::default().image(image)
            .old_layout(vk::ImageLayout::UNDEFINED).new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .subresource_range(range);
        // SAFETY: new image has no outstanding GPU access; host operations
        // are completed before later queue submission makes the writes visible.
        let status = unsafe { (copy.transition)(self.device.handle(), 1, &transition) };
        if status != vk::Result::SUCCESS { return Err(VulkanError::from(status).into()); }
        // The transition succeeded even if the following copy fails.
        self.textures.get_mut(desc.texture)?.layout = vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL;
        let region = vk::MemoryToImageCopyEXT::default()
            .host_pointer(data[offset..].as_ptr().cast())
            .memory_row_length(row).memory_image_height(desc.layout.rows_per_image.get())
            .image_subresource(vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1))
            .image_extent(vk::Extent3D { width: desc.size.width(), height: desc.size.height(), depth: 1 });
        let regions = [region];
        let info = vk::CopyMemoryToImageInfoEXT::default().dst_image(image)
            .dst_image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL).regions(&regions);
        let status = unsafe { (copy.copy)(self.device.handle(), &info) };
        if status != vk::Result::SUCCESS { return Err(VulkanError::from(status).into()); }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_copy_budget_caps_whole_batch() {
        let mut budget = HostCopyBudget::default();
        assert!(budget.allows(128 * 1024));
        budget.consume(128 * 1024);
        assert!(!budget.allows(128 * 1024 + 1));
        budget.consume(128 * 1024);
        assert!(!budget.allows(1));
    }

    #[test]
    #[ignore = "requires native Vulkan device"]
    fn native_host_copy_first_write_then_staging() {
        let mut device = VulkanDevice::new(&DeviceDescriptor::default()).expect("Vulkan device");
        let extent = gfx_core::Extent2d::new(2, 2).unwrap();
        let desc = TextureDescriptor {
            label: Some("host image copy regression".into()),
            size: extent, mip_level_count: 1, format: Format::Rgba8Unorm,
            usage: TextureUsage::SAMPLED | TextureUsage::COPY_SRC | TextureUsage::COPY_DST,
            memory_location: MemoryLocation::GpuOnly, dimension: TextureDimension::D2, sample_count: 1,
        };
        let texture = device.create_texture(&desc).unwrap();
        let eligible = device.textures.get(texture).unwrap().host_copy;
        let write = TextureWriteDescriptor { texture, mip_level: 0, origin: gfx_core::Origin2d::ZERO,
            size: extent, layout: gfx_core::TextureDataLayout::new(4, 12, 2).unwrap() };
        let pixels = [99,99,99,99, 1,2,3,4,5,6,7,8, 99,99,99,99, 9,10,11,12,13,14,15,16];
        let copied = device.try_host_texture_write(write, &pixels).unwrap();
        assert_eq!(copied, eligible);
        if !copied { device.write_texture(write, &pixels).unwrap(); }
        assert_eq!(device.read_texture_pixels(texture).unwrap().bytes, (1u8..=16).collect::<Vec<_>>());
        let patch = TextureWriteDescriptor { texture, mip_level: 0, origin: gfx_core::Origin2d::ZERO,
            size: extent, layout: gfx_core::TextureDataLayout::new(0, 8, 2).unwrap() };
        assert!(!device.try_host_texture_write(patch, &[42;16]).unwrap(), "initialized image must use staging");
        device.write_texture(patch, &[42;16]).unwrap();
        assert_eq!(device.read_texture_pixels(texture).unwrap().bytes, vec![42;16]);
        ResourceDevice::destroy_texture(&mut device, texture).unwrap();
        eprintln!("NOVA_VULKAN_HOST_IMAGE_COPY={}", if copied { "pass" } else { "unsupported_staging_pass" });
    }
}
