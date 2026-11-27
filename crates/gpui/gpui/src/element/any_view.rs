use crate::{
    AnyElement, AnyEntity, AnyWeakEntity, App, Bounds, Context, Element, ElementId, Entity,
    EntityId, GlobalElementId, InspectorElementId, IntoElement, LayoutId, Render, WeakEntity,
    Window,
};
use anyhow::Result;
use crate::layout::LayoutClaimBlocks;
use crate::window::scroll_views::{ScrollBinding, ScrollViewCapture};
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
    fn render_element(&self, window: &mut Window, cx: &mut App) -> AnyElement {
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
        let previous_input_owner = window.input_dependency_owner.replace(self.entity_id());
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
        window.input_dependency_owner = previous_input_owner;
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
    rebuild: Option<Rc<crate::window::ViewRebuild>>,
    splice_safe: bool,
    scroll_capture: Option<Box<ScrollViewCapture>>,
    scroll_read_start: Option<usize>,
}

impl ViewRequestLayout {
    /// Transfer the gap's complete request-layout span. Nested lifecycle
    /// states resolve relocated offsets at release/commit.
    pub(crate) fn adopt_prebuilt_layout(
        &mut self, window: &mut Window,
        claims: &LayoutClaimBlocks, states: &crate::window::StateAccessBlocks,
    ) -> bool {
        let ViewRequestLayoutKind::Built { layout_root, layout_claims, layout_state_range, .. } = &mut self.kind else {
            return false;
        };
        if layout_claims != claims || layout_state_range != states {
            return false;
        }
        let Some((moved_claims, moved_states)) = window.adopt_gap_layout_spans(*layout_root, claims, states) else {
            return false;
        };
        *layout_claims = moved_claims;
        *layout_state_range = moved_states;
        true
    }
}

enum ViewRequestLayoutKind {
    /// The view's committed layout was claimed and its subtree is drawn again unless prepaint
    /// finds that what it inherits changed.
    Retained {
        admission: crate::window::RetainedViewRecord,
        layout_claims: LayoutClaimBlocks,
        layout_state_range: crate::window::StateAccessBlocks,
        retention: RetentionDraft,
        reused: Option<ReusedView>,
        allow_reuse: bool,
    },
    Spliced {
        splice: Option<Box<crate::window::ViewSplice>>,
        retention: RetentionDraft,
        prepaint_range: Option<Range<crate::window::PrepaintStateIndex>>,
        context: Option<Rc<crate::window::RetainedPaintContext>>,
    },
    /// The view was built this frame and will be recorded for the next one.
    Built {
        element: AnyElement,
        layout_root: LayoutId,
        layout_claims: LayoutClaimBlocks,
        prepaint_range: Option<Range<crate::window::PrepaintStateIndex>>,
        context: Option<Rc<crate::window::RetainedPaintContext>>,
        layout_state_range: crate::window::StateAccessBlocks,
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
    scrolled: Option<(u64, ScrollBinding)>,
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
        id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let entity_id = self.view.entity_id();
        let splice_safe = window.splice_context_supported();
        let retained_key = crate::fast::layout_key::layout_key(window);
        if let Some(key) = retained_key {
            if let Some(prebuilt) = window.take_prebuilt_gap(key, id, entity_id) {
                return prebuilt;
            }
            if let Some(splice) = window.try_splice_view(key, entity_id, id, cx) {
                return (splice.anchor.layout_root, ViewRequestLayout {
                    rebuild: splice.anchor.rebuild.clone(), splice_safe,
                    scroll_capture: None, scroll_read_start: None,
                    kind: ViewRequestLayoutKind::Spliced {
                        splice: Some(Box::new(splice)), retention: RetentionDraft::default(),
                        prepaint_range: None, context: None,
                    },
                });
            }
        }
        if let Some(key) = retained_key
            && let Some((admission, layout_claims)) = window.admit_retained_view(key, entity_id, id, cx)
        {
            let layout_root = admission.layout_root;
            let layout_state_range = window.replay_view_layout_states(&admission.layout_state_range);
            return (
                layout_root,
                ViewRequestLayout {
                    rebuild: admission.rebuild.clone(),
                    scroll_capture: None,
                    scroll_read_start: None,
                    splice_safe: splice_safe && admission.splice_safe,
                    kind: ViewRequestLayoutKind::Retained {
                        admission,
                        layout_claims,
                        layout_state_range,
                        retention: RetentionDraft::default(),
                        reused: None,
                        allow_reuse: true,
                    },
                },
            );
        }
        let rebuild = window.capture_view_rebuild(&self.view, id);
        let claim_start = window.claim_log_len();
        let element_state_start = window.begin_view_element_states();
        let type_name = cx.entities.type_name_for_id(entity_id).unwrap_or("unknown");
        let scroll_read_start = window.scroll_row_replay_is_enabled()
            .then(|| cx.entities.state_read_index());
        let previous = cx.entities.set_render_dependency_owner(Some(entity_id));
        let (layout_id, element) = window.with_rendered_view(entity_id, |window| {
            window.record_rendered_view(entity_id, type_name);
            let mut element = self.view.render_element(window, cx);
            let layout_id = element.request_layout(window, cx);
            (layout_id, element)
        });
        let _ = cx.entities.set_render_dependency_owner(previous);
        let layout_claims = LayoutClaimBlocks::request(claim_start..window.claim_log_len());
        let layout_state_range = window.view_layout_state_range(element_state_start);
        (
            layout_id,
            ViewRequestLayout {
                rebuild,
                scroll_capture: None,
                scroll_read_start,
                splice_safe,
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
        state.splice_safe &= window.splice_context_supported();
        if let ViewRequestLayoutKind::Spliced { splice, .. } = &mut state.kind {
            if !window.can_prepaint_view_splice(splice.as_ref().expect("pending splice"), bounds) {
                let anchor = window.abandon_view_splice(*splice.take().expect("pending splice"));
                state.kind = ViewRequestLayoutKind::Retained {
                    admission: anchor, layout_claims: LayoutClaimBlocks::default(), layout_state_range: crate::window::StateAccessBlocks::default(),
                    retention: RetentionDraft::default(), reused: None, allow_reuse: false,
                };
            }
        }
        window.set_view_id(entity_id);
        let prepaint_start = window.prepaint_index();
        let mut retention = Some(RetentionDraft {
            slot: crate::fast::layout_key::prepaint_layout_key(window)
                .map(|key| window.reserve_view_retention(key, entity_id)),
            nested: 0..0,
        });
        let previous = cx.entities.set_render_dependency_owner(Some(entity_id));
        window.with_rendered_view(entity_id, |window| {
            let context = Rc::new(window.current_retained_paint_context());
            let key = crate::fast::layout_key::prepaint_layout_key(window);
            match &mut state.kind {
                ViewRequestLayoutKind::Retained {
                    admission,
                    layout_claims,
                    layout_state_range,
                    reused,
                    allow_reuse,
                    ..
                } => {
                    if *allow_reuse && window.retained_view_context_matches(admission, bounds)
                        && let Some(prepaint_range) = window.reuse_retained_view_prepaint(admission)
                    {
                        *reused = Some(ReusedView { prepaint_range, context, scrolled: None });
                        return;
                    }
                    if *allow_reuse
                        && let Some(key) = key
                        && let Some((prepaint_range, binding)) = window.prepaint_scrolled_view(
                            key, entity_id, admission, &context, bounds, cx,
                        )
                    {
                        *reused = Some(ReusedView { prepaint_range, context, scrolled: Some((key, binding)) });
                        return;
                    }
                    // The committed layout no longer describes what this view inherits. Give the
                    // nodes back and draw the content into the slot the parent allocated.
                    window.release_retained_view(layout_claims);
                    window.cancel_view_layout_states(layout_state_range);
                    // A layout rebuilt in Prepaint cannot vouch for RequestLayout inheritance.
                    if window.scroll_view_translation(admission, &context, bounds, cx).is_none() {
                        state.rebuild = None;
                    }
                    state.scroll_read_start = window.scroll_row_replay_is_enabled()
                        .then(|| cx.entities.state_read_index());
                    let layout_root = admission.layout_root;
                    let claim_start = window.claim_log_len();
                    let mut element = {
                        let type_name =
                            cx.entities.type_name_for_id(entity_id).unwrap_or("unknown");
                        window.record_rendered_view(entity_id, type_name);
                        self.view.render_element(window, cx)
                    };
                    let rebuilt_root = window.with_layout_key_of_prepainting_element(|window| {
                        let rebuilt_root = element.request_layout(window, cx);
                        let available_space = crate::size(
                            crate::AvailableSpace::Definite(bounds.size.width),
                            crate::AvailableSpace::Definite(bounds.size.height),
                        );
                        if rebuilt_root == layout_root {
                            // The parent has already positioned this native node.
                            // Do not lay it out as an independent root and add its
                            // absolute position again.
                            if window.relayout_retained_root(rebuilt_root, available_space, cx) {
                                window.request_animation_frame();
                            }
                            element.prepaint(window, cx);
                        } else {
                            element.layout_as_root(available_space, window, cx);
                            let native_origin =
                                window.layout_bounds(rebuilt_root).origin - window.element_offset();
                            element.prepaint_at(bounds.origin - native_origin, window, cx);
                            window.request_animation_frame();
                        }
                        rebuilt_root
                    });
                    let mut layout_claims = LayoutClaimBlocks::prepaint(claim_start..window.claim_log_len());
                    if rebuilt_root != layout_root {
                        // The parent still references the admitted root. Do not publish
                        // a reusable record for that root after replacing its subtree.
                        layout_claims = LayoutClaimBlocks::default();
                    }
                    state.kind = ViewRequestLayoutKind::Built {
                        element,
                        layout_root,
                        layout_claims,
                        prepaint_range: Some(prepaint_start.clone()..window.prepaint_index()),
                        context: Some(context),
                        layout_state_range: crate::window::StateAccessBlocks::default(),
                        retention: retention.take().expect("view retention slot"),
                    };
                }
                ViewRequestLayoutKind::Spliced { splice, retention: draft, prepaint_range, context: recorded_context } => {
                    let splice = splice.as_mut().expect("admitted splice");
                    let start = window.claim_log_len();
                    window.prepaint_view_splice(splice, cx);
                    splice.claims.push_prepaint(start..window.claim_log_len());
                    *prepaint_range = Some(prepaint_start.clone()..window.prepaint_index());
                    *recorded_context = Some(context);
                    *draft = retention.take().expect("view retention slot");
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
                    layout_claims.push_prepaint(claim_start..window.claim_log_len());
                    *prepaint_range = Some(prepaint_start.clone()..window.prepaint_index());
                    *recorded_context = Some(context);
                    *state_retention = retention.take().expect("view retention slot");
                }
            }
            if let ViewRequestLayoutKind::Built { element, prepaint_range: Some(range), .. } = &mut state.kind
                && let Some(read_start) = state.scroll_read_start
            {
                state.scroll_capture = window.plan_scroll_view_capture(
                    key, element, range, bounds, read_start, cx,
                );
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
            ViewRequestLayoutKind::Built { retention: state_retention, .. }
            | ViewRequestLayoutKind::Spliced { retention: state_retention, .. } => {
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
        let scene_segments_start = window.next_frame.retained_scene_segments.len();
        let splice_safe = state.splice_safe;
        let rebuild = state.rebuild.clone();
        window.with_rendered_view(entity_id, |window| match &mut state.kind {
            ViewRequestLayoutKind::Retained { admission, layout_claims, layout_state_range, retention, reused, .. } => {
                let Some(reused) = reused.as_mut() else {
                    unreachable!("an admitted view is either drawn again or rebuilt in prepaint")
                };
                let Some(slot) = retention.slot.as_ref() else {
                    return;
                };
                let start = window.paint_index();
                if let Some((key, binding)) = reused.scrolled.take() {
                    window.paint_scrolled_view(key, binding, admission, &reused.context, cx);
                } else {
                    window.replay_retained_view_paint(entity_id, admission);
                }
                window.record_debug_view_cache_status(bounds, crate::window::debug_visualization::ViewCacheDebugStatus::Hit, cx);
                let end = window.paint_index();
                window.commit_view_retention(
                    slot,
                    crate::window::RetainedViewRecord {
                        layout_root: admission.layout_root,
                        layout_claims: layout_claims.clone(),
                        prepaint_range: reused.prepaint_range.clone(),
                        paint_range: start..end,
                        bounds,
                        context: reused.context.clone(),
                        rebuild: rebuild.clone(),
                        splice_safe: splice_safe && window.splice_paint_context_matches(&reused.context),
                        nested: retention.nested.clone(),
                        scene_segments: scene_segments_start..window.next_frame.retained_scene_segments.len(),
                        layout_state_range: layout_state_range.clone(),
                    },
                    Some(admission),
                );
            }
            ViewRequestLayoutKind::Spliced { splice, retention, prepaint_range, context } => {
                let splice = splice.as_mut().expect("prepainted splice");
                let start = window.paint_index();
                window.paint_view_splice(splice, entity_id, cx);
                let end = window.paint_index();
                let (Some(slot), Some(prepaint_range), Some(context)) = (retention.slot.as_ref(), prepaint_range.as_ref(), context.as_ref()) else { return; };
                window.commit_view_retention(slot, crate::window::RetainedViewRecord {
                    layout_root: splice.anchor.layout_root, layout_claims: splice.claims.clone(),
                    prepaint_range: prepaint_range.clone(), paint_range: start..end, bounds,
                    context: context.clone(), rebuild: rebuild.clone(),
                    splice_safe: splice_safe && window.splice_paint_context_matches(context),
                    nested: retention.nested.clone(),
                    scene_segments: scene_segments_start..window.next_frame.retained_scene_segments.len(),
                    layout_state_range: splice.states.clone(),
                }, None);
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
                if let (Some(capture), Some(range), Some(context)) =
                    (state.scroll_capture.take(), prepaint_range.as_ref(), context.as_ref())
                {
                    window.paint_scroll_view(*capture, entity_id, element, range, context, cx);
                } else {
                    element.paint(window, cx);
                }
                let end = window.paint_index();
                let (Some(slot), Some(prepaint_range), Some(context)) =
                    (retention.slot.as_ref(), prepaint_range.as_ref(), context.as_ref())
                else {
                    return;
                };
                window.commit_view_retention(
                    slot,
                    crate::window::RetainedViewRecord {
                        layout_root: *layout_root,
                        layout_claims: layout_claims.clone(),
                        prepaint_range: prepaint_range.clone(),
                        paint_range: start..end,
                        bounds,
                        context: context.clone(),
                        rebuild: rebuild.clone(),
                        splice_safe: splice_safe && window.splice_paint_context_matches(context),
                        nested: retention.nested.clone(),
                        scene_segments: scene_segments_start..window.next_frame.retained_scene_segments.len(),
                        layout_state_range: layout_state_range.clone(),
                    },
                    None,
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
    pub(crate) fn entity_id(&self) -> EntityId {
        self.entity.entity_id
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

    #[gpui::test]
    fn native_div_scroll_reuses_the_view_and_refreshes_click_geometry(cx: &mut crate::TestAppContext) {
        use crate::{
            InteractiveElement, MouseButton, MouseDownEvent, MouseUpEvent, ParentElement, PlatformInput,
            ScrollDelta, ScrollWheelEvent, StatefulInteractiveElement, Styled, div, point, px,
        };
        use std::cell::Cell;

        struct Page {
            builds: Rc<Cell<usize>>,
            clicks: Rc<Cell<usize>>,
        }
        impl Render for Page {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                self.builds.set(self.builds.get() + 1);
                let clicks = self.clicks.clone();
                div().relative().w(px(200.)).h(px(512.)).flex_shrink_0().child(
                    div().id("target").absolute().left(px(20.)).top(px(128.))
                        .size(px(32.)).bg(crate::red())
                        .hover(|style| style.bg(crate::green()))
                        .on_click(move |_, _, _| clicks.set(clicks.get() + 1)),
                )
            }
        }
        struct Scroller(Entity<Page>);
        impl Render for Scroller {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div().id("scroller").w(px(200.)).h(px(128.)).overflow_y_scroll()
                    .child(div().child(self.0.clone()))
            }
        }
        let builds = Rc::new(Cell::new(0));
        let clicks = Rc::new(Cell::new(0));
        let executor = cx.executor();
        let (_, cx) = cx.add_window_view(|_, cx| Scroller(cx.new(|_| Page {
            builds: builds.clone(),
            clicks: clicks.clone(),
        })));
        let wheel = |delta| PlatformInput::ScrollWheel(ScrollWheelEvent {
            position: point(px(30.), px(71.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
            ..Default::default()
        });
        cx.update(|window, app| {
            window.draw(app).clear();
            window.dispatch_event(wheel(-32.), app);
            window.draw(app).clear();
            let after_capture = builds.get();
            window.dispatch_event(wheel(-32.), app);
            window.draw(app).clear();
            assert_eq!(builds.get(), after_capture, "pure scroll must bypass Render");

            let scale = window.scale_factor();
            let scene = window.rendered_frame.scene();
            let red = scene.quads.iter().find(|quad| quad.background == crate::red().into())
                .expect("the translated button leaves hover held during a wheel sequence");
            assert_eq!(red.bounds.origin.y, px(64.).scale(scale));
        });

        cx.run_until_parked();
        executor.advance_clock(std::time::Duration::from_millis(100));
        cx.run_until_parked();
        cx.update(|window, app| {
            window.draw(app).clear();
            let scale = window.scale_factor();
            let scene = window.rendered_frame.scene();
            let green = scene.quads.iter().find(|quad| quad.background == crate::green().into())
                .expect("hover returns when the wheel sequence ends");
            assert_eq!(green.bounds.origin.y, px(64.).scale(scale));

            window.dispatch_event(wheel(-16.), app);
            window.draw(app).clear();
            let after_capture = builds.get();
            let position = point(px(30.), px(55.));
            window.dispatch_event(PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Left, position, click_count: 1,
                modifiers: Default::default(), first_mouse: false,
            }), app);
            assert!(builds.get() > after_capture, "input restores captured callbacks");
            window.dispatch_event(PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left, position, click_count: 1, modifiers: Default::default(),
            }), app);
            assert_eq!(clicks.get(), 1);
        });
    }

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
