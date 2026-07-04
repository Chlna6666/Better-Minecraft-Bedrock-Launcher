use crate::{
    AnyDrag, App, Bounds, DispatchPhase, Hitbox, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, Render, SharedString, Style, StyleRefinement, Window,
    record_style_refine,
};
use refineable::Refineable;
use std::{
    any::{Any, TypeId},
    cell::RefCell,
    marker::PhantomData,
    rc::Rc,
    sync::Arc,
};

use super::{frame_state::ElementClickedState, state::Interactivity, style_state::GroupHitboxes};

pub(crate) type DragListener =
    Box<dyn Fn(&dyn Any, Point<Pixels>, &mut Window, &mut App) -> crate::AnyView + 'static>;
pub(crate) type DropListener = Box<dyn Fn(&dyn Any, &mut Window, &mut App) + 'static>;
pub(crate) type CanDropPredicate = Box<dyn Fn(&dyn Any, &mut Window, &mut App) -> bool + 'static>;

/// The styling information for a given group.
pub struct GroupStyle {
    /// The identifier for this group.
    pub group: SharedString,

    /// The specific style refinement that this group would apply
    /// to its children.
    pub style: Box<StyleRefinement>,
}

/// An event for when a drag is moving over this element, with the given state type.
pub struct DragMoveEvent<T> {
    /// The mouse move event that triggered this drag move event.
    pub event: MouseMoveEvent,

    /// The bounds of this element.
    pub bounds: Bounds<Pixels>,
    pub(crate) drag: PhantomData<T>,
    pub(crate) dragged_item: Arc<dyn Any>,
}

impl<T: 'static> DragMoveEvent<T> {
    /// Returns the drag state for this event.
    pub fn drag<'b>(&self, cx: &'b App) -> &'b T {
        cx.active_drag
            .as_ref()
            .and_then(|drag| drag.value.downcast_ref::<T>())
            .expect("DragMoveEvent is only valid when the stored active drag is of the same type.")
    }

    /// An item that is about to be dropped.
    pub fn dragged_item(&self) -> &dyn Any {
        self.dragged_item.as_ref()
    }
}

impl Interactivity {
    /// Apply drag-over styles and cursor updates when a compatible payload is active.
    pub(crate) fn apply_drag_over_styles(
        &self,
        hitbox: &Hitbox,
        style: &mut Style,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(drag) = cx.active_drag.take() {
            let mut can_drop = true;
            if let Some(can_drop_predicate) = &self.can_drop_predicate {
                can_drop = can_drop_predicate(drag.value.as_ref(), window, cx);
            }

            if can_drop {
                for (state_type, group_drag_style) in &self.group_drag_over_styles {
                    if let Some(group_hitbox_id) = GroupHitboxes::get(&group_drag_style.group, cx)
                        && *state_type == drag.value.as_ref().type_id()
                        && group_hitbox_id.is_hovered(window)
                    {
                        record_style_refine(1);
                        style.refine(&group_drag_style.style);
                    }
                }

                for (state_type, build_drag_over_style) in &self.drag_over_styles {
                    if *state_type == drag.value.as_ref().type_id() && hitbox.is_hovered(window) {
                        record_style_refine(1);
                        style.refine(&build_drag_over_style(drag.value.as_ref(), window, cx));
                    }
                }
            }

            style.mouse_cursor = drag.cursor_style;
            cx.active_drag = Some(drag);
        }
    }

    /// Register a computed drag-over style builder for a specific drag payload type.
    pub(crate) fn drag_over<S: 'static>(
        &mut self,
        f: impl 'static + Fn(StyleRefinement, &S, &mut Window, &mut App) -> StyleRefinement,
    ) {
        self.drag_over_styles.push((
            TypeId::of::<S>(),
            Box::new(move |currently_dragged: &dyn Any, window, cx| {
                f(
                    StyleRefinement::default(),
                    currently_dragged.downcast_ref::<S>().unwrap(),
                    window,
                    cx,
                )
            }),
        ));
    }

    /// Register a group drag-over style that activates when a compatible payload is dragged over the group.
    pub(crate) fn group_drag_over_for_type(
        &mut self,
        state_type: TypeId,
        group_name: impl Into<SharedString>,
        f: impl FnOnce(StyleRefinement) -> StyleRefinement,
    ) {
        self.group_drag_over_styles.push((
            state_type,
            GroupStyle {
                group: group_name.into(),
                style: Box::new(f(StyleRefinement::default())),
            },
        ));
    }

    #[allow(missing_docs)]
    pub fn on_drag_move<T>(
        &mut self,
        listener: impl Fn(&DragMoveEvent<T>, &mut Window, &mut App) + 'static,
    ) where
        T: 'static,
    {
        self.mouse_move_listeners
            .push(Box::new(move |event, phase, hitbox, window, cx| {
                if phase == crate::DispatchPhase::Capture
                    && let Some(drag) = &cx.active_drag
                    && drag.value.as_ref().type_id() == TypeId::of::<T>()
                {
                    (listener)(
                        &DragMoveEvent {
                            event: event.clone(),
                            bounds: hitbox.bounds,
                            drag: std::marker::PhantomData,
                            dragged_item: Arc::clone(&drag.value),
                        },
                        window,
                        cx,
                    );
                }
            }));
    }

    #[allow(missing_docs)]
    pub fn on_drop<T: 'static>(&mut self, listener: impl Fn(&T, &mut Window, &mut App) + 'static) {
        self.drop_listeners.push((
            TypeId::of::<T>(),
            Box::new(move |dragged_value, window, cx| {
                listener(dragged_value.downcast_ref().unwrap(), window, cx);
            }),
        ));
    }

    #[allow(missing_docs)]
    pub fn can_drop(
        &mut self,
        predicate: impl Fn(&dyn Any, &mut Window, &mut App) -> bool + 'static,
    ) {
        self.can_drop_predicate = Some(Box::new(predicate));
    }

    #[allow(missing_docs)]
    pub fn on_drag<T, W>(
        &mut self,
        value: T,
        constructor: impl Fn(&T, Point<Pixels>, &mut Window, &mut App) -> crate::Entity<W> + 'static,
    ) where
        Self: Sized,
        T: 'static,
        W: 'static + Render,
    {
        debug_assert!(
            self.drag_listener.is_none(),
            "calling on_drag more than once on the same element is not supported"
        );
        self.drag_listener = Some((
            Arc::new(value),
            Box::new(move |value, offset, window, cx| {
                constructor(value.downcast_ref().unwrap(), offset, window, cx).into()
            }),
        ));
    }
}

pub(crate) fn bind_drop_listeners(
    hitbox: &Hitbox,
    drop_listeners: Vec<(TypeId, DropListener)>,
    can_drop_predicate: Option<CanDropPredicate>,
    window: &mut Window,
) {
    if drop_listeners.is_empty() {
        return;
    }

    let hitbox = hitbox.clone();
    window.on_mouse_event(move |_: &MouseUpEvent, phase, window, cx| {
        if let Some(drag) = &cx.active_drag
            && phase == DispatchPhase::Bubble
            && hitbox.is_hovered(window)
        {
            let drag_state_type = drag.value.as_ref().type_id();
            for (drop_state_type, listener) in &drop_listeners {
                if *drop_state_type == drag_state_type {
                    let drag = cx
                        .active_drag
                        .take()
                        .expect("checked for type drag state type above");

                    let mut can_drop = true;
                    if let Some(predicate) = &can_drop_predicate {
                        can_drop = predicate(drag.value.as_ref(), window, cx);
                    }

                    if can_drop {
                        listener(drag.value.as_ref(), window, cx);
                        window.refresh();
                        cx.stop_propagation();
                    }
                }
            }
        }
    });
}

pub(crate) fn bind_drag_start_listeners(
    hitbox: &Hitbox,
    pending_mouse_down: Rc<RefCell<Option<MouseDownEvent>>>,
    clicked_state: Rc<RefCell<ElementClickedState>>,
    has_active_style: bool,
    drag_cursor_style: Option<crate::CursorStyle>,
    mut drag_listener: Option<(Arc<dyn Any>, DragListener)>,
    window: &mut Window,
) {
    window.on_mouse_event({
        let pending_mouse_down = pending_mouse_down.clone();
        let hitbox = hitbox.clone();
        move |event: &MouseDownEvent, phase, window, _cx| {
            if phase == DispatchPhase::Bubble
                && event.button == MouseButton::Left
                && hitbox.is_hovered(window)
            {
                *pending_mouse_down.borrow_mut() = Some(event.clone());
                if has_active_style {
                    window.refresh();
                }
            }
        }
    });

    window.on_mouse_event({
        let hitbox = hitbox.clone();
        move |event: &MouseMoveEvent, phase, window, cx| {
            if phase == DispatchPhase::Capture {
                return;
            }

            let mut pending_mouse_down = pending_mouse_down.borrow_mut();
            if let Some(mouse_down) = pending_mouse_down.clone()
                && !cx.has_active_drag()
                && (event.position - mouse_down.position).magnitude() > 2.0
                && let Some((drag_value, drag_listener)) = drag_listener.take()
            {
                *clicked_state.borrow_mut() = ElementClickedState::default();
                let cursor_offset = event.position - hitbox.origin;
                let drag = (drag_listener)(drag_value.as_ref(), cursor_offset, window, cx);
                cx.active_drag = Some(AnyDrag {
                    view: drag,
                    value: drag_value,
                    cursor_offset,
                    cursor_style: drag_cursor_style,
                });
                pending_mouse_down.take();
                window.refresh();
                cx.stop_propagation();
            }
        }
    });
}
