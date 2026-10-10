use super::*;

impl VulkanDevice {
    pub(super) fn copy_texture_regions(&mut self, copies: &[gfx_core::TextureCopy]) -> Result<()> {
        if copies.is_empty() {
            return Ok(());
        }
        for copy in copies {
            copy.validate(
                &self.textures.get(copy.source)?.desc,
                &self.textures.get(copy.destination)?.desc,
            )?;
            if self.textures.get(copy.source)?.layout == vk::ImageLayout::UNDEFINED {
                return Err(Error::InvalidInput(
                    "texture copy source has no initialized layout".into(),
                ));
            }
        }
        let commands =
            create_upload_commands(&self.device, self.graphics_queue_family_index, false)?;
        let record = self.record_texture_regions(commands.command_buffer, copies);
        if let Err(error) = record {
            destroy_upload_commands(&self.device, &commands);
            return Err(error);
        }
        let fence =
            match end_submit_deferred(&self.device, self.graphics_queue, commands.command_buffer) {
                Ok(fence) => fence,
                Err(error) => {
                    destroy_upload_commands(&self.device, &commands);
                    return Err(error);
                }
            };
        self.retire_deferred_upload(commands, fence);
        for copy in copies {
            let texture = self.textures.get_mut(copy.destination)?;
            texture.layout = copy_destination_layout(texture);
        }
        Ok(())
    }

    fn record_texture_regions(
        &self,
        commands: vk::CommandBuffer,
        copies: &[gfx_core::TextureCopy],
    ) -> Result<()> {
        begin_one_time_commands(&self.device, commands)?;
        let mut layouts = std::collections::HashMap::new();
        for copy in copies {
            let source = self.textures.get(copy.source)?;
            let destination = self.textures.get(copy.destination)?;
            let source_layout = layouts.get(&copy.source).copied().unwrap_or(source.layout);
            let destination_layout = layouts
                .get(&copy.destination)
                .copied()
                .unwrap_or(destination.layout);
            transition_image_layout_levels(
                &self.device,
                commands,
                source.image,
                source_layout,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                source.desc.mip_level_count,
            );
            transition_image_layout_levels(
                &self.device,
                commands,
                destination.image,
                destination_layout,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                destination.desc.mip_level_count,
            );
            let layers = vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .mip_level(0)
                .base_array_layer(0)
                .layer_count(1);
            let region = vk::ImageCopy::default()
                .src_subresource(layers)
                .dst_subresource(layers)
                .src_offset(vk::Offset3D {
                    x: copy.source_origin.x as i32,
                    y: copy.source_origin.y as i32,
                    z: 0,
                })
                .dst_offset(vk::Offset3D {
                    x: copy.destination_origin.x as i32,
                    y: copy.destination_origin.y as i32,
                    z: 0,
                })
                .extent(vk::Extent3D {
                    width: copy.size.width(),
                    height: copy.size.height(),
                    depth: 1,
                });
            // SAFETY: Recording on the owner queue after transitions; both rectangles are validated.
            unsafe {
                self.device.cmd_copy_image(
                    commands,
                    source.image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    destination.image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[region],
                );
            }
            transition_image_layout_levels(
                &self.device,
                commands,
                source.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                source_layout,
                source.desc.mip_level_count,
            );
            // An undefined destination must acquire a valid layout. Existing attachment/transfer
            // layouts are preserved, including copy-only textures without SAMPLED usage.
            let final_layout = copy_destination_layout(destination);
            transition_image_layout_levels(
                &self.device,
                commands,
                destination.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                final_layout,
                destination.desc.mip_level_count,
            );
            layouts.insert(copy.destination, final_layout);
        }
        Ok(())
    }
}

fn copy_destination_layout(texture: &VulkanTexture) -> vk::ImageLayout {
    if texture.layout != vk::ImageLayout::UNDEFINED {
        texture.layout
    } else if texture.desc.usage.contains(TextureUsage::SAMPLED) {
        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
    } else {
        vk::ImageLayout::TRANSFER_DST_OPTIMAL
    }
}
