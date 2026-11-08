use super::*;

impl Dx12Device {
    pub(super) fn copy_texture_regions(&mut self, copies: &[gfx_core::TextureCopy]) -> Result<()> {
        if copies.is_empty() {
            return Ok(());
        }
        for copy in copies {
            let source = self.textures.get(copy.source)?;
            let destination = self.textures.get(copy.destination)?;
            copy.validate(&source.desc, &destination.desc)?;
            if source.resource.is_none() || destination.resource.is_none() {
                return Err(Error::Backend("texture copy has no native resource".into()));
            }
        }
        let commands = create_dx12_upload_commands(&self.device, false)?;
        let list = &commands.graphics_command_list;
        let mut states = std::collections::HashMap::new();
        for copy in copies {
            self.record_texture_region(list, copy, &mut states)?;
        }
        // SAFETY: All copies were recorded against live resources on the owner thread.
        unsafe { list.Close() }.map_err(|error| Error::Backend(error.to_string()))?;
        let executable: ID3D12CommandList = list
            .cast()
            .map_err(|error| Error::Backend(error.to_string()))?;
        // SAFETY: Closed command list from this device, on its graphics queue.
        unsafe {
            self.graphics_queue
                .ExecuteCommandLists(&[Some(executable.clone())]);
        }
        let fence = self.signal_frame()?;
        self.pending_texture_uploads.retire(
            fence,
            Dx12SubmittedCommandList {
                commands,
                _command_list: executable,
                gpu_work_tag: None,
            },
        );
        for (id, state) in states {
            self.textures.get_mut(id)?.state = state;
        }
        Ok(())
    }

    fn record_texture_region(
        &self,
        list: &ID3D12GraphicsCommandList,
        copy: &gfx_core::TextureCopy,
        states: &mut std::collections::HashMap<TextureId, D3D12_RESOURCE_STATES>,
    ) -> Result<()> {
        let source = self.textures.get(copy.source)?;
        let destination = self.textures.get(copy.destination)?;
        let source_native = source
            .resource
            .as_ref()
            .ok_or_else(|| Error::Backend("copy source has no native resource".into()))?;
        let destination_native = destination
            .resource
            .as_ref()
            .ok_or_else(|| Error::Backend("copy destination has no native resource".into()))?;
        let source_state = states.get(&copy.source).copied().unwrap_or(source.state);
        let destination_state = states
            .get(&copy.destination)
            .copied()
            .unwrap_or(destination.state);
        if source_state != D3D12_RESOURCE_STATE_COPY_SOURCE {
            record_transition_barrier(
                list,
                source_native,
                source_state,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            );
        }
        if destination_state != D3D12_RESOURCE_STATE_COPY_DEST {
            record_transition_barrier(
                list,
                destination_native,
                destination_state,
                D3D12_RESOURCE_STATE_COPY_DEST,
            );
        }
        let mut source_location = texture_location(source_native);
        let mut destination_location = texture_location(destination_native);
        let region = windows::Win32::Graphics::Direct3D12::D3D12_BOX {
            left: copy.source_origin.x,
            top: copy.source_origin.y,
            front: 0,
            right: copy.source_origin.x + copy.size.width(),
            bottom: copy.source_origin.y + copy.size.height(),
            back: 1,
        };
        // SAFETY: Formats and rectangles validated; resource states permit texture region copying.
        unsafe {
            list.CopyTextureRegion(
                &destination_location,
                copy.destination_origin.x,
                copy.destination_origin.y,
                0,
                &source_location,
                Some(&region),
            );
        }
        release_texture_copy_location_resource(&mut source_location);
        release_texture_copy_location_resource(&mut destination_location);
        if source_state != D3D12_RESOURCE_STATE_COPY_SOURCE {
            record_transition_barrier(
                list,
                source_native,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
                source_state,
            );
        }
        let final_state = if destination_state == D3D12_RESOURCE_STATE_COPY_DEST
            && destination.desc.usage.contains(TextureUsage::SAMPLED)
        {
            D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE
        } else {
            destination_state
        };
        if final_state != D3D12_RESOURCE_STATE_COPY_DEST {
            record_transition_barrier(
                list,
                destination_native,
                D3D12_RESOURCE_STATE_COPY_DEST,
                final_state,
            );
        }
        states.insert(copy.destination, final_state);
        Ok(())
    }
}

fn texture_location(resource: &ID3D12Resource) -> D3D12_TEXTURE_COPY_LOCATION {
    D3D12_TEXTURE_COPY_LOCATION {
        pResource: core::mem::ManuallyDrop::new(Some(resource.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: 0,
        },
    }
}

impl Dx12Device {
    /// Queues buffer <-> texture copies using placed footprints.
    pub(super) fn copy_texture_buffer_regions(
        &mut self,
        copies: &[gfx_core::TextureBufferCopy],
    ) -> Result<()> {
        if copies.is_empty() {
            return Ok(());
        }
        let limits = self.texture_copy_limits();
        for copy in copies {
            let texture = self.textures.get(copy.texture)?;
            let buffer = self.buffers.get(copy.buffer)?;
            copy.validate(&texture.desc, &buffer.desc, limits)?;
            if texture.resource.is_none() || buffer.resource.is_none() {
                return Err(Error::Backend(
                    "buffer texture copy has no native resource".into(),
                ));
            }
        }
        let commands = create_dx12_upload_commands(&self.device, false)?;
        let list = &commands.graphics_command_list;
        let mut texture_states = std::collections::HashMap::new();
        let mut buffer_states = std::collections::HashMap::new();
        for copy in copies {
            self.record_texture_buffer_region(
                list,
                copy,
                limits,
                &mut texture_states,
                &mut buffer_states,
            )?;
        }
        // SAFETY: All copies were recorded against live resources on the owner thread.
        unsafe { list.Close() }.map_err(|error| Error::Backend(error.to_string()))?;
        let executable: ID3D12CommandList = list
            .cast()
            .map_err(|error| Error::Backend(error.to_string()))?;
        // SAFETY: Closed command list from this device, on its graphics queue.
        unsafe {
            self.graphics_queue
                .ExecuteCommandLists(&[Some(executable.clone())]);
        }
        let fence = self.signal_frame()?;
        self.pending_texture_uploads.retire(
            fence,
            Dx12SubmittedCommandList {
                commands,
                _command_list: executable,
                gpu_work_tag: None,
            },
        );
        for (id, state) in texture_states {
            self.textures.get_mut(id)?.state = state;
        }
        for (id, state) in buffer_states {
            self.buffers.get_mut(id)?.state = state;
        }
        Ok(())
    }

    fn record_texture_buffer_region(
        &self,
        list: &ID3D12GraphicsCommandList,
        copy: &gfx_core::TextureBufferCopy,
        limits: gfx_core::TextureCopyLimits,
        texture_states: &mut std::collections::HashMap<TextureId, D3D12_RESOURCE_STATES>,
        buffer_states: &mut std::collections::HashMap<BufferId, D3D12_RESOURCE_STATES>,
    ) -> Result<()> {
        let texture = self.textures.get(copy.texture)?;
        let buffer = self.buffers.get(copy.buffer)?;
        let texture_native = texture
            .resource
            .as_ref()
            .ok_or_else(|| Error::Backend("copy texture has no native resource".into()))?;
        let buffer_native = buffer
            .resource
            .as_ref()
            .ok_or_else(|| Error::Backend("copy buffer has no native resource".into()))?;
        let (texture_target, buffer_target) = match copy.direction {
            gfx_core::CopyDirection::BufferToTexture => (
                D3D12_RESOURCE_STATE_COPY_DEST,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            ),
            gfx_core::CopyDirection::TextureToBuffer => (
                D3D12_RESOURCE_STATE_COPY_SOURCE,
                D3D12_RESOURCE_STATE_COPY_DEST,
            ),
        };
        let texture_state = texture_states
            .get(&copy.texture)
            .copied()
            .unwrap_or(texture.state);
        let buffer_state = buffer_states
            .get(&copy.buffer)
            .copied()
            .unwrap_or(buffer.state);
        if texture_state != texture_target {
            record_transition_barrier(list, texture_native, texture_state, texture_target);
        }
        // Readback-heap buffers must stay a copy destination; only transition otherwise.
        if buffer_state != buffer_target && buffer_target != D3D12_RESOURCE_STATE_COPY_DEST {
            record_transition_barrier(list, buffer_native, buffer_state, buffer_target);
        }
        let row_pitch =
            gfx_core::texture_copy_row_pitch(copy.size.width(), texture.desc.format, limits);
        let mut texture_location = texture_location(texture_native);
        let mut buffer_location = D3D12_TEXTURE_COPY_LOCATION {
            pResource: core::mem::ManuallyDrop::new(Some(buffer_native.clone())),
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint:
                    windows::Win32::Graphics::Direct3D12::D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                        Offset: copy.buffer_offset,
                        Footprint:
                            windows::Win32::Graphics::Direct3D12::D3D12_SUBRESOURCE_FOOTPRINT {
                                Format: format_to_dxgi(texture.desc.format),
                                Width: copy.size.width(),
                                Height: copy.size.height(),
                                Depth: 1,
                                RowPitch: u32::try_from(row_pitch).map_err(|_| {
                                    Error::InvalidInput("DX12 copy row pitch exceeds u32".into())
                                })?,
                            },
                    },
            },
        };
        let width = copy.size.width();
        let height = copy.size.height();
        // A texture source is addressed by its rectangle; a buffer source starts at its origin.
        let source_box = windows::Win32::Graphics::Direct3D12::D3D12_BOX {
            left: copy.texture_origin.x,
            top: copy.texture_origin.y,
            front: 0,
            right: copy.texture_origin.x.saturating_add(width),
            bottom: copy.texture_origin.y.saturating_add(height),
            back: 1,
        };
        let buffer_box = windows::Win32::Graphics::Direct3D12::D3D12_BOX {
            left: 0,
            top: 0,
            front: 0,
            right: width,
            bottom: height,
            back: 1,
        };
        // SAFETY: Both resources are live and in copy states; the footprint covers the range.
        unsafe {
            match copy.direction {
                gfx_core::CopyDirection::BufferToTexture => list.CopyTextureRegion(
                    &texture_location,
                    copy.texture_origin.x,
                    copy.texture_origin.y,
                    0,
                    &buffer_location,
                    Some(&buffer_box),
                ),
                gfx_core::CopyDirection::TextureToBuffer => list.CopyTextureRegion(
                    &buffer_location,
                    0,
                    0,
                    0,
                    &texture_location,
                    Some(&source_box),
                ),
            }
        }
        release_texture_copy_location_resource(&mut texture_location);
        release_texture_copy_location_resource(&mut buffer_location);
        if texture_state != texture_target {
            record_transition_barrier(list, texture_native, texture_target, texture_state);
        }
        let final_state = if copy.direction == gfx_core::CopyDirection::BufferToTexture
            && texture_state == D3D12_RESOURCE_STATE_COPY_DEST
            && texture.desc.usage.contains(TextureUsage::SAMPLED)
        {
            D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE
        } else {
            texture_state
        };
        if final_state != texture_state && final_state != texture_target {
            record_transition_barrier(list, texture_native, texture_state, final_state);
        }
        texture_states.insert(copy.texture, final_state);
        buffer_states.insert(copy.buffer, buffer_state);
        Ok(())
    }
}
