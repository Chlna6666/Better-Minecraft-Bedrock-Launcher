use crate::{
    App, AvailableSpace, Bounds, ContentMask, Edges, ElementId, Pixels, Point, Window, px, size,
};
use sum_tree::{Bias, SumTree};

use super::{ListItem, StateInner};
use crate::element::list::{
    configuration::{ListAlignment, ListMeasuringBehavior, ListOffset, RenderItemFn},
    layout::{ItemLayout, LayoutItemsResponse},
    tree::Count,
};

const RETAINED_LIST_ITEM_KEY: &str = "__gpui_retained_list_item";

fn with_item_retained_key<R>(
    window: &mut Window,
    item_index: usize,
    f: impl FnOnce(&mut Window) -> R,
) -> R {
    window.with_retained_child_key(
        ElementId::named_usize(RETAINED_LIST_ITEM_KEY, item_index),
        f,
    )
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

        let cursor = self.items.cursor::<Count>(());
        let available_item_space = size(
            AvailableSpace::Definite(available_width),
            AvailableSpace::MinContent,
        );

        let mut measured_items = Vec::default();
        let mut measured_item_count = 0;

        for (ix, item) in cursor.enumerate() {
            let size = item.size().unwrap_or_else(|| {
                measured_item_count += 1;
                with_item_retained_key(window, ix, |window| {
                    let mut element = render_item(ix, window, cx);
                    element.layout_as_root(available_item_space, window, cx)
                })
            });

            measured_items.push(ListItem::Measured {
                size,
                focus_handle: item.focus_handle(),
            });
        }

        window.record_list_measured_items(measured_item_count);
        self.items = SumTree::from_iter(measured_items, ());
    }

    pub(in crate::element::list) fn layout_items(
        &mut self,
        available_width: Option<Pixels>,
        available_height: Pixels,
        padding: &Edges<Pixels>,
        render_item: &mut RenderItemFn,
        window: &mut Window,
        cx: &mut App,
    ) -> LayoutItemsResponse {
        let old_items = self.items.clone();
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

        // Render items after the scroll top, including those in the trailing overdraw.
        cursor.seek(&Count(scroll_top.item_ix), Bias::Right);
        for (ix, item) in cursor.by_ref().enumerate() {
            let visible_height = rendered_height - scroll_top.offset_in_item;
            if visible_height >= available_height + self.overdraw {
                break;
            }

            let mut item_size = item.size();

            // Bind retained identity to the logical item index, not to materialization order. This
            // prevents measurement-only overdraw items or later `push_front` work from shifting the
            // identity of visible rows.
            if visible_height < available_height || item_size.is_none() {
                let item_index = scroll_top.item_ix + ix;
                let (element, element_size) = with_item_retained_key(window, item_index, |window| {
                    let mut element = render_item(item_index, window, cx);
                    measured_item_count += 1;
                    let element_size = element.layout_as_root(available_item_space, window, cx);
                    (element, element_size)
                });
                item_size = Some(element_size);
                if visible_height < available_height {
                    item_layouts.push_back(ItemLayout {
                        index: item_index,
                        element,
                        size: element_size,
                    });
                    if item.contains_focused(window, cx) {
                        rendered_focused_item = true;
                    }
                }
            }

            let item_size = item_size.unwrap();
            rendered_height += item_size.height;
            max_item_width = max_item_width.max(item_size.width);
            measured_items.push_back(ListItem::Measured {
                size: item_size,
                focus_handle: item.focus_handle(),
            });
        }
        rendered_height += padding.bottom;

        cursor.seek(&Count(scroll_top.item_ix), Bias::Right);

        // If the rendered items do not fill the visible region, fill upward. These elements are
        // created after the forward pass but inserted at the front, so positional retained slots
        // are fundamentally invalid here; item-index keys keep their identity deterministic.
        if rendered_height - scroll_top.offset_in_item < available_height {
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
                    let focus_handle = item.focus_handle();
                    rendered_height += element_size.height;
                    measured_items.push_front(ListItem::Measured {
                        size: element_size,
                        focus_handle,
                    });
                    item_layouts.push_front(ItemLayout {
                        index: item_index,
                        element,
                        size: element_size,
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
        while leading_overdraw < self.overdraw {
            cursor.prev();
            if let Some(item) = cursor.item() {
                let item_index = cursor.start().0;
                let item_size = if let ListItem::Measured { size, .. } = item {
                    *size
                } else {
                    with_item_retained_key(window, item_index, |window| {
                        let mut element = render_item(item_index, window, cx);
                        measured_item_count += 1;
                        element.layout_as_root(available_item_space, window, cx)
                    })
                };

                leading_overdraw += item_size.height;
                measured_items.push_front(ListItem::Measured {
                    size: item_size,
                    focus_handle: item.focus_handle(),
                });
            } else {
                break;
            }
        }

        let measured_range = cursor.start().0..(cursor.start().0 + measured_items.len());
        let mut cursor = old_items.cursor::<Count>(());
        let mut new_items = cursor.slice(&Count(measured_range.start), Bias::Right);
        new_items.extend(measured_items.drain(..), ());
        cursor.seek(&Count(measured_range.end), Bias::Right);
        new_items.append(cursor.suffix(), ());
        self.items = new_items;
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
                            let item_size = element.layout_as_root(available_item_space, window, cx);
                            (element, item_size)
                        });
                    item_layouts.push_back(ItemLayout {
                        index: item_index,
                        element,
                        size: item_size,
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
        }
    }

    pub(in crate::element::list) fn prepaint_items(
        &mut self,
        bounds: Bounds<Pixels>,
        padding: Edges<Pixels>,
        autoscroll: bool,
        render_item: &mut RenderItemFn,
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

            let mut layout_response = self.layout_items(
                Some(bounds.size.width),
                bounds.size.height,
                &padding,
                render_item,
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
                        Some(ContentMask {
                            bounds,
                            ..Default::default()
                        }),
                        |window| {
                            with_item_retained_key(window, item.index, |window| {
                                item.element.prepaint_at(item_origin, window, cx);
                            });
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
            Ok(layout_response)
        })
    }
}
