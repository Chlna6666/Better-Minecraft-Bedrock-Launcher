use crate::{
    device::{Dx11Device, backend, required},
    pipelines::Shader,
    resources::{Bound, BufferView},
};
use gfx_core::*;
use windows::Win32::Graphics::{Direct3D11::*, Dxgi::Common::*};

enum ComputeBinding {
    Read(Bound),
    Write(ID3D11UnorderedAccessView),
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::Interface;

    #[test]
    #[ignore = "requires Windows D3D11 hardware; verifies shared SRV identity and range invalidation"]
    fn graphics_and_compute_reuse_raw_views_without_losing_old_ranges() {
        let mut device = Dx11Device::new(&DeviceDescriptor::default()).unwrap();
        let buffer = device
            .create_buffer(&BufferDescriptor {
                label: None,
                size: 64,
                usage: BufferUsage::STORAGE,
                memory_location: MemoryLocation::GpuOnly,
            })
            .unwrap();
        let binding = BufferBinding {
            buffer,
            offset: 0,
            size: 32,
            stride: None,
        };
        let Bound::Srv(first) = device
            .bind_resource(
                &BindingResource::Buffer(binding),
                BindingType::StorageBuffer,
            )
            .unwrap()
        else {
            panic!("expected raw view");
        };
        let ComputeBinding::Read(Bound::Srv(compute)) = device
            .create_compute_binding(&ComputeBufferBinding {
                binding: 0,
                buffer: binding,
                access: ComputeBufferAccess::ReadOnly,
            })
            .unwrap()
        else {
            panic!("expected compute SRV");
        };
        assert_eq!(first.as_raw(), compute.as_raw());
        let other = BufferBinding {
            offset: 32,
            ..binding
        };
        let Bound::Srv(second) = device
            .bind_resource(&BindingResource::Buffer(other), BindingType::StorageBuffer)
            .unwrap()
        else {
            panic!("expected raw view");
        };
        assert_ne!(first.as_raw(), second.as_raw());
        let mut desc = D3D11_SHADER_RESOURCE_VIEW_DESC::default();
        // SAFETY: first retains the original native view after the cache range was replaced.
        unsafe {
            first.GetDesc(&mut desc);
        }
        // SAFETY: this view was created as BUFFEREX above.
        assert_eq!(unsafe { desc.Anonymous.BufferEx.FirstElement }, 0);
        assert!(
            device
                .bind_resource(
                    &BindingResource::Buffer(BufferBinding {
                        offset: 48,
                        ..binding
                    }),
                    BindingType::StorageBuffer
                )
                .is_err()
        );
    }
}

impl Dx11Device {
    pub(crate) fn dispatch_bind_group_buffers(
        &mut self,
        dispatch: &ComputeBindGroupDispatch,
    ) -> Result<()> {
        dispatch.validate(self.compute_capabilities())?;
        let set = self.sets.get(dispatch.bind_group)?;
        let layout = self.layouts.get(set.layout)?;
        set.desc.validate_against(layout)?;
        // Existing Section meshing remains on dispatch_buffers without new work.
        if layout.entries.iter().all(|entry| matches!(
            entry.binding_type,
            BindingType::UniformBuffer
                | BindingType::StorageBuffer
                | BindingType::StorageBufferReadWrite
        )) {
            let bindings = dispatch.buffer_bindings(&set.desc, layout)?;
            return self.dispatch_buffers(&ComputeDispatch {
                shader: dispatch.shader,
                bindings: &bindings,
                workgroups: dispatch.workgroups,
            });
        }

        let mut native = Vec::with_capacity(layout.entries.len());
        let mut buffer_bindings = Vec::new();
        let mut read_images = Vec::<TextureId>::new();
        let mut write_images = Vec::<TextureId>::new();
        for entry in &layout.entries {
            if !entry.stages.contains(ShaderStages::COMPUTE) {
                return Err(Error::InvalidInput("compute binding is not visible".into()));
            }
            let value = set.desc.entries.iter()
                .find(|item| item.binding == entry.binding)
                .ok_or_else(|| Error::InvalidInput("missing compute binding".into()))?;
            let view = match (entry.binding_type, value.resource) {
                (BindingType::UniformBuffer, BindingResource::Buffer(buffer))
                | (BindingType::StorageBuffer, BindingResource::Buffer(buffer))
                | (BindingType::StorageBufferReadWrite, BindingResource::Buffer(buffer)) => {
                    let access = match entry.binding_type {
                        BindingType::UniformBuffer => ComputeBufferAccess::Uniform,
                        BindingType::StorageBuffer => ComputeBufferAccess::ReadOnly,
                        _ => ComputeBufferAccess::ReadWrite,
                    };
                    let binding = ComputeBufferBinding {
                        binding: entry.binding, buffer, access,
                    };
                    let view = self.create_compute_binding(&binding)?;
                    buffer_bindings.push(binding);
                    view
                }
                (BindingType::SampledTexture, BindingResource::Texture(texture)) => {
                    if entry.binding >= 128 {
                        return Err(Error::InvalidInput("D3D11 sampled image slot exceeds t127".into()));
                    }
                    let view = self.views.get(texture.texture_view)?;
                    read_images.push(view.texture);
                    ComputeBinding::Read(Bound::Srv(view.srv.clone().ok_or_else(|| {
                        Error::InvalidInput("sampled compute image lacks SRV".into())
                    })?))
                }
                (BindingType::StorageTexture { format, .. }, BindingResource::Texture(texture)) => {
                    if entry.binding >= 8 {
                        return Err(Error::InvalidInput("D3D11 storage image slot exceeds u7".into()));
                    }
                    let view = self.views.get(texture.texture_view)?;
                    if view.format != format {
                        return Err(Error::InvalidInput("D3D11 storage image format mismatch".into()));
                    }
                    write_images.push(view.texture);
                    ComputeBinding::Write(view.uav.clone().ok_or_else(|| {
                        Error::InvalidInput("storage compute image lacks UAV".into())
                    })?)
                }
                (BindingType::Sampler, BindingResource::Sampler(sampler)) => {
                    if entry.binding >= 16 {
                        return Err(Error::InvalidInput("D3D11 sampler slot exceeds s15".into()));
                    }
                    ComputeBinding::Read(Bound::Sampler(
                        self.samplers.get(sampler.sampler)?.clone()
                    ))
                }
                _ => return Err(Error::InvalidInput("compute resource kind mismatch".into())),
            };
            native.push((entry.binding, view));
        }
        // A writable texture cannot concurrently be read (or written twice)
        // through a different image view of the same D3D11 texture.
        if read_images.iter().any(|id| write_images.contains(id))
            || write_images.iter().enumerate().any(|(i, id)| write_images[..i].contains(id))
        {
            return Err(Error::InvalidInput("compute image read/write alias".into()));
        }
        ComputeDispatch {
            shader: dispatch.shader,
            bindings: &buffer_bindings,
            workgroups: dispatch.workgroups,
        }.validate_with_limits(self.compute_capabilities(), self.buffer_binding_limits())?;
        let Shader::Compute(shader) = self.shaders.get(dispatch.shader)? else {
            return Err(Error::InvalidInput("dispatch requires a compute shader".into()));
        };
        self.clear_compute_graphics_srvs();
        if !write_images.is_empty() {
            // D3D11 refuses UAV bindings that overlap an active RTV/DSV.
            // Leave the buffer-only fast path untouched.
            unsafe { self.context.OMSetRenderTargets(None, None) };
        }
        for (slot, view) in &native {
            self.set_compute_binding(*slot, view);
        }
        // SAFETY: resources, shader and native slot limits were checked above.
        unsafe {
            let groups = [
                dispatch.workgroups[0], dispatch.workgroups[1], dispatch.workgroups[2], 0,
            ];
            self.context.UpdateSubresource(
                &self.draw_constants, 0, None, groups.as_ptr().cast(), 0, 0,
            );
            self.context.CSSetConstantBuffers(13, Some(&[Some(self.draw_constants.clone())]));
            self.context.CSSetShader(shader, None);
            self.context.Dispatch(
                dispatch.workgroups[0], dispatch.workgroups[1], dispatch.workgroups[2],
            );
            self.context.CSSetShader(None, None);
            self.context.CSSetConstantBuffers(13, Some(&[None]));
        }
        for (slot, view) in &native {
            self.clear_compute_binding(*slot, view);
        }
        Ok(())
    }

    pub(crate) fn dispatch_buffers(&mut self, dispatch: &ComputeDispatch<'_>) -> Result<()> {
        dispatch.validate_with_limits(self.compute_capabilities(), self.buffer_binding_limits())?;
        let Shader::Compute(shader) = self.shaders.get(dispatch.shader)? else {
            return Err(Error::InvalidInput(
                "dispatch requires a compute shader".into(),
            ));
        };
        let bindings = dispatch
            .bindings
            .iter()
            .map(|binding| {
                self.create_compute_binding(binding)
                    .map(|view| (binding.binding, view))
            })
            .collect::<Result<Vec<_>>>()?;
        if dispatch.workgroups.contains(&0) {
            return Ok(());
        }
        self.clear_compute_graphics_srvs();
        for (slot, view) in &bindings {
            self.set_compute_binding(*slot, view);
        }
        // SAFETY: All shader/resource handles, ranges and native slots were validated above.
        // Clear graphics SRVs before UAV writes to prevent read/write binding conflicts.
        // Immediate-context ordering plus UAV unbinding makes these writes visible to later
        // dispatches and render/indirect consumers without a CPU wait or a readback.
        unsafe {
            // Naga represents WGSL num_workgroups in the same reserved b13 ABI used
            // for graphics draw offsets. Populate it for every dispatch, including 2D grids.
            let groups = [
                dispatch.workgroups[0],
                dispatch.workgroups[1],
                dispatch.workgroups[2],
                0,
            ];
            self.context.UpdateSubresource(
                &self.draw_constants,
                0,
                None,
                groups.as_ptr().cast(),
                0,
                0,
            );
            self.context
                .CSSetConstantBuffers(13, Some(&[Some(self.draw_constants.clone())]));
            self.context.CSSetShader(shader, None);
            self.context.Dispatch(
                dispatch.workgroups[0],
                dispatch.workgroups[1],
                dispatch.workgroups[2],
            );
            self.context.CSSetShader(None, None);
            self.context.CSSetConstantBuffers(13, Some(&[None]));
        }
        for (slot, view) in &bindings {
            self.clear_compute_binding(*slot, view);
        }
        Ok(())
    }

    fn clear_compute_graphics_srvs(&self) {
        let none: [Option<ID3D11ShaderResourceView>; 128] = [const { None }; 128];
        for (slots, vertex) in [(&self.vs_srv_slots, true), (&self.ps_srv_slots, false)] {
            let occupied = slots.replace(0);
            if occupied == 0 {
                continue;
            }
            let first = occupied.trailing_zeros();
            let count = (128 - occupied.leading_zeros() - first) as usize;
            // SAFETY: The slot mask only records validated graphics SRV slots, and the
            // cleared span lies inside the native 128-slot register range.
            unsafe {
                if vertex {
                    self.context
                        .VSSetShaderResources(first, Some(&none[..count]));
                } else {
                    self.context
                        .PSSetShaderResources(first, Some(&none[..count]));
                }
            }
        }
    }

    fn set_compute_binding(&self, slot: u32, view: &ComputeBinding) {
        // SAFETY: create_compute_binding validated the native slot/range. The binding
        // owns the resource/view, and all temporary pointer arrays live through the call.
        unsafe {
            match view {
                ComputeBinding::Read(Bound::Srv(view)) => self
                    .context
                    .CSSetShaderResources(slot, Some(&[Some(view.clone())])),
                ComputeBinding::Read(Bound::Uniform {
                    buffer,
                    first,
                    count,
                }) => {
                    self.context.CSSetConstantBuffers1(
                        slot,
                        1,
                        Some([Some(buffer.clone())].as_ptr()),
                        Some(first),
                        Some(count),
                    );
                }
                ComputeBinding::Write(view) => {
                    self.context.CSSetUnorderedAccessViews(
                        slot,
                        1,
                        Some([Some(view.clone())].as_ptr()),
                        None,
                    );
                }
                ComputeBinding::Read(Bound::Sampler(sampler)) => {
                    self.context.CSSetSamplers(slot, Some(&[Some(sampler.clone())]));
                }
            }
        }
    }

    fn clear_compute_binding(&self, slot: u32, view: &ComputeBinding) {
        // SAFETY: Clear the same validated slot populated by set_compute_binding, after
        // dispatch. D3D11 retains submitted resource references until the GPU is finished.
        unsafe {
            match view {
                ComputeBinding::Read(Bound::Srv(_)) => {
                    self.context.CSSetShaderResources(slot, Some(&[None]))
                }
                ComputeBinding::Read(Bound::Uniform { .. }) => {
                    self.context.CSSetConstantBuffers(slot, Some(&[None]))
                }
                ComputeBinding::Read(Bound::Sampler(_)) => {
                    self.context.CSSetSamplers(slot, Some(&[None]))
                }
                ComputeBinding::Write(_) => {
                    self.context
                        .CSSetUnorderedAccessViews(slot, 1, Some([None].as_ptr()), None)
                }
            }
        }
    }

    fn create_compute_binding(&self, binding: &ComputeBufferBinding) -> Result<ComputeBinding> {
        let limit = match binding.access {
            ComputeBufferAccess::Uniform => 13,
            ComputeBufferAccess::ReadOnly => 128,
            ComputeBufferAccess::ReadWrite => 8,
        };
        if binding.binding >= limit {
            return Err(Error::InvalidInput(
                "compute binding exceeds D3D11 register limit".into(),
            ));
        }
        match binding.access {
            ComputeBufferAccess::ReadOnly => self
                .bind_resource(
                    &BindingResource::Buffer(binding.buffer),
                    BindingType::StorageBuffer,
                )
                .map(ComputeBinding::Read),
            ComputeBufferAccess::Uniform => self
                .bind_resource(
                    &BindingResource::Buffer(binding.buffer),
                    BindingType::UniformBuffer,
                )
                .map(ComputeBinding::Read),
            ComputeBufferAccess::ReadWrite => self
                .create_compute_uav(binding.buffer)
                .map(ComputeBinding::Write),
        }
    }

    pub(crate) fn create_compute_uav(&self, binding: BufferBinding) -> Result<ID3D11UnorderedAccessView> {
        let buffer = self.buffers.get(binding.buffer)?;
        if let Some(cached) = buffer
            .compute_uav
            .borrow()
            .as_ref()
            .filter(|view| view.offset == binding.offset && view.size == binding.size)
        {
            return Ok(cached.native.clone());
        }
        binding.validate_against(buffer.desc.size)?;
        if !buffer.desc.usage.contains(BufferUsage::STORAGE_WRITE)
            || binding.offset % 4 != 0
            || binding.size % 4 != 0
        {
            return Err(Error::InvalidInput(
                "compute UAV requires STORAGE_WRITE and an aligned range".into(),
            ));
        }
        let mut view = None;
        // SAFETY: Buffer usage permits raw UAVs and the aligned range is within the allocation.
        unsafe {
            self.native.CreateUnorderedAccessView(
                &buffer.native,
                Some(&D3D11_UNORDERED_ACCESS_VIEW_DESC {
                    Format: DXGI_FORMAT_R32_TYPELESS,
                    ViewDimension: D3D11_UAV_DIMENSION_BUFFER,
                    Anonymous: D3D11_UNORDERED_ACCESS_VIEW_DESC_0 {
                        Buffer: D3D11_BUFFER_UAV {
                            FirstElement: (binding.offset / 4) as u32,
                            NumElements: (binding.size / 4) as u32,
                            Flags: D3D11_BUFFER_UAV_FLAG_RAW.0 as u32,
                        },
                    },
                }),
                Some(&mut view),
            )
        }
        .map_err(backend)?;
        let view = required(view)?;
        *buffer.compute_uav.borrow_mut() = Some(BufferView {
            offset: binding.offset,
            size: binding.size,
            native: view.clone(),
        });
        Ok(view)
    }
}
