use gpui::{Pixels, ScrollHandle, px};
use std::ops::Range;

const DEFAULT_UNMEASURED_VIEWPORT_ITEMS: usize = 8;
const DEFAULT_UNMEASURED_VIEWPORT_MIN_PX: f32 = 600.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VirtualListSlice {
    pub start_index: usize,
    pub end_index: usize,
}

impl VirtualListSlice {
    #[inline]
    pub fn len(self) -> usize {
        self.end_index.saturating_sub(self.start_index)
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.start_index >= self.end_index
    }

    #[inline]
    pub fn contains(self, index: usize) -> bool {
        self.start_index <= index && index < self.end_index
    }

    #[inline]
    pub fn range(self) -> Range<usize> {
        self.start_index..self.end_index
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WindowedListSlice {
    pub start_index: usize,
    pub end_index: usize,
    pub top_spacer: Pixels,
    pub bottom_spacer: Pixels,
}

impl WindowedListSlice {
    #[inline]
    pub fn visible_len(self) -> usize {
        self.end_index.saturating_sub(self.start_index)
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.start_index >= self.end_index
    }

    #[inline]
    pub fn range(self) -> Range<usize> {
        self.start_index..self.end_index
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VirtualListPlan {
    pub render_slice: WindowedListSlice,
    pub visible_slice: VirtualListSlice,
    pub heavy_slice: VirtualListSlice,
}

impl VirtualListPlan {
    #[inline]
    pub fn render_range(self) -> Range<usize> {
        self.render_slice.range()
    }

    #[inline]
    pub fn visible_range(self) -> Range<usize> {
        self.visible_slice.range()
    }

    #[inline]
    pub fn heavy_range(self) -> Range<usize> {
        self.heavy_slice.range()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VirtualListOverscan {
    pub before: usize,
    pub after: usize,
}

impl VirtualListOverscan {
    #[inline]
    pub const fn new(before: usize, after: usize) -> Self {
        Self { before, after }
    }

    #[inline]
    pub const fn symmetric(items: usize) -> Self {
        Self::new(items, items)
    }
}

/// Configuration for a fixed-pitch virtual list.
///
/// The planner is intentionally allocation-free and does not own row state. Callers keep stable
/// domain identities on their row elements while this type only decides which logical indices are
/// worth materializing for the current viewport.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VirtualListConfig {
    item_pitch_px: f32,
    overscan: VirtualListOverscan,
    max_heavy_items: usize,
    fallback_visible_items: usize,
    fallback_min_height_px: f32,
}

impl VirtualListConfig {
    #[inline]
    pub fn new(item_pitch_px: f32) -> Self {
        Self {
            item_pitch_px,
            overscan: VirtualListOverscan::default(),
            max_heavy_items: usize::MAX,
            fallback_visible_items: DEFAULT_UNMEASURED_VIEWPORT_ITEMS,
            fallback_min_height_px: DEFAULT_UNMEASURED_VIEWPORT_MIN_PX,
        }
    }

    #[inline]
    pub fn with_overscan(mut self, overscan: VirtualListOverscan) -> Self {
        self.overscan = overscan;
        self
    }

    #[inline]
    pub fn with_symmetric_overscan(self, items: usize) -> Self {
        self.with_overscan(VirtualListOverscan::symmetric(items))
    }

    #[inline]
    pub fn with_heavy_budget(mut self, max_heavy_items: usize) -> Self {
        self.max_heavy_items = max_heavy_items;
        self
    }

    /// Configure the speculative render viewport used before a `ScrollHandle` has measured bounds.
    ///
    /// This fallback only controls how many rows are materialized. It never widens scroll clamping,
    /// so a transient zero-sized viewport cannot rebase an existing scroll position.
    #[inline]
    pub fn with_unmeasured_viewport(
        mut self,
        min_visible_items: usize,
        min_height_px: f32,
    ) -> Self {
        self.fallback_visible_items = min_visible_items;
        self.fallback_min_height_px = min_height_px;
        self
    }

    #[inline]
    pub fn plan(
        self,
        total_items: usize,
        scroll_offset_y: Pixels,
        viewport_height: Pixels,
    ) -> VirtualListPlan {
        compute_plan(self, total_items, scroll_offset_y, viewport_height)
    }

    #[inline]
    pub fn plan_for_scroll_handle(
        self,
        total_items: usize,
        scroll_handle: &ScrollHandle,
    ) -> VirtualListPlan {
        self.plan(
            total_items,
            scroll_handle.offset().y,
            scroll_handle.bounds().size.height,
        )
    }
}

#[inline]
fn finite_extent_px(item_count: usize, item_pitch_px: f64) -> f64 {
    (item_count as f64 * item_pitch_px).min(f32::MAX as f64)
}

#[inline]
fn spacer_px(item_count: usize, item_pitch_px: f64) -> Pixels {
    px(finite_extent_px(item_count, item_pitch_px) as f32)
}

fn compute_plan(
    config: VirtualListConfig,
    total_items: usize,
    scroll_offset_y: Pixels,
    viewport_height: Pixels,
) -> VirtualListPlan {
    if total_items == 0 || !config.item_pitch_px.is_finite() || config.item_pitch_px <= 0.0 {
        return VirtualListPlan::default();
    }

    let item_pitch_px = f64::from(config.item_pitch_px);
    let measured_viewport_height_px = viewport_height / px(1.0);
    let viewport_is_measured =
        measured_viewport_height_px.is_finite() && measured_viewport_height_px > 0.0;

    // `ScrollHandle::bounds()` is frame-derived and may temporarily report a zero-sized viewport
    // while a retained subtree is being rebuilt. Render a useful batch in that frame, but never use
    // that speculative size for scroll clamping: doing so changes the logical virtual window for one
    // frame and manifests as rows flashing, disappearing, or appearing to lose their data.
    let render_viewport_height_px = if viewport_is_measured {
        f64::from(measured_viewport_height_px).max(item_pitch_px)
    } else {
        let fallback_items = config.fallback_visible_items.max(1);
        let fallback_items_height = finite_extent_px(fallback_items, item_pitch_px);
        let fallback_min_height = if config.fallback_min_height_px.is_finite() {
            f64::from(config.fallback_min_height_px.max(0.0))
        } else {
            0.0
        };
        fallback_items_height.max(fallback_min_height)
    };
    let clamp_viewport_height_px = if viewport_is_measured {
        render_viewport_height_px
    } else {
        item_pitch_px
    };

    let content_height_px = finite_extent_px(total_items, item_pitch_px);
    let max_scroll_top = (content_height_px - clamp_viewport_height_px).max(0.0);
    let requested_scroll_top = -(scroll_offset_y / px(1.0));
    let requested_scroll_top = if requested_scroll_top.is_finite() {
        f64::from(requested_scroll_top)
    } else {
        0.0
    };
    let scroll_top = requested_scroll_top.clamp(0.0, max_scroll_top);

    // Account for the partially clipped leading row instead of always appending one extra row.
    // Exact row-aligned viewports therefore materialize exactly N rows, while fractional offsets
    // still include the trailing row needed to cover the viewport without holes.
    let visible_start = ((scroll_top / item_pitch_px).floor() as usize)
        .min(total_items.saturating_sub(1));
    let first_item_top = finite_extent_px(visible_start, item_pitch_px);
    let leading_hidden = (scroll_top - first_item_top).clamp(0.0, item_pitch_px);
    let visible_span = (render_viewport_height_px + leading_hidden).min(f32::MAX as f64);
    let visible_count = ((visible_span / item_pitch_px).ceil() as usize)
        .max(1)
        .min(total_items);
    let visible_end = visible_start.saturating_add(visible_count).min(total_items);

    let render_start = visible_start.saturating_sub(config.overscan.before);
    let render_end = visible_end
        .saturating_add(config.overscan.after)
        .min(total_items);

    let visible_len = visible_end.saturating_sub(visible_start);
    let heavy_len = visible_len.min(config.max_heavy_items);
    let heavy_end = visible_start.saturating_add(heavy_len).min(visible_end);

    VirtualListPlan {
        render_slice: WindowedListSlice {
            start_index: render_start,
            end_index: render_end,
            top_spacer: spacer_px(render_start, item_pitch_px),
            bottom_spacer: spacer_px(total_items.saturating_sub(render_end), item_pitch_px),
        },
        visible_slice: VirtualListSlice {
            start_index: visible_start,
            end_index: visible_end,
        },
        heavy_slice: VirtualListSlice {
            start_index: visible_start,
            end_index: heavy_end,
        },
    }
}

/// Compatibility helper for existing list call sites.
///
/// New reusable components should prefer [`VirtualListConfig`] so overscan and heavy-work policy are
/// explicit and can evolve independently without growing another positional-argument API.
#[inline]
pub fn compute_virtual_list_plan(
    total_items: usize,
    item_pitch_px: f32,
    scroll_offset_y: Pixels,
    viewport_height: Pixels,
    overscan: usize,
    max_heavy_items: usize,
) -> VirtualListPlan {
    VirtualListConfig::new(item_pitch_px)
        .with_symmetric_overscan(overscan)
        .with_heavy_budget(max_heavy_items)
        .plan(total_items, scroll_offset_y, viewport_height)
}

#[inline]
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
    fn virtual_list_exact_alignment_does_not_materialize_phantom_row() {
        let plan = compute_virtual_list_plan(100, 68.0, px(-680.0), px(340.0), 0, usize::MAX);

        assert_eq!(plan.visible_slice, VirtualListSlice { start_index: 10, end_index: 15 });
        assert_eq!(plan.visible_slice.len(), 5);
        assert_eq!(plan.render_slice.visible_len(), 5);
    }

    #[::core::prelude::v1::test]
    fn virtual_list_partial_leading_row_materializes_trailing_row() {
        let plan = compute_virtual_list_plan(100, 100.0, px(-50.0), px(200.0), 0, usize::MAX);

        assert_eq!(plan.visible_slice, VirtualListSlice { start_index: 0, end_index: 3 });
        assert_eq!(plan.visible_slice.len(), 3);
    }

    #[::core::prelude::v1::test]
    fn virtual_list_limits_heavy_slice_to_budget() {
        let plan = compute_virtual_list_plan(100, 68.0, px(-680.0), px(340.0), 8, 3);

        assert_eq!(plan.visible_slice.start_index, 10);
        assert_eq!(plan.visible_slice.end_index, 15);
        assert_eq!(plan.heavy_slice.start_index, 10);
        assert_eq!(plan.heavy_slice.end_index, 13);
        assert_eq!(plan.heavy_slice.len(), 3);
    }

    #[::core::prelude::v1::test]
    fn virtual_list_zero_heavy_budget_disables_heavy_rows() {
        let plan = compute_virtual_list_plan(100, 68.0, px(-680.0), px(340.0), 2, 0);

        assert!(plan.heavy_slice.is_empty());
        assert_eq!(plan.heavy_slice.start_index, plan.visible_slice.start_index);
    }

    #[::core::prelude::v1::test]
    fn virtual_list_supports_directional_overscan() {
        let plan = VirtualListConfig::new(68.0)
            .with_overscan(VirtualListOverscan::new(1, 3))
            .plan(100, px(-680.0), px(340.0));

        assert_eq!(plan.visible_slice, VirtualListSlice { start_index: 10, end_index: 15 });
        assert_eq!(plan.render_slice.start_index, 9);
        assert_eq!(plan.render_slice.end_index, 18);
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

        // A transient zero-sized ScrollHandle must not use the speculative render batch as its
        // clamp viewport. The logical scroll position is seven rows down and stays there until real
        // bounds arrive, so retained rows cannot jump toward the top for one frame.
        assert_eq!(plan.visible_slice.start_index, 7);
        assert_eq!(plan.render_slice.start_index, 5);
        assert_eq!(plan.visible_slice.end_index, 12);
    }

    #[::core::prelude::v1::test]
    fn virtual_list_rejects_non_finite_geometry() {
        let plan = compute_virtual_list_plan(10, f32::NAN, px(0.0), px(320.0), 2, 8);
        assert!(plan.render_slice.is_empty());

        let plan = compute_virtual_list_plan(10, 64.0, px(f32::NAN), px(320.0), 2, 8);
        assert_eq!(plan.visible_slice.start_index, 0);
    }
}
