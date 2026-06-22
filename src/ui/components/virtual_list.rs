use gpui::{Pixels, px};

#[derive(Clone, Copy, Debug, Default)]
pub struct VirtualListSlice {
    pub start_index: usize,
    pub end_index: usize,
}

impl VirtualListSlice {
    pub fn len(self) -> usize {
        self.end_index.saturating_sub(self.start_index)
    }

    pub fn contains(self, index: usize) -> bool {
        (self.start_index..self.end_index).contains(&index)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WindowedListSlice {
    pub start_index: usize,
    pub end_index: usize,
    pub top_spacer: Pixels,
    pub bottom_spacer: Pixels,
}

impl WindowedListSlice {
    pub fn visible_len(self) -> usize {
        self.end_index.saturating_sub(self.start_index)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct VirtualListPlan {
    pub render_slice: WindowedListSlice,
    pub visible_slice: VirtualListSlice,
    pub heavy_slice: VirtualListSlice,
}

pub fn compute_virtual_list_plan(
    total_items: usize,
    item_pitch_px: f32,
    scroll_offset_y: Pixels,
    viewport_height: Pixels,
    overscan: usize,
    max_heavy_items: usize,
) -> VirtualListPlan {
    if total_items == 0 || item_pitch_px <= 0.0 {
        return VirtualListPlan::default();
    }

    let viewport_height_px = (viewport_height / px(1.0)).max(item_pitch_px);
    let content_height_px = total_items as f32 * item_pitch_px;
    let max_scroll_top = (content_height_px - viewport_height_px).max(0.0);
    let scroll_top = (-(scroll_offset_y / px(1.0))).clamp(0.0, max_scroll_top);
    let visible_count = ((viewport_height_px / item_pitch_px).ceil() as usize).saturating_add(1);
    let visible_start =
        ((scroll_top / item_pitch_px).floor() as usize).min(total_items.saturating_sub(1));
    let visible_end = visible_start.saturating_add(visible_count).min(total_items);

    let render_start = visible_start.saturating_sub(overscan);
    let render_end = visible_end.saturating_add(overscan).min(total_items);

    let heavy_budget = visible_end
        .saturating_sub(visible_start)
        .max(1)
        .min(max_heavy_items.max(1));
    let heavy_start = visible_start;
    let heavy_end = visible_start.saturating_add(heavy_budget).min(visible_end);

    VirtualListPlan {
        render_slice: WindowedListSlice {
            start_index: render_start,
            end_index: render_end,
            top_spacer: px(render_start as f32 * item_pitch_px),
            bottom_spacer: px(total_items.saturating_sub(render_end) as f32 * item_pitch_px),
        },
        visible_slice: VirtualListSlice {
            start_index: visible_start,
            end_index: visible_end,
        },
        heavy_slice: VirtualListSlice {
            start_index: heavy_start,
            end_index: heavy_end,
        },
    }
}

pub fn compute_windowed_list_slice(
    total_items: usize,
    item_pitch_px: f32,
    scroll_offset_y: Pixels,
    viewport_height: Pixels,
    overscan: usize,
) -> WindowedListSlice {
    compute_virtual_list_plan(
        total_items,
        item_pitch_px,
        scroll_offset_y,
        viewport_height,
        overscan,
        usize::MAX,
    )
    .render_slice
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn virtual_list_clamps_overscrolled_short_list() {
        let plan = compute_virtual_list_plan(2, 68.0, px(-680.0), px(340.0), 8, 24);

        assert_eq!(plan.render_slice.start_index, 0);
        assert_eq!(plan.render_slice.end_index, 2);
        assert_eq!(plan.visible_slice.start_index, 0);
        assert_eq!(plan.visible_slice.end_index, 2);
        assert_eq!(plan.heavy_slice.start_index, 0);
        assert_eq!(plan.heavy_slice.end_index, 2);
        assert_eq!(plan.render_slice.top_spacer, px(0.0));
        assert_eq!(plan.render_slice.bottom_spacer, px(0.0));
    }
}
