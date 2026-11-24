//! Native 2D texture allocation, views, bounded reusable staging uploads and samplers.

use super::*;
use objc2_foundation::NSRange;
use objc2_metal::{
    MTLBlitCommandEncoder, MTLOrigin, MTLSamplerAddressMode,
    MTLSamplerBorderColor, MTLSamplerDescriptor, MTLSamplerMinMagFilter,
    MTLSamplerMipFilter, MTLSize, MTLStorageMode, MTLTextureDescriptor,
    MTLTextureType, MTLTextureUsage,
};

// Small atlas uploads normally reuse one of these pages. Never grow a
// permanent pool in response to a burst or block the GPU owner for a page.
const MAX_UPLOAD_PAGES: usize = 4;
const MAX_UPLOAD_PAGE_BYTES: usize = 4 * 1024 * 1024;
const MIN_UPLOAD_PAGE_BYTES: usize = 64 * 1024;

struct UploadPage {
    buffer: Retained<ProtocolObject<dyn MTLBuffer>>,
    capacity: usize,
    used: usize,
    pending: Option<Retained<ProtocolObject<dyn MTLCommandBuffer>>>,
}

#[derive(Default)]
pub(super) struct TextureUploadArena {
    pages: Vec<UploadPage>,
}

impl TextureUploadArena {
    fn acquire(
        &mut self,
        device: &ProtocolObject<dyn MTLDevice>,
        size: usize,
    ) -> Result<(Retained<ProtocolObject<dyn MTLBuffer>>, Option<usize>)> {
        for page in &mut self.pages {
            if let Some(pending) = &page.pending {
                match pending.status() {
                    MTLCommandBufferStatus::Completed => {
                        page.pending = None;
                        page.used = 0;
                    }
                    MTLCommandBufferStatus::Error => {
                        let error = pending.error().as_deref().map(nserror_message)
                            .unwrap_or_else(|| "Metal texture upload failed".into());
                        page.pending = None;
                        page.used = 0;
                        return Err(Error::Backend(error));
                    }
                    _ => {}
                }
            }
        }
        if size <= MAX_UPLOAD_PAGE_BYTES {
            if let Some((index, page)) = self.pages.iter().enumerate()
                .filter(|(_, page)| page.pending.is_none() && page.capacity >= size)
                .min_by_key(|(_, page)| page.capacity)
            {
                return Ok((page.buffer.clone(), Some(index)));
            }
            let replace = self.pages.iter().position(|page| page.pending.is_none());
            if replace.is_some() || self.pages.len() < MAX_UPLOAD_PAGES {
                let capacity = size.max(MIN_UPLOAD_PAGE_BYTES).next_power_of_two();
                let buffer = Self::allocate(device, capacity)?;
                let page = UploadPage { buffer: buffer.clone(), capacity, used: 0, pending: None };
                let index = if let Some(index) = replace {
                    self.pages[index] = page;
                    index
                } else {
                    let index = self.pages.len();
                    self.pages.push(page);
                    index
                };
                return Ok((buffer, Some(index)));
            }
        }
        // Oversized transfers or a fully occupied pool use a transient buffer.
        // The native command buffer retains it until the GPU finishes.
        Ok((Self::allocate(device, size)?, None))
    }

    fn allocate(
        device: &ProtocolObject<dyn MTLDevice>,
        size: usize,
    ) -> Result<Retained<ProtocolObject<dyn MTLBuffer>>> {
        device.newBufferWithLength_options(size, MTLResourceOptions::StorageModeShared)
            .ok_or_else(|| Error::Backend("failed to allocate Metal texture staging buffer".into()))
    }

    fn submitted(
        &mut self, index: Option<usize>, size: usize,
        commands: Retained<ProtocolObject<dyn MTLCommandBuffer>>,
    ) {
        if let Some(index) = index {
            self.pages[index].used = size;
            self.pages[index].pending = Some(commands);
        }
    }

    pub(super) fn release_completed(&mut self) {
        for page in &mut self.pages {
            // Preserve failed commands until acquire can report the native error.
            if page.pending.as_ref().is_some_and(|pending| pending.status() == MTLCommandBufferStatus::Completed) {
                page.pending = None;
                page.used = 0;
            }
        }
    }

    pub(super) fn trim(&mut self, level: gfx_core::MemoryTrimLevel) {
        self.release_completed();
        let mut retain_idle = match level {
            gfx_core::MemoryTrimLevel::Light => return,
            gfx_core::MemoryTrimLevel::Moderate => 1,
            gfx_core::MemoryTrimLevel::Aggressive => 0,
        };
        self.pages.retain(|page| {
            if page.pending.is_some() { return true; }
            if retain_idle == 0 { return false; }
            retain_idle -= 1;
            true
        });
    }

    pub(super) fn stats(&self) -> (u64, u64) {
        let mut used = 0u64;
        let mut capacity = 0u64;
        for page in &self.pages {
            capacity += page.capacity as u64;
            if page.pending.as_ref().is_some_and(|pending| !matches!(
                pending.status(), MTLCommandBufferStatus::Completed | MTLCommandBufferStatus::Error
            )) {
                used += page.used as u64;
            }
        }
        (used, capacity)
    }
}

impl MetalDevice {
    pub(super) fn create_texture(&mut self, desc: &TextureDescriptor) -> Result<TextureId> {
        desc.validate()?;
        if desc.dimension != TextureDimension::D2 || desc.sample_count != 1 {
            return Err(Error::Unavailable(
                "Metal currently supports single-sample 2D textures only".into(),
            ));
        }
        if desc.size.width() > 16_384 || desc.size.height() > 16_384 {
            return Err(Error::InvalidInput("Metal 2D texture exceeds 16384 texels".into()));
        }
        let native = MTLTextureDescriptor::new();
        native.setTextureType(MTLTextureType::Type2D);
        native.setPixelFormat(format_to_metal(desc.format));
        // SAFETY: The portable descriptor validates the complete mip chain,
        // and the dimensions above are bounded for single-sample 2D textures.
        unsafe {
            native.setWidth(desc.size.width() as usize);
            native.setHeight(desc.size.height() as usize);
            native.setMipmapLevelCount(desc.mip_level_count as usize);
        }
        native.setStorageMode(if desc.memory_location == MemoryLocation::GpuOnly {
            MTLStorageMode::Private
        } else if cfg!(target_os = "macos") && !self.device.hasUnifiedMemory() {
            MTLStorageMode::Managed
        } else {
            MTLStorageMode::Shared
        });
        let mut usage = MTLTextureUsage::empty();
        if desc.usage.contains(TextureUsage::SAMPLED) {
            usage |= MTLTextureUsage::ShaderRead;
        }
        if desc.usage.contains(TextureUsage::STORAGE) {
            usage |= MTLTextureUsage::ShaderRead | MTLTextureUsage::ShaderWrite;
        }
        if desc.usage.intersects(TextureUsage::COLOR_ATTACHMENT | TextureUsage::DEPTH_ATTACHMENT) {
            usage |= MTLTextureUsage::RenderTarget;
        }
        native.setUsage(usage);
        let resource = self.device.newTextureWithDescriptor(&native)
            .ok_or_else(|| Error::Backend("failed to allocate Metal texture".into()))?;
        Ok(self.textures.insert(MetalTexture { desc: desc.clone(), resource: Some(resource) }))
    }

    pub(super) fn create_texture_view(&mut self, desc: &TextureViewDescriptor) -> Result<TextureViewId> {
        let texture = self.textures.get(desc.texture)?;
        desc.validate_against(&texture.desc)?;
        let parent = texture.resource.as_ref()
            .ok_or_else(|| Error::Unavailable("Metal texture has no native resource".into()))?;
        let resource = if desc.base_mip_level == 0 && desc.mip_level_count == texture.desc.mip_level_count {
            parent.clone()
        } else {
            // SAFETY: The descriptor validates the mip range and the texture
            // is single-sample 2D, with exactly one array slice.
            unsafe {
                parent.newTextureViewWithPixelFormat_textureType_levels_slices(
                    format_to_metal(desc.format), MTLTextureType::Type2D,
                    NSRange { location: desc.base_mip_level as usize, length: desc.mip_level_count as usize },
                    NSRange { location: 0, length: 1 },
                )
            }.ok_or_else(|| Error::Backend("failed to create Metal texture mip view".into()))?
        };
        Ok(self.texture_views.insert(MetalTextureView { texture: desc.texture, format: desc.format, resource }))
    }

    pub(super) fn write_texture(&mut self, desc: TextureWriteDescriptor, data: &[u8]) -> Result<()> {
        let texture = self.textures.get(desc.texture)?;
        desc.validate_against(&texture.desc, data.len())?;
        if !texture.desc.usage.contains(TextureUsage::COPY_DST) || texture.desc.format.is_depth() {
            return Err(Error::InvalidInput("Metal texture upload requires a non-depth COPY_DST texture".into()));
        }
        let destination = texture.resource.clone()
            .ok_or_else(|| Error::Unavailable("Metal texture has no native resource".into()))?;
        let width = desc.size.width() as usize;
        let height = desc.size.height() as usize;
        let row_bytes = width.checked_mul(texture.desc.format.bytes_per_pixel() as usize)
            .ok_or_else(|| Error::InvalidInput("Metal upload row size overflow".into()))?;
        // A dedicated shared staging buffer avoids CPU writes to textures
        // still sampled by an earlier GPU submission.
        let pitch = row_bytes.checked_add(255).map(|n| n & !255)
            .ok_or_else(|| Error::InvalidInput("Metal upload pitch overflow".into()))?;
        let length = pitch.checked_mul(height)
            .ok_or_else(|| Error::InvalidInput("Metal upload size overflow".into()))?;
        let offset = usize::try_from(desc.layout.offset)
            .map_err(|_| Error::InvalidInput("Metal upload offset overflow".into()))?;
        let source_pitch = desc.layout.bytes_per_row.get() as usize;
        let (staging, page) = self.texture_uploads.acquire(&self.device, length)?;
        // SAFETY: validate_against covers every source row. The staging allocation
        // is CPU-visible, has at least pitch * height bytes, and is not in flight.
        unsafe {
            let ptr = staging.contents().as_ptr().cast::<u8>();
            for row in 0..height {
                std::ptr::copy_nonoverlapping(
                    data.as_ptr().add(offset + row * source_pitch), ptr.add(row * pitch), row_bytes,
                );
            }
        }
        let commands = self.command_queue.commandBuffer()
            .ok_or_else(|| Error::Backend("Metal upload command buffer unavailable".into()))?;
        let blit = commands.blitCommandEncoder()
            .ok_or_else(|| Error::Backend("Metal upload blit encoder unavailable".into()))?;
        // SAFETY: All source and destination ranges were checked. The default
        // commandBuffer retains referenced resources until it completes; the
        // same ordered queue serializes earlier sampling and subsequent draws.
        unsafe {
            blit.copyFromBuffer_sourceOffset_sourceBytesPerRow_sourceBytesPerImage_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                &staging, 0, pitch, 0,
                MTLSize { width, height, depth: 1 },
                &destination, 0, desc.mip_level as usize,
                MTLOrigin { x: desc.origin.x as usize, y: desc.origin.y as usize, z: 0 },
            );
        }
        blit.endEncoding();
        commands.commit();
        self.texture_uploads.submitted(page, length, commands);
        Ok(())
    }

    pub(super) fn create_sampler(&mut self, desc: &SamplerDescriptor) -> Result<SamplerId> {
        desc.validate()?;
        if !desc.lod_min_clamp.is_finite() || !desc.lod_max_clamp.is_finite() || desc.lod_min_clamp < 0.0 {
            return Err(Error::InvalidInput("Metal sampler requires finite non-negative LOD clamps".into()));
        }
        // Native sampler LOD bias is not available on all supported OS versions.
        if desc.lod_bias != 0.0 {
            return Err(Error::Unavailable("Metal portable sampler LOD bias is unsupported".into()));
        }
        let native = MTLSamplerDescriptor::new();
        if let Some(label) = &desc.label {
            native.setLabel(Some(&NSString::from_str(label)));
        }
        native.setMinFilter(filter(desc.min_filter));
        native.setMagFilter(filter(desc.mag_filter));
        native.setMipFilter(match desc.mipmap_filter {
            FilterMode::Nearest => MTLSamplerMipFilter::Nearest,
            FilterMode::Linear => MTLSamplerMipFilter::Linear,
        });
        native.setSAddressMode(address(desc.address_mode_u));
        native.setTAddressMode(address(desc.address_mode_v));
        native.setRAddressMode(address(desc.address_mode_w));
        native.setLodMinClamp(desc.lod_min_clamp);
        native.setLodMaxClamp(desc.lod_max_clamp);
        if desc.anisotropic && desc.min_filter == FilterMode::Linear && desc.mag_filter == FilterMode::Linear {
            native.setMaxAnisotropy(16);
        }
        if let Some(compare) = desc.compare {
            native.setCompareFunction(compare_to_metal(compare));
        }
        if [desc.address_mode_u, desc.address_mode_v, desc.address_mode_w].contains(&AddressMode::ClampToBorder) {
            native.setBorderColor(match desc.border_color.unwrap_or([0.0; 4]) {
                [0.0, 0.0, 0.0, 0.0] => MTLSamplerBorderColor::TransparentBlack,
                [0.0, 0.0, 0.0, 1.0] => MTLSamplerBorderColor::OpaqueBlack,
                [1.0, 1.0, 1.0, 1.0] => MTLSamplerBorderColor::OpaqueWhite,
                _ => return Err(Error::Unavailable("Metal supports only three native sampler border colors".into())),
            });
        }
        let resource = self.device.newSamplerStateWithDescriptor(&native)
            .ok_or_else(|| Error::Backend("failed to create Metal sampler".into()))?;
        Ok(self.samplers.insert(MetalSampler { resource }))
    }
}

fn filter(mode: FilterMode) -> MTLSamplerMinMagFilter {
    match mode {
        FilterMode::Nearest => MTLSamplerMinMagFilter::Nearest,
        FilterMode::Linear => MTLSamplerMinMagFilter::Linear,
    }
}

fn address(mode: AddressMode) -> MTLSamplerAddressMode {
    match mode {
        AddressMode::ClampToEdge => MTLSamplerAddressMode::ClampToEdge,
        AddressMode::Repeat => MTLSamplerAddressMode::Repeat,
        AddressMode::MirrorRepeat => MTLSamplerAddressMode::MirrorRepeat,
        AddressMode::ClampToBorder => MTLSamplerAddressMode::ClampToBorderColor,
    }
}
