use crate::{
    AbsoluteLength, AnyElement, App, BorderStyle, Bounds, BoxShadow, Corners, Display, Edges,
    Element, ElementId, Fill, GlobalElementId, Hitbox, Hsla, ImageCacheProvider,
    InspectorElementId, IntoElement, LayoutId, Overflow, ParentElement, Pixels, Point, Style,
    StyleRefinement, Styled, Visibility, Window, point,
};
use smallvec::SmallVec;
use stacksafe::{StackSafe, stacksafe};
use std::ops::Range;

use super::event::{InteractiveElement, StatefulInteractiveElement};
use super::interactivity::Interactivity;

/// Construct a new [`Div`] element.
#[track_caller]
pub fn div() -> Div {
    Div {
        interactivity: Interactivity::new(),
        source_location: core::panic::Location::caller(),
        children: SmallVec::default(),
        prepaint_listener: None,
        image_cache: None,
    }
}

/// A [`Div`] element, the all-in-one element for building complex UIs in GPUI
pub struct Div {
    interactivity: Interactivity,
    source_location: &'static core::panic::Location<'static>,
    children: SmallVec<[StackSafe<AnyElement>; 2]>,
    prepaint_listener: Option<Box<dyn Fn(Vec<Bounds<Pixels>>, &mut Window, &mut App) + 'static>>,
    image_cache: Option<Box<dyn ImageCacheProvider>>,
}

impl Div {
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
        let interactivity = &self.interactivity;
        interactivity.key_context.is_none()
            && !interactivity.focusable
            && interactivity.tracked_focus_handle.is_none()
            && interactivity.tracked_scroll_handle.is_none()
            && interactivity.scroll_anchor.is_none()
            && interactivity.scroll_offset.is_none()
            && interactivity.group.is_none()
            && interactivity.focus_style.is_none()
            && interactivity.in_focus_style.is_none()
            && interactivity.hover_style.is_none()
            && interactivity.group_hover_style.is_none()
            && interactivity.active_style.is_none()
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
            && interactivity.click_listeners.is_empty()
            && interactivity.drag_listener.is_none()
            && interactivity.hover_listener.is_none()
            && interactivity.tooltip_builder.is_none()
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

/// A frame state for a `Div` element, which contains layout IDs for its children.
///
/// This struct is used internally by the `Div` element to manage the layout state of its children
/// during the UI update cycle. It holds a small vector of `LayoutId` values, each corresponding to
/// a child element of the `Div`. These IDs are used to query the layout engine for the computed
/// bounds of the children after the layout phase is complete.
pub struct DivLayout {
    pub(crate) child_layout_ids: SmallVec<[LayoutId; 2]>,
}

/// Exact visual state for the primitives owned directly by a retained [`Div`].
///
/// Child paint context is deliberately excluded: current text and clipping state is rebuilt while
/// only the parent's shadow/background/border scene spans are replayed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RetainedDivSelfSceneStyle {
    background: Option<Fill>,
    border_color: Option<Hsla>,
    border_style: BorderStyle,
    border_widths: Edges<AbsoluteLength>,
    corner_radii: Corners<AbsoluteLength>,
    box_shadow: Vec<BoxShadow>,
}

/// Exact retained self-scene metadata emitted by a reconciliation-safe [`Div`].
///
/// `child_scene_range` is the current frame span painted by children. The parent's own prefix and
/// suffix are therefore known exactly even when the child list changes between frames.
#[derive(Clone)]
pub(crate) struct RetainedDivSelfScene {
    pub(crate) style: RetainedDivSelfSceneStyle,
    pub(crate) child_scene_range: Range<usize>,
}

/// Paint-time state for a [`Div`].
///
/// `reconciliation_transparent` means the container contributes no own paint/context work at all.
/// `self_scene_style` marks containers whose shadow/background/border can be replayed independently
/// while child lifecycle continues against the current inherited context.
pub struct DivPrepaint {
    hitbox: Option<Hitbox>,
    reconciliation_transparent: bool,
    pub(crate) self_scene_style: Option<RetainedDivSelfSceneStyle>,
    pub(crate) self_scene_child_range: Option<Range<usize>>,
}

impl DivPrepaint {
    pub(crate) fn retained_self_scene(&self) -> Option<RetainedDivSelfScene> {
        Some(RetainedDivSelfScene {
            style: self.self_scene_style.clone()?,
            child_scene_range: self.self_scene_child_range.clone()?,
        })
    }
}

fn retained_div_self_scene_style(style: &Style) -> RetainedDivSelfSceneStyle {
    RetainedDivSelfSceneStyle {
        background: style.background.clone(),
        border_color: style.border_color,
        border_style: style.border_style,
        border_widths: style.border_widths,
        corner_radii: style.corner_radii,
        box_shadow: style.box_shadow.clone(),
    }
}

fn style_allows_self_scene_replay(style: &Style, window: &Window, cx: &App) -> bool {
    style.display != Display::None
        && style.visibility == Visibility::Visible
        && style.backdrop_blur.is_none()
        && style.blur.is_none()
        && style.mouse_cursor.is_none()
        && style.opacity.is_none()
        && style.scale == 1.0
        && style.transition.is_none()
        && !window.debug_visualization(cx).show_layout_bounds
        && {
            #[cfg(debug_assertions)]
            {
                !style.debug && !style.debug_below
            }
            #[cfg(not(debug_assertions))]
            {
                true
            }
        }
}

fn style_is_reconciliation_transparent(style: &Style, window: &Window, cx: &App) -> bool {
    style_allows_self_scene_replay(style, window, cx)
        && style.overflow.x == Overflow::Visible
        && style.overflow.y == Overflow::Visible
        && style.text_style().is_none()
        && style.background.is_none()
        && style.border_color.is_none()
        && style.box_shadow.is_empty()
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
        self.children
            .extend(elements.into_iter().map(StackSafe::new))
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

    #[stacksafe]
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
                        window.request_layout(style, child_layout_ids.iter().copied(), cx)
                    })
                },
            )
        });

        (layout_id, DivLayout { child_layout_ids })
    }

    #[stacksafe]
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
            bounds.size
        } else if let Some(scroll_handle) = self.interactivity.tracked_scroll_handle.as_ref() {
            let mut state = scroll_handle.0.borrow_mut();
            state.child_bounds.clear();
            if state.child_bounds.capacity() < request_layout.child_layout_ids.len() {
                state
                    .child_bounds
                    .reserve(request_layout.child_layout_ids.len());
            }
            for child_layout_id in &request_layout.child_layout_ids {
                let child_bounds = window.layout_bounds(*child_layout_id);
                child_min = child_min.min(&child_bounds.origin);
                child_max = child_max.max(&child_bounds.bottom_right());
                state.child_bounds.push(child_bounds);
            }
            (child_max - child_min).into()
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
                let self_scene_replayable = interactivity_transparent
                    && hitbox.is_none()
                    && style_allows_self_scene_replay(style, window, cx);
                let reconciliation_transparent = self_scene_replayable
                    && style_is_reconciliation_transparent(style, window, cx);
                let self_scene_style = self_scene_replayable
                    .then(|| retained_div_self_scene_style(style));

                // Generic Div traversal is not a progressive boundary. If this structural loop
                // merely sets `draw_was_degraded` after the deadline, GPUI still walks every child
                // and then discards the completed frame, forcing a full view-cache refresh on the
                // retry. Progressive deferral belongs at explicit retained boundaries such as
                // progressive AnyView/deferred work, where work can actually be skipped safely.
                if style.display != Display::None {
                    window.with_element_offset(scroll_offset, |window| {
                        for child in &mut self.children {
                            child.prepaint(window, cx);
                        }
                    });

                    if let Some(listener) = self.prepaint_listener.as_ref() {
                        listener(children_bounds, window, cx);
                    }
                }

                DivPrepaint {
                    hitbox,
                    reconciliation_transparent,
                    self_scene_style,
                    self_scene_child_range: None,
                }
            },
        )
    }

    #[stacksafe]
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
        let debug_below_active = {
            #[cfg(debug_assertions)]
            {
                cx.has_global::<crate::DebugBelow>()
            }
            #[cfg(not(debug_assertions))]
            {
                false
            }
        };

        // Parent self-scene reuse is safe even on a generic application dirty frame: the exact
        // background/shadow/border state is compared with the previous retained range, while
        // children still execute against the current text/clipping context. Frame-bound input
        // state is never replayed by this path.
        if let Some(self_scene_style) = prepaint.self_scene_style.as_ref()
            && !debug_below_active
            && !window.debug_visualization(cx).show_layout_bounds
            && let Some(ranges) =
                window.retained_self_scene_ranges_for_current(bounds, self_scene_style)
        {
            let style = self
                .interactivity
                .compute_style(global_id, prepaint.hitbox.as_ref(), window, cx);
            window.record_debug_element_self_scene_replay(bounds, cx);
            window.replay_retained_scene_range(ranges.prefix);
            let child_scene_start = window.next_frame.scene.len();
            window.with_text_style(style.text_style().cloned(), |window| {
                window.with_content_mask(style.overflow_mask(bounds, window.rem_size()), |window| {
                    for child in &mut self.children {
                        child.paint(window, cx);
                    }
                });
            });
            let child_scene_end = window.next_frame.scene.len();
            prepaint.self_scene_child_range = Some(child_scene_start..child_scene_end);
            window.replay_retained_scene_range(ranges.suffix);
            return;
        }

        if prepaint.reconciliation_transparent
            && !debug_below_active
            && !window.debug_visualization(cx).show_layout_bounds
        {
            window.record_debug_element_traversal_only(bounds, cx);
            let child_scene_start = window.next_frame.scene.len();
            for child in &mut self.children {
                child.paint(window, cx);
            }
            let child_scene_end = window.next_frame.scene.len();
            if prepaint.self_scene_style.is_some() {
                prepaint.self_scene_child_range = Some(child_scene_start..child_scene_end);
            }
            return;
        }

        let image_cache = self
            .image_cache
            .as_mut()
            .map(|provider| provider.provide(window, cx));
        let mut self_scene_child_range = None;

        window.with_image_cache(image_cache, |window| {
            self.interactivity.paint(
                global_id,
                inspector_id,
                bounds,
                prepaint.hitbox.as_ref(),
                window,
                cx,
                |style, window, cx| {
                    // skip children
                    if style.display == Display::None {
                        return;
                    }

                    let child_scene_start = window.next_frame.scene.len();
                    for child in &mut self.children {
                        child.paint(window, cx);
                    }
                    let child_scene_end = window.next_frame.scene.len();
                    self_scene_child_range = Some(child_scene_start..child_scene_end);
                },
            )
        });

        if prepaint.self_scene_style.is_some() {
            prepaint.self_scene_child_range = self_scene_child_range;
        }
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
