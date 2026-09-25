use super::state::FrameRequestReason;
use super::*;

mod throttle;

use throttle::FrameActivity;
pub(super) use throttle::WindowFrameThrottle;

const BACKGROUND_PROGRESSIVE_FRAME_RETRY: Duration = Duration::from_millis(250);
const MINIMIZED_PROGRESSIVE_FRAME_RETRY: Duration = Duration::from_secs(1);
const FRAME_WATCHDOG_TIMEOUT: Duration = Duration::from_millis(100);
const RECENT_INPUT_DIRTY_FRAME_GRACE: Duration = Duration::from_millis(500);
#[cfg(test)]
pub(super) const DIRTY_FRAME_BACKPRESSURE_BUDGET: Duration = Duration::from_millis(4);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FrameCompletion {
    Normal,
    DeferredInactiveDirty,
}

#[derive(Clone, Copy, Default, Debug)]
pub(super) struct FrameWatchdog {
    pub(super) generation: u64,
    pub(super) pending: bool,
    pub(super) platform_generation: u64,
    pub(super) platform_pending: bool,
    pub(super) platform_options: RequestFrameOptions,
}

#[derive(Clone, Copy, Debug)]
struct FrameWorkDecision {
    activity: FrameActivity,
    defer_inactive_dirty_draw: bool,
    draw_frame: bool,
    degrade_to_present: bool,
    submit_visible_frame: bool,
    present_frame: bool,
    skip_frame: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DirtyFrameSchedulingClass {
    Normal,
    InteractiveAnimation,
}

impl DirtyFrameSchedulingClass {
    #[inline]
    fn bypasses_progressive_throttle(self) -> bool {
        matches!(self, Self::InteractiveAnimation)
    }
}

impl Window {
    pub(crate) fn record_rendered_view(&self, entity_id: EntityId, type_name: &'static str) {
        self.dirty_frame_diagnostics
            .borrow_mut()
            .record_rendered_view(entity_id, type_name);
    }

    pub(crate) fn record_selective_splice_attempt(&self) {
        self.dirty_frame_diagnostics
            .borrow_mut()
            .record_selective_splice_attempt();
    }

    pub(crate) fn record_selective_splice_hit(&self) {
        self.dirty_frame_diagnostics
            .borrow_mut()
            .record_selective_splice_hit();
    }

    #[track_caller]
    pub(super) fn record_frame_request_reason(&self, reason: FrameRequestReason) {
        self.dirty_frame_diagnostics
            .borrow_mut()
            .record_frame_request_reason(reason);
    }

    pub(crate) fn request_initial_frame(&mut self) {
        if self.has_completed_rendered_frame || self.dirty_frame_scheduled || self.refreshing {
            return;
        }

        self.record_frame_request_reason(FrameRequestReason::ExplicitRedraw);

        self.invalidator.set_dirty(true);
        self.refreshing = true;
        self.dirty_frame_scheduled = true;
        log::debug!(
            "gpui initial frame requested: window={} active={} minimized={}",
            self.handle.window_id().as_u64(),
            self.active.get(),
            self.platform_window.is_minimized()
        );
        self.request_platform_frame(RequestFrameOptions {
            require_presentation: false,
            force_render: true,
        });
    }

    pub(super) fn mark_view_dirty(&mut self, view_id: EntityId) {
        self.direct_dirty_views.insert(view_id);
        self.dirty_frame_diagnostics
            .borrow_mut()
            .record_view_dirty(view_id);
        // Keep ancestors in the traversal path. An ancestor already in the path also has all of
        // its own ancestors recorded; only `direct_dirty_views` represents direct invalidation.
        for view_id in self
            .rendered_frame
            .dispatch_tree
            .view_path_reversed(view_id)
        {
            if !self.dirty_views.insert(view_id) {
                break;
            }
        }
        self.dirty_views.insert(view_id);
        self.dirty_frame_diagnostics
            .borrow_mut()
            .record_dirty_scopes(
                self.direct_dirty_views.len(),
                self.dirty_views
                    .len()
                    .saturating_sub(self.direct_dirty_views.len()),
            );
        self.schedule_dirty_frame();
    }

    pub(crate) fn view_dirty_scope(&self, view_id: EntityId) -> Option<ViewDirtyScope> {
        if self.direct_dirty_views.contains(&view_id) {
            Some(ViewDirtyScope::Direct)
        } else if self.dirty_views.contains(&view_id) {
            Some(ViewDirtyScope::TraversalAncestor)
        } else {
            None
        }
    }

    /// Mark the window as dirty, scheduling it to be redrawn on the next frame.
    pub fn refresh(&mut self) {
        self.record_frame_request_reason(FrameRequestReason::ExplicitRedraw);
        self.dirty_frame_diagnostics.borrow_mut().record_refresh();
        self.idle_render_frames = 0;
        self.render_trim_policy = RetainedResourceTrimPolicy::None;
        self.force_view_cache_refresh = true;
        self.invalidator.set_dirty(true);
        self.schedule_dirty_frame();
    }

    pub(crate) fn schedule_dirty_frame(&mut self) {
        self.schedule_dirty_frame_with_class(DirtyFrameSchedulingClass::Normal);
    }

    /// Schedule one exact retained layout-animation frame without inheriting background progressive
    /// retry pacing. This does not clear or mutate the throttle: unrelated/background dirty work
    /// keeps its retry deadline, while this animation sample alone may request the next active frame.
    pub(crate) fn schedule_interactive_animation_frame(&mut self) {
        self.schedule_dirty_frame_with_class(DirtyFrameSchedulingClass::InteractiveAnimation);
    }

    /// Schedule a frame when an asynchronous asset or image has finished loading.
    ///
    /// An asset becoming ready represents newly available visual content that must be presented
    /// to the display even if the window is otherwise idle and receives no mouse or keyboard input.
    pub(crate) fn schedule_image_ready_frame(&mut self) {
        self.dirty_frame_deferred_pending = false;
        self.dirty_frame_throttle_pending = false;
        self.frame_throttle.clear_delay();
        self.refreshing = true;
        self.dirty_frame_scheduled = true;
        self.record_frame_request_reason(FrameRequestReason::ImageReady);
        self.request_platform_frame(RequestFrameOptions {
            require_presentation: true,
            force_render: true,
        });
    }

    fn schedule_dirty_frame_with_class(&mut self, class: DirtyFrameSchedulingClass) {
        let now = Instant::now();
        let bypass_progressive_throttle = class.bypasses_progressive_throttle();
        // Treat input newer than the current animation/frame timestamp as a one-shot latency edge.
        // Only the first dirty frame after that input may cancel an inherited progressive throttle;
        // run_platform_frame refreshes animation_time immediately, so subsequent animation frames
        // return to the normal backpressure path instead of holding an input grace window open.
        let fresh_input_edge = self.last_input_timestamp.get() > self.animation_time()
            || self.recently_received_input(now);
        if fresh_input_edge
            && (self.dirty_frame_throttle_pending || self.frame_throttle.should_delay(now))
        {
            self.frame_throttle.clear_delay();
            self.dirty_frame_throttle_pending = false;
            log::trace!(
                "gpui input edge interrupted dirty-frame throttle: window={} dirty={} refreshing={}",
                self.handle.window_id().as_u64(),
                self.invalidator.is_dirty(),
                self.refreshing
            );
        }

        let mut should_request_frame = false;
        if self.invalidator.not_drawing() {
            if self.dirty_frame_scheduled
                || (!bypass_progressive_throttle && self.dirty_frame_throttle_pending)
            {
                record_coalesced_refresh();
                log::trace!(
                    "gpui dirty frame coalesced: window={} dirty={} refreshing={} class={:?}",
                    self.handle.window_id().as_u64(),
                    self.invalidator.is_dirty(),
                    self.refreshing,
                    class
                );
            } else if self.should_defer_dirty_frame() {
                if self.dirty_frame_deferred_pending {
                    self.arm_deferred_dirty_frame_retry();
                    record_coalesced_refresh();
                    log::trace!(
                        "gpui dirty frame coalesced: window={} dirty={} refreshing={} deferred_pending=true",
                        self.handle.window_id().as_u64(),
                        self.invalidator.is_dirty(),
                        self.refreshing
                    );
                } else {
                    self.dirty_frame_deferred_pending = true;
                    self.arm_deferred_dirty_frame_retry();
                    log::trace!(
                        "gpui dirty frame deferred before platform request: window={} dirty={} active={} minimized={} pending_present={} retained_scene_len={}",
                        self.handle.window_id().as_u64(),
                        self.invalidator.is_dirty(),
                        self.active.get(),
                        self.platform_window.is_minimized(),
                        self.needs_present.get(),
                        self.rendered_frame.scene.len()
                    );
                }
            } else if !bypass_progressive_throttle && self.frame_throttle.should_delay(now) {
                self.dirty_frame_deferred_pending = false;
                self.dirty_frame_throttle_pending = true;
                record_coalesced_refresh();
                log::trace!(
                    "gpui dirty frame throttled: window={} dirty={} refreshing={}",
                    self.handle.window_id().as_u64(),
                    self.invalidator.is_dirty(),
                    self.refreshing
                );
                self.schedule_frame_throttle_retry();
            } else {
                self.dirty_frame_deferred_pending = false;
                self.refreshing = true;
                self.dirty_frame_scheduled = true;
                should_request_frame = true;
            }
        }
        if should_request_frame {
            if class.bypasses_progressive_throttle() {
                self.record_frame_request_reason(FrameRequestReason::LayoutAnimation);
            }
            log::trace!(
                "gpui dirty frame requested: window={} dirty={} refreshing={} class={:?}",
                self.handle.window_id().as_u64(),
                self.invalidator.is_dirty(),
                self.refreshing,
                class
            );
            self.request_platform_frame(RequestFrameOptions::from_refresh());
        }
    }

    pub(crate) fn should_defer_dirty_frame(&self) -> bool {
        self.should_defer_dirty_frame_at(Instant::now())
    }

    fn should_defer_dirty_frame_at(&self, now: Instant) -> bool {
        self.invalidator.is_dirty()
            && !self.active.get()
            && (!self.inactive_dirty_redraw_enabled || !self.visibility.is_visible())
            && !self.needs_present.get()
            && !self.recently_received_input(now)
            && self.next_frame_callbacks.borrow().is_empty()
            && self.rendered_frame.scene.len() != 0
    }

    fn recently_received_input(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.last_input_timestamp.get())
            <= RECENT_INPUT_DIRTY_FRAME_GRACE
    }

    pub(super) fn delay_window_frames(&mut self, duration: Duration, _cx: &mut App) {
        let now = Instant::now();
        self.frame_throttle.delay(now, duration);
        log::trace!(
            "gpui progressive frame retry armed: window={} delay={:?} active={} minimized={} dirty={}",
            self.handle.window_id().as_u64(),
            duration,
            self.active.get(),
            self.platform_window.is_minimized(),
            self.invalidator.is_dirty()
        );
        self.schedule_frame_throttle_retry();
    }

    fn progressive_frame_retry_delay(&self) -> Duration {
        if !self.visibility.is_visible() {
            MINIMIZED_PROGRESSIVE_FRAME_RETRY
        } else if !self.active.get() {
            self.inactive_dirty_frame_retry_interval
                .unwrap_or(BACKGROUND_PROGRESSIVE_FRAME_RETRY)
        } else {
            self.frame_throttle.retry_delay()
        }
    }

    fn schedule_frame_throttle_retry(&mut self) {
        let Some((retry_after, retry_generation)) = self.frame_throttle.arm_retry_timer() else {
            return;
        };

        let mut cx = self.async_app.clone();
        let executor = cx.foreground_executor().clone();
        let handle = self.handle;
        executor
            .spawn(async move {
                loop {
                    let now = Instant::now();
                    if now >= retry_after {
                        break;
                    }
                    cx.background_executor()
                        .timer(retry_after.duration_since(now))
                        .await;
                }
                let _ = ignore_window_not_found(handle.update(&mut cx, |_, window, _cx| {
                    if !window
                        .frame_throttle
                        .retry_timer_fired(retry_generation, Instant::now())
                    {
                        return;
                    }
                    window.dirty_frame_throttle_pending = false;
                    window.dirty_frame_deferred_pending = false;
                    if window.invalidator.is_dirty() && !window.refreshing {
                        window.dirty_frame_scheduled = true;
                        window.record_frame_request_reason(FrameRequestReason::ProgressiveWork);
                        window.request_platform_frame(RequestFrameOptions {
                            require_presentation: true,
                            force_render: true,
                        });
                    }
                }));
            })
            .detach();
    }

    fn arm_deferred_dirty_frame_retry(&mut self) {
        let mut retry = self.frame_watchdog.get();
        if retry.pending {
            return;
        }

        retry.generation = retry.generation.wrapping_add(1);
        retry.pending = true;
        self.frame_watchdog.set(retry);

        let generation = retry.generation;
        let delay = self.progressive_frame_retry_delay();
        let handle = self.handle;
        let mut cx = self.async_app.clone();
        let executor = cx.foreground_executor().clone();
        executor
            .spawn(async move {
                cx.background_executor().timer(delay).await;
                let _ = ignore_window_not_found(handle.update(&mut cx, |_, window, _| {
                    window.retry_deferred_dirty_frame(generation);
                }));
            })
            .detach();
    }

    pub(super) fn retry_deferred_dirty_frame(&mut self, generation: u64) {
        let retry = self.frame_watchdog.get();
        if !retry.pending || retry.generation != generation {
            return;
        }

        self.clear_deferred_dirty_frame_retry();
        if !self.dirty_frame_deferred_pending {
            return;
        }
        if !self.invalidator.is_dirty() {
            self.dirty_frame_deferred_pending = false;
            return;
        }
        if self.refreshing {
            return;
        }
        if self.should_defer_dirty_frame() {
            if self.visibility.is_visible() {
                log::trace!(
                    "gpui inactive visible dirty frame retry: window={} generation={} dirty={}",
                    self.handle.window_id().as_u64(),
                    generation,
                    self.invalidator.is_dirty()
                );
                self.dirty_frame_deferred_pending = false;
                self.refreshing = true;
                self.dirty_frame_scheduled = true;
                self.record_frame_request_reason(FrameRequestReason::ProgressiveWork);
                self.request_platform_frame(RequestFrameOptions {
                    require_presentation: true,
                    force_render: true,
                });
                return;
            }
            log::trace!(
                "gpui deferred dirty frame still deferred: window={} generation={} dirty={} active={} minimized={}",
                self.handle.window_id().as_u64(),
                generation,
                self.invalidator.is_dirty(),
                self.active.get(),
                self.platform_window.is_minimized()
            );
            return;
        }

        log::trace!(
            "gpui deferred dirty frame retry: window={} generation={} dirty={} active={} minimized={}",
            self.handle.window_id().as_u64(),
            generation,
            self.invalidator.is_dirty(),
            self.active.get(),
            self.platform_window.is_minimized()
        );
        self.dirty_frame_deferred_pending = false;
        self.schedule_dirty_frame();
    }

    fn clear_deferred_dirty_frame_retry(&mut self) {
        let mut retry = self.frame_watchdog.get();
        retry.pending = false;
        self.frame_watchdog.set(retry);
    }

    pub(super) fn request_platform_frame(&self, options: RequestFrameOptions) {
        #[cfg(feature = "profiler")]
        crate::diagnostics::foreground_profiler::record_frame_request(
            self.handle.window_id().as_u64(),
            self.active_dirty_to_present_started_at
                .or_else(|| self.invalidator.pending_dirty_started_at()),
        );
        self.platform_window.request_frame(options);
        self.arm_platform_frame_watchdog(options);
    }

    fn arm_platform_frame_watchdog(&self, options: RequestFrameOptions) {
        if !options.force_render && !options.require_presentation {
            return;
        }

        let mut watchdog = self.frame_watchdog.get();
        if !prepare_platform_frame_watchdog(&mut watchdog, options) {
            self.frame_watchdog.set(watchdog);
            return;
        }
        self.frame_watchdog.set(watchdog);

        let generation = watchdog.platform_generation;
        let handle = self.handle;
        let mut cx = self.async_app.clone();
        let executor = cx.foreground_executor().clone();
        *self.platform_frame_watchdog_task.borrow_mut() = Some(executor.spawn(async move {
            cx.background_executor().timer(FRAME_WATCHDOG_TIMEOUT).await;
            let _ = ignore_window_not_found(handle.update(&mut cx, |_, window, cx| {
                window.recover_stalled_platform_frame(generation, cx);
            }));
        }));
    }

    pub(super) fn recover_stalled_platform_frame(&mut self, generation: u64, cx: &mut App) {
        let watchdog = self.frame_watchdog.get();
        if !watchdog.platform_pending || watchdog.platform_generation != generation {
            return;
        }

        self.clear_platform_frame_watchdog();
        if !self.has_pending_platform_frame_work() {
            return;
        }

        let frame_options = watchdog.platform_options;
        if !self.active.get() && self.has_completed_rendered_frame {
            log::debug!(
                "gpui inactive platform frame waiting for compositor: window={} generation={} dirty={} refreshing={} scheduled={}",
                self.handle.window_id().as_u64(),
                generation,
                self.invalidator.is_dirty(),
                self.refreshing,
                self.dirty_frame_scheduled
            );
            return;
        }

        self.record_frame_request_reason(FrameRequestReason::Recovery);
        self.platform_window.frame_request_timed_out(frame_options);
        log::warn!(
            "gpui stalled platform frame recovery: window={} generation={} dirty={} refreshing={} scheduled={} force_render={} require_presentation={}",
            self.handle.window_id().as_u64(),
            generation,
            self.invalidator.is_dirty(),
            self.refreshing,
            self.dirty_frame_scheduled,
            frame_options.force_render,
            frame_options.require_presentation
        );

        // The platform callback is the stalled component, so recovery must run
        // the frame work directly instead of requesting another platform frame.
        if self.invalidator.is_dirty()
            || self.needs_present.get()
            || frame_options.force_render
            || frame_options.require_presentation
        {
            self.run_platform_frame(frame_options, cx);
        } else {
            self.dirty_frame_scheduled = false;
            self.refreshing = false;
        }
    }

    pub(super) fn rearm_platform_frame_watchdog_on_activation(&mut self) {
        let watchdog = self.frame_watchdog.get();
        if self.active.get()
            && self.has_pending_platform_frame_work()
            && !watchdog.platform_pending
            && watchdog.platform_options.requires_frame()
        {
            self.arm_platform_frame_watchdog(watchdog.platform_options);
        }
    }

    fn clear_platform_frame_watchdog(&mut self) {
        let mut watchdog = self.frame_watchdog.get();
        watchdog.platform_pending = false;
        self.frame_watchdog.set(watchdog);
        self.platform_frame_watchdog_task.borrow_mut().take();
    }

    fn has_pending_platform_frame_work(&self) -> bool {
        self.dirty_frame_scheduled
            || self.refreshing
            || self.needs_present.get()
            || self.animation_engine_frame_driver.get().is_some()
            || !self.next_frame_callbacks.borrow().is_empty()
    }

    pub(super) fn run_platform_frame(&mut self, frame_options: RequestFrameOptions, cx: &mut App) {
        self.clear_platform_frame_watchdog();
        let frame_started_at = Instant::now();
        self.animation_time.set(frame_started_at);
        self.frame_throttle.record_frame_start(frame_started_at);
        let frame_budget = self.frame_throttle.frame_budget();
        self.run_animation_engine_frame();

        let mut callbacks = self.next_frame_callbacks.take();
        let had_frame_callbacks = !callbacks.is_empty();
        for callback in callbacks.drain(..) {
            callback(self, cx);
        }

        let activity = FrameActivity {
            dirty: self.invalidator.is_dirty(),
            pending_present: self.needs_present.get(),
            active: self.active.get(),
            minimized: self.platform_window.is_minimized(),
        };
        let decision = self.evaluate_frame_work(
            activity,
            frame_options,
            had_frame_callbacks,
            frame_started_at,
        );

        self.log_frame_work_decision(frame_options, decision, cx);
        let presented_frame = self.execute_frame_work(frame_options, decision, frame_budget, cx);
        let frame_completed_at = Instant::now();
        record_frame_decision(decision.drew_frame(), presented_frame, decision.skip_frame);
        let window_id = self.handle.window_id().as_u64();
        if presented_frame {
            #[cfg(feature = "profiler")]
            crate::diagnostics::foreground_profiler::record_frame_presented(
                window_id,
                frame_completed_at,
            );
            if let Some(started_at) = self.active_dirty_to_present_started_at.take() {
                record_window_dirty_to_present(
                    window_id,
                    frame_completed_at.saturating_duration_since(started_at),
                );
            }
        } else if decision.drew_frame() && !self.needs_present.get() {
            // A completed dirty draw that produced no visible change has no presentation to await.
            self.active_dirty_to_present_started_at = None;
            #[cfg(feature = "profiler")]
            crate::diagnostics::foreground_profiler::record_frame_no_present(window_id);
        }
        record_window_runtime_state(
            window_id,
            self.viewport_size.width / px(1.0),
            self.viewport_size.height / px(1.0),
            self.scale_factor,
            activity.active,
            activity.minimized,
            self.visibility.is_visible(),
        );
        record_window_frame_disposition(
            window_id,
            decision.disposition(
                presented_frame,
                decision
                    .drew_frame()
                    .then(|| frame_completed_at.saturating_duration_since(frame_started_at)),
            ),
        );
    }

    fn run_animation_engine_frame(&mut self) {
        let Some(driver) = self.animation_engine_frame_driver.take() else {
            return;
        };
        if !self.visibility.is_visible()
            || (!self.active.get() && !self.inactive_animation_engine_enabled)
        {
            self.animation_engine_frame_driver.set(Some(driver));
            return;
        }

        let tick = self
            .animation_engine
            .borrow_mut()
            .tick_driver(driver, self.animation_time());
        self.backdrop_blur_damage_plan = self
            .rendered_frame
            .scene
            .backdrop_blur_animation_damage_plan(&tick.scene_values);
        self.rendered_frame
            .scene
            .replace_engine_animation_values(tick.scene_values);
        let viewport = Bounds::new(Point::default(), self.viewport_size);
        if !tick.dirty_bounds.is_empty() {
            self.render_dirty_region = DirtyRegion::empty();
        }
        for bounds in tick.dirty_bounds {
            self.record_animation_tick_dirty_bounds(bounds, viewport);
        }
        for bounds in self
            .rendered_frame
            .scene
            .backdrop_blur_output_damage(&self.backdrop_blur_damage_plan)
        {
            self.render_dirty_region.push(bounds);
        }
        self.render_dirty_region.coalesce_if_large(
            viewport.scale(self.scale_factor),
            DIRTY_REGION_FULL_REDRAW_RATIO,
        );
        if tick.active_visual_count > 0
            && tick.has_gpu_or_paint
            && self.visibility.is_visible()
            && (self.active.get() || self.inactive_animation_engine_enabled)
        {
            let interval = self
                .animation_engine
                .borrow()
                .visual_presentation_interval(driver)
                .unwrap_or(Duration::ZERO);
            if interval.is_zero() {
                self.request_animation_engine_frame(driver);
            } else {
                self.request_animation_engine_frame_at(
                    driver,
                    self.animation_time() + interval,
                );
            }
        }
        if tick.has_layout {
            self.request_animation_frame();
        }
    }

    fn evaluate_frame_work(
        &self,
        activity: FrameActivity,
        frame_options: RequestFrameOptions,
        had_frame_callbacks: bool,
        frame_started_at: Instant,
    ) -> FrameWorkDecision {
        let defer_inactive_dirty_draw = self.should_defer_inactive_dirty_draw(
            activity,
            frame_options,
            had_frame_callbacks,
            frame_started_at,
        );
        let draw_frame =
            !defer_inactive_dirty_draw && (activity.dirty || frame_options.force_render);
        let degrade_to_present = draw_frame
            && self.should_degrade_dirty_frame_to_retained_present(frame_options, frame_started_at);
        let submit_visible_frame = draw_frame
            || degrade_to_present
            || (!draw_frame && (frame_options.require_presentation || activity.pending_present));
        let present_frame = degrade_to_present
            || (!draw_frame && (frame_options.require_presentation || activity.pending_present));
        let skip_frame = !draw_frame && !submit_visible_frame;
        FrameWorkDecision {
            activity,
            defer_inactive_dirty_draw,
            draw_frame,
            degrade_to_present,
            submit_visible_frame,
            present_frame,
            skip_frame,
        }
    }

    fn log_frame_work_decision(
        &self,
        frame_options: RequestFrameOptions,
        decision: FrameWorkDecision,
        cx: &App,
    ) {
        if log::log_enabled!(log::Level::Trace) {
            let dirty_frame_diagnostics = *self.dirty_frame_diagnostics.borrow();
            let first_view_dirty_entity = dirty_frame_diagnostics.first_view_dirty_entity;
            let first_notify_entity = dirty_frame_diagnostics.first_notify_entity;
            log::trace!(
                "gpui frame request: window={} request_id={} dirty={} force_render={} require_presentation={} pending_present={} active={} minimized={} draw={} present={} skip={} defer_inactive_dirty={} dirty_refreshes={} dirty_view_marks={} direct_dirty_views={} traversal_ancestor_views={} dirty_notify_invalidations={} first_view_dirty_entity={:?} first_view_dirty_entity_type={:?} first_notify_entity={:?} first_notify_entity_type={:?}",
                self.handle.window_id().as_u64(),
                0,
                decision.activity.dirty,
                frame_options.force_render,
                frame_options.require_presentation,
                decision.activity.pending_present,
                decision.activity.active,
                decision.activity.minimized,
                decision.drew_frame(),
                decision.submit_visible_frame,
                decision.skip_frame,
                decision.defer_inactive_dirty_draw,
                dirty_frame_diagnostics.refreshes,
                dirty_frame_diagnostics.view_dirty,
                dirty_frame_diagnostics.direct_dirty_views,
                dirty_frame_diagnostics.traversal_ancestor_views,
                dirty_frame_diagnostics.notify_invalidations,
                first_view_dirty_entity.map(EntityId::as_u64),
                first_view_dirty_entity.map(|entity_id| cx.entity_type_name(entity_id)),
                first_notify_entity.map(EntityId::as_u64),
                first_notify_entity.map(|entity_id| cx.entity_type_name(entity_id))
            );
        }
    }

    fn execute_frame_work(
        &mut self,
        frame_options: RequestFrameOptions,
        decision: FrameWorkDecision,
        frame_budget: Duration,
        cx: &mut App,
    ) -> bool {
        let presented_frame = if decision.degrade_to_present {
            let result = self.present_framebuffer_only();
            self.refreshing = false;
            result == PlatformFrameResult::Submitted
        } else if decision.defer_inactive_dirty_draw {
            self.refreshing = false;
            log::trace!(
                "gpui inactive dirty frame deferred: window={} request_id={} dirty={} force_render={} pending_present={} retained_scene_len={}",
                self.handle.window_id().as_u64(),
                0,
                decision.activity.dirty,
                frame_options.force_render,
                decision.activity.pending_present,
                self.rendered_frame.scene.len()
            );
            false
        } else if decision.draw_frame {
            self.draw_visible_frame(frame_options.require_presentation, frame_budget, cx)
        } else if decision.present_frame {
            self.present_framebuffer_only() == PlatformFrameResult::Submitted
        } else if decision.activity.active {
            record_retained_frame_skip();
            false
        } else {
            record_inactive_present_skip();
            false
        };

        self.complete_frame(if decision.defer_inactive_dirty_draw {
            FrameCompletion::DeferredInactiveDirty
        } else {
            FrameCompletion::Normal
        });
        presented_frame
    }

    fn draw_visible_frame(
        &mut self,
        require_presentation: bool,
        frame_budget: Duration,
        cx: &mut App,
    ) -> bool {
        let draw_started_at = Instant::now();
        let arena_clear_needed = measure("frame generation", || {
            #[cfg(feature = "profiler")]
            let _profile =
                crate::diagnostics::foreground_profiler::ForegroundWorkSpan::draw(
                    self.handle.window_id().as_u64(),
                );
            self.draw(cx)
        });
        let draw_elapsed = draw_started_at.elapsed();
        let presented_frame = if require_presentation || self.needs_present.get() {
            measure("frame presentation", || self.present()) == PlatformFrameResult::Submitted
        } else {
            false
        };
        measure("frame arena clear", || arena_clear_needed.clear());
        self.finish_draw_budget_accounting(draw_elapsed, frame_budget, cx);
        presented_frame
    }

    fn finish_draw_budget_accounting(
        &mut self,
        generation_elapsed: Duration,
        progressive_budget: Duration,
        cx: &mut App,
    ) {
        let draw_was_degraded = self.draw_was_degraded;
        let warning_budget = self.frame_throttle.generation_warning_budget();
        let generation_budget_missed = generation_elapsed >= warning_budget;
        if generation_budget_missed
            && log::log_enabled!(log::Level::Warn)
            && self
                .frame_throttle
                .should_warn_generation_budget_miss(Instant::now())
        {
            let stats = self.last_generation_stats;
            let dirty_frame_diagnostics = *self.dirty_frame_diagnostics.borrow();
            let first_frame_request = dirty_frame_diagnostics.first_frame_request;
            let first_view_dirty_entity = dirty_frame_diagnostics.first_view_dirty_entity;
            let first_rendered_entity = dirty_frame_diagnostics.first_rendered_entity;
            let first_notify_entity = dirty_frame_diagnostics.first_notify_entity;
            log::warn!(
                "gpui frame generation budget hit: window={} elapsed={:?} budget={:?} progressive_budget={:?} progressive_degraded={} degraded_count={} recovery_full_redraw_count={} deadline_remaining_at_prepaint_start_us={:?} deadline_remaining_at_layout_start_us={:?} deadline_remaining_at_paint_start_us={:?} layout_nodes={} measured_layout_nodes={} layout_roots={} layout_cache_hits={} layout_cache_misses={} layout_cache_reused_roots={} layout_cache_saved_nodes={} layout_bounds_cache_hits={} layout_bounds_cache_misses={} text_layout_hits={} text_layout_reuses={} text_layout_misses={} list_measured_items={} scene_primitives={} scene_batches={} scene_replayed_primitives={} scene_retained_capacity={} frame_retained_capacity={} dirty_refreshes={} dirty_view_marks={} direct_dirty_views={} traversal_ancestor_views={} selective_splice_attempts={} selective_splice_hits={} rendered_views={} rendered_view_types={:?} rendered_view_type_overflow={} dirty_notify_invalidations={} frame_request_reasons=0x{:04x} first_frame_request={:?} first_view_dirty_entity={:?} first_view_dirty_entity_type={:?} first_rendered_entity={:?} first_rendered_entity_type={:?} first_notify_entity={:?} first_notify_entity_type={:?}",
                self.handle.window_id().as_u64(),
                generation_elapsed,
                warning_budget,
                progressive_budget,
                draw_was_degraded,
                self.degraded_draw_count,
                self.recovery_full_redraw_count,
                stats.deadline_remaining_at_prepaint_start_us,
                stats.deadline_remaining_at_layout_start_us,
                stats.deadline_remaining_at_paint_start_us,
                stats.layout.nodes,
                stats.layout.measured_nodes,
                stats.layout.roots,
                stats.layout_cache.hits,
                stats.layout_cache.misses,
                stats.layout.cache_reused_roots,
                stats.layout.cache_saved_roots,
                stats.layout.bounds_cache_hits,
                stats.layout.bounds_cache_misses,
                stats.text_layout.hits,
                stats.text_layout.reuses,
                stats.text_layout.misses,
                stats.list_measured_items,
                stats.scene.primitives,
                stats.scene.batches,
                stats.scene.replayed_primitives,
                stats.scene.retained_capacity,
                stats.frame_retained_capacity,
                dirty_frame_diagnostics.refreshes,
                dirty_frame_diagnostics.view_dirty,
                dirty_frame_diagnostics.direct_dirty_views,
                dirty_frame_diagnostics.traversal_ancestor_views,
                dirty_frame_diagnostics.selective_splice_attempts,
                dirty_frame_diagnostics.selective_splice_hits,
                dirty_frame_diagnostics.rendered_views,
                &dirty_frame_diagnostics.rendered_view_types
                    [..dirty_frame_diagnostics.rendered_view_type_count],
                dirty_frame_diagnostics.rendered_view_type_overflow,
                dirty_frame_diagnostics.notify_invalidations,
                dirty_frame_diagnostics.frame_request_reasons,
                first_frame_request,
                first_view_dirty_entity.map(EntityId::as_u64),
                first_view_dirty_entity.map(|entity_id| cx.entity_type_name(entity_id)),
                first_rendered_entity.map(EntityId::as_u64),
                first_rendered_entity.map(|entity_id| cx.entity_type_name(entity_id)),
                first_notify_entity.map(EntityId::as_u64),
                first_notify_entity.map(|entity_id| cx.entity_type_name(entity_id)),
            );
        }
        if draw_was_degraded {
            self.delay_window_frames(self.progressive_frame_retry_delay(), cx);
        }
    }

    fn should_defer_inactive_dirty_draw(
        &self,
        load: FrameActivity,
        options: RequestFrameOptions,
        had_frame_callbacks: bool,
        now: Instant,
    ) -> bool {
        load.dirty
            && options.force_render
            && !options.require_presentation
            && !load.pending_present
            && !load.active
            && (!self.inactive_dirty_redraw_enabled || load.minimized)
            && !had_frame_callbacks
            && !self.recently_received_input(now)
            && (self.rendered_frame.scene.len() != 0 || load.minimized)
    }

    fn should_degrade_dirty_frame_to_retained_present(
        &self,
        options: RequestFrameOptions,
        now: Instant,
    ) -> bool {
        !options.force_render
            && !options.require_presentation
            && self.transparent_caption_height.is_none()
            && self.dirty_views.is_empty()
            && self.animation_dirty_region.is_empty()
            && !self.recently_received_input(now)
            && self.animation_engine_frame_driver.get().is_none()
            && !self.dirty_frame_diagnostics.borrow().is_interactive_or_animating()
            && self.frame_throttle.should_delay(now)
            && self.rendered_frame.scene.len() != 0
    }

    pub(crate) fn draw_budget_exhausted(&self) -> bool {
        if !self.allows_progressive_frame_degradation() {
            return false;
        }

        self.draw_deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    pub(crate) fn draw_was_degraded(&self) -> bool {
        self.draw_was_degraded
    }

    fn allows_progressive_frame_degradation(&self) -> bool {
        let diagnostics = self.dirty_frame_diagnostics.borrow();
        self.has_completed_rendered_frame
            // The recovery frame after a degraded draw must present progress instead of
            // repeatedly discarding dirty work.
            && !self.recovering_degraded_draw
            && self.transparent_caption_height.is_none()
            && !self.recently_received_input(Instant::now())
            && self.animation_engine_frame_driver.get().is_none()
            && self.animation_dirty_region.is_empty()
            && !diagnostics.is_interactive_or_animating()
            // Image-ready frames are latency critical too: once pixels become available, replaying
            // an older progressive subtree for one more frame produces a visible late pop-in.
            && !diagnostics.requires_fresh_progressive_views()
    }

    pub(crate) fn with_critical_draw<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.critical_draw_depth = self.critical_draw_depth.saturating_add(1);
        let result = f(self);
        self.critical_draw_depth = self.critical_draw_depth.saturating_sub(1);
        result
    }

    pub(crate) fn force_view_cache_refresh(&self) -> bool {
        self.force_view_cache_refresh
    }

    fn record_animation_tick_dirty_bounds(
        &mut self,
        bounds: Bounds<Pixels>,
        viewport: Bounds<Pixels>,
    ) {
        let clipped_bounds = bounds.intersect(&viewport);
        if clipped_bounds.is_empty() {
            return;
        }

        let viewport = viewport.scale(self.scale_factor);
        self.render_dirty_region
            .push(clipped_bounds.scale(self.scale_factor));
        self.render_dirty_region
            .coalesce_if_large(viewport, DIRTY_REGION_FULL_REDRAW_RATIO);
        self.render_present_mode = if self.render_dirty_region.is_full() {
            PartialPresentMode::FullRedraw
        } else {
            PartialPresentMode::Partial
        };
        record_dirty_region_metrics(
            self.render_dirty_region.rect_count(),
            self.render_dirty_region.area() as usize,
        );
    }

    pub(crate) fn degrade_current_draw(&mut self) {
        if !self.allows_progressive_frame_degradation() {
            return;
        }

        self.draw_was_degraded = true;
    }

    pub(super) fn complete_frame(&mut self, completion: FrameCompletion) {
        self.dirty_frame_scheduled = false;
        if completion == FrameCompletion::Normal {
            self.dirty_frame_deferred_pending = false;
            self.clear_deferred_dirty_frame_retry();
        }
        let was_dirty = self.invalidator.is_dirty();
        let previous_idle_render_frames = self.idle_render_frames;
        if self.invalidator.is_dirty() {
            self.idle_render_frames = 0;
            self.render_trim_policy = RetainedResourceTrimPolicy::None;
            if completion == FrameCompletion::Normal {
                self.schedule_dirty_frame();
            }
        } else {
            self.idle_render_frames = self.idle_render_frames.saturating_add(1);
            self.render_trim_policy = if self.idle_render_frames >= WINDOW_STRONG_TRIM_IDLE_FRAMES {
                RetainedResourceTrimPolicy::Strong
            } else if self.idle_render_frames >= WINDOW_LIGHT_TRIM_IDLE_FRAMES {
                RetainedResourceTrimPolicy::Light
            } else {
                RetainedResourceTrimPolicy::None
            };
        }
        let trim_level = if previous_idle_render_frames < WINDOW_STRONG_TRIM_IDLE_FRAMES
            && self.idle_render_frames >= WINDOW_STRONG_TRIM_IDLE_FRAMES
        {
            // Aggressive trim clears text layout cache entries. Keep visible idle trim to capacity
            // reduction so retained subtree layout-index reuse stays valid.
            Some(GpuiMemoryTrimLevel::Moderate)
        } else if previous_idle_render_frames < WINDOW_LIGHT_TRIM_IDLE_FRAMES
            && self.idle_render_frames >= WINDOW_LIGHT_TRIM_IDLE_FRAMES
        {
            Some(GpuiMemoryTrimLevel::Light)
        } else {
            None
        };
        if let Some(trim_level) = trim_level {
            self.trim_gpui_memory(trim_level);
        }
        self.platform_window.completed_frame();
        let dirty_frame_diagnostics =
            std::mem::take(&mut *self.dirty_frame_diagnostics.borrow_mut());
        log::trace!(
            "gpui complete_frame: window={} was_dirty={} refreshing={} idle_render_frames={} needs_present={} trim_policy={:?} completion={:?} dirty_refreshes={} dirty_view_marks={} direct_dirty_views={} traversal_ancestor_views={} selective_splice_attempts={} selective_splice_hits={} rendered_views={} rendered_view_types={:?} rendered_view_type_overflow={} dirty_notify_invalidations={} frame_request_reasons=0x{:04x} first_frame_request={:?} first_view_dirty_entity={:?} first_rendered_entity={:?} first_notify_entity={:?}",
            self.handle.window_id().as_u64(),
            was_dirty,
            self.refreshing,
            self.idle_render_frames,
            self.needs_present.get(),
            self.render_trim_policy,
            completion,
            dirty_frame_diagnostics.refreshes,
            dirty_frame_diagnostics.view_dirty,
            dirty_frame_diagnostics.direct_dirty_views,
            dirty_frame_diagnostics.traversal_ancestor_views,
            dirty_frame_diagnostics.selective_splice_attempts,
            dirty_frame_diagnostics.selective_splice_hits,
            dirty_frame_diagnostics.rendered_views,
            &dirty_frame_diagnostics.rendered_view_types
                [..dirty_frame_diagnostics.rendered_view_type_count],
            dirty_frame_diagnostics.rendered_view_type_overflow,
            dirty_frame_diagnostics.notify_invalidations,
            dirty_frame_diagnostics.frame_request_reasons,
            dirty_frame_diagnostics.first_frame_request,
            dirty_frame_diagnostics
                .first_view_dirty_entity
                .map(EntityId::as_u64),
            dirty_frame_diagnostics
                .first_rendered_entity
                .map(EntityId::as_u64),
            dirty_frame_diagnostics
                .first_notify_entity
                .map(EntityId::as_u64)
        );
    }
}

fn prepare_platform_frame_watchdog(
    watchdog: &mut FrameWatchdog,
    options: RequestFrameOptions,
) -> bool {
    if watchdog.platform_pending {
        // The platform owns one latest-wins frame slot. Keep the watchdog on the first request's
        // deadline as well: rearming it for every coalesced animation request both creates
        // avoidable executor work and can postpone recovery forever under load.
        watchdog.platform_options = watchdog.platform_options.merge(options);
        return false;
    }

    watchdog.platform_generation = watchdog.platform_generation.wrapping_add(1);
    watchdog.platform_pending = true;
    watchdog.platform_options = options;
    true
}

impl FrameWorkDecision {
    const fn drew_frame(self) -> bool {
        self.draw_frame && !self.degrade_to_present
    }

    const fn disposition(
        self,
        presented_frame: bool,
        frame_duration: Option<Duration>,
    ) -> WindowFrameDisposition {
        WindowFrameDisposition {
            drew_frame: self.drew_frame(),
            frame_duration,
            presented_frame,
            skipped_frame: self.skip_frame,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesced_platform_requests_keep_one_watchdog_deadline() {
        let mut watchdog = FrameWatchdog::default();
        assert!(prepare_platform_frame_watchdog(
            &mut watchdog,
            RequestFrameOptions {
                require_presentation: true,
                force_render: false,
            }
        ));
        let first_generation = watchdog.platform_generation;

        assert!(!prepare_platform_frame_watchdog(
            &mut watchdog,
            RequestFrameOptions {
                require_presentation: false,
                force_render: true,
            }
        ));
        assert_eq!(watchdog.platform_generation, first_generation);
        assert!(watchdog.platform_pending);
        assert_eq!(
            watchdog.platform_options,
            RequestFrameOptions {
                require_presentation: true,
                force_render: true,
            }
        );
    }
}
