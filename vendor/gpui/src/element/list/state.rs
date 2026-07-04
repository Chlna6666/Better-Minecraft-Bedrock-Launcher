mod item;
mod layout;

use crate::{App, Bounds, Edges, EntityId, FocusHandle, Pixels, Point, Size, Window, point, px};
use collections::VecDeque;
use std::{cell::RefCell, ops::Range, rc::Rc};
use sum_tree::{Bias, Dimensions, SumTree};

pub(super) use item::{ListItem, ListItemSummary};

use super::{
    layout::ItemLayout,
    tree::{Count, Height},
    types::{ListAlignment, ListMeasuringBehavior, ListOffset, ListScrollEvent},
};

/// The list state that views must hold on behalf of the list element.
#[derive(Clone)]
pub struct ListState(pub(super) Rc<RefCell<StateInner>>);

impl std::fmt::Debug for ListState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ListState")
    }
}

pub(super) struct StateInner {
    pub(super) last_layout_bounds: Option<Bounds<Pixels>>,
    pub(super) last_padding: Option<Edges<Pixels>>,
    pub(super) items: SumTree<ListItem>,
    pub(super) logical_scroll_top: Option<ListOffset>,
    alignment: ListAlignment,
    pub(super) overdraw: Pixels,
    pub(super) reset: bool,
    #[allow(clippy::type_complexity)]
    scroll_handler: Option<Box<dyn FnMut(&ListScrollEvent, &mut Window, &mut App)>>,
    scrollbar_drag_start_height: Option<Pixels>,
    measuring_behavior: ListMeasuringBehavior,
    measured_items_scratch: VecDeque<ListItem>,
    item_layouts_scratch: VecDeque<ItemLayout>,
}

impl ListState {
    /// Construct a new list state, for storage on a view.
    ///
    /// The overdraw parameter controls how much extra space is rendered
    /// above and below the visible area. Elements within this area will
    /// be measured even though they are not visible. This can help ensure
    /// that the list doesn't flicker or pop in when scrolling.
    pub fn new(item_count: usize, alignment: ListAlignment, overdraw: Pixels) -> Self {
        let this = Self(Rc::new(RefCell::new(StateInner {
            last_layout_bounds: None,
            last_padding: None,
            items: SumTree::default(),
            logical_scroll_top: None,
            alignment,
            overdraw,
            scroll_handler: None,
            reset: false,
            scrollbar_drag_start_height: None,
            measuring_behavior: ListMeasuringBehavior::default(),
            measured_items_scratch: VecDeque::new(),
            item_layouts_scratch: VecDeque::new(),
        })));
        this.splice(0..0, item_count);
        this
    }

    /// Set the list to measure all items in the list in the first layout phase.
    ///
    /// This is useful for ensuring that the scrollbar size is correct instead of based on only rendered elements.
    pub fn measure_all(self) -> Self {
        self.0.borrow_mut().measuring_behavior = ListMeasuringBehavior::Measure(false);
        self
    }

    /// Reset this instantiation of the list state.
    ///
    /// Note that this will cause scroll events to be dropped until the next paint.
    pub fn reset(&self, element_count: usize) {
        let old_count = {
            let state = &mut *self.0.borrow_mut();
            state.reset = true;
            state.measuring_behavior.reset();
            state.logical_scroll_top = None;
            state.scrollbar_drag_start_height = None;
            state.items.summary().count
        };

        self.splice(0..old_count, element_count);
    }

    /// The number of items in this list.
    pub fn item_count(&self) -> usize {
        self.0.borrow().items.summary().count
    }

    /// Inform the list state that the items in `old_range` have been replaced
    /// by `count` new items that must be recalculated.
    pub fn splice(&self, old_range: Range<usize>, count: usize) {
        self.splice_focusable(old_range, (0..count).map(|_| None))
    }

    /// Register with the list state that the items in `old_range` have been replaced
    /// by new items. As opposed to [`Self::splice`], this method allows an iterator of optional focus handles
    /// to be supplied to properly integrate with items in the list that can be focused. If a focused item
    /// is scrolled out of view, the list will continue to render it to allow keyboard interaction.
    pub fn splice_focusable(
        &self,
        old_range: Range<usize>,
        focus_handles: impl IntoIterator<Item = Option<FocusHandle>>,
    ) {
        let state = &mut *self.0.borrow_mut();

        let mut old_items = state.items.cursor::<Count>(());
        let mut new_items = old_items.slice(&Count(old_range.start), Bias::Right);
        old_items.seek_forward(&Count(old_range.end), Bias::Right);

        let mut spliced_count = 0;
        new_items.extend(
            focus_handles.into_iter().map(|focus_handle| {
                spliced_count += 1;
                ListItem::Unmeasured { focus_handle }
            }),
            (),
        );
        new_items.append(old_items.suffix(), ());
        drop(old_items);
        state.items = new_items;

        if let Some(ListOffset {
            item_ix,
            offset_in_item,
        }) = state.logical_scroll_top.as_mut()
        {
            if old_range.contains(item_ix) {
                *item_ix = old_range.start;
                *offset_in_item = px(0.);
            } else if old_range.end <= *item_ix {
                *item_ix = *item_ix - (old_range.end - old_range.start) + spliced_count;
            }
        }
    }

    /// Set a handler that will be called when the list is scrolled.
    pub fn set_scroll_handler(
        &self,
        handler: impl FnMut(&ListScrollEvent, &mut Window, &mut App) + 'static,
    ) {
        self.0.borrow_mut().scroll_handler = Some(Box::new(handler))
    }

    /// Get the current scroll offset, in terms of the list's items.
    pub fn logical_scroll_top(&self) -> ListOffset {
        self.0.borrow().logical_scroll_top()
    }

    /// Scroll the list by the given offset
    pub fn scroll_by(&self, distance: Pixels) {
        if distance == px(0.) {
            return;
        }

        let current_offset = self.logical_scroll_top();
        let state = &mut *self.0.borrow_mut();
        let mut cursor = state.items.cursor::<ListItemSummary>(());
        cursor.seek(&Count(current_offset.item_ix), Bias::Right);

        let start_pixel_offset = cursor.start().height + current_offset.offset_in_item;
        let new_pixel_offset = (start_pixel_offset + distance).max(px(0.));
        if new_pixel_offset > start_pixel_offset {
            cursor.seek_forward(&Height(new_pixel_offset), Bias::Right);
        } else {
            cursor.seek(&Height(new_pixel_offset), Bias::Right);
        }

        state.logical_scroll_top = Some(ListOffset {
            item_ix: cursor.start().count,
            offset_in_item: new_pixel_offset - cursor.start().height,
        });
    }

    /// Scroll the list to the given offset
    pub fn scroll_to(&self, mut scroll_top: ListOffset) {
        let state = &mut *self.0.borrow_mut();
        let item_count = state.items.summary().count;
        if scroll_top.item_ix >= item_count {
            scroll_top.item_ix = item_count;
            scroll_top.offset_in_item = px(0.);
        }

        state.logical_scroll_top = Some(scroll_top);
    }

    /// Scroll the list to the given item, such that the item is fully visible.
    pub fn scroll_to_reveal_item(&self, ix: usize) {
        let state = &mut *self.0.borrow_mut();

        let mut scroll_top = state.logical_scroll_top();
        let height = state
            .last_layout_bounds
            .map_or(px(0.), |bounds| bounds.size.height);
        let padding = state.last_padding.unwrap_or_default();

        if ix <= scroll_top.item_ix {
            scroll_top.item_ix = ix;
            scroll_top.offset_in_item = px(0.);
        } else {
            let mut cursor = state.items.cursor::<ListItemSummary>(());
            cursor.seek(&Count(ix + 1), Bias::Right);
            let bottom = cursor.start().height + padding.top;
            let goal_top = px(0.).max(bottom - height + padding.bottom);

            cursor.seek(&Height(goal_top), Bias::Left);
            let start_ix = cursor.start().count;
            let start_item_top = cursor.start().height;

            if start_ix >= scroll_top.item_ix {
                scroll_top.item_ix = start_ix;
                scroll_top.offset_in_item = goal_top - start_item_top;
            }
        }

        state.logical_scroll_top = Some(scroll_top);
    }

    /// Get the bounds for the given item in window coordinates, if it's
    /// been rendered.
    pub fn bounds_for_item(&self, ix: usize) -> Option<Bounds<Pixels>> {
        let state = &*self.0.borrow();

        let bounds = state.last_layout_bounds.unwrap_or_default();
        let scroll_top = state.logical_scroll_top();
        if ix < scroll_top.item_ix {
            return None;
        }

        let mut cursor = state.items.cursor::<Dimensions<Count, Height>>(());
        cursor.seek(&Count(scroll_top.item_ix), Bias::Right);

        let scroll_top = cursor.start().1.0 + scroll_top.offset_in_item;

        cursor.seek_forward(&Count(ix), Bias::Right);
        if let Some(&ListItem::Measured { size, .. }) = cursor.item() {
            let &Dimensions(Count(count), Height(top), _) = cursor.start();
            if count == ix {
                let top = bounds.top() + top - scroll_top;
                return Some(Bounds::from_corners(
                    point(bounds.left(), top),
                    point(bounds.right(), top + size.height),
                ));
            }
        }
        None
    }

    /// Call this method when the user starts dragging the scrollbar.
    ///
    /// This will prevent the height reported to the scrollbar from changing during the drag
    /// as items in the overdraw get measured, and help offset scroll position changes accordingly.
    pub fn scrollbar_drag_started(&self) {
        let mut state = self.0.borrow_mut();
        state.scrollbar_drag_start_height = Some(state.items.summary().height);
    }

    /// Called when the user stops dragging the scrollbar.
    ///
    /// See `scrollbar_drag_started`.
    pub fn scrollbar_drag_ended(&self) {
        self.0.borrow_mut().scrollbar_drag_start_height.take();
    }

    /// Set the offset from the scrollbar
    pub fn set_offset_from_scrollbar(&self, point: Point<Pixels>) {
        self.0.borrow_mut().set_offset_from_scrollbar(point);
    }

    /// Returns the maximum scroll offset according to the items we have measured.
    /// This value remains constant while dragging to prevent the scrollbar from moving away unexpectedly.
    pub fn max_offset_for_scrollbar(&self) -> Size<Pixels> {
        let state = self.0.borrow();
        let bounds = state.last_layout_bounds.unwrap_or_default();

        let height = state
            .scrollbar_drag_start_height
            .unwrap_or_else(|| state.items.summary().height);

        Size::new(Pixels::ZERO, Pixels::ZERO.max(height - bounds.size.height))
    }

    /// Returns the current scroll offset adjusted for the scrollbar
    pub fn scroll_px_offset_for_scrollbar(&self) -> Point<Pixels> {
        let state = &self.0.borrow();
        let logical_scroll_top = state.logical_scroll_top();

        let mut cursor = state.items.cursor::<ListItemSummary>(());
        let summary: ListItemSummary =
            cursor.summary(&Count(logical_scroll_top.item_ix), Bias::Right);
        let content_height = state.items.summary().height;
        let drag_offset =
            // if dragging the scrollbar, we want to offset the point if the height changed
            content_height - state.scrollbar_drag_start_height.unwrap_or(content_height);
        let offset = summary.height + logical_scroll_top.offset_in_item - drag_offset;

        Point::new(px(0.), -offset)
    }

    /// Return the bounds of the viewport in pixels.
    pub fn viewport_bounds(&self) -> Bounds<Pixels> {
        self.0.borrow().last_layout_bounds.unwrap_or_default()
    }
}

impl StateInner {
    pub(super) fn recycle_item_layouts(&mut self, mut item_layouts: VecDeque<ItemLayout>) {
        item_layouts.clear();
        self.item_layouts_scratch = item_layouts;
    }

    fn visible_range(&self, height: Pixels, scroll_top: &ListOffset) -> Range<usize> {
        let mut cursor = self.items.cursor::<ListItemSummary>(());
        cursor.seek(&Count(scroll_top.item_ix), Bias::Right);
        let start_y = cursor.start().height + scroll_top.offset_in_item;
        cursor.seek_forward(&Height(start_y + height), Bias::Left);
        scroll_top.item_ix..cursor.start().count + 1
    }

    pub(super) fn scroll(
        &mut self,
        scroll_top: &ListOffset,
        height: Pixels,
        delta: Point<Pixels>,
        current_view: EntityId,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(event) = self.apply_scroll(scroll_top, height, delta) else {
            return;
        };

        if let Some(scroll_handler) = &mut self.scroll_handler {
            scroll_handler(&event, window, cx);
        }

        cx.notify(current_view);
    }

    #[cfg(test)]
    pub(in crate::element::list) fn scroll_for_test(
        &mut self,
        scroll_top: &ListOffset,
        height: Pixels,
        delta: Point<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(event) = self.apply_scroll(scroll_top, height, delta) else {
            return;
        };

        if let Some(scroll_handler) = &mut self.scroll_handler {
            scroll_handler(&event, window, cx);
        }
    }

    fn apply_scroll(
        &mut self,
        scroll_top: &ListOffset,
        height: Pixels,
        delta: Point<Pixels>,
    ) -> Option<ListScrollEvent> {
        // Drop scroll events after a reset, since we can't calculate
        // the new logical scroll top without the item heights
        if self.reset {
            return None;
        }

        if delta.y == px(0.) {
            return None;
        }

        let padding = self.last_padding.unwrap_or_default();
        let scroll_max =
            (self.items.summary().height + padding.top + padding.bottom - height).max(px(0.));
        let old_scroll_top = self.scroll_top(scroll_top);
        let new_scroll_top = (old_scroll_top - delta.y).max(px(0.)).min(scroll_max);
        if new_scroll_top == old_scroll_top {
            return None;
        }

        let (start, ..) =
            self.items
                .find::<ListItemSummary, _>((), &Height(new_scroll_top), Bias::Right);
        let event_scroll_top = ListOffset {
            item_ix: start.count,
            offset_in_item: new_scroll_top - start.height,
        };
        if self.alignment == ListAlignment::Bottom && new_scroll_top == scroll_max {
            self.logical_scroll_top = None;
        } else {
            self.logical_scroll_top = Some(event_scroll_top);
        }

        let visible_range = self.visible_range(height, &event_scroll_top);
        Some(ListScrollEvent {
            visible_range,
            count: self.items.summary().count,
            is_scrolled: self.logical_scroll_top.is_some(),
        })
    }

    fn logical_scroll_top(&self) -> ListOffset {
        self.logical_scroll_top
            .unwrap_or_else(|| match self.alignment {
                ListAlignment::Top => ListOffset {
                    item_ix: 0,
                    offset_in_item: px(0.),
                },
                ListAlignment::Bottom => ListOffset {
                    item_ix: self.items.summary().count,
                    offset_in_item: px(0.),
                },
            })
    }

    fn scroll_top(&self, logical_scroll_top: &ListOffset) -> Pixels {
        let (start, ..) = self.items.find::<ListItemSummary, _>(
            (),
            &Count(logical_scroll_top.item_ix),
            Bias::Right,
        );
        start.height + logical_scroll_top.offset_in_item
    }

    // Scrollbar support

    fn set_offset_from_scrollbar(&mut self, point: Point<Pixels>) {
        let Some(bounds) = self.last_layout_bounds else {
            return;
        };
        let height = bounds.size.height;

        let padding = self.last_padding.unwrap_or_default();
        let content_height = self.items.summary().height;
        let scroll_max = (content_height + padding.top + padding.bottom - height).max(px(0.));
        let drag_offset =
            // if dragging the scrollbar, we want to offset the point if the height changed
            content_height - self.scrollbar_drag_start_height.unwrap_or(content_height);
        let new_scroll_top = (point.y - drag_offset).abs().max(px(0.)).min(scroll_max);

        if self.alignment == ListAlignment::Bottom && new_scroll_top == scroll_max {
            self.logical_scroll_top = None;
        } else {
            let (start, _, _) =
                self.items
                    .find::<ListItemSummary, _>((), &Height(new_scroll_top), Bias::Right);

            let item_ix = start.count;
            let offset_in_item = new_scroll_top - start.height;
            self.logical_scroll_top = Some(ListOffset {
                item_ix,
                offset_in_item,
            });
        }
    }
}
