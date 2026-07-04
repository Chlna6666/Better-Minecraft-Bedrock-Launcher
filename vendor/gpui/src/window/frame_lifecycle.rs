use super::*;

mod throttle;

use throttle::FrameActivity;
pub(super) use throttle::WindowFrameThrottle;

pub(crate) const TARGET_FRAME_GENERATION_BUDGET: Duration = Duration::from_millis(4);
const BACKGROUND_PROGRESSIVE_FRAME_RETRY: Duration = Duration::from_millis(250);
const MINIMIZED_PROGRESSIVE_FRAME_RETRY: Duration = Duration::from_secs(1);
const FRAME_WATCHDOG_TIMEOUT: Duration = Duration::from_millis(100);
const RECENT_INPUT_DIRTY_FRAME_GRACE: Duration = Duration::from_millis(500);
pub(super) const DIRTY_FRAME_BACKPRESSURE_BUDGET: Duration = TARGET_FRAME_GENERATION_BUDGET;
pub(super) const DIRTY_FRAME_BACKPRESSURE_DEFERRED_DELAY: Duration = Duration::from_millis(8);

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

impl Window {
    pub(crate) fn request_initial_frame(&mut self) {
        if self.has_completed_rendered_frame || self.dirty_frame_scheduled || self.refreshing {
            return;
        }

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
        self.dirty_frame_diagnostics
            .borrow_mut()
            .record_view_dirty(view_id);
        // Mark ancestor views as dirty. If already in the `dirty_views` set, then all its ancestors
        // should already be dirty.
        for view_id in self
            .rendered_frame
            .dispatch_tree
            .view_path(view_id)
            .into_iter()
            .rev()
        {
            if !self.dirty_views.insert(view_id) {
                break;
            }
        }
        self.schedule_dirty_frame();
    }

    /// Mark the window as dirty, scheduling it to be redrawn on the next frame.
    pub fn refresh(&mut self) {
        self.dirty_frame_diagnostics.borrow_mut().record_refresh();
        self.idle_render_frames = 0;
        self.render_trim_policy = RetainedResourceTrimPolicy::None;
        self.force_view_cache_refresh = true;
        self.invalidator.set_dirty(true);
        self.schedule_dirty_frame();
    }

    pub(crate) fn schedule_dirty_frame(&mut self) {
        let mut should_request_frame = false;
        if self.invalidator.not_drawing() {
            if self.dirty_frame_scheduled || self.dirty_frame_throttle_pending {
                record_coalesced_refresh();
                log::trace!(
                    "gpui dirty frame coalesced: window={} dirty={} refreshing={}",
                    self.handle.window_id().as_u64(),
                    self.invalidator.is_dirty(),
                    self.refreshing
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
            } else if self.frame_throttle.should_delay(Instant::now()) {
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
            log::trace!(
                "gpui dirty frame requested: window={} dirty={} refreshing={}",
                self.handle.window_id().as_u64(),
                self.invalidator.is_dirty(),
                self.refreshing
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
        if self.platform_window.is_minimized() {
            MINIMIZED_PROGRESSIVE_FRAME_RETRY
        } else if !self.active.get() {
            BACKGROUND_PROGRESSIVE_FRAME_RETRY
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

    fn request_platform_frame(&mut self, options: RequestFrameOptions) {
        self.platform_window.request_frame(options);
        self.arm_platform_frame_watchdog(options);
    }

    fn arm_platform_frame_watchdog(&mut self, options: RequestFrameOptions) {
        if !options.force_render && !options.require_presentation {
            return;
        }

        let mut watchdog = self.frame_watchdog.get();
        watchdog.platform_generation = watchdog.platform_generation.wrapping_add(1);
        watchdog.platform_pending = true;
        watchdog.platform_options = options;
        self.frame_watchdog.set(watchdog);

        let generation = watchdog.platform_generation;
        let handle = self.handle;
        let mut cx = self.async_app.clone();
        let executor = cx.foreground_executor().clone();
        executor
            .spawn(async move {
                cx.background_executor().timer(FRAME_WATCHDOG_TIMEOUT).await;
                let _ = ignore_window_not_found(handle.update(&mut cx, |_, window, cx| {
                    window.recover_stalled_platform_frame(generation, cx);
                }));
            })
            .detach();
    }

    pub(super) fn recover_stalled_platform_frame(&mut self, generation: u64, cx: &mut App) {
        let watchdog = self.frame_watchdog.get();
        if !watchdog.platform_pending || watchdog.platform_generation != generation {
            return;
        }

        self.clear_platform_frame_watchdog();
        if !self.dirty_frame_scheduled && !self.refreshing {
            return;
        }

        let frame_options = watchdog.platform_options;
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

    fn clear_platform_frame_watchdog(&mut self) {
        let mut watchdog = self.frame_watchdog.get();
        watchdog.platform_pending = false;
        self.frame_watchdog.set(watchdog);
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

        record_frame_decision(
            decision.drew_frame(),
            decision.submit_visible_frame,
            decision.skip_frame,
        );
        record_window_frame_disposition(self.handle.window_id().as_u64(), decision.disposition());
        self.log_frame_work_decision(frame_options, decision, cx);
        self.execute_frame_work(frame_options, decision, frame_budget, cx);
    }

    fn run_animation_engine_frame(&mut self) {
        let Some(driver) = self.animation_engine_frame_driver.take() else {
            return;
        };

        let tick = self
            .animation_engine
            .borrow_mut()
            .tick_driver(driver, self.animation_time());
        let viewport = Bounds::new(Point::default(), self.viewport_size);
        if !tick.dirty_bounds.is_empty() {
            self.render_dirty_region = DirtyRegion::empty();
        }
        for bounds in tick.dirty_bounds {
            self.record_animation_tick_dirty_bounds(bounds, viewport);
        }
        if tick.active_count > 0 && tick.has_gpu_or_paint {
            self.request_animation_engine_frame(driver);
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
                "gpui frame request: window={} request_id={} dirty={} force_render={} require_presentation={} pending_present={} active={} minimized={} draw={} present={} skip={} defer_inactive_dirty={} dirty_refreshes={} dirty_view_marks={} dirty_notify_invalidations={} first_view_dirty_entity={:?} first_view_dirty_entity_type={:?} first_notify_entity={:?} first_notify_entity_type={:?}",
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
    ) {
        if decision.degrade_to_present {
            self.present_framebuffer_only();
            self.refreshing = false;
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
        } else if decision.draw_frame {
            self.draw_visible_frame(frame_budget, cx);
        } else if decision.present_frame {
            self.present_framebuffer_only();
        } else if decision.activity.active {
            record_retained_frame_skip();
        } else {
            record_inactive_present_skip();
        }

        self.complete_frame(if decision.defer_inactive_dirty_draw {
            FrameCompletion::DeferredInactiveDirty
        } else {
            FrameCompletion::Normal
        });
    }

    fn draw_visible_frame(&mut self, frame_budget: Duration, cx: &mut App) {
        let draw_started_at = Instant::now();
        let arena_clear_needed = measure("frame generation", || self.draw(cx));
        let draw_elapsed = draw_started_at.elapsed();
        if self.needs_present.get() {
            measure("frame presentation", || self.present());
        }
        measure("frame arena clear", || arena_clear_needed.clear());
        self.finish_draw_budget_accounting(draw_elapsed, frame_budget, cx);
    }

    fn finish_draw_budget_accounting(
        &mut self,
        generation_elapsed: Duration,
        frame_budget: Duration,
        cx: &mut App,
    ) {
        let draw_was_degraded = self.draw_was_degraded;
        let generation_budget_missed = generation_elapsed >= frame_budget;
        if generation_budget_missed
            && log::log_enabled!(log::Level::Warn)
            && self
                .frame_throttle
                .should_warn_generation_budget_miss(Instant::now())
        {
            let stats = self.last_generation_stats;
            let dirty_frame_diagnostics = *self.dirty_frame_diagnostics.borrow();
            let first_view_dirty_entity = dirty_frame_diagnostics.first_view_dirty_entity;
            let first_notify_entity = dirty_frame_diagnostics.first_notify_entity;
            log::warn!(
                "gpui frame generation budget hit: window={} elapsed={:?} budget={:?} progressive_degraded={} layout_nodes={} measured_layout_nodes={} layout_roots={} layout_cache_hits={} layout_cache_misses={} layout_bounds_cache_hits={} layout_bounds_cache_misses={} text_layout_hits={} text_layout_reuses={} text_layout_misses={} list_measured_items={} scene_primitives={} scene_batches={} scene_retained_capacity={} frame_retained_capacity={} dirty_refreshes={} dirty_view_marks={} dirty_notify_invalidations={} first_view_dirty_entity={:?} first_view_dirty_entity_type={:?} first_notify_entity={:?} first_notify_entity_type={:?}",
                self.handle.window_id().as_u64(),
                generation_elapsed,
                frame_budget,
                draw_was_degraded,
                stats.layout.nodes,
                stats.layout.measured_nodes,
                stats.layout.roots,
                stats.layout_cache.hits,
                stats.layout_cache.misses,
                stats.layout.bounds_cache_hits,
                stats.layout.bounds_cache_misses,
                stats.text_layout.hits,
                stats.text_layout.reuses,
                stats.text_layout.misses,
                stats.list_measured_items,
                stats.scene.primitives,
                stats.scene.batches,
                stats.scene.retained_capacity,
                stats.frame_retained_capacity,
                dirty_frame_diagnostics.refreshes,
                dirty_frame_diagnostics.view_dirty,
                dirty_frame_diagnostics.notify_invalidations,
                first_view_dirty_entity.map(EntityId::as_u64),
                first_view_dirty_entity.map(|entity_id| cx.entity_type_name(entity_id)),
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

    pub(crate) fn draw_budget_exhausted_for_optional_work(&self) -> bool {
        self.critical_draw_depth == 0 && self.draw_budget_exhausted()
    }

    fn allows_progressive_frame_degradation(&self) -> bool {
        self.has_completed_rendered_frame
            // The recovery frame after a degraded draw must present progress instead of
            // repeatedly discarding dirty work.
            && !self.recovering_degraded_draw
            && self.transparent_caption_height.is_none()
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

    pub(crate) fn mark_animation_dirty(&mut self, bounds: Bounds<Pixels>) {
        self.invalidator.debug_assert_paint_or_prepaint();
        self.record_animation_dirty_bounds_for_next_draw(bounds, self.content_mask().bounds);
    }

    fn record_animation_dirty_bounds_for_next_draw(
        &mut self,
        bounds: Bounds<Pixels>,
        clip_bounds: Bounds<Pixels>,
    ) {
        let clipped_bounds = bounds.intersect(&clip_bounds);
        if !clipped_bounds.is_empty() {
            self.animation_dirty_region
                .push(clipped_bounds.scale(self.scale_factor));
        }
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
            "gpui complete_frame: window={} was_dirty={} refreshing={} idle_render_frames={} needs_present={} trim_policy={:?} completion={:?} dirty_refreshes={} dirty_view_marks={} dirty_notify_invalidations={} first_view_dirty_entity={:?} first_notify_entity={:?}",
            self.handle.window_id().as_u64(),
            was_dirty,
            self.refreshing,
            self.idle_render_frames,
            self.needs_present.get(),
            self.render_trim_policy,
            completion,
            dirty_frame_diagnostics.refreshes,
            dirty_frame_diagnostics.view_dirty,
            dirty_frame_diagnostics.notify_invalidations,
            dirty_frame_diagnostics
                .first_view_dirty_entity
                .map(EntityId::as_u64),
            dirty_frame_diagnostics
                .first_notify_entity
                .map(EntityId::as_u64)
        );
    }
}

impl FrameWorkDecision {
    const fn drew_frame(self) -> bool {
        self.draw_frame && !self.degrade_to_present
    }

    const fn disposition(self) -> WindowFrameDisposition {
        WindowFrameDisposition {
            drew_frame: self.drew_frame(),
            presented_frame: self.submit_visible_frame,
            skipped_frame: self.skip_frame,
        }
    }
}
