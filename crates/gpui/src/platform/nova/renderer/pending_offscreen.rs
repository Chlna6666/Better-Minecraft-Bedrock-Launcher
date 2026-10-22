//! Fence-safe offscreen preparation: an unsubmitted Root/Filter allocation is
//! independent from Active and may be created while old GPU readers are in flight.
//! The new targets carry NO valid texels. Their contents become reusable only
//! through the existing successful-Present/Fence version protocol.
use super::*;

/// A single physical retirement generation can contain Root and/or Filter.
/// All native submissions that could still sample either allocation are
/// snapshotted at the moment of replacement. New submissions never inherit
/// a retired view, so they never extend its retirement barrier.
pub(super) struct RetiredOffscreenTargets {
    pub(super) targets: PendingOffscreenTargets,
    readers: OffscreenFenceReaders,
}

#[derive(Default)]
struct OffscreenFenceReaders {
    ids: SmallVec<[SubmissionId; 4]>,
}

impl OffscreenFenceReaders {
    fn from_in_flight(submissions: &[PendingSubmission]) -> Self {
        Self {
            ids: submissions.iter().map(|entry| entry.submission).collect(),
        }
    }

    fn complete(&mut self, id: SubmissionId) {
        self.ids.retain(|reader| *reader != id);
    }

    fn safe_to_destroy(&self) -> bool {
        self.ids.is_empty()
    }
}

impl NovaRenderer {
    /// New physical allocations may replace Active while previous GPU
    /// readers are still executing. Capture only those old native fences.
    /// No GPU wait and no early BindGroup/Texture/View destruction.
    pub(super) fn retire_offscreen_targets(
        &mut self,
        root: Option<RootColorTarget>,
        filters: Option<BackdropBlurTargets>,
    ) {
        if root.is_none() && filters.is_none() {
            return;
        }
        let readers = OffscreenFenceReaders::from_in_flight(&self.pending_submissions);
        let targets = PendingOffscreenTargets {
            size: self.current_size,
            root,
            filters,
        };
        if readers.safe_to_destroy() {
            self.destroy_obsolete_pending_offscreen(targets);
        } else {
            log::trace!(
                target: "gpui::diagnostics",
                "nova offscreen retirement deferred: reader_fences={} root={} filter={}",
                readers.ids.len(), targets.root.is_some(), targets.filters.is_some(),
            );
            self.retired_offscreen_targets
                .push(RetiredOffscreenTargets { targets, readers });
        }
    }

    /// Only Fence::Complete retires physical readers. A failed submission
    /// retains its reference until backend/device teardown.
    pub(super) fn on_offscreen_submission_complete(&mut self, submission: SubmissionId) {
        let mut index = 0;
        while index < self.retired_offscreen_targets.len() {
            self.retired_offscreen_targets[index]
                .readers
                .complete(submission);
            if self.retired_offscreen_targets[index]
                .readers
                .safe_to_destroy()
            {
                let completed = self.retired_offscreen_targets.swap_remove(index);
                self.destroy_obsolete_pending_offscreen(completed.targets);
            } else {
                index += 1;
            }
        }
    }
}

pub(super) struct PendingOffscreenTargets {
    pub(super) size: DrawableSize,
    pub(super) root: Option<RootColorTarget>,
    pub(super) filters: Option<BackdropBlurTargets>,
}

impl PendingOffscreenTargets {
    pub(super) fn matches(&self, size: DrawableSize) -> bool {
        self.size == size
    }
}

fn prepare<D: ResourceDevice + PipelineDevice>(
    device: &mut D,
    backend: &str,
    size: DrawableSize,
    root_descriptor: Option<root_color::Descriptor>,
    filter_descriptor: Option<BackdropBlurTargetDescriptor>,
) -> Result<PendingOffscreenTargets> {
    let root = root_descriptor
        .map(|descriptor| root_color::create_target(device, descriptor))
        .transpose()?;
    let filters = match filter_descriptor
        .map(|descriptor| create_backdrop_blur_target_chain(device, backend, descriptor))
        .transpose()
    {
        Ok(filters) => filters,
        Err(error) => {
            if let Some(root) = root {
                root_color::destroy_target(device, root);
            }
            return Err(error);
        }
    };
    Ok(PendingOffscreenTargets {
        size,
        root,
        filters,
    })
}

pub(super) fn destroy<D: ResourceDevice + PipelineDevice>(
    device: &mut D,
    backend: &str,
    pending: PendingOffscreenTargets,
) {
    if let Some(filters) = pending.filters {
        destroy_backdrop_blur_target_chain(device, filters, backend);
    }
    if let Some(root) = pending.root {
        root_color::destroy_target(device, root);
    }
}

impl NovaRenderer {
    /// Prepare at most ONE independent size generation. Never mutate the
    /// Active Root/Filter views, resource sets or versions in this phase.
    ///
    /// A fast WM_SIZE burst may supersede the prepared extent. Do not
    /// repeatedly allocate on every intermediate pixel size: discard that
    /// one obsolete bundle at the committed resize boundary instead.
    pub(super) fn prepare_pending_offscreen(&mut self, size: DrawableSize) -> Result<()> {
        if self.pending_offscreen_targets.is_some() {
            return Ok(());
        }
        let root_needed = self.root_color.target.is_some();
        let filter_needed =
            self.filters.targets.is_some() && !self.frame_upload.backdrop_blur_configs().is_empty();
        if !root_needed && !filter_needed {
            return Ok(());
        }
        let extent = Extent2d::new(size.width, size.height)?;
        let root_descriptor = root_needed.then(|| root_color::Descriptor {
            size: extent,
            format: self.surface_format,
            pass_layout: self.backdrop_blur_pass_resource_set_layout,
            composite_layout: self.backdrop_blur_resource_set_layout,
            frames: self.frame_resource_buffers(),
            sampler: self.atlas_sampler,
        });
        let filter_descriptor = filter_needed.then(|| self.backdrop_blur_target_descriptor(extent));
        let pending: PendingOffscreenTargets = match &mut *lock_backend(&self.backend) {
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            NovaBackend::OpenGl(device) => {
                prepare(device, "OpenGL", size, root_descriptor, filter_descriptor)?
            }
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            NovaBackend::Dx11(device) => {
                prepare(device, "DX11", size, root_descriptor, filter_descriptor)?
            }
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => {
                prepare(device, "DX12", size, root_descriptor, filter_descriptor)?
            }
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => {
                prepare(device, "Metal", size, root_descriptor, filter_descriptor)?
            }
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => {
                prepare(device, "Vulkan", size, root_descriptor, filter_descriptor)?
            }
            #[cfg(not(any(
                all(
                    feature = "nova-gfx-opengl",
                    any(target_os = "windows", target_os = "linux")
                ),
                all(feature = "nova-gfx-dx11", target_os = "windows"),
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            NovaBackend::Unavailable => {
                anyhow::bail!("offscreen preparation requires a Nova backend")
            }
        };
        log::trace!(
            target: "gpui::diagnostics",
            "nova pending offscreen prepared: physical_px={}x{} root={} filter={} old_gpu_inflight={}",
            size.width, size.height, pending.root.is_some(), pending.filters.is_some(),
            self.pending_submissions.len(),
        );
        self.pending_offscreen_targets = Some(pending);
        Ok(())
    }

    /// Called only after the old native submissions were fully retired by
    /// Resize. An obsolete Pending never gains active identity or content
    /// validity. A matching bundle atomically installs its physical targets;
    /// the next actual GPU Present separately publishes their texel versions.
    pub(super) fn publish_pending_offscreen_after_resize(&mut self, size: DrawableSize) {
        let Some(pending) = self.pending_offscreen_targets.take() else {
            return;
        };
        if pending.matches(size) {
            self.root_color.target = pending.root;
            self.filters.targets = pending.filters;
            self.root_color.invalidate();
            self.filters.invalidate();
            log::trace!(
                target: "gpui::diagnostics",
                "nova pending offscreen installed: physical_px={}x{}",
                size.width, size.height,
            );
        } else {
            self.destroy_obsolete_pending_offscreen(pending);
        }
    }

    pub(super) fn destroy_obsolete_pending_offscreen(&mut self, pending: PendingOffscreenTargets) {
        match &mut *lock_backend(&self.backend) {
            #[cfg(all(
                feature = "nova-gfx-opengl",
                any(target_os = "windows", target_os = "linux")
            ))]
            NovaBackend::OpenGl(device) => destroy(device, "OpenGL", pending),
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            NovaBackend::Dx11(device) => destroy(device, "DX11", pending),
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => destroy(device, "DX12", pending),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => destroy(device, "Metal", pending),
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => destroy(device, "Vulkan", pending),
            #[cfg(not(any(
                all(
                    feature = "nova-gfx-opengl",
                    any(target_os = "windows", target_os = "linux")
                ),
                all(feature = "nova-gfx-dx11", target_os = "windows"),
                all(feature = "nova-gfx-dx12", target_os = "windows"),
                all(feature = "nova-gfx-metal", target_os = "macos"),
                all(
                    feature = "nova-gfx-vulkan",
                    any(target_os = "windows", target_os = "linux", target_os = "freebsd")
                )
            )))]
            NovaBackend::Unavailable => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offscreen_retirement_waits_only_for_snapshotted_gpu_fence_readers() {
        let a = SubmissionId::from_parts(1, 1);
        let b = SubmissionId::from_parts(2, 1);
        let future = SubmissionId::from_parts(3, 1);
        let mut readers = OffscreenFenceReaders {
            ids: smallvec::smallvec![a, b],
        };
        readers.complete(future);
        assert!(
            !readers.safe_to_destroy(),
            "future submissions cannot retire old targets"
        );
        readers.complete(b);
        assert!(
            !readers.safe_to_destroy(),
            "remaining old submission still samples texture"
        );
        readers.complete(a);
        assert!(readers.safe_to_destroy());
    }

    #[test]
    fn overlapping_offscreen_generations_retire_at_their_own_last_reader() {
        let a = SubmissionId::from_parts(10, 1);
        let b = SubmissionId::from_parts(11, 1);
        let c = SubmissionId::from_parts(12, 1);
        // Two physical generations were replaced between native fence events.
        // Generation 1 only needs A/B; generation 2 needs B/C.
        let mut old = OffscreenFenceReaders {
            ids: smallvec::smallvec![a, b],
        };
        let mut newer = OffscreenFenceReaders {
            ids: smallvec::smallvec![b, c],
        };
        old.complete(a);
        newer.complete(a);
        assert!(!old.safe_to_destroy());
        assert!(!newer.safe_to_destroy());
        old.complete(b);
        newer.complete(b);
        assert!(
            old.safe_to_destroy(),
            "C cannot delay A/B's retired allocation"
        );
        assert!(
            !newer.safe_to_destroy(),
            "C still owns the next retired allocation"
        );
        newer.complete(c);
        assert!(newer.safe_to_destroy());
    }

    #[test]
    fn pending_offscreen_size_identity_cannot_publish_stale_resize_resources() {
        let prepared = PendingOffscreenTargets {
            size: DrawableSize {
                width: 900,
                height: 600,
            },
            root: None,
            filters: None,
        };
        assert!(prepared.matches(DrawableSize {
            width: 900,
            height: 600
        }));
        assert!(!prepared.matches(DrawableSize {
            width: 901,
            height: 600
        }));
        assert!(!prepared.matches(DrawableSize {
            width: 900,
            height: 601
        }));
    }
}
