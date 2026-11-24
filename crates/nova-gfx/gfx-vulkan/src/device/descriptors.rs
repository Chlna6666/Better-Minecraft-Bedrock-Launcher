//! Device-owned pool pages. Live and fence-retired sets keep their original pool.
use super::*;

/// Enabled native descriptor-indexing subset, not a portable bindless API.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DescriptorIndexingCapabilities {
    /// Sampled image indices may vary between shader invocations.
    pub sampled_image_non_uniform: bool,
    /// Runtime-sized descriptor arrays.
    pub runtime_arrays: bool,
    /// Unused descriptors need not be populated.
    pub partially_bound: bool,
    /// Last binding can have a variable allocation count.
    pub variable_count: bool,
    /// Ordinary sampled-image descriptor limit per stage.
    pub max_sampled_images_per_stage: u32,
    /// Ordinary sampled-image descriptor limit per pipeline layout.
    pub max_sampled_images_per_set: u32,
}
impl DescriptorIndexingCapabilities {
    pub(super) fn enable(
        supported: &vk::PhysicalDeviceVulkan12Features<'_>,
        limits: &vk::PhysicalDeviceLimits,
        enabled: &mut vk::PhysicalDeviceVulkan12Features<'_>,
    ) -> Self {
        let core = supported.descriptor_indexing == vk::TRUE;
        let result = Self {
            sampled_image_non_uniform: core && supported.shader_sampled_image_array_non_uniform_indexing == vk::TRUE,
            runtime_arrays: core && supported.runtime_descriptor_array == vk::TRUE,
            partially_bound: core && supported.descriptor_binding_partially_bound == vk::TRUE,
            variable_count: core && supported.descriptor_binding_variable_descriptor_count == vk::TRUE,
            max_sampled_images_per_stage: limits.max_per_stage_descriptor_sampled_images,
            max_sampled_images_per_set: limits.max_descriptor_set_sampled_images,
        };
        enabled.descriptor_indexing = u32::from(core);
        enabled.shader_sampled_image_array_non_uniform_indexing = u32::from(result.sampled_image_non_uniform);
        enabled.runtime_descriptor_array = u32::from(result.runtime_arrays);
        enabled.descriptor_binding_partially_bound = u32::from(result.partially_bound);
        enabled.descriptor_binding_variable_descriptor_count = u32::from(result.variable_count);
        result
    }

    /// Upper bound for an ordinary sparse variable-size sampled-image table,
    /// before accounting for other descriptors in its pipeline layout.
    pub fn sparse_sampled_image_capacity(self) -> u32 {
        if self.sampled_image_non_uniform && self.runtime_arrays && self.partially_bound && self.variable_count {
            self.max_sampled_images_per_stage.min(self.max_sampled_images_per_set)
        } else { 0 }
    }
}
impl VulkanDevice {
    /// Enabled native features, without promising a bindless resource API.
    pub fn descriptor_indexing_capabilities(&self) -> DescriptorIndexingCapabilities {
        self.descriptor_indexing
    }
}

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
    fn descriptor_indexing_plan_never_enables_unqueried_or_update_after_bind_features() {
        let mut supported = vk::PhysicalDeviceVulkan12Features::default()
            .descriptor_indexing(true).shader_sampled_image_array_non_uniform_indexing(true)
            .runtime_descriptor_array(true).descriptor_binding_partially_bound(true)
            .descriptor_binding_sampled_image_update_after_bind(true);
        let limits = vk::PhysicalDeviceLimits {
            max_per_stage_descriptor_sampled_images: 128,
            max_descriptor_set_sampled_images: 256,
            ..Default::default()
        };
        let mut enabled = vk::PhysicalDeviceVulkan12Features::default();
        let partial = DescriptorIndexingCapabilities::enable(&supported, &limits, &mut enabled);
        assert_eq!(partial.sparse_sampled_image_capacity(), 0);
        assert_eq!(enabled.descriptor_binding_variable_descriptor_count, vk::FALSE);
        assert_eq!(enabled.descriptor_binding_sampled_image_update_after_bind, vk::FALSE);
        supported.descriptor_binding_variable_descriptor_count = vk::TRUE;
        let full = DescriptorIndexingCapabilities::enable(&supported, &limits, &mut enabled);
        assert_eq!(full.sparse_sampled_image_capacity(), 128);
        supported.descriptor_indexing = vk::FALSE;
        let disabled = DescriptorIndexingCapabilities::enable(&supported, &limits, &mut enabled);
        assert_eq!(disabled.sparse_sampled_image_capacity(), 0);
        assert_eq!(enabled.runtime_descriptor_array, vk::FALSE);
    }

    #[test]
    #[ignore = "requires native Vulkan device"]
    fn native_sparse_descriptor_indexing_allocation() {
        let device = VulkanDevice::new(&DeviceDescriptor::default()).expect("Vulkan device");
        let caps = device.descriptor_indexing_capabilities();
        eprintln!("NOVA_VULKAN_DESCRIPTOR_INDEXING={caps:?}");
        if caps.sparse_sampled_image_capacity() < 4 {
            eprintln!("NOVA_VULKAN_DESCRIPTOR_INDEXING_SPARSE=unsupported");
            return;
        }
        let bindings = [vk::DescriptorSetLayoutBinding::default().binding(0)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .descriptor_count(4).stage_flags(vk::ShaderStageFlags::FRAGMENT)];
        let flags = [vk::DescriptorBindingFlags::PARTIALLY_BOUND
            | vk::DescriptorBindingFlags::VARIABLE_DESCRIPTOR_COUNT];
        let mut flag_info = vk::DescriptorSetLayoutBindingFlagsCreateInfo::default().binding_flags(&flags);
        let info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings).push_next(&mut flag_info);
        // Native layout and allocation proof; no descriptor is submitted or read.
        unsafe {
            let layout = device.device.create_descriptor_set_layout(&info, None).expect("sparse layout");
            let sizes = [vk::DescriptorPoolSize { ty: vk::DescriptorType::SAMPLED_IMAGE, descriptor_count: 2 }];
            let pool = device.device.create_descriptor_pool(&vk::DescriptorPoolCreateInfo::default()
                .max_sets(1).pool_sizes(&sizes), None).expect("sparse pool");
            let layouts = [layout];
            let counts = [2u32];
            let mut variable = vk::DescriptorSetVariableDescriptorCountAllocateInfo::default().descriptor_counts(&counts);
            let sets = device.device.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::default()
                .descriptor_pool(pool).set_layouts(&layouts).push_next(&mut variable)).expect("variable count allocation");
            assert_eq!(sets.len(), 1);
            device.device.destroy_descriptor_pool(pool, None);
            device.device.destroy_descriptor_set_layout(layout, None);
        }
        eprintln!("NOVA_VULKAN_DESCRIPTOR_INDEXING_SPARSE=pass");
    }

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
