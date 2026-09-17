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
    if total_items == 0 || !item_pitch_px.is_finite() || item_pitch_px <= 0.0 {
        return VirtualListPlan::default();
    }

    let measured_viewport_height_px = viewport_height / px(1.0);
    let viewport_is_measured =
        measured_viewport_height_px.is_finite() && measured_viewport_height_px > 0.0;

    // `ScrollHandle::bounds()` is frame-derived and may temporarily report a zero-sized viewport
    // while a retained subtree is being rebuilt. We still render a useful initial batch in that
    // frame, but that speculative batch height must never participate in scroll clamping. Doing so
    // rebases a valid non-zero scroll offset toward the top for one frame, materializes a different
    // virtual window, and then snaps back when the real viewport bounds return. With retained
    // rendering that presents as rows flashing, disappearing, or appearing to lose their data.
    let render_viewport_height_px = if viewport_is_measured {
        measured_viewport_height_px.max(item_pitch_px)
    } else {
        (item_pitch_px * 8.0).max(600.0)
    };
    let clamp_viewport_height_px = if viewport_is_measured {
        render_viewport_height_px
    } else {
        // Preserve the caller's logical scroll position until real bounds are available. One row is
        // the smallest useful viewport and therefore the least destructive clamp we can prove.
        item_pitch_px
    };

    let content_height_px = total_items as f32 * item_pitch_px;
    let max_scroll_top = (content_height_px - clamp_viewport_height_px).max(0.0);
    let requested_scroll_top = -(scroll_offset_y / px(1.0));
    let requested_scroll_top = if requested_scroll_top.is_finite() {
        requested_scroll_top
    } else {
        0.0
    };
    let scroll_top = requested_scroll_top.clamp(0.0, max_scroll_top);
    let visible_count =
        ((render_viewport_height_px / item_pitch_px).ceil() as usize).saturating_add(1);
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

    #[::core::prelude::v1::test]
    fn virtual_list_limits_heavy_slice_to_budget() {
        let plan = compute_virtual_list_plan(100, 68.0, px(-680.0), px(340.0), 8, 3);

        assert_eq!(plan.visible_slice.start_index, 10);
        assert_eq!(plan.visible_slice.end_index, 16);
        assert_eq!(plan.heavy_slice.start_index, 10);
        assert_eq!(plan.heavy_slice.end_index, 13);
        assert_eq!(plan.heavy_slice.len(), 3);
    }

    #[::core::prelude::v1::test]
    fn virtual_list_unmeasured_viewport_renders_initial_batch() {
        let plan = compute_virtual_list_plan(20, 96.0, px(0.0), px(0.0), 1, 10);

        // Even with 0 viewport height, initial batch renders at least 8 items.
        assert_eq!(plan.visible_slice.start_index, 0);
        assert!(plan.render_slice.end_index >= 8);
    }

    #[::core::prelude::v1::test]
    fn virtual_list_unmeasured_viewport_preserves_scrolled_window() {
        let plan = compute_virtual_list_plan(12, 84.0, px(-588.0), px(0.0), 2, 12);

        // A transient zero-sized ScrollHandle must not use the speculative 672px render batch as
        // its clamp viewport. The logical scroll position is seven rows down and stays there until
        // measured bounds arrive, so retained rows cannot jump toward the top for one frame.
        assert_eq!(plan.visible_slice.start_index, 7);
        assert_eq!(plan.render_slice.start_index, 5);
        assert_eq!(plan.visible_slice.end_index, 12);
    }

    #[::core::prelude::v1::test]
    fn virtual_list_rejects_non_finite_geometry() {
        let plan = compute_virtual_list_plan(10, f32::NAN, px(0.0), px(320.0), 2, 8);
        assert_eq!(plan.render_slice.visible_len(), 0);

        let plan = compute_virtual_list_plan(10, 64.0, px(f32::NAN), px(320.0), 2, 8);
        assert_eq!(plan.visible_slice.start_index, 0);
    }
}
