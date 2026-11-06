//! Device-reported 2D-array texture limits and backend-neutral allocation planning.
//!
//! Physical hardware limits do not imply the API abstraction has implemented
//! array creation, layer upload, or indexed shader sampling. Keep the three
//! implementation capabilities separate until all native operations exist.

use crate::{Error, Extent2d, Format, Origin2d, Result, TextureDataLayout, TextureDescriptor, TextureDimension, TextureId};

/// Limits of the selected graphics device, plus availability of the complete
/// layered-texture path in the current backend implementation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TextureArrayCapabilities {
    /// Maximum width or height of one 2D slice.
    pub max_dimension_2d: u32,
    /// Maximum number of slices in a 2D image array.
    pub max_array_layers: u32,
    /// The backend can allocate and create a sampled D2Array view.
    pub array_resources: bool,
    /// The backend can upload to an independently addressed array layer.
    pub layer_uploads: bool,
    /// Generated-mesh shader translation can sample a dynamic array index.
    pub indexed_sampling: bool,
}

/// Validated homogeneous 2D texture array layout. Its mip chain repeats for
/// every layer, unlike a vertical strip atlas.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextureArrayPlan {
    pub width: u32,
    pub height: u32,
    pub layers: u32,
    pub mip_levels: u32,
    pub total_bytes: u64,
}

/// One independently uploaded subresource of a homogeneous 2D texture array.
/// Mip and layer addressing are explicit instead of hiding the layer as an
/// assumed third coordinate in an ordinary D2 write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextureArrayLayerUpload {
    pub layer: u32,
    pub mip_level: u32,
    pub origin: Origin2d,
    pub extent: Extent2d,
    pub layout: TextureDataLayout,
}

/// Upload a single slice of a texture-array resource.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextureArrayLayerWriteDescriptor {
    pub texture: TextureId,
    pub upload: TextureArrayLayerUpload,
}

impl TextureArrayLayerWriteDescriptor {
    /// Validate the native subresource index and source pitch against the
    /// *actual* live texture descriptor, not a caller-supplied plan.
    pub fn validate_against(
        self,
        descriptor: &TextureDescriptor,
        capabilities: TextureArrayCapabilities,
        data_len: usize,
    ) -> Result<()> {
        descriptor.validate()?;
        let TextureDimension::D2Array { layers } = descriptor.dimension else {
            return Err(Error::InvalidInput("layered upload requires a D2Array texture".into()));
        };
        if !descriptor.usage.contains(crate::TextureUsage::COPY_DST) {
            return Err(Error::InvalidInput("layered upload requires COPY_DST".into()));
        }
        if !capabilities.array_resources || !capabilities.layer_uploads {
            return Err(Error::Unavailable("backend does not implement texture-array uploads".into()));
        }
        let plan = capabilities.plan(
            descriptor.size.width(), descriptor.size.height(), layers,
            descriptor.mip_level_count, descriptor.format,
        )?;
        plan.validate_layer_upload(self.upload, descriptor.format, data_len)
    }

    /// Return the D3D11/D3D12-style mip-major subresource address. Backends
    /// with other native storage layouts can use the same validated layer/mip.
    pub fn subresource_index(self, mip_levels: u32) -> Result<u32> {
        self.upload.layer.checked_mul(mip_levels)
            .and_then(|base| base.checked_add(self.upload.mip_level))
            .ok_or_else(|| Error::InvalidInput("texture array subresource index overflow".into()))
    }
}

impl TextureArrayPlan {
    /// Validate a layer/mip upload against the planned native allocation and
    /// the caller's byte slice, before any backend creates staging buffers.
    pub fn validate_layer_upload(
        self,
        upload: TextureArrayLayerUpload,
        format: Format,
        data_len: usize,
    ) -> Result<()> {
        if upload.layer >= self.layers || upload.mip_level >= self.mip_levels {
            return Err(Error::InvalidInput("texture array layer or mip is out of bounds".into()));
        }
        let mip_width = (self.width >> upload.mip_level).max(1);
        let mip_height = (self.height >> upload.mip_level).max(1);
        let end_x = upload.origin.x.checked_add(upload.extent.width());
        let end_y = upload.origin.y.checked_add(upload.extent.height());
        if end_x.is_none_or(|x| x > mip_width) || end_y.is_none_or(|y| y > mip_height) {
            return Err(Error::InvalidInput("texture array upload rectangle exceeds mip dimensions".into()));
        }
        let row_bytes = u64::from(upload.extent.width())
            .checked_mul(u64::from(format.bytes_per_pixel()))
            .ok_or_else(|| Error::InvalidInput("texture array row byte count overflows".into()))?;
        let row_pitch = u64::from(upload.layout.bytes_per_row.get());
        if row_pitch < row_bytes || upload.layout.rows_per_image.get() < upload.extent.height() {
            return Err(Error::InvalidInput("texture array upload pitch or row count is too small".into()));
        }
        let size = upload.layout.offset
            .checked_add(
                u64::from(upload.extent.height().saturating_sub(1))
                    .checked_mul(row_pitch)
                    .ok_or_else(|| Error::InvalidInput("texture array upload row stride overflows".into()))?,
            )
            .and_then(|offset| offset.checked_add(row_bytes))
            .ok_or_else(|| Error::InvalidInput("texture array upload byte range overflows".into()))?;
        if size > data_len as u64 {
            return Err(Error::InvalidInput("texture array upload source data is truncated".into()));
        }
        Ok(())
    }
}

impl TextureArrayCapabilities {
    /// Whether the renderer may safely select an array-backed material path.
    /// Native support and shader implementation must both exist.
    #[must_use]
    pub const fn supports_generated_mesh_arrays(self) -> bool {
        self.array_resources && self.layer_uploads && self.indexed_sampling
    }

    /// Verify a candidate array against the physical device limits.
    /// A successful plan is *not* permission to enable the array-backed
    /// rendering path without checking supports_generated_mesh_arrays().
    pub fn plan(
        self,
        width: u32,
        height: u32,
        layers: u32,
        mip_levels: u32,
        format: Format,
    ) -> Result<TextureArrayPlan> {
        if self.max_dimension_2d == 0 || self.max_array_layers == 0 {
            return Err(Error::Unavailable("texture array limits were not reported".into()));
        }
        if width == 0 || height == 0 || layers == 0
            || width > self.max_dimension_2d
            || height > self.max_dimension_2d
            || layers > self.max_array_layers
        {
            return Err(Error::InvalidInput("texture array dimensions exceed device limits".into()));
        }
        let available_mips = u32::BITS - width.max(height).leading_zeros();
        if mip_levels == 0 || mip_levels > available_mips {
            return Err(Error::InvalidInput("texture array mip count is invalid".into()));
        }
        let mut bytes_per_layer = 0u64;
        for level in 0..mip_levels {
            let mip_width = u64::from((width >> level).max(1));
            let mip_height = u64::from((height >> level).max(1));
            let level_bytes = mip_width.checked_mul(mip_height)
                .and_then(|pixels| pixels.checked_mul(u64::from(format.bytes_per_pixel())))
                .ok_or_else(|| Error::InvalidInput("texture array mip size overflows".into()))?;
            bytes_per_layer = bytes_per_layer.checked_add(level_bytes)
                .ok_or_else(|| Error::InvalidInput("texture array byte count overflows".into()))?;
        }
        let total_bytes = bytes_per_layer.checked_mul(u64::from(layers))
            .ok_or_else(|| Error::InvalidInput("texture array byte count overflows".into()))?;
        Ok(TextureArrayPlan { width, height, layers, mip_levels, total_bytes })
    }

    /// Require a fully implemented native resource + shader path, not just a
    /// valid hardware limit. Failing closed prevents displaying incorrect
    /// texture layers when one backend has not implemented indexed sampling.
    pub fn plan_for_generated_mesh(
        self,
        width: u32,
        height: u32,
        layers: u32,
        mip_levels: u32,
        format: Format,
    ) -> Result<TextureArrayPlan> {
        if !self.supports_generated_mesh_arrays() {
            return Err(Error::Unavailable("indexed texture-array rendering is not implemented by this backend".into()));
        }
        self.plan(width, height, layers, mip_levels, format)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_limits_validate_mips_layers_and_overflow() {
        let limits = TextureArrayCapabilities {
            max_dimension_2d: 8192,
            max_array_layers: 256,
            ..Default::default()
        };
        let plan = limits.plan(16, 16, 2, 5, Format::Rgba8Unorm).unwrap();
        assert_eq!(plan.total_bytes, (256 + 64 + 16 + 4 + 1) * 4 * 2);
        assert!(limits.plan(16, 16, 257, 1, Format::Rgba8Unorm).is_err());
        assert!(limits.plan(8193, 16, 1, 1, Format::Rgba8Unorm).is_err());
        assert!(limits.plan(16, 16, 1, 6, Format::Rgba8Unorm).is_err());
        assert!(limits.plan(0, 16, 1, 1, Format::Rgba8Unorm).is_err());
        assert!(TextureArrayCapabilities::default()
            .plan(16, 16, 1, 1, Format::Rgba8Unorm).is_err());
    }

    #[test]
    fn layered_upload_validation_rejects_cross_mip_and_layer_writes() {
        let caps = TextureArrayCapabilities {
            max_dimension_2d: 1024,
            max_array_layers: 32,
            ..Default::default()
        };
        let plan = caps.plan(16, 16, 3, 5, Format::Rgba8Unorm).unwrap();
        let valid = TextureArrayLayerUpload {
            layer: 2,
            mip_level: 2,
            origin: Origin2d { x: 1, y: 1 },
            extent: Extent2d::new(3, 3).unwrap(),
            layout: TextureDataLayout::new(0, 12, 3).unwrap(),
        };
        assert!(plan.validate_layer_upload(valid, Format::Rgba8Unorm, 36).is_ok());
        assert!(plan.validate_layer_upload(TextureArrayLayerUpload {
            layer: 3, ..valid
        }, Format::Rgba8Unorm, 36).is_err());
        assert!(plan.validate_layer_upload(TextureArrayLayerUpload {
            mip_level: 5, ..valid
        }, Format::Rgba8Unorm, 36).is_err());
        assert!(plan.validate_layer_upload(TextureArrayLayerUpload {
            origin: Origin2d { x: 2, y: 2 }, ..valid
        }, Format::Rgba8Unorm, 36).is_err());
        assert!(plan.validate_layer_upload(valid, Format::Rgba8Unorm, 35).is_err());
        assert!(plan.validate_layer_upload(TextureArrayLayerUpload {
            layout: TextureDataLayout::new(0, 8, 3).unwrap(),
            ..valid
        }, Format::Rgba8Unorm, 36).is_err());
    }

    #[test]
    fn physical_array_support_does_not_enable_unimplemented_sampling() {
        let mut limits = TextureArrayCapabilities {
            max_dimension_2d: 16384,
            max_array_layers: 2048,
            ..Default::default()
        };
        assert!(limits.plan(16, 16, 64, 1, Format::Rgba8Unorm).is_ok());
        assert!(limits.plan_for_generated_mesh(16, 16, 64, 1, Format::Rgba8Unorm).is_err());
        limits.array_resources = true;
        limits.layer_uploads = true;
        assert!(!limits.supports_generated_mesh_arrays());
        limits.indexed_sampling = true;
        assert!(limits.plan_for_generated_mesh(16, 16, 64, 1, Format::Rgba8Unorm).is_ok());
    }
}
