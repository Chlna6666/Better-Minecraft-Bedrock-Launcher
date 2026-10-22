use super::*;

impl VulkanDevice {
    pub(super) fn validate_compact_bindings(&self) -> Result<()> {
        for id in {
            let ids: Vec<BindGroupId> = self.bind_groups.live_ids();
            ids
        } {
            let set = self.bind_groups.get(id)?;
            let layout = self.bind_group_layouts.get(set.layout)?;
            set.desc.validate_against(&layout.desc)?;
            for binding in &set.desc.entries {
                match binding.resource {
                    BindingResource::Buffer(binding) => {
                        binding.validate_against(self.buffers.get(binding.buffer)?.desc.size)?;
                    }
                    BindingResource::Texture(binding) => {
                        self.texture_views.get(binding.texture_view)?;
                    }
                    BindingResource::Sampler(binding) => {
                        self.samplers.get(binding.sampler)?;
                    }
                }
            }
        }
        Ok(())
    }

    pub(super) fn rewrite_compact_set(&self, id: BindGroupId) -> Result<()> {
        let set = self.bind_groups.get(id)?;
        let layout = self.bind_group_layouts.get(set.layout)?;
        for binding in &set.desc.entries {
            let mut write = vk::WriteDescriptorSet::default()
                .dst_set(set.descriptor_set)
                .dst_binding(binding.binding);
            match binding.resource {
                BindingResource::Buffer(binding_resource) => {
                    let info = vk::DescriptorBufferInfo::default()
                        .buffer(self.buffers.get(binding_resource.buffer)?.buffer)
                        .offset(binding_resource.offset)
                        .range(binding_resource.size);
                    let kind = layout
                        .desc
                        .entries
                        .iter()
                        .find(|entry| entry.binding == binding.binding)
                        .expect("binding validated before compact commit")
                        .binding_type;
                    write = write
                        .descriptor_type(resource_binding_type_to_vk(kind))
                        .buffer_info(std::slice::from_ref(&info));
                    unsafe {
                        self.device.update_descriptor_sets(&[write], &[]);
                    }
                }
                BindingResource::Texture(binding_resource) => {
                    let info = vk::DescriptorImageInfo::default()
                        .image_view(self.texture_views.get(binding_resource.texture_view)?.view)
                        .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL);
                    write = write
                        .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                        .image_info(std::slice::from_ref(&info));
                    unsafe {
                        self.device.update_descriptor_sets(&[write], &[]);
                    }
                }
                BindingResource::Sampler(_) => {}
            }
        }
        Ok(())
    }
}
