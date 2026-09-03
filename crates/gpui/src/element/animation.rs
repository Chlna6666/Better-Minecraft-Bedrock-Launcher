use std::{
    rc::Rc,
    time::{Duration, Instant},
};

use crate::{
    AnimationDriver, AnimationSpec, AnyElement, App, Bounds, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, LegacyAnimationTimeline, LegacyAnimationTiming, Pixels, Point,
    Radians, RepeatMode, SceneAnimationId, TransformOrigin, TransitionProperty, Window,
    sample_legacy_easing,
};

pub use easing::*;
mod timing;
use smallvec::SmallVec;
use timing::sample_element_animation;

const REPEATING_ANIMATION_FRAME_INTERVAL: Duration = Duration::from_millis(3);
const SPRING_TRANSLATION_PROGRESS_MIN: f32 = -0.05;
const SPRING_TRANSLATION_PROGRESS_MAX: f32 = 2.05;

#[derive(Clone)]
pub struct Animation {
    pub duration: Duration,
    pub oneshot: bool,
    pub easing: Rc<dyn Fn(f32) -> f32>,
    spec: AnimationSpec,
    property: Option<AnimationProperty>,
    spring: Option<crate::Spring>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimationProperty {
    property: TransitionProperty,
    from: [f32; 4],
    to: [f32; 4],
}

impl AnimationProperty {
    pub fn opacity(from: f32, to: f32) -> Self {
        Self {
            property: TransitionProperty::Opacity,
            from: [from.clamp(0.0, 1.0), 0.0, 0.0, 0.0],
            to: [to.clamp(0.0, 1.0), 0.0, 0.0, 0.0],
        }
    }

    pub fn rotation(from: impl Into<Radians>, to: impl Into<Radians>) -> Self {
        Self {
            property: TransitionProperty::Rotation,
            from: [from.into().0, 0.0, 0.0, 0.0],
            to: [to.into().0, 0.0, 0.0, 0.0],
        }
    }

    pub fn translation(from: Point<Pixels>, to: Point<Pixels>) -> Self {
        Self {
            property: TransitionProperty::Translation,
            from: [from.x.0, from.y.0, 0.0, 0.0],
            to: [to.x.0, to.y.0, 0.0, 0.0],
        }
    }

    pub fn scale_opacity(
        from_scale: f32,
        to_scale: f32,
        from_opacity: f32,
        to_opacity: f32,
        origin: TransformOrigin,
    ) -> Self {
        Self {
            property: TransitionProperty::Transform,
            from: [from_scale, from_opacity.clamp(0.0, 1.0), origin.x, origin.y],
            to: [to_scale, to_opacity.clamp(0.0, 1.0), origin.x, origin.y],
        }
    }

    fn resolved_values(self, bounds: Bounds<Pixels>, scale_factor: f32) -> ([f32; 4], [f32; 4]) {
        if self.property != TransitionProperty::Transform {
            return (self.from, self.to);
        }
        let origin = TransformOrigin::new(self.from[2], self.from[3]).resolve(bounds);
        let mut from = self.from;
        let mut to = self.to;
        from[2] = origin.x.0 * scale_factor;
        from[3] = origin.y.0 * scale_factor;
        to[2] = from[2];
        to[3] = from[3];
        (from, to)
    }

    fn dirty_bounds(self, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        match self.property {
            TransitionProperty::Translation => {
                translated_bounds(bounds, self.from).union(&translated_bounds(bounds, self.to))
            }
            TransitionProperty::Rotation => rotation_bounds(bounds),
            _ => bounds,
        }
    }

    fn spring_translation_dirty_bounds(self, bounds: Bounds<Pixels>) -> Option<Bounds<Pixels>> {
        if self.property != TransitionProperty::Translation {
            return None;
        }
        let first = translated_bounds_at_progress(
            bounds,
            self.from,
            self.to,
            SPRING_TRANSLATION_PROGRESS_MIN,
        );
        let last = translated_bounds_at_progress(
            bounds,
            self.from,
            self.to,
            SPRING_TRANSLATION_PROGRESS_MAX,
        );
        Some(first.union(&last))
    }
}

impl Animation {
    pub fn new(duration: Duration) -> Self {
        Self::from_spec(AnimationSpec::new(duration))
    }

    pub fn spring(spring: crate::Spring) -> Self {
        let mut animation =
            Self::from_spec(AnimationSpec::new(Duration::ZERO).driver(AnimationDriver::Paint));
        animation.spring = Some(spring);
        animation
    }

    pub fn from_spec(spec: AnimationSpec) -> Self {
        let easing = spec.easing.clone();
        Self {
            duration: spec.duration,
            oneshot: !matches!(spec.repeat, RepeatMode::Forever),
            easing: Rc::new(move |progress| easing.sample(progress)),
            spec,
            property: None,
            spring: None,
        }
    }

    pub fn repeat(mut self) -> Self {
        self.oneshot = false;
        self.spec.repeat = RepeatMode::Forever;
        self
    }

    pub fn with_easing(mut self, easing: impl Fn(f32) -> f32 + 'static) -> Self {
        self.spring = None;
        let easing = Rc::new(easing);
        self.easing = easing.clone();
        self.spec.easing = crate::Easing::Custom(easing);
        self
    }

    pub fn with_property(mut self, property: AnimationProperty) -> Self {
        self.property = Some(property);
        self
    }

    fn scene_animation(&self) -> Option<(AnimationProperty, &AnimationSpec)> {
        let property = self.property?;
        (!matches!(self.spec.driver, AnimationDriver::Layout)).then_some((property, &self.spec))
    }
}

pub trait AnimationExt {
    fn with_animation(
        self,
        id: impl Into<ElementId>,
        animation: Animation,
        animator: impl Fn(Self, f32) -> Self + 'static,
    ) -> AnimationElement<Self>
    where
        Self: Sized,
    {
        AnimationElement {
            id: id.into(),
            element: Some(self),
            animator: Box::new(move |this, _, value| animator(this, value)),
            animations: smallvec::smallvec![animation],
        }
    }

    fn with_animations(
        self,
        id: impl Into<ElementId>,
        animations: Vec<Animation>,
        animator: impl Fn(Self, usize, f32) -> Self + 'static,
    ) -> AnimationElement<Self>
    where
        Self: Sized,
    {
        AnimationElement {
            id: id.into(),
            element: Some(self),
            animator: Box::new(animator),
            animations: animations.into(),
        }
    }

    fn with_sampled_animation(
        self,
        property: AnimationProperty,
        progress: f32,
    ) -> SampledAnimationElement<Self>
    where
        Self: Sized,
    {
        SampledAnimationElement {
            element: Some(self),
            property,
            progress,
        }
    }

    fn with_layout_animation_target(
        self,
        id: impl Into<ElementId>,
        animating: bool,
    ) -> LayoutAnimationTargetElement<Self>
    where
        Self: Sized,
    {
        LayoutAnimationTargetElement {
            id: id.into(),
            element: Some(self),
            animating,
        }
    }
}

impl<E: IntoElement + 'static> AnimationExt for E {}

pub struct LayoutAnimationTargetElement<E> {
    id: ElementId,
    element: Option<E>,
    animating: bool,
}

impl<E: IntoElement + 'static> IntoElement for LayoutAnimationTargetElement<E> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<E: IntoElement + 'static> Element for LayoutAnimationTargetElement<E> {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
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
                .expect("layout animation target must have a retained identity");
            window.request_layout_animation_frame(retained_id);
        }

        let mut element = self
            .element
            .take()
            .expect("layout animation target should only be laid out once")
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

pub struct SampledAnimationElement<E> {
    element: Option<E>,
    property: AnimationProperty,
    progress: f32,
}

impl<E: IntoElement + 'static> IntoElement for SampledAnimationElement<E> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<E: IntoElement + 'static> Element for SampledAnimationElement<E> {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (crate::LayoutId, Self::RequestLayoutState) {
        let mut element = self
            .element
            .take()
            .expect("sampled animation element should only be laid out once")
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
        bounds: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (from, to) = self.property.resolved_values(bounds, window.scale_factor());
        window.with_sampled_scene_animation(
            self.property.property,
            self.progress,
            from,
            to,
            |window| element.paint(window, cx),
        );
    }
}

pub struct AnimationElement<E> {
    id: ElementId,
    element: Option<E>,
    animations: SmallVec<[Animation; 1]>,
    animator: Box<dyn Fn(E, usize, f32) -> E + 'static>,
}

impl<E> AnimationElement<E> {
    pub fn map_element(mut self, f: impl FnOnce(E) -> E) -> AnimationElement<E> {
        self.element = self.element.map(f);
        self
    }
}

impl<E: IntoElement + 'static> IntoElement for AnimationElement<E> {
    type Element = AnimationElement<E>;

    fn into_element(self) -> Self::Element {
        self
    }
}

struct AnimationState(LegacyAnimationTimeline);

#[derive(Clone, Debug, PartialEq)]
struct SceneAnimationState {
    animation_id: SceneAnimationId,
    property: AnimationProperty,
    spec: AnimationSpec,
    spring: Option<crate::Spring>,
}

impl<E: IntoElement + 'static> Element for AnimationElement<E> {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (crate::LayoutId, Self::RequestLayoutState) {
        if self.animations.is_empty() {
            let element = self.element.take().expect("should only be called once");
            let mut element = element.into_any_element();
            return (element.request_layout(window, cx), element);
        }

        if let Some((animation_index, progress)) = self.initial_scene_animation_sample() {
            let element = self.element.take().expect("should only be called once");
            let mut element =
                (self.animator)(element, animation_index, progress).into_any_element();
            return (element.request_layout(window, cx), element);
        }

        let global_id =
            global_id.expect("AnimationElement always supplies an element id for state tracking");
        let retained_id = window
            .current_retained_element_id()
            .expect("AnimationElement must have a retained identity");
        window.with_element_state(global_id, |state, window| {
            let now = window.animation_time();
            let mut state =
                state.unwrap_or_else(|| AnimationState(LegacyAnimationTimeline::new(now)));
            let (animation_ix, delta, done) =
                sample_element_animation(&mut state.0, &self.animations, now);

            debug_assert!(delta.is_finite(), "eased progress must be finite");

            let element = self.element.take().expect("should only be called once");
            let mut element = (self.animator)(element, animation_ix, delta).into_any_element();

            let repeats = self
                .animations
                .get(animation_ix)
                .is_some_and(|animation| !animation.oneshot);
            schedule_next_animation_frame(window, cx, now, done, repeats, &retained_id);

            ((element.request_layout(window, cx), element), state)
        })
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: crate::Bounds<crate::Pixels>,
        element: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        element.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: crate::Bounds<crate::Pixels>,
        element: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some((property, spec)) = self
            .scene_animation()
            .map(|(property, spec)| (property, spec.clone()))
        else {
            element.paint(window, cx);
            return;
        };
        let global_id =
            global_id.expect("AnimationElement always supplies an element id for state tracking");
        let spring = self.animations[0].spring;
        let dirty_bounds = if property.property == TransitionProperty::Translation {
            Bounds::new(Point::default(), window.viewport_size())
        } else {
            property.dirty_bounds(bounds)
        };
        let animation_id =
            window.with_element_state(global_id, |state: Option<SceneAnimationState>, window| {
                let state = match state {
                    Some(state)
                        if state.property == property
                            && state.spec == spec
                            && state.spring == spring => state,
                    _ => {
                        let animation_id = window.start_scene_animation(
                            global_id,
                            property.property,
                            spec.clone(),
                            dirty_bounds,
                            property.from,
                            property.to,
                        );
                        if let Some(spring) = spring {
                            window.set_scene_animation_spring(global_id, property.property, spring);
                        }
                        SceneAnimationState {
                            animation_id,
                            property,
                            spec,
                            spring,
                        }
                    }
                };
                (state.animation_id, state)
            });
        window.with_scene_animation(animation_id, property.property, |window| {
            element.paint(window, cx);
            if spring.is_some()
                && property.property == TransitionProperty::Translation
                && let Some(bounds) = window.scene_animation_visual_bounds(animation_id)
                && let Some(dirty_bounds) = property.spring_translation_dirty_bounds(bounds)
            {
                let _ = window.set_scene_animation_dirty_bounds(
                    global_id,
                    property.property,
                    dirty_bounds,
                );
            }
        });
    }
}

impl<E> AnimationElement<E> {
    fn scene_animation(&self) -> Option<(AnimationProperty, &AnimationSpec)> {
        (self.animations.len() == 1)
            .then(|| self.animations.first()?.scene_animation())
            .flatten()
    }

    fn initial_scene_animation_sample(&self) -> Option<(usize, f32)> {
        let (_, spec) = self.scene_animation()?;
        Some((
            0,
            if self.animations[0].spring.is_some() {
                0.0
            } else {
                spec.sample_elapsed(Duration::ZERO).eased_progress
            },
        ))
    }
}

fn translated_bounds(bounds: Bounds<Pixels>, translation: [f32; 4]) -> Bounds<Pixels> {
    Bounds::new(
        bounds.origin + Point::new(crate::px(translation[0]), crate::px(translation[1])),
        bounds.size,
    )
}

fn translated_bounds_at_progress(
    bounds: Bounds<Pixels>,
    from: [f32; 4],
    to: [f32; 4],
    progress: f32,
) -> Bounds<Pixels> {
    let translation = [
        from[0] + (to[0] - from[0]) * progress,
        from[1] + (to[1] - from[1]) * progress,
        0.0,
        0.0,
    ];
    translated_bounds(bounds, translation)
}

fn rotation_bounds(bounds: Bounds<Pixels>) -> Bounds<Pixels> {
    let radius = (bounds.size.width.0.mul_add(
        bounds.size.width.0,
        bounds.size.height.0 * bounds.size.height.0,
    ))
    .sqrt()
        * 0.5;
    Bounds::new(
        Point::new(
            bounds.center().x - crate::px(radius),
            bounds.center().y - crate::px(radius),
        ),
        crate::size(crate::px(radius * 2.0), crate::px(radius * 2.0)),
    )
}

fn schedule_next_animation_frame(
    window: &Window,
    cx: &App,
    now: Instant,
    done: bool,
    repeats: bool,
    retained_id: &GlobalElementId,
) {
    match next_animation_frame_delay(done, repeats, window.is_window_active()) {
        None => {}
        Some(delay) if delay.is_zero() => {
            window.request_layout_animation_frame(retained_id.clone());
        }
        Some(delay) => {
            window.request_layout_animation_frame_at(retained_id.clone(), now + delay, cx);
        }
    }
}

fn next_animation_frame_delay(done: bool, repeats: bool, window_active: bool) -> Option<Duration> {
    if done || (repeats && !window_active) {
        None
    } else if repeats {
        Some(REPEATING_ANIMATION_FRAME_INTERVAL)
    } else {
        Some(Duration::ZERO)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_opacity_uses_scene_animation_metadata() {
        let animation = Animation::new(Duration::from_millis(900))
            .with_property(AnimationProperty::opacity(0.25, 0.9));
        let (property, spec) = animation.scene_animation().expect("scene animation");
        assert_eq!(property.property, TransitionProperty::Opacity);
        assert_eq!(property.from, [0.25, 0.0, 0.0, 0.0]);
        assert_eq!(property.to, [0.9, 0.0, 0.0, 0.0]);
        assert_eq!(spec.driver, AnimationDriver::Auto);
    }

    #[test]
    fn declared_rotation_uses_scene_animation_metadata() {
        let animation = Animation::new(Duration::from_millis(900)).with_property(
            AnimationProperty::rotation(crate::radians(0.0), crate::radians(1.0)),
        );
        let (property, spec) = animation.scene_animation().expect("scene animation");
        assert_eq!(property.property, TransitionProperty::Rotation);
        assert_eq!(property.from, [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(property.to, [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(spec.driver, AnimationDriver::Auto);
    }

    #[test]
    fn declared_translation_expands_dirty_bounds_across_motion() {
        let property = AnimationProperty::translation(
            Point::new(crate::px(0.0), crate::px(0.0)),
            Point::new(crate::px(40.0), crate::px(10.0)),
        );
        let bounds = Bounds::new(
            Point::new(crate::px(5.0), crate::px(7.0)),
            crate::size(crate::px(20.0), crate::px(30.0)),
        );
        assert_eq!(
            property.dirty_bounds(bounds),
            Bounds::new(
                Point::new(crate::px(5.0), crate::px(7.0)),
                crate::size(crate::px(60.0), crate::px(40.0)),
            )
        );
    }

    #[test]
    fn spring_translation_dirty_bounds_cover_physical_overshoot_without_viewport_damage() {
        let property = AnimationProperty::translation(
            Point::new(crate::px(18.0), crate::px(0.0)),
            Point::new(crate::px(0.0), crate::px(0.0)),
        );
        let bounds = Bounds::new(
            Point::new(crate::px(100.0), crate::px(40.0)),
            crate::size(crate::px(200.0), crate::px(120.0)),
        );
        let dirty = property
            .spring_translation_dirty_bounds(bounds)
            .expect("translation envelope");
        assert!(dirty.origin.x < bounds.origin.x);
        assert!(dirty.size.width > bounds.size.width);
        assert!(dirty.size.width < crate::px(240.0));
        assert_eq!(dirty.origin.y, bounds.origin.y);
        assert_eq!(dirty.size.height, bounds.size.height);
    }

    #[test]
    fn explicit_layout_driver_keeps_legacy_animation_path() {
        let animation = Animation::from_spec(
            AnimationSpec::new(Duration::from_millis(100)).driver(AnimationDriver::Layout),
        )
        .with_property(AnimationProperty::rotation(
            crate::radians(0.0),
            crate::radians(1.0),
        ));
        assert!(animation.scene_animation().is_none());
    }

    #[test]
    fn repeating_animation_uses_gpui_frame_cadence() {
        assert_eq!(
            next_animation_frame_delay(false, true, true),
            Some(REPEATING_ANIMATION_FRAME_INTERVAL)
        );
    }

    #[test]
    fn inactive_repeating_animation_stops_scheduling() {
        assert_eq!(next_animation_frame_delay(false, true, false), None);
    }

    #[test]
    fn finite_animation_keeps_immediate_frame_scheduling() {
        assert_eq!(
            next_animation_frame_delay(false, false, true),
            Some(Duration::ZERO)
        );
    }
}

mod easing {
    use std::f32::consts::PI;

    pub fn linear(delta: f32) -> f32 {
        delta
    }

    pub fn quadratic(delta: f32) -> f32 {
        delta * delta
    }

    pub fn ease_in_out(delta: f32) -> f32 {
        if delta < 0.5 {
            2.0 * delta * delta
        } else {
            let x = -2.0 * delta + 2.0;
            1.0 - x * x / 2.0
        }
    }

    pub fn ease_out_quint() -> impl Fn(f32) -> f32 {
        move |delta| 1.0 - (1.0 - delta).powi(5)
    }

    pub fn bounce(easing: impl Fn(f32) -> f32) -> impl Fn(f32) -> f32 {
        move |delta| {
            if delta < 0.5 {
                easing(delta * 2.0)
            } else {
                easing((1.0 - delta) * 2.0)
            }
        }
    }

    pub fn pulsating_between(min: f32, max: f32) -> impl Fn(f32) -> f32 {
        let range = max - min;
        move |delta| {
            let t = (delta * 2.0 * PI).sin();
            let breath = (t * t * t + t) / 2.0;
            let normalized_alpha = (breath + 1.0) / 2.0;
            min + (normalized_alpha * range)
        }
    }
}
