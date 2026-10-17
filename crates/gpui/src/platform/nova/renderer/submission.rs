use super::buffer_upload::{record_empty_buffer_batch, upload_buffer_batch};
use super::*;
use gfx_core::BufferUploadBatch;

fn presentation_clock_snapshot(frame_time: Instant) -> [u8; 24] {
    let seconds = crate::animation::presentation_clock_seconds_at(frame_time);
    let tick_60hz = ((seconds as f64 * 60.0).floor() as u64 & u64::from(u32::MAX)) as u32;
    let mut clock = [0; 24];
    clock[16..20].copy_from_slice(&seconds.to_ne_bytes());
    clock[20..24].copy_from_slice(&tick_60hz.to_ne_bytes());
    clock
}

/// Static built-in Nova shaders depend on viewport/alpha but not the
/// presentation clock fields. Do not issue a native uniform-buffer update on
/// an exposure/refresh frame whose committed content is wholly static.
///
/// Dynamic Scenes, including retained custom renderer extensions, retain the
/// exact frame-owned clock sample even when they expose no CPU animation
/// primitives. Never decide this from the dirty region alone.
fn needs_presentation_clock_upload(upload: &FrameUpload) -> bool {
    !upload.renderer_extensions.is_empty()
        || !upload.sampled_animation_values.is_empty()
        || !upload.gpu_indexed_animation_slots.is_empty()
        || !upload.animated_primitives.is_empty()
}

fn submit_presentation_buffers(
    backend: &SharedBackend,
    batch: &BufferUploadBatch<'_>,
) -> Result<()> {
    // A static frame has no presentation uniform or animation table updates.
    // The shared GPU owner must not take the backend lock for an empty batch.
    if batch.writes().len() == 0 {
        record_empty_buffer_batch(batch, Instant::now());
        return Ok(());
    }
    match &mut *lock_backend(backend) {
        #[cfg(all(
            feature = "nova-gfx-opengl",
            any(target_os = "windows", target_os = "linux")
        ))]
        NovaBackend::OpenGl(device) => upload_buffer_batch(device, batch),
        #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
        NovaBackend::Dx11(device) => upload_buffer_batch(device, batch),
        #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
        NovaBackend::Dx12(device) => upload_buffer_batch(device, batch),
        #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
        NovaBackend::Metal(device) => upload_buffer_batch(device, batch),
        #[cfg(all(
            feature = "nova-gfx-vulkan",
            any(target_os = "windows", target_os = "linux", target_os = "freebsd")
        ))]
        NovaBackend::Vulkan(device) => upload_buffer_batch(device, batch),
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
            anyhow::bail!("nova-gfx renderer requires an explicit nova-gfx backend feature")
        }
    }
}

impl NovaRenderer {
    /// Preflight before any target rebind or upload. Pending GPU readers keep their resources;
    /// the owner retries this packet without occupying the shared thread in a fence wait.
    pub(super) fn frame_submission_is_ready(&mut self, scene: &crate::Scene) -> Result<bool> {
        self.submission_retry_pending = false;
        if self.presentation_submission_mode() == GpuSubmissionMode::Synchronous {
            return Ok(true);
        }
        self.poll_pending_submissions()?;
        let path_rebind = (!self.frame_upload.path_rasterization_vertices.is_empty()
            || !self.frame_upload.path_sprites.is_empty())
            && (self.path_texture_size.width() != self.current_size.width
                || self.path_texture_size.height() != self.current_size.height);
        let filter_rebind = !self.frame_upload.backdrop_blurs.is_empty()
            && self.filters.targets.as_ref().is_some_and(|targets| {
                !targets.is_layout_compatible(
                    self.frame_upload.backdrop_blur_configs(),
                    self.frame_upload.isolated_blur_source_indices(),
                    super::present::has_root_backdrop_blurs(&self.frame_upload),
                )
            });
        let atlas_retirement =
            if !self.pending_submissions.is_empty() && self.atlas.has_pending_removals() {
                let mut live_tiles = FxHashSet::default();
                scene.collect_atlas_tile_ids_into(&mut live_tiles);
                self.atlas.has_retirable_pending_removals(&live_tiles)
            } else {
                false
            };
        self.submission_retry_pending = self.pending_submissions.len() >= MAX_IN_FLIGHT_SUBMISSIONS
            || self.next_available_frame_resource_index().is_err()
            || atlas_retirement
            || ((path_rebind || filter_rebind) && !self.pending_submissions.is_empty());
        if self.submission_retry_pending {
            log::trace!(target: "gpui::diagnostics",
                "Nova submission deferred: pending={} path_rebind={} filter_rebind={} atlas_retirement={}",
                self.pending_submissions.len(), path_rebind, filter_rebind, atlas_retirement);
        }
        Ok(!self.submission_retry_pending)
    }

    pub(crate) fn has_submission_retry(&self) -> bool {
        self.submission_retry_pending
    }

    pub(super) fn prepare_for_frame_submission(&mut self, frame_time: Instant) -> Result<()> {
        if self.presentation_submission_mode() == GpuSubmissionMode::Synchronous {
            let had_pending_submissions = !self.pending_submissions.is_empty();
            let wait_started_at = Instant::now();
            self.wait_for_pending_submissions()?;
            if had_pending_submissions {
                crate::diagnostics::performance_metrics::record_frame_slot_wait(
                    wait_started_at.elapsed(),
                );
            }
            self.activate_frame_resources(0)?;
            self.upload_presentation_buffers(frame_time)?;
            return Ok(());
        }
        self.poll_pending_submissions()?;
        if self.pending_submissions.len() >= MAX_IN_FLIGHT_SUBMISSIONS {
            anyhow::bail!("nova submission preflight must reserve a fence-safe frame slot");
        }
        let frame_resource_index = self.next_available_frame_resource_index()?;
        self.activate_frame_resources(frame_resource_index)?;
        self.upload_presentation_buffers(frame_time)?;
        Ok(())
    }

    /// Writes dynamic presentation state only after activating a fence-safe resource slot.
    fn upload_presentation_buffers(&mut self, frame_time: Instant) -> Result<()> {
        let plan_started = Instant::now(); // CPU upload planning, separate from frame_time.
        let dynamic_clock = needs_presentation_clock_upload(&self.frame_upload);
        let clock = dynamic_clock.then(|| presentation_clock_snapshot(frame_time));
        if let Some(clock) = &clock
            && self.frame_upload.globals.len() >= clock.len()
        {
            self.frame_upload.globals[16..24].copy_from_slice(&clock[16..24]);
        }
        self.frame_upload.rebuild_indexed_animation_values();
        self.prepare_static_buffers()?;
        // The selected slot is fence-safe; grow and rebind before writing its table.
        self.ensure_stream_capacity()?;
        let values = &self.frame_upload.gpu_indexed_animation_values;
        let mut batch = BufferUploadBatch::default();
        if let Some(clock) = &clock {
            batch.push(self.global_buffer, clock, 16..24)?;
        }
        batch.push(self.animation_value_buffer, values, 0..values.len())?;
        crate::diagnostics::performance_metrics::record_nova_buffer_upload_time(
            plan_started.elapsed(),
        );
        submit_presentation_buffers(&self.backend, &batch)
    }

    fn poll_pending_submissions(&mut self) -> Result<()> {
        let mut index = 0;
        while index < self.pending_submissions.len() {
            let submission = self.pending_submissions[index].submission;
            let status = lock_backend(&self.backend).poll_submission(submission)?;
            match status {
                SubmissionStatus::Pending => index += 1,
                SubmissionStatus::Complete => {
                    self.pending_submissions.remove(index);
                }
                SubmissionStatus::Failed(error) => {
                    self.pending_submissions.remove(index);
                    return Err(gfx_core::Error::Backend(error).into());
                }
            }
        }
        Ok(())
    }

    pub(super) fn wait_for_pending_submissions(&mut self) -> Result<()> {
        while let Some(submission) = self
            .pending_submissions
            .first()
            .map(|submission| submission.submission)
        {
            let started_at = Instant::now();
            let result = lock_backend(&self.backend).wait_submission(submission);
            let waited = started_at.elapsed();
            crate::diagnostics::performance_metrics::record_gpu_submission_wait(waited);
            if waited >= std::time::Duration::from_millis(8) {
                // The GPU owner is shared across windows: a single long fence wait
                // delays every other window's queued work. Keep backend/time visible
                // without logging the usual sub-millisecond polling completions.
                log::warn!(
                    target: "gpui_gpu_stall",
                    "gpu_owner_submission_wait backend={} wait_us={} remaining_submissions={}",
                    self.backend_info.label(),
                    waited.as_micros(),
                    self.pending_submissions.len(),
                );
            }
            result?;
            self.pending_submissions.remove(0);
        }
        Ok(())
    }

    pub(super) fn prepare_for_resize(&mut self) -> Result<()> {
        self.wait_for_pending_submissions()
    }

    /// Attempts to apply the latest drawable size at a frame boundary without blocking the
    /// native resize event path.
    ///
    /// All backends first retire this renderer's submissions by polling. A pending
    /// resize prevents new old-size submissions, so animations cannot keep this
    /// check busy forever. Vulkan retains its driver-level device-idle barrier
    /// for swapchain replacement; it does not wait for unrelated windows in the
    /// global readiness predicate, which could starve this window indefinitely.
    pub(crate) fn try_resize(&mut self, size: Size<DevicePixels>) -> Result<bool> {
        let requested = DrawableSize {
            width: size.width.0.max(1) as u32,
            height: size.height.0.max(1) as u32,
        };
        // Keep the committed swapchain and its render viewport in one pixel
        // coordinate system. Oversizing DXGI swapchains during WM_SIZE used to
        // leave a visibly black native client region even when Scene/Present
        // reported the latest geometry: source-size cropping is not a native
        // backbuffer resize. Reuse immutable GPU atlas/upload resources instead
        // of allocating a larger *presentation* surface.
        let width = requested.width;
        let height = requested.height;
        let size = crate::size(DevicePixels(width as i32), DevicePixels(height as i32));
        if self.current_size.width == width && self.current_size.height == height {
            return Ok(true);
        }

        self.poll_pending_submissions()?;
        if !self.pending_submissions.is_empty() {
            return Ok(false);
        }

        #[cfg(all(
            feature = "nova-gfx-vulkan",
            any(target_os = "windows", target_os = "linux", target_os = "freebsd")
        ))]
        if self.backend_info.is_vulkan() {
            self.resize(size)?;
            return Ok(true);
        }

        if lock_backend(&self.backend).has_pending_resize_work(self.swapchain)? {
            return Ok(false);
        }

        self.resize(size)?;
        Ok(true)
    }

    pub(super) fn submit_present_frame<D>(
        submission_mode: GpuSubmissionMode,
        async_capabilities: AsyncCapabilities,
        pending_submissions: &mut Vec<PendingSubmission>,
        device: &mut D,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: &[RenderStepDescriptor],
        clear_color: ClearColor,
        depth_attachment: Option<RenderPassDepthAttachment>,
        frame_resource_index: usize,
        damage: Option<ScissorRect>,
    ) -> Result<Option<gfx_core::PresentationFrame>>
    where
        D: BackendPresentationCompat + BackendQueue,
    {
        let mut frame = device.render_step_list_and_present_deferred_with_damage_measured(
            swapchain,
            render_pass,
            RenderStepList::from_render_steps(steps),
            clear_color,
            depth_attachment,
            damage,
        )?;

        let use_deferred_submission = submission_mode != GpuSubmissionMode::Synchronous
            && async_capabilities.async_presentation;
        if !use_deferred_submission {
            if let Some(submission) = frame.as_mut().and_then(|frame| frame.submission) {
                let wait_started = Instant::now();
                let result = gfx_core::SubmissionDevice::wait_submission(device, submission);
                let elapsed = wait_started.elapsed();
                crate::diagnostics::gpu_owner::record_blocking_wait(elapsed);
                result?;
                if let Some(timings) = frame.as_mut().and_then(|frame| frame.timings.as_mut()) {
                    timings.submission_wait = elapsed;
                }
            }
        } else if let Some(submission) = frame.as_mut().and_then(|frame| frame.submission) {
            pending_submissions.push(PendingSubmission {
                submission,
                frame_resource_index,
            });
        }
        Ok(frame)
    }

    pub(super) fn presentation_submission_mode(&self) -> GpuSubmissionMode {
        self.submission_mode
    }

    fn next_available_frame_resource_index(&self) -> Result<usize> {
        for index in 0..self.frame_resources.len() {
            if self
                .pending_submissions
                .iter()
                .all(|submission| submission.frame_resource_index != index)
            {
                return Ok(index);
            }
        }
        anyhow::bail!("no available nova frame resource slot")
    }
}

#[cfg(test)]
mod presentation_clock_upload_tests {
    use super::*;

    #[test]
    fn genuinely_static_present_has_no_gpu_clock_write() {
        let upload = FrameUpload::default();
        assert!(!needs_presentation_clock_upload(&upload));
        assert!(upload.gpu_indexed_animation_values.is_empty());
    }

    #[test]
    fn gpu_indexed_slots_keep_clock_updates_during_inactive_samples() {
        let mut upload = FrameUpload::default();
        upload
            .gpu_indexed_animation_slots
            .insert(crate::SceneAnimationId(11), 0);
        assert!(
            needs_presentation_clock_upload(&upload),
            "retained GPU timelines must not freeze just because values are absent"
        );
    }

    #[test]
    fn cpu_animation_sample_keeps_frame_owned_clock() {
        let upload = FrameUpload {
            sampled_animation_values: vec![crate::SceneAnimationValue {
                animation_id: crate::SceneAnimationId(12),
                property: crate::TransitionProperty::Opacity,
                progress: 0.5,
                from: [0.0; 4],
                to: [1.0; 4],
            }],
            ..Default::default()
        };
        assert!(needs_presentation_clock_upload(&upload));
    }

    #[test]
    fn clock_timestamp_fields_use_the_provided_presentation_instant() {
        let time = Instant::now();
        assert_eq!(presentation_clock_snapshot(time), presentation_clock_snapshot(time));
        let clock = presentation_clock_snapshot(time);
        assert_eq!(clock.len(), 24);
        assert!(clock[..16].iter().all(|value| *value == 0));
    }
}
