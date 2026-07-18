use crate::{
    AnyElement, App, AvailableSpace, Bounds, ContentMask, DispatchPhase, Element, GlobalElementId,
    HitboxBehavior, InspectorElementId, IntoElement, Overflow, Pixels, ScrollDelta,
    ScrollWheelEvent, Style, StyleRefinement, Styled, Window, px, size,
};
use refineable::Refineable as _;
use sum_tree::SumTree;

use super::{
    layout::ListPrepaintState,
    state::{ListItem, ListState},
    types::{ListSizingBehavior, RenderItemFn},
};

/// Construct a new list element
pub fn list(
    state: ListState,
    render_item: impl FnMut(usize, &mut Window, &mut App) -> AnyElement + 'static,
) -> List {
    List {
        state,
        render_item: Box::new(render_item),
        style: StyleRefinement::default(),
        sizing_behavior: ListSizingBehavior::default(),
    }
}

/// A list element
pub struct List {
    state: ListState,
    render_item: Box<RenderItemFn>,
    style: StyleRefinement,
    sizing_behavior: ListSizingBehavior,
}

impl List {
    /// Set the sizing behavior for the list.
    pub fn with_sizing_behavior(mut self, behavior: ListSizingBehavior) -> Self {
        self.sizing_behavior = behavior;
        self
    }
}

impl Element for List {
    type RequestLayoutState = ();
    type PrepaintState = ListPrepaintState;

    fn id(&self) -> Option<crate::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (crate::LayoutId, Self::RequestLayoutState) {
        let layout_id = match self.sizing_behavior {
            ListSizingBehavior::Infer => {
                let mut style = Style::default();
                style.overflow.y = Overflow::Scroll;
                style.refine(&self.style);
                window.with_text_style(style.text_style().cloned(), |window| {
                    let state = &mut *self.state.0.borrow_mut();

                    let available_height = if let Some(last_bounds) = state.last_layout_bounds {
                        last_bounds.size.height
                    } else {
                        // If we don't have the last layout bounds (first render),
                        // we might just use the overdraw value as the available height to layout enough items.
                        state.overdraw
                    };
                    let padding = style.padding.to_pixels(
                        state.last_layout_bounds.unwrap_or_default().size.into(),
                        window.rem_size(),
                    );

                    let layout_response = state.layout_items(
                        None,
                        available_height,
                        &padding,
                        &mut self.render_item,
                        window,
                        cx,
                    );
                    let max_element_width = layout_response.max_item_width;

                    let summary = state.items.summary();
                    let total_height = summary.height;
                    state.recycle_item_layouts(layout_response.item_layouts);

                    window.request_measured_layout(
                        style,
                        move |known_dimensions, available_space, _window, _cx| {
                            let width =
                                known_dimensions
                                    .width
                                    .unwrap_or(match available_space.width {
                                        AvailableSpace::Definite(x) => x,
                                        AvailableSpace::MinContent | AvailableSpace::MaxContent => {
                                            max_element_width
                                        }
                                    });
                            let height = match available_space.height {
                                AvailableSpace::Definite(height) => total_height.min(height),
                                AvailableSpace::MinContent | AvailableSpace::MaxContent => {
                                    total_height
                                }
                            };
                            size(width, height)
                        },
                    )
                })
            }
            ListSizingBehavior::Auto => {
                let mut style = Style::default();
                style.refine(&self.style);
                window.with_text_style(style.text_style().cloned(), |window| {
                    window.request_layout(style, None, cx)
                })
            }
        };
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> ListPrepaintState {
        let state = &mut *self.state.0.borrow_mut();
        state.reset = false;

        let mut style = Style::default();
        style.refine(&self.style);

        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);

        // If the width of the list has changed, invalidate all cached item heights
        if state
            .last_layout_bounds
            .is_none_or(|last_bounds| last_bounds.size.width != bounds.size.width)
        {
            let new_items = SumTree::from_iter(
                state.items.iter().map(|item| ListItem::Unmeasured {
                    focus_handle: item.focus_handle(),
                }),
                (),
            );

            state.items = new_items;
        }

        let padding = style
            .padding
            .to_pixels(bounds.size.into(), window.rem_size());
        let layout =
            match state.prepaint_items(bounds, padding, true, &mut self.render_item, window, cx) {
                Ok(layout) => layout,
                Err(autoscroll_request) => {
                    state.logical_scroll_top = Some(autoscroll_request);
                    state
                        .prepaint_items(bounds, padding, false, &mut self.render_item, window, cx)
                        .unwrap()
                }
            };

        state.last_layout_bounds = Some(bounds);
        state.last_padding = Some(padding);
        ListPrepaintState { hitbox, layout }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<crate::Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let current_view = window.current_view();
        window.with_content_mask(
            Some(ContentMask {
                bounds,
                ..Default::default()
            }),
            |window| {
                for item in &mut prepaint.layout.item_layouts {
                    item.element.paint(window, cx);
                }
            },
        );

        let height = bounds.size.height;
        let scroll_top = prepaint.layout.scroll_top;
        let hitbox_id = prepaint.hitbox.id;
        let item_layouts = std::mem::take(&mut prepaint.layout.item_layouts);
        self.state.0.borrow_mut().recycle_item_layouts(item_layouts);

        let list_state = self.state.clone();
        let mut accumulated_scroll_delta = ScrollDelta::default();
        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && hitbox_id.should_handle_scroll(window) {
                accumulated_scroll_delta = accumulated_scroll_delta.coalesce(event.delta);
                let pixel_delta = accumulated_scroll_delta.pixel_delta(px(20.));
                list_state.0.borrow_mut().scroll(
                    &scroll_top,
                    height,
                    pixel_delta,
                    current_view,
                    window,
                    cx,
                )
            }
        });
    }
}

impl IntoElement for List {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Styled for List {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}
