use super::fingerprint::render_fingerprint;
use crate::Styled;
use crate::{
    AnyElement, AnyEntity, AnyWeakEntity, App, Bounds, ContentMask, Context, Element, ElementId,
    Entity, EntityId, GlobalElementId, InspectorElementId, IntoElement, LayoutId, PaintIndex,
    ParentElement, Pixels, PrepaintStateIndex, Render, Style, StyleRefinement, TextStyle,
    WeakEntity, div,
};
use crate::{Empty, Window};
use anyhow::Result;
use collections::FxHashSet;
use refineable::Refineable;
use std::hash::Hash;
use std::mem;
use std::rc::Rc;
use std::{any::TypeId, fmt, ops::Range};

struct AnyViewState {
    prepaint_range: Range<PrepaintStateIndex>,
    paint_range: Range<PaintIndex>,
    cache_key: ViewCacheKey,
    accessed_entities: FxHashSet<EntityId>,
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
                    let view_dirty = window.dirty_views.contains(&self.entity_id());
                    let force_refresh = window.force_view_cache_refresh();
                    let can_reuse_refresh =
                        self.reuse_on_window_refresh && force_refresh && !view_dirty;
                    let can_defer_dirty_view = view_dirty
                        && self.progressive
                        && !self.critical
                        && window.draw_budget_exhausted();

                    if let Some(mut element_state) = element_state
                        && element_state.cache_key.bounds == bounds
                        && element_state.cache_key.content_mask == content_mask
                        && element_state.cache_key.text_style == text_style
                        && element_state.cache_key.fingerprint == cache_fingerprint
                        && (!force_refresh || can_reuse_refresh || can_defer_dirty_view)
                        && (!view_dirty || can_defer_dirty_view)
                        && window.can_reuse_prepaint(&element_state.prepaint_range)
                        && (can_defer_dirty_view
                            || window.can_reuse_paint(&element_state.paint_range))
                    {
                        if can_defer_dirty_view {
                            window.degrade_current_draw();
                        }
                        let prepaint_start = window.prepaint_index();
                        if !window.reuse_prepaint(element_state.prepaint_range.clone()) {
                            window.degrade_current_draw();
                            return (None, element_state);
                        }
                        cx.entities
                            .extend_accessed(&element_state.accessed_entities);
                        let prepaint_end = window.prepaint_index();
                        if !window.draw_was_degraded() {
                            element_state.prepaint_range = prepaint_start..prepaint_end;
                        }

                        return (None, element_state);
                    }
                    let refreshing = mem::replace(&mut window.refreshing, true);
                    let prepaint_start = window.prepaint_index();
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
                            let refreshing = mem::replace(&mut window.refreshing, true);
                            with_optional_critical_draw(critical, window, |window| {
                                element.paint(window, cx);
                            });
                            window.refreshing = refreshing;
                        } else if !window.reuse_paint(element_state.paint_range.clone()) {
                            window.degrade_current_draw();
                        }

                        let paint_end = window.paint_index();
                        if !window.draw_was_degraded() {
                            element_state.paint_range = paint_start..paint_end;
                        }

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
mod tests;
