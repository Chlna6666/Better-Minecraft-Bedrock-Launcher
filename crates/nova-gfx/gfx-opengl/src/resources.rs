use crate::device::{OpenGlDevice, native, native_extent};
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
        (0..self.desc.mip_level_count)
            .map(|mip| {
                u64::from((self.desc.size.width() >> mip).max(1))
                    * u64::from((self.desc.size.height() >> mip).max(1))
                    * u64::from(self.desc.format.bytes_per_pixel())
            })
            .sum()
    }
}
pub(crate) struct View {
    pub(crate) native: glow::NativeTexture,
    pub(crate) size: Extent2d,
    pub(crate) format: Format,
    pub(crate) usage: TextureUsage,
}
pub(crate) struct Mirror {
    pub(crate) size: u64,
    pub(crate) binding: u32,
    pub(crate) native: glow::NativeBuffer,
}
pub(crate) struct ResourceSet {
    pub(crate) desc: ResourceSetDescriptor,
    pub(crate) mirrors: Vec<Mirror>,
}
pub(crate) fn format(value: Format) -> (u32, u32, u32) {
    match value {
        Format::R8Unorm => (glow::R8, glow::RED, glow::UNSIGNED_BYTE),
        Format::Bgra8Unorm => (glow::RGBA8, glow::BGRA, glow::UNSIGNED_BYTE),
        Format::Bgra8UnormSrgb => (glow::SRGB8_ALPHA8, glow::BGRA, glow::UNSIGNED_BYTE),
        Format::Rgba8Unorm => (glow::RGBA8, glow::RGBA, glow::UNSIGNED_BYTE),
        Format::Rgba8UnormSrgb => (glow::SRGB8_ALPHA8, glow::RGBA, glow::UNSIGNED_BYTE),
        Format::Depth32Float => (glow::DEPTH_COMPONENT32F, glow::DEPTH_COMPONENT, glow::FLOAT),
    }
}

impl ResourceDevice for OpenGlDevice {
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
            if write.descriptor.offset.checked_add(write.data.len() as u64)
                .is_none_or(|end| end > buffer.desc.size)
            {
                return Err(Error::InvalidInput("OpenGL buffer write out of bounds".into()));
            }
            if bound.is_none() {
                self.park()?;
            }
            // SAFETY: The owner context is current and GL consumes each validated source slice.
            unsafe {
                if bound != Some(write.descriptor.buffer) {
                    self.gl.bind_buffer(glow::COPY_WRITE_BUFFER, Some(buffer.native));
                }
                self.gl.buffer_sub_data_u8_slice(glow::COPY_WRITE_BUFFER,
                    write.descriptor.offset as i32, write.data);
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
            self.gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            self.gl.tex_storage_2d(
                glow::TEXTURE_2D,
                desc.mip_level_count as i32,
                internal,
                width,
                height,
            );
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
    fn create_texture_view(&mut self, desc: &TextureViewDescriptor) -> Result<TextureViewId> {
        let texture = self.textures.get(desc.texture)?;
        desc.validate_against(&texture.desc)?;
        if desc.format != texture.desc.format {
            return Err(Error::InvalidInput(
                "OpenGL view format must match texture storage".into(),
            ));
        }
        let size = texture.desc.mip_extent(desc.base_mip_level)?;
        self.park()?;
        // SAFETY: source has immutable storage; the new name has never been bound and the
        // validated mip range shares native storage without a texture copy.
        let view = unsafe {
            let view = self.gl.create_texture().map_err(native)?;
            (self.view_texture)(
                view.0.get(),
                glow::TEXTURE_2D,
                texture.native.0.get(),
                format(desc.format).0,
                desc.base_mip_level,
                desc.mip_level_count,
                0,
                1,
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
            size,
            format: desc.format,
            usage: texture.desc.usage,
        }))
    }
    fn create_sampler(&mut self, desc: &SamplerDescriptor) -> Result<SamplerId> {
        self.park()?;
        let address = |value| {
            if value == AddressMode::Repeat {
                glow::REPEAT
            } else {
                glow::CLAMP_TO_EDGE
            }
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
    fn create_resource_set_layout(
        &mut self,
        desc: &ResourceSetLayoutDescriptor,
    ) -> Result<ResourceSetLayoutId> {
        desc.validate()?;
        Ok(self.layouts.insert(desc.clone()))
    }
    fn create_resource_set(&mut self, desc: &ResourceSetDescriptor) -> Result<ResourceSetId> {
        let layout = self.layouts.get(desc.layout)?;
        desc.validate_against(layout)?;
        self.park()?;
        // Validate every resource before allocating mirrors, so an invalid later
        // binding cannot leak native buffers allocated for earlier bindings.
        let mut ranges = Vec::new();
        for binding in &desc.bindings {
            if let BindingResource::Buffer(range) = binding.resource {
                let buffer = self.buffers.get(range.buffer)?;
                range.validate_against(buffer.desc.size)?;
                let kind = layout
                    .entries
                    .iter()
                    .find(|entry| entry.binding == binding.binding)
                    .ok_or_else(|| Error::InvalidInput("missing OpenGL layout entry".into()))?
                    .binding_type;
                let (usage, alignment) = if kind == ResourceBindingType::UniformBuffer {
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
        Ok(self.sets.insert(ResourceSet {
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
    fn destroy_resource_set_layout(&mut self, id: ResourceSetLayoutId) -> Result<()> {
        self.layouts.take(id)?;
        Ok(())
    }
    fn destroy_resource_set(&mut self, id: ResourceSetId) -> Result<()> {
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
