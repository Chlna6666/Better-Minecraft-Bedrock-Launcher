//! Device-owned pool pages. Live and fence-retired sets keep their original pool.
use super::*;

#[derive(Default)]
pub(super) struct Pools {
    pages: Vec<vk::DescriptorPool>,
}

impl Pools {
    pub(super) fn allocate(
        &mut self,
        device: &ash::Device,
        layout: vk::DescriptorSetLayout,
    ) -> Result<(vk::DescriptorSet, vk::DescriptorPool)> {
        // Try freed capacity in older pages as well, so repeated target churn
        // reuses the high-water allocation instead of growing indefinitely.
        for pool in self.pages.iter().rev().copied() {
            match allocate(device, pool, layout) {
                Ok(set) => return Ok((set, pool)),
                Err(vk::Result::ERROR_OUT_OF_POOL_MEMORY | vk::Result::ERROR_FRAGMENTED_POOL) => {}
                Err(error) => return Err(VulkanError::from(error).into()),
            }
        }
        let pool = create_pool(device)?;
        match allocate(device, pool, layout) {
            Ok(set) => {
                self.pages.push(pool);
                Ok((set, pool))
            }
            Err(error) => {
                // SAFETY: No set was allocated from this new pool.
                unsafe { device.destroy_descriptor_pool(pool, None) };
                Err(VulkanError::from(error).into())
            }
        }
    }

    // Called only after device idle and all live/deferred sets have been destroyed.
    pub(super) fn destroy(&mut self, device: &ash::Device) {
        for pool in self.pages.drain(..) {
            // SAFETY: Pools belong to this device and no descriptor sets remain in use.
            unsafe { device.destroy_descriptor_pool(pool, None) };
        }
    }
}

fn allocate(
    device: &ash::Device,
    pool: vk::DescriptorPool,
    layout: vk::DescriptorSetLayout,
) -> std::result::Result<vk::DescriptorSet, vk::Result> {
    let layouts = [layout];
    let info = vk::DescriptorSetAllocateInfo::default()
        .descriptor_pool(pool)
        .set_layouts(&layouts);
    // SAFETY: Pool and layout belong to this device; the owner serializes allocation/free.
    let sets = unsafe { device.allocate_descriptor_sets(&info) }?;
    Ok(sets[0])
}

fn create_pool(device: &ash::Device) -> Result<vk::DescriptorPool> {
    let sizes = [
        vk::DescriptorType::UNIFORM_BUFFER,
        vk::DescriptorType::STORAGE_BUFFER,
        vk::DescriptorType::SAMPLED_IMAGE,
        vk::DescriptorType::SAMPLER,
    ]
    .map(|ty| vk::DescriptorPoolSize {
        ty,
        descriptor_count: 256,
    });
    let info = vk::DescriptorPoolCreateInfo::default()
        .flags(vk::DescriptorPoolCreateFlags::FREE_DESCRIPTOR_SET)
        .max_sets(256)
        .pool_sizes(&sizes);
    // SAFETY: All sizes are nonzero and the device owns the resulting pool.
    unsafe { device.create_descriptor_pool(&info, None) }
        .map_err(|error| VulkanError::from(error).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires hardware Vulkan"]
    fn pool_pages_grow_and_reuse_freed_capacity() {
        let device = VulkanDevice::new(&DeviceDescriptor::default()).expect("Vulkan device");
        let info = vk::DescriptorSetLayoutCreateInfo::default();
        // SAFETY: An empty layout is valid and remains live through all allocations below.
        let layout = unsafe { device.device.create_descriptor_set_layout(&info, None) }
            .expect("test layout");
        let mut pools = Pools::default();
        for _ in 0..3 {
            let sets: Vec<_> = (0..600)
                .map(|_| {
                    pools
                        .allocate(&device.device, layout)
                        .expect("set beyond one page")
                })
                .collect();
            assert_eq!(
                pools.pages.len(),
                3,
                "reuse freed pages across allocation cycles"
            );
            for (set, pool) in sets {
                // SAFETY: The test never submits these sets, and frees each to its owner once.
                unsafe { device.device.free_descriptor_sets(pool, &[set]) }.expect("free test set");
            }
        }
        pools.destroy(&device.device);
        // SAFETY: All sets using this layout have been freed.
        unsafe { device.device.destroy_descriptor_set_layout(layout, None) };
    }
}
