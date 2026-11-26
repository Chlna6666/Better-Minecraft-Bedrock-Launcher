use crate::{
    AnyElement, AnyEntity, AnyWeakEntity, App, Bounds, Context, Element, ElementId, Entity,
    EntityId, GlobalElementId, InspectorElementId, IntoElement, LayoutId, Render, WeakEntity,
    Window,
};
use anyhow::Result;
use std::{any::TypeId, fmt, ops::Range, rc::Rc};

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
        let previous_owner = cx.entities.set_render_dependency_owner(Some(self.entity_id()));
        let previous_input_owner = window.input_dependency_owner.replace(self.entity_id());
        let scroll_scope = crate::window::scroll_dependencies::ScrollReadScope::begin();
        cx.update_entity_erased(&self.entity, &mut |entity, cx| {
            let weak = weak
                .take()
                .expect("rendered entity callback must execute exactly once");
            #[cfg(gpui_hot_reload)]
            let rendered = subsecond::HotFn::current(render).call((entity, weak, window, cx));
            #[cfg(not(gpui_hot_reload))]
            let rendered = render(entity, weak, window, cx);
            element = Some(rendered);
        });
        window.record_scroll_reads(self.entity_id(), scroll_scope.finish());
        window.input_dependency_owner = previous_input_owner;
        cx.entities.set_render_dependency_owner(previous_owner);
        element.expect("rendered entity callback must produce an element")
    }
}

impl PartialEq for AnyView {
    fn eq(&self, other: &Self) -> bool {
        self.entity == other.entity
    }
}

impl Eq for AnyView {}

/// Shared lifecycle proxy for automatically retained rendered entities.
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

/// Frame-local state for an automatically retained view.
#[doc(hidden)]
pub struct ViewRequestLayout {
    kind: ViewRequestLayoutKind,
}

enum ViewRequestLayoutKind {
    /// The view's committed layout was claimed and its subtree is drawn again unless prepaint
    /// finds that what it inherits changed.
    Retained {
        admission: crate::window::RetainedViewRecord,
        layout_claims: crate::layout::LayoutClaimSpans,
        layout_state_range: Range<usize>,
        retention: RetentionDraft,
        reused: Option<ReusedView>,
    },
    /// The view was built this frame and will be recorded for the next one.
    Built {
        element: AnyElement,
        layout_root: LayoutId,
        layout_claims: crate::layout::LayoutClaimSpans,
        prepaint_range: Option<Range<crate::window::PrepaintStateIndex>>,
        context: Option<Rc<crate::window::RetainedPaintContext>>,
        layout_state_range: Range<usize>,
        retention: RetentionDraft,
    },
}

/// This frame's record position and the views drawn inside it.
#[derive(Default)]
struct RetentionDraft {
    slot: Option<crate::window::ViewRetentionSlot>,
    nested: Range<usize>,
}

struct ReusedView {
    prepaint_range: Range<crate::window::PrepaintStateIndex>,
    context: Rc<crate::window::RetainedPaintContext>,
}

impl Element for ViewElement {
    type RequestLayoutState = ViewRequestLayout;
    type PrepaintState = ();
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
        if let Some(global_id) = window.current_retained_element_id()
            && let Some((admission, layout_claims)) = window.admit_retained_view(&global_id, entity_id, cx)
        {
            let layout_root = admission.layout_root;
            let layout_state_range = window.replay_view_layout_states(&admission.layout_state_range);
            return (
                layout_root,
                ViewRequestLayout {
                    kind: ViewRequestLayoutKind::Retained {
                        admission,
                        layout_claims,
                        layout_state_range,
                        retention: RetentionDraft::default(),
                        reused: None,
                    },
                },
            );
        }
        let claim_start = window.claim_log_len();
        let element_state_start = window.begin_view_element_states();
        let type_name = cx.entities.type_name_for_id(entity_id).unwrap_or("unknown");
        let previous = cx.entities.set_render_dependency_owner(Some(entity_id));
        let (layout_id, element) = window.with_rendered_view(entity_id, |window| {
            window.record_rendered_view(entity_id, type_name);
            let mut element = self.view.render_element(window, cx);
            let layout_id = element.request_layout(window, cx);
            (layout_id, element)
        });
        let _ = cx.entities.set_render_dependency_owner(previous);
        let layout_claims = [claim_start..window.claim_log_len(), 0..0];
        let layout_state_range = window.view_layout_state_range(element_state_start);
        (
            layout_id,
            ViewRequestLayout {
                kind: ViewRequestLayoutKind::Built {
                    element,
                    layout_root: layout_id,
                    layout_claims,
                    prepaint_range: None,
                    context: None,
                    layout_state_range,
                    retention: RetentionDraft::default(),
                },
            },
        )
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<crate::Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let entity_id = self.view.entity_id();
        window.set_view_id(entity_id);
        let prepaint_start = window.prepaint_index();
        let mut retention = Some(RetentionDraft {
            slot: window
                .current_retained_element_id()
                .map(|key| window.reserve_view_retention(key)),
            nested: 0..0,
        });
        let previous = cx.entities.set_render_dependency_owner(Some(entity_id));
        window.with_rendered_view(entity_id, |window| {
            let context = Rc::new(window.current_retained_paint_context());
            match &mut state.kind {
                ViewRequestLayoutKind::Retained {
                    admission,
                    layout_claims,
                    layout_state_range,
                    reused,
                    ..
                } => {
                    if window.retained_view_context_matches(admission, bounds)
                        && let Some(prepaint_range) = window.reuse_retained_view_prepaint(admission)
                    {
                        *reused = Some(ReusedView { prepaint_range, context });
                        return;
                    }
                    // The committed layout no longer describes what this view inherits. Give the
                    // nodes back and draw the content into the slot the parent allocated.
                    window.release_retained_view(layout_claims);
                    window.cancel_view_layout_states(layout_state_range);
                    let layout_root = admission.layout_root;
                    let claim_start = window.claim_log_len();
                    let element_state_start = window.begin_view_element_states();
                    let mut element = {
                        let type_name =
                            cx.entities.type_name_for_id(entity_id).unwrap_or("unknown");
                        window.record_rendered_view(entity_id, type_name);
                        self.view.render_element(window, cx)
                    };
                    let rebuilt_root = window.with_retained_child_scope(|window| {
                        let rebuilt_root = element.request_layout(window, cx);
                        element.prepaint_as_root(
                            bounds.origin,
                            crate::size(
                                crate::AvailableSpace::Definite(bounds.size.width),
                                crate::AvailableSpace::Definite(bounds.size.height),
                            ),
                            window,
                            cx,
                        );
                        rebuilt_root
                    });
                    let mut layout_claims = [0..0, claim_start..window.claim_log_len()];
                    if rebuilt_root != layout_root {
                        // The parent still references the admitted root. Do not publish
                        // a reusable record for that root after replacing its subtree.
                        layout_claims = [0..0, 0..0];
                    }
                    state.kind = ViewRequestLayoutKind::Built {
                        element,
                        layout_root,
                        layout_claims,
                        prepaint_range: Some(prepaint_start.clone()..window.prepaint_index()),
                        context: Some(context),
                        layout_state_range: element_state_start..element_state_start,
                        retention: retention.take().expect("view retention slot"),
                    };
                }
                ViewRequestLayoutKind::Built {
                    element,
                    layout_claims,
                    prepaint_range,
                    context: recorded_context,
                    retention: state_retention,
                    ..
                } => {
                    let claim_start = window.claim_log_len();
                    element.prepaint(window, cx);
                    layout_claims[1] = claim_start..window.claim_log_len();
                    *prepaint_range = Some(prepaint_start.clone()..window.prepaint_index());
                    *recorded_context = Some(context);
                    *state_retention = retention.take().expect("view retention slot");
                }
            }
        });
        let _ = cx.entities.set_render_dependency_owner(previous);

        match &mut state.kind {
            ViewRequestLayoutKind::Retained {
                retention: state_retention,
                reused,
                ..
            } => {
                if reused.is_some() {
                    let mut draft = retention.take().expect("view retention slot");
                    if let Some(slot) = draft.slot.as_ref() {
                        draft.nested = window.finish_view_retention_scope(slot);
                    }
                    *state_retention = draft;
                }
            }
            ViewRequestLayoutKind::Built {
                retention: state_retention,
                ..
            } => {
                if let Some(slot) = state_retention.slot.as_ref() {
                    state_retention.nested = window.finish_view_retention_scope(slot);
                }
            }
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<crate::Pixels>,
        state: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let entity_id = self.view.entity_id();
        let previous = cx.entities.set_render_dependency_owner(Some(entity_id));
        window.with_rendered_view(entity_id, |window| match &mut state.kind {
            ViewRequestLayoutKind::Retained { admission, layout_claims, layout_state_range, retention, reused } => {
                let Some(reused) = reused.as_ref() else {
                    unreachable!("an admitted view is either drawn again or rebuilt in prepaint")
                };
                let Some(slot) = retention.slot.as_ref() else {
                    return;
                };
                let start = window.paint_index();
                window.replay_validated_paint(admission.paint_range.clone());
                window.record_debug_view_cache_status(bounds, crate::window::debug_visualization::ViewCacheDebugStatus::Hit, cx);
                let end = window.paint_index();
                window.commit_view_retention(
                    slot,
                    retention.nested.clone(),
                    crate::window::ViewRetentionCommit {
                        layout_root: admission.layout_root,
                        layout_claims: layout_claims.clone(),
                        prepaint_range: reused.prepaint_range.clone(),
                        paint_range: start..end,
                        bounds,
                        context: reused.context.clone(),
                        layout_state_range: layout_state_range.clone(),
                        reused: Some(admission.clone()),
                    },
                );
            }
            ViewRequestLayoutKind::Built {
                element,
                layout_root,
                layout_claims,
                prepaint_range,
                context,
                layout_state_range,
                retention,
                ..
            } => {
                let start = window.paint_index();
                element.paint(window, cx);
                let end = window.paint_index();
                let (Some(slot), Some(prepaint_range), Some(context)) =
                    (retention.slot.as_ref(), prepaint_range.as_ref(), context.as_ref())
                else {
                    return;
                };
                window.commit_view_retention(
                    slot,
                    retention.nested.clone(),
                    crate::window::ViewRetentionCommit {
                        layout_root: *layout_root,
                        layout_claims: layout_claims.clone(),
                        prepaint_range: prepaint_range.clone(),
                        paint_range: start..end,
                        bounds,
                        context: context.clone(),
                        layout_state_range: layout_state_range.clone(),
                        reused: None,
                    },
                );
            }
        });
        let _ = cx.entities.set_render_dependency_owner(previous);
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
