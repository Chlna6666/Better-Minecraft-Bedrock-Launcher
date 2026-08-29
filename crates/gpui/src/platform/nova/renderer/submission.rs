use super::*;

#[cfg(target_os = "windows")]
const INTERACTIVE_RESIZE_MIN_INTERVAL: std::time::Duration = std::time::Duration::from_millis(16);
#[cfg(target_os = "windows")]
const INTERACTIVE_RESIZE_MAX_INTERVAL: std::time::Duration = std::time::Duration::from_millis(40);
#[cfg(target_os = "windows")]
const INTERACTIVE_RESIZE_SETTLE_GRACE: std::time::Duration = std::time::Duration::from_millis(12);
#[cfg(target_os = "windows")]
const INTERACTIVE_RESIZE_MAX_STALE: std::time::Duration = std::time::Duration::from_millis(50);

#[cfg(target_os = "windows")]
#[derive(Clone, Copy)]
struct InteractiveResizePacing {
    last_applied_at: Instant,
    min_interval: std::time::Duration,
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

    /// Resize only after every tracked frame submission has completed.
    ///
    /// Interactive Windows resize uses this non-blocking gate so the event loop
    /// can keep presenting the previous buffer while GPU work retires. Exact
    /// swapchain/resource recreation is paced independently of monitor refresh rate
    /// and coalesces a continuously moving resize target. Without this, a 120-240 Hz
    /// desktop can rebuild DX12/Vulkan swapchains plus path/depth/blur attachments at
    /// pointer-event frequency, which is substantially more expensive than drawing a
    /// normal frame and manifests as resize judder despite otherwise stable FPS.
    #[cfg(target_os = "windows")]
    pub(crate) fn try_resize(&mut self, size: Size<DevicePixels>) -> Result<bool> {
        let width = size.width.0.max(1) as u32;
        let height = size.height.0.max(1) as u32;
        if self.current_size.width == width && self.current_size.height == height {
            return Ok(true);
        }

        self.poll_pending_submissions()?;
        if !self.pending_submissions.is_empty() || self.backend.has_pending_resize_work()? {
            return Ok(false);
        }

        let now = Instant::now();
        if interactive_resize_should_defer(self, width, height, now) {
            return Ok(false);
        }

        let started_at = Instant::now();
        self.resize(size)?;
        record_interactive_resize(
            self,
            width,
            height,
            started_at.elapsed(),
            Instant::now(),
        );
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
            // Make the first resize after a long idle eligible immediately. Once the
            // drag is active, subsequent targets are coalesced below.
            last_applied_at: now.checked_sub(INTERACTIVE_RESIZE_MAX_STALE).unwrap_or(now),
            min_interval: INTERACTIVE_RESIZE_MIN_INTERVAL,
            target_width: width,
            target_height: height,
            target_changed_at: now,
        });

        if state.target_width != width || state.target_height != height {
            state.target_width = width;
            state.target_height = height;
            state.target_changed_at = now;
        }

        let since_applied = now.saturating_duration_since(state.last_applied_at);
        if since_applied < state.min_interval {
            return true;
        }

        // If the target is still moving faster than an ordinary display frame, keep
        // the last presented surface and let the compositor/WSI cover the live drag.
        // Periodically force an exact resize so a long drag never leaves layout stale,
        // then converge quickly as soon as the pointer settles.
        let target_is_moving =
            now.saturating_duration_since(state.target_changed_at) < INTERACTIVE_RESIZE_SETTLE_GRACE;
        target_is_moving && since_applied < INTERACTIVE_RESIZE_MAX_STALE
    })
}

#[cfg(target_os = "windows")]
fn record_interactive_resize(
    renderer: &NovaRenderer,
    width: u32,
    height: u32,
    resize_cost: std::time::Duration,
    completed_at: Instant,
) {
    let key = interactive_resize_key(renderer);
    let min_interval = interactive_resize_interval(resize_cost);
    INTERACTIVE_RESIZE_PACING.with(|pacing| {
        let mut pacing = pacing.borrow_mut();
        if pacing.len() >= 64 && !pacing.contains_key(&key) {
            pacing.clear();
        }
        match pacing.get_mut(&key) {
            Some(state) => {
                state.last_applied_at = completed_at;
                state.min_interval = min_interval;
                state.target_width = width;
                state.target_height = height;
            }
            None => {
                pacing.insert(
                    key,
                    InteractiveResizePacing {
                        last_applied_at: completed_at,
                        min_interval,
                        target_width: width,
                        target_height: height,
                        target_changed_at: completed_at,
                    },
                );
            }
        }
    });
}

#[cfg(target_os = "windows")]
fn interactive_resize_interval(resize_cost: std::time::Duration) -> std::time::Duration {
    resize_cost
        .saturating_mul(2)
        .clamp(INTERACTIVE_RESIZE_MIN_INTERVAL, INTERACTIVE_RESIZE_MAX_INTERVAL)
}

fn is_real_submission(submission: SubmissionId) -> bool {
    submission.raw() != 0
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    #[test]
    fn interactive_resize_interval_has_a_refresh_rate_independent_floor() {
        assert_eq!(
            interactive_resize_interval(std::time::Duration::from_millis(1)),
            INTERACTIVE_RESIZE_MIN_INTERVAL
        );
    }

    #[test]
    fn interactive_resize_interval_tracks_expensive_swapchain_rebuilds() {
        assert_eq!(
            interactive_resize_interval(std::time::Duration::from_millis(12)),
            std::time::Duration::from_millis(24)
        );
        assert_eq!(
            interactive_resize_interval(std::time::Duration::from_millis(30)),
            INTERACTIVE_RESIZE_MAX_INTERVAL
        );
    }
}
