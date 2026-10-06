use super::*;

impl Window {
    pub(crate) fn schedule_image_frame(
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
            && presentation_deadline <= now + self.frame_throttle.presentation_interval_hint();
        if follow_platform_cadence || presentation_deadline <= now {
            self.request_image_frame(entity, now);
            return;
        }

        self.arm_image_frame_deadline(entity, presentation_deadline, now, cx);
    }

    fn request_image_frame(&self, entity: EntityId, now: Instant) {
        // A previously armed slower deadline is now obsolete. Its task observes the missing
        // map entry and exits without notifying the view.
        self.image_animation_deadline_pending
            .borrow_mut()
            .remove(&entity);
        self.last_inactive_animation_frame.set(Some(now));
        self.record_frame_request_reason(FrameRequestReason::ImageReady);
        self.request_animation_frame();
    }

    fn arm_image_frame_deadline(
        &self,
        entity: EntityId,
        presentation_deadline: Instant,
        now: Instant,
        cx: &App,
    ) {
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
}
