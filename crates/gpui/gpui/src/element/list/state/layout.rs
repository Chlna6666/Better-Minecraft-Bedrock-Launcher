use crate::{
    App, AvailableSpace, Bounds, ContentMask, Edges, Pixels, Point, Window, px, size,
};
use sum_tree::{Bias, SumTree};
use crate::window::scroll_rows::{RowListIdentity, RowPlan, RowPrepaint, RowRevision};

use super::{ListItem, StateInner, replace_item_runs};
use crate::element::list::{
    configuration::{ListAlignment, ListMeasuringBehavior, ListOffset, RenderItemFn},
    layout::{ItemLayout, LayoutItemsResponse},
    tree::Count,
};

// gpui-fast #170 limits speculative work rather than paying for every
// overscan row after a content change. Here only unknown sizes require a
// render/layout; the ordinary List has no raster layer to rebuild.
const MAX_OVERSCAN_MEASUREMENTS_PER_SIDE: usize = 32;

fn with_item_retained_key<R>(
    window: &mut Window,
    item_index: usize,
    f: impl FnOnce(&mut Window) -> R,
) -> R {
    window.with_list_item_layout_key(item_index, f)
}

impl StateInner {
    fn layout_all_items(
        &mut self,
        available_width: Pixels,
        render_item: &mut RenderItemFn,
        window: &mut Window,
        cx: &mut App,
    ) {
        match &mut self.measuring_behavior {
            ListMeasuringBehavior::Visible => {
                return;
            }
            ListMeasuringBehavior::Measure(has_measured) => {
                if *has_measured {
                    return;
                }
                *has_measured = true;
            }
        }

        if self.items.summary().unrendered_count == 0 {
            return;
        }
        let available_item_space = size(
            AvailableSpace::Definite(available_width),
            AvailableSpace::MinContent,
        );
        let summary = self.items.summary();
        if summary.unrendered_count >= summary.count.div_ceil(2) {
            // Initial eager measurement is usually dense. Feed the native
            // tree builder directly instead of retaining one scratch record
            // for every row of a potentially very large list.
            let mut measured_count = 0;
            let next = SumTree::from_iter(self.items.iter().enumerate().map(|(ix, item)| {
                let size = item.size().unwrap_or_else(|| {
                    measured_count += 1;
                    with_item_retained_key(window, ix, |window| {
                        let mut element = render_item(ix, window, cx);
                        element.layout_as_root(available_item_space, window, cx)
                    })
                });
                ListItem::Measured { size, focus_handle: item.focus_handle() }
            }), ());
            self.items = next;
            self.version.bump_if(measured_count != 0);
            window.record_list_measured_items(measured_count);
            return;
        }

        let mut measured_items = std::mem::take(&mut self.measured_items_scratch);
        measured_items.clear();
        {
            let mut cursor = self.items.filter::<_, Count>((), |summary| summary.unrendered_count != 0);
            cursor.next();
            while let Some(item) = cursor.item() {
                let ix = cursor.start().0;
                debug_assert!(item.size().is_none());
                let size = with_item_retained_key(window, ix, |window| {
                    let mut element = render_item(ix, window, cx);
                    element.layout_as_root(available_item_space, window, cx)
                });
                measured_items.push_back((ix, ListItem::Measured {
                    size,
                    focus_handle: item.focus_handle(),
                }));
                cursor.next();
            }
        }
        let measured_item_count = measured_items.len();
        window.record_list_measured_items(measured_item_count);
        if measured_item_count != 0 {
            self.items = replace_item_runs(&self.items, &mut measured_items);
            self.version.bump();
        }
        self.measured_items_scratch = measured_items;
    }

    pub(in crate::element::list) fn layout_items(
        &mut self,
        available_width: Option<Pixels>,
        available_height: Pixels,
        padding: &Edges<Pixels>,
        render_item: &mut RenderItemFn,
        retained_rows: Option<(&RowPlan, Point<Pixels>)>,
        window: &mut Window,
        cx: &mut App,
    ) -> LayoutItemsResponse {
        let old_items = self.items.clone();
        let previous_top = self.logical_scroll_top;
        let mut measured_items = std::mem::take(&mut self.measured_items_scratch);
        measured_items.clear();
        let mut item_layouts = std::mem::take(&mut self.item_layouts_scratch);
        item_layouts.clear();
        let mut rendered_height = padding.top;
        let mut max_item_width = px(0.);
        let mut scroll_top = self.logical_scroll_top();
        let mut rendered_focused_item = false;
        let mut measured_item_count = 0;

        let available_item_space = size(
            available_width.map_or(AvailableSpace::MinContent, |width| {
                AvailableSpace::Definite(width)
            }),
            AvailableSpace::MinContent,
        );

        let mut cursor = old_items.cursor::<Count>(());

        // Explicit measure_all retains its eager measurement semantics.
        let overscan_budget = if matches!(self.measuring_behavior, ListMeasuringBehavior::Visible) {
            MAX_OVERSCAN_MEASUREMENTS_PER_SIDE
        } else {
            usize::MAX
        };
        let mut trailing_budget = overscan_budget;

        // Render items after the scroll top, including those in the trailing overdraw.
        cursor.seek(&Count(scroll_top.item_ix), Bias::Right);
        for (ix, item) in cursor.by_ref().enumerate() {
            let visible_height = rendered_height - scroll_top.offset_in_item;
            if visible_height >= available_height + self.overdraw {
                break;
            }

            let mut item_size = item.size();
            if visible_height >= available_height && item_size.is_none() {
                if trailing_budget == 0 {
                    break;
                }
                trailing_budget -= 1;
            }

            // Bind retained identity to the logical item index, not to materialization order. This
            // prevents measurement-only overdraw items or later `push_front` work from shifting the
            // identity of visible rows.
            if visible_height < available_height || item_size.is_none() {
                let item_index = scroll_top.item_ix + ix;
                let retained_size = item_size.filter(|_| {
                    visible_height < available_height && retained_rows.is_some_and(|(plan, origin)| {
                        let item_origin = origin + Point::new(px(0.), visible_height);
                        window.can_reuse_scroll_row(plan, item_index, item_origin)
                    })
                });
                let (element, element_size, capture_reads) = if let Some(size) = retained_size {
                    (None, size, None)
                } else {
                    with_item_retained_key(window, item_index, |window| {
                        let read_start = retained_rows.map(|_| cx.entities.state_read_index());
                        let mut element = render_item(item_index, window, cx);
                        let read_start = read_start.filter(|_| {
                            visible_height < available_height
                                && retained_rows.is_some_and(|(plan, _)| plan.can_capture(&mut element))
                        });
                        measured_item_count += 1;
                        let element_size = element.layout_as_root(available_item_space, window, cx);
                        let reads = read_start.map(|start| cx.entities.state_reads_since(start));
                        (Some(element), element_size, reads)
                    })
                };
                item_size = Some(element_size);
                if visible_height < available_height {
                    item_layouts.push_back(ItemLayout {
                        index: item_index,
                        element,
                        size: element_size,
                        capture_reads,
                    });
                    if item.contains_focused(window, cx) {
                        rendered_focused_item = true;
                    }
                }
            }

            let item_size = item_size.unwrap();
            if item.size() != Some(item_size) {
                measured_items.push_back((scroll_top.item_ix + ix, ListItem::Measured {
                    size: item_size,
                    focus_handle: item.focus_handle(),
                }));
            }
            rendered_height += item_size.height;
            max_item_width = max_item_width.max(item_size.width);
        }
        rendered_height += padding.bottom;

        cursor.seek(&Count(scroll_top.item_ix), Bias::Right);

        // If the rendered items do not fill the visible region, fill upward. These elements are
        // created after the forward pass but inserted at the front, so positional retained slots
        // are fundamentally invalid here; item-index keys keep their identity deterministic.
        if rendered_height - scroll_top.offset_in_item < available_height {
            if item_layouts.iter().any(|row| row.element.is_none()) {
                // Anchoring at the tail changes every row origin. Decide the
                // actual placement in the ordinary pass before carrying input.
                self.measured_items_scratch = measured_items;
                self.item_layouts_scratch = item_layouts;
                return self.layout_items(
                    available_width, available_height, padding, render_item, None, window, cx,
                );
            }
            while rendered_height < available_height {
                cursor.prev();
                if let Some(item) = cursor.item() {
                    let item_index = cursor.start().0;
                    let (element, element_size) =
                        with_item_retained_key(window, item_index, |window| {
                            let mut element = render_item(item_index, window, cx);
                            measured_item_count += 1;
                            let element_size =
                                element.layout_as_root(available_item_space, window, cx);
                            (element, element_size)
                        });
                    if item.size() != Some(element_size) {
                        measured_items.push_front((item_index, ListItem::Measured {
                            size: element_size,
                            focus_handle: item.focus_handle(),
                        }));
                    }
                    rendered_height += element_size.height;
                    item_layouts.push_front(ItemLayout {
                        index: item_index,
                        element: Some(element),
                        size: element_size,
                        capture_reads: None,
                    });
                    if item.contains_focused(window, cx) {
                        rendered_focused_item = true;
                    }
                } else {
                    break;
                }
            }

            scroll_top = ListOffset {
                item_ix: cursor.start().0,
                offset_in_item: rendered_height - available_height,
            };

            match self.alignment {
                ListAlignment::Top => {
                    scroll_top.offset_in_item = scroll_top.offset_in_item.max(px(0.));
                    self.logical_scroll_top = Some(scroll_top);
                }
                ListAlignment::Bottom => {
                    scroll_top = ListOffset {
                        item_ix: cursor.start().0,
                        offset_in_item: rendered_height - available_height,
                    };
                    self.logical_scroll_top = None;
                }
            };
        }

        let mut leading_overdraw = scroll_top.offset_in_item;
        let mut leading_budget = overscan_budget;
        while leading_overdraw < self.overdraw {
            cursor.prev();
            if let Some(item) = cursor.item() {
                let item_index = cursor.start().0;
                let item_size = if let ListItem::Measured { size, .. } = item {
                    *size
                } else {
                    if leading_budget == 0 {
                        break;
                    }
                    leading_budget -= 1;
                    with_item_retained_key(window, item_index, |window| {
                        let mut element = render_item(item_index, window, cx);
                        measured_item_count += 1;
                        element.layout_as_root(available_item_space, window, cx)
                    })
                };

                if item.size() != Some(item_size) {
                    measured_items.push_front((item_index, ListItem::Measured {
                        size: item_size,
                        focus_handle: item.focus_handle(),
                    }));
                }
                leading_overdraw += item_size.height;
            } else {
                break;
            }
        }

        // Only changed rows own new measurement records. Reuse unchanged
        // native SumTree stretches between them, including disjoint changes.
        // The forward pass appends, and the backward passes prepend, so the
        // changed indices are already strictly increasing.
        let geometry_changed = !measured_items.is_empty();
        if geometry_changed {
            self.items = replace_item_runs(&old_items, &mut measured_items);
        }
        self.version.bump_if(geometry_changed || previous_top != self.logical_scroll_top);
        self.measured_items_scratch = measured_items;

        if !rendered_focused_item {
            let mut cursor = self
                .items
                .filter::<_, Count>((), |summary| summary.has_focus_handles);
            cursor.next();
            while let Some(item) = cursor.item() {
                if item.contains_focused(window, cx) {
                    let item_index = cursor.start().0;
                    let (element, item_size) =
                        with_item_retained_key(window, item_index, |window| {
                            let mut element = render_item(item_index, window, cx);
                            measured_item_count += 1;
                            let item_size =
                                element.layout_as_root(available_item_space, window, cx);
                            (element, item_size)
                        });
                    item_layouts.push_back(ItemLayout {
                        index: item_index,
                        element: Some(element),
                        size: item_size,
                        capture_reads: None,
                    });
                    break;
                }
                cursor.next();
            }
        }

        window.record_list_measured_items(measured_item_count);
        LayoutItemsResponse {
            max_item_width,
            scroll_top,
            item_layouts,
            row_plan: None,
        }
    }

    pub(in crate::element::list) fn prepaint_items(
        &mut self,
        bounds: Bounds<Pixels>,
        padding: Edges<Pixels>,
        autoscroll: bool,
        render_item: &mut RenderItemFn,
        row_revision: Option<RowRevision>,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<LayoutItemsResponse, ListOffset> {
        window.transact(|window| {
            match self.measuring_behavior {
                ListMeasuringBehavior::Measure(has_measured) if !has_measured => {
                    self.layout_all_items(bounds.size.width, render_item, window, cx);
                }
                _ => {}
            }

            let mask = ContentMask::new(bounds);
            let mut row_plan = if autoscroll
                && window.scroll_row_replay_is_enabled()
                && !self.items.summary().has_focus_handles
            {
                let version = if row_revision.is_some() { &self.structure_version } else { &self.items_version };
                let identity = RowListIdentity::Variable(crate::StateRead::new(version));
                let available_space = size(bounds.size.width.into(), AvailableSpace::MinContent);
                window.with_content_mask(Some(mask), |window| {
                    window.plan_scroll_rows(identity, self.items.summary().count, available_space, row_revision, cx)
                })
            } else {
                None
            };
            let mut layout_response = self.layout_items(
                Some(bounds.size.width),
                bounds.size.height,
                &padding,
                render_item,
                row_plan.as_ref().map(|plan| (plan, bounds.origin)),
                window,
                cx,
            );
            let mut measured_item_count = 0;

            window.take_autoscroll();

            if bounds.size.height > padding.top + padding.bottom {
                let mut item_origin = bounds.origin + Point::new(px(0.), padding.top);
                item_origin.y -= layout_response.scroll_top.offset_in_item;
                for item in &mut layout_response.item_layouts {
                    window.with_content_mask(
                        Some(mask),
                        |window| {
                            let start = item.capture_reads.as_ref()
                                .map(|_| (window.prepaint_index(), cx.entities.state_read_index()));
                            with_item_retained_key(window, item.index, |window| {
                                if let Some(element) = item.element.as_mut() {
                                    element.prepaint_at(item_origin, window, cx);
                                }
                            });
                            if let Some(plan) = row_plan.as_mut() {
                                if item.element.is_none() {
                                    let prepaint = window.reuse_scroll_row(plan, item.index, item_origin);
                                    plan.record_prepaint(item.index, prepaint);
                                } else if let (Some((start, read_start)), Some(reads)) = (start, item.capture_reads.take()) {
                                    let prepaint_reads = cx.entities.state_reads_since(read_start);
                                    let reads = if prepaint_reads.is_empty() {
                                        reads
                                    } else {
                                        reads.iter().chain(prepaint_reads.iter()).cloned().collect()
                                    };
                                    plan.record_prepaint(item.index, RowPrepaint {
                                        row_revision: plan.row_revision(item.index),
                                        origin: item_origin,
                                        records: start..window.prepaint_index(),
                                        reads,
                                    });
                                }
                            }
                        },
                    );

                    if let Some(autoscroll_bounds) = window.take_autoscroll()
                        && autoscroll
                    {
                        if autoscroll_bounds.top() < bounds.top() {
                            window.record_list_measured_items(measured_item_count);
                            return Err(ListOffset {
                                item_ix: item.index,
                                offset_in_item: autoscroll_bounds.top() - item_origin.y,
                            });
                        } else if autoscroll_bounds.bottom() > bounds.bottom() {
                            let mut cursor = self.items.cursor::<Count>(());
                            cursor.seek(&Count(item.index), Bias::Right);
                            let mut height = bounds.size.height - padding.top - padding.bottom;
                            height -= autoscroll_bounds.bottom() - item_origin.y;

                            while height > Pixels::ZERO {
                                cursor.prev();
                                let Some(item) = cursor.item() else {
                                    break;
                                };

                                let item_index = cursor.start().0;
                                let item_size = item.size().unwrap_or_else(|| {
                                    with_item_retained_key(window, item_index, |window| {
                                        let mut item = render_item(item_index, window, cx);
                                        let available = size(
                                            bounds.size.width.into(),
                                            AvailableSpace::MinContent,
                                        );
                                        measured_item_count += 1;
                                        item.layout_as_root(available, window, cx)
                                    })
                                });
                                height -= item_size.height;
                            }

                            window.record_list_measured_items(measured_item_count);
                            return Err(ListOffset {
                                item_ix: cursor.start().0,
                                offset_in_item: if height < Pixels::ZERO {
                                    -height
                                } else {
                                    Pixels::ZERO
                                },
                            });
                        }
                    }

                    item_origin.y += item.size.height;
                }
            } else {
                layout_response.item_layouts.clear();
            }

            window.record_list_measured_items(measured_item_count);
            layout_response.row_plan = row_plan;
            Ok(layout_response)
        })
    }
}
