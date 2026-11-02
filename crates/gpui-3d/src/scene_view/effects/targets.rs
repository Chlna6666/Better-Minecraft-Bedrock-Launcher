use gfx_core::*;
use gpui::ExtensionDevice;

pub(super) struct Target {
    pub texture: TextureId,
    pub view: TextureViewId,
}
pub(super) struct Targets {
    pub extent: Extent2d,
    pub color: Target,
    pub depth: Target,
    pub surface: Target,
    pub surface_depth: Target,
    pub albedo: Target,
    pub resolved: Target,
}
impl Target {
    pub(super) fn record(&self, cleanup: &mut super::construction::Construction) {
        cleanup.record(self.texture, |d, id| {
            let _ = d.destroy_texture(id);
        });
        cleanup.record(self.view, |d, id| {
            let _ = d.destroy_texture_view(id);
        });
    }
    pub(super) fn create(
        device: &mut dyn ExtensionDevice,
        extent: Extent2d,
        format: Format,
        depth: bool,
    ) -> gpui::Result<Self> {
        let texture = device.create_texture(&TextureDescriptor {
            label: Some("gpui-3d effect target".into()),
            size: extent,
            mip_level_count: 1,
            format,
            usage: TextureUsage::SAMPLED
                | TextureUsage::COPY_SRC
                | if depth {
                    TextureUsage::DEPTH_ATTACHMENT
                } else {
                    TextureUsage::COLOR_ATTACHMENT
                },
            memory_location: MemoryLocation::GpuOnly,
            dimension: TextureDimension::D2,
        })?;
        let view = match device.create_texture_view(&TextureViewDescriptor {
            label: None,
            texture,
            base_mip_level: 0,
            mip_level_count: 1,
            format,
        }) {
            Ok(view) => view,
            Err(error) => {
                let _ = device.destroy_texture(texture);
                return Err(error.into());
            }
        };
        Ok(Self { texture, view })
    }
    pub(super) fn destroy(self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        device.destroy_texture_view(self.view)?;
        device.destroy_texture(self.texture)?;
        Ok(())
    }
}
impl Targets {
    pub(super) fn create(device: &mut dyn ExtensionDevice, extent: Extent2d) -> gpui::Result<Self> {
        let mut targets = Vec::new();
        for format in [
            Format::Rgba16Float,
            Format::Depth32Float,
            Format::Rgba16Float,
            Format::Depth32Float,
            Format::Rgba16Float,
            Format::Rgba8Unorm,
        ] {
            match Target::create(device, extent, format, format == Format::Depth32Float) {
                Ok(target) => targets.push(target),
                Err(error) => {
                    for target in targets {
                        let _ = target.destroy(device);
                    }
                    return Err(error);
                }
            }
        }
        let mut targets = targets.into_iter();
        let mut next = || {
            targets
                .next()
                .ok_or_else(|| anyhow::anyhow!("missing screen target"))
        };
        Ok(Self {
            extent,
            color: next()?,
            depth: next()?,
            surface: next()?,
            surface_depth: next()?,
            albedo: next()?,
            resolved: next()?,
        })
    }
    pub(super) fn destroy(self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        for target in [
            self.color,
            self.depth,
            self.surface,
            self.surface_depth,
            self.albedo,
            self.resolved,
        ] {
            target.destroy(device)?;
        }
        Ok(())
    }
}
pub(super) fn uniform(
    device: &mut dyn ExtensionDevice,
    _backend: BackendKind,
) -> gpui::Result<BufferId> {
    Ok(device.create_buffer(&BufferDescriptor {
        label: Some("gpui-3d screen frame".into()),
        size: super::FRAME_BYTES,
        usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
        memory_location: MemoryLocation::GpuOnly,
    })?)
}
