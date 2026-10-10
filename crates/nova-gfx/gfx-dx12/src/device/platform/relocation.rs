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
