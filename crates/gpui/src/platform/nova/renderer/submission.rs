use super::*;

impl NovaRenderer {
    pub(super) fn prepare_for_frame_submission(&mut self) -> Result<()> {
        if self.presentation_submission_mode() == GpuSubmissionMode::Synchronous {
            self.wait_for_pending_submissions()?;
            self.activate_frame_resources(0)?;
            return Ok(());
        }
        self.poll_pending_submissions()?;
        if self.pending_submissions.len() >= MAX_IN_FLIGHT_SUBMISSIONS {
            self.wait_for_oldest_submission()?;
        }
        let frame_resource_index = self.next_available_frame_resource_index()?;
        self.activate_frame_resources(frame_resource_index)?;
        Ok(())
    }

    fn poll_pending_submissions(&mut self) -> Result<()> {
        let mut index = 0;
        while index < self.pending_submissions.len() {
            let submission = self.pending_submissions[index].submission;
            let status = self.backend.poll_submission(submission)?;
            match status {
                SubmissionStatus::Pending => index += 1,
                SubmissionStatus::Complete => {
                    self.pending_submissions.remove(index);
                }
                SubmissionStatus::Failed(error) => {
                    self.pending_submissions.remove(index);
                    return Err(gfx_core::GfxError::Backend(error).into());
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
        let result = self.backend.wait_submission(submission);
        crate::diagnostics::performance_metrics::record_gpu_submission_wait(started_at.elapsed());
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
            let result = self.backend.wait_submission(submission);
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
    /// Window systems can emit resize/configure events substantially faster than either the
    /// compositor or GPU can produce frames. The platform layer therefore coalesces those events
    /// into one latest size. At the next frame boundary we retire completed submissions and only
    /// rebuild the swapchain when the renderer and that swapchain are safe to replace.
    ///
    /// This intentionally has no fixed debounce/settle interval: a fast GPU can follow nearly
    /// every display frame, while a busy GPU naturally coalesces more native resize events. That
    /// keeps layout feedback live instead of freezing it behind an arbitrary timer.
    pub(crate) fn try_resize(&mut self, size: Size<DevicePixels>) -> Result<bool> {
        let width = size.width.0.max(1) as u32;
        let height = size.height.0.max(1) as u32;
        if self.current_size.width == width && self.current_size.height == height {
            return Ok(true);
        }

        self.poll_pending_submissions()?;
        if !self.pending_submissions.is_empty()
            || self
                .backend
                .has_pending_resize_work(self.swapchain)?
        {
            return Ok(false);
        }

        self.resize(size)?;
        Ok(true)
    }

    pub(super) fn submit_present_frame<D>(
        submission_mode: GpuSubmissionMode,
        async_capabilities: BackendAsyncCapabilities,
        pending_submissions: &mut Vec<PendingSubmission>,
        device: &mut D,
        swapchain: SwapchainId,
        render_pass: RenderPassId,
        steps: &[RenderStepDescriptor],
        clear_color: ClearColor,
        depth_attachment: Option<RenderPassDepthAttachment>,
        frame_resource_index: usize,
        damage: Option<ScissorRect>,
    ) -> Result<()>
    where
        D: BackendPresentationCompat + BackendQueue,
    {
        if submission_mode == GpuSubmissionMode::Synchronous
            || !async_capabilities.async_presentation
        {
            device.render_step_list_and_present_with_damage_compat(
                swapchain,
                render_pass,
                RenderStepList::from_render_steps(steps),
                clear_color,
                depth_attachment,
                damage,
            )?;
            return Ok(());
        }

        let submission = device.render_step_list_and_present_deferred_with_damage_compat(
            swapchain,
            render_pass,
            RenderStepList::from_render_steps(steps),
            clear_color,
            depth_attachment,
            damage,
        )?;
        if is_real_submission(submission) {
            pending_submissions.push(PendingSubmission {
                submission,
                frame_resource_index,
            });
        }
        Ok(())
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

fn is_real_submission(submission: SubmissionId) -> bool {
    submission.raw() != 0
}
