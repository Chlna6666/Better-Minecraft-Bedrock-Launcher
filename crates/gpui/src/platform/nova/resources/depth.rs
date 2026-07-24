use anyhow::{Context as _, Result};

use super::super::*;

pub(in crate::platform::nova) fn create_depth_texture<D>(
    device: &mut D,
    label: &str,
    size: Extent2d,
) -> Result<TextureId>
where
    D: BackendResources,
{
    device
        .create_texture(&TextureDescriptor {
            label: Some(format!("{label} depth texture")),
            size,
            format: Format::Depth32Float,
            usage: TextureUsage::DEPTH_ATTACHMENT,
            memory_location: MemoryLocation::GpuOnly,
            dimension: TextureDimension::D2,
        })
        .context("creating nova depth texture")
}
