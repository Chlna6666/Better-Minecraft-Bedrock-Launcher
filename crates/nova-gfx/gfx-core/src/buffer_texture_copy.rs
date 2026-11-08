//! Buffer <-> texture copies.
//!
//! Row pitch and placement alignment are device properties, not constants: each backend
//! reports its own requirement through [`TextureCopyLimits`], and callers size buffers with
//! [`texture_copy_buffer_bytes`] so no fixed alignment leaks into the portable API.

use crate::{
    BufferDescriptor, BufferId, BufferUsage, Error, Extent2d, Format, Origin2d, Result, TextureId,
    TextureUsage,
};

/// Device-specific alignment for buffer <-> texture copies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextureCopyLimits {
    /// Buffer rows must start at a multiple of this many bytes.
    ///
    /// One means the backend accepts a tightly packed row.
    pub row_pitch_alignment: u64,
    /// The copied buffer range must start at a multiple of this many bytes.
    pub buffer_offset_alignment: u64,
}

impl TextureCopyLimits {
    /// Tightly packed rows with unaligned offsets: valid for backends without padding rules.
    pub const TIGHT: Self = Self {
        row_pitch_alignment: 1,
        buffer_offset_alignment: 1,
    };
}

impl Default for TextureCopyLimits {
    fn default() -> Self {
        Self::TIGHT
    }
}

/// Direction of one buffer <-> texture copy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CopyDirection {
    /// The buffer supplies pixels for the texture.
    BufferToTexture,
    /// The texture writes pixels into the buffer.
    TextureToBuffer,
}

/// One buffer <-> texture copy of a tightly row-pitched rectangle.
///
/// The row pitch is not stored: it follows from the device alignment, the copy width and the
/// texture format through [`texture_copy_row_pitch`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextureBufferCopy {
    /// Live buffer, created with `COPY_SRC` (texture to buffer) or `COPY_DST` (buffer to texture).
    pub buffer: BufferId,
    /// Live texture, created with the matching `COPY_*` usage.
    pub texture: TextureId,
    /// Which side supplies the pixels.
    pub direction: CopyDirection,
    /// Byte offset of the first row inside the buffer.
    pub buffer_offset: u64,
    /// Top-left texel of the copied texture rectangle.
    pub texture_origin: Origin2d,
    /// Size of the copied rectangle in texels.
    pub size: Extent2d,
}

impl TextureBufferCopy {
    /// Checks usages, the rectangle, and the buffer range against the device alignment.
    ///
    /// # Errors
    /// Returns [`Error::InvalidInput`] for a depth format, a missing copy usage, an
    /// out-of-bounds rectangle, a misaligned offset, or a buffer too small for the rows.
    pub fn validate(
        &self,
        texture: &crate::TextureDescriptor,
        buffer: &BufferDescriptor,
        limits: TextureCopyLimits,
    ) -> Result<()> {
        if texture.format.is_depth() {
            return Err(Error::InvalidInput(
                "buffer copies do not support depth formats".into(),
            ));
        }
        if texture.sample_count != 1 {
            return Err(Error::InvalidInput(
                "buffer copies require a single-sampled texture; resolve MSAA first".into(),
            ));
        }
        if texture.dimension != crate::TextureDimension::D2 {
            return Err(Error::InvalidInput(
                "buffer copies support only 2D textures".into(),
            ));
        }
        let width = texture.size.width();
        let height = texture.size.height();
        let x = u32::try_from(self.texture_origin.x)
            .map_err(|_| Error::InvalidInput("texture origin x is negative".into()))?;
        let y = u32::try_from(self.texture_origin.y)
            .map_err(|_| Error::InvalidInput("texture origin y is negative".into()))?;
        if x.checked_add(self.size.width())
            .is_none_or(|right| right > width)
            || y.checked_add(self.size.height())
                .is_none_or(|bottom| bottom > height)
        {
            return Err(Error::InvalidInput(
                "buffer copy rectangle exceeds the texture extent".into(),
            ));
        }
        let required_usage = match self.direction {
            CopyDirection::BufferToTexture => TextureUsage::COPY_DST,
            CopyDirection::TextureToBuffer => TextureUsage::COPY_SRC,
        };
        if !texture.usage.contains(required_usage) {
            return Err(Error::InvalidInput(
                "texture lacks the copy usage for this direction".into(),
            ));
        }
        let required_buffer_usage = match self.direction {
            CopyDirection::BufferToTexture => BufferUsage::COPY_SRC,
            CopyDirection::TextureToBuffer => BufferUsage::COPY_DST,
        };
        if !buffer.usage.contains(required_buffer_usage) {
            return Err(Error::InvalidInput(
                "buffer lacks the copy usage for this direction".into(),
            ));
        }
        if limits.buffer_offset_alignment > 1
            && self.buffer_offset % limits.buffer_offset_alignment != 0
        {
            return Err(Error::InvalidInput(format!(
                "buffer offset {} must be a multiple of the device alignment {}",
                self.buffer_offset, limits.buffer_offset_alignment
            )));
        }
        let end = self
            .buffer_offset
            .checked_add(texture_copy_buffer_bytes(self.size, texture.format, limits))
            .ok_or_else(|| Error::InvalidInput("buffer copy range overflows u64".into()))?;
        if end > buffer.size {
            return Err(Error::InvalidInput(format!(
                "buffer copy needs {end} bytes but the buffer holds {}",
                buffer.size
            )));
        }
        Ok(())
    }
}

/// Bytes per row a backend expects for `width` texels of `format`.
///
/// Callers pass the device alignment from [`crate::ResourceDevice::texture_copy_limits`], so a
/// tightly packed destination is never assumed for a backend that requires padding.
#[must_use]
pub fn texture_copy_row_pitch(width: u32, format: Format, limits: TextureCopyLimits) -> u64 {
    let row = u64::from(width) * u64::from(format.bytes_per_pixel());
    let alignment = limits.row_pitch_alignment.max(1);
    row.div_ceil(alignment) * alignment
}

/// Bytes a buffer must reserve to receive one copy rectangle.
#[must_use]
pub fn texture_copy_buffer_bytes(size: Extent2d, format: Format, limits: TextureCopyLimits) -> u64 {
    texture_copy_row_pitch(size.width(), format, limits) * u64::from(size.height())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_texture_copy_rejects_msaa_instead_of_relying_on_backend_failure() {
        let texture = crate::TextureDescriptor {
            label: None,
            size: Extent2d::new(4, 4).unwrap(),
            mip_level_count: 1,
            format: Format::Rgba8Unorm,
            usage: TextureUsage::COPY_DST,
            memory_location: crate::MemoryLocation::GpuOnly,
            dimension: crate::TextureDimension::D2,
            sample_count: 4,
        };
        let buffer = BufferDescriptor {
            label: None,
            size: 256,
            usage: BufferUsage::COPY_SRC,
            memory_location: crate::MemoryLocation::GpuOnly,
        };
        let copy = TextureBufferCopy {
            buffer: crate::BufferId::new(1),
            texture: crate::TextureId::new(2),
            direction: CopyDirection::BufferToTexture,
            buffer_offset: 0,
            texture_origin: Origin2d::ZERO,
            size: texture.size,
        };
        assert!(copy.validate(&texture, &buffer, TextureCopyLimits::TIGHT).is_err());
    }
}
