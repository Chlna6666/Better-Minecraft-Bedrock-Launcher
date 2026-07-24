use crate::{AnyElement, App, Bounds, Entity, Pixels, Point, Window};
use std::ops::Range;

/// A decoration for a [`UniformList`](super::UniformList). This can be used for various things,
/// such as rendering indent guides, or other visual effects.
pub trait UniformListDecoration {
    /// Compute the decoration element, given the visible range of list items,
    /// the bounds of the list, and the height of each item.
    fn compute(
        &self,
        visible_range: Range<usize>,
        bounds: Bounds<Pixels>,
        scroll_offset: Point<Pixels>,
        item_height: Pixels,
        item_count: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement;
}

impl<T: UniformListDecoration + 'static> UniformListDecoration for Entity<T> {
    fn compute(
        &self,
        visible_range: Range<usize>,
        bounds: Bounds<Pixels>,
        scroll_offset: Point<Pixels>,
        item_height: Pixels,
        item_count: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        self.update(cx, |inner, cx| {
            inner.compute(
                visible_range,
                bounds,
                scroll_offset,
                item_height,
                item_count,
                window,
                cx,
            )
        })
    }
}
