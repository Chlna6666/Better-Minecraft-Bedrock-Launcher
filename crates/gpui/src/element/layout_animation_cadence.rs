use std::time::Duration;

use crate::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    Pixels, Window,
};

/// Extension methods for caller-sampled CPU/layout animation that does not need display-rate
/// cadence.
///
/// This is intended for discrete effects whose visible state changes at a known lower frequency,
/// such as text scrambling or low-FPS status indicators. Smooth geometry animation should keep
/// using [`crate::AnimationExt::with_layout_animation_target`].
pub trait LayoutAnimationCadenceExt {
    /// Attach a retained layout-animation target that wakes no more often than `interval`.
    ///
    /// The target keeps the same `ReconcileSubtree` semantics as the ordinary layout animation
    /// target, but uses GPUI's coalesced per-target deadline instead of requesting every compositor
    /// frame. Repeated renders cannot stack timers: the earliest pending deadline for the retained
    /// target wins.
    #[track_caller]
    fn with_layout_animation_target_interval(
        self,
        animating: bool,
        interval: Duration,
    ) -> LayoutAnimationCadenceElement<Self>
    where
        Self: Sized,
    {
        LayoutAnimationCadenceElement {
            source: core::panic::Location::caller(),
            element: Some(self),
            animating,
            interval,
        }
    }
}

impl<E: IntoElement + 'static> LayoutAnimationCadenceExt for E {}

/// A retained invalidation boundary for lower-frequency caller-sampled layout animation.
pub struct LayoutAnimationCadenceElement<E> {
    source: &'static core::panic::Location<'static>,
    element: Option<E>,
    animating: bool,
    interval: Duration,
}

impl<E: IntoElement + 'static> IntoElement for LayoutAnimationCadenceElement<E> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<E: IntoElement + 'static> Element for LayoutAnimationCadenceElement<E> {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        Some(self.source)
    }

    fn request_layout(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (crate::LayoutId, Self::RequestLayoutState) {
        if self.animating {
            let retained_id = window
                .current_retained_element_id()
                .expect("layout animation cadence target must have a retained identity");
            if self.interval.is_zero() {
                window.request_layout_animation_frame(retained_id);
            } else {
                window.request_layout_animation_frame_at(
                    retained_id,
                    window.animation_time() + self.interval,
                    cx,
                );
            }
        }

        let mut element = self
            .element
            .take()
            .expect("layout animation cadence target should only be laid out once")
            .into_any_element();
        (element.request_layout(window, cx), element)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        element.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        element.paint(window, cx);
    }
}
