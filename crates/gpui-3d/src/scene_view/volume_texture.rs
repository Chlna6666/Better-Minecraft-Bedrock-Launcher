//! Cross-backend native 3D GPU texture allocation for scene-view effects.
//!
//! This intentionally has no CPU texel storage, per-frame staging, 2D-array
//! emulation, or render-thread locks. Callers retain the two native handles
//! and retire them through the existing GPUI GPU-owner lifetime protocol.

use gfx_core::{
    Extent2d, Format, MemoryLocation, TextureDescriptor, TextureDimension, TextureId,
    TextureUsage, TextureViewDescriptor, TextureViewId,
};
use gpui::ExtensionDevice;

/// Allocate one true native 3D image with a full-depth sampled/storage view.
///
/// The GPU owner is responsible for recording its native resource transitions.
/// A failure never leaks a texture after view creation fails.
pub(super) fn create_gpu_volume(
    device: &mut dyn ExtensionDevice,
    label: &str,
    extent: [u32; 3],
    format: Format,
    usage: TextureUsage,
    mip_level_count: u32,
) -> gpui::Result<(TextureId, TextureViewId)> {
    let descriptor = volume_descriptor(label, extent, format, usage, mip_level_count)?;
    device.texture_volume_capabilities().validate(&descriptor)?;

    let texture = device.create_texture(&descriptor)?;
    let view = match device.create_texture_view(&TextureViewDescriptor {
        label: Some(label.into()),
        texture,
        base_mip_level: 0,
        mip_level_count,
        format,
    }) {
        Ok(view) => view,
        Err(error) => {
            let _ = device.destroy_texture(texture);
            return Err(error.into());
        }
    };
    Ok((texture, view))
}

/// Build the common volume descriptor for Froxel, 3D LUT and future 3D
/// material/lighting caches. A real D3 resource has one image layer and a
/// separate Z dimension; it must not alias a D2Array descriptor.
pub(super) fn volume_descriptor(
    label: &str,
    extent: [u32; 3],
    format: Format,
    usage: TextureUsage,
    mip_level_count: u32,
) -> gpui::Result<TextureDescriptor> {
    let descriptor = TextureDescriptor {
        label: Some(label.into()),
        size: Extent2d::new(extent[0], extent[1])?,
        mip_level_count,
        format,
        usage,
        memory_location: MemoryLocation::GpuOnly,
        dimension: TextureDimension::D3 { depth: extent[2] },
        sample_count: 1,
    };
    descriptor.validate()?;
    Ok(descriptor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_is_native_3d_not_an_array_of_2d_slices() {
        let desc = volume_descriptor(
            "gpu-volume", [128, 72, 64], Format::Rgba16Float,
            TextureUsage::SAMPLED | TextureUsage::STORAGE, 1,
        ).unwrap();
        assert_eq!(desc.dimension, TextureDimension::D3 { depth: 64 });
        assert_eq!(desc.dimension.layers(), 1);
        assert_eq!(desc.byte_size(), 128 * 72 * 64 * 8);
    }

    #[test]
    fn volume_rejects_legacy_2d_attachment_or_invalid_mip_usage() {
        assert!(volume_descriptor(
            "invalid", [64, 64, 64], Format::Rgba16Float,
            TextureUsage::COLOR_ATTACHMENT, 1,
        ).is_err());
        assert!(volume_descriptor(
            "invalid", [64, 64, 0], Format::Rgba16Float,
            TextureUsage::STORAGE, 1,
        ).is_err());
        assert!(volume_descriptor(
            "invalid", [64, 64, 64], Format::Rgba16Float,
            TextureUsage::STORAGE, 2,
        ).is_err());
    }
}
