//! Device-owned pool pages. Live and fence-retired sets keep their original pool.
use super::*;

const PAGE_CAPACITY: u32 = 256;
const TYPES: [vk::DescriptorType; 5] = [
    vk::DescriptorType::UNIFORM_BUFFER,
    vk::DescriptorType::STORAGE_BUFFER,
    vk::DescriptorType::SAMPLED_IMAGE,
    vk::DescriptorType::STORAGE_IMAGE,
    vk::DescriptorType::SAMPLER,
];

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Demand([u32; 5]);

impl Demand {
    pub(super) fn from_layout(layout: &BindGroupLayoutDescriptor) -> Self {
        let mut demand = Self::default();
        for entry in &layout.entries {
            let ty = resource_binding_type_to_vk(entry.binding_type);
            let index = TYPES.iter().position(|&known| known == ty).expect("supported descriptor type");
            demand.0[index] += 1;
        }
        demand
    }
}

struct Page {
    pool: vk::DescriptorPool,
    free_sets: u32,
    free: [u32; 5],
}

impl Page {
    fn fits(&self, demand: Demand) -> bool {
        self.free_sets != 0 && self.free.iter().zip(demand.0).all(|(&free, count)| free >= count)
    }
    fn consume(&mut self, demand: Demand) {
        self.free_sets -= 1;
        for (free, count) in self.free.iter_mut().zip(demand.0) { *free -= count; }
    }
    fn release(&mut self, demand: Demand) {
        self.free_sets += 1;
        for (free, count) in self.free.iter_mut().zip(demand.0) { *free += count; }
    }
}

#[derive(Default)]
pub(super) struct Pools {
    pages: Vec<Page>,
}

impl Pools {
    pub(super) fn allocate(
        &mut self,
        device: &ash::Device,
        layout: vk::DescriptorSetLayout,
        demand: Demand,
    ) -> Result<(vk::DescriptorSet, vk::DescriptorPool)> {
        // Capacity is restored only after successful native free at retirement.
        // Skip exhausted pages without an expensive failed driver allocation.
        for page in self.pages.iter_mut().rev() {
            if !page.fits(demand) { continue; }
            match allocate(device, page.pool, layout) {
                Ok(set) => {
                    page.consume(demand);
                    return Ok((set, page.pool));
                },
                Err(vk::Result::ERROR_OUT_OF_POOL_MEMORY | vk::Result::ERROR_FRAGMENTED_POOL) => {}
                Err(error) => return Err(VulkanError::from(error).into()),
            }
        }
        let capacity = demand.0.map(|count| count.max(PAGE_CAPACITY));
        let pool = create_pool(device, capacity)?;
        match allocate(device, pool, layout) {
            Ok(set) => {
                let mut page = Page { pool, free_sets: PAGE_CAPACITY, free: capacity };
                page.consume(demand);
                self.pages.push(page);
                Ok((set, pool))
            }
            Err(error) => {
                // SAFETY: No set was allocated from this new pool.
                unsafe { device.destroy_descriptor_pool(pool, None) };
                Err(VulkanError::from(error).into())
            }
        }
    }

    pub(super) fn free(
        &mut self, device: &ash::Device, pool: vk::DescriptorPool,
        set: vk::DescriptorSet, demand: Demand,
    ) -> Result<()> {
        let page = self.pages.iter_mut().find(|page| page.pool == pool)
            .ok_or_else(|| Error::Backend("descriptor pool ownership missing".into()))?;
        // SAFETY: caller proved the descriptor set is no longer in use.
        unsafe { device.free_descriptor_sets(pool, &[set]) }.map_err(VulkanError::from)?;
        page.release(demand);
        Ok(())
    }

    // Called only after device idle and all live/deferred sets have been destroyed.
    pub(super) fn destroy(&mut self, device: &ash::Device) {
        for page in self.pages.drain(..) {
            // SAFETY: Pools belong to this device and no descriptor sets remain in use.
            unsafe { device.destroy_descriptor_pool(page.pool, None) };
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

fn create_pool(device: &ash::Device, capacity: [u32; 5]) -> Result<vk::DescriptorPool> {
    let sizes = std::array::from_fn::<_, 5, _>(|index| vk::DescriptorPoolSize {
        ty: TYPES[index],
        descriptor_count: capacity[index],
    });
    let info = vk::DescriptorPoolCreateInfo::default()
        .flags(vk::DescriptorPoolCreateFlags::FREE_DESCRIPTOR_SET)
        .max_sets(PAGE_CAPACITY)
        .pool_sizes(&sizes);
    // SAFETY: All sizes are nonzero and the device owns the resulting pool.
    unsafe { device.create_descriptor_pool(&info, None) }
        .map_err(|error| VulkanError::from(error).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_capacity_accounts_for_each_type_and_set() {
        let mut page = Page { pool: vk::DescriptorPool::null(), free_sets: 2, free: [4, 2, 4, 4, 4] };
        let demand = Demand([1, 2, 0, 0, 0]);
        assert!(page.fits(demand));
        page.consume(demand);
        assert!(!page.fits(demand));
        assert!(page.fits(Demand([1, 0, 0, 0, 0])));
        page.release(demand);
        assert!(page.fits(demand));
    }

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
                        .allocate(&device.device, layout, Demand::default())
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
                pools.free(&device.device, pool, set, Demand::default()).expect("free test set");
            }
        }
        pools.destroy(&device.device);
        // SAFETY: All sets using this layout have been freed.
        unsafe { device.device.destroy_descriptor_set_layout(layout, None) };
    }
}
