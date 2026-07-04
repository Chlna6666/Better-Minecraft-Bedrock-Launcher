use crate::{
    InteractiveElement, ListHorizontalSizingBehavior, ListSizingBehavior, Overflow, Pixels, point,
};

use super::{UniformList, UniformListDecoration, UniformListScrollHandle};

impl UniformList {
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
        self.interactivity.tracked_scroll_handle = Some(handle.0.borrow().base_handle.clone());
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
                    scroll_state.y_flipped = is_y_flipped;
                }
                None if is_y_flipped => {
                    base_handle.set_offset(point(offset.x, Pixels::MIN));
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
