//! A View lifecycle without the automatic reactive cache, for fallback-routing tests.
use super::*;

pub(super) struct View(pub(super) AnyView);

impl IntoElement for View {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for View {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(ElementId::View(self.0.entity_id()))
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        window.with_rendered_view(self.0.entity_id(), |window| {
            let mut element = self.0.render_element(window, cx);
            let layout_id = element.request_layout(window, cx);
            (layout_id, element)
        })
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        element: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.set_view_id(self.0.entity_id());
        window.with_rendered_view(self.0.entity_id(), |window| {
            element.prepaint(window, cx);
        });
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        element: &mut AnyElement,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_rendered_view(self.0.entity_id(), |window| element.paint(window, cx));
    }
}
