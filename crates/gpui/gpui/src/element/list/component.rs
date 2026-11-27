use crate::{
    AnyElement, App, Bounds, ContentMask, DispatchPhase, Element,
    GlobalElementId, HitboxBehavior, InspectorElementId, IntoElement, Overflow, Pixels,
    ScrollWheelEvent, Style, StyleRefinement, Styled, Window, px, size,
};
use refineable::Refineable as _;
use std::rc::Rc;
use crate::window::scroll_rows::RowRevision;

use super::{
    configuration::{ListSizingBehavior, RenderItemFn},
    layout::ListPrepaintLayout,
    state::ListState,
};

/// Construct a new list element
pub fn list(
    state: ListState,
    render_item: impl FnMut(usize, &mut Window, &mut App) -> AnyElement + 'static,
) -> List {
    List {
        state,
        render_item: Box::new(render_item),
        row_revision: None,
        style: StyleRefinement::default(),
        sizing_behavior: ListSizingBehavior::default(),
    }
}

/// A list element
pub struct List {
    state: ListState,
    render_item: Box<RenderItemFn>,
    row_revision: Option<RowRevision>,
    style: StyleRefinement,
    sizing_behavior: ListSizingBehavior,
}

impl List {
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
    /// For offscreen height changes, also call [ListState::invalidate_items].
    pub fn with_row_revision(mut self, revision: impl Fn(usize) -> u64 + 'static) -> Self {
        self.row_revision = Some(Rc::new(revision));
        self
    }

    /// Set the sizing behavior for the list.
    pub fn with_sizing_behavior(mut self, behavior: ListSizingBehavior) -> Self {
        self.sizing_behavior = behavior;
        self
    }
}

impl Element for List {
    type RequestLayoutState = ();
    type PrepaintState = ListPrepaintLayout;

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
                        None,
                        window,
                        cx,
                    );
                    let max_element_width = layout_response.max_item_width;

                    let summary = state.items.summary();
                    let total_height = summary.height;
                    state.recycle_item_layouts(layout_response.item_layouts);

                    window.request_size_layout(
                        style,
                        crate::layout::SizeMeasurement::List(size(max_element_width, total_height)),
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
    ) -> ListPrepaintLayout {
        let mut state = self.state.0.borrow_mut();
        state.reset = false;

        let mut style = Style::default();
        style.refine(&self.style);

        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);

        if state
            .last_layout_bounds
            .is_none_or(|last_bounds| last_bounds.size.width != bounds.size.width)
        {
            state.invalidate_width_measurements();
        }

        let padding = style
            .padding
            .to_pixels(bounds.size.into(), window.rem_size());
        let input_start = window.next_frame.hitboxes.len();
        let viewport = ContentMask::new(window.visual_bounds(bounds))
            .intersect(&window.visual_content_mask());
        let mut layout =
            match state.prepaint_items(bounds, padding, true, &mut self.render_item, self.row_revision.clone(), window, cx) {
                Ok(layout) => layout,
                Err(autoscroll_request) => {
                    state.version.bump_if(state.logical_scroll_top != Some(autoscroll_request));
                    state.logical_scroll_top = Some(autoscroll_request);
                    state
                        .prepaint_items(bounds, padding, false, &mut self.render_item, None, window, cx)
                        .unwrap()
                }
            };

        // Only the successful prepaint transaction contributes input records.
        window.next_frame.record_layer_input(input_start, viewport.bounds);
        state.version.bump_if(state.last_layout_bounds != Some(bounds) || state.last_padding != Some(padding));
        state.last_layout_bounds = Some(bounds);
        state.last_padding = Some(padding);
        drop(state);

        // Capture the exact measurement geometry before Paint can execute
        // user code. Publish the binding only with the completed frame.
        let scroll_top = layout.scroll_top;
        let visible = scroll_top.item_ix..scroll_top.item_ix + layout.item_layouts.len();
        if let Some(plan) = layout.row_plan.as_mut()
            && layout.item_layouts.iter().map(|item| item.index).eq(visible.clone())
            && let Some(proof) = self.state.placement_proof(scroll_top, visible.clone())
        {
            plan.bind_variable_placement(proof, visible);
        }
        ListPrepaintLayout { hitbox, layout }
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
                    window.paint_planned_scroll_row(
                        prepaint.layout.row_plan.as_mut(),
                        item.index,
                        item.element.take(),
                        cx,
                    );
                }
            },
        );

        let height = bounds.size.height;
        if let Some(plan) = prepaint.layout.row_plan.as_ref() {
            window.remember_row_placement(plan);
        }
        let hitbox_id = prepaint.hitbox.id;
        let item_layouts = std::mem::take(&mut prepaint.layout.item_layouts);
        self.state.0.borrow_mut().recycle_item_layouts(item_layouts);

        let list_state = self.state.clone();
        window.watch_mounted_scroll(current_view, &list_state.0.borrow().version);
        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && hitbox_id.should_handle_scroll(window) {
                let pixel_delta = event.delta.pixel_delta(px(20.));
                list_state.0.borrow_mut().scroll(
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
