use super::lifecycle::RetainedInvalidationScope;
use super::state::FrameRequestReason;
use super::*;
use crate::{AnimationSpec, SceneAnimationId, TransitionProperty};

impl Window {
    /// Schedules the given function to be run at the end of the current effect cycle, allowing entities
    /// that are currently on the stack to be returned to the app.
    pub fn defer(&self, cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static) {
        let handle = self.handle;
        cx.defer(move |cx| {
            handle.update(cx, |_, window, cx| f(window, cx)).ok();
        });
    }

    /// Creates an [`AsyncWindowContext`], which has a static lifetime and can be held across
    /// await points in async code.
    pub fn to_async(&self, cx: &App) -> AsyncWindowContext {
        AsyncWindowContext::new_context(cx.to_async(), self.handle)
    }

    fn enqueue_next_frame_callback(
        &self,
        reason: FrameRequestReason,
        callback: impl FnOnce(&mut Window, &mut App) + 'static,
    ) {
        let should_request_frame = {
            let mut next_frame_callbacks = self.next_frame_callbacks.borrow_mut();
            let should_request_frame = next_frame_callbacks.is_empty();
            next_frame_callbacks.push(Box::new(callback));
            should_request_frame
        };
        if should_request_frame {
            self.record_frame_request_reason(reason);
            self.request_platform_frame(RequestFrameOptions {
                require_presentation: true,
                force_render: false,
            });
        }
    }

    /// Schedule the given closure to be run directly after the current frame is rendered.
    pub fn on_next_frame(&self, callback: impl FnOnce(&mut Window, &mut App) + 'static) {
        self.enqueue_next_frame_callback(FrameRequestReason::ExplicitRedraw, callback);
    }

    /// Schedule a presentation-animation callback without classifying the request as an explicit
    /// redraw. Animation elements use this so diagnostics and scheduling see one cadence owner.
    pub(crate) fn on_next_presentation_frame(
        &self,
        callback: impl FnOnce(&mut Window, &mut App) + 'static,
    ) {
        self.enqueue_next_frame_callback(FrameRequestReason::PresentationAnimation, callback);
    }

    /// Schedule deferred upload/progressive work without misclassifying it as an explicit redraw.
    pub(crate) fn on_next_progressive_frame(
        &self,
        callback: impl FnOnce(&mut Window, &mut App) + 'static,
    ) {
        self.enqueue_next_frame_callback(FrameRequestReason::ProgressiveWork, callback);
    }

    /// Schedule a frame to be drawn on the next animation frame.
    ///
    /// This is useful for elements that need to animate continuously, such as a video player or an animated GIF.
    /// It will cause the window to redraw on the next frame, even if no other changes have occurred.
    ///
    /// If called from within a view, it will notify that view on the next frame. Otherwise, it will refresh the entire window.
    #[track_caller]
    pub fn request_animation_frame(&self) {
        let Some(entity) = self.current_view_or_root() else {
            return;
        };
        if !self
            .animation_frame_pending_entities
            .borrow_mut()
            .insert(entity)
        {
            record_coalesced_refresh();
            return;
        }

        if log::log_enabled!(log::Level::Trace) {
            let caller = std::panic::Location::caller();
            log::trace!(
                "gpui animation frame requested: window={} entity={:?} active={} caller={}:{}",
                self.handle.window_id().as_u64(),
                entity.as_u64(),
                self.active.get(),
                caller.file(),
                caller.line()
            );
        }

        let pending_entities = self.animation_frame_pending_entities.clone();
        if self.active.get() {
            RefCell::borrow_mut(&self.next_frame_callbacks).push(Box::new(move |_, cx| {
                pending_entities.borrow_mut().remove(&entity);
                cx.notify(entity);
            }));

            self.record_frame_request_reason(FrameRequestReason::PresentationAnimation);
            self.request_platform_frame(RequestFrameOptions {
                require_presentation: true,
                force_render: true,
            });
        } else if !self.inactive_animation_frame_pending.replace(true) {
            RefCell::borrow_mut(&self.next_frame_callbacks).push(Box::new(move |window, cx| {
                window.inactive_animation_frame_pending.set(false);
                pending_entities.borrow_mut().remove(&entity);
                cx.notify(entity);
            }));
            self.record_frame_request_reason(FrameRequestReason::PresentationAnimation);
            self.request_platform_frame(RequestFrameOptions {
                require_presentation: true,
                force_render: false,
            });
        } else {
            self.animation_frame_pending_entities
                .borrow_mut()
                .remove(&entity);
        }
    }

    /// Schedule the next frame of a layout animation for one retained element path.
    ///
    /// Unlike [`Window::request_animation_frame`], this does not emit an application-level
    /// `Notify`. The owning view is marked dirty only so it can resample layout state, while the
    /// invalidator keeps the retained element path. Layout animation descendants are reconciled,
    /// not blindly invalidated: a stable relative/flex container must be traversed when an absolute
    /// child moves, while unrelated siblings outside the target remain replayable.
    pub(crate) fn request_layout_animation_frame(&self, retained_id: GlobalElementId) {
        let Some(entity) = self.current_view_or_root() else {
            return;
        };

        if log::log_enabled!(log::Level::Trace) {
            log::trace!(
                "gpui targeted layout animation frame requested: window={} entity={:?} retained_id={} active={}",
                self.handle.window_id().as_u64(),
                entity.as_u64(),
                retained_id,
                self.active.get()
            );
        }

        // One exact retained target owns at most one next-frame callback. Animation progress is
        // sampled from the window clock when the dirty frame actually renders, so duplicate
        // callbacks carry no useful historical state; they only create stale frame pressure.
        if !self
            .invalidator
            .arm_layout_animation_frame(entity, &retained_id)
        {
            record_coalesced_refresh();
            return;
        }

        // ReconcileSubtree is distinct from InvalidateSubtree. Descendants are visited so a fixed
        // parent cannot hide a moving child, but reusable leaves/subtrees may still prove equality.
        self.record_frame_request_reason(FrameRequestReason::LayoutAnimation);
        self.enqueue_next_frame_callback(FrameRequestReason::LayoutAnimation, move |window, _cx| {
            if !window
                .invalidator
                .take_layout_animation_frame(entity, &retained_id)
            {
                return;
            }
            if window.invalidator.invalidate_retained_path_with_scope(
                entity,
                Some(&retained_id),
                RetainedInvalidationScope::ReconcileSubtree,
            ) {
                // This callback already runs inside the platform frame requested by the
                // layout cadence. Marking the target dirty is enough for the current
                // evaluate_frame_work pass; scheduling here would queue a redundant frame.
                window.record_frame_request_reason(FrameRequestReason::LayoutAnimation);
            }
        });
    }

    /// Schedule a delayed layout-animation sample for one retained element path.
    ///
    /// This is the targeted equivalent of [`Window::request_invalidation_at`] and is used by
    /// repeating legacy/layout animations so their cadence does not turn into whole-view cache
    /// invalidation.
    pub(crate) fn request_layout_animation_frame_at(
        &self,
        retained_id: GlobalElementId,
        deadline: Instant,
        cx: &App,
    ) {
        let Some(entity) = self.current_view_or_root() else {
            return;
        };
        let Some(generation) =
            self.invalidator
                .arm_layout_animation_deadline(entity, &retained_id, deadline)
        else {
            record_coalesced_refresh();
            return;
        };

        let handle = self.handle;
        let delay = deadline.saturating_duration_since(Instant::now());
        self.spawn(cx, async move |cx| {
            cx.background_executor().timer(delay).await;
            let _ = ignore_window_not_found(handle.update(cx, |_, window, _cx| {
                if !window.invalidator.take_layout_animation_deadline(
                    entity,
                    &retained_id,
                    deadline,
                    generation,
                ) {
                    return;
                }
                if window.invalidator.invalidate_retained_path_with_scope(
                    entity,
                    Some(&retained_id),
                    RetainedInvalidationScope::ReconcileSubtree,
                ) {
                    window.record_frame_request_reason(FrameRequestReason::LayoutAnimation);
                    window.schedule_interactive_animation_frame();
                }
            }));
        })
        .detach();
    }

    /// Overrides the dirty-redraw retry interval while this window is visible but inactive.
    ///
    /// This does not enable background animation by itself. It only controls how quickly an
    /// already-dirty inactive window may publish a refreshed frame. Passing None restores the
    /// framework default. Values below one display-frame budget are clamped to 16 ms.
    pub fn set_inactive_dirty_frame_retry_interval(&mut self, interval: Option<Duration>) {
        self.inactive_dirty_frame_retry_interval =
            interval.map(|interval| interval.max(Duration::from_millis(16)));
    }

    /// Allows this window to redraw ordinary dirty UI while visible but inactive.
    ///
    /// Intended for diagnostics/monitor windows whose content must continue reflecting another
    /// window in real time. Minimized windows still use the normal background defer policy.
    pub fn set_inactive_dirty_redraw_enabled(&mut self, enabled: bool) {
        self.inactive_dirty_redraw_enabled = enabled;
    }

    /// Re-arms deferred dirty or retained animation work after the window becomes visible.
    ///
    /// Hidden windows intentionally stop presentation work. When visibility is restored, this hook
    /// clears any stale throttle delay and schedules the pending work without rebuilding the view.
    pub(super) fn presentation_visibility_changed(&mut self) {
        if !self.visibility.is_visible() {
            return;
        }

        // A hidden window may have accumulated dirty state or retained animation work without a
        // platform frame. Visibility restoration is the authoritative point to re-arm that work.
        self.frame_throttle.clear_delay();
        if self.animation_engine_frame_driver.get().is_some()
            && (self.active.get() || self.inactive_animation_engine_enabled)
        {
            self.record_frame_request_reason(FrameRequestReason::PresentationAnimation);
            self.request_platform_frame(RequestFrameOptions {
                require_presentation: true,
                force_render: false,
            });
        }
        if self.invalidator.is_dirty() && !self.refreshing {
            self.schedule_dirty_frame();
        }
    }

    /// Opts this window into retained paint/GPU animation while it is visible but inactive.
    ///
    /// This is intended for NOACTIVATE panels such as desktop lyrics or HUD windows. The default is
    /// disabled, so ordinary inactive windows retain the existing power-saving behavior. Hidden
    /// windows never advance animation-engine frames even when this option is enabled.
    pub fn set_inactive_animation_engine_enabled(&mut self, enabled: bool) {
        if self.inactive_animation_engine_enabled == enabled {
            return;
        }
        self.inactive_animation_engine_enabled = enabled;

        // An animation may already have armed the engine while the inactive policy was disabled.
        // Enabling the policy resumes that pending retained timeline without notifying or rebuilding
        // the owning view.
        if enabled
            && !self.active.get()
            && self.visibility.is_visible()
            && self.animation_engine_frame_driver.get().is_some()
        {
            self.record_frame_request_reason(FrameRequestReason::PresentationAnimation);
            self.request_platform_frame(RequestFrameOptions {
                require_presentation: true,
                force_render: false,
            });
        }
    }

    /// Schedule a frame for the window animation engine without unnecessarily invalidating an
    /// entire view.
    ///
    /// Paint and GPU drivers advance retained visual state without relayout. A layout driver first
    /// tries to capture the retained element path that is currently being built. Component-local
    /// layout animations therefore become targeted retained invalidations automatically. Only
    /// callers outside an element lifecycle fall back to [`Window::request_animation_frame`].
    #[track_caller]
    pub fn request_animation_engine_frame(&self, driver: AnimationDriver) {
        if matches!(driver, AnimationDriver::Layout) {
            if let Some(retained_id) = self.current_retained_element_id() {
                self.request_layout_animation_frame(retained_id);
            } else {
                self.request_animation_frame();
            }
            return;
        }

        let mut driver = driver;
        if let Some((_, _, delayed_driver)) =
            self.animation_engine_frame_deadline.replace(None)
        {
            self.animation_engine_frame_deadline_generation
                .set(self.animation_engine_frame_deadline_generation.get().wrapping_add(1));
            driver = merge_requested_drivers(Some(delayed_driver), driver);
        }

        if !self.animation_engine.borrow_mut().mark_frame_pending() {
            record_coalesced_refresh();
            return;
        }

        self.animation_engine_frame_driver
            .set(Some(merge_requested_drivers(
                self.animation_engine_frame_driver.get(),
                driver,
            )));
        if !self.visibility.is_visible()
            || (!self.active.get() && !self.inactive_animation_engine_enabled)
        {
            return;
        }
        self.record_frame_request_reason(FrameRequestReason::PresentationAnimation);
        self.request_platform_frame(RequestFrameOptions {
            require_presentation: true,
            force_render: false,
        });
    }

    pub(crate) fn request_animation_engine_frame_at(
        &self,
        driver: AnimationDriver,
        deadline: Instant,
    ) {
        if matches!(driver, AnimationDriver::Layout) || deadline <= Instant::now() {
            self.request_animation_engine_frame(driver);
            return;
        }

        if let Some((existing_deadline, generation, existing_driver)) =
            self.animation_engine_frame_deadline.get()
        {
            let merged_driver = merge_requested_drivers(Some(existing_driver), driver);
            if existing_deadline <= deadline {
                self.animation_engine_frame_deadline
                    .set(Some((existing_deadline, generation, merged_driver)));
                return;
            }
        }

        let generation = self
            .animation_engine_frame_deadline_generation
            .get()
            .wrapping_add(1);
        self.animation_engine_frame_deadline_generation.set(generation);
        self.animation_engine_frame_deadline
            .set(Some((deadline, generation, driver)));

        let handle = self.handle;
        let deadline_state = self.animation_engine_frame_deadline.clone();
        let mut cx = self.async_app.clone();
        let executor = cx.foreground_executor().clone();
        executor
            .spawn(async move {
                let now = Instant::now();
                if deadline > now {
                    cx.background_executor().timer(deadline - now).await;
                }
                let _ = ignore_window_not_found(handle.update(&mut cx, |_, window, _cx| {
                    let Some((armed_deadline, armed_generation, armed_driver)) =
                        deadline_state.get()
                    else {
                        return;
                    };
                    if armed_deadline != deadline || armed_generation != generation {
                        return;
                    }
                    deadline_state.set(None);
                    window.request_animation_engine_frame(armed_driver);
                }));
            })
            .detach();
    }

    /// Start an engine-owned sequence timeline and schedule its first frame.
    pub fn start_animation_sequence(&self, sequence: AnimationSequence) -> AnimationGroupId {
        let (group_id, driver) = {
            let mut engine = self.animation_engine.borrow_mut();
            let group_id = engine.start_sequence(sequence, self.animation_time());
            let driver = engine
                .group_driver(group_id)
                .unwrap_or(AnimationDriver::Auto);
            (group_id, driver)
        };
        self.request_animation_engine_frame(driver);
        group_id
    }

    /// Start an engine-owned parallel timeline and schedule its first frame.
    pub fn start_animation_parallel(&self, parallel: AnimationParallel) -> AnimationGroupId {
        let (group_id, driver) = {
            let mut engine = self.animation_engine.borrow_mut();
            let group_id = engine.start_parallel(parallel, self.animation_time());
            let driver = engine
                .group_driver(group_id)
                .unwrap_or(AnimationDriver::Auto);
            (group_id, driver)
        };
        self.request_animation_engine_frame(driver);
        group_id
    }
    /// Start an engine-owned stagger timeline and schedule its first frame.
    pub fn start_animation_stagger(&self, stagger: AnimationStagger) -> AnimationGroupId {
        let (group_id, driver) = {
            let mut engine = self.animation_engine.borrow_mut();
            let group_id = engine.start_stagger(stagger, self.animation_time());
            let driver = engine
                .group_driver(group_id)
                .unwrap_or(AnimationDriver::Auto);
            (group_id, driver)
        };
        self.request_animation_engine_frame(driver);
        group_id
    }

    /// Sample an engine-owned animation group at the current window animation time.
    pub fn sample_animation_group(
        &self,
        group_id: AnimationGroupId,
    ) -> Option<AnimationGroupSample> {
        self.animation_engine
            .borrow()
            .sample_group(group_id, self.animation_time())
    }

    /// Cancel an engine-owned animation group.
    pub fn cancel_animation_group(&self, group_id: AnimationGroupId) -> bool {
        self.animation_engine.borrow_mut().cancel_group(group_id)
    }

    /// Associate dirty visual bounds with an engine-owned animation group.
    pub fn set_animation_group_bounds(
        &self,
        group_id: AnimationGroupId,
        bounds: Bounds<Pixels>,
    ) -> bool {
        self.animation_engine
            .borrow_mut()
            .set_group_bounds(group_id, bounds)
    }

    /// Start a renderer-owned scene animation. Translation endpoints are already resolved to
    /// device pixels by `AnimationProperty::resolved_values`; do not scale them a second time here.
    pub(crate) fn start_scene_animation(
        &self,
        element_id: &GlobalElementId,
        property: TransitionProperty,
        spec: AnimationSpec,
        bounds: Bounds<Pixels>,
        from: [f32; 4],
        to: [f32; 4],
    ) -> SceneAnimationId {
        let animation_id = SceneAnimationId(self.next_scene_animation_id.get());
        self.next_scene_animation_id
            .set(self.next_scene_animation_id.get().wrapping_add(1));
        let mut engine = self.animation_engine.borrow_mut();
        engine.start_transition(element_id, property, spec, self.animation_time());
        engine.set_transition_bounds(element_id, property, bounds);
        engine.bind_scene_animation(element_id, property, animation_id, from, to);
        let driver = engine
            .transition_driver(element_id, property)
            .unwrap_or(crate::AnimationDriver::Paint);
        drop(engine);
        self.request_animation_engine_frame(driver);
        animation_id
    }

    pub(crate) fn retarget_scene_animation(
        &self,
        element_id: &GlobalElementId,
        property: TransitionProperty,
        animation_id: SceneAnimationId,
        spec: AnimationSpec,
        spring: Option<crate::Spring>,
        old_bounds: Bounds<Pixels>,
        new_bounds: Bounds<Pixels>,
        dirty_bounds: Bounds<Pixels>,
        to: [f32; 4],
    ) -> bool {
        let scale_factor = self.scale_factor();
        let base_translation_delta = [
            (old_bounds.origin.x.0 - new_bounds.origin.x.0) * scale_factor,
            (old_bounds.origin.y.0 - new_bounds.origin.y.0) * scale_factor,
        ];
        let mut engine = self.animation_engine.borrow_mut();
        let retargeted = engine.retarget_scene_animation(
            element_id,
            property,
            animation_id,
            spec,
            spring,
            self.animation_time(),
            dirty_bounds,
            base_translation_delta,
            to,
        );
        let driver = retargeted
            .then(|| engine.transition_driver(element_id, property))
            .flatten();
        drop(engine);

        if let Some(driver) = driver {
            self.request_animation_engine_frame(driver);
        }
        retargeted
    }

    pub(crate) fn set_scene_animation_spring(
        &self,
        element_id: &GlobalElementId,
        property: TransitionProperty,
        spring: crate::Spring,
    ) {
        self.animation_engine
            .borrow_mut()
            .set_transition_spring(element_id, property, spring);
    }

    pub(crate) fn scene_animation_is_active(&self, animation_id: SceneAnimationId) -> bool {
        self.animation_engine
            .borrow()
            .scene_animation_is_active(animation_id)
    }

    /// Notify the current view at or after the given deadline without requesting
    /// continuous animation frames or an immediate presentation.
    ///
    /// Use this for UI that is otherwise static but has time-based visibility,
    /// such as toast expiry or low-FPS status indicators. Unlike
    /// [`Window::request_animation_frame`], this schedules a single dirty-frame
    /// invalidation and coalesces later requests for the same view. If there is
    /// no currently rendering view, the root view is used.
    pub fn request_invalidation_at(&self, deadline: Instant, cx: &App) {
        let Some(entity) = self.current_view_or_root() else {
            return;
        };
        self.request_invalidation_for(entity, deadline, cx);
    }

    /// Notify a specific view at or after the given deadline.
    pub fn request_invalidation_for(&self, entity: EntityId, deadline: Instant, cx: &App) {
        let existing = self
            .deadline_invalidation_pending
            .borrow()
            .get(&entity)
            .copied();
        if existing.is_some_and(|(pending_deadline, _)| pending_deadline <= deadline) {
            return;
        }

        let generation = self.deadline_invalidation_generation.get().wrapping_add(1);
        self.deadline_invalidation_generation.set(generation);
        self.deadline_invalidation_pending
            .borrow_mut()
            .insert(entity, (deadline, generation));

        let pending = self.deadline_invalidation_pending.clone();
        let handle = self.handle;
        let delay = deadline.saturating_duration_since(Instant::now());
        self.spawn(cx, async move |cx| {
            cx.background_executor().timer(delay).await;
            if pending.borrow().get(&entity).copied() != Some((deadline, generation)) {
                return;
            }

            pending.borrow_mut().remove(&entity);
            let _ = ignore_window_not_found(handle.update(cx, |_, window, cx| {
                window.record_frame_request_reason(FrameRequestReason::Timer);
                cx.notify(entity);
            }));
        })
        .detach();
    }

    pub(crate) fn request_image_animation_frame_at(
        &self,
        deadline: Instant,
        cx: &App,
        animation_config: crate::AnimatedImageConfig,
    ) {
        let Some(entity) = self.current_view_or_root() else {
            return;
        };
        // Animated media must not keep a hidden window alive. A visibility/activation redraw will
        // paint the image again and re-arm playback from the current media frame.
        if !self.visibility.is_visible() {
            return;
        }

        let minimum_frame_duration = if self.active.get() {
            animation_config.minimum_frame_duration()
        } else {
            animation_config.inactive_minimum_frame_duration()
        };
        let now = Instant::now();
        let rate_limited_deadline = self
            .last_inactive_animation_frame
            .get()
            .and_then(|last_frame| last_frame.checked_add(minimum_frame_duration))
            .unwrap_or(now);
        // Respect the media frame's real presentation deadline. The old path only enforced the FPS
        // ceiling and could redraw the same GIF/APNG frame repeatedly before that deadline.
        let presentation_deadline = deadline.max(rate_limited_deadline);

        // If the media frame becomes ready before the next expected presentation, do not create
        // a high-frequency timer (for example a 1ms timer from a malformed/very-fast GIF). Keep
        // exactly one platform-frame request pending and let VSync determine 60/120/144/240Hz.
        // Slow media still sleeps until its real deadline below, so this does not redraw the same
        // frame continuously.
        let follow_platform_cadence = self.active.get()
            && presentation_deadline
                <= now + self.frame_throttle.presentation_interval_hint();
        if follow_platform_cadence || presentation_deadline <= now {
            // A previously armed slower deadline is now obsolete. Its task observes the missing
            // map entry and exits without notifying the view.
            self.image_animation_deadline_pending
                .borrow_mut()
                .remove(&entity);
            self.last_inactive_animation_frame.set(Some(now));
            self.record_frame_request_reason(FrameRequestReason::ImageReady);
            self.request_animation_frame();
            return;
        }

        let existing = self
            .image_animation_deadline_pending
            .borrow()
            .get(&entity)
            .copied();
        if existing.is_some_and(|(pending_deadline, _)| pending_deadline <= presentation_deadline) {
            return;
        }

        let generation = self
            .image_animation_deadline_generation
            .get()
            .wrapping_add(1);
        self.image_animation_deadline_generation.set(generation);
        self.image_animation_deadline_pending
            .borrow_mut()
            .insert(entity, (presentation_deadline, generation));

        let pending = self.image_animation_deadline_pending.clone();
        let last_frame = self.last_inactive_animation_frame.clone();
        let handle = self.handle;
        let delay = presentation_deadline.saturating_duration_since(now);
        self.spawn(cx, async move |cx| {
            cx.background_executor().timer(delay).await;
            if pending.borrow().get(&entity).copied() != Some((presentation_deadline, generation)) {
                return;
            }
            pending.borrow_mut().remove(&entity);
            let _ = ignore_window_not_found(handle.update(cx, |_, window, cx| {
                if !window.visibility.is_visible() {
                    return;
                }
                last_frame.set(Some(Instant::now()));
                window.record_frame_request_reason(FrameRequestReason::ImageReady);
                cx.notify(entity);
            }));
        })
        .detach();
    }

    /// Spawn the future returned by the given closure on the application thread pool.
    /// The closure is provided a handle to the current window and an `AsyncWindowContext` for
    /// use within your future.
    #[track_caller]
    pub fn spawn<AsyncFn, R>(&self, cx: &App, f: AsyncFn) -> Task<R>
    where
        R: 'static,
        AsyncFn: AsyncFnOnce(&mut AsyncWindowContext) -> R + 'static,
    {
        let handle = self.handle;
        cx.spawn(async move |app| {
            let mut async_window_cx = AsyncWindowContext::new_context(app.clone(), handle);
            f(&mut async_window_cx).await
        })
    }
    /// Spawn the future returned by the given closure on the application thread
    /// pool, with the given priority.
    #[track_caller]
    pub fn spawn_with_priority<AsyncFn, R>(
        &self,
        _priority: impl Send + 'static,
        cx: &App,
        f: AsyncFn,
    ) -> Task<R>
    where
        R: 'static,
        AsyncFn: AsyncFnOnce(&mut AsyncWindowContext) -> R + 'static,
    {
        let handle = self.handle;
        cx.spawn(async move |app| {
            let mut async_window_cx = AsyncWindowContext::new_context(app.clone(), handle);
            f(&mut async_window_cx).await
        })
    }

    pub(super) fn current_view_or_root(&self) -> Option<EntityId> {
        self.rendered_entity_stack
            .last()
            .copied()
            .or_else(|| self.root.as_ref().map(AnyView::entity_id))
    }
}
