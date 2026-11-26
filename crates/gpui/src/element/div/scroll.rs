use crate::{
    App, Bounds, DispatchPhase, Hitbox, IsZero, Overflow, Pixels, Point, ScrollWheelEvent, Size,
    Window, px, size,
};
use std::{cell::RefCell, cmp::Ordering, mem, rc::Rc};
use crate::window::scroll_dependencies::{ScrollReadKind, observe};

use super::interactivity::Interactivity;
use super::state::InteractiveElementState;

/// Represents an element that can be scrolled *to* in its parent element.
/// Unlike [`ScrollHandle::scroll_to_active_item`], an anchored element does not
/// have to be an immediate child of the parent.
#[derive(Clone)]
pub struct ScrollAnchor {
    pub(crate) handle: ScrollHandle,
    pub(crate) last_origin: Rc<RefCell<Point<Pixels>>>,
}

impl ScrollAnchor {
    /// Creates a [ScrollAnchor] associated with a given [ScrollHandle].
    pub fn for_handle(handle: ScrollHandle) -> Self {
        Self {
            handle,
            last_origin: Default::default(),
        }
    }

    /// Request scroll to this item on the next frame.
    pub fn scroll_to(&self, window: &mut Window, _cx: &mut App) {
        let this = self.clone();

        window.on_next_frame(move |_, _| {
            let viewport_bounds = this.handle.bounds();
            let self_bounds = *this.last_origin.borrow();
            this.handle.set_offset(viewport_bounds.origin - self_bounds);
        });
    }
}

#[derive(Default, Debug)]
pub(crate) struct ScrollHandleState {
    pub(crate) offset: Rc<RefCell<Point<Pixels>>>,
    pub(crate) ongoing_scroll: Rc<RefCell<crate::OngoingScroll>>,
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) max_offset: Size<Pixels>,
    pub(crate) child_bounds: Vec<Bounds<Pixels>>,
    pub(crate) scroll_to_bottom: bool,
    pub(crate) overflow: Point<Overflow>,
    pub(crate) active_item: Option<ScrollActiveItem>,
}

#[derive(Default, Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct ScrollActiveItem {
    pub(crate) index: usize,
    pub(crate) strategy: ScrollStrategy,
}

#[derive(Default, Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum ScrollStrategy {
    #[default]
    FirstVisible,
    Top,
}

/// A handle to the scrollable aspects of an element.
#[derive(Clone, Debug, Default)]
pub struct ScrollHandle(pub(crate) Rc<RefCell<ScrollHandleState>>);

fn visible_item_index(state: &ScrollHandleState, target: Pixels) -> usize {
    match state.child_bounds.binary_search_by(|bounds| {
        if target < bounds.top() {
            Ordering::Greater
        } else if target > bounds.bottom() {
            Ordering::Less
        } else {
            Ordering::Equal
        }
    }) {
        Ok(index) => index,
        Err(index) => index.min(state.child_bounds.len().saturating_sub(1)),
    }
}

fn logical_scroll_position(state: &ScrollHandleState, use_bottom_edge: bool) -> (usize, Pixels) {
    let offset = state.offset.borrow().y;
    let viewport_edge = if use_bottom_edge { state.bounds.bottom() } else { state.bounds.top() };
    let index = visible_item_index(state, viewport_edge - offset);
    if let Some(child_bounds) = state.child_bounds.get(index) {
        let child_edge = if use_bottom_edge { child_bounds.bottom() } else { child_bounds.top() };
        (index, child_edge + offset - viewport_edge)
    } else {
        (index, px(0.))
    }
}

impl ScrollHandle {
    fn set_active_item(&self, index: usize, strategy: ScrollStrategy) {
        self.0.borrow_mut().active_item = Some(ScrollActiveItem { index, strategy });
    }

    /// Construct a new scroll handle.
    pub fn new() -> Self {
        Self(Rc::default())
    }

    /// Get the current scroll offset.
    pub fn offset(&self) -> Point<Pixels> {
        let state = self.0.borrow();
        let value = *state.offset.borrow();
        observe(&state.offset, ScrollReadKind::Offset, 0, value, |offset| *offset)
    }

    /// Get the maximum scroll offset.
    pub fn max_offset(&self) -> Size<Pixels> {
        let value = self.0.borrow().max_offset;
        observe(&self.0, ScrollReadKind::MaxOffset, 0, value, |state| state.max_offset)
    }

    /// Get the top child that's scrolled into view.
    pub fn top_item(&self) -> usize {
        let read = |state: &ScrollHandleState| {
            visible_item_index(state, state.bounds.top() - state.offset.borrow().y)
        };
        let value = read(&self.0.borrow());
        observe(&self.0, ScrollReadKind::TopItem, 0, value, read)
    }

    /// Get the bottom child that's scrolled into view.
    pub fn bottom_item(&self) -> usize {
        let read = |state: &ScrollHandleState| {
            visible_item_index(state, state.bounds.bottom() - state.offset.borrow().y)
        };
        let value = read(&self.0.borrow());
        observe(&self.0, ScrollReadKind::BottomItem, 0, value, read)
    }

    /// Return the bounds into which this child is painted.
    pub fn bounds(&self) -> Bounds<Pixels> {
        let value = self.0.borrow().bounds;
        observe(&self.0, ScrollReadKind::Bounds, 0, value, |state| state.bounds)
    }

    /// Get the bounds for a specific child.
    pub fn bounds_for_item(&self, ix: usize) -> Option<Bounds<Pixels>> {
        let value = self.0.borrow().child_bounds.get(ix).copied();
        observe(&self.0, ScrollReadKind::ItemBounds, ix, value, move |state| state.child_bounds.get(ix).copied())
    }

    /// Update the active item for scrolling to during prepaint.
    pub fn scroll_to_item(&self, ix: usize) {
        self.set_active_item(ix, ScrollStrategy::default());
    }

    /// Scroll so that the item becomes the first visible element.
    pub fn scroll_to_top_of_item(&self, ix: usize) {
        self.set_active_item(ix, ScrollStrategy::Top);
    }

    /// Scroll the active item into view according to the current strategy.
    pub(crate) fn scroll_to_active_item(&self) {
        let mut state = self.0.borrow_mut();

        let Some(active_item) = state.active_item else {
            return;
        };

        let active_item = match state.child_bounds.get(active_item.index) {
            Some(bounds) => {
                let mut scroll_offset = state.offset.borrow_mut();

                match active_item.strategy {
                    ScrollStrategy::FirstVisible => {
                        if state.overflow.y == Overflow::Scroll {
                            if bounds.top() + scroll_offset.y < state.bounds.top() {
                                scroll_offset.y = state.bounds.top() - bounds.top();
                            } else if bounds.bottom() + scroll_offset.y > state.bounds.bottom() {
                                scroll_offset.y = state.bounds.bottom() - bounds.bottom();
                            }
                        }
                    }
                    ScrollStrategy::Top => {
                        scroll_offset.y = state.bounds.top() - bounds.top();
                    }
                }

                if state.overflow.x == Overflow::Scroll {
                    if bounds.left() + scroll_offset.x < state.bounds.left() {
                        scroll_offset.x = state.bounds.left() - bounds.left();
                    } else if bounds.right() + scroll_offset.x > state.bounds.right() {
                        scroll_offset.x = state.bounds.right() - bounds.right();
                    }
                }
                None
            }
            None => Some(active_item),
        };
        state.active_item = active_item;
    }

    /// Scrolls to the bottom.
    pub fn scroll_to_bottom(&self) {
        let mut state = self.0.borrow_mut();
        state.scroll_to_bottom = true;
    }

    /// Set the offset explicitly. Further scrolling down makes the offset more negative.
    pub fn set_offset(&self, position: Point<Pixels>) {
        let state = self.0.borrow();
        *state.offset.borrow_mut() = position;
    }

    /// Get the logical scroll top, based on a child index and a pixel offset.
    pub fn logical_scroll_top(&self) -> (usize, Pixels) {
        let read = |state: &ScrollHandleState| logical_scroll_position(state, false);
        let value = read(&self.0.borrow());
        observe(&self.0, ScrollReadKind::LogicalTop, 0, value, read)
    }

    /// Get the logical scroll bottom, based on a child index and a pixel offset.
    pub fn logical_scroll_bottom(&self) -> (usize, Pixels) {
        let read = |state: &ScrollHandleState| logical_scroll_position(state, true);
        let value = read(&self.0.borrow());
        observe(&self.0, ScrollReadKind::LogicalBottom, 0, value, read)
    }

    /// Get the number of tracked scrollable children.
    pub fn children_count(&self) -> usize {
        let value = self.0.borrow().child_bounds.len();
        observe(&self.0, ScrollReadKind::ChildCount, 0, value, |state| state.child_bounds.len())
    }
}

impl Interactivity {
    pub(crate) fn ensure_focus_handle(
        &mut self,
        element_state: &mut InteractiveElementState,
        cx: &mut App,
    ) {
        let focus_handle = match &mut element_state.focus_handle {
            Some(handle) => handle.clone(),
            None => element_state.focus_handle.insert(cx.focus_handle()).clone(),
        };
        let mut handle = focus_handle.tab_stop(self.tab_stop);

        if let Some(index) = self.tab_index {
            handle = handle.tab_index(index);
        }

        self.tracked_focus_handle = Some(handle);
    }

    pub(crate) fn resolve_scroll_offset(
        &mut self,
        element_state: Option<&mut InteractiveElementState>,
    ) {
        if let Some(scroll_handle) = self.tracked_scroll_handle.as_ref() {
            let state = scroll_handle.0.borrow();
            self.scroll_offset = Some(state.offset.clone());
            self.ongoing_scroll = Some(state.ongoing_scroll.clone());
        } else if (self.base_style.overflow.x == Some(Overflow::Scroll)
            || self.base_style.overflow.y == Some(Overflow::Scroll))
            && let Some(element_state) = element_state
        {
            self.scroll_offset = Some(element_state.ensure_scroll_offset());
            self.ongoing_scroll = Some(element_state.ensure_ongoing_scroll());
        }
    }

    /// Use the exact same logical bounds and snapped padding for prepaint
    /// clamping and wheel admission (the hitbox may be visually transformed).
    fn scroll_max(&self, bounds: Bounds<Pixels>, style: &crate::Style, window: &Window) -> Size<Pixels> {
        let padding = style.padding.to_pixels(bounds.size.into(), window.rem_size())
            .map(|edge| crate::layout::snap_logical_to_device_pixel(*edge, window.scale_factor()));
        let padding_size = size(padding.left + padding.right, padding.top + padding.bottom);
        (self.content_size + padding_size - bounds.size)
            .map(|pixels| (pixels * 100.0).round() / 100.0)
            .max(&Default::default())
    }

    pub(crate) fn clamp_scroll_position(
        &self,
        bounds: Bounds<Pixels>,
        style: &crate::Style,
        window: &mut Window,
        cx: &mut App,
    ) -> Point<Pixels> {

        if let Some(scroll_offset) = self.scroll_offset.as_ref() {
            let mut scroll_to_bottom = false;
            let mut tracked_scroll_handle = self
                .tracked_scroll_handle
                .as_ref()
                .map(|handle| handle.0.borrow_mut());
            if let Some(mut scroll_handle_state) = tracked_scroll_handle.as_deref_mut() {
                scroll_handle_state.overflow = style.overflow;
                scroll_to_bottom = mem::take(&mut scroll_handle_state.scroll_to_bottom);
            }

            let scroll_max = self.scroll_max(bounds, style, window);
            let mut scroll_offset = scroll_offset.borrow_mut();

            scroll_offset.x = scroll_offset.x.clamp(-scroll_max.width, px(0.));
            scroll_offset.y = if scroll_to_bottom {
                -scroll_max.height
            } else {
                scroll_offset.y.clamp(-scroll_max.height, px(0.))
            };

            let mut viewport_size_changed = false;
            if let Some(mut scroll_handle_state) = tracked_scroll_handle {
                viewport_size_changed = scroll_handle_state.bounds.size != bounds.size;
                scroll_handle_state.max_offset = scroll_max;
                scroll_handle_state.bounds = bounds;
            }

            let resolved_offset = *scroll_offset;
            drop(scroll_offset);
            if viewport_size_changed {
                // Render may have selected virtual rows using the previous viewport. A notify
                // during prepaint is consumed by this draw, so invalidate after it commits.
                let view = window.current_view();
                window.defer(cx, move |window, _| window.invalidate_native_scroll(view));
            }
            resolved_offset
        } else {
            Point::default()
        }
    }

    pub(crate) fn paint_scroll_listener(
        &self,
        bounds: Bounds<Pixels>,
        hitbox: &Hitbox,
        style: &crate::Style,
        window: &mut Window,
        _cx: &mut App,
    ) {
        if let Some(scroll_offset) = self.scroll_offset.clone() {
            let ongoing_scroll = self.ongoing_scroll.clone();
            let overflow = style.overflow;
            let allow_concurrent_scroll = style.allow_concurrent_scroll;
            let restrict_scroll_to_axis = style.restrict_scroll_to_axis;
            let line_height = window.line_height();
            let scroll_max = self.tracked_scroll_handle.as_ref()
                .map(|handle| handle.0.borrow().max_offset)
                .unwrap_or_else(|| self.scroll_max(bounds, style, window));
            let hitbox = hitbox.clone();
            let current_view = window.current_view();
            if let Some(handle) = self.tracked_scroll_handle.as_ref() {
                window.watch_mounted_scroll(current_view, &handle.0, |state| {
                    (*state.offset.borrow(), state.scroll_to_bottom, state.active_item)
                });
            } else {
                window.watch_mounted_scroll(current_view, &scroll_offset, |offset| *offset);
            }
            window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, _cx| {
                if phase == DispatchPhase::Bubble && hitbox.should_handle_scroll(window) {
                    let mut scroll_offset = scroll_offset.borrow_mut();
                    let old_scroll_offset = *scroll_offset;
                    let mut delta = event.delta.pixel_delta(line_height);

                    if restrict_scroll_to_axis
                        && event.delta.precise()
                        && let Some(ongoing_scroll) = &ongoing_scroll
                    {
                        ongoing_scroll
                            .borrow_mut()
                            .filter(&mut delta, event.touch_phase);
                    }

                    let mut delta_x = Pixels::ZERO;
                    if overflow.x == Overflow::Scroll {
                        if !delta.x.is_zero() {
                            delta_x = delta.x;
                        } else if !restrict_scroll_to_axis && overflow.y != Overflow::Scroll {
                            delta_x = delta.y;
                        }
                    }
                    let mut delta_y = Pixels::ZERO;
                    if overflow.y == Overflow::Scroll {
                        if !delta.y.is_zero() {
                            delta_y = delta.y;
                        } else if !restrict_scroll_to_axis && overflow.x != Overflow::Scroll {
                            delta_y = delta.x;
                        }
                    }
                    if !allow_concurrent_scroll && !delta_x.is_zero() && !delta_y.is_zero() {
                        if delta_x.abs() > delta_y.abs() {
                            delta_y = Pixels::ZERO;
                        } else {
                            delta_x = Pixels::ZERO;
                        }
                    }
                    // Adapt GPUI Fast's wheel admission: a wheel at an end
                    // of the scroll range cannot move content and must not
                    // invalidate the whole owning view.
                    let moves = |offset: Pixels, delta: Pixels, max: Pixels| {
                        !delta.is_zero() && (offset + delta).clamp(-max, px(0.)) != offset
                    };
                    if !moves(old_scroll_offset.x, delta_x, scroll_max.width)
                        && !moves(old_scroll_offset.y, delta_y, scroll_max.height)
                    {
                        return;
                    }
                    scroll_offset.y = (old_scroll_offset.y + delta_y).clamp(-scroll_max.height, px(0.));
                    scroll_offset.x = (old_scroll_offset.x + delta_x).clamp(-scroll_max.width, px(0.));
                    let changed = *scroll_offset != old_scroll_offset;
                    drop(scroll_offset);
                    if changed {
                        window.invalidate_native_scroll(current_view);
                    }
                }
            });
        }
    }
}
