use super::*;

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

    /// Schedule the given closure to be run directly after the current frame is rendered.
    pub fn on_next_frame(&self, callback: impl FnOnce(&mut Window, &mut App) + 'static) {
        let should_request_frame = {
            let mut next_frame_callbacks = self.next_frame_callbacks.borrow_mut();
            let should_request_frame = next_frame_callbacks.is_empty();
            next_frame_callbacks.push(Box::new(callback));
            should_request_frame
        };
        if should_request_frame {
            self.platform_window.request_frame(RequestFrameOptions {
                require_presentation: true,
                force_render: false,
            });
        }
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

            self.platform_window.request_frame(RequestFrameOptions {
                require_presentation: true,
                force_render: true,
            });
        } else if !self.inactive_animation_frame_pending.replace(true) {
            RefCell::borrow_mut(&self.next_frame_callbacks).push(Box::new(move |window, cx| {
                window.inactive_animation_frame_pending.set(false);
                pending_entities.borrow_mut().remove(&entity);
                cx.notify(entity);
            }));
            self.platform_window.request_frame(RequestFrameOptions {
                require_presentation: true,
                force_render: false,
            });
        } else {
            self.animation_frame_pending_entities
                .borrow_mut()
                .remove(&entity);
        }
    }

    /// Schedule a frame for the window animation engine without notifying a view.
    ///
    /// Paint and GPU drivers can advance retained visual state without forcing a
    /// full layout pass. Layout driver requests intentionally fall back to
    /// [`Window::request_animation_frame`] because layout-affecting animation
    /// closures must rerender their owning view.
    #[track_caller]
    pub fn request_animation_engine_frame(&self, driver: AnimationDriver) {
        if matches!(driver, AnimationDriver::Layout) {
            self.request_animation_frame();
            return;
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
        self.platform_window.request_frame(RequestFrameOptions {
            require_presentation: true,
            force_render: false,
        });
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
            let _ = ignore_window_not_found(handle.update(cx, |_, _window, cx| {
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
        let existing = self
            .image_animation_deadline_pending
            .borrow()
            .get(&entity)
            .copied();
        if existing.is_some_and(|(pending_deadline, _)| pending_deadline <= deadline) {
            return;
        }

        let minimum_frame_duration = if self.active.get() {
            animation_config.minimum_frame_duration()
        } else {
            animation_config.inactive_minimum_frame_duration()
        };
        let now = deadline.min(Instant::now());
        let remaining = match self.last_inactive_animation_frame.get() {
            Some(last_frame) => minimum_frame_duration
                .checked_sub(now.saturating_duration_since(last_frame))
                .unwrap_or_default(),
            None => Duration::ZERO,
        };

        if remaining.is_zero() {
            self.last_inactive_animation_frame.set(Some(now));
            self.request_animation_frame();
            return;
        }

        let generation = self
            .image_animation_deadline_generation
            .get()
            .wrapping_add(1);
        self.image_animation_deadline_generation.set(generation);
        self.image_animation_deadline_pending
            .borrow_mut()
            .insert(entity, (deadline, generation));

        let pending = self.image_animation_deadline_pending.clone();
        let last_frame = self.last_inactive_animation_frame.clone();
        let handle = self.handle;
        self.spawn(cx, async move |cx| {
            cx.background_executor().timer(remaining).await;
            if pending.borrow().get(&entity).copied() != Some((deadline, generation)) {
                return;
            }
            pending.borrow_mut().remove(&entity);
            last_frame.set(Some(Instant::now()));
            let _ = ignore_window_not_found(handle.update(cx, |_, _window, cx| {
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
