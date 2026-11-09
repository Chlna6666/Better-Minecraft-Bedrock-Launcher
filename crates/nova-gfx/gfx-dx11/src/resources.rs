use crate::device::{Dx11Device, backend, required};
use gfx_core::*;
use std::cell::RefCell;
use windows::Win32::Graphics::Direct3D::{
    D3D_SRV_DIMENSION_BUFFEREX, D3D_SRV_DIMENSION_TEXTURE2D, D3D_SRV_DIMENSION_TEXTURE2DARRAY,
    D3D_SRV_DIMENSION_TEXTURE2DMS, D3D_SRV_DIMENSION_TEXTURE2DMSARRAY,
};
use windows::Win32::Graphics::{Direct3D11::*, Dxgi::Common::*};

mod buffer_upload;
mod compute;

pub(crate) struct Buffer {
    pub(crate) native: ID3D11Buffer,
    pub(crate) desc: BufferDescriptor,
    pub(crate) native_size: u32,
    uniform_bytes: Option<Vec<u8>>,
    // Keep one range per view type. Graphics and compute share raw SRVs; changing
    // ranges replace the cached view without invalidating views retained by bind groups.
    srv: RefCell<Option<BufferView<ID3D11ShaderResourceView>>>,
    compute_uav: RefCell<Option<BufferView<ID3D11UnorderedAccessView>>>,
}

struct BufferView<T> {
    offset: u64,
    size: u64,
    native: T,
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
        self.desc.byte_size()
    }
}
pub(crate) struct View {
    pub(crate) srv: Option<ID3D11ShaderResourceView>,
    pub(crate) rtv: Option<ID3D11RenderTargetView>,
    pub(crate) dsv: Option<ID3D11DepthStencilView>,
    pub(crate) size: Extent2d,
    pub(crate) format: Format,
    pub(crate) sample_count: u32,
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
pub(crate) struct BindGroup {
    pub(crate) layout: BindGroupLayoutId,
    pub(crate) desc: BindGroupDescriptor,
    pub(crate) bindings: Vec<(u32, ShaderStages, Bound)>,
}

pub(crate) fn format(value: Format) -> DXGI_FORMAT {
    match value {
        Format::R8Unorm => DXGI_FORMAT_R8_UNORM,
        Format::Bgra8Unorm => DXGI_FORMAT_B8G8R8A8_UNORM,
        Format::Bgra8UnormSrgb => DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
        Format::Rgba8Unorm => DXGI_FORMAT_R8G8B8A8_UNORM,
        Format::Rgba8UnormSrgb => DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
        Format::Rgba16Float => DXGI_FORMAT_R16G16B16A16_FLOAT,
        Format::Depth32Float => DXGI_FORMAT_D32_FLOAT,
        Format::Rg8Unorm => DXGI_FORMAT_R8G8_UNORM,
        Format::R16Float => DXGI_FORMAT_R16_FLOAT,
        Format::Rg16Float => DXGI_FORMAT_R16G16_FLOAT,
        Format::R32Float => DXGI_FORMAT_R32_FLOAT,
        Format::Rgba32Float => DXGI_FORMAT_R32G32B32A32_FLOAT,
        Format::R8Uint => DXGI_FORMAT_R8_UINT,
        Format::Rg8Uint => DXGI_FORMAT_R8G8_UINT,
        Format::Rgba8Uint => DXGI_FORMAT_R8G8B8A8_UINT,
    }
}

impl ResourceDevice for Dx11Device {
    fn max_color_attachments(&self) -> u32 {
        // D3D11 feature level 11.0 guarantees eight simultaneous render targets.
        gfx_core::MAX_COLOR_ATTACHMENTS as u32
    }

    fn multisample_capabilities(&self) -> gfx_core::MultisampleCapabilities {
        use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM;
        let mut max_sample_count = 1;
        for count in [1, 2, 4, 8, 16, 32, 64] {
            // SAFETY: device is live; the binding returns the supported quality level.
            let quality = unsafe {
                self.native
                    .CheckMultisampleQualityLevels(DXGI_FORMAT_R8G8B8A8_UNORM, count)
            }
            .unwrap_or_default();
            if quality > 0 {
                max_sample_count = count;
            } else {
                break;
            }
        }
        gfx_core::MultisampleCapabilities {
            max_sample_count,
            stencil_supported: true,
        }
    }

    fn texture_array_capabilities(&self) -> TextureArrayCapabilities {
        // This backend requires FL11_0, so D3D11's mandatory resource limits
        // apply; they are not queryable per adapter. Array resource creation
        // and layer sampling are wired below, while dynamic indexed shader
        // sampling stays disabled.
        TextureArrayCapabilities {
            max_dimension_2d:
                windows::Win32::Graphics::Direct3D11::D3D11_REQ_TEXTURE2D_U_OR_V_DIMENSION,
            max_array_layers:
                windows::Win32::Graphics::Direct3D11::D3D11_REQ_TEXTURE2D_ARRAY_AXIS_DIMENSION,
            array_resources: true,
            layer_uploads: true,
            indexed_sampling: false,
        }
    }

    fn buffer_binding_limits(&self) -> BufferBindingLimits {
        // Native D3D11 raw SRV/UAV views use 32-bit words, not Vulkan's
        // uniform 256-byte storage-offset requirement. Keep the guaranteed
        // D3D11 resource range below the documented maximum raw texel count.
        BufferBindingLimits {
            storage_offset_alignment: 4,
            max_storage_binding_bytes: (1u64 << 27) * 4,
            uniform_offset_alignment: 256,
            max_uniform_binding_bytes: 64 * 1024,
        }
    }

    fn compute_capabilities(&self) -> ComputeCapabilities {
        ComputeCapabilities {
            // D3D11 feature-level 11.0 guaranteed compute minimums.
            max_workgroups: [65535; 3],
            // Shader model 5 compute threads per group (D3D11_CS_THREAD_GROUP_MAX_THREADS_PER_GROUP).
            max_invocations_per_workgroup: 1024,
            max_workgroup_size: [1024, 1024, 64],
            max_workgroup_shared_memory_bytes: 32 * 1024,
            // DrawIndirect is always accepted; devices without D3D11.1
            // constant-buffer offsetting take the per-draw CPU staging path
            // in commands::indirect instead of GPU base-offset packing.
            indirect_draw: true,
        }
    }

    fn compute_bind_group_capabilities(&self) -> ComputeBindGroupCapabilities {
        ComputeBindGroupCapabilities::buffer_only()
    }
    fn dispatch_compute(&mut self, dispatch: &ComputeDispatch<'_>) -> Result<()> {
        self.dispatch_buffers(dispatch)
    }
    fn dispatch_compute_bind_group(&mut self, dispatch: &ComputeBindGroupDispatch) -> Result<()> {
        self.dispatch_bind_group_buffers(dispatch)
    }
    fn gpu_work_timestamps_supported(&self) -> bool {
        self.timestamp_queries_supported
    }
    fn dispatch_compute_batch_profiled(
        &mut self,
        dispatches: &[ComputeDispatch<'_>],
        tag: GpuTimestampTag,
    ) -> Result<()> {
        if dispatches.is_empty() {
            return Ok(());
        }
        self.profile_gpu_work(tag, |device| {
            for dispatch in dispatches {
                device.dispatch_buffers(dispatch)?;
            }
            Ok(())
        })
    }
    fn write_buffer_batch_profiled<'a>(
        &mut self,
        writes: impl IntoIterator<Item = BufferWrite<'a>>,
        tag: GpuTimestampTag,
    ) -> Result<BufferUploadStats> {
        self.profile_gpu_work(tag, |device| device.upload_buffer_batch(writes))
    }
    fn drain_gpu_work_timestamps(
        &mut self,
        output: &mut Vec<GpuTimestampSample>,
    ) -> Result<()> {
        self.retire()?;
        output.append(&mut self.gpu_work_samples);
        Ok(())
    }
    fn copy_texture_buffer_batch(&mut self, copies: &[gfx_core::TextureBufferCopy]) -> Result<()> {
        self.copy_texture_buffer_regions(copies)
    }
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
        let limits = self.buffer_binding_limits();
        let alignment = if uniform {
            u64::from(limits.uniform_offset_alignment)
        } else {
            u64::from(limits.storage_offset_alignment)
        };
        let native_size = u32::try_from(
            desc.size
                .checked_add(alignment - 1)
                .ok_or_else(|| Error::InvalidInput("buffer size overflow".into()))?
                / alignment
                * alignment,
        )
        .map_err(|_| Error::InvalidInput("D3D11 buffer exceeds 32-bit size".into()))?;
        if uniform && u64::from(native_size) > limits.max_uniform_binding_bytes {
            return Err(Error::InvalidInput(
                "D3D11 uniform buffer exceeds the reported binding limit".into(),
            ));
        }
        let mut bind = 0;
        if uniform {
            bind |= D3D11_BIND_CONSTANT_BUFFER.0 as u32;
        }
        if desc.usage.contains(BufferUsage::STORAGE) {
            bind |= D3D11_BIND_SHADER_RESOURCE.0 as u32;
        }
        if desc.usage.contains(BufferUsage::STORAGE_WRITE) {
            bind |= D3D11_BIND_UNORDERED_ACCESS.0 as u32;
        }
        if desc.usage.contains(BufferUsage::INDEX) {
            bind |= D3D11_BIND_INDEX_BUFFER.0 as u32;
        }
        if desc.usage.contains(BufferUsage::VERTEX) {
            bind |= D3D11_BIND_VERTEX_BUFFER.0 as u32;
        }
        let mut misc = if desc.usage.contains(BufferUsage::STORAGE) {
            D3D11_RESOURCE_MISC_BUFFER_ALLOW_RAW_VIEWS.0 as u32
        } else {
            0
        };
        if desc.usage.contains(BufferUsage::INDIRECT) {
            misc |= D3D11_RESOURCE_MISC_DRAWINDIRECT_ARGS.0 as u32;
        }
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
            srv: RefCell::new(None),
            compute_uav: RefCell::new(None),
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
        if let TextureDimension::D2Array { layers } = desc.dimension {
            self.texture_array_capabilities().plan(
                desc.size.width(),
                desc.size.height(),
                layers,
                desc.mip_level_count,
                desc.format,
            )?;
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
        if desc.usage.contains(TextureUsage::STORAGE) {
            bind |= D3D11_BIND_UNORDERED_ACCESS.0 as u32;
        }
        let mut native = None;
        // SAFETY: initialized texture descriptor, no borrowed initial data.
        unsafe {
            self.native.CreateTexture2D(
                &D3D11_TEXTURE2D_DESC {
                    Width: desc.size.width(),
                    Height: desc.size.height(),
                    MipLevels: desc.mip_level_count,
                    ArraySize: desc.dimension.layers(),
                    // Depth maps are sampled through typed views, so the resource is typeless.
                    Format: if desc.format == Format::Depth32Float
                        && desc.usage.contains(TextureUsage::SAMPLED)
                    {
                        DXGI_FORMAT_R32_TYPELESS
                    } else {
                        format(desc.format)
                    },
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: desc.sample_count,
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
        if texture.desc.dimension != TextureDimension::D2 {
            return Err(Error::InvalidInput(
                "use write_texture_array_layer for D2Array".into(),
            ));
        }
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
    fn write_texture_array_layer(
        &mut self,
        desc: TextureArrayLayerWriteDescriptor,
        data: &[u8],
    ) -> Result<()> {
        let texture = self.textures.get(desc.texture)?;
        desc.validate_against(&texture.desc, self.texture_array_capabilities(), data.len())?;
        let subresource = desc.subresource_index(texture.desc.mip_level_count)?;
        let upload = desc.upload;
        let region = D3D11_BOX {
            left: upload.origin.x,
            top: upload.origin.y,
            right: upload.origin.x + upload.extent.width(),
            bottom: upload.origin.y + upload.extent.height(),
            front: 0,
            back: 1,
        };
        // Native D3D11 subresources index mip + array_slice * mip_count.
        // GPU command order preserves writes and subsequent sampled draws.
        unsafe {
            self.context.UpdateSubresource(
                &texture.native,
                subresource,
                Some(&region),
                data.as_ptr().add(upload.layout.offset as usize).cast(),
                upload.layout.bytes_per_row.get(),
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
            sample_count: texture.desc.sample_count,
        };
        // SAFETY: the texture is live; each view descriptor selects a validated mip range.
        unsafe {
            if texture.desc.usage.contains(TextureUsage::SAMPLED) {
                self.native
                    .CreateShaderResourceView(
                        &texture.native,
                        Some(&D3D11_SHADER_RESOURCE_VIEW_DESC {
                            Format: if desc.format == Format::Depth32Float {
                                DXGI_FORMAT_R32_FLOAT
                            } else {
                                format(desc.format)
                            },
                            // Multisampled textures need the MS view dimension; a plain
                            // TEXTURE2D SRV over MS storage is E_INVALIDARG on D3D11.
                            ViewDimension: match (
                                texture.desc.dimension == TextureDimension::D2,
                                texture.desc.sample_count > 1,
                            ) {
                                (true, false) => D3D_SRV_DIMENSION_TEXTURE2D,
                                (false, false) => D3D_SRV_DIMENSION_TEXTURE2DARRAY,
                                (true, true) => D3D_SRV_DIMENSION_TEXTURE2DMS,
                                (false, true) => D3D_SRV_DIMENSION_TEXTURE2DMSARRAY,
                            },
                            Anonymous: if let TextureDimension::D2Array { layers } =
                                texture.desc.dimension
                            {
                                D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                                    Texture2DArray: D3D11_TEX2D_ARRAY_SRV {
                                        MostDetailedMip: desc.base_mip_level,
                                        MipLevels: desc.mip_level_count,
                                        FirstArraySlice: 0,
                                        ArraySize: layers,
                                    },
                                }
                            } else {
                                D3D11_SHADER_RESOURCE_VIEW_DESC_0 {
                                    Texture2D: D3D11_TEX2D_SRV {
                                        MostDetailedMip: desc.base_mip_level,
                                        MipLevels: desc.mip_level_count,
                                    },
                                }
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
                // Multisampled storage requires the MS view dimensions; TEXTURE2D is
                // E_INVALIDARG over a multisampled resource on D3D11.
                let multisampled = texture.desc.sample_count > 1;
                if desc.format == Format::Depth32Float {
                    self.native
                        .CreateDepthStencilView(
                            &texture.native,
                            Some(&D3D11_DEPTH_STENCIL_VIEW_DESC {
                                Format: format(desc.format),
                                ViewDimension: if multisampled {
                                    D3D11_DSV_DIMENSION_TEXTURE2DMS
                                } else {
                                    D3D11_DSV_DIMENSION_TEXTURE2D
                                },
                                Flags: 0,
                                Anonymous: if multisampled {
                                    D3D11_DEPTH_STENCIL_VIEW_DESC_0::default()
                                } else {
                                    D3D11_DEPTH_STENCIL_VIEW_DESC_0 {
                                        Texture2D: D3D11_TEX2D_DSV {
                                            MipSlice: desc.base_mip_level,
                                        },
                                    }
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
                                ViewDimension: if multisampled {
                                    D3D11_RTV_DIMENSION_TEXTURE2DMS
                                } else {
                                    D3D11_RTV_DIMENSION_TEXTURE2D
                                },
                                Anonymous: if multisampled {
                                    D3D11_RENDER_TARGET_VIEW_DESC_0::default()
                                } else {
                                    D3D11_RENDER_TARGET_VIEW_DESC_0 {
                                        Texture2D: D3D11_TEX2D_RTV {
                                            MipSlice: desc.base_mip_level,
                                        },
                                    }
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
        desc.validate()?;
        let address = |value| match value {
            AddressMode::ClampToEdge => D3D11_TEXTURE_ADDRESS_CLAMP,
            AddressMode::Repeat => D3D11_TEXTURE_ADDRESS_WRAP,
            AddressMode::MirrorRepeat => D3D11_TEXTURE_ADDRESS_MIRROR,
            AddressMode::ClampToBorder => D3D11_TEXTURE_ADDRESS_BORDER,
        };
        let filter = if desc.anisotropic {
            if desc.compare.is_some() {
                D3D11_FILTER_COMPARISON_ANISOTROPIC
            } else {
                D3D11_FILTER_ANISOTROPIC
            }
        } else {
            let bits = (if desc.min_filter == FilterMode::Linear {
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
            });
            let comparison_bit = if desc.compare.is_some() { 0x80 } else { 0 };
            D3D11_FILTER(bits | comparison_bit)
        };
        let mut sampler = None;
        // SAFETY: initialized sampler descriptor and owned output.
        unsafe {
            self.native.CreateSamplerState(
                &D3D11_SAMPLER_DESC {
                    Filter: filter,
                    AddressU: address(desc.address_mode_u),
                    AddressV: address(desc.address_mode_v),
                    AddressW: address(desc.address_mode_w),
                    MaxAnisotropy: if desc.anisotropic { 16 } else { 1 },
                    ComparisonFunc: desc
                        .compare
                        .map(crate::pipelines::comparison)
                        .unwrap_or(D3D11_COMPARISON_NEVER),
                    MinLOD: desc.lod_min_clamp,
                    MaxLOD: desc.lod_max_clamp,
                    MipLODBias: desc.lod_bias,
                    BorderColor: desc
                        .border_color
                        .map(|c| [c[0], c[1], c[2], c[3]])
                        .unwrap_or([0.0; 4]),
                    ..Default::default()
                },
                Some(&mut sampler),
            )
        }
        .map_err(backend)?;
        Ok(self.samplers.insert(required(sampler)?))
    }
    fn create_bind_group_layout(
        &mut self,
        desc: &BindGroupLayoutDescriptor,
    ) -> Result<BindGroupLayoutId> {
        desc.validate()?;
        for entry in &desc.entries {
            let limit = match entry.binding_type {
                BindingType::UniformBuffer => 13,
                BindingType::Sampler => 16,
                BindingType::StorageBuffer | BindingType::SampledTexture => 128,
                BindingType::StorageTexture { .. } => {
                    return Err(Error::Unavailable(
                        "D3D11 storage textures need an unordered-access-view binding path".into(),
                    ));
                }
            };
            if entry.binding >= limit {
                return Err(Error::InvalidInput(
                    "D3D11 register slot exceeds native limit (b13 reserved)".into(),
                ));
            }
        }
        Ok(self.layouts.insert(desc.clone()))
    }
    fn create_bind_group(&mut self, desc: &BindGroupDescriptor) -> Result<BindGroupId> {
        let layout = self.layouts.get(desc.layout)?;
        desc.validate_against(layout)?;
        let bindings = desc
            .entries
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
        Ok(self.sets.insert(BindGroup {
            layout: desc.layout,
            desc: desc.clone(),
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
    fn destroy_bind_group_layout(&mut self, id: BindGroupLayoutId) -> Result<()> {
        self.layouts.take(id)?;
        Ok(())
    }
    fn destroy_bind_group(&mut self, id: BindGroupId) -> Result<()> {
        self.sets.take(id)?;
        Ok(())
    }
}

impl Dx11Device {
    pub(crate) fn bind_resource(
        &self,
        resource: &BindingResource,
        kind: BindingType,
    ) -> Result<Bound> {
        match resource {
            BindingResource::Buffer(binding) => {
                let buffer = self.buffers.get(binding.buffer)?;
                binding.validate_against(buffer.desc.size)?;
                if kind == BindingType::UniformBuffer {
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
                    if let Some(cached) =
                        buffer.srv.borrow().as_ref().filter(|view| {
                            view.offset == binding.offset && view.size == binding.size
                        })
                    {
                        return Ok(Bound::Srv(cached.native.clone()));
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
                    let view = required(srv)?;
                    *buffer.srv.borrow_mut() = Some(BufferView {
                        offset: binding.offset,
                        size: binding.size,
                        native: view.clone(),
                    });
                    Ok(Bound::Srv(view))
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

impl Dx11Device {
    /// Copies between a buffer and a texture through D3D11 staging resources.
    ///
    /// D3D11 has no buffer/texture copy primitive, so the portable row pitch is tight and the
    /// staging object carries the rectangle to or from CPU-visible memory.
    pub(crate) fn copy_texture_buffer_regions(
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
        for copy in copies {
            let texture = self.textures.get(copy.texture)?;
            let buffer = self.buffers.get(copy.buffer)?;
            let width = copy.size.width();
            let height = copy.size.height();
            let row = u64::from(width) * u64::from(texture.desc.format.bytes_per_pixel());
            let bytes = usize::try_from(row * u64::from(height))
                .map_err(|_| Error::InvalidInput("D3D11 copy size exceeds usize".into()))?;
            let staging_texture = create_staging_texture(
                &self.native,
                width,
                height,
                crate::resources::format(texture.desc.format),
                copy.direction == gfx_core::CopyDirection::BufferToTexture,
            )?;
            let region = windows::Win32::Graphics::Direct3D11::D3D11_BOX {
                left: copy.texture_origin.x,
                top: copy.texture_origin.y,
                front: 0,
                right: copy.texture_origin.x + width,
                bottom: copy.texture_origin.y + height,
                back: 1,
            };
            match copy.direction {
                gfx_core::CopyDirection::TextureToBuffer => {
                    // SAFETY: Both textures are live, same format; the box is inside the source.
                    unsafe {
                        self.context.CopySubresourceRegion(
                            &staging_texture,
                            0,
                            0,
                            0,
                            0,
                            &texture.native,
                            0,
                            Some(&region),
                        );
                    }
                    let mut mapped =
                        windows::Win32::Graphics::Direct3D11::D3D11_MAPPED_SUBRESOURCE::default();
                    // SAFETY: Staging texture is CPU-readable and not bound to the pipeline.
                    unsafe {
                        self.context
                            .Map(
                                &staging_texture,
                                0,
                                windows::Win32::Graphics::Direct3D11::D3D11_MAP_READ,
                                0,
                                Some(&mut mapped),
                            )
                            .map_err(backend)?;
                    }
                    let mut pixels = vec![0u8; bytes];
                    // SAFETY: The mapped region covers the whole staging texture.
                    unsafe {
                        let source = std::slice::from_raw_parts(
                            mapped.pData.cast::<u8>(),
                            mapped.RowPitch as usize * height as usize,
                        );
                        for y in 0..height as usize {
                            let from = y * mapped.RowPitch as usize;
                            let to = y * row as usize;
                            pixels[to..to + row as usize]
                                .copy_from_slice(&source[from..from + row as usize]);
                        }
                        self.context.Unmap(&staging_texture, 0);
                    }
                    let target = windows::Win32::Graphics::Direct3D11::D3D11_BOX {
                        left: u32::try_from(copy.buffer_offset).map_err(|_| {
                            Error::InvalidInput("D3D11 copy offset exceeds u32".into())
                        })?,
                        top: 0,
                        front: 0,
                        right: u32::try_from(copy.buffer_offset + row * u64::from(height))
                            .map_err(|_| {
                                Error::InvalidInput("D3D11 copy end exceeds u32".into())
                            })?,
                        bottom: 1,
                        back: 1,
                    };
                    // SAFETY: The destination box lies inside the validated buffer range.
                    unsafe {
                        self.context.UpdateSubresource(
                            &buffer.native,
                            0,
                            Some(&target),
                            pixels.as_ptr().cast(),
                            0,
                            0,
                        );
                    }
                }
                gfx_core::CopyDirection::BufferToTexture => {
                    let staging_buffer = create_staging_buffer(&self.native, buffer.native_size)?;
                    // SAFETY: Both buffers are live; CopyResource needs identical descriptors,
                    // so the staging buffer is created with the source's byte width.
                    unsafe {
                        self.context.CopyResource(&staging_buffer, &buffer.native);
                    }
                    let mut mapped =
                        windows::Win32::Graphics::Direct3D11::D3D11_MAPPED_SUBRESOURCE::default();
                    // SAFETY: Staging buffer is CPU-readable and not bound to the pipeline.
                    unsafe {
                        self.context
                            .Map(
                                &staging_buffer,
                                0,
                                windows::Win32::Graphics::Direct3D11::D3D11_MAP_READ,
                                0,
                                Some(&mut mapped),
                            )
                            .map_err(backend)?;
                    }
                    let mut pixels = vec![0u8; bytes];
                    // SAFETY: The mapped region covers the staged buffer bytes.
                    unsafe {
                        let offset = usize::try_from(copy.buffer_offset).unwrap_or(usize::MAX);
                        let source = std::slice::from_raw_parts(
                            mapped.pData.cast::<u8>(),
                            usize::try_from(buffer.native_size).unwrap_or(0),
                        );
                        match source.get(offset..offset + bytes) {
                            Some(range) => pixels.copy_from_slice(range),
                            None => {
                                self.context.Unmap(&staging_buffer, 0);
                                return Err(Error::InvalidInput(
                                    "D3D11 copy range exceeds the source buffer".into(),
                                ));
                            }
                        }
                        self.context.Unmap(&staging_buffer, 0);
                    }
                    let mut write =
                        windows::Win32::Graphics::Direct3D11::D3D11_MAPPED_SUBRESOURCE::default();
                    // SAFETY: Staging texture is CPU-writable and not bound to the pipeline.
                    unsafe {
                        self.context
                            .Map(
                                &staging_texture,
                                0,
                                windows::Win32::Graphics::Direct3D11::D3D11_MAP_WRITE,
                                0,
                                Some(&mut write),
                            )
                            .map_err(backend)?;
                        let target = std::slice::from_raw_parts_mut(
                            write.pData.cast::<u8>(),
                            write.RowPitch as usize * height as usize,
                        );
                        for y in 0..height as usize {
                            let from = y * row as usize;
                            let to = y * write.RowPitch as usize;
                            target[to..to + row as usize]
                                .copy_from_slice(&pixels[from..from + row as usize]);
                        }
                        self.context.Unmap(&staging_texture, 0);
                        self.context.CopySubresourceRegion(
                            &texture.native,
                            0,
                            copy.texture_origin.x,
                            copy.texture_origin.y,
                            0,
                            &staging_texture,
                            0,
                            None,
                        );
                    }
                }
            }
        }
        Ok(())
    }
}

fn create_staging_texture(
    device: &windows::Win32::Graphics::Direct3D11::ID3D11Device,
    width: u32,
    height: u32,
    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
    write: bool,
) -> Result<windows::Win32::Graphics::Direct3D11::ID3D11Texture2D> {
    use windows::Win32::Graphics::{Direct3D11::*, Dxgi::Common::DXGI_SAMPLE_DESC};
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: if write {
            D3D11_CPU_ACCESS_WRITE.0 as u32
        } else {
            D3D11_CPU_ACCESS_READ.0 as u32
        },
        MiscFlags: 0,
    };
    let mut staging = None;
    // SAFETY: The descriptor describes a CPU-visible staging texture.
    unsafe {
        device
            .CreateTexture2D(&desc, None, Some(&mut staging))
            .map_err(backend)?
    };
    required(staging)
}

fn create_staging_buffer(
    device: &windows::Win32::Graphics::Direct3D11::ID3D11Device,
    size: u32,
) -> Result<windows::Win32::Graphics::Direct3D11::ID3D11Buffer> {
    use windows::Win32::Graphics::Direct3D11::*;
    let desc = D3D11_BUFFER_DESC {
        ByteWidth: size,
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
        StructureByteStride: 0,
    };
    let mut staging = None;
    // SAFETY: The descriptor describes a CPU-visible staging buffer.
    unsafe {
        device
            .CreateBuffer(&desc, None, Some(&mut staging))
            .map_err(backend)?
    };
    required(staging)
}
