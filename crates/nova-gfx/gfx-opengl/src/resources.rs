use crate::device::{OpenGlDevice, native, native_extent};
use crate::render::comparison;
use gfx_core::*;
use glow::HasContext as _;

pub(crate) struct Buffer {
    pub(crate) native: glow::NativeBuffer,
    pub(crate) desc: BufferDescriptor,
    pub(crate) native_size: i32,
}
pub(crate) struct Texture {
    pub(crate) native: glow::NativeTexture,
    pub(crate) desc: TextureDescriptor,
}
impl Texture {
    pub(crate) fn bytes(&self) -> u64 {
        self.desc.byte_size()
    }
}
pub(crate) struct View {
    pub(crate) native: glow::NativeTexture,
    pub(crate) target: u32,
    pub(crate) size: Extent2d,
    pub(crate) format: Format,
    pub(crate) usage: TextureUsage,
    pub(crate) sample_count: u32,
}
pub(crate) struct Mirror {
    pub(crate) size: u64,
    pub(crate) binding: u32,
    pub(crate) native: glow::NativeBuffer,
}
pub(crate) struct BindGroup {
    pub(crate) desc: BindGroupDescriptor,
    pub(crate) mirrors: Vec<Mirror>,
}
pub(crate) fn format(value: Format) -> (u32, u32, u32) {
    match value {
        Format::R8Unorm => (glow::R8, glow::RED, glow::UNSIGNED_BYTE),
        Format::Bgra8Unorm => (glow::RGBA8, glow::BGRA, glow::UNSIGNED_BYTE),
        Format::Bgra8UnormSrgb => (glow::SRGB8_ALPHA8, glow::BGRA, glow::UNSIGNED_BYTE),
        Format::Rgba8Unorm => (glow::RGBA8, glow::RGBA, glow::UNSIGNED_BYTE),
        Format::Rgba8UnormSrgb => (glow::SRGB8_ALPHA8, glow::RGBA, glow::UNSIGNED_BYTE),
        Format::Rgba16Float => (glow::RGBA16F, glow::RGBA, glow::HALF_FLOAT),
        Format::Rg8Unorm => (glow::RG8, glow::RG, glow::UNSIGNED_BYTE),
        Format::R16Float => (glow::R16F, glow::RED, glow::HALF_FLOAT),
        Format::Rg16Float => (glow::RG16F, glow::RG, glow::HALF_FLOAT),
        Format::R32Float => (glow::R32F, glow::RED, glow::FLOAT),
        Format::Rgba32Float => (glow::RGBA32F, glow::RGBA, glow::FLOAT),
        Format::R8Uint => (glow::R8UI, glow::RED_INTEGER, glow::UNSIGNED_BYTE),
        Format::Rg8Uint => (glow::RG8UI, glow::RG_INTEGER, glow::UNSIGNED_BYTE),
        Format::Rgba8Uint => (glow::RGBA8UI, glow::RGBA_INTEGER, glow::UNSIGNED_BYTE),
        Format::Depth32Float => (glow::DEPTH_COMPONENT32F, glow::DEPTH_COMPONENT, glow::FLOAT),
    }
}

impl ResourceDevice for OpenGlDevice {
    fn max_color_attachments(&self) -> u32 {
        self.max_color_attachments_cap
            .min(gfx_core::MAX_COLOR_ATTACHMENTS as u32)
    }

    fn multisample_capabilities(&self) -> MultisampleCapabilities {
        MultisampleCapabilities {
            max_sample_count: self.max_samples_cap,
            stencil_supported: true,
        }
    }

    fn texture_array_capabilities(&self) -> TextureArrayCapabilities {
        TextureArrayCapabilities {
            array_resources: true,
            layer_uploads: true,
            indexed_sampling: false,
            ..self.texture_array_limits
        }
    }

    fn buffer_binding_limits(&self) -> BufferBindingLimits {
        self.binding_limits
    }

    fn compute_capabilities(&self) -> ComputeCapabilities {
        self.compute.capabilities
    }
    fn dispatch_compute(&mut self, dispatch: &ComputeDispatch<'_>) -> Result<()> {
        self.dispatch_buffers(dispatch)
    }
    fn gpu_work_timestamps_supported(&self) -> bool {
        true // This backend requires an OpenGL 4.5 core context.
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
        self.profile_gpu_work(tag, |device| ResourceDevice::write_buffer_batch(device, writes))
    }
    fn drain_gpu_work_timestamps(
        &mut self,
        output: &mut Vec<GpuTimestampSample>,
    ) -> Result<()> {
        self.drain_profiled_gpu_work(output)
    }
    fn dispatch_compute_bind_group(
        &mut self,
        dispatch: &gfx_core::ComputeBindGroupDispatch,
    ) -> Result<()> {
        self.dispatch_bind_group(dispatch)
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
        self.park()?;
        for copy in copies {
            let source = self.textures.get(copy.source)?.native;
            let destination = self.textures.get(copy.destination)?.native;
            // SAFETY: Current owner context, distinct same-format immutable textures and validated
            // mip-zero rectangles. GL command ordering/lifetime rules protect prior draws.
            unsafe {
                self.gl.copy_image_sub_data(
                    source,
                    glow::TEXTURE_2D,
                    0,
                    copy.source_origin.x as i32,
                    copy.source_origin.y as i32,
                    0,
                    destination,
                    glow::TEXTURE_2D,
                    0,
                    copy.destination_origin.x as i32,
                    copy.destination_origin.y as i32,
                    0,
                    copy.size.width() as i32,
                    copy.size.height() as i32,
                    1,
                );
            }
        }
        self.check()
    }
    fn create_buffer(&mut self, desc: &BufferDescriptor) -> Result<BufferId> {
        desc.validate()?;
        if desc.memory_location == MemoryLocation::GpuToCpu {
            return Err(Error::Unavailable(
                "OpenGL CPU readback buffers are not exposed".into(),
            ));
        }
        let native_size = i32::try_from(
            desc.size
                .checked_add(15)
                .ok_or_else(|| Error::InvalidInput("buffer size overflow".into()))?
                / 16
                * 16,
        )
        .map_err(|_| Error::InvalidInput("OpenGL buffer exceeds signed 32-bit size".into()))?;
        self.park()?;
        // SAFETY: context is current, allocation uses a validated size with no borrowed source.
        let buffer = unsafe {
            let buffer = self.gl.create_buffer().map_err(native)?;
            self.gl.bind_buffer(glow::COPY_WRITE_BUFFER, Some(buffer));
            self.gl.buffer_data_size(
                glow::COPY_WRITE_BUFFER,
                native_size,
                if desc.memory_location == MemoryLocation::CpuToGpu {
                    glow::DYNAMIC_DRAW
                } else {
                    glow::STATIC_DRAW
                },
            );
            buffer
        };
        if let Err(error) = self.check() {
            unsafe {
                self.gl.delete_buffer(buffer);
            }
            return Err(error);
        }
        Ok(self.buffers.insert(Buffer {
            native: buffer,
            desc: desc.clone(),
            native_size,
        }))
    }
    fn write_buffer(&mut self, id: BufferId, offset: u64, bytes: &[u8]) -> Result<()> {
        let buffer = self.buffers.get(id)?;
        if offset
            .checked_add(bytes.len() as u64)
            .is_none_or(|end| end > buffer.desc.size)
        {
            return Err(Error::InvalidInput(
                "OpenGL buffer write out of bounds".into(),
            ));
        }
        self.park()?;
        // SAFETY: validated byte range and current owner context; GL copies the source.
        unsafe {
            self.gl
                .bind_buffer(glow::COPY_WRITE_BUFFER, Some(buffer.native));
            self.gl
                .buffer_sub_data_u8_slice(glow::COPY_WRITE_BUFFER, offset as i32, bytes);
        }
        self.check()
    }
    fn write_buffer_batch<'a>(
        &mut self,
        writes: impl IntoIterator<Item = BufferWrite<'a>>,
    ) -> Result<BufferUploadStats> {
        let mut stats = BufferUploadStats::default();
        let mut bound = None;
        for write in writes {
            if write.data.is_empty() {
                continue;
            }
            let buffer = self.buffers.get(write.descriptor.buffer)?;
            if write
                .descriptor
                .offset
                .checked_add(write.data.len() as u64)
                .is_none_or(|end| end > buffer.desc.size)
            {
                return Err(Error::InvalidInput(
                    "OpenGL buffer write out of bounds".into(),
                ));
            }
            if bound.is_none() {
                self.park()?;
            }
            // SAFETY: The owner context is current and GL consumes each validated source slice.
            unsafe {
                if bound != Some(write.descriptor.buffer) {
                    self.gl
                        .bind_buffer(glow::COPY_WRITE_BUFFER, Some(buffer.native));
                }
                self.gl.buffer_sub_data_u8_slice(
                    glow::COPY_WRITE_BUFFER,
                    write.descriptor.offset as i32,
                    write.data,
                );
            }
            bound = Some(write.descriptor.buffer);
            stats.calls = stats.calls.saturating_add(1);
            stats.bytes = stats.bytes.saturating_add(write.data.len() as u64);
        }
        if bound.is_some() {
            self.check()?;
        }
        Ok(stats)
    }
    fn create_texture(&mut self, desc: &TextureDescriptor) -> Result<TextureId> {
        desc.validate()?;
        let (width, height) = native_extent(desc.size)?;
        // A multisampled texture must be bound to the multisample target; GL rejects
        // rebinding one texture object to a different target.
        let target = match desc.dimension {
            TextureDimension::D2 if desc.sample_count > 1 => glow::TEXTURE_2D_MULTISAMPLE,
            TextureDimension::D2 => glow::TEXTURE_2D,
            TextureDimension::D2Array { layers } => {
                self.texture_array_capabilities().plan(
                    desc.size.width(),
                    desc.size.height(),
                    layers,
                    desc.mip_level_count,
                    desc.format,
                )?;
                glow::TEXTURE_2D_ARRAY
            }
        };
        if desc.memory_location == MemoryLocation::GpuToCpu {
            return Err(Error::InvalidInput(
                "use OpenGL read_texture for CPU readback".into(),
            ));
        }
        let (internal, _, _) = format(desc.format);
        self.park()?;
        // SAFETY: immutable texture storage is allocated on the owned current GL context.
        let texture = unsafe {
            let texture = self.gl.create_texture().map_err(native)?;
            self.gl.bind_texture(target, Some(texture));
            if desc.sample_count > 1 {
                if desc.dimension != TextureDimension::D2
                    || desc.mip_level_count != 1
                    || desc.sample_count > self.max_samples_cap
                {
                    self.gl.delete_texture(texture);
                    return Err(Error::InvalidInput(
                        "OpenGL MSAA textures require a single-level D2 target within the sample cap"
                            .into(),
                    ));
                }
                self.gl.tex_storage_2d_multisample(
                    target,
                    desc.sample_count as i32,
                    internal,
                    width,
                    height,
                    true,
                );
            } else if let TextureDimension::D2Array { layers } = desc.dimension {
                self.gl.tex_storage_3d(
                    target,
                    desc.mip_level_count as i32,
                    internal,
                    width,
                    height,
                    i32::try_from(layers).map_err(|_| {
                        Error::InvalidInput("OpenGL array depth exceeds native i32".into())
                    })?,
                );
            } else {
                self.gl.tex_storage_2d(
                    target,
                    desc.mip_level_count as i32,
                    internal,
                    width,
                    height,
                );
            }
            texture
        };
        if let Err(error) = self.check() {
            unsafe {
                self.gl.delete_texture(texture);
            }
            return Err(error);
        }
        Ok(self.textures.insert(Texture {
            native: texture,
            desc: desc.clone(),
        }))
    }
    fn write_texture(&mut self, desc: TextureWriteDescriptor, bytes: &[u8]) -> Result<()> {
        let texture = self.textures.get(desc.texture)?;
        if texture.desc.dimension != TextureDimension::D2 {
            return Err(Error::InvalidInput(
                "use write_texture_array_layer for D2Array".into(),
            ));
        }
        desc.validate_against(&texture.desc, bytes.len())?;
        if !texture.desc.usage.contains(TextureUsage::COPY_DST)
            || desc.layout.bytes_per_row.get() % texture.desc.format.bytes_per_pixel() != 0
        {
            return Err(Error::InvalidInput(
                "OpenGL texture upload requires COPY_DST and pixel-aligned row pitch".into(),
            ));
        }
        self.park()?;
        let (_, external, kind) = format(texture.desc.format);
        // SAFETY: source covers the validated pitch/extent; upload resets pixel-store state.
        unsafe {
            self.gl.bind_texture(glow::TEXTURE_2D, Some(texture.native));
            self.gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            self.gl.pixel_store_i32(
                glow::UNPACK_ROW_LENGTH,
                (desc.layout.bytes_per_row.get() / texture.desc.format.bytes_per_pixel()) as i32,
            );
            self.gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                desc.mip_level as i32,
                desc.origin.x as i32,
                desc.origin.y as i32,
                desc.size.width() as i32,
                desc.size.height() as i32,
                external,
                kind,
                glow::PixelUnpackData::Slice(Some(&bytes[desc.layout.offset as usize..])),
            );
            self.gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0);
        }
        self.check()
    }
    fn write_texture_array_layer(
        &mut self,
        desc: TextureArrayLayerWriteDescriptor,
        data: &[u8],
    ) -> Result<()> {
        let texture = self.textures.get(desc.texture)?;
        desc.validate_against(&texture.desc, self.texture_array_capabilities(), data.len())?;
        let native = texture.native;
        let pixel_format = texture.desc.format;
        let upload = desc.upload;
        if upload.layout.bytes_per_row.get() % pixel_format.bytes_per_pixel() != 0 {
            return Err(Error::InvalidInput(
                "array row pitch must be pixel-aligned".into(),
            ));
        }
        let offset = usize::try_from(upload.layout.offset)
            .map_err(|_| Error::InvalidInput("array source offset exceeds usize".into()))?;
        let layer = i32::try_from(upload.layer)
            .map_err(|_| Error::InvalidInput("array layer exceeds GL signed index".into()))?;
        let row_pixels =
            i32::try_from(upload.layout.bytes_per_row.get() / pixel_format.bytes_per_pixel())
                .map_err(|_| Error::InvalidInput("array row pitch exceeds GL i32".into()))?;
        let level = i32::try_from(upload.mip_level)
            .map_err(|_| Error::InvalidInput("array mip index exceeds GL i32".into()))?;
        let x = i32::try_from(upload.origin.x)
            .map_err(|_| Error::InvalidInput("array X exceeds GL i32".into()))?;
        let y = i32::try_from(upload.origin.y)
            .map_err(|_| Error::InvalidInput("array Y exceeds GL i32".into()))?;
        let width = i32::try_from(upload.extent.width())
            .map_err(|_| Error::InvalidInput("array width exceeds GL i32".into()))?;
        let height = i32::try_from(upload.extent.height())
            .map_err(|_| Error::InvalidInput("array height exceeds GL i32".into()))?;
        self.park()?;
        let (_, external, kind) = format(pixel_format);
        // GLSL sampler2DArray sees the true texture slice. GL sub-image
        // upload depth is one, so a write cannot cross layer boundaries.
        unsafe {
            self.gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(native));
            self.gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            self.gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, row_pixels);
            self.gl.tex_sub_image_3d(
                glow::TEXTURE_2D_ARRAY,
                level,
                x,
                y,
                layer,
                width,
                height,
                1,
                external,
                kind,
                glow::PixelUnpackData::Slice(Some(&data[offset..])),
            );
            self.gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0);
        }
        self.check()
    }

    fn create_texture_view(&mut self, desc: &TextureViewDescriptor) -> Result<TextureViewId> {
        let texture = self.textures.get(desc.texture)?;
        desc.validate_against(&texture.desc)?;
        if desc.format != texture.desc.format {
            return Err(Error::InvalidInput(
                "OpenGL view format must match texture storage".into(),
            ));
        }
        let size = texture.desc.mip_extent(desc.base_mip_level)?;
        let target = if matches!(texture.desc.dimension, TextureDimension::D2Array { .. }) {
            glow::TEXTURE_2D_ARRAY
        } else if texture.desc.sample_count > 1 {
            glow::TEXTURE_2D_MULTISAMPLE
        } else {
            glow::TEXTURE_2D
        };
        let layers = texture.desc.dimension.layers();
        self.park()?;
        // SAFETY: source has immutable storage; the new name has never been bound and the
        // validated mip range shares native storage without a texture copy.
        let view = unsafe {
            let view = self.gl.create_texture().map_err(native)?;
            (self.view_texture)(
                view.0.get(),
                target,
                texture.native.0.get(),
                format(desc.format).0,
                desc.base_mip_level,
                desc.mip_level_count,
                0,
                layers,
            );
            view
        };
        if let Err(error) = self.check() {
            unsafe {
                self.gl.delete_texture(view);
            }
            return Err(error);
        }
        Ok(self.views.insert(View {
            native: view,
            target,
            size,
            format: desc.format,
            usage: texture.desc.usage,
            sample_count: texture.desc.sample_count,
        }))
    }
    fn create_sampler(&mut self, desc: &SamplerDescriptor) -> Result<SamplerId> {
        desc.validate()?;
        self.park()?;
        let address = |value| match value {
            AddressMode::ClampToEdge => glow::CLAMP_TO_EDGE,
            AddressMode::Repeat => glow::REPEAT,
            AddressMode::MirrorRepeat => glow::MIRRORED_REPEAT,
            AddressMode::ClampToBorder => glow::CLAMP_TO_BORDER,
        };
        let min = match (desc.min_filter, desc.mipmap_filter) {
            (FilterMode::Nearest, FilterMode::Nearest) => glow::NEAREST_MIPMAP_NEAREST,
            (FilterMode::Nearest, FilterMode::Linear) => glow::NEAREST_MIPMAP_LINEAR,
            (FilterMode::Linear, FilterMode::Nearest) => glow::LINEAR_MIPMAP_NEAREST,
            (FilterMode::Linear, FilterMode::Linear) => glow::LINEAR_MIPMAP_LINEAR,
        };
        // SAFETY: all sampler parameters are native enums; optional anisotropy is queried.
        let sampler = unsafe {
            let sampler = self.gl.create_sampler().map_err(native)?;
            self.gl
                .sampler_parameter_i32(sampler, glow::TEXTURE_MIN_FILTER, min as i32);
            self.gl.sampler_parameter_i32(
                sampler,
                glow::TEXTURE_MAG_FILTER,
                if desc.mag_filter == FilterMode::Linear {
                    glow::LINEAR as i32
                } else {
                    glow::NEAREST as i32
                },
            );
            self.gl.sampler_parameter_i32(
                sampler,
                glow::TEXTURE_WRAP_S,
                address(desc.address_mode_u) as i32,
            );
            self.gl.sampler_parameter_i32(
                sampler,
                glow::TEXTURE_WRAP_T,
                address(desc.address_mode_v) as i32,
            );
            self.gl.sampler_parameter_i32(
                sampler,
                glow::TEXTURE_WRAP_R,
                address(desc.address_mode_w) as i32,
            );
            if let Some(border) = desc.border_color {
                self.gl
                    .sampler_parameter_f32_slice(sampler, glow::TEXTURE_BORDER_COLOR, &border);
            }
            self.gl
                .sampler_parameter_f32(sampler, glow::TEXTURE_MIN_LOD, desc.lod_min_clamp);
            self.gl
                .sampler_parameter_f32(sampler, glow::TEXTURE_MAX_LOD, desc.lod_max_clamp);
            self.gl
                .sampler_parameter_f32(sampler, glow::TEXTURE_LOD_BIAS, desc.lod_bias);
            if let Some(compare) = desc.compare {
                self.gl.sampler_parameter_i32(
                    sampler,
                    glow::TEXTURE_COMPARE_MODE,
                    glow::COMPARE_REF_TO_TEXTURE as i32,
                );
                self.gl.sampler_parameter_i32(
                    sampler,
                    glow::TEXTURE_COMPARE_FUNC,
                    comparison(compare) as i32,
                );
            }
            if desc.anisotropic {
                if !self
                    .gl
                    .supported_extensions()
                    .contains("GL_EXT_texture_filter_anisotropic")
                    && !self
                        .gl
                        .supported_extensions()
                        .contains("GL_ARB_texture_filter_anisotropic")
                {
                    self.gl.delete_sampler(sampler);
                    return Err(Error::Unavailable(
                        "OpenGL driver does not support requested anisotropy".into(),
                    ));
                }
                let max = self
                    .gl
                    .get_parameter_f32(glow::MAX_TEXTURE_MAX_ANISOTROPY_EXT);
                self.gl.sampler_parameter_f32(
                    sampler,
                    glow::TEXTURE_MAX_ANISOTROPY_EXT,
                    max.min(16.0),
                );
            }
            sampler
        };
        if let Err(error) = self.check() {
            unsafe {
                self.gl.delete_sampler(sampler);
            }
            return Err(error);
        }
        Ok(self.samplers.insert(sampler))
    }
    fn create_bind_group_layout(
        &mut self,
        desc: &BindGroupLayoutDescriptor,
    ) -> Result<BindGroupLayoutId> {
        desc.validate()?;
        Ok(self.layouts.insert(desc.clone()))
    }
    fn create_bind_group(&mut self, desc: &BindGroupDescriptor) -> Result<BindGroupId> {
        let layout = self.layouts.get(desc.layout)?;
        desc.validate_against(layout)?;
        self.park()?;
        // Validate every resource before allocating mirrors, so an invalid later
        // binding cannot leak native buffers allocated for earlier bindings.
        let mut ranges = Vec::new();
        for binding in &desc.entries {
            if let BindingResource::Buffer(range) = binding.resource {
                let buffer = self.buffers.get(range.buffer)?;
                range.validate_against(buffer.desc.size)?;
                let kind = layout
                    .entries
                    .iter()
                    .find(|entry| entry.binding == binding.binding)
                    .ok_or_else(|| Error::InvalidInput("missing OpenGL layout entry".into()))?
                    .binding_type;
                let (usage, alignment) = if kind == BindingType::UniformBuffer {
                    (BufferUsage::UNIFORM, self.uniform_alignment)
                } else {
                    (BufferUsage::STORAGE, self.storage_alignment)
                };
                if !buffer.desc.usage.contains(usage) {
                    return Err(Error::InvalidInput(
                        "OpenGL buffer usage differs from layout".into(),
                    ));
                }
                if range.offset % alignment != 0 {
                    ranges.push((binding.binding, range.size));
                }
            } else if let BindingResource::Texture(binding) = binding.resource {
                self.views.get(binding.texture_view)?;
            } else if let BindingResource::Sampler(binding) = binding.resource {
                self.samplers.get(binding.sampler)?;
            }
        }
        let mirrors = self.create_mirrors(&ranges)?;
        Ok(self.sets.insert(BindGroup {
            desc: desc.clone(),
            mirrors,
        }))
    }
    fn destroy_buffer(&mut self, id: BufferId) -> Result<()> {
        self.park()?;
        let buffer = self.buffers.take(id)?;
        unsafe {
            self.gl.delete_buffer(buffer.native);
        }
        self.check()
    }
    fn destroy_texture(&mut self, id: TextureId) -> Result<()> {
        self.park()?;
        let texture = self.textures.take(id)?;
        unsafe {
            self.gl.delete_texture(texture.native);
        }
        self.check()
    }
    fn destroy_texture_view(&mut self, id: TextureViewId) -> Result<()> {
        self.park()?;
        let view = self.views.take(id)?;
        unsafe {
            self.gl.delete_texture(view.native);
        }
        self.check()
    }
    fn destroy_sampler(&mut self, id: SamplerId) -> Result<()> {
        self.park()?;
        let sampler = self.samplers.take(id)?;
        unsafe {
            self.gl.delete_sampler(sampler);
        }
        self.check()
    }
    fn destroy_bind_group_layout(&mut self, id: BindGroupLayoutId) -> Result<()> {
        self.layouts.take(id)?;
        Ok(())
    }
    fn destroy_bind_group(&mut self, id: BindGroupId) -> Result<()> {
        self.park()?;
        let set = self.sets.take(id)?;
        for mirror in set.mirrors {
            unsafe {
                self.gl.delete_buffer(mirror.native);
            }
        }
        self.check()
    }
}
impl OpenGlDevice {
    fn create_mirrors(&self, ranges: &[(u32, u64)]) -> Result<Vec<Mirror>> {
        let mut mirrors = Vec::with_capacity(ranges.len());
        let allocation = (|| {
            for &(binding, size) in ranges {
                // SAFETY: this owner allocates native GPU copies for otherwise
                // unaligned public buffer ranges. Draw refreshes each live source.
                let native = unsafe { self.gl.create_buffer() }.map_err(native)?;
                mirrors.push(Mirror {
                    binding,
                    native,
                    size: size.div_ceil(16) * 16,
                });
                unsafe {
                    self.gl.bind_buffer(glow::COPY_WRITE_BUFFER, Some(native));
                    self.gl.buffer_data_size(
                        glow::COPY_WRITE_BUFFER,
                        (size.div_ceil(16) * 16) as i32,
                        glow::DYNAMIC_COPY,
                    );
                }
                self.check()?;
            }
            Ok(())
        })();
        if let Err(error) = allocation {
            for mirror in mirrors {
                unsafe {
                    self.gl.delete_buffer(mirror.native);
                }
            }
            return Err(error);
        }
        Ok(mirrors)
    }

    pub(crate) fn readback(&self, id: TextureId) -> Result<TextureReadback> {
        let texture = self.textures.get(id)?;
        if !texture.desc.usage.contains(TextureUsage::COPY_SRC) {
            return Err(Error::InvalidInput(
                "OpenGL readback requires COPY_SRC".into(),
            ));
        }
        self.park()?;
        let (_, external, kind) = format(texture.desc.format);
        let size = texture.desc.size;
        let row = size.width() * texture.desc.format.bytes_per_pixel();
        let mut bytes = vec![0; row as usize * size.height() as usize];
        // SAFETY: temporary FBO attaches this live mip zero; readback destination covers the
        // tightly packed full image. ReadPixels synchronizes prior GPU writes.
        unsafe {
            self.gl
                .bind_framebuffer(glow::FRAMEBUFFER, Some(self.framebuffer));
            let depth = texture.desc.format == Format::Depth32Float;
            self.gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                if depth { None } else { Some(texture.native) },
                0,
            );
            self.gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::DEPTH_ATTACHMENT,
                glow::TEXTURE_2D,
                if depth { Some(texture.native) } else { None },
                0,
            );
            self.gl.read_buffer(if depth {
                glow::NONE
            } else {
                glow::COLOR_ATTACHMENT0
            });
            self.gl.draw_buffer(if depth {
                glow::NONE
            } else {
                glow::COLOR_ATTACHMENT0
            });
            self.gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
            self.gl.read_pixels(
                0,
                0,
                size.width() as i32,
                size.height() as i32,
                external,
                kind,
                glow::PixelPackData::Slice(Some(&mut bytes)),
            );
            self.gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                None,
                0,
            );
            self.gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::DEPTH_ATTACHMENT,
                glow::TEXTURE_2D,
                None,
                0,
            );
            self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
        self.check()?;
        Ok(TextureReadback {
            format: texture.desc.format,
            size,
            bytes_per_row: row,
            bytes,
        })
    }
}

impl OpenGlDevice {
    /// Copies between a buffer object and a texture through the pixel-pack/unpack paths.
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
        self.park()?;
        for copy in copies {
            let texture = self.textures.get(copy.texture)?;
            let buffer = self.buffers.get(copy.buffer)?;
            let (_, external, kind) = format(texture.desc.format);
            let bytes_per_texel = u64::from(texture.desc.format.bytes_per_pixel()).max(1);
            let row_pitch =
                gfx_core::texture_copy_row_pitch(copy.size.width(), texture.desc.format, limits);
            let row_texels = i32::try_from(row_pitch / bytes_per_texel)
                .map_err(|_| Error::InvalidInput("OpenGL copy row length exceeds i32".into()))?;
            let offset = u32::try_from(copy.buffer_offset)
                .map_err(|_| Error::InvalidInput("OpenGL copy offset exceeds u32".into()))?;
            let x = i32::try_from(copy.texture_origin.x).unwrap_or(i32::MAX);
            let y = i32::try_from(copy.texture_origin.y).unwrap_or(i32::MAX);
            let width = i32::try_from(copy.size.width()).unwrap_or(i32::MAX);
            let height = i32::try_from(copy.size.height()).unwrap_or(i32::MAX);
            // SAFETY: The buffer and texture belong to this owner context; the offset and
            // rectangle were validated against both descriptors.
            unsafe {
                match copy.direction {
                    gfx_core::CopyDirection::TextureToBuffer => {
                        // GL exposes no texture-source readback, so attach the mip and read
                        // pixels; the pack row length carries the caller's row pitch.
                        self.gl
                            .bind_framebuffer(glow::FRAMEBUFFER, Some(self.framebuffer));
                        self.gl.framebuffer_texture_2d(
                            glow::FRAMEBUFFER,
                            glow::COLOR_ATTACHMENT0,
                            glow::TEXTURE_2D,
                            Some(texture.native),
                            0,
                        );
                        self.gl.read_buffer(glow::COLOR_ATTACHMENT0);
                        self.gl
                            .bind_buffer(glow::PIXEL_PACK_BUFFER, Some(buffer.native));
                        self.gl.pixel_store_i32(glow::PACK_ROW_LENGTH, row_texels);
                        self.gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
                        self.gl.read_pixels(
                            x,
                            y,
                            width,
                            height,
                            external,
                            kind,
                            glow::PixelPackData::BufferOffset(offset),
                        );
                        self.gl.pixel_store_i32(glow::PACK_ROW_LENGTH, 0);
                        self.gl.bind_buffer(glow::PIXEL_PACK_BUFFER, None);
                        self.gl.framebuffer_texture_2d(
                            glow::FRAMEBUFFER,
                            glow::COLOR_ATTACHMENT0,
                            glow::TEXTURE_2D,
                            None,
                            0,
                        );
                        self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                    }
                    gfx_core::CopyDirection::BufferToTexture => {
                        self.gl
                            .bind_buffer(glow::PIXEL_UNPACK_BUFFER, Some(buffer.native));
                        self.gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, row_texels);
                        self.gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
                        self.gl.texture_sub_image_2d(
                            texture.native,
                            0,
                            x,
                            y,
                            width,
                            height,
                            external,
                            kind,
                            glow::PixelUnpackData::BufferOffset(offset),
                        );
                        self.gl.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0);
                        self.gl.bind_buffer(glow::PIXEL_UNPACK_BUFFER, None);
                    }
                }
            }
        }
        self.check()
    }
}
