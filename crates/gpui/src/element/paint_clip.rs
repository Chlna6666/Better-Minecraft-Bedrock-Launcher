use crate::{
    AnyElement, App, Bounds, ContentMask, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, Window, point, size,
};

/// The fixed edge from which a vertical paint-only reveal exposes its child.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VerticalRevealEdge {
    Top,
    Bottom,
}

/// Extension trait for clipping an element vertically without feeding the animated reveal size
/// back into layout.
///
/// The child keeps its final layout bounds for the entire animation. Only the prepaint/paint content
/// mask changes, so descendant text keeps stable glyph origins and subpixel variants while the
/// visible portion of the subtree is revealed from one fixed edge.
pub trait PaintClipExt: IntoElement + Sized + 'static {
    #[track_caller]
    fn with_vertical_reveal_clip(
        self,
        visible_height: Pixels,
        edge: VerticalRevealEdge,
    ) -> VerticalRevealClipElement<Self> {
        VerticalRevealClipElement {
            source: core::panic::Location::caller(),
            element: Some(self),
            visible_height,
            edge,
        }
    }
}

impl<E: IntoElement + Sized + 'static> PaintClipExt for E {}

pub struct VerticalRevealClipElement<E> {
    source: &'static core::panic::Location<'static>,
    element: Option<E>,
    visible_height: Pixels,
    edge: VerticalRevealEdge,
}

impl<E: IntoElement + 'static> VerticalRevealClipElement<E> {
    fn reveal_bounds(&self, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        let visible_height = self
            .visible_height
            .max(Pixels::ZERO)
            .min(bounds.size.height);
        let origin_y = match self.edge {
            VerticalRevealEdge::Top => bounds.origin.y,
            VerticalRevealEdge::Bottom => bounds.origin.y + bounds.size.height - visible_height,
        };
        Bounds::new(
            point(bounds.origin.x, origin_y),
            size(bounds.size.width, visible_height),
        )
    }
}

impl<E: IntoElement + 'static> IntoElement for VerticalRevealClipElement<E> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<E: IntoElement + 'static> Element for VerticalRevealClipElement<E> {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        Some(self.source)
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
            .expect("vertical reveal clip element should only be laid out once")
            .into_any_element();
        let layout_id = element.request_layout(window, cx);
        (layout_id, element)
    }

    fn prepaint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let mask = ContentMask::new(self.reveal_bounds(bounds));
        window.with_content_mask(Some(mask), |window| {
            element.prepaint(window, cx);
        });
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
        let mask = ContentMask::new(self.reveal_bounds(bounds));
        window.with_content_mask(Some(mask), |window| {
            element.paint(window, cx);
        });
    }
}
