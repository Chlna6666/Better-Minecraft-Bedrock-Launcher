use crate::{
    Action, App, ClickEvent, KeyDownEvent, KeyUpEvent, ModifiersChangedEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ScrollWheelEvent, Window, WindowControlArea,
};
use std::{
    any::{Any, TypeId},
    rc::Rc,
};

use super::state::Interactivity;
pub(crate) type MouseDownListener = Box<
    dyn Fn(&MouseDownEvent, crate::DispatchPhase, &crate::Hitbox, &mut Window, &mut App) + 'static,
>;
pub(crate) type MouseUpListener = Box<
    dyn Fn(&MouseUpEvent, crate::DispatchPhase, &crate::Hitbox, &mut Window, &mut App) + 'static,
>;
pub(crate) type MouseMoveListener = Box<
    dyn Fn(&MouseMoveEvent, crate::DispatchPhase, &crate::Hitbox, &mut Window, &mut App) + 'static,
>;
pub(crate) type ScrollWheelListener = Box<
    dyn Fn(&ScrollWheelEvent, crate::DispatchPhase, &crate::Hitbox, &mut Window, &mut App)
        + 'static,
>;
pub(crate) type ClickListener = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;
pub(crate) type KeyDownListener =
    Box<dyn Fn(&KeyDownEvent, crate::DispatchPhase, &mut Window, &mut App) + 'static>;
pub(crate) type KeyUpListener =
    Box<dyn Fn(&KeyUpEvent, crate::DispatchPhase, &mut Window, &mut App) + 'static>;
pub(crate) type ModifiersChangedListener =
    Box<dyn Fn(&ModifiersChangedEvent, &mut Window, &mut App) + 'static>;
pub(crate) type ActionListener =
    Box<dyn Fn(&dyn Any, crate::DispatchPhase, &mut Window, &mut App) + 'static>;

impl Interactivity {
    #[allow(missing_docs)]
    pub fn on_mouse_down(
        &mut self,
        button: MouseButton,
        listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) {
        self.mouse_down_listeners
            .push(Box::new(move |event, phase, hitbox, window, cx| {
                if phase == crate::DispatchPhase::Bubble
                    && event.button == button
                    && hitbox.is_hovered(window)
                {
                    (listener)(event, window, cx);
                }
            }));
    }

    #[allow(missing_docs)]
    pub fn capture_any_mouse_down(
        &mut self,
        listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) {
        self.mouse_down_listeners
            .push(Box::new(move |event, phase, hitbox, window, cx| {
                if phase == crate::DispatchPhase::Capture && hitbox.is_hovered(window) {
                    (listener)(event, window, cx);
                }
            }));
    }

    #[allow(missing_docs)]
    pub fn on_any_mouse_down(
        &mut self,
        listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) {
        self.mouse_down_listeners
            .push(Box::new(move |event, phase, hitbox, window, cx| {
                if phase == crate::DispatchPhase::Bubble && hitbox.is_hovered(window) {
                    (listener)(event, window, cx);
                }
            }));
    }

    #[allow(missing_docs)]
    pub fn on_mouse_up(
        &mut self,
        button: MouseButton,
        listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) {
        self.mouse_up_listeners
            .push(Box::new(move |event, phase, hitbox, window, cx| {
                if phase == crate::DispatchPhase::Bubble
                    && event.button == button
                    && hitbox.is_hovered(window)
                {
                    (listener)(event, window, cx);
                }
            }));
    }

    #[allow(missing_docs)]
    pub fn capture_any_mouse_up(
        &mut self,
        listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) {
        self.mouse_up_listeners
            .push(Box::new(move |event, phase, hitbox, window, cx| {
                if phase == crate::DispatchPhase::Capture && hitbox.is_hovered(window) {
                    (listener)(event, window, cx);
                }
            }));
    }

    #[allow(missing_docs)]
    pub fn on_any_mouse_up(
        &mut self,
        listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) {
        self.mouse_up_listeners
            .push(Box::new(move |event, phase, hitbox, window, cx| {
                if phase == crate::DispatchPhase::Bubble && hitbox.is_hovered(window) {
                    (listener)(event, window, cx);
                }
            }));
    }

    #[allow(missing_docs)]
    pub fn on_mouse_down_out(
        &mut self,
        listener: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) {
        self.mouse_down_listeners
            .push(Box::new(move |event, phase, hitbox, window, cx| {
                if phase == crate::DispatchPhase::Capture
                    && !hitbox.contains(&window.mouse_position())
                {
                    (listener)(event, window, cx);
                }
            }));
    }

    #[allow(missing_docs)]
    pub fn on_mouse_up_out(
        &mut self,
        button: MouseButton,
        listener: impl Fn(&MouseUpEvent, &mut Window, &mut App) + 'static,
    ) {
        self.mouse_up_listeners
            .push(Box::new(move |event, phase, hitbox, window, cx| {
                if phase == crate::DispatchPhase::Capture
                    && event.button == button
                    && !hitbox.is_hovered(window)
                {
                    (listener)(event, window, cx);
                }
            }));
    }

    #[allow(missing_docs)]
    pub fn on_mouse_move(
        &mut self,
        listener: impl Fn(&MouseMoveEvent, &mut Window, &mut App) + 'static,
    ) {
        self.mouse_move_listeners
            .push(Box::new(move |event, phase, hitbox, window, cx| {
                if phase == crate::DispatchPhase::Bubble && hitbox.is_hovered(window) {
                    (listener)(event, window, cx);
                }
            }));
    }

    #[allow(missing_docs)]
    pub fn on_scroll_wheel(
        &mut self,
        listener: impl Fn(&ScrollWheelEvent, &mut Window, &mut App) + 'static,
    ) {
        self.scroll_wheel_listeners
            .push(Box::new(move |event, phase, hitbox, window, cx| {
                if phase == crate::DispatchPhase::Bubble && hitbox.should_handle_scroll(window) {
                    (listener)(event, window, cx);
                }
            }));
    }

    #[allow(missing_docs)]
    pub fn capture_action<A: Action>(
        &mut self,
        listener: impl Fn(&A, &mut Window, &mut App) + 'static,
    ) {
        self.action_listeners.push((
            TypeId::of::<A>(),
            Box::new(move |action, phase, window, cx| {
                let action = action.downcast_ref().unwrap();
                if phase == crate::DispatchPhase::Capture {
                    (listener)(action, window, cx);
                } else {
                    cx.propagate();
                }
            }),
        ));
    }

    #[allow(missing_docs)]
    pub fn on_action<A: Action>(&mut self, listener: impl Fn(&A, &mut Window, &mut App) + 'static) {
        self.action_listeners.push((
            TypeId::of::<A>(),
            Box::new(move |action, phase, window, cx| {
                if phase == crate::DispatchPhase::Bubble {
                    (listener)(action.downcast_ref().unwrap(), window, cx);
                }
            }),
        ));
    }

    #[allow(missing_docs)]
    pub fn on_boxed_action(
        &mut self,
        action: &dyn Action,
        listener: impl Fn(&dyn Action, &mut Window, &mut App) + 'static,
    ) {
        let action = action.boxed_clone();
        self.action_listeners.push((
            (*action).type_id(),
            Box::new(move |_, phase, window, cx| {
                if phase == crate::DispatchPhase::Bubble {
                    (listener)(&*action, window, cx);
                }
            }),
        ));
    }

    #[allow(missing_docs)]
    pub fn on_key_down(
        &mut self,
        listener: impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static,
    ) {
        self.key_down_listeners
            .push(Box::new(move |event, phase, window, cx| {
                if phase == crate::DispatchPhase::Bubble {
                    (listener)(event, window, cx);
                }
            }));
    }

    #[allow(missing_docs)]
    pub fn capture_key_down(
        &mut self,
        listener: impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static,
    ) {
        self.key_down_listeners
            .push(Box::new(move |event, phase, window, cx| {
                if phase == crate::DispatchPhase::Capture {
                    listener(event, window, cx);
                }
            }));
    }

    #[allow(missing_docs)]
    pub fn on_key_up(&mut self, listener: impl Fn(&KeyUpEvent, &mut Window, &mut App) + 'static) {
        self.key_up_listeners
            .push(Box::new(move |event, phase, window, cx| {
                if phase == crate::DispatchPhase::Bubble {
                    listener(event, window, cx);
                }
            }));
    }

    #[allow(missing_docs)]
    pub fn capture_key_up(
        &mut self,
        listener: impl Fn(&KeyUpEvent, &mut Window, &mut App) + 'static,
    ) {
        self.key_up_listeners
            .push(Box::new(move |event, phase, window, cx| {
                if phase == crate::DispatchPhase::Capture {
                    listener(event, window, cx);
                }
            }));
    }

    #[allow(missing_docs)]
    pub fn on_modifiers_changed(
        &mut self,
        listener: impl Fn(&ModifiersChangedEvent, &mut Window, &mut App) + 'static,
    ) {
        self.modifiers_changed_listeners
            .push(Box::new(move |event, window, cx| {
                listener(event, window, cx);
            }));
    }

    #[allow(missing_docs)]
    pub fn on_click(&mut self, listener: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static)
    where
        Self: Sized,
    {
        self.click_listeners.push(Rc::new(move |event, window, cx| {
            listener(event, window, cx);
        }));
    }

    #[allow(missing_docs)]
    pub fn on_hover(&mut self, listener: impl Fn(&bool, &mut Window, &mut App) + 'static)
    where
        Self: Sized,
    {
        debug_assert!(
            self.hover_listener.is_none(),
            "calling on_hover more than once on the same element is not supported"
        );
        self.hover_listener = Some(Box::new(listener));
    }

    #[allow(missing_docs)]
    pub fn occlude_mouse(&mut self) {
        self.hitbox_behavior = crate::HitboxBehavior::BlockMouse;
    }

    #[allow(missing_docs)]
    pub fn window_control_area(&mut self, area: WindowControlArea) {
        self.window_control = Some(area);
    }

    #[allow(missing_docs)]
    pub fn block_mouse_except_scroll(&mut self) {
        self.hitbox_behavior = crate::HitboxBehavior::BlockMouseExceptScroll;
    }
}
