use refineable::Refineable as _;

use crate::{
    App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement, Pixels,
    Style, StyleRefinement, Styled, Window,
};

/// Creates a zero-paint element that reports its final layout bounds during prepaint.
///
/// This is useful for components that need exact, post-layout geometry for anchors, popovers,
/// guided-tour spotlights, debug overlays, or other window-space positioning. The callback receives
/// absolute window-space [`Bounds<Pixels>`], matching the coordinates used by GPUI hitboxes and
/// paint primitives.
///
/// The observer participates in normal layout but does not paint or insert hitboxes. A common
/// pattern is attaching it to a `relative()` container with `.absolute().inset_0()` so it observes
/// exactly the container's final bounds without changing layout.
pub fn bounds_observer(
    callback: impl FnMut(Bounds<Pixels>, &mut Window, &mut App) + 'static,
) -> BoundsObserver {
    BoundsObserver {
        callback: Box::new(callback),
        style: StyleRefinement::default(),
    }
}

/// A zero-paint element that reports its resolved layout bounds during prepaint.
pub struct BoundsObserver {
    callback: Box<dyn FnMut(Bounds<Pixels>, &mut Window, &mut App)>,
    style: StyleRefinement,
}

impl IntoElement for BoundsObserver {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for BoundsObserver {
    type RequestLayoutState = Style;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
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
        let mut style = Style::default();
        style.refine(&self.style);
        let layout_id = window.request_layout(style.clone(), [], cx);
        (layout_id, style)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        (self.callback)(bounds, window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }
}

impl Styled for BoundsObserver {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}
