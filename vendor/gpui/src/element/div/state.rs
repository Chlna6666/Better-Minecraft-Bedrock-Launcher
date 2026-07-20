//! Div is the central, reusable element that most GPUI trees will be built from.
//! It functions as a container for other elements, and provides a number of
//! useful features for laying out and styling its children as well as binding
//! mouse events and action handlers. It is meant to be similar to the HTML `<div>`
//! element, but for GPUI.
//!
//! # Build your own div
//!
//! GPUI does not directly provide APIs for stateful, multi step events like `click`
//! and `drag`. We want GPUI users to be able to build their own abstractions for
//! their own needs. However, as a UI framework, we're also obliged to provide some
//! building blocks to make the process of building your own elements easier.
//! For this we have the [`Interactivity`] and the [`StyleRefinement`] structs, as well
//! as several associated traits. Together, these provide the full suite of Dom-like events
//! and Tailwind-like styling that you can use to build your own custom elements. Div is
//! constructed by combining these two systems into an all-in-one element.

use crate::{
    App, Bounds, ElementId, FocusHandle, GlobalElementId, Hitbox, HitboxBehavior,
    InspectorElementId, KeyContext, LayoutId, Pixels, Point, SharedString, Size, Style,
    StyleRefinement, TooltipId, Visibility, Window, WindowControlArea,
};
use std::{
    any::{Any, TypeId},
    cell::RefCell,
    rc::Rc,
    sync::Arc,
};

use super::drag_drop::GroupStyle;
use super::drag_drop::{CanDropPredicate, DragListener, DropListener};
use super::event_handlers::{
    ActionListener, ClickListener, KeyDownListener, KeyUpListener, ModifiersChangedListener,
    MouseDownListener, MouseMoveListener, MouseUpListener, ScrollWheelListener,
};
use super::frame_state::{ElementClickedState, InteractiveElementState};
use super::scroll::{ScrollAnchor, ScrollHandle};
use super::style_state::{ComputedStyleCache, GroupHitboxes};
use super::tooltip::TooltipBuilder;

/// The interactivity struct. Powers all of the general-purpose
/// interactivity in the `Div` element.
#[derive(Default)]
pub struct Interactivity {
    /// The element ID of the element. In id is required to support a stateful subset of the interactivity such as on_click.
    pub element_id: Option<ElementId>,
    /// Whether the element was clicked. This will only be present after layout.
    pub active: Option<bool>,
    /// Whether the element was hovered. This will only be present after paint if an hitbox
    /// was created for the interactive element.
    pub hovered: Option<bool>,
    pub(crate) tooltip_id: Option<TooltipId>,
    pub(crate) content_size: Size<Pixels>,
    pub(crate) key_context: Option<KeyContext>,
    pub(crate) focusable: bool,
    pub(crate) tracked_focus_handle: Option<FocusHandle>,
    pub(crate) tracked_scroll_handle: Option<ScrollHandle>,
    pub(crate) scroll_anchor: Option<ScrollAnchor>,
    pub(crate) scroll_offset: Option<Rc<RefCell<Point<Pixels>>>>,
    pub(crate) group: Option<SharedString>,
    /// The base style of the element, before any modifications are applied
    /// by focus, active, etc.
    pub base_style: Box<StyleRefinement>,
    pub(crate) focus_style: Option<Box<StyleRefinement>>,
    pub(crate) in_focus_style: Option<Box<StyleRefinement>>,
    pub(crate) hover_style: Option<Box<StyleRefinement>>,
    pub(crate) group_hover_style: Option<GroupStyle>,
    pub(crate) active_style: Option<Box<StyleRefinement>>,
    pub(crate) group_active_style: Option<GroupStyle>,
    pub(crate) drag_over_styles: Vec<(
        TypeId,
        Box<dyn Fn(&dyn Any, &mut Window, &mut App) -> StyleRefinement>,
    )>,
    pub(crate) group_drag_over_styles: Vec<(TypeId, GroupStyle)>,
    pub(crate) mouse_down_listeners: Vec<MouseDownListener>,
    pub(crate) mouse_up_listeners: Vec<MouseUpListener>,
    pub(crate) mouse_move_listeners: Vec<MouseMoveListener>,
    pub(crate) scroll_wheel_listeners: Vec<ScrollWheelListener>,
    pub(crate) key_down_listeners: Vec<KeyDownListener>,
    pub(crate) key_up_listeners: Vec<KeyUpListener>,
    pub(crate) modifiers_changed_listeners: Vec<ModifiersChangedListener>,
    pub(crate) action_listeners: Vec<(TypeId, ActionListener)>,
    pub(crate) drop_listeners: Vec<(TypeId, DropListener)>,
    pub(crate) can_drop_predicate: Option<CanDropPredicate>,
    pub(crate) click_listeners: Vec<ClickListener>,
    pub(crate) drag_listener: Option<(Arc<dyn Any>, DragListener)>,
    pub(crate) hover_listener: Option<Box<dyn Fn(&bool, &mut Window, &mut App)>>,
    pub(crate) tooltip_builder: Option<TooltipBuilder>,
    pub(crate) computed_style_cache: Option<ComputedStyleCache>,
    pub(crate) window_control: Option<WindowControlArea>,
    pub(crate) hitbox_behavior: HitboxBehavior,
    pub(crate) tab_index: Option<isize>,
    pub(crate) tab_group: bool,
    pub(crate) tab_stop: bool,

    #[cfg(any(feature = "inspector", debug_assertions))]
    pub(crate) source_location: Option<&'static core::panic::Location<'static>>,

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) debug_selector: Option<String>,
}

impl Interactivity {
    pub(crate) fn should_insert_hitbox(&self, style: &Style, window: &Window, cx: &App) -> bool {
        self.hitbox_behavior != HitboxBehavior::Normal
            || self.window_control.is_some()
            || style.mouse_cursor.is_some()
            || self.group.is_some()
            || self.scroll_offset.is_some()
            || self.tracked_focus_handle.is_some()
            || self.hover_style.is_some()
            || self.group_hover_style.is_some()
            || self.hover_listener.is_some()
            || !self.mouse_up_listeners.is_empty()
            || !self.mouse_down_listeners.is_empty()
            || !self.mouse_move_listeners.is_empty()
            || !self.click_listeners.is_empty()
            || !self.scroll_wheel_listeners.is_empty()
            || self.drag_listener.is_some()
            || !self.drop_listeners.is_empty()
            || self.tooltip_builder.is_some()
            || window.is_inspector_picking(cx)
    }

    /// Layout this element according to this interactivity state's configured styles
    pub fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
        f: impl FnOnce(Style, &mut Window, &mut App) -> LayoutId,
    ) -> LayoutId {
        self.sync_inspector_layout_state(inspector_id, window, cx);

        window.with_optional_element_state::<InteractiveElementState, _>(
            global_id,
            |element_state, window| {
                let mut element_state =
                    element_state.map(|element_state| element_state.unwrap_or_default());

                if let Some(element_state) = element_state.as_ref()
                    && cx.has_active_drag()
                {
                    if let Some(pending_mouse_down) = element_state.pending_mouse_down.as_ref() {
                        *pending_mouse_down.borrow_mut() = None;
                    }
                    if let Some(clicked_state) = element_state.clicked_state.as_ref() {
                        *clicked_state.borrow_mut() = ElementClickedState::default();
                    }
                }

                if self.focusable
                    && self.tracked_focus_handle.is_none()
                    && let Some(element_state) = element_state.as_mut()
                {
                    self.ensure_focus_handle(element_state, cx);
                }

                self.resolve_scroll_offset(element_state.as_mut());

                let style = self.compute_style_internal(None, element_state.as_mut(), window, cx);
                let layout_id = f(style, window, cx);
                (layout_id, element_state)
            },
        )
    }

    /// Commit the bounds of this element according to this interactivity state's configured styles.
    pub fn prepaint<R>(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        content_size: Size<Pixels>,
        window: &mut Window,
        cx: &mut App,
        f: impl FnOnce(&Style, Point<Pixels>, Option<Hitbox>, &mut Window, &mut App) -> R,
    ) -> R {
        self.content_size = content_size;

        self.sync_inspector_prepaint_state(inspector_id, bounds, content_size, window, cx);

        if let Some(focus_handle) = self.tracked_focus_handle.as_ref() {
            window.set_focus_handle(focus_handle, cx);
        }
        window.with_optional_element_state::<InteractiveElementState, _>(
            global_id,
            |element_state, window| {
                let mut element_state =
                    element_state.map(|element_state| element_state.unwrap_or_default());
                let style = self.compute_style_internal(None, element_state.as_mut(), window, cx);

                if let Some(element_state) = element_state.as_mut() {
                    if let Some(clicked_state) = element_state.clicked_state.as_ref() {
                        let clicked_state = clicked_state.borrow();
                        self.active = Some(clicked_state.element);
                    }
                    self.sync_active_tooltip(element_state, window);
                }

                window.with_element_scale(bounds, style.scale, style.transform_origin, |window| {
                    window.with_text_style(style.text_style().cloned(), |window| {
                        window.with_content_mask(
                            style.overflow_mask(bounds, window.rem_size()),
                            |window| {
                                let hitbox = if self.should_insert_hitbox(&style, window, cx) {
                                    Some(window.insert_hitbox(bounds, self.hitbox_behavior))
                                } else {
                                    None
                                };

                                let scroll_offset =
                                    self.clamp_scroll_position(bounds, &style, window, cx);
                                let result = f(&style, scroll_offset, hitbox, window, cx);
                                (result, element_state)
                            },
                        )
                    })
                })
            },
        )
    }

    /// Paint this element according to this interactivity state's configured styles
    /// and bind the element's mouse and keyboard events.
    ///
    /// content_size is the size of the content of the element, which may be larger than the
    /// element's bounds if the element is scrollable.
    ///
    /// the final computed style will be passed to the provided function, along
    /// with the current scroll offset
    pub fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        hitbox: Option<&Hitbox>,
        window: &mut Window,
        cx: &mut App,
        f: impl FnOnce(&Style, &mut Window, &mut App),
    ) {
        self.hovered = hitbox.map(|hitbox| hitbox.is_hovered(window));
        window.with_optional_element_state::<InteractiveElementState, _>(
            global_id,
            |element_state, window| {
                let mut element_state =
                    element_state.map(|element_state| element_state.unwrap_or_default());

                let style = self.compute_style_internal(hitbox, element_state.as_mut(), window, cx);

                #[cfg(any(feature = "test-support", test))]
                if let Some(debug_selector) = &self.debug_selector {
                    window
                        .next_frame
                        .debug_bounds
                        .insert(debug_selector.clone(), window.visual_bounds(bounds));
                }

                self.paint_hover_group_handler(window, cx);

                if style.visibility == Visibility::Hidden {
                    return ((), element_state);
                }

                let mut tab_group = None;
                if self.tab_group {
                    tab_group = self.tab_index;
                }
                if let Some(focus_handle) = &self.tracked_focus_handle {
                    window.next_frame.tab_stops.insert(focus_handle);
                }

                window.with_element_scale(bounds, style.scale, style.transform_origin, |window| {
                    window.with_element_opacity(style.opacity, |window| {
                        style.paint(bounds, window, cx, |window: &mut Window, cx: &mut App| {
                            window.with_text_style(style.text_style().cloned(), |window| {
                                window.with_content_mask(
                                    style.overflow_mask(bounds, window.rem_size()),
                                    |window| {
                                        window.with_tab_group(tab_group, |window| {
                                            if let Some(hitbox) = hitbox {
                                                #[cfg(debug_assertions)]
                                                self.paint_debug_info(
                                                    global_id, hitbox, &style, window, cx,
                                                );

                                                if let Some(drag) = cx.active_drag.as_ref() {
                                                    if let Some(mouse_cursor) = drag.cursor_style {
                                                        window
                                                            .set_window_cursor_style(mouse_cursor);
                                                    }
                                                } else {
                                                    if let Some(mouse_cursor) = style.mouse_cursor {
                                                        window
                                                            .set_cursor_style(mouse_cursor, hitbox);
                                                    }
                                                }

                                                if let Some(group) = self.group.clone() {
                                                    GroupHitboxes::push(group, hitbox.id, cx);
                                                }

                                                if let Some(area) = self.window_control {
                                                    window.insert_window_control_hitbox(
                                                        area,
                                                        hitbox.clone(),
                                                    );
                                                }

                                                self.paint_mouse_listeners(
                                                    hitbox,
                                                    element_state.as_mut(),
                                                    window,
                                                    cx,
                                                );
                                                self.paint_scroll_listener(
                                                    hitbox, &style, window, cx,
                                                );
                                            }

                                            self.paint_keyboard_listeners(window, cx);
                                            f(&style, window, cx);

                                            if let Some(_hitbox) = hitbox {
                                                #[cfg(any(
                                                    feature = "inspector",
                                                    debug_assertions
                                                ))]
                                                window.insert_inspector_hitbox(
                                                    _hitbox.id,
                                                    _inspector_id,
                                                    cx,
                                                );

                                                if let Some(group) = self.group.as_ref() {
                                                    GroupHitboxes::pop(group, cx);
                                                }
                                            }
                                        })
                                    },
                                );
                            });
                        });
                    });
                });

                ((), element_state)
            },
        );
    }
}
