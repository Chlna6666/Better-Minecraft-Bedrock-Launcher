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
                image_aspect_for_format(source.desc.format),
            );
            transition_image_layout_levels(
                &self.device,
                commands,
                destination.image,
                destination_layout,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                destination.desc.mip_level_count,
                image_aspect_for_format(destination.desc.format),
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
                image_aspect_for_format(source.desc.format),
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
                image_aspect_for_format(destination.desc.format),
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

impl VulkanDevice {
    /// Queues buffer <-> texture copies with explicit image-layout transitions.
    pub(super) fn copy_texture_buffer_regions(
        &mut self,
        copies: &[gfx_core::TextureBufferCopy],
    ) -> Result<()> {
        if copies.is_empty() {
            return Ok(());
        }
        let limits = self.texture_copy_limits();
        for copy in copies {
            copy.validate(
                &self.textures.get(copy.texture)?.desc,
                &self.buffers.get(copy.buffer)?.desc,
                limits,
            )?;
        }
        let commands =
            create_upload_commands(&self.device, self.graphics_queue_family_index, false)?;
        let record = self.record_texture_buffer_regions(commands.command_buffer, copies, limits);
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
            let texture = self.textures.get_mut(copy.texture)?;
            texture.layout = copy_destination_layout(texture);
        }
        Ok(())
    }

    fn record_texture_buffer_regions(
        &self,
        commands: vk::CommandBuffer,
        copies: &[gfx_core::TextureBufferCopy],
        limits: gfx_core::TextureCopyLimits,
    ) -> Result<()> {
        begin_one_time_commands(&self.device, commands)?;
        let mut layouts = std::collections::HashMap::new();
        for copy in copies {
            let texture = self.textures.get(copy.texture)?;
            let buffer = self.buffers.get(copy.buffer)?;
            let layout = layouts
                .get(&copy.texture)
                .copied()
                .unwrap_or(texture.layout);
            let target = match copy.direction {
                gfx_core::CopyDirection::BufferToTexture => vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                gfx_core::CopyDirection::TextureToBuffer => vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            };
            transition_image_layout_levels(
                &self.device,
                commands,
                texture.image,
                layout,
                target,
                texture.desc.mip_level_count,
                image_aspect_for_format(texture.desc.format),
            );
            let bytes_per_texel = u64::from(texture.desc.format.bytes_per_pixel()).max(1);
            let row_pitch =
                gfx_core::texture_copy_row_pitch(copy.size.width(), texture.desc.format, limits);
            let region = vk::BufferImageCopy::default()
                .buffer_offset(copy.buffer_offset)
                // 0 means tightly packed to imageExtent.width; the buffer is sized with the
                // same device pitch, so Vulkan needs no explicit row length.
                .buffer_row_length(0)
                .buffer_image_height(0)
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .mip_level(0)
                        .base_array_layer(0)
                        .layer_count(1),
                )
                .image_offset(vk::Offset3D {
                    x: i32::try_from(copy.texture_origin.x).unwrap_or(i32::MAX),
                    y: i32::try_from(copy.texture_origin.y).unwrap_or(i32::MAX),
                    z: 0,
                })
                .image_extent(vk::Extent3D {
                    width: copy.size.width(),
                    height: copy.size.height(),
                    depth: 1,
                });
            // SAFETY: Both handles are live, and the barriers above put them in transfer layouts.
            unsafe {
                match copy.direction {
                    gfx_core::CopyDirection::BufferToTexture => {
                        self.device.cmd_copy_buffer_to_image(
                            commands,
                            buffer.buffer,
                            texture.image,
                            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                            &[region],
                        );
                    }
                    gfx_core::CopyDirection::TextureToBuffer => {
                        self.device.cmd_copy_image_to_buffer(
                            commands,
                            texture.image,
                            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                            buffer.buffer,
                            &[region],
                        );
                    }
                }
            }
            // Leave the image in a layout later passes can bind, mirroring texture copies.
            let final_layout = copy_destination_layout(texture);
            transition_image_layout_levels(
                &self.device,
                commands,
                texture.image,
                target,
                final_layout,
                texture.desc.mip_level_count,
                image_aspect_for_format(texture.desc.format),
            );
            // A later copy in this command buffer may read the buffer this one wrote;
            // transfer blocks are not ordered without an explicit memory barrier.
            let barrier = vk::MemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::TRANSFER_READ | vk::AccessFlags::TRANSFER_WRITE);
            // SAFETY: Recording on the owning queue; the barrier only orders transfer writes.
            unsafe {
                self.device.cmd_pipeline_barrier(
                    commands,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::PipelineStageFlags::TRANSFER,
                    vk::DependencyFlags::empty(),
                    &[barrier],
                    &[],
                    &[],
                );
            }
            layouts.insert(copy.texture, final_layout);
        }
        Ok(())
    }
}
