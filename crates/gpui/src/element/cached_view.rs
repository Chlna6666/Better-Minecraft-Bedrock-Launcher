use super::any_view::{AnyView, AnyWeakView};
#[cfg(test)]
use super::any_view::ViewElement;
use super::fingerprint::render_fingerprint;
use crate::Styled;
use crate::{
    AnyElement, App, Bounds, ContentMask, Element, ElementId, Entity, EntityId, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, PaintIndex, ParentElement, Pixels, Point,
    PrepaintStateIndex, Render, Style, StyleRefinement, TextStyle, Window, div,
};
use crate::window::debug_visualization::ViewCacheDebugStatus;
use crate::window::{CachedViewTraversalContext, RetainedElementRange, ViewDirtyScope};
use collections::FxHashSet;
use refineable::Refineable;
use std::hash::Hash;
use std::mem;
use std::rc::Rc;
use std::{any::TypeId, ops::Range};

struct CachedViewState {
    weak_view: CachedWeakView,
    traversal_context: CachedViewTraversalContext,
    prepaint_range: Range<PrepaintStateIndex>,
    paint_range: Range<PaintIndex>,
    metadata_range: Range<usize>,
    cache_key: CachedViewCacheKey,
    accessed_entities: FxHashSet<EntityId>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct CachedViewPaintContext {
    opacity: f32,
    scale: f32,
    translation: Point<Pixels>,
    visual_content_mask: ContentMask<Pixels>,
}

#[derive(Default)]
struct CachedViewCacheKey {
    bounds: Bounds<Pixels>,
    content_mask: ContentMask<Pixels>,
    text_style: TextStyle,
    paint_context: CachedViewPaintContext,
    fingerprint: u64,
}


/// Opaque paint handoff used internally by CachedView reconciliation.
#[doc(hidden)]
pub struct CachedViewPrepaintState(CachedViewPrepaintStateKind);

enum CachedViewPrepaintStateKind {
    Fresh(AnyElement),
    Replay {
        source_prepaint_range: Range<PrepaintStateIndex>,
        source_paint_range: Range<PaintIndex>,
        source_metadata_range: Range<usize>,
    },
    Selective(Box<SelectiveCachedViewPatch>),
}

struct SelectiveCachedViewTarget {
    view: CachedView,
    state_global_id: GlobalElementId,
    retained_id: GlobalElementId,
    traversal_context: CachedViewTraversalContext,
    source_outer: RetainedElementRange,
}

struct SelectiveCachedViewTargetPatch {
    target_view: CachedView,
    target_global_id: GlobalElementId,
    target_retained_id: GlobalElementId,
    target_context: CachedViewTraversalContext,
    target_bounds: Bounds<Pixels>,
    target_request_layout: Option<AnyElement>,
    target_prepaint: Box<CachedViewPrepaintState>,
    source_target: RetainedElementRange,
    source_prepaint_before: Range<PrepaintStateIndex>,
    source_paint_before: Range<PaintIndex>,
    source_metadata_before: Range<usize>,
    target_prepaint_before: Range<PrepaintStateIndex>,
    target_prepaint_range: Range<PrepaintStateIndex>,
}

struct SelectiveCachedViewPatch {
    target: SelectiveCachedViewTargetPatch,
    source_prepaint_tail: Range<PrepaintStateIndex>,
    source_paint_tail: Range<PaintIndex>,
    source_metadata_tail: Range<usize>,
    target_prepaint_tail: Range<PrepaintStateIndex>,
    parent_prepaint_range: Range<PrepaintStateIndex>,
}

fn selective_cached_view_target(
    window: &Window,
    ancestor_retained_id: &GlobalElementId,
    parent_state: &CachedViewState,
) -> Option<SelectiveCachedViewTarget> {
    let (owner_id, retained_id) = window.invalidator.single_reconcile_target_below(
        ancestor_retained_id,
        parent_state.weak_view.view.entity_id(),
        &window.rendered_frame.dispatch_tree,
    )?;
    if window.view_dirty_scope(owner_id) != Some(ViewDirtyScope::Direct) {
        return None;
    }

    let source_outer = window
        .rendered_frame
        .retained_element_ranges
        .get(&retained_id)?
        .clone();
    if !source_outer.identity_stable
        || source_outer.metadata_range.start < parent_state.metadata_range.start
        || source_outer.metadata_range.end > parent_state.metadata_range.end
    {
        return None;
    }

    let state_global_id = window
        .invalidator
        .cached_view_state_global_id(owner_id, &retained_id)?;
    let boxed = window
        .rendered_frame
        .element_states
        .get(&(state_global_id.clone(), TypeId::of::<CachedViewState>()))?;
    let state = boxed
        .inner
        .downcast_ref::<Option<CachedViewState>>()?
        .as_ref()?;
    if state.weak_view.view.entity_id() != owner_id {
        return None;
    }

    Some(SelectiveCachedViewTarget {
        view: state.weak_view.upgrade()?,
        state_global_id,
        retained_id,
        traversal_context: state.traversal_context.clone(),
        source_outer,
    })
}

fn try_selective_cached_view_prepaint(
    ancestor_retained_id: &GlobalElementId,
    parent_state: &CachedViewState,
    window: &mut Window,
    cx: &mut App,
) -> Option<SelectiveCachedViewPatch> {
    let target = selective_cached_view_target(window, ancestor_retained_id, parent_state)?;
    let source = &target.source_outer;

    let source_prepaint_before =
        parent_state.prepaint_range.start.clone()..source.prepaint_range.start.clone();
    let source_paint_before =
        parent_state.paint_range.start.clone()..source.paint_range.start.clone();
    let source_metadata_before =
        parent_state.metadata_range.start..source.metadata_range.start;
    let source_prepaint_tail =
        source.prepaint_range.end.clone()..parent_state.prepaint_range.end.clone();
    let source_paint_tail =
        source.paint_range.end.clone()..parent_state.paint_range.end.clone();
    let source_metadata_tail =
        source.metadata_range.end..parent_state.metadata_range.end;

    if !window.can_splice_plain_view_target(
        &parent_state.prepaint_range,
        &source.prepaint_range,
        target.view.entity_id(),
    )
        || !window.can_reuse_prepaint_fragment(&source_prepaint_before)
        || !window.can_reuse_paint(&source_paint_before)
        || !window.can_reuse_prepaint_fragment(&source_prepaint_tail)
        || !window.can_reuse_paint(&source_paint_tail)
    {
        return None;
    }

    let context_matches =
        window.with_cached_view_traversal_context(&target.traversal_context, |window| {
            window.current_retained_element_id().as_ref() == Some(&target.retained_id)
                && window.current_retained_paint_context() == source.paint_context
        });
    if !context_matches {
        return None;
    }

    let parent_prepaint_start = window.prepaint_index();
    let mut replay = window.begin_prepaint_fragment_replay(&parent_state.prepaint_range)?;

    let target_prepaint_before_start = window.prepaint_index();
    if !window.reuse_prepaint_fragment(source_prepaint_before.clone(), &mut replay) {
        window.degrade_current_draw();
        return None;
    }
    let target_prepaint_before_end = window.prepaint_index();

    let target_prepaint_start = window.prepaint_index();
    if !window.begin_fresh_view_dispatch_for_fragment(
        &source.prepaint_range,
        target.view.entity_id(),
        &mut replay,
    ) {
        window.degrade_current_draw();
        return None;
    }

    let source_target = target.source_outer;
    let mut target_view = target.view;
    let mut target_request_layout = None;
    let target_bounds = source_target.bounds;
    let target_prepaint =
        window.with_cached_view_traversal_context(&target.traversal_context, |window| {
            target_view.prepaint(
                Some(&target.state_global_id),
                None,
                target_bounds,
                &mut target_request_layout,
                window,
                cx,
            )
        });
    let target_prepaint_end = window.prepaint_index();
    window.finish_fresh_view_dispatch_for_fragment(&replay);

    let target_patch = SelectiveCachedViewTargetPatch {
        target_view,
        target_global_id: target.state_global_id,
        target_retained_id: target.retained_id,
        target_context: target.traversal_context,
        target_bounds,
        target_request_layout,
        target_prepaint: Box::new(target_prepaint),
        source_target,
        source_prepaint_before,
        source_paint_before,
        source_metadata_before,
        target_prepaint_before: target_prepaint_before_start..target_prepaint_before_end,
        target_prepaint_range: target_prepaint_start..target_prepaint_end,
    };

    let target_prepaint_tail_start = window.prepaint_index();
    if !window.reuse_prepaint_fragment(source_prepaint_tail.clone(), &mut replay) {
        window.degrade_current_draw();
        return None;
    }
    let target_prepaint_tail_end = window.prepaint_index();

    cx.entities.extend_accessed(&parent_state.accessed_entities);

    Some(SelectiveCachedViewPatch {
        target: target_patch,
        source_prepaint_tail,
        source_paint_tail,
        source_metadata_tail,
        target_prepaint_tail: target_prepaint_tail_start..target_prepaint_tail_end.clone(),
        parent_prepaint_range: parent_prepaint_start..target_prepaint_tail_end,
    })
}


/// A weak handle to an explicit cached view boundary.
///
/// The handle preserves cache policy needed to reconstruct the boundary if the backing view
/// remains alive, without keeping that entity alive by itself.
#[derive(Clone)]
pub struct CachedWeakView {
    view: AnyWeakView,
    style: Rc<StyleRefinement>,
    fingerprint: u64,
    progressive: bool,
    critical: bool,
    reuse_on_window_refresh: bool,
}

impl CachedWeakView {
    /// Upgrade this weak cached-view handle while the backing view is still alive.
    pub fn upgrade(&self) -> Option<CachedView> {
        Some(CachedView {
            view: self.view.upgrade()?,
            style: self.style.clone(),
            fingerprint: self.fingerprint,
            progressive: self.progressive,
            critical: self.critical,
            reuse_on_window_refresh: self.reuse_on_window_refresh,
        })
    }
}

/// Explicit retained/cache boundary for a reactive view.
///
/// Ordinary `AnyView` and `Entity<V>` values do not carry any of this policy.
pub struct CachedView {
    view: AnyView,
    style: Rc<StyleRefinement>,
    fingerprint: u64,
    progressive: bool,
    critical: bool,
    reuse_on_window_refresh: bool,
}

impl CachedView {
    fn new(view: AnyView, style: StyleRefinement, fingerprint: Option<u64>) -> Self {
        let fingerprint = fingerprint.unwrap_or_else(|| {
            render_fingerprint(&(view.entity_type(), view.entity_id().as_u64()))
        });
        Self {
            view,
            style: Rc::new(style),
            fingerprint,
            progressive: false,
            critical: false,
            reuse_on_window_refresh: false,
        }
    }

    /// Permit deferred dirty reuse when the frame budget is exhausted.
    ///
    /// The previous retained subtree may be reused for the current frame while the view remains
    /// dirty so a later frame can complete the rebuild.
    pub fn progressive(mut self) -> Self {
        self.progressive = true;
        self
    }

    /// Permit reuse across an ordinary window refresh when this cache key remains stable and the
    /// backing view itself is not dirty.
    pub fn reuse_on_window_refresh(mut self) -> Self {
        self.reuse_on_window_refresh = true;
        self
    }

    /// Mark this boundary as critical so it is rebuilt rather than progressively deferred when
    /// the draw budget is exhausted.
    pub fn critical(mut self) -> Self {
        self.critical = true;
        self
    }

    /// Convert this cached boundary to a weak handle that does not keep its backing entity alive.
    pub fn downgrade(&self) -> CachedWeakView {
        CachedWeakView {
            view: self.view.downgrade(),
            style: self.style.clone(),
            fingerprint: self.fingerprint,
            progressive: self.progressive,
            critical: self.critical,
            reuse_on_window_refresh: self.reuse_on_window_refresh,
        }
    }

    #[inline]
    fn entity_id(&self) -> EntityId {
        self.view.entity_id()
    }

    #[inline]
    fn cache_fingerprint(&self) -> u64 {
        self.fingerprint
    }
}

impl IntoElement for CachedView {
    type Element = Self;

    #[inline]
    fn into_element(self) -> Self::Element {
        self
    }
}

impl AnyView {
    /// Create an explicit retained/cache boundary around this view.
    pub fn cached(self, style: StyleRefinement) -> CachedView {
        CachedView::new(self, style, None)
    }

    /// Create an explicit cached boundary using a caller-supplied stable subtree fingerprint.
    pub fn cached_with_fingerprint(
        self,
        style: StyleRefinement,
        fingerprint: u64,
    ) -> CachedView {
        CachedView::new(self, style, Some(fingerprint))
    }

    /// Create an explicit cached boundary whose stable fingerprint is derived from `key`.
    pub fn cached_by<K: Hash + ?Sized>(self, style: StyleRefinement, key: &K) -> CachedView {
        self.cached_with_fingerprint(style, render_fingerprint(key))
    }

    /// Create a full-inset absolute cached boundary whose fingerprint is derived from `key`.
    pub fn cached_absolute_by<K: Hash + ?Sized>(self, key: &K) -> CachedView {
        self.cached_by(StyleRefinement::default().absolute().inset_0(), key)
    }
}

impl<V: Render> Entity<V> {
    /// Wrap this entity in an explicit cached boundary laid out using `style`.
    pub fn cached(self, style: StyleRefinement) -> CachedView {
        AnyView::from(self).cached(style)
    }

    /// Wrap this entity in an explicit cached boundary using a caller-supplied stable fingerprint.
    pub fn cached_with_fingerprint(
        self,
        style: StyleRefinement,
        fingerprint: u64,
    ) -> CachedView {
        AnyView::from(self).cached_with_fingerprint(style, fingerprint)
    }

    /// Wrap this entity in an explicit cached boundary whose fingerprint is derived from `key`.
    pub fn cached_by<K: Hash + ?Sized>(self, style: StyleRefinement, key: &K) -> CachedView {
        AnyView::from(self).cached_by(style, key)
    }

    /// Wrap this entity in a full-inset absolute cached boundary keyed by `key`.
    pub fn cached_absolute_by<K: Hash + ?Sized>(self, key: &K) -> CachedView {
        AnyView::from(self).cached_absolute_by(key)
    }
}

impl Element for CachedView {
    type RequestLayoutState = Option<AnyElement>;
    type PrepaintState = CachedViewPrepaintState;
    const RETAINED_REPLAY_CAPABILITY: crate::RetainedReplayCapability =
        crate::RetainedReplayCapability::OwnsFrameLocalCacheBoundary;

    fn id(&self) -> Option<ElementId> {
        Some(ElementId::View(self.entity_id()))
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
        window.with_rendered_view(self.entity_id(), |window| {
            if !window.is_inspector_picking(cx) {
                let mut root_style = Style::default();
                root_style.refine(self.style.as_ref());
                let layout_id = window.request_layout(root_style, None, cx);
                (layout_id, None)
            } else {
                window.record_rendered_view(
                    self.entity_id(),
                    cx.entities.type_name_for_id(self.entity_id()).unwrap_or("unknown"),
                );
                let mut element = self.view.render_element(window, cx);
                let layout_id = element.request_layout(window, cx);
                (layout_id, Some(element))
            }
        })
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        window.set_view_id(self.entity_id());
        let retained_id = window.current_retained_element_id();
        let traversal_context = window.capture_cached_view_traversal_context();
        if let Some(retained_id) = retained_id.as_ref()
            && let Some(state_global_id) = global_id
        {
            window.invalidator.register_cached_view_retained_target(
                self.entity_id(),
                retained_id,
                state_global_id,
            );
        }
        window.with_rendered_view(self.entity_id(), |window| {
            if let Some(mut element) = element.take() {
                element.prepaint(window, cx);
                return CachedViewPrepaintState(CachedViewPrepaintStateKind::Fresh(element));
            }

            window.with_element_state::<CachedViewState, _>(
                global_id.unwrap(),
                |mut element_state, window| {
                    let retained_id = retained_id
                        .clone()
                        .expect("CachedView must have a retained identity");
                    let traversal_context = traversal_context.clone();
                    if let Some(state) = element_state.as_mut() {
                        state.weak_view = self.downgrade();
                        state.traversal_context = traversal_context.clone();
                    }

                    let content_mask = window.content_mask();
                    let text_style = window.text_style();
                    let paint_context = CachedViewPaintContext {
                        opacity: window.element_opacity(),
                        scale: window.element_visual_transform.scale,
                        translation: window.element_visual_transform.translation,
                        visual_content_mask: window.visual_content_mask(),
                    };

                    let cache_fingerprint = self.cache_fingerprint();
                    let dirty_scope = window.view_dirty_scope(self.entity_id());
                    let view_dirty = dirty_scope.is_some();
                    let force_refresh = window.force_view_cache_refresh();
                    let targeted_replay = window.retained_replay_is_targeted();
                    // Progressive reuse is appropriate for ordinary/background dirty work, but an
                    // exact retained target is an interactive sample that must reach the current
                    // frame. Replaying the previous dirty view here creates a visible plateau while
                    // animation time keeps advancing, followed by a large position/opacity jump on
                    // the next frame that is allowed to rebuild.
                    let can_defer_dirty_view = view_dirty
                        && self.progressive
                        && !self.critical
                        && !targeted_replay
                        && window.draw_budget_exhausted();
                    // `reuse_on_window_refresh` is only safe for ordinary refreshes. A degraded draw
                    // moves frame-local cache state through `next_frame` before discarding that frame;
                    // its numeric ranges can remain in-bounds while referring to different primitives
                    // in the last committed frame. The recovery frame is therefore a hard cache barrier.
                    let can_reuse_refresh = self.reuse_on_window_refresh
                        && !self.critical
                        && !window.recovering_degraded_draw()
                        && force_refresh
                        && !view_dirty;
                    let can_reuse_prepaint = element_state
                        .as_ref()
                        .is_some_and(|state| window.can_reuse_prepaint(&state.prepaint_range));
                    let can_reuse_paint = element_state
                        .as_ref()
                        .is_some_and(|state| window.can_reuse_paint(&state.paint_range));

                    let stable_cache_key = element_state.as_ref().is_some_and(|state| {
                        state.cache_key.bounds == bounds
                            && state.cache_key.content_mask == content_mask
                            && state.cache_key.paint_context == paint_context
                            && state.cache_key.text_style == text_style
                            && state.cache_key.fingerprint == cache_fingerprint
                    });
                    // Selective splice is a workload-reduction path, not optional progressive
                    // work. Do not disable it merely because the generation deadline has already
                    // elapsed: falling back to rendering the traversal ancestor is strictly broader
                    // work and creates a deadline-miss -> full-render -> deadline-miss feedback loop.
                    // Recovery/forced refresh remain hard barriers because their retained ranges may
                    // no longer be trustworthy.
                    let selective_candidate =
                        dirty_scope == Some(ViewDirtyScope::TraversalAncestor)
                            && stable_cache_key
                            && !force_refresh
                            && !window.recovering_degraded_draw();

                    if dirty_scope == Some(ViewDirtyScope::TraversalAncestor)
                        && !selective_candidate
                        && log::log_enabled!(log::Level::Trace)
                    {
                        log::trace!(
                            "gpui selective splice blocked: view={} type={} stable_cache_key={} force_refresh={} recovering_degraded={} budget_exhausted={} targeted_replay={} active_targets={} generic_dirty_views={}",
                            self.entity_id().as_u64(),
                            cx.entities.type_name_for_id(self.entity_id()).unwrap_or("unknown"),
                            stable_cache_key,
                            force_refresh,
                            window.recovering_degraded_draw(),
                            window.draw_budget_exhausted(),
                            targeted_replay,
                            window.invalidator.active_targeted_element_count(),
                            window.invalidator.active_generic_dirty_view_count()
                        );
                    }

                    if selective_candidate {
                        window.record_selective_splice_attempt();
                    }
                    if selective_candidate
                        && let Some(state) = element_state.as_ref()
                        && let Some(patch) = try_selective_cached_view_prepaint(
                            &retained_id,
                            state,
                            window,
                            cx,
                        )
                    {
                        window.record_selective_splice_hit();
                        window.record_debug_view_cache_status(
                            bounds,
                            ViewCacheDebugStatus::Hit,
                            cx,
                        );
                        let mut state = element_state
                            .take()
                            .expect("selective traversal requires an existing cache state");
                        state.prepaint_range = patch.parent_prepaint_range.clone();
                        return (
                            CachedViewPrepaintState(CachedViewPrepaintStateKind::Selective(Box::new(patch))),
                            state,
                        );
                    }
                    if selective_candidate && log::log_enabled!(log::Level::Trace) {
                        log::trace!(
                            "gpui selective splice plan miss: view={} type={} retained_id={} budget_exhausted={} targeted_replay={} active_targets={} generic_dirty_views={}",
                            self.entity_id().as_u64(),
                            cx.entities.type_name_for_id(self.entity_id()).unwrap_or("unknown"),
                            retained_id,
                            window.draw_budget_exhausted(),
                            targeted_replay,
                            window.invalidator.active_targeted_element_count(),
                            window.invalidator.active_generic_dirty_view_count()
                        );
                    }

                    let cache_debug_status = match element_state.as_ref() {
                        None => ViewCacheDebugStatus::MissCold,
                        Some(state) if state.cache_key.bounds != bounds => {
                            ViewCacheDebugStatus::MissBounds
                        }
                        Some(state) if state.cache_key.content_mask != content_mask => {
                            ViewCacheDebugStatus::MissContentMask
                        }
                        Some(state) if state.cache_key.paint_context != paint_context => {
                            ViewCacheDebugStatus::MissContentMask
                        }
                        Some(state) if state.cache_key.text_style != text_style => {
                            ViewCacheDebugStatus::MissTextStyle
                        }
                        Some(state) if state.cache_key.fingerprint != cache_fingerprint => {
                            ViewCacheDebugStatus::MissFingerprint
                        }
                        Some(_) if force_refresh && !can_reuse_refresh && !can_defer_dirty_view => {
                            ViewCacheDebugStatus::MissRefresh
                        }
                        Some(_)
                            if dirty_scope == Some(ViewDirtyScope::TraversalAncestor)
                                && !can_defer_dirty_view =>
                        {
                            ViewCacheDebugStatus::MissTraversalAncestor
                        }
                        Some(_) if view_dirty && !can_defer_dirty_view => {
                            ViewCacheDebugStatus::MissDirty
                        }
                        Some(_) if !can_reuse_prepaint => {
                            ViewCacheDebugStatus::MissPrepaintRange
                        }
                        Some(_) if !can_defer_dirty_view && !can_reuse_paint => {
                            ViewCacheDebugStatus::MissPaintRange
                        }
                        Some(_) if can_defer_dirty_view => {
                            ViewCacheDebugStatus::DeferredDirtyReuse
                        }
                        Some(_) => ViewCacheDebugStatus::Hit,
                    };
                    window.record_debug_view_cache_status(bounds, cache_debug_status, cx);

                    if let Some(mut element_state) = element_state
                        && element_state.cache_key.bounds == bounds
                        && element_state.cache_key.content_mask == content_mask
                        && element_state.cache_key.paint_context == paint_context
                        && element_state.cache_key.text_style == text_style
                        && element_state.cache_key.fingerprint == cache_fingerprint
                        && (!force_refresh || can_reuse_refresh || can_defer_dirty_view)
                        && (!view_dirty || can_defer_dirty_view)
                        && can_reuse_prepaint
                        && (can_defer_dirty_view || can_reuse_paint)
                    {
                        if can_defer_dirty_view {
                            window.degrade_current_draw();
                        }
                        let source_prepaint_range = element_state.prepaint_range.clone();
                        let source_paint_range = element_state.paint_range.clone();
                        let source_metadata_range = element_state.metadata_range.clone();
                        let prepaint_start = window.prepaint_index();
                        if !window.reuse_prepaint(source_prepaint_range.clone()) {
                            window.record_debug_view_cache_status(
                                bounds,
                                ViewCacheDebugStatus::ReuseFailed,
                                cx,
                            );
                            window.degrade_current_draw();
                            return (
                                CachedViewPrepaintState(CachedViewPrepaintStateKind::Replay {
                                    source_prepaint_range,
                                    source_paint_range,
                                    source_metadata_range,
                                }),
                                element_state,
                            );
                        }
                        cx.entities
                            .extend_accessed(&element_state.accessed_entities);
                        let prepaint_end = window.prepaint_index();
                        if !window.draw_was_degraded() {
                            element_state.prepaint_range = prepaint_start..prepaint_end;
                        }

                        return (
                            CachedViewPrepaintState(CachedViewPrepaintStateKind::Replay {
                                source_prepaint_range,
                                source_paint_range,
                                source_metadata_range,
                            }),
                            element_state,
                        );
                    }
                    let refreshing = mem::replace(&mut window.refreshing, true);
                    let prepaint_start = window.prepaint_index();
                    window.record_rendered_view(
                        self.entity_id(),
                        cx.entities.type_name_for_id(self.entity_id()).unwrap_or("unknown"),
                    );
                    let (mut element, accessed_entities) = cx.detect_accessed_entities(|cx| {
                        let mut element = div()
                            .relative()
                            .size_full()
                            .child(self.view.render_element(window, cx))
                            .into_any_element();
                        element.layout_as_root(bounds.size.into(), window, cx);
                        element.prepaint_at(bounds.origin, window, cx);
                        element
                    });

                    let prepaint_end = window.prepaint_index();
                    window.refreshing = refreshing;

                    (
                        CachedViewPrepaintState(CachedViewPrepaintStateKind::Fresh(element)),
                        CachedViewState {
                            weak_view: self.downgrade(),
                            traversal_context,
                            accessed_entities,
                            prepaint_range: prepaint_start..prepaint_end,
                            paint_range: PaintIndex::default()..PaintIndex::default(),
                            metadata_range: 0..0,
                            cache_key: CachedViewCacheKey {
                                bounds,
                                content_mask,
                                text_style,
                                paint_context,
                                fingerprint: cache_fingerprint,
                            },
                        },
                    )
                },
            )
        })
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        element: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_rendered_view(self.entity_id(), |window| {
            let caching_disabled = window.is_inspector_picking(cx);
            if !caching_disabled {
                window.with_element_state::<CachedViewState, _>(
                    global_id.unwrap(),
                    |element_state, window| {
                        let mut element_state = element_state.unwrap();

                        let metadata_start = window.retained_element_metadata_len();
                        let paint_start = window.paint_index();

                        match &mut element.0 {
                            CachedViewPrepaintStateKind::Fresh(element) => {
                                // Cached view missed and is forwarding current work to its rendered
                                // child. The CachedView wrapper itself owns no scene primitives.
                                window.record_debug_element_traversal_only(bounds, cx);
                                let refreshing = mem::replace(&mut window.refreshing, true);
                                element.paint(window, cx);
                                window.refreshing = refreshing;
                            }
                            CachedViewPrepaintStateKind::Replay {
                                source_prepaint_range,
                                source_paint_range,
                                source_metadata_range,
                            } => {
                                // Full cached subtree replay: replace Drawable's provisional red
                                // marker with a retained-green marker before copying prior ranges.
                                window.record_debug_element_self_scene_replay(bounds, cx);
                                if !window.reuse_paint((*source_paint_range).clone()) {
                                    window.record_debug_view_cache_status(
                                        bounds,
                                        ViewCacheDebugStatus::ReuseFailed,
                                        cx,
                                    );
                                    window.degrade_current_draw();
                                } else {
                                    let target_paint = paint_start.clone()..window.paint_index();
                                    if source_metadata_range.start != source_metadata_range.end
                                        && !window.replay_retained_element_metadata(
                                            &*source_prepaint_range,
                                            &*source_paint_range,
                                            &*source_metadata_range,
                                            &element_state.prepaint_range,
                                            &target_paint,
                                        )
                                    {
                                        window.record_debug_view_cache_status(
                                            bounds,
                                            ViewCacheDebugStatus::ReuseFailed,
                                            cx,
                                        );
                                        window.degrade_current_draw();
                                    }
                                }
                            }
                            CachedViewPrepaintStateKind::Selective(patch) => {
                                window.record_debug_element_traversal_only(bounds, cx);

                                let target = &mut patch.target;
                                let before_paint_start = window.paint_index();
                                if !window.reuse_paint(target.source_paint_before.clone()) {
                                    window.degrade_current_draw();
                                }
                                let before_paint_end = window.paint_index();
                                let target_paint_before =
                                    before_paint_start..before_paint_end.clone();
                                if !window.replay_retained_element_metadata_fragment(
                                    &target.source_prepaint_before,
                                    &target.source_paint_before,
                                    &target.source_metadata_before,
                                    &target.target_prepaint_before,
                                    &target_paint_before,
                                ) {
                                    window.degrade_current_draw();
                                }

                                let target_paint_start = before_paint_end;
                                let target_metadata_start =
                                    window.retained_element_metadata_len();
                                if !window.activate_reconciled_view_dispatch(
                                    target.target_view.entity_id(),
                                ) {
                                    window.degrade_current_draw();
                                }
                                let retained_ok = window.with_cached_view_traversal_context(
                                    &target.target_context,
                                    |window| {
                                        target.target_view.paint(
                                            Some(&target.target_global_id),
                                            None,
                                            target.target_bounds,
                                            &mut target.target_request_layout,
                                            target.target_prepaint.as_mut(),
                                            window,
                                            cx,
                                        );
                                        let target_paint_end = window.paint_index();
                                        window.record_reconciled_cached_view_boundary(
                                            target.target_retained_id.clone(),
                                            &target.source_target,
                                            target.target_bounds,
                                            target.target_prepaint_range.clone(),
                                            target_paint_start.clone()..target_paint_end,
                                            target_metadata_start,
                                        )
                                    },
                                );
                                if !retained_ok {
                                    window.degrade_current_draw();
                                }

                                let tail_paint_start = window.paint_index();
                                if !window.reuse_paint(patch.source_paint_tail.clone()) {
                                    window.degrade_current_draw();
                                }
                                let tail_paint_end = window.paint_index();
                                let target_paint_tail =
                                    tail_paint_start..tail_paint_end;
                                if !window.replay_retained_element_metadata_fragment(
                                    &patch.source_prepaint_tail,
                                    &patch.source_paint_tail,
                                    &patch.source_metadata_tail,
                                    &patch.target_prepaint_tail,
                                    &target_paint_tail,
                                ) {
                                    window.degrade_current_draw();
                                }
                            }
                        }

                        let paint_end = window.paint_index();
                        if !window.draw_was_degraded() {
                            element_state.paint_range = paint_start..paint_end;
                            element_state.metadata_range =
                                metadata_start..window.retained_element_metadata_len();
                        }

                        ((), element_state)
                    },
                )
            } else {
                window.record_debug_element_traversal_only(bounds, cx);
                match &mut element.0 {
                    CachedViewPrepaintStateKind::Fresh(element) => element.paint(window, cx),
                    CachedViewPrepaintStateKind::Replay { .. }
                    | CachedViewPrepaintStateKind::Selective(_) => {
                        unreachable!("inspector cache bypass must carry fresh prepaint state")
                    }
                }
            }
        });
    }
}


#[cfg(test)]
mod retained_boundary_tests;
#[cfg(test)]
mod tests;
