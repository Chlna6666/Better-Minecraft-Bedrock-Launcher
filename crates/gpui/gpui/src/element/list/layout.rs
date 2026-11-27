use crate::{AnyElement, Hitbox, Pixels, Size};
use crate::window::scroll_rows::RowPlan;
use collections::VecDeque;

use super::configuration::ListOffset;

pub(super) struct LayoutItemsResponse {
    pub(super) max_item_width: Pixels,
    pub(super) scroll_top: ListOffset,
    pub(super) item_layouts: VecDeque<ItemLayout>,
    pub(super) row_plan: Option<RowPlan>,
}

pub(super) struct ItemLayout {
    pub(super) index: usize,
    pub(super) element: Option<AnyElement>,
    pub(super) size: Size<Pixels>,
    pub(super) capture_reads: Option<crate::RenderDependencies>,
}

/// Frame state used by the [List] element after layout.
pub struct ListPrepaintLayout {
    pub(super) hitbox: Hitbox,
    pub(super) layout: LayoutItemsResponse,
}
