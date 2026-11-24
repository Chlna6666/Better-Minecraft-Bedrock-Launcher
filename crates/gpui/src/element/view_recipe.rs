use crate::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, Pixels, Window,
};
use std::rc::Rc;

/// An explicitly replayable View render result.
///
/// The factory must be repeatable and depend only on captured immutable render
/// output. It is invoked to construct fresh frame-local elements, never to
/// retain an arena allocation. The owning View must notify on changes to
/// its own state, and read external dependencies during Render. Do not opt in
/// when Render performs required per-frame side effects, reads an untracked
/// clock or mutable state, or depends on untracked window properties.
/// Avoid capturing the owning Entity strongly in the factory.
#[derive(Clone)]
pub struct RetainedViewRecipe {
    factory: Rc<dyn Fn() -> AnyElement>,
}

impl RetainedViewRecipe {
    /// Create a repeatable factory from the View's computed render output.
    pub fn new<E: IntoElement, F: Fn() -> E + 'static>(factory: F) -> Self {
        Self {
            factory: Rc::new(move || factory().into_any_element()),
        }
    }

    pub(crate) fn materialize(&self) -> AnyElement {
        (self.factory)()
    }
}

impl IntoElement for RetainedViewRecipe {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for RetainedViewRecipe {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    const RETAINED_REPLAY_CAPABILITY: crate::RetainedReplayCapability =
        crate::RetainedReplayCapability::OwnsFrameLocalCacheBoundary;

    fn id(&self) -> Option<ElementId> { None }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> { None }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, AnyElement) {
        let mut element = self.materialize();
        let layout = element.request_layout(window, cx);
        (layout, element)
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
        element.prepaint(window, cx);
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
        element.paint(window, cx);
    }
}
