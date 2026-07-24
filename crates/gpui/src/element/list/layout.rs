use crate::{AnyElement, Hitbox, Pixels, Size};
use collections::VecDeque;

use super::types::ListOffset;

pub(super) struct LayoutItemsResponse {
    pub(super) max_item_width: Pixels,
    pub(super) scroll_top: ListOffset,
    pub(super) item_layouts: VecDeque<ItemLayout>,
}

pub(super) struct ItemLayout {
    pub(super) index: usize,
    pub(super) element: AnyElement,
    pub(super) size: Size<Pixels>,
}

/// Frame state used by the [List] element after layout.
pub struct ListPrepaintState {
    pub(super) hitbox: Hitbox,
    pub(super) layout: LayoutItemsResponse,
}
