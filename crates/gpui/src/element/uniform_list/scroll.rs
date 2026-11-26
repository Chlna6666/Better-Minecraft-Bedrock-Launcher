use crate::{Pixels, ScrollHandle, Size};
use std::{cell::RefCell, rc::Rc};
use crate::window::scroll_dependencies::StateVersion;

/// A handle for controlling the scroll position of a uniform list.
/// This should be stored in your view and passed to the uniform list on each frame.
#[derive(Clone, Debug, Default)]
pub struct UniformListScrollHandle(pub(super) Rc<RefCell<UniformListScroll>>);

/// Where to place the element scrolled to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollStrategy {
    /// Place the element at the top of the list's viewport.
    Top,
    /// Attempt to place the element in the middle of the list's viewport.
    /// May not be possible if there's not enough list items above the item scrolled to:
    /// in this case, the element will be placed at the closest possible position.
    Center,
    /// Attempt to place the element at the bottom of the list's viewport.
    /// May not be possible if there's not enough list items above the item scrolled to:
    /// in this case, the element will be placed at the closest possible position.
    Bottom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DeferredScrollToItem {
    /// The item index to scroll to
    pub item_index: usize,
    /// The scroll strategy to use
    pub strategy: ScrollStrategy,
    /// The offset in number of items
    pub offset: usize,
    pub scroll_strict: bool,
}

#[derive(Clone, Debug, Default)]
pub(super) struct UniformListScroll {
    pub(super) version: StateVersion,
    pub(super) base_handle: ScrollHandle,
    pub(super) deferred_scroll_to_item: Option<DeferredScrollToItem>,
    /// Size of the item, captured during last layout.
    pub(super) last_item_size: Option<ItemSize>,
    /// Whether the list was vertically flipped during last layout.
    pub(super) y_flipped: bool,
}

#[derive(Copy, Clone, Debug, Default, PartialEq)]
/// The size of the item and its contents.
pub struct ItemSize {
    /// The size of the item.
    pub item: Size<Pixels>,
    /// The size of the item's contents, which may be larger than the item itself,
    /// if the item was bounded by a parent element.
    pub contents: Size<Pixels>,
}

impl UniformListScrollHandle {
    /// Create a new scroll handle to bind to a uniform list.
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind an existing scroll handle, including a separately rendered scrollbar.
    pub fn with_base_handle(base_handle: ScrollHandle) -> Self {
        Self(Rc::new(RefCell::new(UniformListScroll { base_handle, ..Default::default() })))
    }

    /// The shared native scroll handle.
    pub fn base_handle(&self) -> ScrollHandle {
        self.0.borrow().base_handle.clone()
    }

    /// Last measured item and viewport sizes.
    pub fn last_item_size(&self) -> Option<ItemSize> {
        let state = self.0.borrow();
        state.version.read();
        state.last_item_size
    }

    fn defer_scroll(&self, next: DeferredScrollToItem) {
        let mut state = self.0.borrow_mut();
        let changed = state.deferred_scroll_to_item != Some(next);
        state.version.bump_if(changed);
        state.base_handle.0.borrow().version.bump_if(changed);
        state.deferred_scroll_to_item = Some(next);
    }

    /// Scroll the list so that the given item index is visible.
    ///
    /// This uses non-strict scrolling: if the item is already fully visible, no scrolling occurs.
    /// If the item is out of view, it scrolls the minimum amount to bring it into view according
    /// to the strategy.
    pub fn scroll_to_item(&self, ix: usize, strategy: ScrollStrategy) {
        self.defer_scroll(DeferredScrollToItem {
            item_index: ix,
            strategy,
            offset: 0,
            scroll_strict: false,
        });
    }

    /// Scroll the list so that the given item index is at scroll strategy position.
    ///
    /// This uses strict scrolling: the item will always be scrolled to match the strategy position,
    /// even if it's already visible. Use this when you need precise positioning.
    pub fn scroll_to_item_strict(&self, ix: usize, strategy: ScrollStrategy) {
        self.defer_scroll(DeferredScrollToItem {
            item_index: ix,
            strategy,
            offset: 0,
            scroll_strict: true,
        });
    }

    /// Scroll the list to the given item index with an offset in number of items.
    ///
    /// This uses non-strict scrolling: if the item is already visible within the offset region,
    /// no scrolling occurs.
    ///
    /// The offset parameter shrinks the effective viewport by the specified number of items
    /// from the corresponding edge, then applies the scroll strategy within that reduced viewport:
    /// - `ScrollStrategy::Top`: Shrinks from top, positions item at the new top
    /// - `ScrollStrategy::Center`: Shrinks from top, centers item in the reduced viewport
    /// - `ScrollStrategy::Bottom`: Shrinks from bottom, positions item at the new bottom
    pub fn scroll_to_item_with_offset(&self, ix: usize, strategy: ScrollStrategy, offset: usize) {
        self.defer_scroll(DeferredScrollToItem {
            item_index: ix,
            strategy,
            offset,
            scroll_strict: false,
        });
    }

    /// Scroll the list so that the given item index is at the exact scroll strategy position with an offset.
    ///
    /// This uses strict scrolling: the item will always be scrolled to match the strategy position,
    /// even if it's already visible.
    ///
    /// The offset parameter shrinks the effective viewport by the specified number of items
    /// from the corresponding edge, then applies the scroll strategy within that reduced viewport:
    /// - `ScrollStrategy::Top`: Shrinks from top, positions item at the new top
    /// - `ScrollStrategy::Center`: Shrinks from top, centers item in the reduced viewport
    /// - `ScrollStrategy::Bottom`: Shrinks from bottom, positions item at the new bottom
    pub fn scroll_to_item_exact(&self, ix: usize, strategy: ScrollStrategy, offset: usize) {
        self.defer_scroll(DeferredScrollToItem {
            item_index: ix,
            strategy,
            offset,
            scroll_strict: true,
        });
    }

    /// Check if the list is flipped vertically.
    pub fn y_flipped(&self) -> bool {
        let state = self.0.borrow();
        state.version.read();
        state.y_flipped
    }

    /// Get the index of the topmost visible child.
    #[cfg(any(test, feature = "test-support"))]
    pub fn logical_scroll_top_index(&self) -> usize {
        let this = self.0.borrow();
        this.version.read();
        this.deferred_scroll_to_item
            .as_ref()
            .map(|deferred| deferred.item_index)
            .unwrap_or_else(|| this.base_handle.logical_scroll_top().0)
    }

    /// Checks if the list can be scrolled vertically.
    pub fn is_scrollable(&self) -> bool {
        let state = self.0.borrow();
        state.version.read();
        state.last_item_size.is_some_and(|size| size.contents.height > size.item.height)
    }
}
