use super::state::{FrameRequestReason, InputModality};
use super::*;
use crate::{ExternalPaths, TouchEvent, TouchPhase};
use crate::gestures::RecognizedTouchGesture;

mod window_control;

impl Window {
    /// Sets an input handler, such as [`ElementInputHandler`][element_input_handler], which interfaces with the
    /// platform to receive textual input with proper integration with concerns such
    /// as IME interactions. This handler will be active for the upcoming frame until the following frame is
    /// rendered.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    ///
    /// [element_input_handler]: crate::ElementInputHandler
    pub fn set_input_handler(
        &mut self,
        focus_handle: &FocusHandle,
        input_handler: impl InputHandler,
        cx: &App,
    ) {
        self.invalidator.debug_assert_paint();

        if focus_handle.is_focused(self) {
            let cx = self.to_async(cx);
            self.next_frame
                .input_handlers
                .push(Some(PlatformInputHandler::new(cx, Box::new(input_handler))));
        }
    }

    /// Register a mouse event listener on the window for the next frame. The type of event
    /// is determined by the first parameter of the given listener. When the next frame is rendered
    /// the listener will be cleared.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn on_mouse_event<Event: MouseEvent>(
        &mut self,
        mut handler: impl FnMut(&Event, DispatchPhase, &mut Window, &mut App) + 'static,
    ) {
        self.invalidator.debug_assert_paint();

        self.next_frame
            .mouse_listeners
            .push(MouseListener::new::<Event>(Box::new(
                move |event: &dyn Any, phase: DispatchPhase, window: &mut Window, cx: &mut App| {
                    if let Some(event) = event.downcast_ref() {
                        handler(event, phase, window, cx)
                    }
                },
            )));
    }

    /// Register a framework-owned mouse listener whose state can only change when hit testing
    /// changes. Unlike [`Self::on_mouse_event`], an unchanged `MouseMoveEvent` does not invoke this
    /// callback. Explicit application mouse-move handlers keep continuous delivery semantics.
    pub(crate) fn on_mouse_hit_test_transition<Event: MouseEvent>(
        &mut self,
        mut handler: impl FnMut(&Event, DispatchPhase, &mut Window, &mut App) + 'static,
    ) {
        self.invalidator.debug_assert_paint();

        self.next_frame
            .mouse_listeners
            .push(MouseListener::new_hit_test_transition::<Event>(Box::new(
                move |event: &dyn Any, phase: DispatchPhase, window: &mut Window, cx: &mut App| {
                    if let Some(event) = event.downcast_ref() {
                        handler(event, phase, window, cx)
                    }
                },
            )));
    }

    /// Register a key event listener on the window for the next frame. The type of event
    /// is determined by the first parameter of the given listener. When the next frame is rendered
    /// the listener will be cleared.
    ///
    /// This is a fairly low-level method, so prefer using event handlers on elements unless you have
    /// a specific need to register a global listener.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn on_key_event<Event: KeyEvent>(
        &mut self,
        listener: impl Fn(&Event, DispatchPhase, &mut Window, &mut App) + 'static,
    ) {
        self.invalidator.debug_assert_paint();

        self.next_frame.dispatch_tree.on_key_event(Rc::new(
            move |event: &dyn Any, phase, window: &mut Window, cx: &mut App| {
                if let Some(event) = event.downcast_ref::<Event>() {
                    listener(event, phase, window, cx)
                }
            },
        ));
    }

    /// Register a modifiers changed event listener on the window for the next frame.
    ///
    /// This is a fairly low-level method, so prefer using event handlers on elements unless you have
    /// a specific need to register a global listener.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn on_modifiers_changed(
        &mut self,
        listener: impl Fn(&ModifiersChangedEvent, &mut Window, &mut App) + 'static,
    ) {
        self.invalidator.debug_assert_paint();

        self.next_frame.dispatch_tree.on_modifiers_changed(Rc::new(
            move |event: &ModifiersChangedEvent, window: &mut Window, cx: &mut App| {
                listener(event, window, cx)
            },
        ));
    }

    /// Register a listener to be called when the given focus handle or one of its descendants receives focus.
    /// This does not fire if the given focus handle - or one of its descendants was previously focused.
    /// Returns a subscription and persists until the subscription is dropped.
    pub fn on_focus_in(
        &mut self,
        handle: &FocusHandle,
        cx: &mut App,
        mut listener: impl FnMut(&mut Window, &mut App) + 'static,
    ) -> Subscription {
        let focus_id = handle.id;
        let (subscription, activate) =
            self.new_focus_listener(Box::new(move |event, window, cx| {
                if event.is_focus_in(focus_id) {
                    listener(window, cx);
                }
                true
            }));
        cx.defer(move |_| activate());
        subscription
    }

    /// Register a listener to be called when the given focus handle or one of its descendants loses focus.
    /// Returns a subscription and persists until the subscription is dropped.
    pub fn on_focus_out(
        &mut self,
        handle: &FocusHandle,
        cx: &mut App,
        mut listener: impl FnMut(FocusOutEvent, &mut Window, &mut App) + 'static,
    ) -> Subscription {
        let focus_id = handle.id;
        let (subscription, activate) =
            self.new_focus_listener(Box::new(move |event, window, cx| {
                if let Some(blurred_id) = event.previous_focus_path.last().copied()
                    && event.is_focus_out(focus_id)
                {
                    let event = FocusOutEvent {
                        blurred: WeakFocusHandle {
                            id: blurred_id,
                            handles: Arc::downgrade(&cx.focus_handles),
                        },
                    };
                    listener(event, window, cx)
                }
                true
            }));
        cx.defer(move |_| activate());
        subscription
    }

    pub(super) fn reset_cursor_style(&self, cx: &mut App) {
        // Set the cursor only if we're the active window.
        if self.is_window_hovered() {
            let style = if matches!(self.window_decorations(), Decorations::Client { .. }) {
                self.client_inset
                    .and_then(|inset| resize_edge_hit_test(self, self.mouse_position(), inset))
                    .map(resize_edge_cursor_style)
                    .or_else(|| self.rendered_frame.cursor_style(self))
                    .unwrap_or(CursorStyle::Arrow)
            } else {
                self.rendered_frame
                    .cursor_style(self)
                    .unwrap_or(CursorStyle::Arrow)
            };
            cx.platform.set_cursor_style(style);
        }
    }

    /// Dispatch a given keystroke as though the user had typed it.
    /// You can create a keystroke with Keystroke::parse("").
    pub fn dispatch_keystroke(&mut self, keystroke: Keystroke, cx: &mut App) -> bool {
        let keystroke = keystroke.with_simulated_ime();
        let result = self.dispatch_event(
            PlatformInput::KeyDown(KeyDownEvent {
                keystroke: keystroke.clone(),
                is_held: false,
            }),
            cx,
        );
        if !result.propagate {
            return true;
        }

        if let Some(input) = keystroke.key_char
            && let Some(mut input_handler) = self.platform_window.take_input_handler()
        {
            input_handler.dispatch_input(&input, self, cx);
            self.platform_window.set_input_handler(input_handler);
            return true;
        }

        false
    }

    /// Return a key binding string for an action, to display in the UI. Uses the highest precedence
    /// binding for the action (last binding added to the keymap).
    pub fn keystroke_text_for(&self, action: &dyn Action) -> String {
        self.binding_for_action(action)
            .map(|binding| {
                binding
                    .keystrokes()
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_else(|| action.name().to_string())
    }

    /// Dispatch a mouse, keyboard, or touch event on the window.
    #[profiling::function]
    pub fn dispatch_event(&mut self, event: PlatformInput, cx: &mut App) -> DispatchEventResult {
        let event_started_at = Instant::now();
        let event_name = platform_input_name(&event);
        #[cfg(feature = "profiler")]
        let _profile = crate::diagnostics::foreground_profiler::ForegroundWorkSpan::input(
            event_name,
            self.handle.window_id().as_u64(),
        );
        if event.unconditionally_extends_recent_input_present() {
            self.last_input_timestamp.set(Instant::now());
            self.record_frame_request_reason(FrameRequestReason::Input);
        }

        // Keyboard navigation suppresses pointer hover until pointer/touch input resumes.
        // Only actual modality transitions invalidate the frame, so repeated events within one
        // modality do not repeatedly force view-cache refreshes.
        let previous_input_modality = self.last_input_modality;
        self.last_input_modality = match &event {
            PlatformInput::KeyDown(_) => InputModality::Keyboard,
            PlatformInput::MouseMove(_) | PlatformInput::MouseDown(_) => InputModality::Mouse,
            PlatformInput::Touch(_) | PlatformInput::LongPress(_) | PlatformInput::TouchDrag(_) => {
                InputModality::Touch
            }
            _ => self.last_input_modality,
        };
        let input_modality_changed = self.last_input_modality != previous_input_modality;

        // Handlers may set this to false by calling `stop_propagation`.
        cx.propagate_event = true;
        // Handlers may set this to true by calling `prevent_default`.
        self.default_prevented = false;

        if input_modality_changed && self.has_completed_rendered_frame {
            if matches!(&event, PlatformInput::KeyDown(_) | PlatformInput::MouseMove(_)) {
                // KeyDown only suppresses pointer hover, while a real MouseMove legitimately
                // restores it. Re-run framework-owned hover-transition listeners without forcing
                // every cached view through MissRefresh.
                self.dispatch_input_modality_hover_transition(cx);
                // A hover callback may stop propagation or prevent the synthetic transition.
                // Those flags must not leak into the real keyboard/pointer event.
                cx.propagate_event = true;
                self.default_prevented = false;
            } else {
                // MouseDown/touch changes modality without representing mouse motion. Keep the
                // conservative redraw used upstream so tooltip timing and touch semantics are not
                // accidentally converted into synthetic MouseMove behavior.
                self.refresh();
            }
        }

        let event = match event {
            // Track the mouse position with our own state, since accessing the platform
            // API for the mouse position can only occur on the main thread.
            PlatformInput::MouseMove(mouse_move) => {
                self.mouse_position = mouse_move.position;
                self.modifiers = mouse_move.modifiers;
                PlatformInput::MouseMove(mouse_move)
            }
            PlatformInput::MouseDown(mouse_down) => {
                self.mouse_position = mouse_down.position;
                self.modifiers = mouse_down.modifiers;
                PlatformInput::MouseDown(mouse_down)
            }
            PlatformInput::MouseUp(mouse_up) => {
                self.mouse_position = mouse_up.position;
                self.modifiers = mouse_up.modifiers;
                PlatformInput::MouseUp(mouse_up)
            }
            PlatformInput::MouseExited(mouse_exited) => {
                // Preserve the platform leave position for tooltip/drag bookkeeping, but the
                // dispatch path below deliberately clears the hit-test instead of treating this
                // coordinate as an in-window hover point.
                self.mouse_position = mouse_exited.position;
                self.modifiers = mouse_exited.modifiers;
                PlatformInput::MouseExited(mouse_exited)
            }
            PlatformInput::ModifiersChanged(modifiers_changed) => {
                self.modifiers = modifiers_changed.modifiers;
                self.capslock = modifiers_changed.capslock;
                PlatformInput::ModifiersChanged(modifiers_changed)
            }
            PlatformInput::ScrollWheel(scroll_wheel) => {
                self.mouse_position = scroll_wheel.position;
                self.modifiers = scroll_wheel.modifiers;
                PlatformInput::ScrollWheel(scroll_wheel)
            }
            // Translate dragging and dropping of external files from the operating system
            // to internal drag and drop events.
            PlatformInput::Touch(touch) => PlatformInput::Touch(touch),
            PlatformInput::LongPress(long_press) => {
                self.mouse_position = long_press.start_position;
                PlatformInput::LongPress(long_press)
            }
            PlatformInput::TouchDrag(touch_drag) => {
                self.mouse_position = touch_drag.start_position;
                PlatformInput::TouchDrag(touch_drag)
            }
            PlatformInput::FileDrop(file_drop) => match file_drop {
                FileDropEvent::Entered { position, paths } => {
                    self.mouse_position = position;
                    if let Some(active_drag) = cx.active_drag.as_mut() {
                        // Platform file-drop backends may refine the same logical drag payload as
                        // additional files are discovered. Keep ExternalPaths latest-wins so
                        // DragMoveEvent::drag() and the eventual on_drop handler see the complete
                        // batch instead of the first path that entered the window.
                        if active_drag
                            .value
                            .downcast_ref::<ExternalPaths>()
                            .is_some()
                        {
                            active_drag.value = Arc::new(paths);
                        }
                    } else {
                        cx.active_drag = Some(AnyDrag {
                            value: Arc::new(paths.clone()),
                            view: cx.new(|_| paths).into(),
                            cursor_offset: position,
                            cursor_style: None,
                        });
                        // The first drag frame must visit interactive application content once so
                        // drag-over/group-drag-over transition listeners are installed. Subsequent
                        // drag motion can stay replay-only and invalidate only changed hit targets.
                        self.refresh();
                    }
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position,
                        pressed_button: Some(MouseButton::Left),
                        modifiers: Modifiers::default(),
                    })
                }
                FileDropEvent::Pending { position } => {
                    self.mouse_position = position;
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position,
                        pressed_button: Some(MouseButton::Left),
                        modifiers: Modifiers::default(),
                    })
                }
                FileDropEvent::Submit { position } => {
                    cx.activate(true);
                    self.mouse_position = position;
                    PlatformInput::MouseUp(MouseUpEvent {
                        button: MouseButton::Left,
                        position,
                        modifiers: Modifiers::default(),
                        click_count: 1,
                    })
                }
                FileDropEvent::Exited => {
                    cx.active_drag.take();
                    // Drag end changes normal-hover/drag-over semantics across interactive content,
                    // so keep one conservative refresh here. High-frequency drag movement itself
                    // stays replay-only in dispatch_mouse_event.
                    self.refresh();
                    PlatformInput::FileDrop(FileDropEvent::Exited)
                }
            },
            PlatformInput::KeyDown(_) | PlatformInput::KeyUp(_) => event,
        };

        if !self.has_completed_rendered_frame {
            self.request_initial_frame();
            return DispatchEventResult {
                propagate: cx.propagate_event,
                default_prevented: self.default_prevented,
            };
        }

        if let Some(any_mouse_event) = event.mouse_event() {
            self.dispatch_mouse_event(any_mouse_event, cx);
        } else if let Some(any_key_event) = event.keyboard_event() {
            self.dispatch_key_event(any_key_event, cx);
        } else if let Some(touch_event) = event.touch_event() {
            self.dispatch_touch_event(touch_event, cx);
        }

        // The winit-based Windows backend reports committed text through KeyEvent::text,
        // which is stored in Keystroke::key_char. Key actions still run first, but text input
        // must not depend on event propagation: unrelated key listeners may stop propagation
        // after the focused input has been selected.
        #[cfg(target_os = "windows")]
        if let PlatformInput::KeyDown(key_down) = &event
            && is_text_input_keystroke(&key_down.keystroke)
            && let Some(input) = key_down.keystroke.key_char.as_deref()
            && let Some(mut input_handler) = self.platform_window.take_input_handler()
        {
            input_handler.dispatch_input(input, self, cx);
            self.platform_window.set_input_handler(input_handler);
            cx.propagate_event = false;
        }

        let event_elapsed = event_started_at.elapsed();
        log_timed_gpui_event("gpui dispatch_event", event_elapsed, || {
            format!(
                "event={} propagate={} default_prevented={}",
                event_name, cx.propagate_event, self.default_prevented
            )
        });

        DispatchEventResult {
            propagate: cx.propagate_event,
            default_prevented: self.default_prevented,
        }
    }

    /// Reconciles modality-aware hover state without synthesizing an application mouse event.
    ///
    /// The committed frame owns all framework hover/group-hover/tooltip listeners. They are
    /// invoked in normal capture/bubble order with a current-position MouseMoveEvent, but regular
    /// application MouseMove listeners are deliberately excluded.
    fn dispatch_input_modality_hover_transition(&mut self, cx: &mut App) {
        let event = MouseMoveEvent {
            position: self.mouse_position,
            pressed_button: None,
            modifiers: self.modifiers,
        };
        let event_type = TypeId::of::<MouseMoveEvent>();
        let mut mouse_listeners = mem::take(&mut self.rendered_frame.mouse_listeners);

        for listener in &mut mouse_listeners {
            if !listener.handles_input_modality_transition(event_type) {
                continue;
            }
            if let Some(mut listener) = listener.listener_mut() {
                listener(&event, DispatchPhase::Capture, self, cx);
            }
        }

        for listener in mouse_listeners.iter_mut().rev() {
            if !listener.handles_input_modality_transition(event_type) {
                continue;
            }
            if let Some(mut listener) = listener.listener_mut() {
                listener(&event, DispatchPhase::Bubble, self, cx);
            }
        }

        self.rendered_frame.mouse_listeners = mouse_listeners;
    }

    fn dispatch_touch_event(&mut self, event: &TouchEvent, cx: &mut App) {
        let recognized = self.touch_gestures.handle_event(event);
        let mut tapped = false;

        for gesture in recognized {
            tapped |= matches!(gesture, RecognizedTouchGesture::Tap { .. });
            self.dispatch_recognized_touch_gesture(gesture, cx);
        }

        if event.phase == TouchPhase::Started
            && let Some(touch_drag) = self.touch_gestures.offer_touch_drag(event.id)
        {
            self.dispatch_recognized_touch_gesture(touch_drag, cx);
        }

        if event.phase == TouchPhase::Started {
            self.schedule_long_press_timer(cx);
        } else if self.touch_gestures.pending_long_press().is_none() {
            self.long_press_timer.take();
        }

        if tapped && self.invalidator.is_dirty() {
            self.draw(cx).clear();
        }

        if self.touch_gestures.has_momentum() {
            self.schedule_touch_momentum_tick();
        }
    }

    fn dispatch_recognized_touch_gesture(
        &mut self,
        gesture: RecognizedTouchGesture,
        cx: &mut App,
    ) {
        match gesture {
            RecognizedTouchGesture::Scroll(scroll) => {
                self.mouse_position = scroll.position;
                cx.propagate_event = true;
                self.dispatch_mouse_event(&scroll, cx);
            }
            RecognizedTouchGesture::Tap { down, up } => {
                self.mouse_position = up.position;
                cx.propagate_event = true;
                self.dispatch_mouse_event(&down, cx);
                cx.propagate_event = true;
                self.dispatch_mouse_event(&up, cx);
            }
            RecognizedTouchGesture::TouchDrag(touch_drag) => {
                self.mouse_position = touch_drag.start_position;
                cx.propagate_event = true;
                self.default_prevented = false;
                let started = touch_drag.phase == TouchPhase::Started;
                self.dispatch_mouse_event(&touch_drag, cx);
                if started {
                    self.touch_gestures
                        .resolve_touch_drag(self.default_prevented);
                }
            }
            RecognizedTouchGesture::LongPress(long_press) => {
                self.mouse_position = long_press.start_position;
                cx.propagate_event = true;
                self.default_prevented = false;
                let started = long_press.phase == TouchPhase::Started;
                self.dispatch_mouse_event(&long_press, cx);
                if started {
                    self.touch_gestures
                        .resolve_long_press(self.default_prevented);
                }
            }
        }
    }

    fn schedule_long_press_timer(&mut self, cx: &mut App) {
        self.long_press_timer.take();
        let Some((touch_id, duration)) = self.touch_gestures.pending_long_press() else {
            return;
        };

        self.long_press_timer = Some(self.spawn(cx, async move |cx| {
            cx.background_executor.timer(duration).await;
            let _ = ignore_window_not_found(cx.update(move |window, cx| {
                window.long_press_timer.take();
                if let Some(gesture) = window.touch_gestures.offer_long_press(touch_id) {
                    window.dispatch_recognized_touch_gesture(gesture, cx);
                }
            }));
        }));
    }

    fn schedule_touch_momentum_tick(&mut self) {
        self.on_next_frame(|window, cx| {
            if let Some(gesture) = window.touch_gestures.tick_momentum() {
                window.dispatch_recognized_touch_gesture(gesture, cx);
            }
            if window.touch_gestures.has_momentum() {
                window.schedule_touch_momentum_tick();
            }
        });
    }

    pub(super) fn dispatch_mouse_event(&mut self, event: &dyn Any, cx: &mut App) {
        let mouse_position = event
            .downcast_ref::<MouseMoveEvent>()
            .map(|event| event.position)
            .or_else(|| {
                event
                    .downcast_ref::<MouseDownEvent>()
                    .map(|event| event.position)
            })
            .or_else(|| {
                event
                    .downcast_ref::<MouseUpEvent>()
                    .map(|event| event.position)
            })
            .or_else(|| {
                event
                    .downcast_ref::<MouseExitEvent>()
                    .map(|event| event.position)
            })
            .unwrap_or_else(|| self.mouse_position());
        let hit_test = if event.is::<MouseExitEvent>() {
            HitTest::default()
        } else {
            self.rendered_frame.hit_test(mouse_position)
        };
        let hit_test_unchanged = hit_test == self.mouse_hit_test;
        if hit_test != self.mouse_hit_test {
            self.mouse_hit_test = hit_test;
            self.reset_cursor_style(cx);
        }

        let client_resize_edge = if event.is::<MouseExitEvent>() {
            None
        } else if matches!(self.window_decorations(), Decorations::Client { .. }) {
            self.client_inset
                .and_then(|inset| resize_edge_hit_test(self, mouse_position, inset))
        } else {
            None
        };

        if let Some(edge) = client_resize_edge {
            if event.is::<MouseMoveEvent>() {
                self.reset_cursor_style(cx);
            } else if event
                .downcast_ref::<MouseDownEvent>()
                .is_some_and(|mouse_down| mouse_down.button == MouseButton::Left)
            {
                self.start_window_resize(edge);
                cx.propagate_event = false;
                self.default_prevented = true;
                return;
            }
        }

        self.dispatch_window_control_mouse_event(event, cx);
        if !cx.propagate_event {
            return;
        }

        #[cfg(any(feature = "inspector", debug_assertions))]
        if self.is_inspector_picking(cx) {
            self.handle_inspector_mouse_event(event, cx);
            // When inspector is picking, all other mouse handling is skipped.
            return;
        }

        let unpressed_mouse_move = event
            .downcast_ref::<MouseMoveEvent>()
            .is_some_and(|event| event.pressed_button.is_none());
        if unpressed_mouse_move
            && hit_test_unchanged
            && !cx.has_active_drag()
            && !self.rendered_frame.has_continuous_mouse_move_listener
        {
            // The hit-test set did not change, so all framework hover/tooltip transitions are
            // already stable. Cursor style still depends on the exact pointer coordinate for
            // client-side resize edges, therefore resolve it before skipping event traversal.
            self.reset_cursor_style(cx);
            record_skipped_pointer_frame();
            if log::log_enabled!(log::Level::Trace) {
                log::trace!(
                    "gpui mouse move skipped: unchanged hit-test without continuous listeners; active_drag={} ids={}",
                    cx.has_active_drag(),
                    self.mouse_hit_test.ids.len()
                );
            }
            return;
        }

        let active_drag_before_dispatch = cx.has_active_drag();
        let mut mouse_listeners = mem::take(&mut self.rendered_frame.mouse_listeners);
        let event_type = event.type_id();

        // Capture phase, events bubble from back to front. Handlers for this phase are used for
        // special purposes, such as detecting events outside of a given Bounds.
        for listener in &mut mouse_listeners {
            if !listener.handles(event_type, hit_test_unchanged) {
                continue;
            }
            let Some(mut listener) = listener.listener_mut() else {
                continue;
            };
            listener(event, DispatchPhase::Capture, self, cx);
            if !cx.propagate_event {
                break;
            }
        }

        // Bubble phase, where most normal handlers do their work.
        if cx.propagate_event {
            for listener in mouse_listeners.iter_mut().rev() {
                if !listener.handles(event_type, hit_test_unchanged) {
                    continue;
                }
                let Some(mut listener) = listener.listener_mut() else {
                    continue;
                };
                listener(event, DispatchPhase::Bubble, self, cx);
                if !cx.propagate_event {
                    break;
                }
            }
        }

        self.rendered_frame.mouse_listeners = mouse_listeners;

        if cx.has_active_drag() {
            if event.is::<MouseMoveEvent>() {
                if active_drag_before_dispatch {
                    // The drag preview is a window-owned overlay. Drag-over style targets installed
                    // by the first conservative drag frame are invalidated by hit-test transition
                    // listeners, so ordinary movement must not force every cached application view
                    // through MissRefresh.
                    self.redraw_without_view_cache_refresh();
                } else {
                    // A drag created by this mouse move changes global interaction semantics:
                    // normal hover is suppressed and drag-over listeners must be installed across
                    // the application tree. Pay one conservative refresh at drag start only.
                    self.refresh();
                }
            } else if event.is::<MouseUpEvent>() {
                // Ending a drag restores normal hover semantics across interactive content. Keep
                // this one transition conservative; high-frequency drag movement above is local.
                cx.active_drag = None;
                self.refresh();
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn is_text_input_keystroke(keystroke: &Keystroke) -> bool {
    let modifiers = keystroke.modifiers;
    !modifiers.platform
        && !modifiers.function
        && (!modifiers.control || modifiers.alt)
        && (!modifiers.alt || modifiers.control)
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;

    fn keystroke(modifiers: Modifiers) -> Keystroke {
        Keystroke {
            modifiers,
            key: "a".to_string(),
            key_char: Some("a".to_string()),
        }
    }

    #[test]
    fn text_input_allows_plain_and_altgr_keys_but_not_shortcuts() {
        assert!(is_text_input_keystroke(&keystroke(Modifiers::default())));
        assert!(is_text_input_keystroke(&keystroke(Modifiers {
            control: true,
            alt: true,
            ..Modifiers::default()
        })));
        assert!(!is_text_input_keystroke(&keystroke(Modifiers::control())));
        assert!(!is_text_input_keystroke(&keystroke(Modifiers::alt())));
        assert!(!is_text_input_keystroke(&keystroke(Modifiers::windows())));
    }
}
