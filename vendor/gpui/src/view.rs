use crate::Styled;
use crate::{
    AnyElement, AnyEntity, AnyWeakEntity, App, Bounds, ContentMask, Context, Element, ElementId,
    Entity, EntityId, GlobalElementId, InspectorElementId, IntoElement, LayoutId, PaintIndex,
    Pixels, PrepaintStateIndex, Render, Style, StyleRefinement, TextStyle, WeakEntity,
    record_view_cache_paint_hit, record_view_cache_prepaint_hit,
};
use crate::{Empty, Window};
use anyhow::Result;
use collections::FxHashSet;
use refineable::Refineable;
use seahash::SeaHasher;
use std::hash::{Hash, Hasher};
use std::mem;
use std::rc::Rc;
use std::{any::TypeId, fmt, ops::Range};

/// A framework-owned fingerprint builder for render cache validation.
///
/// Use this for UI cache keys and frame validation signatures instead of choosing a hasher in
/// application code. Applications should only record the small semantic values that affect the
/// rendered output; GPUI owns the hashing implementation.
#[derive(Clone)]
pub struct RenderFingerprint {
    hasher: SeaHasher,
}

impl RenderFingerprint {
    /// Creates an empty render fingerprint.
    pub fn new() -> Self {
        Self {
            hasher: SeaHasher::new(),
        }
    }

    /// Records a semantic value into the fingerprint.
    pub fn record<T: Hash + ?Sized>(&mut self, value: &T) -> &mut Self {
        value.hash(self);
        self
    }

    /// Returns the current fingerprint value.
    pub fn value(&self) -> u64 {
        self.hasher.finish()
    }

    /// Returns the current fingerprint value as a fixed-width hex string.
    pub fn hex(&self) -> String {
        format!("{:016x}", self.value())
    }
}

impl Default for RenderFingerprint {
    fn default() -> Self {
        Self::new()
    }
}

impl Hasher for RenderFingerprint {
    fn finish(&self) -> u64 {
        self.hasher.finish()
    }

    fn write(&mut self, bytes: &[u8]) {
        self.hasher.write(bytes);
    }
}

/// Computes a framework-owned render fingerprint for a semantic cache key.
pub fn render_fingerprint<T: Hash + ?Sized>(value: &T) -> u64 {
    let mut fingerprint = RenderFingerprint::new();
    fingerprint.record(value);
    fingerprint.value()
}

/// Computes a framework-owned render fingerprint as a fixed-width hex string.
pub fn render_fingerprint_hex<T: Hash + ?Sized>(value: &T) -> String {
    let mut fingerprint = RenderFingerprint::new();
    fingerprint.record(value);
    fingerprint.hex()
}

struct AnyViewState {
    prepaint_range: Range<PrepaintStateIndex>,
    paint_range: Range<PaintIndex>,
    cache_key: ViewCacheKey,
    accessed_entities: FxHashSet<EntityId>,
    incomplete: bool,
}

#[derive(Default)]
struct ViewCacheKey {
    bounds: Bounds<Pixels>,
    content_mask: ContentMask<Pixels>,
    text_style: TextStyle,
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
        let mut element = self.update(cx, |view, cx| view.render(window, cx).into_any_element());
        let layout_id = window.with_rendered_view(self.entity_id(), |window| {
            element.request_layout(window, cx)
        });
        (layout_id, element)
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
        window.set_view_id(self.entity_id());
        window.with_rendered_view(self.entity_id(), |window| element.prepaint(window, cx));
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_rendered_view(self.entity_id(), |window| element.paint(window, cx));
    }
}

/// A dynamically-typed handle to a view, which can be downcast to a [Entity] for a specific type.
#[derive(Clone, Debug)]
pub struct AnyView {
    entity: AnyEntity,
    render: fn(&AnyView, &mut Window, &mut App) -> AnyElement,
    cached_style: Option<Rc<StyleRefinement>>,
    cache_fingerprint: Option<u64>,
    progressive: bool,
    critical: bool,
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
        }
    }
}

fn with_optional_critical_draw<R>(
    critical: bool,
    window: &mut Window,
    f: impl FnOnce(&mut Window) -> R,
) -> R {
    if critical {
        window.with_critical_draw(f)
    } else {
        f(window)
    }
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
    type PrepaintState = Option<AnyElement>;

    fn id(&self) -> Option<ElementId> {
        Some(ElementId::View(self.entity_id()))
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
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
    ) -> Option<AnyElement> {
        window.set_view_id(self.entity_id());
        window.with_rendered_view(self.entity_id(), |window| {
            let critical = self.critical;
            if let Some(mut element) = element.take() {
                with_optional_critical_draw(critical, window, |window| {
                    element.prepaint(window, cx);
                });
                return Some(element);
            }

            window.with_element_state::<AnyViewState, _>(
                global_id.unwrap(),
                |element_state, window| {
                    let content_mask = window.content_mask();
                    let text_style = window.text_style();

                    let cache_fingerprint = self.cache_fingerprint();

                    let mut element_state = element_state;
                    let cache_key_matches = element_state.as_ref().is_some_and(|state| {
                        state.cache_key.bounds == bounds
                            && state.cache_key.content_mask == content_mask
                            && state.cache_key.text_style == text_style
                            && state.cache_key.fingerprint == cache_fingerprint
                    });
                    let cache_state_is_complete = element_state
                        .as_ref()
                        .is_some_and(|state| !state.incomplete);
                    if self.progressive
                        && !self.critical
                        && cache_key_matches
                        && cache_state_is_complete
                        && window.draw_budget_exhausted()
                        && window.dirty_views.contains(&self.entity_id())
                        && !window.force_view_cache_refresh()
                    {
                        let mut element_state = element_state
                            .take()
                            .expect("cache key match requires existing view state");
                        window.degrade_current_draw();
                        let prepaint_start = window.prepaint_index();
                        window.reuse_prepaint(element_state.prepaint_range.clone());
                        cx.entities
                            .extend_accessed(&element_state.accessed_entities);
                        let prepaint_end = window.prepaint_index();
                        element_state.prepaint_range = prepaint_start..prepaint_end;
                        record_view_cache_prepaint_hit(true);
                        return (None, element_state);
                    }

                    if cache_key_matches
                        && cache_state_is_complete
                        && !window.dirty_views.contains(&self.entity_id())
                        && !window.force_view_cache_refresh()
                    {
                        let mut element_state = element_state
                            .take()
                            .expect("cache key match requires existing view state");
                        let prepaint_start = window.prepaint_index();
                        window.reuse_prepaint(element_state.prepaint_range.clone());
                        cx.entities
                            .extend_accessed(&element_state.accessed_entities);
                        let prepaint_end = window.prepaint_index();
                        element_state.prepaint_range = prepaint_start..prepaint_end;
                        record_view_cache_prepaint_hit(true);

                        return (None, element_state);
                    }

                    record_view_cache_prepaint_hit(false);
                    let refreshing = mem::replace(&mut window.refreshing, true);
                    let prepaint_start = window.prepaint_index();
                    let degraded_before = window.draw_degraded_this_frame();
                    let (mut element, accessed_entities) = cx.detect_accessed_entities(|cx| {
                        with_optional_critical_draw(critical, window, |window| {
                            let mut element = (self.render)(self, window, cx);
                            element.layout_as_root(bounds.size.into(), window, cx);
                            element.prepaint_at(bounds.origin, window, cx);
                            element
                        })
                    });

                    let prepaint_end = window.prepaint_index();
                    window.refreshing = refreshing;

                    if !self.critical
                        && window.draw_budget_exhausted()
                        && cache_key_matches
                        && cache_state_is_complete
                    {
                        if let Some(mut element_state) = element_state.take() {
                            window.degrade_current_draw();
                            window.truncate_prepaint_to(prepaint_start);
                            let prepaint_start = window.prepaint_index();
                            window.reuse_prepaint(element_state.prepaint_range.clone());
                            cx.entities
                                .extend_accessed(&element_state.accessed_entities);
                            let prepaint_end = window.prepaint_index();
                            element_state.prepaint_range = prepaint_start..prepaint_end;
                            record_view_cache_prepaint_hit(true);
                            return (None, element_state);
                        }
                    }

                    (
                        Some(element),
                        AnyViewState {
                            accessed_entities,
                            prepaint_range: prepaint_start..prepaint_end,
                            paint_range: PaintIndex::default()..PaintIndex::default(),
                            cache_key: ViewCacheKey {
                                bounds,
                                content_mask,
                                text_style,
                                fingerprint: cache_fingerprint,
                            },
                            incomplete: !self.critical
                                && (window.draw_degraded_this_frame()
                                    || (window.draw_budget_exhausted() && !degraded_before)),
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
        _bounds: Bounds<Pixels>,
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

                        let paint_start = window.paint_index();

                        if let Some(element) = element {
                            record_view_cache_paint_hit(false);
                            let refreshing = mem::replace(&mut window.refreshing, true);
                            let degraded_before = window.draw_degraded_this_frame();
                            with_optional_critical_draw(critical, window, |window| {
                                element.paint(window, cx);
                            });
                            if !critical
                                && (window.draw_degraded_this_frame()
                                    || window.draw_budget_exhausted())
                            {
                                element_state.incomplete = true;
                            } else if !degraded_before {
                                element_state.incomplete = false;
                            }
                            window.refreshing = refreshing;
                        } else {
                            record_view_cache_paint_hit(true);
                            window.reuse_paint(element_state.paint_range.clone());
                        }

                        let paint_end = window.paint_index();
                        element_state.paint_range = paint_start..paint_end;

                        ((), element_state)
                    },
                )
            } else {
                with_optional_critical_draw(critical, window, |window| {
                    element.as_mut().unwrap().paint(window, cx);
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

    fn into_element(self) -> Self::Element {
        self
    }
}

/// A weak, dynamically-typed view handle that does not prevent the view from being released.
pub struct AnyWeakView {
    entity: AnyWeakEntity,
    render: fn(&AnyView, &mut Window, &mut App) -> AnyElement,
    cached_style: Option<Rc<StyleRefinement>>,
    cache_fingerprint: Option<u64>,
    progressive: bool,
    critical: bool,
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
mod tests {
    use super::*;
    use crate::element::ParentElement;
    use crate::{
        AnyWindowHandle, AppContext, TestAppContext, WindowOptions, performance_metrics_snapshot,
        point, px, size,
    };

    #[derive(Default)]
    struct CachedLeafView {
        revisions: usize,
    }

    impl Render for CachedLeafView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            crate::div().child(format!("leaf-{}", self.revisions))
        }
    }

    struct CachedRootView {
        stable: Entity<CachedLeafView>,
        dirty: Entity<CachedLeafView>,
    }

    impl Render for CachedRootView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            crate::div()
                .child(
                    AnyView::from(self.stable.clone())
                        .cached(StyleRefinement::default())
                        .into_any_element(),
                )
                .child(
                    AnyView::from(self.dirty.clone())
                        .cached(StyleRefinement::default())
                        .into_any_element(),
                )
        }
    }

    struct BudgetFallbackElement {
        prepaint_count: Rc<std::cell::Cell<usize>>,
        paint_count: Rc<std::cell::Cell<usize>>,
        exhaust_budget: Rc<std::cell::Cell<bool>>,
    }

    impl IntoElement for BudgetFallbackElement {
        type Element = Self;

        fn into_element(self) -> Self::Element {
            self
        }
    }

    impl Element for BudgetFallbackElement {
        type RequestLayoutState = ();
        type PrepaintState = ();

        fn id(&self) -> Option<ElementId> {
            None
        }

        fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
            None
        }

        fn request_layout(
            &mut self,
            _id: Option<&GlobalElementId>,
            _inspector_id: Option<&InspectorElementId>,
            window: &mut Window,
            cx: &mut App,
        ) -> (LayoutId, Self::RequestLayoutState) {
            (window.request_layout(Style::default(), None, cx), ())
        }

        fn prepaint(
            &mut self,
            _id: Option<&GlobalElementId>,
            _inspector_id: Option<&InspectorElementId>,
            _bounds: Bounds<Pixels>,
            _request_layout: &mut Self::RequestLayoutState,
            window: &mut Window,
            _cx: &mut App,
        ) -> Self::PrepaintState {
            let previous_count = self.prepaint_count.get();
            self.prepaint_count.set(previous_count.saturating_add(1));
            if self.exhaust_budget.get() {
                window.test_exhaust_draw_budget();
            }
        }

        fn paint(
            &mut self,
            _id: Option<&GlobalElementId>,
            _inspector_id: Option<&InspectorElementId>,
            _bounds: Bounds<Pixels>,
            _request_layout: &mut Self::RequestLayoutState,
            _prepaint: &mut Self::PrepaintState,
            _window: &mut Window,
            _cx: &mut App,
        ) {
            self.paint_count
                .set(self.paint_count.get().saturating_add(1));
        }
    }

    struct BudgetFallbackLeafView {
        revisions: usize,
        prepaint_count: Rc<std::cell::Cell<usize>>,
        paint_count: Rc<std::cell::Cell<usize>>,
        exhaust_budget: Rc<std::cell::Cell<bool>>,
    }

    impl Render for BudgetFallbackLeafView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            crate::div()
                .child(format!("revision-{}", self.revisions))
                .child(BudgetFallbackElement {
                    prepaint_count: self.prepaint_count.clone(),
                    paint_count: self.paint_count.clone(),
                    exhaust_budget: self.exhaust_budget.clone(),
                })
        }
    }

    struct BudgetFallbackRootView {
        leaf: Entity<BudgetFallbackLeafView>,
        progressive: bool,
        critical: bool,
    }

    impl Render for BudgetFallbackRootView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let mut view = AnyView::from(self.leaf.clone())
                .cached_by(StyleRefinement::default(), &"budget-fallback-leaf");
            if self.critical {
                view = view.critical();
            }
            crate::div().child(if self.progressive {
                view.progressive().into_any_element()
            } else {
                view.into_any_element()
            })
        }
    }

    struct PaintMarkerView {
        marker_x: f32,
    }

    impl Render for PaintMarkerView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let marker_x = self.marker_x;
            crate::canvas(
                |_bounds, _window, _cx| (),
                move |_bounds, (), window, _cx| {
                    window.paint_quad(crate::fill(
                        Bounds::new(point(px(marker_x), px(0.0)), size(px(10.0), px(10.0))),
                        crate::rgb(0xffffff),
                    ));
                },
            )
            .absolute()
            .left(px(self.marker_x))
            .top(px(0.0))
            .w(px(10.0))
            .h(px(10.0))
        }
    }

    struct CachedChromeOrderRoot {
        background: Entity<PaintMarkerView>,
        page: Entity<PaintMarkerView>,
        chrome: Entity<PaintMarkerView>,
    }

    impl Render for CachedChromeOrderRoot {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            crate::div()
                .relative()
                .size_full()
                .child(
                    AnyView::from(self.background.clone())
                        .cached_absolute_by(&"background")
                        .progressive()
                        .into_any_element(),
                )
                .child(
                    AnyView::from(self.page.clone())
                        .cached_absolute_by(&"page")
                        .progressive()
                        .into_any_element(),
                )
                .child(
                    AnyView::from(self.chrome.clone())
                        .cached_absolute_by(&"chrome")
                        .critical()
                        .into_any_element(),
                )
        }
    }

    fn marker_quad_order(window: &Window) -> Vec<i32> {
        let scale_factor = window.scale_factor();
        window
            .rendered_frame
            .scene
            .quads
            .iter()
            .filter_map(|quad| {
                let x = (quad.bounds.origin.x.0 / scale_factor).round() as i32;
                matches!(x, 1 | 2 | 3).then_some(x)
            })
            .collect()
    }

    #[gpui::test]
    fn cached_view_infers_stable_fingerprint(cx: &mut TestAppContext) {
        let view = cx.update(|cx| cx.new(|_| EmptyView));

        let cached_view = AnyView::from(view.clone()).cached(StyleRefinement::default());
        let same_cached_view = AnyView::from(view).cached(StyleRefinement::default());

        assert_eq!(
            cached_view.cache_fingerprint(),
            same_cached_view.cache_fingerprint()
        );
        assert!(cached_view.cache_fingerprint().is_some());
    }

    #[gpui::test]
    fn cached_view_preserves_explicit_fingerprint_through_weak_upgrade(cx: &mut TestAppContext) {
        let view = cx.update(|cx| cx.new(|_| EmptyView));
        let cached_view =
            AnyView::from(view).cached_with_fingerprint(StyleRefinement::default(), 42);
        let weak_view = cached_view.downgrade();
        let upgraded = weak_view.upgrade().expect("view should still be alive");

        assert_eq!(upgraded.cache_fingerprint(), Some(42));
    }

    #[gpui::test]
    fn cached_view_hashes_semantic_key_in_framework(cx: &mut TestAppContext) {
        let view = cx.update(|cx| cx.new(|_| EmptyView));
        let cached_view =
            AnyView::from(view).cached_by(StyleRefinement::default(), &("route", 7_u64));

        assert_eq!(
            cached_view.cache_fingerprint(),
            Some(render_fingerprint(&("route", 7_u64)))
        );
    }

    #[gpui::test]
    fn progressive_cached_view_preserves_flag_through_weak_upgrade(cx: &mut TestAppContext) {
        let view = cx.update(|cx| cx.new(|_| EmptyView));
        let cached_view = AnyView::from(view)
            .cached(StyleRefinement::default())
            .progressive();
        let weak_view = cached_view.downgrade();
        let upgraded = weak_view.upgrade().expect("view should still be alive");

        assert!(upgraded.progressive);
        assert!(upgraded.cache_fingerprint().is_some());
    }

    #[gpui::test]
    fn clean_cached_sibling_reuses_prepaint_and_paint_during_dirty_frame(cx: &mut TestAppContext) {
        let (dirty, window) = cx.update(|cx| {
            let stable = cx.new(|_| CachedLeafView::default());
            let dirty = cx.new(|_| CachedLeafView::default());
            let window = cx
                .open_window(WindowOptions::default(), |_, cx| {
                    cx.new(|_| CachedRootView {
                        stable,
                        dirty: dirty.clone(),
                    })
                })
                .unwrap();
            (dirty, AnyWindowHandle::from(window))
        });

        let before = performance_metrics_snapshot();
        cx.update(|cx| {
            dirty.update(cx, |dirty, cx| {
                dirty.revisions += 1;
                cx.notify();
            });
            cx.update_window(window, |_, window, cx| {
                window.draw(cx, crate::window::SLOW_FRAME_REQUEST).clear();
            })
            .unwrap();
        });
        let after = performance_metrics_snapshot();

        assert!(after.view_cache_prepaint_hits > before.view_cache_prepaint_hits);
        assert!(after.view_cache_paint_hits > before.view_cache_paint_hits);
    }

    #[gpui::test]
    fn dirty_background_does_not_replay_over_cached_chrome(cx: &mut TestAppContext) {
        let (background, window) = cx.update(|cx| {
            let background = cx.new(|_| PaintMarkerView { marker_x: 1.0 });
            let page = cx.new(|_| PaintMarkerView { marker_x: 2.0 });
            let chrome = cx.new(|_| PaintMarkerView { marker_x: 3.0 });
            let window = cx
                .open_window(WindowOptions::default(), |_, cx| {
                    cx.new(|_| CachedChromeOrderRoot {
                        background: background.clone(),
                        page,
                        chrome,
                    })
                })
                .unwrap();
            (background, AnyWindowHandle::from(window))
        });

        cx.update_window(window, |_, window, _| {
            assert_eq!(marker_quad_order(window), [1, 2, 3]);
        })
        .unwrap();

        cx.update(|cx| {
            background.update(cx, |_, cx| cx.notify());
            cx.update_window(window, |_, window, cx| {
                window.draw(cx, crate::window::SLOW_FRAME_REQUEST).clear();
                assert_eq!(marker_quad_order(window), [1, 2, 3]);
            })
            .unwrap();
        });
    }

    #[gpui::test]
    fn cached_views_are_reused_per_window_when_another_window_is_dirty(cx: &mut TestAppContext) {
        let (dirty, dirty_window, clean_window) = cx.update(|cx| {
            let dirty_stable = cx.new(|_| CachedLeafView::default());
            let dirty = cx.new(|_| CachedLeafView::default());
            let dirty_window = cx
                .open_window(WindowOptions::default(), |_, cx| {
                    cx.new(|_| CachedRootView {
                        stable: dirty_stable,
                        dirty: dirty.clone(),
                    })
                })
                .unwrap();

            let clean_stable = cx.new(|_| CachedLeafView::default());
            let clean_leaf = cx.new(|_| CachedLeafView::default());
            let clean_window = cx
                .open_window(WindowOptions::default(), |_, cx| {
                    cx.new(|_| CachedRootView {
                        stable: clean_stable,
                        dirty: clean_leaf,
                    })
                })
                .unwrap();

            (
                dirty,
                AnyWindowHandle::from(dirty_window),
                AnyWindowHandle::from(clean_window),
            )
        });

        let before = performance_metrics_snapshot();
        cx.update(|cx| {
            dirty.update(cx, |dirty, cx| {
                dirty.revisions += 1;
                cx.notify();
            });
            cx.update_window(dirty_window, |_, window, cx| {
                window.draw(cx, crate::window::SLOW_FRAME_REQUEST).clear();
            })
            .unwrap();
            cx.update_window(clean_window, |_, window, cx| {
                window.draw(cx, crate::window::SLOW_FRAME_REQUEST).clear();
            })
            .unwrap();
        });
        let after = performance_metrics_snapshot();

        assert!(after.view_cache_prepaint_hits > before.view_cache_prepaint_hits);
        assert!(after.view_cache_paint_hits > before.view_cache_paint_hits);
    }

    #[gpui::test]
    fn dirty_cached_view_reuses_previous_paint_when_prepaint_exceeds_budget(
        cx: &mut TestAppContext,
    ) {
        let prepaint_count = Rc::new(std::cell::Cell::new(0usize));
        let paint_count = Rc::new(std::cell::Cell::new(0usize));
        let exhaust_budget = Rc::new(std::cell::Cell::new(false));
        let (leaf, window) = cx.update(|cx| {
            let leaf = cx.new(|_| BudgetFallbackLeafView {
                revisions: 0,
                prepaint_count: prepaint_count.clone(),
                paint_count: paint_count.clone(),
                exhaust_budget: exhaust_budget.clone(),
            });
            let window = cx
                .open_window(WindowOptions::default(), |_, cx| {
                    cx.new(|_| BudgetFallbackRootView {
                        leaf: leaf.clone(),
                        progressive: false,
                        critical: false,
                    })
                })
                .unwrap();
            (leaf, AnyWindowHandle::from(window))
        });

        cx.update_window(window, |_, window, cx| {
            window.draw(cx, crate::window::SLOW_FRAME_REQUEST).clear();
        })
        .unwrap();
        let baseline_prepaint_count = prepaint_count.get();
        let baseline_paint_count = paint_count.get();
        assert!(baseline_prepaint_count > 0);
        assert!(baseline_paint_count > 0);

        cx.update(|cx| {
            let dirty_view = leaf.entity_id();
            exhaust_budget.set(true);
            leaf.update(cx, |leaf, _cx| {
                leaf.revisions = leaf.revisions.saturating_add(1);
            });
            cx.update_window(window, |_, window, cx| {
                assert!(!window.force_view_cache_refresh());
                window.dirty_views.insert(dirty_view);
                window.invalidator.set_dirty(true);
                window.draw(cx, std::time::Duration::ZERO).clear();

                assert!(window.invalidator.is_dirty());
            })
            .unwrap();
        });
        assert!(prepaint_count.get() > baseline_prepaint_count);
        assert_eq!(paint_count.get(), baseline_paint_count);

        exhaust_budget.set(false);
        cx.update_window(window, |_, window, cx| {
            window.draw(cx, crate::window::SLOW_FRAME_REQUEST).clear();

            assert!(!window.invalidator.is_dirty());
        })
        .unwrap();
        assert!(paint_count.get() > baseline_paint_count);
    }

    #[gpui::test]
    fn progressive_dirty_cached_view_defers_prepaint_when_budget_is_already_exhausted(
        cx: &mut TestAppContext,
    ) {
        let prepaint_count = Rc::new(std::cell::Cell::new(0usize));
        let paint_count = Rc::new(std::cell::Cell::new(0usize));
        let exhaust_budget = Rc::new(std::cell::Cell::new(false));
        let (leaf, window) = cx.update(|cx| {
            let leaf = cx.new(|_| BudgetFallbackLeafView {
                revisions: 0,
                prepaint_count: prepaint_count.clone(),
                paint_count: paint_count.clone(),
                exhaust_budget: exhaust_budget.clone(),
            });
            let window = cx
                .open_window(WindowOptions::default(), |_, cx| {
                    cx.new(|_| BudgetFallbackRootView {
                        leaf: leaf.clone(),
                        progressive: true,
                        critical: false,
                    })
                })
                .unwrap();
            (leaf, AnyWindowHandle::from(window))
        });

        cx.update_window(window, |_, window, cx| {
            window.draw(cx, crate::window::SLOW_FRAME_REQUEST).clear();
        })
        .unwrap();
        let baseline_prepaint_count = prepaint_count.get();
        let baseline_paint_count = paint_count.get();

        cx.update(|cx| {
            let dirty_view = leaf.entity_id();
            leaf.update(cx, |leaf, _cx| {
                leaf.revisions = leaf.revisions.saturating_add(1);
            });
            cx.update_window(window, |_, window, cx| {
                window.dirty_views.insert(dirty_view);
                window.invalidator.set_dirty(true);
                window.draw(cx, std::time::Duration::ZERO).clear();

                assert!(window.invalidator.is_dirty());
                assert_eq!(prepaint_count.get(), baseline_prepaint_count);
                assert_eq!(paint_count.get(), baseline_paint_count);
            })
            .unwrap();
        });

        cx.update_window(window, |_, window, cx| {
            window.draw(cx, crate::window::SLOW_FRAME_REQUEST).clear();

            assert!(!window.invalidator.is_dirty());
        })
        .unwrap();
        assert!(prepaint_count.get() > baseline_prepaint_count);
        assert!(paint_count.get() > baseline_paint_count);
    }

    #[gpui::test]
    fn critical_cached_view_finishes_when_budget_is_exhausted(cx: &mut TestAppContext) {
        let prepaint_count = Rc::new(std::cell::Cell::new(0usize));
        let paint_count = Rc::new(std::cell::Cell::new(0usize));
        let exhaust_budget = Rc::new(std::cell::Cell::new(false));
        let (leaf, window) = cx.update(|cx| {
            let leaf = cx.new(|_| BudgetFallbackLeafView {
                revisions: 0,
                prepaint_count: prepaint_count.clone(),
                paint_count: paint_count.clone(),
                exhaust_budget: exhaust_budget.clone(),
            });
            let window = cx
                .open_window(WindowOptions::default(), |_, cx| {
                    cx.new(|_| BudgetFallbackRootView {
                        leaf: leaf.clone(),
                        progressive: false,
                        critical: true,
                    })
                })
                .unwrap();
            (leaf, AnyWindowHandle::from(window))
        });

        cx.update_window(window, |_, window, cx| {
            window.draw(cx, crate::window::SLOW_FRAME_REQUEST).clear();
        })
        .unwrap();
        let baseline_prepaint_count = prepaint_count.get();
        let baseline_paint_count = paint_count.get();

        cx.update(|cx| {
            let dirty_view = leaf.entity_id();
            exhaust_budget.set(true);
            leaf.update(cx, |leaf, _cx| {
                leaf.revisions = leaf.revisions.saturating_add(1);
            });
            cx.update_window(window, |_, window, cx| {
                window.dirty_views.insert(dirty_view);
                window.invalidator.set_dirty(true);
                window.draw(cx, std::time::Duration::ZERO).clear();

                assert!(!window.invalidator.is_dirty());
            })
            .unwrap();
        });

        assert!(prepaint_count.get() > baseline_prepaint_count);
        assert!(paint_count.get() > baseline_paint_count);
    }
}
