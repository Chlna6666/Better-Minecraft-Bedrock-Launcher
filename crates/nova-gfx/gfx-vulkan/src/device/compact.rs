//! Bounded maintenance at an idle GPU-owner boundary. Only existing holes are used.
use super::*;
mod allocation;
mod bindings;
use allocation::{allocate_buffer, allocate_texture, allocation_memory};

#[derive(Default)]
pub(super) struct HeapReplacement {
    buffers: Vec<(BufferId, VulkanBuffer)>,
    textures: Vec<(TextureId, VulkanTexture)>,
    views: Vec<(TextureViewId, VulkanTextureView)>,
}

impl VulkanDevice {
    pub(super) fn compact_heap(&mut self) -> Result<gfx_core::MemoryCompactReport> {
        self.poll_cleanup();
        let before = self.allocator.detailed_report();
        let mut report = gfx_core::MemoryCompactReport {
            reserved_before: before.reserved_bytes,
            reserved_peak: before.reserved_bytes,
            reserved_after: before.reserved_bytes,
            ..Default::default()
        };
        // Unsubmitted encoders retain native objects; never invalidate them.
        if self.command_encoders.live_len() != 0 {
            return Ok(report);
        }
        if self.allocator.vulkan_relocation_candidate().is_none()
            && self.deferred_destroys.is_empty()
        {
            return Ok(report);
        }
        self.wait_for_pending_work()?;
        // Cleanup can release candidate blocks. Select again using current allocation ownership.
        let Some(source) = self.allocator.vulkan_relocation_candidate() else {
            report.reserved_after = self.allocator.detailed_report().reserved_bytes;
            return Ok(report);
        };
        self.validate_compact_bindings()?;
        let mut replacement = HeapReplacement::default();
        match self.prepare_heap_replacement(source, &mut replacement) {
            Ok(true) => {}
            Ok(false) => {
                self.discard_heap_replacement(&mut replacement)?;
                report.reserved_after = self.allocator.detailed_report().reserved_bytes;
                return Ok(report);
            }
            Err(error) => {
                self.discard_heap_replacement(&mut replacement)?;
                return Err(error);
            }
        }
        report.reserved_peak = report
            .reserved_peak
            .max(self.allocator.detailed_report().reserved_bytes);
        if let Err(error) = self.copy_heap_contents(&mut replacement) {
            self.discard_heap_replacement(&mut replacement)?;
            return Err(error);
        }
        report.moved_resources = replacement.buffers.len() + replacement.textures.len();
        report.moved_bytes = replacement
            .buffers
            .iter()
            .map(|(_, buffer)| buffer.allocation.size())
            .sum::<u64>()
            + replacement
                .textures
                .iter()
                .map(|(_, texture)| texture.allocation.size())
                .sum::<u64>();
        self.commit_heap_replacement(&mut replacement)?;
        self.allocator.trim();
        report.reserved_after = self.allocator.detailed_report().reserved_bytes;
        Ok(report)
    }

    fn prepare_heap_replacement(
        &mut self,
        source: vk::DeviceMemory,
        replacement: &mut HeapReplacement,
    ) -> Result<bool> {
        for id in self.buffers.live_ids() {
            let old = self.buffers.get(id)?;
            if allocation_memory(&old.allocation)? != source {
                continue;
            }
            let Some(buffer) =
                allocate_buffer(&self.device, &mut self.allocator, &old.desc, source)?
            else {
                return Ok(false);
            };
            replacement.buffers.push((id, buffer));
        }
        for id in self.textures.live_ids() {
            let old = self.textures.get(id)?;
            if allocation_memory(&old.allocation)? != source {
                continue;
            }
            let Some(texture) =
                allocate_texture(&self.device, &mut self.allocator, &old.desc, source)?
            else {
                return Ok(false);
            };
            replacement.textures.push((id, texture));
        }
        // Upload pages are not stable public resources; defer a block containing one.
        if self
            .upload_pages
            .iter()
            .flatten()
            .any(|buffer| allocation_memory(&buffer.allocation).ok() == Some(source))
        {
            return Ok(false);
        }
        for id in self.texture_views.live_ids() {
            let old = self.texture_views.get(id)?;
            let Some((_, texture)) = replacement
                .textures
                .iter()
                .find(|(id, _)| *id == old.texture)
            else {
                continue;
            };
            let desc = &old.desc;
            let view = create_image_view_range(
                &self.device,
                texture.image,
                format_to_vk(desc.format),
                image_aspect_for_format(desc.format),
                desc.base_mip_level,
                desc.mip_level_count,
            )?;
            replacement.views.push((
                id,
                VulkanTextureView {
                    view,
                    texture: old.texture,
                    desc: desc.clone(),
                },
            ));
        }
        Ok(!replacement.buffers.is_empty() || !replacement.textures.is_empty())
    }

    pub(super) fn discard_heap_replacement(
        &mut self,
        replacement: &mut HeapReplacement,
    ) -> Result<()> {
        // SAFETY: No commands were submitted, or the maintenance copy has completed.
        unsafe {
            for (_, view) in replacement.views.drain(..) {
                self.device.destroy_image_view(view.view, None);
            }
            for (_, texture) in replacement.textures.drain(..) {
                self.device.destroy_image(texture.image, None);
                self.allocator.free(texture.allocation)?;
            }
            for (_, buffer) in replacement.buffers.drain(..) {
                self.device.destroy_buffer(buffer.buffer, None);
                self.allocator.free(buffer.allocation)?;
            }
        }
        Ok(())
    }

    fn copy_heap_contents(&mut self, replacement: &mut HeapReplacement) -> Result<()> {
        let upload = create_upload_commands(&self.device, self.graphics_queue_family_index, false)?;
        let commands = upload.command_buffer;
        if let Err(error) = begin_one_time_commands(&self.device, commands) {
            destroy_upload_commands(&self.device, &upload);
            return Err(error);
        }
        let barrier = vk::MemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::MEMORY_WRITE | vk::AccessFlags::HOST_WRITE)
            .dst_access_mask(vk::AccessFlags::TRANSFER_READ);
        // SAFETY: Previous submissions are complete, coherent mapped host writes are published,
        // and recording is exclusive to the owner. Every native buffer includes transfer usage.
        unsafe {
            self.device.cmd_pipeline_barrier(
                commands,
                vk::PipelineStageFlags::ALL_COMMANDS | vk::PipelineStageFlags::HOST,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[barrier],
                &[],
                &[],
            );
        }
        for (id, destination) in &replacement.buffers {
            let source = self.buffers.get(*id)?;
            let region = vk::BufferCopy::default().size(source.desc.size);
            unsafe {
                self.device
                    .cmd_copy_buffer(commands, source.buffer, destination.buffer, &[region]);
            }
        }
        for (id, destination) in &replacement.textures {
            self.record_heap_image_copy(commands, self.textures.get(*id)?, destination);
        }
        let barrier = vk::MemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(
                vk::AccessFlags::MEMORY_READ
                    | vk::AccessFlags::MEMORY_WRITE
                    | vk::AccessFlags::HOST_READ,
            );
        unsafe {
            self.device.cmd_pipeline_barrier(
                commands,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::ALL_COMMANDS | vk::PipelineStageFlags::HOST,
                vk::DependencyFlags::empty(),
                &[barrier],
                &[],
                &[],
            );
        }
        let fence = match end_submit_deferred(&self.device, self.graphics_queue, commands) {
            Ok(fence) => fence,
            Err(error) => {
                destroy_upload_commands(&self.device, &upload);
                return Err(error);
            }
        };
        // A failed wait cannot establish completion. Keep the command pool and all destination
        // allocations in the existing retirement queue instead of freeing them under the GPU.
        if let Err(error) = unsafe { self.device.wait_for_fences(&[fence], true, u64::MAX) } {
            self.deferred_destroys.retire(
                0,
                DeferredResource::HeapReplacement {
                    fence,
                    commands: upload,
                    replacement: std::mem::take(replacement),
                },
            );
            return Err(VulkanError::from(error).into());
        }
        destroy_fence_if_needed(&self.device, fence);
        destroy_upload_commands(&self.device, &upload);
        Ok(())
    }

    fn record_heap_image_copy(
        &self,
        commands: vk::CommandBuffer,
        source: &VulkanTexture,
        destination: &VulkanTexture,
    ) {
        if source.layout == vk::ImageLayout::UNDEFINED {
            return;
        }
        let aspect = image_aspect_for_format(source.desc.format);
        heap_image_barrier(
            &self.device,
            commands,
            source,
            source.layout,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
        );
        heap_image_barrier(
            &self.device,
            commands,
            destination,
            vk::ImageLayout::UNDEFINED,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        );
        for mip in 0..source.desc.mip_level_count {
            let layers = vk::ImageSubresourceLayers::default()
                .aspect_mask(aspect)
                .mip_level(mip)
                .layer_count(1);
            let region = vk::ImageCopy::default()
                .src_subresource(layers)
                .dst_subresource(layers)
                .extent(vk::Extent3D {
                    width: (source.desc.size.width() >> mip).max(1),
                    height: (source.desc.size.height() >> mip).max(1),
                    depth: 1,
                });
            // SAFETY: Identical mip extents/formats; resources were created with transfer usage.
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
        }
        heap_image_barrier(
            &self.device,
            commands,
            source,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            source.layout,
        );
        heap_image_barrier(
            &self.device,
            commands,
            destination,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            source.layout,
        );
    }

    fn commit_heap_replacement(&mut self, replacement: &mut HeapReplacement) -> Result<()> {
        let mut old_buffers = Vec::new();
        let mut old_textures = Vec::new();
        let mut old_views = Vec::new();
        for (id, buffer) in replacement.buffers.drain(..) {
            old_buffers.push(self.buffers.replace_live(id, buffer)?);
        }
        for (id, mut texture) in replacement.textures.drain(..) {
            texture.layout = self.textures.get(id)?.layout;
            old_textures.push(self.textures.replace_live(id, texture)?);
        }
        for (id, view) in replacement.views.drain(..) {
            old_views.push(self.texture_views.replace_live(id, view)?);
        }
        // All descriptor references were checked before changing any native object.
        for id in {
            let ids: Vec<ResourceSetId> = self.resource_sets.live_ids();
            ids
        } {
            self.rewrite_compact_set(id)?;
        }
        unsafe {
            for view in old_views {
                self.device.destroy_image_view(view.view, None);
            }
            for texture in old_textures {
                self.device.destroy_image(texture.image, None);
                self.allocator.free(texture.allocation)?;
            }
            for buffer in old_buffers {
                self.device.destroy_buffer(buffer.buffer, None);
                self.allocator.free(buffer.allocation)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

fn heap_image_barrier(
    device: &ash::Device,
    commands: vk::CommandBuffer,
    texture: &VulkanTexture,
    old: vk::ImageLayout,
    new: vk::ImageLayout,
) {
    let range = vk::ImageSubresourceRange::default()
        .aspect_mask(image_aspect_for_format(texture.desc.format))
        .level_count(texture.desc.mip_level_count)
        .layer_count(1);
    let barrier = vk::ImageMemoryBarrier::default()
        .image(texture.image)
        .old_layout(old)
        .new_layout(new)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .src_access_mask(if old == vk::ImageLayout::UNDEFINED {
            vk::AccessFlags::empty()
        } else {
            vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE
        })
        .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)
        .subresource_range(range);
    // SAFETY: Owner-maintenance commands reference live images, preserving format aspect and mips.
    unsafe {
        device.cmd_pipeline_barrier(
            commands,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        );
    }
}
