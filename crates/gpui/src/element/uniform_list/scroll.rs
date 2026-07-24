use crate::{Pixels, ScrollHandle, Size};
use std::{cell::RefCell, rc::Rc};

/// A handle for controlling the scroll position of a uniform list.
/// This should be stored in your view and passed to the uniform list on each frame.
#[derive(Clone, Debug, Default)]
pub struct UniformListScrollHandle(pub Rc<RefCell<UniformListScrollState>>);

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

#[derive(Clone, Copy, Debug)]
#[allow(missing_docs)]
pub struct DeferredScrollToItem {
    /// The item index to scroll to
    pub item_index: usize,
    /// The scroll strategy to use
    pub strategy: ScrollStrategy,
    /// The offset in number of items
    pub offset: usize,
    pub scroll_strict: bool,
}

#[derive(Clone, Debug, Default)]
#[allow(missing_docs)]
pub struct UniformListScrollState {
    pub base_handle: ScrollHandle,
    pub deferred_scroll_to_item: Option<DeferredScrollToItem>,
    /// Size of the item, captured during last layout.
    pub last_item_size: Option<ItemSize>,
    /// Whether the list was vertically flipped during last layout.
    pub y_flipped: bool,
}

#[derive(Copy, Clone, Debug, Default)]
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
        Self(Rc::new(RefCell::new(UniformListScrollState {
            base_handle: ScrollHandle::new(),
            deferred_scroll_to_item: None,
            last_item_size: None,
            y_flipped: false,
        })))
    }

    /// Scroll the list so that the given item index is visible.
    ///
    /// This uses non-strict scrolling: if the item is already fully visible, no scrolling occurs.
    /// If the item is out of view, it scrolls the minimum amount to bring it into view according
    /// to the strategy.
    pub fn scroll_to_item(&self, ix: usize, strategy: ScrollStrategy) {
        self.0.borrow_mut().deferred_scroll_to_item = Some(DeferredScrollToItem {
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
        self.0.borrow_mut().deferred_scroll_to_item = Some(DeferredScrollToItem {
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
        self.0.borrow_mut().deferred_scroll_to_item = Some(DeferredScrollToItem {
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
    pub fn scroll_to_item_strict_with_offset(
        &self,
        ix: usize,
        strategy: ScrollStrategy,
        offset: usize,
    ) {
        self.0.borrow_mut().deferred_scroll_to_item = Some(DeferredScrollToItem {
            item_index: ix,
            strategy,
            offset,
            scroll_strict: true,
        });
    }

    /// Check if the list is flipped vertically.
    pub fn y_flipped(&self) -> bool {
        self.0.borrow().y_flipped
    }

    /// Get the index of the topmost visible child.
    #[cfg(any(test, feature = "test-support"))]
    pub fn logical_scroll_top_index(&self) -> usize {
        let this = self.0.borrow();
        this.deferred_scroll_to_item
            .as_ref()
            .map(|deferred| deferred.item_index)
            .unwrap_or_else(|| this.base_handle.logical_scroll_top().0)
    }

    /// Checks if the list can be scrolled vertically.
    pub fn is_scrollable(&self) -> bool {
        if let Some(size) = self.0.borrow().last_item_size {
            size.contents.height > size.item.height
        } else {
            false
        }
    }
}
