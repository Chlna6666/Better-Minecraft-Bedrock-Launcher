use crate::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Window,
};

/// Extension trait for promoting an arbitrary GPUI subtree into one retained compositor layer.
///
/// The subtree keeps its normal layout and hit-testing semantics. During paint it is captured as a
/// single retained GPU layer, so an outer renderer-owned translation/scale/opacity animation can
/// update only the final composite record instead of repainting every child primitive. This is
/// especially useful for complex pages containing text, SVG/path content and nested clips, which
/// cannot safely be transformed by changing only the primitive types that support scene animation.
pub trait CompositeLayerExt: IntoElement + Sized + 'static {
    /// Capture this element's complete painted subtree into a retained compositor layer.
    ///
    /// The capture is tightened to actual pixel-producing child operations rather than this
    /// element's layout box. Place renderer-owned animations *outside* this wrapper, for example:
    /// `element.composite_layer().with_animation(...with_property(...), |element, _| element)`.
    fn composite_layer(self) -> CompositeLayerElement<Self> {
        CompositeLayerElement {
            element: Some(self),
        }
    }
}

impl<E: IntoElement + Sized + 'static> CompositeLayerExt for E {}

/// An element that preserves its child's layout while capturing the painted subtree into one
/// retained zero-filter composite target.
pub struct CompositeLayerElement<E> {
    element: Option<E>,
}

impl<E: IntoElement + 'static> IntoElement for CompositeLayerElement<E> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<E: IntoElement + 'static> Element for CompositeLayerElement<E> {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut element = self
            .element
            .take()
            .expect("composite layer element should only be laid out once")
            .into_any_element();
        let layout_id = element.request_layout(window, cx);
        (layout_id, element)
    }

    fn prepaint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        element.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.paint_composite_layer(bounds, |window| {
            element.paint(window, cx);
        });
    }
}
