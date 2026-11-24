use gfx_core::*;
use gpui::ExtensionDevice;

pub(super) struct Target {
    pub texture: TextureId,
    pub view: TextureViewId,
}
/// Multisampled color attachments with single-sample resolve destinations.
/// The two depth attachments and albedo are sampled at their original sample count.
pub(super) struct MsaaTargets {
    pub color: Target,
    pub surface: Target,
}
pub(super) struct Targets {
    pub extent: Extent2d,
    pub color: Target,
    pub depth: Target,
    pub surface: Target,
    pub surface_depth: Target,
    pub albedo: Target,
    pub resolved: Target,
    pub msaa: Option<MsaaTargets>,
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
        sample_count: u32,
    ) -> gpui::Result<Self> {
        Self::create_with_hint(device, extent, format, depth, sample_count, TextureUsage::empty())
    }

    fn create_with_hint(
        device: &mut dyn ExtensionDevice,
        extent: Extent2d,
        format: Format,
        depth: bool,
        sample_count: u32,
        hint: TextureUsage,
    ) -> gpui::Result<Self> {
        let texture = device.create_texture(&TextureDescriptor {
            label: Some("gpui-3d effect target".into()),
            size: extent,
            mip_level_count: 1,
            format,
            usage: if hint.contains(TextureUsage::TRANSIENT_ATTACHMENT) {
                hint | TextureUsage::COLOR_ATTACHMENT
            } else {
                hint | TextureUsage::SAMPLED | TextureUsage::COPY_SRC
                    | if depth { TextureUsage::DEPTH_ATTACHMENT } else { TextureUsage::COLOR_ATTACHMENT }
            },
            memory_location: MemoryLocation::GpuOnly,
            dimension: TextureDimension::D2,
            sample_count,
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
    pub(super) fn create(
        device: &mut dyn ExtensionDevice,
        extent: Extent2d,
        msaa_sample_count: u32,
        backend: BackendKind,
    ) -> gpui::Result<Self> {
        let mut cleanup = super::construction::Construction::default();
        let result = (|| {
            let mut create = |format, samples, hint| -> gpui::Result<Target> {
                let target = Target::create_with_hint(
                    device,
                    extent,
                    format,
                    format == Format::Depth32Float,
                    samples,
                    hint,
                )?;
                target.record(&mut cleanup);
                Ok(target)
            };
            let msaa_hint = TextureUsage::DISCARD_AFTER_RESOLVE | if matches!(backend, BackendKind::Vulkan | BackendKind::Metal) {
                TextureUsage::TRANSIENT_ATTACHMENT
            } else { TextureUsage::empty() };
            Ok(Self {
                extent,
                color: create(Format::Rgba16Float, 1, TextureUsage::empty())?,
                depth: create(Format::Depth32Float, msaa_sample_count, TextureUsage::empty())?,
                surface: create(Format::Rgba16Float, 1, TextureUsage::empty())?,
                surface_depth: create(Format::Depth32Float, msaa_sample_count, TextureUsage::empty())?,
                albedo: create(Format::Rgba16Float, msaa_sample_count, TextureUsage::empty())?,
                resolved: create(Format::Rgba8Unorm, 1, TextureUsage::empty())?,
                msaa: if msaa_sample_count > 1 {
                    Some(MsaaTargets {
                        // Only these two sources are resolved and never sampled.
                        // Depth and albedo remain live at their original sample count.
                        color: create(Format::Rgba16Float, msaa_sample_count, msaa_hint)?,
                        surface: create(Format::Rgba16Float, msaa_sample_count, msaa_hint)?,
                    })
                } else {
                    None
                },
            })
        })();
        cleanup.finish(device, result)
    }
    pub(super) fn record(&self, cleanup: &mut super::construction::Construction) {
        for target in [
            &self.color,
            &self.depth,
            &self.surface,
            &self.surface_depth,
            &self.albedo,
            &self.resolved,
        ] {
            target.record(cleanup);
        }
        if let Some(msaa) = &self.msaa {
            msaa.color.record(cleanup);
            msaa.surface.record(cleanup);
        }
    }
    pub(super) fn destroy(self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        if let Some(msaa) = self.msaa {
            for target in [msaa.color, msaa.surface] {
                target.destroy(device)?;
            }
        }
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
