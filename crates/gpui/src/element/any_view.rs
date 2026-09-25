use super::fingerprint::render_fingerprint;
use crate::Styled;
use crate::{
    AnyElement, AnyEntity, AnyWeakEntity, App, Bounds, ContentMask, Context, Element, ElementId,
    Entity, EntityId, GlobalElementId, InspectorElementId, IntoElement, LayoutId, PaintIndex,
    ParentElement, Pixels, Point, PrepaintStateIndex, Render, Style, StyleRefinement, TextStyle,
    WeakEntity, div,
};
use crate::{Empty, Window};
use crate::window::{CachedViewTraversalContext, RetainedElementRange, ViewDirtyScope};
use crate::window::debug_visualization::ViewCacheDebugStatus;
use anyhow::Result;
use collections::FxHashSet;
use refineable::Refineable;
use smallvec::SmallVec;
use std::hash::Hash;
use std::mem;
use std::rc::Rc;
use std::{any::TypeId, fmt, ops::Range};

struct AnyViewState {
    owner_id: EntityId,
    retained_id: GlobalElementId,
    weak_view: AnyWeakView,
    traversal_context: CachedViewTraversalContext,
    prepaint_range: Range<PrepaintStateIndex>,
    paint_range: Range<PaintIndex>,
    metadata_range: Range<usize>,
    replay_source_prepaint_range: Option<Range<PrepaintStateIndex>>,
    replay_source_paint_range: Option<Range<PaintIndex>>,
    replay_source_metadata_range: Option<Range<usize>>,
    cache_key: ViewCacheKey,
    accessed_entities: FxHashSet<EntityId>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct ViewPaintContext {
    opacity: f32,
    scale: f32,
    translation: Point<Pixels>,
    visual_content_mask: ContentMask<Pixels>,
}

#[derive(Default)]
struct ViewCacheKey {
    bounds: Bounds<Pixels>,
    content_mask: ContentMask<Pixels>,
    text_style: TextStyle,
    paint_context: ViewPaintContext,
    fingerprint: Option<u64>,
}

impl<V: Render> Element for Entity<V> {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

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
        let entity_id = self.entity_id();
        window.record_rendered_view(entity_id, std::any::type_name::<V>());
        let element = self.update(cx, |view, cx| view.render(window, cx).into_any_element());
        request_layout_rendered_entity(entity_id, element, window, cx)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        prepaint_rendered_entity(self.entity_id(), element, window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        paint_rendered_entity(self.entity_id(), bounds, element, window, cx);
    }
}

#[inline(never)]
fn request_layout_rendered_entity(
    entity_id: EntityId,
    mut element: AnyElement,
    window: &mut Window,
    cx: &mut App,
) -> (LayoutId, AnyElement) {
    let layout_id =
        window.with_rendered_view(entity_id, |window| element.request_layout(window, cx));
    (layout_id, element)
}

#[inline(never)]
fn prepaint_rendered_entity(
    entity_id: EntityId,
    element: &mut AnyElement,
    window: &mut Window,
    cx: &mut App,
) {
    window.set_view_id(entity_id);
    window.with_rendered_view(entity_id, |window| element.prepaint(window, cx));
}

#[inline(never)]
fn paint_rendered_entity(
    entity_id: EntityId,
    bounds: Bounds<Pixels>,
    element: &mut AnyElement,
    window: &mut Window,
    cx: &mut App,
) {
    // Entity is a lifecycle proxy. Its rendered element owns all actual scene primitives.
    window.record_debug_element_traversal_only(bounds, cx);
    window.with_rendered_view(entity_id, |window| element.paint(window, cx));
}

/// A dynamically-typed handle to a view, which can be downcast to an Entity for a specific type.
#[derive(Clone, Debug)]
pub struct AnyView {
    entity: AnyEntity,
    render: fn(&AnyView, &mut Window, &mut App) -> AnyElement,
    cached_style: Option<Rc<StyleRefinement>>,
    cache_fingerprint: Option<u64>,
    progressive: bool,
    critical: bool,
    reuse_on_window_refresh: bool,
}

impl<V: Render> From<Entity<V>> for AnyView {
    fn from(value: Entity<V>) -> Self {
        AnyView {
            entity: value.into_any(),
            render: any_view::render::<V>,
            cached_style: None,
            cache_fingerprint: None,
            progressive: false,
            critical: false,
            reuse_on_window_refresh: false,
        }
    }
}

fn with_optional_critical_draw<R>(
    _critical: bool,
    window: &mut Window,
    f: impl FnOnce(&mut Window) -> R,
) -> R {
    f(window)
}

/// Opaque paint handoff used internally by cached AnyView reconciliation.
#[doc(hidden)]
pub struct AnyViewPrepaintState(AnyViewPrepaintStateKind);

enum AnyViewPrepaintStateKind {
    Fresh(AnyElement),
    Replay,
    Selective(Box<SelectiveAnyViewPatch>),
}

struct SelectiveAnyViewTarget {
    view: AnyView,
    state_global_id: GlobalElementId,
    retained_id: GlobalElementId,
    traversal_context: CachedViewTraversalContext,
    source_outer: RetainedElementRange,
}

struct SelectiveAnyViewTargetPatch {
    target_view: AnyView,
    target_global_id: GlobalElementId,
    target_retained_id: GlobalElementId,
    target_context: CachedViewTraversalContext,
    target_bounds: Bounds<Pixels>,
    target_request_layout: Option<AnyElement>,
    target_prepaint: Box<AnyViewPrepaintState>,
    source_target: RetainedElementRange,
    source_prepaint_before: Range<PrepaintStateIndex>,
    source_paint_before: Range<PaintIndex>,
    source_metadata_before: Range<usize>,
    target_prepaint_before: Range<PrepaintStateIndex>,
    target_prepaint_range: Range<PrepaintStateIndex>,
}

struct SelectiveAnyViewPatch {
    targets: SmallVec<[SelectiveAnyViewTargetPatch; 4]>,
    source_prepaint_tail: Range<PrepaintStateIndex>,
    source_paint_tail: Range<PaintIndex>,
    source_metadata_tail: Range<usize>,
    target_prepaint_tail: Range<PrepaintStateIndex>,
    parent_prepaint_range: Range<PrepaintStateIndex>,
}

fn selective_any_view_targets(
    window: &Window,
    ancestor_retained_id: &GlobalElementId,
    parent_state: &AnyViewState,
) -> Option<SmallVec<[SelectiveAnyViewTarget; 4]>> {
    let raw_targets = window.invalidator.reconcile_targets_below(
        ancestor_retained_id,
        parent_state.owner_id,
        &window.rendered_frame.dispatch_tree,
    )?;

    // Temporarily keep selective splice single-target only. Multi-target scene surgery was added
    // after the last known-good rendering baseline and is much harder to prove correct across
    // blur/composite/image capture boundaries. Multiple dirty targets fall back to ordinary fresh
    // ancestor rendering; single-target retained reconciliation remains enabled.
    if raw_targets.len() != 1 {
        return None;
    }

    let mut targets = SmallVec::<[SelectiveAnyViewTarget; 4]>::new();

    for (owner_id, retained_id) in raw_targets {
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
            .get(&(state_global_id.clone(), TypeId::of::<AnyViewState>()))?;
        let state = boxed
            .inner
            .downcast_ref::<Option<AnyViewState>>()?
            .as_ref()?;
        if state.owner_id != owner_id || state.retained_id != retained_id {
            return None;
        }

        let view = state.weak_view.upgrade()?;
        if view.cached_style.is_none() {
            return None;
        }
        targets.push(SelectiveAnyViewTarget {
            view,
            state_global_id,
            retained_id,
            traversal_context: state.traversal_context.clone(),
            source_outer,
        });
    }

    // Retained metadata is post-order and every subtree owns one contiguous interval. Sort outer
    // ranges before descendants that share a start, then collapse nested direct-dirty targets: a
    // fresh outer View render will naturally rebuild any directly dirty cached descendant inside it.
    targets.sort_by(|left, right| {
        left.source_outer
            .metadata_range
            .start
            .cmp(&right.source_outer.metadata_range.start)
            .then_with(|| {
                right
                    .source_outer
                    .metadata_range
                    .end
                    .cmp(&left.source_outer.metadata_range.end)
            })
    });

    let mut disjoint = SmallVec::<[SelectiveAnyViewTarget; 4]>::new();
    for target in targets {
        if let Some(previous) = disjoint.last() {
            let previous_range = &previous.source_outer.metadata_range;
            let target_range = &target.source_outer.metadata_range;
            if target_range.start >= previous_range.start
                && target_range.end <= previous_range.end
            {
                continue;
            }
            if target_range.start < previous_range.end {
                // Retained subtree intervals should be nested or disjoint. Partial overlap means
                // provenance is inconsistent, so do not risk replaying stale frame-local state.
                return None;
            }
        }
        disjoint.push(target);
    }

    (!disjoint.is_empty()).then_some(disjoint)
}

fn try_selective_any_view_prepaint(
    ancestor_retained_id: &GlobalElementId,
    parent_state: &AnyViewState,
    window: &mut Window,
    cx: &mut App,
) -> Option<SelectiveAnyViewPatch> {
    let targets = selective_any_view_targets(window, ancestor_retained_id, parent_state)?;

    // Validate the entire splice plan before moving any frame-local listeners/state out of the
    // committed frame. Each source gap must be independently replayable and every fresh target
    // must preserve the paint/traversal context captured when it was originally mounted.
    let mut source_prepaint_cursor = parent_state.prepaint_range.start.clone();
    let mut source_paint_cursor = parent_state.paint_range.start.clone();
    let mut source_metadata_cursor = parent_state.metadata_range.start;
    for target in &targets {
        let source = &target.source_outer;
        if source_metadata_cursor > source.metadata_range.start {
            return None;
        }
        let source_prepaint_before =
            source_prepaint_cursor.clone()..source.prepaint_range.start.clone();
        let source_paint_before =
            source_paint_cursor.clone()..source.paint_range.start.clone();

        if !window.can_splice_plain_view_target(
            &parent_state.prepaint_range,
            &source.prepaint_range,
            target.view.entity_id(),
        )
            || !window.can_reuse_prepaint_fragment(&source_prepaint_before)
            || !window.can_reuse_paint(&source_paint_before)
        {
            return None;
        }

        let context_matches = window.with_cached_view_traversal_context(
            &target.traversal_context,
            |window| {
                window.current_retained_element_id().as_ref() == Some(&target.retained_id)
                    && window.current_retained_paint_context() == source.paint_context
            },
        );
        if !context_matches {
            return None;
        }

        source_prepaint_cursor = source.prepaint_range.end.clone();
        source_paint_cursor = source.paint_range.end.clone();
        source_metadata_cursor = source.metadata_range.end;
    }

    let source_prepaint_tail =
        source_prepaint_cursor.clone()..parent_state.prepaint_range.end.clone();
    let source_paint_tail =
        source_paint_cursor.clone()..parent_state.paint_range.end.clone();
    if source_metadata_cursor > parent_state.metadata_range.end
        || !window.can_reuse_prepaint_fragment(&source_prepaint_tail)
        || !window.can_reuse_paint(&source_paint_tail)
    {
        return None;
    }

    let parent_prepaint_start = window.prepaint_index();
    let mut replay = window.begin_prepaint_fragment_replay(&parent_state.prepaint_range)?;
    let mut patches = SmallVec::<[SelectiveAnyViewTargetPatch; 4]>::new();
    source_prepaint_cursor = parent_state.prepaint_range.start.clone();
    source_paint_cursor = parent_state.paint_range.start.clone();
    source_metadata_cursor = parent_state.metadata_range.start;

    for target in targets {
        let source_target = target.source_outer.clone();
        let source_prepaint_before =
            source_prepaint_cursor.clone()..source_target.prepaint_range.start.clone();
        let source_paint_before =
            source_paint_cursor.clone()..source_target.paint_range.start.clone();
        let source_metadata_before =
            source_metadata_cursor..source_target.metadata_range.start;

        let target_prepaint_before_start = window.prepaint_index();
        if !window.reuse_prepaint_fragment(source_prepaint_before.clone(), &mut replay) {
            window.degrade_current_draw();
            return None;
        }
        let target_prepaint_before_end = window.prepaint_index();

        let target_prepaint_start = window.prepaint_index();
        if !window.begin_fresh_view_dispatch_for_fragment(
            &source_target.prepaint_range,
            target.view.entity_id(),
            &mut replay,
        ) {
            window.degrade_current_draw();
            return None;
        }

        let mut target_view = target.view;
        let mut target_request_layout = None;
        let target_bounds = source_target.bounds;
        let target_prepaint = window.with_cached_view_traversal_context(
            &target.traversal_context,
            |window| {
                target_view.prepaint(
                    Some(&target.state_global_id),
                    None,
                    target_bounds,
                    &mut target_request_layout,
                    window,
                    cx,
                )
            },
        );
        let target_prepaint_end = window.prepaint_index();
        window.finish_fresh_view_dispatch_for_fragment(&replay);

        patches.push(SelectiveAnyViewTargetPatch {
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
        });

        source_prepaint_cursor = patches
            .last()
            .expect("selective target patch was just appended")
            .source_target
            .prepaint_range
            .end
            .clone();
        source_paint_cursor = patches
            .last()
            .expect("selective target patch was just appended")
            .source_target
            .paint_range
            .end
            .clone();
        source_metadata_cursor = patches
            .last()
            .expect("selective target patch was just appended")
            .source_target
            .metadata_range
            .end;
    }

    let source_prepaint_tail =
        source_prepaint_cursor..parent_state.prepaint_range.end.clone();
    let source_paint_tail = source_paint_cursor..parent_state.paint_range.end.clone();
    let source_metadata_tail = source_metadata_cursor..parent_state.metadata_range.end;
    let target_prepaint_tail_start = window.prepaint_index();
    if !window.reuse_prepaint_fragment(source_prepaint_tail.clone(), &mut replay) {
        window.degrade_current_draw();
        return None;
    }
    let target_prepaint_tail_end = window.prepaint_index();
    let parent_prepaint_end = target_prepaint_tail_end.clone();

    cx.entities.extend_accessed(&parent_state.accessed_entities);

    Some(SelectiveAnyViewPatch {
        targets: patches,
        source_prepaint_tail,
        source_paint_tail,
        source_metadata_tail,
        target_prepaint_tail: target_prepaint_tail_start..target_prepaint_tail_end,
        parent_prepaint_range: parent_prepaint_start..parent_prepaint_end,
    })
}

impl AnyView {
    /// Indicate that this view should be cached when using it as an element.
    /// When using this method, the view's previous layout and paint will be recycled from the previous frame if [Context::notify] has not been called since it was rendered.
    /// The one exception is when [Window::refresh] is called, in which case caching is ignored.
    pub fn cached(mut self, style: StyleRefinement) -> Self {
        self.cached_style = Some(style.into());
        self
    }

    /// Attach a stable subtree fingerprint used by the framework cache to reuse prepaint/paint.
    pub fn cached_with_fingerprint(mut self, style: StyleRefinement, fingerprint: u64) -> Self {
        self.cached_style = Some(style.into());
        self.cache_fingerprint = Some(fingerprint);
        self
    }

    /// Cache this view using a semantic key hashed by GPUI.
    pub fn cached_by<K: Hash + ?Sized>(self, style: StyleRefinement, key: &K) -> Self {
        self.cached_with_fingerprint(style, render_fingerprint(key))
    }

    /// Cache an overlay view whose root is absolutely positioned over its containing block.
    ///
    /// Cached views use the provided style during layout on cache hits, so overlay views must
    /// preserve their absolute positioning even when their `Render` implementation is skipped.
    pub fn cached_absolute_by<K: Hash + ?Sized>(self, key: &K) -> Self {
        self.cached_by(StyleRefinement::default().absolute().inset_0(), key)
    }

    /// Allow this cached view to reuse its previous retained subtree when the frame budget is
    /// exhausted, leaving the view dirty so it can finish on a following frame.
    pub fn progressive(mut self) -> Self {
        self.progressive = true;
        self
    }

    /// Allow this cached view to reuse its previous retained subtree during
    /// [`Window::refresh`] when the cache key is unchanged and the view itself
    /// is not dirty.
    pub fn reuse_on_window_refresh(mut self) -> Self {
        self.reuse_on_window_refresh = true;
        self
    }

    /// Keep this view on the critical rendering path when the frame budget is exhausted.
    ///
    /// Critical views are intended for chrome, overlays, and other small always-visible UI
    /// surfaces that must not disappear while a heavy sibling is being progressively rendered.
    pub fn critical(mut self) -> Self {
        self.critical = true;
        self
    }

    /// Convert this to a weak handle.
    pub fn downgrade(&self) -> AnyWeakView {
        AnyWeakView {
            entity: self.entity.downgrade(),
            render: self.render,
            cached_style: self.cached_style.clone(),
            cache_fingerprint: self.cache_fingerprint,
            progressive: self.progressive,
            critical: self.critical,
            reuse_on_window_refresh: self.reuse_on_window_refresh,
        }
    }

    /// Convert this to a [Entity] of a specific type.
    /// If this handle does not contain a view of the specified type, returns itself in an `Err` variant.
    pub fn downcast<T: 'static>(self) -> Result<Entity<T>, Self> {
        match self.entity.downcast() {
            Ok(entity) => Ok(entity),
            Err(entity) => Err(Self {
                entity,
                render: self.render,
                cached_style: self.cached_style,
                cache_fingerprint: self.cache_fingerprint,
                progressive: self.progressive,
                critical: self.critical,
                reuse_on_window_refresh: self.reuse_on_window_refresh,
            }),
        }
    }

    /// Gets the [TypeId] of the underlying view.
    pub fn entity_type(&self) -> TypeId {
        self.entity.entity_type
    }

    /// Gets the entity id of this handle.
    pub fn entity_id(&self) -> EntityId {
        self.entity.entity_id()
    }

    fn cache_fingerprint(&self) -> Option<u64> {
        if let Some(fingerprint) = self.cache_fingerprint {
            return Some(fingerprint);
        }

        self.cached_style.as_ref()?;

        Some(render_fingerprint(&(
            self.entity.entity_type,
            self.entity.entity_id().as_u64(),
        )))
    }
}

impl PartialEq for AnyView {
    fn eq(&self, other: &Self) -> bool {
        self.entity == other.entity
    }
}

impl Eq for AnyView {}

impl Element for AnyView {
    type RequestLayoutState = Option<AnyElement>;
    type PrepaintState = AnyViewPrepaintState;
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
            let critical = self.critical;
            // Disable caching when inspecting so that mouse_hit_test has all hitboxes.
            let caching_disabled = window.is_inspector_picking(cx);
            match self.cached_style.as_ref() {
                Some(style) if !caching_disabled => {
                    let mut root_style = Style::default();
                    root_style.refine(style);
                    let layout_id = window.request_layout(root_style, None, cx);
                    (layout_id, None)
                }
                _ => {
                    let (layout_id, element) =
                        with_optional_critical_draw(critical, window, |window| {
                            window.record_rendered_view(
                                self.entity_id(),
                                cx.entities.type_name_for_id(self.entity_id()).unwrap_or("unknown"),
                            );
                            let mut element = (self.render)(self, window, cx);
                            let layout_id = element.request_layout(window, cx);
                            (layout_id, element)
                        });
                    (layout_id, Some(element))
                }
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
        if self.cached_style.is_some()
            && let Some(retained_id) = retained_id.as_ref()
            && let Some(state_global_id) = global_id
        {
            window.invalidator.register_cached_view_retained_target(
                self.entity_id(),
                retained_id,
                state_global_id,
            );
        }
        window.with_rendered_view(self.entity_id(), |window| {
            let critical = self.critical;
            if let Some(mut element) = element.take() {
                with_optional_critical_draw(critical, window, |window| {
                    element.prepaint(window, cx);
                });
                return AnyViewPrepaintState(AnyViewPrepaintStateKind::Fresh(element));
            }

            window.with_element_state::<AnyViewState, _>(
                global_id.unwrap(),
                |mut element_state, window| {
                    let retained_id = retained_id
                        .clone()
                        .expect("cached AnyView must have a retained identity");
                    let traversal_context = traversal_context.clone();
                    if let Some(state) = element_state.as_mut() {
                        state.owner_id = self.entity_id();
                        state.retained_id = retained_id.clone();
                        state.weak_view = self.downgrade();
                        state.traversal_context = traversal_context.clone();
                    }

                    let content_mask = window.content_mask();
                    let text_style = window.text_style();
                    let paint_context = ViewPaintContext {
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
                        && let Some(patch) = try_selective_any_view_prepaint(
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
                        state.replay_source_prepaint_range = None;
                        state.replay_source_paint_range = None;
                        state.replay_source_metadata_range = None;
                        return (
                            AnyViewPrepaintState(AnyViewPrepaintStateKind::Selective(Box::new(patch))),
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
                            return (AnyViewPrepaintState(AnyViewPrepaintStateKind::Replay), element_state);
                        }
                        cx.entities
                            .extend_accessed(&element_state.accessed_entities);
                        let prepaint_end = window.prepaint_index();
                        if !window.draw_was_degraded() {
                            element_state.prepaint_range = prepaint_start..prepaint_end;
                            element_state.replay_source_prepaint_range =
                                Some(source_prepaint_range);
                            element_state.replay_source_paint_range = Some(source_paint_range);
                            element_state.replay_source_metadata_range =
                                Some(source_metadata_range);
                        }

                        return (AnyViewPrepaintState(AnyViewPrepaintStateKind::Replay), element_state);
                    }
                    let refreshing = mem::replace(&mut window.refreshing, true);
                    let prepaint_start = window.prepaint_index();
                    window.record_rendered_view(
                        self.entity_id(),
                        cx.entities.type_name_for_id(self.entity_id()).unwrap_or("unknown"),
                    );
                    let (mut element, accessed_entities) = cx.detect_accessed_entities(|cx| {
                        with_optional_critical_draw(critical, window, |window| {
                            let mut element = div()
                                .relative()
                                .size_full()
                                .child((self.render)(self, window, cx))
                                .into_any_element();
                            element.layout_as_root(bounds.size.into(), window, cx);
                            element.prepaint_at(bounds.origin, window, cx);
                            element
                        })
                    });

                    let prepaint_end = window.prepaint_index();
                    window.refreshing = refreshing;

                    (
                        AnyViewPrepaintState(AnyViewPrepaintStateKind::Fresh(element)),
                        AnyViewState {
                            owner_id: self.entity_id(),
                            retained_id,
                            weak_view: self.downgrade(),
                            traversal_context,
                            accessed_entities,
                            prepaint_range: prepaint_start..prepaint_end,
                            paint_range: PaintIndex::default()..PaintIndex::default(),
                            metadata_range: 0..0,
                            replay_source_prepaint_range: None,
                            replay_source_paint_range: None,
                            replay_source_metadata_range: None,
                            cache_key: ViewCacheKey {
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
            let critical = self.critical;
            let caching_disabled = window.is_inspector_picking(cx);
            if self.cached_style.is_some() && !caching_disabled {
                window.with_element_state::<AnyViewState, _>(
                    global_id.unwrap(),
                    |element_state, window| {
                        let mut element_state = element_state.unwrap();

                        let metadata_start = window.retained_element_metadata_len();
                        let paint_start = window.paint_index();

                        match &mut element.0 {
                            AnyViewPrepaintStateKind::Fresh(element) => {
                                // Cached view missed and is forwarding current work to its rendered
                                // child. The AnyView wrapper itself owns no scene primitives.
                                window.record_debug_element_traversal_only(bounds, cx);
                                let refreshing = mem::replace(&mut window.refreshing, true);
                                with_optional_critical_draw(critical, window, |window| {
                                    element.paint(window, cx);
                                });
                                window.refreshing = refreshing;
                            }
                            AnyViewPrepaintStateKind::Replay => {
                                // Full cached subtree replay: replace Drawable's provisional red
                                // marker with a retained-green marker before copying prior ranges.
                                window.record_debug_element_self_scene_replay(bounds, cx);
                                let source_prepaint = element_state
                                    .replay_source_prepaint_range
                                    .take()
                                    .unwrap_or_else(|| element_state.prepaint_range.clone());
                                let source_paint = element_state
                                    .replay_source_paint_range
                                    .take()
                                    .unwrap_or_else(|| element_state.paint_range.clone());
                                let source_metadata = element_state
                                    .replay_source_metadata_range
                                    .take()
                                    .unwrap_or_else(|| element_state.metadata_range.clone());
                                if !window.reuse_paint(source_paint.clone()) {
                                    window.record_debug_view_cache_status(
                                        bounds,
                                        ViewCacheDebugStatus::ReuseFailed,
                                        cx,
                                    );
                                    window.degrade_current_draw();
                                } else {
                                    let target_paint = paint_start.clone()..window.paint_index();
                                    if !source_metadata.is_empty()
                                        && !window.replay_retained_element_metadata(
                                            &source_prepaint,
                                            &source_paint,
                                            &source_metadata,
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
                            AnyViewPrepaintStateKind::Selective(patch) => {
                                window.record_debug_element_traversal_only(bounds, cx);

                                for target in &mut patch.targets {
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
                            element_state.replay_source_prepaint_range = None;
                            element_state.replay_source_paint_range = None;
                            element_state.replay_source_metadata_range = None;
                        }

                        ((), element_state)
                    },
                )
            } else {
                window.record_debug_element_traversal_only(bounds, cx);
                with_optional_critical_draw(critical, window, |window| {
                    match &mut element.0 {
                        AnyViewPrepaintStateKind::Fresh(element) => element.paint(window, cx),
                        AnyViewPrepaintStateKind::Replay
                        | AnyViewPrepaintStateKind::Selective(_) => {
                            unreachable!("uncached AnyView must carry fresh prepaint state")
                        }
                    }
                });
            }
        });
    }
}

impl<V: 'static + Render> IntoElement for Entity<V> {
    type Element = Entity<V>;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl IntoElement for AnyView {
    type Element = Self;

    fn into_element(self) -> AnyView {
        self
    }
}

/// A weak, dynamically-typed handle to a view that does not prevent the view from being released.
#[derive(Clone)]
pub struct AnyWeakView {
    entity: AnyWeakEntity,
    render: fn(&AnyView, &mut Window, &mut App) -> AnyElement,
    cached_style: Option<Rc<StyleRefinement>>,
    cache_fingerprint: Option<u64>,
    progressive: bool,
    critical: bool,
    reuse_on_window_refresh: bool,
}

impl AnyWeakView {
    /// Convert to a strongly-typed handle if the referenced view has not yet been released.
    pub fn upgrade(&self) -> Option<AnyView> {
        let entity = self.entity.upgrade()?;
        Some(AnyView {
            entity,
            render: self.render,
            cached_style: self.cached_style.clone(),
            cache_fingerprint: self.cache_fingerprint,
            progressive: self.progressive,
            critical: self.critical,
            reuse_on_window_refresh: self.reuse_on_window_refresh,
        })
    }
}

impl<V: 'static + Render> From<WeakEntity<V>> for AnyWeakView {
    fn from(view: WeakEntity<V>) -> Self {
        AnyWeakView {
            entity: view.into(),
            render: any_view::render::<V>,
            cached_style: None,
            cache_fingerprint: None,
            progressive: false,
            critical: false,
            reuse_on_window_refresh: false,
        }
    }
}

impl PartialEq for AnyWeakView {
    fn eq(&self, other: &Self) -> bool {
        self.entity == other.entity
    }
}

impl std::fmt::Debug for AnyWeakView {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnyWeakView")
            .field("entity_id", &self.entity.entity_id)
            .finish_non_exhaustive()
    }
}

mod any_view {
    use crate::{AnyElement, AnyView, App, IntoElement, Render, Window};

    pub(crate) fn render<V: 'static + Render>(
        view: &AnyView,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let view = view.clone().downcast::<V>().unwrap();
        view.update(cx, |view, cx| view.render(window, cx).into_any_element())
    }
}

/// A view that renders nothing
pub struct EmptyView;

impl Render for EmptyView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

#[cfg(test)]
mod retained_boundary_tests;
#[cfg(test)]
mod tests;
