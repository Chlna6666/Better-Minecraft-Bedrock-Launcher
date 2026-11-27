use crate::{
    InteractiveElement, ListHorizontalSizingBehavior, ListSizingBehavior, Overflow, Pixels, point,
};

use super::{UniformList, UniformListDecoration, UniformListScrollHandle};

impl UniformList {
    /// Opt in to row-local content retention across notifications of the
    /// list's own view. The revision must change whenever anything affecting
    /// that index's rendered content, layout, or callbacks changes, including
    /// untracked closure captures. Reordered/replaced rows must have different
    /// revisions unless their entire observable row is identical.
    ///
    /// The function must be pure, cheap and stable throughout a draw.
    /// Notify the owning view normally. Without this contract, ordinary
    /// owner invalidation remains conservative. Global refreshes, changed
    /// ancestor paths, input restrictions and changed inherited context
    /// still use the normal fallback.
    pub fn with_row_revision(mut self, revision: impl Fn(usize) -> u64 + 'static) -> Self {
        self.row_revision = Some(std::rc::Rc::new(revision));
        self
    }

    /// Uses an explicitly known row height (including its vertical spacing).
    ///
    /// For an Auto-sized, FitList viewport this avoids rendering a sample row
    /// solely to measure it. Infer sizing and horizontal scrolling still need
    /// intrinsic width, so they retain normal measurement. Nonpositive or
    /// nonfinite heights also fall back to measurement.
    pub fn with_item_height(mut self, height: Pixels) -> Self {
        self.fixed_item_height = (height.0.is_finite() && height.0 > 0.0).then_some(height);
        self
    }

    /// Selects a specific list item for measurement.
    pub fn with_width_from_item(mut self, item_index: Option<usize>) -> Self {
        self.item_to_measure_index = item_index.unwrap_or(0);
        self
    }

    /// Sets the sizing behavior, similar to the `List` element.
    pub fn with_sizing_behavior(mut self, behavior: ListSizingBehavior) -> Self {
        self.sizing_behavior = behavior;
        self
    }

    /// Sets the horizontal sizing behavior, controlling the way list items laid out horizontally.
    /// With [`ListHorizontalSizingBehavior::Unconstrained`] behavior, every item and the list itself will
    /// have the size of the widest item and lay out pushing the `end_slot` to the right end.
    pub fn with_horizontal_sizing_behavior(
        mut self,
        behavior: ListHorizontalSizingBehavior,
    ) -> Self {
        self.horizontal_sizing_behavior = behavior;
        match behavior {
            ListHorizontalSizingBehavior::FitList => {
                self.interactivity.base_style.overflow.x = None;
            }
            ListHorizontalSizingBehavior::Unconstrained => {
                self.interactivity.base_style.overflow.x = Some(Overflow::Scroll);
            }
        }
        self
    }

    /// Adds a decoration element to the list.
    pub fn with_decoration(mut self, decoration: impl UniformListDecoration + 'static) -> Self {
        self.decorations.push(Box::new(decoration));
        self
    }

    /// Track and render scroll state of this list with reference to the given scroll handle.
    pub fn track_scroll(mut self, handle: UniformListScrollHandle) -> Self {
        self.interactivity.tracked_scroll_handle = Some(handle.base_handle());
        self.scroll_handle = Some(handle);
        self
    }

    /// Sets whether the list is flipped vertically, such that item 0 appears at the bottom.
    pub fn y_flipped(mut self, is_y_flipped: bool) -> Self {
        if let Some(ref scroll_handle) = self.scroll_handle {
            let mut scroll_state = scroll_handle.0.borrow_mut();
            let base_handle = &scroll_state.base_handle;
            let offset = base_handle.offset();
            match scroll_state.last_item_size {
                Some(last_size) if scroll_state.y_flipped != is_y_flipped => {
                    let new_y_offset =
                        -(offset.y + last_size.contents.height - last_size.item.height);
                    base_handle.set_offset(point(offset.x, new_y_offset));
                    scroll_state.version.bump_if(scroll_state.y_flipped != is_y_flipped);
                    scroll_state.y_flipped = is_y_flipped;
                }
                None if is_y_flipped => {
                    base_handle.set_offset(point(offset.x, Pixels::MIN));
                    scroll_state.version.bump_if(scroll_state.y_flipped != is_y_flipped);
                    scroll_state.y_flipped = is_y_flipped;
                }
                _ => {}
            }
        }
        self
    }
}

impl InteractiveElement for UniformList {
    fn interactivity(&mut self) -> &mut crate::Interactivity {
        &mut self.interactivity
    }
}
