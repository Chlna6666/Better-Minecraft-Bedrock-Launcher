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
/// `AnyView` is intentionally only a type-erased handle. Cache policy, retained ranges,
/// progressive rendering and selective-splice state live in `CachedView`, not here.
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
/// This preserves parent-driven layout while recording the geometry and frame-local ranges
/// needed to reconcile an unchanged-layout subtree without rendering its ancestors.
#[doc(hidden)]
pub struct ViewElement {
    boundary: super::cached_view::CachedView,
}

impl ViewElement {
    #[inline]
    fn new(view: AnyView) -> Self {
        Self {
            boundary: super::cached_view::CachedView::reactive(view),
        }
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
    type RequestLayoutState = Option<AnyElement>;
    type PrepaintState = super::cached_view::CachedViewPrepaintState;
    const RETAINED_REPLAY_CAPABILITY: crate::RetainedReplayCapability =
        crate::RetainedReplayCapability::OwnsFrameLocalCacheBoundary;

    fn id(&self) -> Option<ElementId> {
        self.boundary.id()
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.boundary.request_layout(id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<crate::Pixels>,
        element: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.boundary
            .prepaint(id, inspector_id, bounds, element, window, cx)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<crate::Pixels>,
        element: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.boundary
            .paint(id, inspector_id, bounds, element, prepaint, window, cx);
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

    #[test]
    fn ordinary_view_element_rebases_its_frame_local_ranges() {
        assert_eq!(
            <ViewElement as Element>::RETAINED_REPLAY_CAPABILITY,
            crate::RetainedReplayCapability::OwnsFrameLocalCacheBoundary,
        );
    }
}
