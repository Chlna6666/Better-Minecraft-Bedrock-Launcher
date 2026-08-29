use super::*;

#[cfg(target_os = "windows")]
const INTERACTIVE_RESIZE_SETTLE_GRACE: std::time::Duration = std::time::Duration::from_millis(48);

#[cfg(target_os = "windows")]
#[derive(Clone, Copy)]
struct InteractiveResizePacing {
    target_width: u32,
    target_height: u32,
    target_changed_at: Instant,
}

#[cfg(target_os = "windows")]
thread_local! {
    static INTERACTIVE_RESIZE_PACING: std::cell::RefCell<std::collections::HashMap<usize, InteractiveResizePacing>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

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

    /// Applies an exact swapchain resize only after the native resize target has settled.
    ///
    /// Windows continuously emits resize targets while the pointer is moving. Rebuilding
    /// the swapchain plus path/depth/blur attachments during that stream is much more
    /// expensive than presenting a normal frame, and Vulkan additionally has to retire
    /// swapchain-owned work before recreation. The previous implementation periodically
    /// forced an exact rebuild every ~33 ms, which made both DX12 and Vulkan alternate
    /// between a stretched compositor frame and a newly rebuilt surface. That cadence is
    /// visible as resize judder even when the application itself is not dropping frames.
    ///
    /// Keep the last presented surface covering the client area while the target is moving
    /// and perform one exact rebuild after the target has been stable for a short grace
    /// period. Target tracking happens before the GPU-idle gate so a busy GPU can never hide
    /// newer pointer resize events and accidentally make a moving target look settled.
    #[cfg(target_os = "windows")]
    pub(crate) fn try_resize(&mut self, size: Size<DevicePixels>) -> Result<bool> {
        let width = size.width.0.max(1) as u32;
        let height = size.height.0.max(1) as u32;
        if self.current_size.width == width && self.current_size.height == height {
            return Ok(true);
        }

        let now = Instant::now();
        if interactive_resize_should_defer(self, width, height, now) {
            // Still retire completed submissions while live resize is compositor/WSI driven,
            // but never block or recreate swapchain resources until the target settles.
            self.poll_pending_submissions()?;
            return Ok(false);
        }

        self.poll_pending_submissions()?;
        if !self.pending_submissions.is_empty() || self.backend.has_pending_resize_work()? {
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

#[cfg(target_os = "windows")]
fn interactive_resize_key(renderer: &NovaRenderer) -> usize {
    renderer as *const NovaRenderer as usize
}

#[cfg(target_os = "windows")]
fn interactive_resize_should_defer(
    renderer: &NovaRenderer,
    width: u32,
    height: u32,
    now: Instant,
) -> bool {
    let key = interactive_resize_key(renderer);
    INTERACTIVE_RESIZE_PACING.with(|pacing| {
        let mut pacing = pacing.borrow_mut();
        if pacing.len() >= 64 && !pacing.contains_key(&key) {
            pacing.clear();
        }
        let state = pacing.entry(key).or_insert(InteractiveResizePacing {
            target_width: width,
            target_height: height,
            target_changed_at: now,
        });

        if state.target_width != width || state.target_height != height {
            state.target_width = width;
            state.target_height = height;
            state.target_changed_at = now;
        }

        !interactive_resize_target_is_settled(state.target_changed_at, now)
    })
}

#[cfg(target_os = "windows")]
fn interactive_resize_target_is_settled(target_changed_at: Instant, now: Instant) -> bool {
    now.saturating_duration_since(target_changed_at) >= INTERACTIVE_RESIZE_SETTLE_GRACE
}

fn is_real_submission(submission: SubmissionId) -> bool {
    submission.raw() != 0
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn interactive_resize_waits_until_target_has_settled() {
        let started_at = Instant::now();
        assert!(!interactive_resize_target_is_settled(
            started_at,
            started_at + INTERACTIVE_RESIZE_SETTLE_GRACE.saturating_sub(std::time::Duration::from_millis(1))
        ));
        assert!(interactive_resize_target_is_settled(
            started_at,
            started_at + INTERACTIVE_RESIZE_SETTLE_GRACE
        ));
    }

    #[test]
    fn interactive_resize_grace_spans_multiple_display_frames() {
        assert!(INTERACTIVE_RESIZE_SETTLE_GRACE >= std::time::Duration::from_millis(32));
    }
}
