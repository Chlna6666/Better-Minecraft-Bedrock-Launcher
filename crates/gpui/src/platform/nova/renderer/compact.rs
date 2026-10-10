use super::*;

pub(in crate::platform::nova) fn compact_atlas<D: BackendResources>(
    atlas: &NovaAtlas,
    textures: &mut FxHashMap<AtlasTextureId, NovaGpuAtlasTexture>,
    device: &mut D,
    label: &str,
    descriptor: &AtlasResourceDescriptor,
) -> Result<bool> {
    atlas.compact_page(|plan| {
        let source = textures
            .get(&plan.source.id)
            .context("compact source page is not resident")?;
        let created = if textures.contains_key(&plan.destination.id) {
            None
        } else {
            Some(create_atlas_texture_resources(
                device,
                label,
                plan.destination.id,
                plan.destination.size,
                descriptor,
                NovaAtlasResourceSetMode::UsedByTextureKind,
            )?)
        };
        let destination = created
            .as_ref()
            .or_else(|| textures.get(&plan.destination.id))
            .context("compact destination page is not resident")?;
        let copies = plan
            .tiles
            .iter()
            .map(|(source_tile, destination_tile)| {
                let padding = source_tile.padding as i32;
                Ok(gfx_core::TextureCopy {
                    source: source.texture,
                    destination: destination.texture,
                    source_origin: gfx_core::Origin2d {
                        x: (source_tile.bounds.origin.x.0 - padding) as u32,
                        y: (source_tile.bounds.origin.y.0 - padding) as u32,
                    },
                    destination_origin: gfx_core::Origin2d {
                        x: (destination_tile.bounds.origin.x.0 - padding) as u32,
                        y: (destination_tile.bounds.origin.y.0 - padding) as u32,
                    },
                    size: Extent2d::new(
                        (source_tile.bounds.size.width.0 + 2 * padding) as u32,
                        (source_tile.bounds.size.height.0 + 2 * padding) as u32,
                    )?,
                })
            })
            .collect::<gfx_core::Result<Vec<_>>>();
        let copies = match copies {
            Ok(copies) => copies,
            Err(error) => {
                if let Some(destination) = created {
                    destroy_gpu_atlas_texture(device, destination, label, plan.destination.id);
                }
                return Err(error.into());
            }
        };
        if let Err(error) = device.copy_texture_batch(&copies) {
            if let Some(destination) = created {
                destroy_gpu_atlas_texture(device, destination, label, plan.destination.id);
            }
            return Err(error.into());
        }
        // Native backend retirement orders source destruction after this queued copy. The
        // stable logical tile map switches only when this callback has succeeded.
        if let Some(destination) = created {
            textures.insert(plan.destination.id, destination);
        }
        Ok(())
    })
}

impl NovaRenderer {
    pub(super) fn compact_atlas_page(&mut self) -> Result<bool> {
        self.sync_atlas_textures_for_current_backend()?;
        let descriptor = self.atlas_resource_descriptor();
        let label = self.backend_info.label();
        let mut backend = lock_backend(&self.backend);
        macro_rules! compact {
            ($device:expr) => {
                compact_atlas(
                    &self.atlas,
                    &mut self.gpu_atlas_textures,
                    $device,
                    label,
                    &descriptor,
                )
            };
        }
        #[allow(unreachable_patterns)]
        match &mut *backend {
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            NovaBackend::Dx11(device) => compact!(device),
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => compact!(device),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => compact!(device),
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            NovaBackend::OpenGl(device) => compact!(device),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => compact!(device),
            _ => Ok(false),
        }
    }
}
