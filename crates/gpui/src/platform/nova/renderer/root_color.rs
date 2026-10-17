//! Window-owned color residency. Filter topology never owns or retires these resources.
use super::*;
use crate::platform::nova::composite_target::DirectCompositeTarget;

#[derive(Default)]
pub(super) struct RootColor {
    pub(super) target: Option<RootColorTarget>,
    content_version: Option<u64>,
    /// Only signaled by the native GPU fence (or synchronous submission).
    /// The submitted version above is queue-ordered but may still be in flight.
    completed_content_version: Option<u64>,
    next_version: u64,
    /// Samples represented by the last successfully REFRESHED root pixels,
    /// not the latest swapchain-only Present. Otherwise an animation-only
    /// compositor Present can overwrite the retained source baseline without
    /// updating its texels, concealing later source damage. Filter refreshes
    /// keep their own independent snapshots.
    animation_values: Vec<crate::SceneAnimationValue>,
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

    pub(super) fn completed_content_version(&self) -> Option<u64> {
        self.completed_content_version
    }

    pub(super) fn complete_submission(
        &mut self,
        identity: (TextureId, TextureViewId),
        version: u64,
    ) {
        // An older fence cannot complete a newly allocated or newer target.
        if self.identity() == Some(identity) && self.content_version == Some(version) {
            self.completed_content_version = Some(version);
        }
    }

    pub(super) fn resident_on(&self, identity: Option<(TextureId, TextureViewId)>) -> bool {
        self.content_version.is_some() && identity.is_some() && identity == self.identity()
    }

    pub(super) fn invalidate(&mut self) {
        self.content_version = None;
        self.completed_content_version = None;
        self.animation_values.clear();
    }

    pub(super) fn animation_values(&self) -> &[crate::SceneAnimationValue] {
        &self.animation_values
    }

    /// Publish only after successful submission; a filter-index move cannot import pixels.
    pub(super) fn record_present(
        &mut self,
        identity: Option<(TextureId, TextureViewId)>,
        refreshed: bool,
        animation_values: &[crate::SceneAnimationValue],
    ) {
        if identity.is_none() || identity != self.identity() {
            self.invalidate();
        } else if refreshed {
            self.next_version = self.next_version.wrapping_add(1).max(1);
            self.content_version = Some(self.next_version);
            self.completed_content_version = None;
            // Publish source samples atomically with the successful GPU color
            // refresh. A composite-only Present must neither advance the
            // pixel content version nor mutate the samples describing it.
            self.animation_values.clear();
            self.animation_values.extend_from_slice(animation_values);
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
    /// Retire physical Root Color through the same exact-fence owner as
    /// Filter targets. A native submission cannot lose its Root Texture/View
    /// just because a newer scene stopped requiring root retention.
    pub(super) fn retire_root_color(&mut self) {
        let root = self.root_color.take_target();
        self.retire_offscreen_targets(root, None);
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
    fn root_fence_completion_cannot_certify_newer_pixels_or_reallocated_texture() {
        let mut root = RootColor::default();
        root.target = Some(target(1));
        let first_identity = root.identity().unwrap();
        root.record_present(Some(first_identity), true, &[]);
        let first = root.content_version().unwrap();
        assert_eq!(root.completed_content_version(), None);

        root.record_present(Some(first_identity), true, &[]);
        let second = root.content_version().unwrap();
        assert_ne!(first, second);
        root.complete_submission(first_identity, first);
        assert_eq!(root.completed_content_version(), None);
        root.complete_submission(first_identity, second);
        assert_eq!(root.completed_content_version(), Some(second));

        root.target = Some(target(2));
        root.record_present(root.identity(), true, &[]);
        assert_eq!(root.completed_content_version(), None);
        root.complete_submission(first_identity, second);
        assert_eq!(root.completed_content_version(), None);
        let newest_identity = root.identity().unwrap();
        let newest = root.content_version().unwrap();
        root.complete_submission(newest_identity, newest);
        assert_eq!(root.completed_content_version(), Some(newest));
        root.invalidate();
        assert_eq!(root.completed_content_version(), None);
    }

    #[test]
    fn compositor_only_presents_preserve_actual_root_pixel_sample_baseline() {
        let mut root = RootColor::default();
        root.target = Some(target(1));
        let identity = root.identity();
        let sample = |progress: f32| crate::SceneAnimationValue {
            animation_id: crate::SceneAnimationId(99),
            property: crate::TransitionProperty::Opacity,
            progress,
            from: [0.; 4],
            to: [1., 0., 0., 0.],
        };
        root.record_present(identity, true, &[sample(0.1)]);
        let committed_version = root.content_version();
        assert!(committed_version.is_some());

        // N+1 and N+2 merely composite from an existing GPU texture.
        // They must not consume N+1 samples as the baseline for the next
        // source refresh, and must not touch the old sample vector.
        for progress in [0.4, 0.7] {
            root.record_present(identity, false, &[sample(progress)]);
            assert_eq!(root.content_version(), committed_version);
            assert_eq!(root.animation_values(), &[sample(0.1)]);
        }

        root.record_present(identity, true, &[sample(0.8)]);
        assert_ne!(root.content_version(), committed_version);
        assert_eq!(root.animation_values(), &[sample(0.8)]);

        // A replaced GPU allocation is never allowed to inherit the
        // old texture's samples or content version.
        root.target = Some(target(2));
        root.record_present(identity, false, &[sample(0.9)]);
        assert!(root.content_version().is_none());
        assert!(root.animation_values().is_empty());
    }

    #[test]
    fn root_version_belongs_to_window_allocation_not_filter_registry() {
        let mut root = RootColor::default();
        root.target = Some(target(1));
        let identity = root.identity();
        let sample = crate::SceneAnimationValue {
            animation_id: crate::SceneAnimationId(1),
            property: crate::TransitionProperty::Opacity,
            progress: 0.2,
            from: [0.; 4],
            to: [1., 0., 0., 0.],
        };
        root.record_present(identity, true, &[sample]);
        let first_version = root.content_version().expect("submitted root");
        let mut filters = filters::FilterRegistry::new(None);
        filters.invalidate();
        assert_eq!(root.content_version(), Some(first_version));
        assert_eq!(root.animation_values(), &[sample]);
        root.record_present(identity, false, &[sample]);
        assert_eq!(root.content_version(), Some(first_version));
        let retired = root.take_target().expect("single allocation retirement");
        assert_eq!(
            Some((retired.target.texture, retired.target.texture_view)),
            identity
        );
        assert!(root.take_target().is_none());
        assert!(root.content_version().is_none());
        assert!(root.animation_values().is_empty());
        root.target = Some(target(2));
        root.record_present(identity, true, &[sample]);
        assert!(
            root.content_version().is_none(),
            "old graph cannot validate new allocation"
        );
        root.record_present(root.identity(), false, &[sample]);
        assert!(
            root.content_version().is_none(),
            "new allocation needs a source pass"
        );
        root.record_present(root.identity(), true, &[sample]);
        assert_ne!(root.content_version(), Some(first_version));
        assert!(root.resident_on(root.identity()));
        root.record_present(None, false, &[sample]);
        assert!(root.content_version().is_none());
        assert!(root.animation_values().is_empty());
        assert!(
            root.target.is_some(),
            "logical Root removal keeps its GPU allocation warm"
        );
    }
}
