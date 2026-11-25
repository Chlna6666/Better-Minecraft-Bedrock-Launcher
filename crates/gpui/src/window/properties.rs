use super::state::ResolvedTextStyle;
use super::*;

pub(super) const INPUT_POINTER: u8 = 1;
pub(super) const INPUT_MODIFIERS: u8 = 2;
pub(super) const INPUT_CAPSLOCK: u8 = 4;
pub(super) const INPUT_KEYBOARD_MODALITY: u8 = 8;

#[derive(Clone, Copy)]
pub(super) struct InputReadSnapshot {
    pointer: Point<Pixels>,
    modifiers: Modifiers,
    capslock: Capslock,
    keyboard: bool,
}

impl InputReadSnapshot {
    fn changed(self, next: Self) -> u8 {
        u8::from(self.pointer != next.pointer) * INPUT_POINTER
            | u8::from(self.modifiers != next.modifiers) * INPUT_MODIFIERS
            | u8::from(self.capslock != next.capslock) * INPUT_CAPSLOCK
            | u8::from(self.keyboard != next.keyboard) * INPUT_KEYBOARD_MODALITY
    }
}

fn matching_input_readers(
    readers: &FxHashMap<EntityId, u8>,
    changed: u8,
) -> impl Iterator<Item = EntityId> + '_ {
    readers.iter().filter_map(move |(&owner, &mask)| (mask & changed != 0).then_some(owner))
}

impl Window {
    #[inline]
    fn record_input_read(&self, mask: u8) {
        if let Some(owner) = self.input_dependency_owner {
            *self.next_input_readers.borrow_mut().entry(owner).or_default() |= mask;
        }
    }

    pub(super) fn input_read_snapshot(&self) -> InputReadSnapshot {
        InputReadSnapshot {
            pointer: self.mouse_position,
            modifiers: self.modifiers,
            capslock: self.capslock,
            keyboard: self.last_input_modality == super::state::InputModality::Keyboard,
        }
    }

    pub(super) fn invalidate_input_reads(&mut self, before: InputReadSnapshot) {
        let changed = before.changed(self.input_read_snapshot());
        if changed == 0 || self.input_readers.is_empty() {
            return;
        }
        if self.invalidator.invalidate_dependent_views(matching_input_readers(&self.input_readers, changed)) {
            self.schedule_dirty_frame();
        }
    }

    pub(super) fn commit_input_reads(&mut self) {
        let next = self.next_input_readers.get_mut();
        if self.replayed_dependency_ranges {
            // Some deferred lifecycles can be replayed without executing
            // their Render. Never drop a still-mounted reader in that case.
            for (&owner, &mask) in &self.input_readers {
                if self.rendered_frame.dispatch_tree.view_path_reversed(owner).next().is_some() {
                    *next.entry(owner).or_default() |= mask;
                }
            }
        }
        // A reader without a committed dispatch route must not be silently
        // dropped: mark_view_dirty will conservatively refresh that window.
        mem::swap(&mut self.input_readers, next);
        next.clear();
    }

    /// Accessor for this window's foreground-thread text layout system.
    pub fn text_system(&self) -> &Rc<WindowTextSystem> {
        &self.text_system
    }

    /// The current text style. Which is composed of all the style refinements provided to `with_text_style`.
    ///
    /// Every painted element asks for this at least twice a frame, and building it clones the
    /// window's default style and replays the whole refinement stack. Both the depth and the
    /// default style are tracked, so an unchanged stack returns the cached value.
    pub fn text_style(&self) -> TextStyle {
        let depth = self.text_style_stack.len();
        let generation = self.text_style_generation.get();
        if let Some(cached) = self.resolved_text_style.borrow().as_ref()
            && cached.depth == depth
            && cached.generation == generation
        {
            return cached.style.clone();
        }

        let mut style = self.default_text_style.clone();
        for refinement in &self.text_style_stack {
            style.refine(refinement);
        }
        *self.resolved_text_style.borrow_mut() = Some(ResolvedTextStyle {
            depth,
            generation,
            style: style.clone(),
        });
        style
    }

    /// Borrows the current text style when one is cached, otherwise resolves it.
    ///
    /// Hot retained paths only need to compare or copy this value; they must not force a
    /// clone just to look at it.
    pub(crate) fn with_text_style_ref<R>(&self, f: impl FnOnce(&TextStyle) -> R) -> R {
        let depth = self.text_style_stack.len();
        let generation = self.text_style_generation.get();
        {
            let cached = self.resolved_text_style.borrow();
            if let Some(cached) = cached.as_ref()
                && cached.depth == depth
                && cached.generation == generation
            {
                return f(&cached.style);
            }
        }
        let style = self.text_style();
        f(&style)
    }

    /// Returns the appearance of the current window.
    pub fn appearance(&self) -> WindowAppearance {
        self.appearance
    }

    /// Record geometry reads performed by a rendered View. Bounds and viewport reads must
    /// participate in the same resize invalidation domain; otherwise a retained boundary that reads
    /// `Window::bounds()` can keep its previous viewport-sized contents after a native resize.
    pub(super) fn record_viewport_dependency(&self) {
        if let Some(view_id) = self.rendered_entity_stack.last().copied() {
            self.viewport_dependent_views.borrow_mut().insert(view_id);
        }
    }

    /// Returns the size of the drawable area within the window.
    pub fn viewport_size(&self) -> Size<Pixels> {
        self.record_viewport_dependency();
        self.viewport_size
    }

    /// Returns whether this window is focused by the operating system (receiving key events).
    pub fn is_window_active(&self) -> bool {
        self.active.get()
    }

    /// Returns whether the platform is currently presenting this window's frames.
    pub fn visibility(&self) -> WindowVisibility {
        self.visibility
    }

    /// Returns true when the platform reports that this window can be presented.
    pub fn is_window_visible(&self) -> bool {
        self.visibility.is_visible()
    }

    /// Returns whether UI content has been built and the platform's initial handoff is complete.
    ///
    /// On Windows this waits for a submitted GPU frame, even when an initial background is
    /// already visible. This does not establish physical scanout or current visibility.
    pub fn is_content_ready(&self) -> bool {
        self.has_completed_rendered_frame && self.platform_window.is_content_ready()
    }

    /// Returns whether this window is considered to be the window
    /// that currently owns the mouse cursor.
    /// On mac, this is equivalent to `is_window_active`.
    pub fn is_window_hovered(&self) -> bool {
        if cfg!(any(
            target_os = "windows",
            target_os = "linux",
            target_os = "freebsd"
        )) {
            self.hovered.get()
        } else {
            self.is_window_active()
        }
    }

    /// The scale factor of the display associated with the window. For example, it could
    /// return 2.0 for a "retina" display, indicating that each logical pixel should actually
    /// be rendered as two pixels on screen.
    pub fn scale_factor(&self) -> f32 {
        self.scale_factor
    }

    /// The size of an em for the base font of the application. Adjusting this value allows the
    /// UI to scale, just like zooming a web page.
    pub fn rem_size(&self) -> Pixels {
        self.rem_size_override_stack
            .last()
            .copied()
            .unwrap_or(self.rem_size)
    }

    /// Returns the immutable timestamp captured at the start of the current platform frame.
    ///
    /// Render-time animation sampling should use this value instead of calling `Instant::now()`
    /// independently. Layout, paint and renderer-owned scene animations then observe the same
    /// timeline sample for the entire frame, even when CPU work makes the frame expensive.
    pub fn animation_time(&self) -> Instant {
        self.animation_time.get()
    }

    /// Returns seconds on GPUI's renderer presentation clock at this frame's stable timestamp.
    ///
    /// Retained custom GPU effects can store an offset against this clock and continue advancing on
    /// presentation-only frames without invalidating their owning View.
    pub fn presentation_time_seconds(&self) -> f32 {
        crate::animation::presentation_clock_seconds_at(self.animation_time())
    }

    /// Sets the size of an em for the base font of the application. Adjusting this value allows the
    /// UI to scale, just like zooming a web page.
    pub fn set_rem_size(&mut self, rem_size: impl Into<Pixels>) {
        self.rem_size = rem_size.into();
    }

    pub(crate) fn set_default_text_style(&mut self, text_style: TextStyle) {
        self.default_text_style = text_style;
        self.text_style_generation
            .set(self.text_style_generation.get().wrapping_add(1));
        self.text_system.clear_layout_cache();
        self.refresh();
    }

    /// The line height associated with the current text style.
    pub fn line_height(&self) -> Pixels {
        self.text_style().line_height_in_pixels(self.rem_size())
    }

    /// Call to prevent the default action of an event. Currently only used to prevent
    /// parent elements from becoming focused on mouse down.
    pub fn prevent_default(&mut self) {
        self.default_prevented = true;
    }

    /// Obtain whether default has been prevented for the event currently being dispatched.
    pub fn default_prevented(&self) -> bool {
        self.default_prevented
    }

    /// The position of the mouse relative to the window.
    pub fn mouse_position(&self) -> Point<Pixels> {
        self.record_input_read(INPUT_POINTER);
        self.mouse_position
    }
    /// Returns true when keyboard input is the current input modality.
    pub fn last_input_was_keyboard(&self) -> bool {
        self.record_input_read(INPUT_KEYBOARD_MODALITY);
        self.last_input_modality == super::state::InputModality::Keyboard
    }

    /// The current state of the keyboard's modifiers
    pub fn modifiers(&self) -> Modifiers {
        self.record_input_read(INPUT_MODIFIERS);
        self.modifiers
    }

    /// The current state of the keyboard's capslock
    pub fn capslock(&self) -> Capslock {
        self.record_input_read(INPUT_CAPSLOCK);
        self.capslock
    }

    /// For testing: set the current modifier keys state.
    /// This does not generate any events.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_modifiers(&mut self, modifiers: Modifiers) {
        self.modifiers = modifiers;
    }
}

#[cfg(test)]
mod input_dependency_tests {
    use super::*;

    #[test]
    fn input_masks_distinguish_pointer_modifiers_capslock_and_keyboard() {
        let baseline = InputReadSnapshot {
            pointer: point(px(0.), px(0.)),
            modifiers: Modifiers::default(),
            capslock: Capslock::default(),
            keyboard: false,
        };
        assert_eq!(baseline.changed(baseline), 0);
        let mut moved = baseline;
        moved.pointer = point(px(10.), px(20.));
        assert_eq!(baseline.changed(moved), INPUT_POINTER);
        moved.modifiers.shift = true;
        assert_eq!(baseline.changed(moved), INPUT_POINTER | INPUT_MODIFIERS);
        moved.capslock.on = true;
        moved.keyboard = true;
        assert_eq!(baseline.changed(moved), INPUT_POINTER | INPUT_MODIFIERS | INPUT_CAPSLOCK | INPUT_KEYBOARD_MODALITY);
    }

    #[test]
    fn only_readers_of_changed_input_are_selected() {
        let pointer = EntityId::from(1);
        let keyboard = EntityId::from(2);
        let both = EntityId::from(3);
        let readers = FxHashMap::from_iter([
            (pointer, INPUT_POINTER),
            (keyboard, INPUT_KEYBOARD_MODALITY),
            (both, INPUT_POINTER | INPUT_MODIFIERS),
        ]);
        assert_eq!(matching_input_readers(&readers, INPUT_POINTER).collect::<FxHashSet<_>>(),
            FxHashSet::from_iter([pointer, both]));
        assert_eq!(matching_input_readers(&readers, INPUT_MODIFIERS).collect::<Vec<_>>(), vec![both]);
        assert_eq!(matching_input_readers(&readers, INPUT_CAPSLOCK).count(), 0);
    }
}
