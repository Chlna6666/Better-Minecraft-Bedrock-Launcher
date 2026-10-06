use super::*;
use gfx_core::ResourceDevice;

impl NovaRenderer {
    pub(super) fn prepare_for_frame_submission(&mut self) -> Result<()> {
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
            self.upload_presentation_clock()?;
            self.upload_gpu_indexed_animation_values()?;
            return Ok(());
        }
        self.poll_pending_submissions()?;
        if self.pending_submissions.len() >= MAX_IN_FLIGHT_SUBMISSIONS {
            self.wait_for_oldest_submission()?;
        }
        let frame_resource_index = self.next_available_frame_resource_index()?;
        self.activate_frame_resources(frame_resource_index)?;
        self.upload_presentation_clock()?;
        self.upload_gpu_indexed_animation_values()?;
        Ok(())
    }

    /// Upload the renderer presentation clock into the currently active frame-resource slot.
    ///
    /// The first 16 bytes of GlobalParams remain static scene state. Only the final 8 bytes change
    /// on presentation-only frames, so custom retained GPU effects can advance without rebuilding
    /// scene primitives, layout, or View state.
    fn upload_presentation_clock(&mut self) -> Result<()> {
        const CLOCK_OFFSET: usize = 16;
        const CLOCK_BYTES: usize = 8;

        let seconds = crate::animation::presentation_clock_seconds_now();
        let tick_60hz = ((seconds as f64 * 60.0).floor() as u64 & u64::from(u32::MAX)) as u32;
        let mut bytes = [0_u8; CLOCK_BYTES];
        bytes[..4].copy_from_slice(&seconds.to_ne_bytes());
        bytes[4..].copy_from_slice(&tick_60hz.to_ne_bytes());

        if self.frame_upload.globals.len() >= CLOCK_OFFSET + CLOCK_BYTES {
            self.frame_upload.globals[CLOCK_OFFSET..CLOCK_OFFSET + CLOCK_BYTES]
                .copy_from_slice(&bytes);
        }

        let buffer = self.global_buffer;
        match &mut *lock_backend(&self.backend) {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => {
                device.write_buffer(buffer, CLOCK_OFFSET as u64, &bytes)?
            }
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => {
                device.write_buffer(buffer, CLOCK_OFFSET as u64, &bytes)?
            }
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => {
                device.write_buffer(buffer, CLOCK_OFFSET as u64, &bytes)?
            }
            #[cfg(not(any(
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
        Ok(())
    }

    /// Upload one dense timeline table after the destination frame-resource slot is activated.
    /// Quad/glyph/image primitive bytes stay resident; hundreds of glyphs sharing one animation
    /// therefore cost one 64-byte timeline record instead of hundreds of primitive rewrites.
    fn upload_gpu_indexed_animation_values(&mut self) -> Result<()> {
        self.frame_upload.rebuild_indexed_animation_values();
        let bytes = &self.frame_upload.gpu_indexed_animation_values;
        if bytes.is_empty() {
            return Ok(());
        }
        let buffer = self.animation_value_buffer;
        match &mut *lock_backend(&self.backend) {
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => device.write_buffer(buffer, 0, bytes)?,
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => device.write_buffer(buffer, 0, bytes)?,
            #[cfg(all(
                feature = "nova-gfx-vulkan",
                any(target_os = "windows", target_os = "linux", target_os = "freebsd")
            ))]
            NovaBackend::Vulkan(device) => device.write_buffer(buffer, 0, bytes)?,
            #[cfg(not(any(
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
        Ok(())
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

    fn wait_for_oldest_submission(&mut self) -> Result<()> {
        let Some(submission) = self
            .pending_submissions
            .first()
            .map(|submission| submission.submission)
        else {
            return Ok(());
        };
        let started_at = Instant::now();
        let result = lock_backend(&self.backend).wait_submission(submission);
        let elapsed = started_at.elapsed();
        crate::diagnostics::performance_metrics::record_gpu_submission_wait(elapsed);
        crate::diagnostics::performance_metrics::record_frame_slot_wait(elapsed);
        result?;
        self.pending_submissions.remove(0);
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
            crate::diagnostics::performance_metrics::record_gpu_submission_wait(
                started_at.elapsed(),
            );
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
    /// DX12 keeps the non-blocking readiness check because its size-dependent resources use
    /// fence-backed retirement. Vulkan intentionally follows the known-good synchronization
    /// model used before the live-resize regression: coalesce native events at the platform
    /// boundary, then complete one exact resize transaction at the render boundary. Trying to
    /// keep Vulkan permanently non-blocking here can starve resize while animations continuously
    /// keep one submission in flight, and replacing a presentable swapchain with narrower
    /// synchronization proved unsafe on Win32 drivers.
    pub(crate) fn try_resize(&mut self, size: Size<DevicePixels>) -> Result<bool> {
        let width = size.width.0.max(1) as u32;
        let height = size.height.0.max(1) as u32;
        if self.current_size.width == width && self.current_size.height == height {
            // A native resize burst may temporarily stretch the old compositor surface and then
            // coalesce back to the swapchain's existing drawable size. Treating that as a pure
            // no-op leaves the temporary transform alive indefinitely, so windowed content can
            // remain linearly scaled/soft until a later real resize (for example maximize) clears
            // it. Reassert identity even when no buffer resize is required.
            if let Err(error) =
                lock_backend(&self.backend).set_swapchain_content_stretch(self.swapchain, None)
            {
                log::warn!("failed to reset nova-gfx coalesced resize stretch: {error:#}");
            }
            return Ok(true);
        }

        #[cfg(all(
            feature = "nova-gfx-vulkan",
            any(target_os = "windows", target_os = "linux", target_os = "freebsd")
        ))]
        if self.backend_info.is_vulkan() {
            self.resize(size)?;
            return Ok(true);
        }

        self.poll_pending_submissions()?;
        if !self.pending_submissions.is_empty()
            || lock_backend(&self.backend).has_pending_resize_work(self.swapchain)?
        {
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
                gfx_core::SubmissionDevice::wait_submission(device, submission)?;
                if let Some(timings) = frame.as_mut().and_then(|frame| frame.timings.as_mut()) {
                    timings.submission_wait = wait_started.elapsed();
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
