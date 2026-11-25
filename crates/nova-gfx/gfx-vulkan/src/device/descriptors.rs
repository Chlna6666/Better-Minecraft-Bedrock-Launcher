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
    /// Aggregate resource limit for a shader stage, including sampled images.
    pub max_resources_per_stage: u32,
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
            max_resources_per_stage: limits.max_per_stage_resources,
        };
        enabled.descriptor_indexing = u32::from(core);
        enabled.shader_sampled_image_array_non_uniform_indexing = u32::from(result.sampled_image_non_uniform);
        enabled.runtime_descriptor_array = u32::from(result.runtime_arrays);
        enabled.descriptor_binding_partially_bound = u32::from(result.partially_bound);
        enabled.descriptor_binding_variable_descriptor_count = u32::from(result.variable_count);
        result
    }

    /// Upper bound for a fully populated, fixed-size sampled image table.
    /// This path requires non-uniform indexing but not variable allocation,
    /// partially bound descriptors, or update-after-bind.
    pub fn fixed_sampled_image_capacity(self) -> u32 {
        if self.sampled_image_non_uniform {
            self.max_sampled_images_per_stage
                .min(self.max_sampled_images_per_set)
                .min(self.max_resources_per_stage)
        } else { 0 }
    }

    /// Upper bound for an ordinary sparse variable-size sampled-image table,
    /// before accounting for other descriptors in its pipeline layout.
    pub fn sparse_sampled_image_capacity(self) -> u32 {
        if self.sampled_image_non_uniform && self.runtime_arrays && self.partially_bound && self.variable_count {
            self.max_sampled_images_per_stage
                .min(self.max_sampled_images_per_set)
                .min(self.max_resources_per_stage)
        } else { 0 }
    }
}
impl VulkanDevice {
    /// Enabled native features, without promising a bindless resource API.
    pub fn descriptor_indexing_capabilities(&self) -> DescriptorIndexingCapabilities {
        self.descriptor_indexing
    }
}

impl VulkanDevice {
    pub(super) fn create_sampled_table_layout(&mut self, capacity: u32) -> Result<BindGroupLayoutId> {
        if capacity == 0 || capacity > self.descriptor_indexing.fixed_sampled_image_capacity() {
            return Err(Error::Unavailable("sampled image table exceeds enabled Vulkan limits".into()));
        }
        let binding = vk::DescriptorSetLayoutBinding::default()
            .binding(0).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .descriptor_count(capacity).stage_flags(vk::ShaderStageFlags::FRAGMENT);
        let bindings = [binding];
        let info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        // SAFETY: fully populated ordinary array, no update-after-bind flags.
        let layout = unsafe { self.device.create_descriptor_set_layout(&info, None) }
            .map_err(VulkanError::from)?;
        Ok(self.bind_group_layouts.insert(VulkanResourceSetLayout {
            layout,
            desc: BindGroupLayoutDescriptor {
                label: Some("sampled texture table".into()),
                entries: vec![gfx_core::BindGroupLayoutEntry {
                    binding: 0, binding_type: BindingType::SampledTexture,
                    stages: ShaderStages::FRAGMENT,
                }],
            },
            sampled_table_capacity: Some(capacity),
        }))
    }

    pub(super) fn sampled_table_infos(&self, views: &[TextureViewId]) -> Result<Vec<vk::DescriptorImageInfo>> {
        views.iter().map(|&id| {
            let view = self.texture_views.get(id)?;
            let texture = self.textures.get(view.texture)?;
            if !texture.desc.usage.contains(TextureUsage::SAMPLED)
                || texture.desc.dimension != TextureDimension::D2
                || texture.desc.sample_count != 1
                || texture.desc.format == Format::Depth32Float
            {
                return Err(Error::InvalidInput("sampled table requires single-sample sampled color D2 views".into()));
            }
            Ok(vk::DescriptorImageInfo::default()
                .image_view(view.view).image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL))
        }).collect()
    }

    pub(super) fn create_sampled_table(&mut self, layout_id: BindGroupLayoutId, views: &[TextureViewId]) -> Result<BindGroupId> {
        self.poll_cleanup();
        let layout = self.bind_group_layouts.get(layout_id)?;
        let capacity = layout.sampled_table_capacity.ok_or_else(||
            Error::InvalidInput("layout is not a sampled texture table".into()))?;
        if views.len() != capacity as usize {
            return Err(Error::InvalidInput("sampled texture table must be fully populated".into()));
        }
        let native_layout = layout.layout;
        let infos = self.sampled_table_infos(views)?;
        let demand = Demand([0, 0, capacity, 0, 0]);
        let (descriptor_set, descriptor_pool) =
            self.descriptor_pools.allocate(&self.device, native_layout, demand)?;
        let write = vk::WriteDescriptorSet::default().dst_set(descriptor_set)
            .dst_binding(0).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .image_info(&infos);
        // SAFETY: this freshly allocated set has no GPU readers.
        unsafe { self.device.update_descriptor_sets(&[write], &[]) };
        Ok(self.bind_groups.insert(VulkanResourceSet {
            sampled_depth: Vec::new(),
            sampled_table: Some(views.to_vec()),
            descriptor_set, descriptor_pool, descriptor_demand: demand,
            layout: layout_id,
            desc: BindGroupDescriptor { label: None, layout: layout_id, entries: Vec::new() },
        }))
    }
}

// Partition into contiguous dirty/unchanged spans without a dense scratch bitmap.
fn table_spans(capacity: u32, changes: &[(u32, TextureViewId)]) -> Result<Vec<(u32, u32, bool)>> {
    if changes.is_empty() || changes.iter().any(|&(index, _)| index >= capacity)
        || changes.windows(2).any(|pair| pair[0].0 >= pair[1].0)
    {
        return Err(Error::InvalidInput("table edits must be nonempty, ordered, unique and in range".into()));
    }
    let mut spans = Vec::new();
    let mut cursor = 0;
    let mut i = 0;
    while i < changes.len() {
        let start = changes[i].0;
        if cursor < start { spans.push((cursor, start - cursor, false)); }
        i += 1;
        let mut end = start + 1;
        while i < changes.len() && changes[i].0 == end { end += 1; i += 1; }
        spans.push((start, end - start, true));
        cursor = end;
    }
    if cursor < capacity { spans.push((cursor, capacity - cursor, false)); }
    Ok(spans)
}

impl VulkanDevice {
    pub(super) fn fork_sampled_table(&mut self, source_id: BindGroupId, changes: &[(u32, TextureViewId)]) -> Result<BindGroupId> {
        self.poll_cleanup();
        let source = self.bind_groups.get(source_id)?;
        let mut views = source.sampled_table.clone().ok_or_else(||
            Error::InvalidInput("source is not a sampled texture table".into()))?;
        let capacity = views.len() as u32;
        let spans = table_spans(capacity, changes)?;
        let layout_id = source.layout;
        let source_set = source.descriptor_set;
        let layout = self.bind_group_layouts.get(layout_id)?.layout;
        for &(slot, view) in changes { views[slot as usize] = view; }
        for &id in &views {
            let view = self.texture_views.get(id)?;
            self.textures.get(view.texture)?;
        }
        let dirty_views: Vec<_> = changes.iter().map(|&(_, view)| view).collect();
        let infos = self.sampled_table_infos(&dirty_views)?;
        let demand = Demand([0, 0, capacity, 0, 0]);
        let (descriptor_set, descriptor_pool) =
            self.descriptor_pools.allocate(&self.device, layout, demand)?;
        let mut writes = Vec::new();
        let mut copies = Vec::new();
        let mut offset = 0;
        for (start, count, dirty) in spans {
            if dirty {
                let end = offset + count as usize;
                writes.push(vk::WriteDescriptorSet::default().dst_set(descriptor_set)
                    .dst_binding(0).dst_array_element(start)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .image_info(&infos[offset..end]));
                offset = end;
            } else {
                copies.push(vk::CopyDescriptorSet::default().src_set(source_set)
                    .src_binding(0).src_array_element(start)
                    .dst_set(descriptor_set).dst_binding(0).dst_array_element(start)
                    .descriptor_count(count));
            }
        }
        // Only the new destination changes; existing GPU readers keep their old set.
        unsafe { self.device.update_descriptor_sets(&writes, &copies) };
        Ok(self.bind_groups.insert(VulkanResourceSet {
            sampled_depth: Vec::new(), sampled_table: Some(views),
            descriptor_set, descriptor_pool, descriptor_demand: demand,
            layout: layout_id,
            desc: BindGroupDescriptor { label: None, layout: layout_id, entries: Vec::new() },
        }))
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
    fn sampled_table_dirty_spans_are_exact_and_ordered() {
        let view = TextureViewId::from_parts(1, 1);
        assert_eq!(table_spans(8, &[(1, view), (2, view), (5, view)]).unwrap(),
            [(0, 1, false), (1, 2, true), (3, 2, false), (5, 1, true), (6, 2, false)]);
        assert!(table_spans(8, &[(2, view), (2, view)]).is_err());
        assert!(table_spans(8, &[(8, view)]).is_err());
        assert!(table_spans(8, &[(3, view), (1, view)]).is_err());
        assert!(table_spans(8, &[]).is_err());
    }

    #[test]
    #[ignore = "requires native Vulkan device"]
    fn native_sampled_table_shader_reads_nonuniform_slots() {
        use gfx_core::{ColorTargetState, ColorAttachmentDescriptor, RenderPassDescriptor,
            PipelineLayoutDescriptor, RenderPipelineDescriptor, ShaderModuleDescriptor,
            RenderPassCommands, RenderCommand, DrawCommand, ClearColor, LoadOp, BlendMode,
            PrimitiveTopology, TextureViewDescriptor, TextureWriteDescriptor, TextureDataLayout,
            Origin2d, Extent2d, ShaderStage, TextureTransferDevice, PresentationDevice};
        const SOURCE: &str = r"
@group(0) @binding(0) var images: binding_array<texture_2d<f32>, 4>;
@vertex fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = array<vec2<f32>, 3>(vec2<f32>(-1., -1.), vec2<f32>(3., -1.), vec2<f32>(-1., 3.));
    return vec4<f32>(p[i], 0., 1.);
}
@fragment fn fs_main(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(images[u32(p.x)], vec2<i32>(0, 0), 0);
}
";
        let mut device = VulkanDevice::new(&DeviceDescriptor::default()).expect("Vulkan device");
        assert!(ResourceDevice::sampled_texture_table_capacity(&device) >= 4);
        let module = gfx_shader::WgslModule::parse_sampled_texture_table(SOURCE).expect("binding array WGSL");
        let vs = device.create_shader_module(&ShaderModuleDescriptor {
            label: None, binary: module.compile_spirv(ShaderStage::Vertex, "vs_main").unwrap(),
        }).unwrap();
        let fs = device.create_shader_module(&ShaderModuleDescriptor {
            label: None, binary: module.compile_spirv(ShaderStage::Fragment, "fs_main").unwrap(),
        }).unwrap();
        let mut views = Vec::new();
        let colors = [[255u8, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 0, 255]];
        let unit = Extent2d::new(1, 1).unwrap();
        for color in colors {
            let texture = device.create_texture(&TextureDescriptor {
                label: None, size: unit, mip_level_count: 1, format: Format::Rgba8Unorm,
                usage: TextureUsage::SAMPLED | TextureUsage::COPY_DST,
                memory_location: MemoryLocation::GpuOnly, dimension: TextureDimension::D2, sample_count: 1,
            }).unwrap();
            TextureTransferDevice::write_texture(&mut device, TextureWriteDescriptor {
                texture, mip_level: 0, origin: Origin2d::ZERO, size: unit,
                layout: TextureDataLayout::new(0, 4, 1).unwrap(),
            }, &color).unwrap();
            views.push(device.create_texture_view(&TextureViewDescriptor {
                label: None, texture, base_mip_level: 0, mip_level_count: 1, format: Format::Rgba8Unorm,
            }).unwrap());
        }
        let size = Extent2d::new(4, 1).unwrap();
        let target = device.create_texture(&TextureDescriptor {
            label: None, size, mip_level_count: 1, format: Format::Rgba8Unorm,
            usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::COPY_SRC,
            memory_location: MemoryLocation::GpuOnly, dimension: TextureDimension::D2, sample_count: 1,
        }).unwrap();
        let target_view = device.create_texture_view(&TextureViewDescriptor {
            label: None, texture: target, base_mip_level: 0, mip_level_count: 1, format: Format::Rgba8Unorm,
        }).unwrap();
        let layout = device.create_sampled_table_layout(4).unwrap();
        let old = device.create_sampled_table(layout, &views).unwrap();
        let next = device.fork_sampled_table(old, &[(1, views[3]), (3, views[1])]).unwrap();
        let pass = device.create_render_pass(&RenderPassDescriptor {
            label: None, color_attachments: vec![ColorAttachmentDescriptor { format: Format::Rgba8Unorm }],
            depth_attachment: None, sample_count: 1, resolve_target: None,
        }).unwrap();
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: None, bind_group_layouts: vec![layout],
        }).unwrap();
        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: None, vertex_shader: vs, vertex_entry_point: "vs_main".into(),
            fragment_shader: fs, fragment_entry_point: "fs_main".into(), vertex_buffers: Vec::new(),
            render_pass: pass, pipeline_layout: Some(pipeline_layout),
            color_targets: vec![ColorTargetState::new(Format::Rgba8Unorm, BlendMode::Replace)],
            primitive_topology: PrimitiveTopology::TriangleList, raster: Default::default(), depth_state: None,
        }, size).unwrap();
        for (set, order) in [(old, [0, 1, 2, 3]), (next, [0, 3, 2, 1]), (old, [0, 1, 2, 3])] {
            let commands = [RenderCommand::Draw(DrawCommand {
                pipeline, bind_groups: gfx_core::bind_group_list([set]),
                vertex_count: 3, first_vertex: 0, instance_count: 1, first_instance: 0, scissor: None,
            })];
            PresentationDevice::render_to_textures(&mut device, &[RenderPassCommands {
                color_attachments: &[target_view], render_pass: pass, commands: &commands,
                color_load_op: LoadOp::Clear(ClearColor::default()), depth_attachment: None,
                clear_region: None, viewport: None, resolve_target: None,
            }]).unwrap();
            let pixels = TextureTransferDevice::read_texture(&mut device, target).unwrap();
            let expected: Vec<u8> = order.into_iter().flat_map(|i| colors[i]).collect();
            assert_eq!(pixels.bytes, expected);
        }
        eprintln!("NOVA_VULKAN_SAMPLED_TABLE_PIXELS=pass");
    }

    #[test]
    #[ignore = "requires native Vulkan device"]
    fn native_sampled_table_snapshot_preserves_old_slots() {
        let mut device = VulkanDevice::new(&DeviceDescriptor::default()).expect("Vulkan device");
        if ResourceDevice::sampled_texture_table_capacity(&device) < 4 {
            eprintln!("NOVA_VULKAN_SAMPLED_TABLE=unsupported");
            return;
        }
        let mut views = Vec::new();
        let mut textures = Vec::new();
        for _ in 0..2 {
            let texture = device.create_texture(&TextureDescriptor {
                label: None, size: gfx_core::Extent2d::new(1, 1).unwrap(), mip_level_count: 1,
                format: Format::Rgba8Unorm, usage: TextureUsage::SAMPLED | TextureUsage::COPY_DST,
                memory_location: MemoryLocation::GpuOnly, dimension: TextureDimension::D2, sample_count: 1,
            }).unwrap();
            let view = device.create_texture_view(&gfx_core::TextureViewDescriptor {
                label: None, texture, base_mip_level: 0, mip_level_count: 1, format: Format::Rgba8Unorm,
            }).unwrap();
            textures.push(texture);
            views.push(view);
        }
        let layout = device.create_sampled_table_layout(4).unwrap();
        let old = device.create_sampled_table(layout, &[views[0]; 4]).unwrap();
        let next = device.fork_sampled_table(old, &[(1, views[1]), (2, views[1])]).unwrap();
        let a = device.bind_groups.get(old).unwrap();
        let b = device.bind_groups.get(next).unwrap();
        assert_ne!(a.descriptor_set, b.descriptor_set);
        assert_eq!(a.sampled_table.as_deref(), Some(&[views[0]; 4][..]));
        assert_eq!(b.sampled_table.as_deref(), Some(&[views[0], views[1], views[1], views[0]][..]));
        assert!(device.fork_sampled_table(old, &[(4, views[1])]).is_err());
        device.destroy_bind_group(old).unwrap();
        device.destroy_bind_group(next).unwrap();
        device.destroy_bind_group_layout(layout).unwrap();
        for view in views { device.destroy_texture_view(view).unwrap(); }
        for texture in textures { device.destroy_texture(texture).unwrap(); }
        eprintln!("NOVA_VULKAN_SAMPLED_TABLE=pass");
    }

    #[test]
    fn descriptor_indexing_plan_never_enables_unqueried_or_update_after_bind_features() {
        let mut supported = vk::PhysicalDeviceVulkan12Features::default()
            .descriptor_indexing(true).shader_sampled_image_array_non_uniform_indexing(true)
            .runtime_descriptor_array(true).descriptor_binding_partially_bound(true)
            .descriptor_binding_sampled_image_update_after_bind(true);
        let limits = vk::PhysicalDeviceLimits {
            max_per_stage_descriptor_sampled_images: 128,
            max_descriptor_set_sampled_images: 256,
            max_per_stage_resources: 96,
            ..Default::default()
        };
        let mut enabled = vk::PhysicalDeviceVulkan12Features::default();
        let partial = DescriptorIndexingCapabilities::enable(&supported, &limits, &mut enabled);
        assert_eq!(partial.sparse_sampled_image_capacity(), 0);
        assert_eq!(enabled.descriptor_binding_variable_descriptor_count, vk::FALSE);
        assert_eq!(enabled.descriptor_binding_sampled_image_update_after_bind, vk::FALSE);
        supported.descriptor_binding_variable_descriptor_count = vk::TRUE;
        let full = DescriptorIndexingCapabilities::enable(&supported, &limits, &mut enabled);
        // The aggregate resource limit can be smaller than either image limit.
        assert_eq!(full.sparse_sampled_image_capacity(), 96);
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
