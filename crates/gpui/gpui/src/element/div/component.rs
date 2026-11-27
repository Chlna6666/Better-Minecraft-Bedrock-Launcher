use crate::{
    AnyElement, App, Bounds, Display, Element, ElementId, Fill, GlobalElementId,
    Hitbox, HitboxBehavior, ImageCacheProvider,
    InspectorElementId, IntoElement, LayoutId, Overflow, ParentElement, Pixels, Point, ScrollCapture, Style,
    StyleRefinement, Styled, Visibility, Window, point,
};
use smallvec::SmallVec;

#[cfg(feature = "stacker")]
type StackSafe<T> = stacksafe::StackSafe<T>;
#[cfg(not(feature = "stacker"))]
type StackSafe<T> = T;

use super::event::{InteractiveElement, StatefulInteractiveElement};
use super::interactivity::{Interactivity, InteractivityBox};

/// Construct a new [`Div`] element.
#[track_caller]
pub fn div() -> Div {
    #[allow(unused_mut)]
    let mut interactivity = InteractivityBox::new();
    #[cfg(any(feature = "inspector", debug_assertions))]
    {
        interactivity.source_location = Some(core::panic::Location::caller());
    }
    Div {
        interactivity,
        source_location: core::panic::Location::caller(),
        children: SmallVec::default(),
        prepaint_listener: None,
        image_cache: None,
    }
}

/// A [`Div`] element, the all-in-one element for building complex UIs in GPUI.
///
/// Divs created during a draw use the frame arena and must not be stored across frame clears.
/// Outside a draw their interaction state is heap-owned; child [`AnyElement`] values still follow
/// their own arena lifetime.
pub struct Div {
    interactivity: InteractivityBox,
    source_location: &'static core::panic::Location<'static>,
    children: SmallVec<[StackSafe<AnyElement>; 2]>,
    prepaint_listener: Option<Box<dyn Fn(Vec<Bounds<Pixels>>, &mut Window, &mut App) + 'static>>,
    image_cache: Option<Box<dyn ImageCacheProvider>>,
}

impl Div {
    /// All coordinates and clips emitted by these built-in children can be
    /// projected from an overscanned row into its actual viewport.
    pub(crate) fn can_capture_scrolling_subtree(&mut self, capture: ScrollCapture) -> bool {
        let base = &self.interactivity.base_style;
        let descendants = if capture == ScrollCapture::RoundedViewport && clips_content(base) {
            ScrollCapture::ClippedRow
        } else {
            capture
        };
        self.prepaint_listener.is_none()
            && self.interactivity_can_move(capture == ScrollCapture::View)
            && (descendants != ScrollCapture::ClippedRow
                || (self.interactivity.hover_style.is_none() && base.mouse_cursor.is_none()))
            && capture_style(base, capture)
            && self.interactivity.hover_style.as_deref().is_none_or(|style| capture_style(style, capture))
            && self.interactivity.active_style.as_deref().is_none_or(|style| capture_style(style, capture))
            && self.children.iter_mut().all(|child| child.can_capture_scrolling_subtree(descendants))
    }

    /// Animate this leaf decoration's top/bottom presentation offsets in logical pixels.
    ///
    /// Each pair is `[top, bottom]` relative to stable final layout. This returns an animation
    /// wrapper without child/style mutation methods, so the validated shape stays a single Quad.
    /// Layout, hit targets and ancestor clips do not animate. Nominal corner radii stay fixed;
    /// effective radii are limited to half the shortest presented side for very short shapes.
    /// The animation's existing timing, fill and retarget policies apply.
    ///
    /// # Errors
    /// Returns `InvalidQuadGeometryTarget` for children, event/state styles, shadows, filters or
    /// image-cache hooks; returns `LayoutDrivenProperty` for a layout-driven animation.
    pub fn with_vertical_edges_animation(
        self,
        id: impl Into<ElementId>,
        animation: crate::Animation,
        from: [Pixels; 2],
        to: [Pixels; 2],
    ) -> Result<impl IntoElement, crate::VisualAnimationError> {
        use crate::AnimationExt;
        let style = &self.interactivity.base_style;
        if !self.children.is_empty()
            || self.prepaint_listener.is_some()
            || !self.interactivity_is_reconciliation_transparent()
            || style
                .box_shadow
                .as_ref()
                .is_some_and(|shadows| !shadows.is_empty())
            || style.background_blur.is_some()
            || style.filter_blur.is_some()
        {
            return Err(crate::VisualAnimationError::InvalidQuadGeometryTarget);
        }
        self.with_visual_animation(
            id,
            animation.with_property(crate::AnimationProperty::vertical_edges(from, to)),
        )
    }

    /// Add a listener to be called when the children of this `Div` are prepainted.
    /// This allows you to store the [`Bounds`] of the children for later use.
    pub fn on_children_prepainted(
        mut self,
        listener: impl Fn(Vec<Bounds<Pixels>>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.prepaint_listener = Some(Box::new(listener));
        self
    }

    /// Add an image cache at the location of this div in the element tree.
    pub fn image_cache(mut self, cache: impl ImageCacheProvider) -> Self {
        self.image_cache = Some(Box::new(cache));
        self
    }

    /// Returns whether this `Div` has no paint-time interactivity of its own.
    ///
    /// Hitbox-producing state is checked again after prepaint; this predicate covers the remaining
    /// keyboard/action/tab and state-style paths that can mutate the dispatch tree without a hitbox.
    fn interactivity_is_reconciliation_transparent(&self) -> bool {
        self.interactivity.hover_style.is_none() && self.interactivity_can_move(false)
    }

    fn interactivity_can_move(&self, restore_input: bool) -> bool {
        let interactivity = &self.interactivity;
        interactivity.hitbox_behavior == HitboxBehavior::Normal
            && interactivity.key_context.is_none()
            && !interactivity.focusable
            && interactivity.tracked_focus_handle.is_none()
            && interactivity.tracked_scroll_handle.is_none()
            && interactivity.scroll_anchor.is_none()
            && interactivity.scroll_offset.is_none()
            && interactivity.group.is_none()
            && interactivity.focus_style.is_none()
            && interactivity.in_focus_style.is_none()
            && interactivity.group_hover_style.is_none()
            && (restore_input || interactivity.active_style.is_none())
            && interactivity.group_active_style.is_none()
            && interactivity.drag_over_styles.is_empty()
            && interactivity.group_drag_over_styles.is_empty()
            && interactivity.mouse_down_listeners.is_empty()
            && interactivity.mouse_up_listeners.is_empty()
            && interactivity.mouse_move_listeners.is_empty()
            && interactivity.scroll_wheel_listeners.is_empty()
            && interactivity.key_down_listeners.is_empty()
            && interactivity.key_up_listeners.is_empty()
            && interactivity.modifiers_changed_listeners.is_empty()
            && interactivity.action_listeners.is_empty()
            && interactivity.drop_listeners.is_empty()
            && interactivity.can_drop_predicate.is_none()
            && (restore_input || interactivity.click_listeners.is_empty())
            && interactivity.drag_listener.is_none()
            && interactivity.hover_listener.is_none()
            && (restore_input || interactivity.tooltip_builder.is_none())
            && interactivity.window_control.is_none()
            && interactivity.tab_index.is_none()
            && !interactivity.tab_group
            && !interactivity.tab_stop
            && self.image_cache.is_none()
            && {
                #[cfg(any(test, feature = "test-support"))]
                {
                    interactivity.debug_selector.is_none()
                }
                #[cfg(not(any(test, feature = "test-support")))]
                {
                    true
                }
            }
    }
}

fn clips_content(style: &StyleRefinement) -> bool {
    style.overflow.x.is_some_and(|overflow| overflow != Overflow::Visible)
        || style.overflow.y.is_some_and(|overflow| overflow != Overflow::Visible)
}

fn capture_style(style: &StyleRefinement, capture: ScrollCapture) -> bool {
    style.background_blur.is_none()
        && style.filter_blur.is_none()
        && style.transition.is_none()
        && style.scale.is_none_or(|scale| scale == 1.)
        // A moving descendant clip can happen to match the viewport at
        // capture time. Its hits cannot be restored from that intersection.
        && (!clips_content(style) || match capture {
            ScrollCapture::View => false,
            ScrollCapture::Row => true,
            ScrollCapture::RoundedViewport | ScrollCapture::ClippedRow => {
                let corners = &style.corner_radii;
                [corners.top_left, corners.top_right, corners.bottom_right, corners.bottom_left]
                    .into_iter().all(|radius| radius.is_none_or(|radius| radius.is_zero()))
            }
        })
        && {
            #[cfg(debug_assertions)]
            { !style.debug.unwrap_or(false) && !style.debug_below.unwrap_or(false) }
            #[cfg(not(debug_assertions))]
            { true }
        }
}

/// A frame state for a `Div` element, which contains layout IDs for its children.
///
/// This struct is used internally by the `Div` element to manage the layout state of its children
/// during the UI update cycle. It holds a small vector of `LayoutId` values, each corresponding to
/// a child element of the `Div`. These IDs are used to query the layout engine for the computed
/// bounds of the children after the layout phase is complete.
pub struct DivLayout {
    pub(crate) child_layout_ids: SmallVec<[LayoutId; 2]>,
}

/// Paint-time state for a [`Div`].
pub struct DivPrepaint {
    hitbox: Option<Hitbox>,
    reconciliation_transparent: bool,
}

fn style_allows_traversal_only(style: &Style, window: &Window, cx: &App) -> bool {
    style.display != Display::None
        && style.visibility == Visibility::Visible
        && style.background_blur.is_none()
        && style.filter_blur.is_none()
        && style.opacity.is_none()
        && style.scale == 1.0
        && style.transition.is_none()
        && !window.debug_visualization(cx).show_layout_bounds
        && {
            #[cfg(debug_assertions)]
            {
                !style.debug && !style.debug_below && !cx.has_global::<crate::DebugBelow>()
            }
            #[cfg(not(debug_assertions))]
            {
                true
            }
        }
}

fn style_has_self_scene_primitives(style: &Style) -> bool {
    !style.box_shadow.is_empty()
        || style
            .background
            .as_ref()
            .and_then(Fill::color)
            .is_some_and(|color| !color.is_transparent())
        || (style
            .border_color
            .is_some_and(|color| !color.is_transparent())
            && style.border_widths.any(|length| !length.is_zero()))
}

fn style_is_reconciliation_transparent(style: &Style, window: &Window, cx: &App) -> bool {
    style_allows_traversal_only(style, window, cx)
        && style.overflow.x == Overflow::Visible
        && style.overflow.y == Overflow::Visible
        && style.text_style().is_none()
        && !style_has_self_scene_primitives(style)
}

impl Styled for Div {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.interactivity.base_style
    }
}

impl InteractiveElement for Div {
    fn interactivity(&mut self) -> &mut Interactivity {
        &mut self.interactivity
    }
}

impl ParentElement for Div {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        #[cfg(feature = "stacker")]
        self.children
            .extend(elements.into_iter().map(StackSafe::new));
        #[cfg(not(feature = "stacker"))]
        self.children.extend(elements);
    }
}

impl Element for Div {
    type RequestLayoutState = DivLayout;
    type PrepaintState = DivPrepaint;

    fn id(&self) -> Option<ElementId> {
        self.interactivity.element_id.clone()
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        Some(self.source_location)
    }

    #[cfg_attr(feature = "stacker", stacksafe::stacksafe)]
    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut child_layout_ids = SmallVec::new();
        let image_cache = self
            .image_cache
            .as_mut()
            .map(|provider| provider.provide(window, cx));

        let layout_id = window.with_image_cache(image_cache, |window| {
            self.interactivity.request_layout(
                global_id,
                inspector_id,
                window,
                cx,
                |style, window, cx| {
                    window.with_text_style(style.text_style().cloned(), |window| {
                        child_layout_ids = self
                            .children
                            .iter_mut()
                            .map(|child| child.request_layout(window, cx))
                            .collect::<SmallVec<_>>();
                        window.request_layout_slice(&style, &child_layout_ids)
                    })
                },
            )
        });

        (
            layout_id,
            DivLayout { child_layout_ids },
        )
    }

    #[cfg_attr(feature = "stacker", stacksafe::stacksafe)]
    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> DivPrepaint {
        let has_prepaint_listener = self.prepaint_listener.is_some();
        let mut children_bounds = Vec::with_capacity(if has_prepaint_listener {
            request_layout.child_layout_ids.len()
        } else {
            0
        });

        let mut child_min = point(Pixels::MAX, Pixels::MAX);
        let mut child_max = Point::default();
        if let Some(handle) = self.interactivity.scroll_anchor.as_ref() {
            *handle.last_origin.borrow_mut() = bounds.origin - window.element_offset();
        }
        let content_size = if request_layout.child_layout_ids.is_empty() {
            if let Some(handle) = self.interactivity.tracked_scroll_handle.as_ref() {
                let mut state = handle.0.borrow_mut();
                state.version.bump_if(!state.child_bounds.is_empty());
                state.child_bounds.clear();
            }
            bounds.size
        } else if let Some(scroll_handle) = self.interactivity.tracked_scroll_handle.as_ref() {
            let mut state = scroll_handle.0.borrow_mut();
            let mut changed = state.child_bounds.len() != request_layout.child_layout_ids.len();
            state.child_bounds.truncate(request_layout.child_layout_ids.len());
            for (index, child_layout_id) in request_layout.child_layout_ids.iter().enumerate() {
                let child_bounds = window.layout_bounds(*child_layout_id);
                child_min = child_min.min(&child_bounds.origin);
                child_max = child_max.max(&child_bounds.bottom_right());
                if let Some(previous) = state.child_bounds.get_mut(index) {
                    changed |= *previous != child_bounds;
                    *previous = child_bounds;
                } else {
                    state.child_bounds.push(child_bounds);
                }
            }
            state.version.bump_if(changed);
            (child_max - child_min).into()
        } else if self.interactivity.scroll_offset.is_none()
            && !has_prepaint_listener && inspector_id.is_none()
        {
            // No scroll clamp, child-bounds observer or inspector consumes
            // the aggregate. Avoid a second geometry lookup per child.
            bounds.size
        } else {
            for child_layout_id in &request_layout.child_layout_ids {
                let child_bounds = window.layout_bounds(*child_layout_id);
                child_min = child_min.min(&child_bounds.origin);
                child_max = child_max.max(&child_bounds.bottom_right());

                if has_prepaint_listener {
                    children_bounds.push(child_bounds);
                }
            }
            (child_max - child_min).into()
        };

        if let Some(scroll_handle) = self.interactivity.tracked_scroll_handle.as_ref() {
            scroll_handle.scroll_to_active_item();
        }

        let interactivity_transparent = self.interactivity_is_reconciliation_transparent();
        self.interactivity.prepaint(
            global_id,
            inspector_id,
            bounds,
            content_size,
            window,
            cx,
            |style, scroll_offset, hitbox, window, cx| {
                let reconciliation_transparent = interactivity_transparent
                    && hitbox.is_none()
                    && style_is_reconciliation_transparent(style, window, cx);

                // Generic Div traversal is not a progressive boundary. If this structural loop
                // merely sets `draw_was_degraded` after the deadline, GPUI still walks every child
                // and then discards the completed frame, forcing a full view-cache refresh on the
                // retry. Progressive deferral belongs at explicit retained boundaries such as
                // progressive AnyView/deferred work, where work can actually be skipped safely.
                if style.display != Display::None {
                    window.with_element_opacity(style.opacity, |window| {
                        let scrolls = global_id.is_some()
                            && style.overflow.y == Overflow::Scroll
                            && style.overflow.x != Overflow::Scroll;
                        window.with_scroll_viewport(scrolls, scroll_offset, |window| {
                            window.with_element_offset(scroll_offset, |window| {
                                for child in &mut self.children {
                                    child.prepaint(window, cx);
                                }
                            });
                        });

                        if let Some(listener) = self.prepaint_listener.as_ref() {
                            listener(children_bounds, window, cx);
                        }
                    });
                }

                DivPrepaint {
                    hitbox,
                    reconciliation_transparent,
                }
            },
        )
    }

    #[cfg_attr(feature = "stacker", stacksafe::stacksafe)]
    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut DivPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        if prepaint.reconciliation_transparent {
            window.record_element_traversal_only(bounds, cx);
            for child in &mut self.children {
                child.paint(window, cx);
            }
            return;
        }

        let image_cache = self
            .image_cache
            .as_mut()
            .map(|provider| provider.provide(window, cx));
        window.with_image_cache(image_cache, |window| {
            self.interactivity.paint(
                global_id,
                inspector_id,
                bounds,
                prepaint.hitbox.as_ref(),
                window,
                cx,
                |style, window, cx| {
                    if style.display != Display::None {
                        for child in &mut self.children {
                            child.paint(window, cx);
                        }
                    }
                },
            );
        });
    }
}

impl IntoElement for Div {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// A wrapper around an element that can store state, produced after assigning an ElementId.
pub struct Stateful<E> {
    pub(crate) element: E,
}

impl<E> Styled for Stateful<E>
where
    E: Styled,
{
    fn style(&mut self) -> &mut StyleRefinement {
        self.element.style()
    }
}

impl<E> StatefulInteractiveElement for Stateful<E>
where
    E: Element,
    Self: InteractiveElement,
{
}

impl<E> InteractiveElement for Stateful<E>
where
    E: InteractiveElement,
{
    fn interactivity(&mut self) -> &mut Interactivity {
        self.element.interactivity()
    }
}

impl<E> Element for Stateful<E>
where
    E: Element,
{
    type RequestLayoutState = E::RequestLayoutState;
    type PrepaintState = E::PrepaintState;

    fn id(&self) -> Option<ElementId> {
        self.element.id()
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        self.element.source_location()
    }

    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.element.request_layout(id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> E::PrepaintState {
        self.element
            .prepaint(id, inspector_id, bounds, state, window, cx)
    }

    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.element.paint(
            id,
            inspector_id,
            bounds,
            request_layout,
            prepaint,
            window,
            cx,
        );
    }
}

impl<E> IntoElement for Stateful<E>
where
    E: Element,
{
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<E> ParentElement for Stateful<E>
where
    E: ParentElement,
{
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.element.extend(elements);
    }
}
