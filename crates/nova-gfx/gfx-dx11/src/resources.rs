use crate::device::{Dx11Device, backend, required};
use gfx_core::*;
use windows::Win32::Graphics::Direct3D::{D3D_SRV_DIMENSION_BUFFEREX, D3D_SRV_DIMENSION_TEXTURE2D};
use windows::Win32::Graphics::{Direct3D11::*, Dxgi::Common::*};

mod buffer_upload;

pub(crate) struct Buffer {
    pub(crate) native: ID3D11Buffer,
    pub(crate) desc: BufferDescriptor,
    pub(crate) native_size: u32,
    uniform_bytes: Option<Vec<u8>>,
}
impl Buffer {
    pub(crate) fn cpu_shadow_bytes(&self) -> u64 {
        self.uniform_bytes
            .as_ref()
            .map_or(0, |bytes| bytes.capacity() as u64)
    }
}
pub(crate) struct Texture {
    pub(crate) native: ID3D11Texture2D,
    pub(crate) desc: TextureDescriptor,
}
impl Texture {
    pub(crate) fn bytes(&self) -> u64 {
        (0..self.desc.mip_level_count)
            .map(|level| {
                u64::from((self.desc.size.width() >> level).max(1))
                    * u64::from((self.desc.size.height() >> level).max(1))
                    * u64::from(self.desc.format.bytes_per_pixel())
            })
            .sum()
    }
}
pub(crate) struct View {
    pub(crate) srv: Option<ID3D11ShaderResourceView>,
    pub(crate) rtv: Option<ID3D11RenderTargetView>,
    pub(crate) dsv: Option<ID3D11DepthStencilView>,
    pub(crate) size: Extent2d,
    pub(crate) format: Format,
}
pub(crate) enum Bound {
    Uniform {
        buffer: ID3D11Buffer,
        first: u32,
        count: u32,
    },
    Srv(ID3D11ShaderResourceView),
    Sampler(ID3D11SamplerState),
}
pub(crate) struct ResourceSet {
    pub(crate) layout: ResourceSetLayoutId,
    pub(crate) bindings: Vec<(u32, ShaderStages, Bound)>,
}

pub(crate) fn format(value: Format) -> DXGI_FORMAT {
    match value {
        Format::R8Unorm => DXGI_FORMAT_R8_UNORM,
        Format::Bgra8Unorm => DXGI_FORMAT_B8G8R8A8_UNORM,
        Format::Bgra8UnormSrgb => DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
        Format::Rgba8Unorm => DXGI_FORMAT_R8G8B8A8_UNORM,
        Format::Rgba8UnormSrgb => DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
        Format::Depth32Float => DXGI_FORMAT_D32_FLOAT,
    }
}

impl ResourceDevice for Dx11Device {
    fn copy_texture_batch(&mut self, copies: &[TextureCopy]) -> Result<()> {
        for copy in copies {
            copy.validate(
                &self.textures.get(copy.source)?.desc,
                &self.textures.get(copy.destination)?.desc,
            )?;
        }
        for copy in copies {
            let source = &self.textures.get(copy.source)?.native;
            let destination = &self.textures.get(copy.destination)?.native;
            let region = D3D11_BOX {
                left: copy.source_origin.x,
                top: copy.source_origin.y,
                front: 0,
                right: copy.source_origin.x + copy.size.width(),
                bottom: copy.source_origin.y + copy.size.height(),
                back: 1,
            };
            // SAFETY: Both same-format resources and rectangles were validated above. The owner
            // immediate context orders this copy after previous draws and before subsequent draws.
            unsafe {
                self.context.CopySubresourceRegion(
                    destination,
                    0,
                    copy.destination_origin.x,
                    copy.destination_origin.y,
                    0,
                    source,
                    0,
                    Some(&region),
                );
            }
        }
        Ok(())
    }
    fn create_buffer(&mut self, desc: &BufferDescriptor) -> Result<BufferId> {
        desc.validate()?;
        let uniform = desc.usage.contains(BufferUsage::UNIFORM);
        if desc.memory_location == MemoryLocation::GpuToCpu
            || (uniform
                && desc
                    .usage
                    .intersects(BufferUsage::STORAGE | BufferUsage::INDEX | BufferUsage::VERTEX))
        {
            return Err(Error::InvalidInput(
                "unsupported D3D11 buffer usage/placement".into(),
            ));
        }
        let alignment = if uniform { 256 } else { 4 };
        let native_size = u32::try_from(
            desc.size
                .checked_add(alignment - 1)
                .ok_or_else(|| Error::InvalidInput("buffer size overflow".into()))?
                / alignment
                * alignment,
        )
        .map_err(|_| Error::InvalidInput("D3D11 buffer exceeds 32-bit size".into()))?;
        if uniform && native_size > 65536 {
            return Err(Error::InvalidInput(
                "D3D11 uniform buffer exceeds 64KiB".into(),
            ));
        }
        let mut bind = 0;
        if uniform {
            bind |= D3D11_BIND_CONSTANT_BUFFER.0 as u32;
        }
        if desc.usage.contains(BufferUsage::STORAGE) {
            bind |= D3D11_BIND_SHADER_RESOURCE.0 as u32;
        }
        if desc.usage.contains(BufferUsage::INDEX) {
            bind |= D3D11_BIND_INDEX_BUFFER.0 as u32;
        }
        if desc.usage.contains(BufferUsage::VERTEX) {
            bind |= D3D11_BIND_VERTEX_BUFFER.0 as u32;
        }
        let misc = if desc.usage.contains(BufferUsage::STORAGE) {
            D3D11_RESOURCE_MISC_BUFFER_ALLOW_RAW_VIEWS.0 as u32
        } else {
            0
        };
        let mut native = None;
        // SAFETY: valid fully initialized descriptor and writable owned output.
        unsafe {
            self.native.CreateBuffer(
                &D3D11_BUFFER_DESC {
                    ByteWidth: native_size,
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: bind,
                    MiscFlags: misc,
                    ..Default::default()
                },
                None,
                Some(&mut native),
            )
        }
        .map_err(backend)?;
        Ok(self.buffers.insert(Buffer {
            native: required(native)?,
            desc: desc.clone(),
            native_size,
            uniform_bytes: uniform.then(|| vec![0; native_size as usize]),
        }))
    }
    fn write_buffer(&mut self, id: BufferId, offset: u64, data: &[u8]) -> Result<()> {
        let buffer = self.buffers.get_mut(id)?;
        if offset
            .checked_add(data.len() as u64)
            .is_none_or(|end| end > buffer.desc.size)
        {
            return Err(Error::InvalidInput(
                "D3D11 buffer write out of range".into(),
            ));
        }
        if data.is_empty() {
            return Ok(());
        }
        // SAFETY: source bytes cover the validated destination range. D3D11 copies the
        // data before returning and orders this update before subsequent immediate draws.
        unsafe {
            if let Some(bytes) = &mut buffer.uniform_bytes {
                let offset = offset as usize;
                bytes[offset..offset + data.len()].copy_from_slice(data);
                self.context.UpdateSubresource(
                    &buffer.native,
                    0,
                    None,
                    bytes.as_ptr().cast(),
                    0,
                    0,
                );
            } else {
                let region = D3D11_BOX {
                    left: offset as u32,
                    right: (offset + data.len() as u64) as u32,
                    top: 0,
                    bottom: 1,
                    front: 0,
                    back: 1,
                };
                self.context.UpdateSubresource(
                    &buffer.native,
                    0,
                    Some(&region),
                    data.as_ptr().cast(),
                    0,
                    0,
                );
            }
        }
        Ok(())
    }
    fn write_buffer_batch<'a>(
        &mut self,
        writes: impl IntoIterator<Item = BufferWrite<'a>>,
    ) -> Result<BufferUploadStats> {
        self.upload_buffer_batch(writes)
    }
    fn create_texture(&mut self, desc: &TextureDescriptor) -> Result<TextureId> {
        desc.validate()?;
        if desc.memory_location == MemoryLocation::GpuToCpu {
            return Err(Error::InvalidInput(
                "use read_texture for D3D11 readback".into(),
            ));
        }
        let mut bind = 0;
        if desc.usage.contains(TextureUsage::SAMPLED) {
            bind |= D3D11_BIND_SHADER_RESOURCE.0 as u32;
        }
        if desc.usage.contains(TextureUsage::COLOR_ATTACHMENT) {
            bind |= D3D11_BIND_RENDER_TARGET.0 as u32;
        }
        if desc.usage.contains(TextureUsage::DEPTH_ATTACHMENT) {
            bind |= D3D11_BIND_DEPTH_STENCIL.0 as u32;
        }
        if desc.format == Format::Depth32Float && desc.usage.contains(TextureUsage::SAMPLED) {
            return Err(Error::Unavailable(
                "D3D11 sampled depth textures require typeless storage".into(),
            ));
        }
        let mut native = None;
        // SAFETY: initialized texture descriptor, no borrowed initial data.
        unsafe {
            self.native.CreateTexture2D(
                &D3D11_TEXTURE2D_DESC {
                    Width: desc.size.width(),
                    Height: desc.size.height(),
                    MipLevels: desc.mip_level_count,
                    ArraySize: 1,
                    Format: format(desc.format),
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: bind,
                    ..Default::default()
                },
                None,
                Some(&mut native),
            )
        }
        .map_err(backend)?;
        Ok(self.textures.insert(Texture {
            native: required(native)?,
            desc: desc.clone(),
        }))
    }
    fn write_texture(&mut self, desc: TextureWriteDescriptor, data: &[u8]) -> Result<()> {
        let texture = self.textures.get(desc.texture)?;
        desc.validate_against(&texture.desc, data.len())?;
        if !texture.desc.usage.contains(TextureUsage::COPY_DST) {
            return Err(Error::InvalidInput("texture lacks COPY_DST".into()));
        }
        let region = D3D11_BOX {
            left: desc.origin.x,
            top: desc.origin.y,
            right: desc.origin.x + desc.size.width(),
            bottom: desc.origin.y + desc.size.height(),
            front: 0,
            back: 1,
        };
        // SAFETY: descriptor validation covers pitch, byte offset and complete source rows.
        unsafe {
            self.context.UpdateSubresource(
                &texture.native,
                desc.mip_level,
                Some(&region),
                data.as_ptr().add(desc.layout.offset as usize).cast(),
                desc.layout.bytes_per_row.get(),
                0,
            )
        };
        Ok(())
    }
    fn create_texture_view(&mut self, desc: &TextureViewDescriptor) -> Result<TextureViewId> {
        let texture = self.textures.get(desc.texture)?;
        desc.validate_against(&texture.desc)?;
        if desc.format != texture.desc.format {
            return Err(Error::InvalidInput(
                "D3D11 view format must match storage".into(),
            ));
        }
        let mut view = View {
            srv: None,
            rtv: None,
            dsv: None,
            size: texture.desc.mip_extent(desc.base_mip_level)?,
            format: desc.format,
        };
        // SAFETY: the texture is live; each view descriptor selects a validated mip range.
        unsafe {
            if texture.desc.usage.contains(TextureUsage::SAMPLED) {
                self.native
                    .CreateShaderResourceView(
                        &texture.native,
                        Some(&D3D11_SHADER_RESOURCE_VIEW_DESC {
                            Format: format(desc.format),
                            ViewDimension: D3D_SRV_DIMENSION_TEXTURE2D,
                            Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                                Texture2D: D3D11_TEX2D_SRV {
                                    MostDetailedMip: desc.base_mip_level,
                                    MipLevels: desc.mip_level_count,
                                },
                            },
                        }),
                        Some(&mut view.srv),
                    )
                    .map_err(backend)?;
            }
            if texture
                .desc
                .usage
                .intersects(TextureUsage::COLOR_ATTACHMENT | TextureUsage::DEPTH_ATTACHMENT)
            {
                if desc.mip_level_count != 1 {
                    return Err(Error::InvalidInput(
                        "attachment view must select one mip".into(),
                    ));
                }
                if desc.format == Format::Depth32Float {
                    self.native
                        .CreateDepthStencilView(
                            &texture.native,
                            Some(&D3D11_DEPTH_STENCIL_VIEW_DESC {
                                Format: format(desc.format),
                                ViewDimension: D3D11_DSV_DIMENSION_TEXTURE2D,
                                Flags: 0,
                                Anonymous: D3D11_DEPTH_STENCIL_VIEW_DESC_0 {
                                    Texture2D: D3D11_TEX2D_DSV {
                                        MipSlice: desc.base_mip_level,
                                    },
                                },
                            }),
                            Some(&mut view.dsv),
                        )
                        .map_err(backend)?;
                } else {
                    self.native
                        .CreateRenderTargetView(
                            &texture.native,
                            Some(&D3D11_RENDER_TARGET_VIEW_DESC {
                                Format: format(desc.format),
                                ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2D,
                                Anonymous: D3D11_RENDER_TARGET_VIEW_DESC_0 {
                                    Texture2D: D3D11_TEX2D_RTV {
                                        MipSlice: desc.base_mip_level,
                                    },
                                },
                            }),
                            Some(&mut view.rtv),
                        )
                        .map_err(backend)?;
                }
            }
        }
        Ok(self.views.insert(view))
    }
    fn create_sampler(&mut self, desc: &SamplerDescriptor) -> Result<SamplerId> {
        let address = |value| match value {
            AddressMode::ClampToEdge => D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressMode::Repeat => D3D11_TEXTURE_ADDRESS_WRAP,
        };
        let filter = if desc.anisotropic {
            D3D11_FILTER_ANISOTROPIC
        } else {
            D3D11_FILTER(
                (if desc.min_filter == FilterMode::Linear {
                    0x10
                } else {
                    0
                }) | (if desc.mag_filter == FilterMode::Linear {
                    0x4
                } else {
                    0
                }) | (if desc.mipmap_filter == FilterMode::Linear {
                    0x1
                } else {
                    0
                }),
            )
        };
        let mut sampler = None;
        // SAFETY: initialized sampler descriptor and owned output.
        unsafe {
            self.native.CreateSamplerState(
                &D3D11_SAMPLER_DESC {
                    Filter: filter,
                    AddressU: address(desc.address_mode_u),
                    AddressV: address(desc.address_mode_v),
                    AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                    MaxAnisotropy: if desc.anisotropic { 16 } else { 1 },
                    ComparisonFunc: D3D11_COMPARISON_NEVER,
                    MinLOD: 0.0,
                    MaxLOD: f32::MAX,
                    ..Default::default()
                },
                Some(&mut sampler),
            )
        }
        .map_err(backend)?;
        Ok(self.samplers.insert(required(sampler)?))
    }
    fn create_resource_set_layout(
        &mut self,
        desc: &ResourceSetLayoutDescriptor,
    ) -> Result<ResourceSetLayoutId> {
        desc.validate()?;
        for entry in &desc.entries {
            let limit = match entry.binding_type {
                ResourceBindingType::UniformBuffer => 13,
                ResourceBindingType::Sampler => 16,
                ResourceBindingType::StorageBuffer | ResourceBindingType::SampledTexture => 128,
            };
            if entry.binding >= limit {
                return Err(Error::InvalidInput(
                    "D3D11 register slot exceeds native limit (b13 reserved)".into(),
                ));
            }
        }
        Ok(self.layouts.insert(desc.clone()))
    }
    fn create_resource_set(&mut self, desc: &ResourceSetDescriptor) -> Result<ResourceSetId> {
        let layout = self.layouts.get(desc.layout)?;
        desc.validate_against(layout)?;
        let bindings = desc
            .bindings
            .iter()
            .map(|binding| {
                let entry = layout
                    .entries
                    .iter()
                    .find(|entry| entry.binding == binding.binding)
                    .ok_or_else(|| Error::InvalidInput("missing binding".into()))?;
                Ok((
                    binding.binding,
                    entry.stages,
                    self.bind_resource(&binding.resource, entry.binding_type)?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(self.sets.insert(ResourceSet {
            layout: desc.layout,
            bindings,
        }))
    }
    fn destroy_buffer(&mut self, id: BufferId) -> Result<()> {
        self.buffers.take(id)?;
        Ok(())
    }
    fn destroy_texture(&mut self, id: TextureId) -> Result<()> {
        self.textures.take(id)?;
        Ok(())
    }
    fn destroy_texture_view(&mut self, id: TextureViewId) -> Result<()> {
        self.views.take(id)?;
        Ok(())
    }
    fn destroy_sampler(&mut self, id: SamplerId) -> Result<()> {
        self.samplers.take(id)?;
        Ok(())
    }
    fn destroy_resource_set_layout(&mut self, id: ResourceSetLayoutId) -> Result<()> {
        self.layouts.take(id)?;
        Ok(())
    }
    fn destroy_resource_set(&mut self, id: ResourceSetId) -> Result<()> {
        self.sets.take(id)?;
        Ok(())
    }
}

impl Dx11Device {
    fn bind_resource(
        &self,
        resource: &BindingResource,
        kind: ResourceBindingType,
    ) -> Result<Bound> {
        match resource {
            BindingResource::Buffer(binding) => {
                let buffer = self.buffers.get(binding.buffer)?;
                binding.validate_against(buffer.desc.size)?;
                if kind == ResourceBindingType::UniformBuffer {
                    if !buffer.desc.usage.contains(BufferUsage::UNIFORM)
                        || binding.offset % 256 != 0
                    {
                        return Err(Error::InvalidInput(
                            "D3D11 constant range requires UNIFORM and 256-byte aligned offset"
                                .into(),
                        ));
                    }
                    Ok(Bound::Uniform {
                        buffer: buffer.native.clone(),
                        first: (binding.offset / 16) as u32,
                        count: binding.size.div_ceil(256) as u32 * 16,
                    })
                } else {
                    if !buffer.desc.usage.contains(BufferUsage::STORAGE)
                        || binding.offset % 4 != 0
                        || binding.size % 4 != 0
                    {
                        return Err(Error::InvalidInput(
                            "D3D11 raw SRV requires STORAGE and 4-byte aligned range".into(),
                        ));
                    }
                    let mut srv = None;
                    // SAFETY: native buffer allows raw views; range validated above.
                    unsafe {
                        self.native.CreateShaderResourceView(
                            &buffer.native,
                            Some(&D3D11_SHADER_RESOURCE_VIEW_DESC {
                                Format: DXGI_FORMAT_R32_TYPELESS,
                                ViewDimension: D3D_SRV_DIMENSION_BUFFEREX,
                                Anonymous: D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                                    BufferEx: D3D11_BUFFEREX_SRV {
                                        FirstElement: (binding.offset / 4) as u32,
                                        NumElements: (binding.size / 4) as u32,
                                        Flags: D3D11_BUFFEREX_SRV_FLAG_RAW.0 as u32,
                                    },
                                },
                            }),
                            Some(&mut srv),
                        )
                    }
                    .map_err(backend)?;
                    Ok(Bound::Srv(required(srv)?))
                }
            }
            BindingResource::Texture(binding) => Ok(Bound::Srv(
                self.views
                    .get(binding.texture_view)?
                    .srv
                    .clone()
                    .ok_or_else(|| Error::InvalidInput("view is not sampleable".into()))?,
            )),
            BindingResource::Sampler(binding) => {
                Ok(Bound::Sampler(self.samplers.get(binding.sampler)?.clone()))
            }
        }
    }
}
