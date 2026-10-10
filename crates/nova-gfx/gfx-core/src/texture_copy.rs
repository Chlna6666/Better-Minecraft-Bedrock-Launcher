use crate::{
    Error, Extent2d, Format, Origin2d, Result, TextureDescriptor, TextureId, TextureUsage,
};

/// GPU-only pixel relocation between distinct, format-identical color 2D textures.
/// Coordinates address mip zero; no scaling, conversion, or CPU readback occurs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextureCopy {
    /// Live source texture, created with COPY_SRC.
    pub source: TextureId,
    /// Live destination texture, created with COPY_DST.
    pub destination: TextureId,
    /// Top-left pixel of the source rectangle.
    pub source_origin: Origin2d,
    /// Top-left pixel of the destination rectangle.
    pub destination_origin: Origin2d,
    /// Width and height copied without format conversion.
    pub size: Extent2d,
}

impl TextureCopy {
    /// Checks usage, format, and both rectangles before recording native commands.
    ///
    /// # Errors
    /// Returns InvalidInput for aliasing textures, depth formats, incompatible usages or out-of-bounds
    /// rectangles. Same-texture copies are deliberately excluded to avoid overlapping hazards.
    pub fn validate(
        &self,
        source: &TextureDescriptor,
        destination: &TextureDescriptor,
    ) -> Result<()> {
        if self.source == self.destination
            || source.format != destination.format
            || source.format == Format::Depth32Float
            || !source.usage.contains(TextureUsage::COPY_SRC)
            || !destination.usage.contains(TextureUsage::COPY_DST)
        {
            return Err(Error::InvalidInput(
                "texture copy requires distinct, matching color COPY_SRC/COPY_DST textures".into(),
            ));
        }
        for (origin, texture) in [
            (self.source_origin, source),
            (self.destination_origin, destination),
        ] {
            if origin
                .x
                .checked_add(self.size.width())
                .is_none_or(|end| end > texture.size.width())
                || origin
                    .y
                    .checked_add(self.size.height())
                    .is_none_or(|end| end > texture.size.height())
            {
                return Err(Error::InvalidInput(
                    "texture copy rectangle exceeds texture".into(),
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemoryLocation, TextureDimension};

    fn descriptor(format: Format) -> TextureDescriptor {
        TextureDescriptor {
            label: None,
            size: Extent2d::new(8, 8).expect("extent"),
            mip_level_count: 1,
            format,
            usage: TextureUsage::COPY_SRC | TextureUsage::COPY_DST,
            memory_location: MemoryLocation::GpuOnly,
            dimension: TextureDimension::D2,
        }
    }

    #[test]
    fn validates_boundaries_aliasing_usage_and_color_format() {
        let mut copy = TextureCopy {
            source: TextureId::from_parts(1, 0),
            destination: TextureId::from_parts(2, 0),
            source_origin: Origin2d { x: 6, y: 6 },
            destination_origin: Origin2d::ZERO,
            size: Extent2d::new(2, 2).expect("extent"),
        };
        let source = descriptor(Format::R8Unorm);
        assert!(copy.validate(&source, &source).is_ok());
        copy.source_origin.x = u32::MAX;
        assert!(copy.validate(&source, &source).is_err());
        copy.source_origin = Origin2d::ZERO;
        copy.destination = copy.source;
        assert!(copy.validate(&source, &source).is_err());
        copy.destination = TextureId::from_parts(2, 0);
        assert!(
            copy.validate(&source, &descriptor(Format::Bgra8Unorm))
                .is_err()
        );
        let depth = descriptor(Format::Depth32Float);
        assert!(copy.validate(&depth, &depth).is_err());
        let mut destination = source.clone();
        destination.usage = TextureUsage::SAMPLED;
        assert!(copy.validate(&source, &destination).is_err());
    }
}
