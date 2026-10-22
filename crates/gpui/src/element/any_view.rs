use crate::{
    AnyElement, AnyEntity, AnyWeakEntity, App, Bounds, Context, Element, ElementId, Entity,
    EntityId, GlobalElementId, InspectorElementId, IntoElement, LayoutId, Render, WeakEntity,
    Window,
};
use anyhow::Result;
use std::{any::TypeId, fmt};

type RenderViewFn = fn(&mut dyn std::any::Any, AnyWeakEntity, &mut Window, &mut App) -> AnyElement;

/// A dynamically typed renderable entity handle.
///
/// `AnyView` is intentionally only a type-erased handle. Retained ranges and dirty scopes are
/// owned by the window; this type only forwards rendering and identity.
#[derive(Clone, Debug)]
pub struct AnyView {
    entity: AnyEntity,
    render: RenderViewFn,
}

impl<V: Render> From<Entity<V>> for AnyView {
    fn from(value: Entity<V>) -> Self {
        Self {
            entity: value.into_any(),
            render: render_entity::<V>,
        }
    }
}

impl AnyView {
    /// Convert this view to a weak type-erased handle.
    pub fn downgrade(&self) -> AnyWeakView {
        AnyWeakView {
            entity: self.entity.downgrade(),
            render: self.render,
        }
    }

    /// Convert this view to a strongly typed entity handle.
    pub fn downcast<T: 'static>(self) -> Result<Entity<T>, Self> {
        match self.entity.downcast() {
            Ok(entity) => Ok(entity),
            Err(entity) => Err(Self {
                entity,
                render: self.render,
            }),
        }
    }

    /// The runtime type of the backing entity.
    pub fn entity_type(&self) -> TypeId {
        self.entity.entity_type()
    }

    /// The id of the backing entity.
    pub fn entity_id(&self) -> EntityId {
        self.entity.entity_id()
    }

    #[inline(never)]
    pub(super) fn render_element(&self, window: &mut Window, cx: &mut App) -> AnyElement {
        let _work = if crate::window::element_work::enabled() {
            crate::window::element_work::begin_named(
                cx.entities
                    .type_name_for_id(self.entity_id())
                    .unwrap_or("unknown"),
                crate::window::element_work::ElementWorkPhase::Render,
            )
        } else {
            None
        };
        let mut weak = Some(self.entity.downgrade());
        let mut element = None;
        let render = self.render;
        cx.update_entity_erased(&self.entity, &mut |entity, cx| {
            element = Some(render(
                entity,
                weak.take()
                    .expect("rendered entity callback must execute exactly once"),
                window,
                cx,
            ));
        });
        element.expect("rendered entity callback must produce an element")
    }
}

impl PartialEq for AnyView {
    fn eq(&self, other: &Self) -> bool {
        self.entity == other.entity
    }
}

impl Eq for AnyView {}

/// Shared non-generic lifecycle proxy for ordinary rendered entities.
///
/// The view renders into an ordinary element subtree during its own lifecycle. There is no
/// separate cache boundary: callers that need to avoid re-rendering a view must rely on the
/// window's dirty scopes, and unchanged subtrees reconcile through the regular element retained
/// replay proofs.
#[doc(hidden)]
pub struct ViewElement {
    view: AnyView,
}

impl ViewElement {
    #[inline]
    fn new(view: AnyView) -> Self {
        Self { view }
    }

    #[inline]
    fn from_entity<V: Render>(view: Entity<V>) -> Self {
        Self::new(view.into())
    }
}

impl IntoElement for ViewElement {
    type Element = Self;

    #[inline]
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for ViewElement {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();
    // A view's content changes are reported through entity notifications, not through element
    // semantics. An ancestor retained replay must therefore never skip this lifecycle: the view
    // Render has to run so its dirty state and child element tree are rebuilt in the current frame.
    const RETAINED_REPLAY_CAPABILITY: crate::RetainedReplayCapability =
        crate::RetainedReplayCapability::OwnsFrameLocalCacheBoundary;

    fn id(&self) -> Option<ElementId> {
        Some(ElementId::View(self.view.entity_id()))
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let entity_id = self.view.entity_id();
        let type_name = cx.entities.type_name_for_id(entity_id).unwrap_or("unknown");
        window.with_rendered_view(entity_id, |window| {
            window.record_rendered_view(entity_id, type_name);
            let mut element = self.view.render_element(window, cx);
            // Lay out the rendered tree inside the same view scope. Element request-layout code
            // queries the current view, exactly as it did when the view cache owned this boundary.
            let layout_id = element.request_layout(window, cx);
            (layout_id, element)
        })
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<crate::Pixels>,
        element: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let entity_id = self.view.entity_id();
        window.set_view_id(entity_id);
        window.with_rendered_view(entity_id, |window| element.prepaint(window, cx));
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<crate::Pixels>,
        element: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let entity_id = self.view.entity_id();
        window.with_rendered_view(entity_id, |window| element.paint(window, cx));
    }
}

#[inline(never)]
fn render_entity<V: Render>(
    entity: &mut dyn std::any::Any,
    weak: AnyWeakEntity,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let view = entity
        .downcast_mut::<V>()
        .expect("rendered entity type must match its render function");
    let weak = weak
        .downcast::<V>()
        .expect("rendered entity type must match its render function");
    let mut view_cx = Context::new_context(cx, weak);
    view.render(window, &mut view_cx).into_any_element()
}

impl<V: 'static + Render> IntoElement for Entity<V> {
    type Element = ViewElement;

    #[inline]
    fn into_element(self) -> Self::Element {
        ViewElement::from_entity(self)
    }

    #[track_caller]
    #[inline(never)]
    fn into_any_element(self) -> AnyElement {
        <ViewElement as Element>::into_any(ViewElement::from_entity(self))
    }
}

impl IntoElement for AnyView {
    type Element = ViewElement;

    #[inline]
    fn into_element(self) -> Self::Element {
        ViewElement::new(self)
    }
}

/// A weak, dynamically typed view handle.
#[derive(Clone)]
pub struct AnyWeakView {
    entity: AnyWeakEntity,
    render: RenderViewFn,
}

impl AnyWeakView {
    #[inline]
    pub(super) fn entity_id(&self) -> EntityId {
        self.entity.entity_id()
    }

    /// Upgrade to a strong type-erased view handle.
    pub fn upgrade(&self) -> Option<AnyView> {
        Some(AnyView {
            entity: self.entity.upgrade()?,
            render: self.render,
        })
    }
}

impl<V: 'static + Render> From<WeakEntity<V>> for AnyWeakView {
    fn from(view: WeakEntity<V>) -> Self {
        Self {
            entity: view.into(),
            render: render_entity::<V>,
        }
    }
}

impl PartialEq for AnyWeakView {
    fn eq(&self, other: &Self) -> bool {
        self.entity == other.entity
    }
}

impl fmt::Debug for AnyWeakView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnyWeakView")
            .field("entity_id", &self.entity.entity_id)
            .finish_non_exhaustive()
    }
}

/// A view that renders nothing.
pub struct EmptyView;

impl Render for EmptyView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::Empty
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppContext as _;

    #[test]
    fn ordinary_view_element_uses_caller_view_identity() {
        let cx = crate::TestAppContext::single();
        cx.update(|cx| {
            let entity = cx.new(|_| EmptyView);
            let id = entity.entity_id();
            let element = ViewElement::from_entity(entity);
            assert_eq!(element.id(), Some(ElementId::View(id)));
        });
    }
}
