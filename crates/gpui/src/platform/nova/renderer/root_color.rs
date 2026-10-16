//! Window-owned color residency. Filter topology never owns or retires these resources.
use super::*;
use crate::platform::nova::composite_target::DirectCompositeTarget;

#[derive(Default)]
pub(super) struct RootColor {
    pub(super) target: Option<RootColorTarget>,
    content_version: Option<u64>,
    next_version: u64,
}

pub(super) struct Descriptor {
    pub(super) size: Extent2d,
    pub(super) format: Format,
    pub(super) pass_layout: ResourceSetLayoutId,
    pub(super) composite_layout: ResourceSetLayoutId,
    pub(super) frames: Vec<FrameResourceBuffers>,
    pub(super) sampler: SamplerId,
}

impl RootColor {
    pub(super) fn source_target(&self, slot: usize) -> Option<DirectCompositeTarget> {
        let root = self.target.as_ref()?;
        Some(DirectCompositeTarget {
            texture_view: root.target.texture_view,
            source_resource_set: *root.pass_resource_sets.get(slot)?,
        })
    }

    pub(super) fn identity(&self) -> Option<(TextureId, TextureViewId)> {
        self.target
            .as_ref()
            .map(|root| (root.target.texture, root.target.texture_view))
    }

    pub(super) fn content_version(&self) -> Option<u64> {
        self.content_version
    }

    pub(super) fn resident_on(&self, identity: Option<(TextureId, TextureViewId)>) -> bool {
        self.content_version.is_some() && identity.is_some() && identity == self.identity()
    }

    pub(super) fn invalidate(&mut self) {
        self.content_version = None;
    }

    /// Publish only after successful submission; a filter-index move cannot import pixels.
    pub(super) fn record_present(
        &mut self,
        identity: Option<(TextureId, TextureViewId)>,
        refreshed: bool,
    ) {
        if identity.is_none() || identity != self.identity() {
            self.invalidate();
        } else if refreshed {
            self.next_version = self.next_version.wrapping_add(1).max(1);
            self.content_version = Some(self.next_version);
        }
    }

    /// Caller drains all window submissions before taking the allocation for retirement.
    pub(super) fn take_target(&mut self) -> Option<RootColorTarget> {
        self.invalidate();
        self.target.take()
    }
}

/// Resolve compositor bindings from their actual GPU owner. Root does not depend on a Filter slot.
pub(super) fn composite_resource_set(
    root: &RootColor,
    root_index: Option<u32>,
    filters: Option<&BackdropBlurTargets>,
    config: BackdropBlurConfig,
    slot: usize,
) -> Option<ResourceSetId> {
    if root_index.is_some_and(|index| config.exclusively_contains_member_index(index)) {
        root.target
            .as_ref()?
            .composite_resource_sets
            .get(slot)
            .copied()
    } else {
        filters?.resource_set_for_config(config, slot)
    }
}

pub(super) fn create_target<D: BackendResources + BackendPipelines>(
    device: &mut D,
    descriptor: Descriptor,
) -> Result<RootColorTarget> {
    let target = create_render_texture_target(
        device,
        "gpui window root color",
        descriptor.size,
        descriptor.format,
    )?;
    let mut pass_resource_sets = Vec::with_capacity(descriptor.frames.len());
    let mut composite_resource_sets = Vec::with_capacity(descriptor.frames.len());
    let result = (|| -> Result<()> {
        for buffers in &descriptor.frames {
            pass_resource_sets.push(device.create_resource_set(&ResourceSetDescriptor {
                label: Some("gpui root color source binding".into()),
                layout: descriptor.pass_layout,
                bindings: backdrop_blur_pass_resource_bindings(
                    buffers,
                    target.texture_view,
                    descriptor.sampler,
                ),
            })?);
            composite_resource_sets.push(device.create_resource_set(&ResourceSetDescriptor {
                label: Some("gpui root color composite binding".into()),
                layout: descriptor.composite_layout,
                bindings: backdrop_blur_resource_bindings(
                    buffers,
                    target.texture_view,
                    descriptor.sampler,
                ),
            })?);
        }
        Ok(())
    })();
    let root = RootColorTarget {
        target,
        pass_resource_sets,
        composite_resource_sets,
    };
    if let Err(error) = result {
        destroy_target(device, root);
        return Err(error);
    }
    Ok(root)
}

/// Retire descriptors before the allocation, after the window's GPU fences complete.
pub(super) fn destroy_target<D: BackendResources + BackendPipelines>(
    device: &mut D,
    root: RootColorTarget,
) {
    for set in root
        .composite_resource_sets
        .into_iter()
        .chain(root.pass_resource_sets)
    {
        if let Err(error) = device.destroy_resource_set(set) {
            log::debug!("failed to retire root color descriptor: {error}");
        }
    }
    destroy_render_texture_target(device, root.target, "window root color");
}

impl NovaRenderer {
    /// Called only after the window's pending submissions have drained.
    pub(super) fn retire_root_color(&mut self) {
        let Some(root) = self.root_color.take_target() else {
            return;
        };
        match &mut *lock_backend(&self.backend) {
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            NovaBackend::Dx11(device) => destroy_target(device, root),
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => destroy_target(device, root),
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            NovaBackend::OpenGl(device) => destroy_target(device, root),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => destroy_target(device, root),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => destroy_target(device, root),
            #[cfg(not(any(
                all(feature = "nova-gfx-dx11", target_os = "windows"),
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(
                    feature = "nova-gfx-opengl",
                    any(target_os = "windows", target_os = "linux")
                ),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                ),
                all(feature = "nova-gfx-metal", target_os = "macos")
            )))]
            NovaBackend::Unavailable => {}
        }
    }

    pub(super) fn ensure_root_color(&mut self) -> Result<()> {
        if self.frame_upload.root_color.is_none() || self.root_color.target.is_some() {
            return Ok(());
        }
        let descriptor = Descriptor {
            size: Extent2d::new(self.current_size.width, self.current_size.height)?,
            format: self.surface_format,
            pass_layout: self.backdrop_blur_pass_resource_set_layout,
            composite_layout: self.backdrop_blur_resource_set_layout,
            frames: self.frame_resource_buffers(),
            sampler: self.atlas_sampler,
        };
        let target = match &mut *lock_backend(&self.backend) {
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            NovaBackend::Dx11(device) => create_target(device, descriptor)?,
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => create_target(device, descriptor)?,
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            NovaBackend::OpenGl(device) => create_target(device, descriptor)?,
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => create_target(device, descriptor)?,
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => create_target(device, descriptor)?,
            #[cfg(not(any(
                all(feature = "nova-gfx-dx11", target_os = "windows"),
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(
                    feature = "nova-gfx-opengl",
                    any(target_os = "windows", target_os = "linux")
                ),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                ),
                all(feature = "nova-gfx-metal", target_os = "macos")
            )))]
            NovaBackend::Unavailable => anyhow::bail!("root color requires a Nova backend"),
        };
        self.root_color.target = Some(target);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(texture: u32) -> RootColorTarget {
        RootColorTarget {
            target: RenderTarget {
                byte_size: 256,
                texture: TextureId::from_parts(texture, 1),
                texture_view: TextureViewId::from_parts(texture, 1),
            },
            pass_resource_sets: vec![ResourceSetId::from_parts(20, 1)],
            composite_resource_sets: vec![ResourceSetId::from_parts(21, 1)],
        }
    }

    #[test]
    fn root_bindings_resolve_without_any_filter_target_chain() {
        let mut root = RootColor::default();
        root.target = Some(target(1));
        let config = test_zero_radius_blur_config(3);
        assert_eq!(
            composite_resource_set(&root, Some(3), None, config, 0),
            Some(ResourceSetId::from_parts(21, 1))
        );
        assert_eq!(
            root.source_target(0).unwrap().source_resource_set,
            ResourceSetId::from_parts(20, 1)
        );
        assert!(composite_resource_set(&root, Some(2), None, config, 0).is_none());
        assert!(composite_resource_set(&root, Some(3), None, config, 1).is_none());
    }

    #[test]
    fn root_version_belongs_to_window_allocation_not_filter_registry() {
        let mut root = RootColor::default();
        root.target = Some(target(1));
        let identity = root.identity();
        root.record_present(identity, true);
        let first_version = root.content_version().expect("submitted root");
        let mut filters = filters::FilterRegistry::new(None);
        filters.invalidate();
        assert_eq!(root.content_version(), Some(first_version));
        root.record_present(identity, false);
        assert_eq!(root.content_version(), Some(first_version));
        let retired = root.take_target().expect("single allocation retirement");
        assert_eq!(
            Some((retired.target.texture, retired.target.texture_view)),
            identity
        );
        assert!(root.take_target().is_none());
        assert!(root.content_version().is_none());
        root.target = Some(target(2));
        root.record_present(identity, true);
        assert!(
            root.content_version().is_none(),
            "old graph cannot validate new allocation"
        );
        root.record_present(root.identity(), false);
        assert!(
            root.content_version().is_none(),
            "new allocation needs a source pass"
        );
        root.record_present(root.identity(), true);
        assert_ne!(root.content_version(), Some(first_version));
        assert!(root.resident_on(root.identity()));
        root.record_present(None, false);
        assert!(root.content_version().is_none());
        assert!(
            root.target.is_some(),
            "logical Root removal keeps its GPU allocation warm"
        );
    }
}
