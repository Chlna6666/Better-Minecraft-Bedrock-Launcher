use std::{
    cell::Cell,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

use crate::{
    AnimationDriver, AnimationSpec, AnyElement, App, Bounds, Element, ElementId, GlobalElementId,
    InspectorElementId, IntoElement, Pixels, Point, Radians, RepeatMode, SceneAnimationId,
    TransformOrigin, TransitionProperty, Window,
};

pub use easing::*;
mod timing;
use smallvec::SmallVec;
use timing::{ElementAnimationTimeline, sample_element_animation};

// A zero-initial-velocity damped spring step stays inside normalized progress [0, 2]. Keep a small
// numerical guard so retained partial-presentation damage cannot clip an extremal undamped sample.
const SPRING_TRANSLATION_PROGRESS_MIN: f32 = -0.05;
const SPRING_TRANSLATION_PROGRESS_MAX: f32 = 2.05;
// Scene-local sampled ids occupy the low range and engine-owned timelines occupy the high bit.
// Reserve the middle quarter for caller-sampled animations whose primitives must keep one identity
// across retained replay frames.
const STABLE_SAMPLED_ANIMATION_ID_START: u32 = 1 << 30;
const ENGINE_ANIMATION_ID_START: u32 = 1 << 31;
static NEXT_STABLE_SAMPLED_ANIMATION_ID: AtomicU32 =
    AtomicU32::new(STABLE_SAMPLED_ANIMATION_ID_START);

fn allocate_stable_sampled_animation_id() -> SceneAnimationId {
    let id = NEXT_STABLE_SAMPLED_ANIMATION_ID.fetch_add(1, Ordering::Relaxed);
    assert!(
        id < ENGINE_ANIMATION_ID_START,
        "stable sampled animation id space exhausted"
    );
    SceneAnimationId(id)
}

/// An animation that can be applied to an element.
#[derive(Clone)]
pub struct Animation {
    spec: AnimationSpec,
    property: Option<AnimationProperty>,
    spring: Option<crate::Spring>,
}

/// Why an animation cannot use the callback-free visual animation path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VisualAnimationError {
    /// A renderer-owned visual property was not declared.
    MissingProperty,
    /// Layout-driven animations need an animator callback and cannot use the presentation lane.
    LayoutDrivenProperty(TransitionProperty),
}

impl std::fmt::Display for VisualAnimationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingProperty => {
                formatter.write_str("a visual animation must declare its animated property")
            }
            Self::LayoutDrivenProperty(property) => {
                write!(formatter, "{property:?} is layout-driven")
            }
        }
    }
}

impl std::error::Error for VisualAnimationError {}

/// A small set of independent visual property tracks committed as one retained target.
///
/// Each track keeps its own duration, easing, and spring. GPUI samples the tracks together on the
/// presentation owner and packs their current opacity, translation, and scale into one renderer
/// value, so the group adds no nested composite layers. Scale keeps the existing primitive-center
/// pivot semantics; use a composite transform when the whole subtree must scale around one shared
/// origin.
#[derive(Clone)]
pub struct AnimationGroup {
    tracks: SmallVec<[Animation; 3]>,
}

/// Why an [`AnimationGroup`] cannot be created from the supplied tracks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnimationGroupError {
    /// A group needs at least one track.
    Empty,
    /// At most three independent visual properties fit in one packed renderer value.
    TooManyTracks,
    /// Every track must declare an [`AnimationProperty`].
    MissingVisualProperty,
    /// Layout-driven tracks cannot run on the presentation owner.
    LayoutDrivenProperty(TransitionProperty),
    /// The packed group currently supports only opacity, translation, and scale.
    UnsupportedProperty(TransitionProperty),
    /// Properties that capture a subtree must keep using their composite animation wrapper.
    CapturedSubtreeProperty(TransitionProperty),
    /// A group can contain each visual property only once.
    DuplicateProperty(TransitionProperty),
    /// Combined translation-opacity values overlap the group's independent opacity channel.
    CombinedTranslationOpacity,
}

impl std::fmt::Display for AnimationGroupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => formatter.write_str("an animation group cannot be empty"),
            Self::TooManyTracks => {
                formatter.write_str("an animation group supports at most three tracks")
            }
            Self::MissingVisualProperty => {
                formatter.write_str("each animation group track must declare a visual property")
            }
            Self::LayoutDrivenProperty(property) => {
                write!(
                    formatter,
                    "{property:?} is layout-driven and cannot run in a visual group"
                )
            }
            Self::UnsupportedProperty(property) => {
                write!(
                    formatter,
                    "{property:?} is not supported by packed visual groups"
                )
            }
            Self::CapturedSubtreeProperty(property) => {
                write!(
                    formatter,
                    "{property:?} captures a subtree and cannot run in a packed visual group"
                )
            }
            Self::DuplicateProperty(property) => {
                write!(
                    formatter,
                    "{property:?} appears more than once in the animation group"
                )
            }
            Self::CombinedTranslationOpacity => formatter.write_str(
                "translation-opacity must use separate tracks when declared in an animation group",
            ),
        }
    }
}

impl std::error::Error for AnimationGroupError {}

impl AnimationGroup {
    /// Animate a background decoration with two independent horizontal springs.
    ///
    /// Both offsets are relative to its stable final layout. The smaller offset moves its left
    /// edge; their absolute difference extends its width. Height, border widths, and corner radii
    /// remain unchanged. This is intended for quad/shadow decorations without text or images;
    /// it does not change child layout or hit targets. Stable element identity preserves current
    /// presentation values and velocities when either target changes.
    pub fn horizontal_edges(
        from: Pixels,
        to: Pixels,
        first: crate::Spring,
        second: crate::Spring,
    ) -> Self {
        let track = |spring, property| {
            Animation::spring(spring).with_property(AnimationProperty {
                property,
                from: [from.0, 0.0, 0.0, 0.0],
                to: [to.0, 0.0, 0.0, 0.0],
                capture_subtree: false,
            })
        };
        Self {
            tracks: smallvec::smallvec![
                track(first, TransitionProperty::HorizontalEdgeFirst),
                track(second, TransitionProperty::HorizontalEdgeSecond),
            ],
        }
    }

    /// Create parallel compositor tracks for one retained element.
    ///
    /// The group accepts one to three unique opacity, translation, and scale properties. Tracks
    /// retain independent specs and springs; geometry/layout properties and subtree-capture
    /// properties remain on their existing APIs. A scale track scales each primitive around that
    /// primitive's center, matching [`AnimationProperty::uniform_scale`].
    pub fn parallel(
        tracks: impl IntoIterator<Item = Animation>,
    ) -> Result<Self, AnimationGroupError> {
        let mut group_tracks = SmallVec::<[Animation; 3]>::new();
        for track in tracks {
            if group_tracks.len() == 3 {
                return Err(AnimationGroupError::TooManyTracks);
            }
            group_tracks.push(track);
        }
        if group_tracks.is_empty() {
            return Err(AnimationGroupError::Empty);
        }

        let mut properties = SmallVec::<[TransitionProperty; 3]>::new();
        for track in &group_tracks {
            let Some(property) = track.property else {
                return Err(AnimationGroupError::MissingVisualProperty);
            };
            if matches!(track.spec.driver, AnimationDriver::Layout) {
                return Err(AnimationGroupError::LayoutDrivenProperty(property.property));
            }
            if property.capture_subtree {
                return Err(AnimationGroupError::CapturedSubtreeProperty(
                    property.property,
                ));
            }
            if !matches!(
                property.property,
                TransitionProperty::Opacity
                    | TransitionProperty::Translation
                    | TransitionProperty::Scale
            ) {
                return Err(AnimationGroupError::UnsupportedProperty(property.property));
            }
            if property.property == TransitionProperty::Translation
                && (property.from[3] == 1.0 || property.to[3] == 1.0)
            {
                return Err(AnimationGroupError::CombinedTranslationOpacity);
            }
            if properties.contains(&property.property) {
                return Err(AnimationGroupError::DuplicateProperty(property.property));
            }
            properties.push(property.property);
        }

        Ok(Self {
            tracks: group_tracks,
        })
    }
}

/// A renderer-owned visual property animated by [`AnimationExt::with_animation`].
///
/// Declaring one of these properties lets GPUI select the GPU or paint driver
/// without changing the `with_animation` API. Animations without a declared
/// visual property retain the legacy layout-driven behavior.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimationProperty {
    property: TransitionProperty,
    from: [f32; 4],
    to: [f32; 4],
    capture_subtree: bool,
}

/// The fixed edge from which a horizontal renderer-owned reveal exposes its child.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HorizontalRevealEdge {
    /// Reveal toward the right while keeping the child's left edge fixed.
    Left,
    /// Reveal toward the left while keeping the child's right edge fixed.
    Right,
}

impl AnimationProperty {
    /// Animate visual opacity without changing layout.
    pub fn opacity(from: f32, to: f32) -> Self {
        Self {
            property: TransitionProperty::Opacity,
            from: [from.clamp(0.0, 1.0), 0.0, 0.0, 0.0],
            to: [to.clamp(0.0, 1.0), 0.0, 0.0, 0.0],
            capture_subtree: false,
        }
    }

    /// Animate visual scale around each primitive's static center without changing layout.
    ///
    /// This property can be combined with opacity and translation in an [`AnimationGroup`].
    /// Use [`AnimationProperty::scale_opacity`] when both values need one explicit shared origin.
    pub fn uniform_scale(from: f32, to: f32) -> Self {
        Self {
            property: TransitionProperty::Scale,
            from: [from, 0.0, 0.0, 0.0],
            to: [to, 0.0, 0.0, 0.0],
            capture_subtree: false,
        }
    }

    /// Animate a visual rotation around the element center.
    pub fn rotation(from: impl Into<Radians>, to: impl Into<Radians>) -> Self {
        Self {
            property: TransitionProperty::Rotation,
            from: [from.into().0, 0.0, 0.0, 0.0],
            to: [to.into().0, 0.0, 0.0, 0.0],
            capture_subtree: false,
        }
    }

    /// Animate a CSS-style Gaussian blur radius for the whole retained subtree.
    ///
    /// Values are logical pixels. Nova captures the subtree once using the largest endpoint
    /// radius, then advances the actual filter radius without rerendering the owning view.
    pub fn filter_blur(from: Pixels, to: Pixels) -> Self {
        let sanitize = |value: Pixels| {
            let value = f32::from(value);
            if value.is_finite() {
                value.max(0.0)
            } else {
                0.0
            }
        };
        Self {
            property: TransitionProperty::FilterBlur,
            from: [sanitize(from), 0.0, 0.0, 0.0],
            to: [sanitize(to), 0.0, 0.0, 0.0],
            capture_subtree: false,
        }
    }

    /// Animate a visual translation without changing layout.
    pub fn translation(from: Point<Pixels>, to: Point<Pixels>) -> Self {
        Self {
            property: TransitionProperty::Translation,
            from: [from.x.0, from.y.0, 0.0, 0.0],
            to: [to.x.0, to.y.0, 0.0, 0.0],
            capture_subtree: false,
        }
    }

    /// Translate a clipped subtree as one retained image so its local text and image clips move
    /// with it while an ancestor scroll viewport remains fixed.
    pub fn clipped_translation(from: Point<Pixels>, to: Point<Pixels>) -> Self {
        Self {
            capture_subtree: true,
            ..Self::translation(from, to)
        }
    }

    /// Animate visual translation as fractions of the element's own final bounds.
    ///
    /// An X value of 1.0 means one element width and a Y value of 1.0 means one element height.
    /// Fractions resolve from stable final layout bounds before retained presentation begins, so
    /// callers do not need pixel dimensions and layout does not change. Values are not clamped.
    pub fn relative_translation(from: Point<f32>, to: Point<f32>) -> Self {
        Self {
            property: TransitionProperty::Translation,
            // Lane 3 is declaration-only metadata and is cleared before values reach Nova.
            from: [from.x, from.y, 0.0, 2.0],
            to: [to.x, to.y, 0.0, 2.0],
            capture_subtree: false,
        }
    }

    /// Animate translation and opacity through one renderer-owned slot.
    ///
    /// The fourth lane is a stable marker understood by Nova's Translation resolver; ordinary
    /// Translation values keep that lane at zero and retain their existing ABI and behavior.
    pub fn translation_opacity(
        from: Point<Pixels>,
        to: Point<Pixels>,
        from_opacity: f32,
        to_opacity: f32,
    ) -> Self {
        Self {
            property: TransitionProperty::Translation,
            from: [from.x.0, from.y.0, from_opacity.clamp(0.0, 1.0), 1.0],
            to: [to.x.0, to.y.0, to_opacity.clamp(0.0, 1.0), 1.0],
            capture_subtree: false,
        }
    }
    /// Move and fade a complex clipped subtree as one retained visual object.
    ///
    /// Unlike [`Self::translation_opacity`], this captures the subtree once and applies the packed
    /// translation/opacity sample to that composite. Text, SVGs, images, local clips and quads
    /// therefore share exactly one transform and cannot drift relative to each other while the
    /// animation is running.
    pub fn clipped_translation_opacity(
        from: Point<Pixels>,
        to: Point<Pixels>,
        from_opacity: f32,
        to_opacity: f32,
    ) -> Self {
        Self {
            capture_subtree: true,
            ..Self::translation_opacity(from, to, from_opacity, to_opacity)
        }
    }

    /// Animate a vertical reveal from one fixed edge without changing child layout.
    ///
    /// Fractions are relative to the element's final height. The renderer applies one shared clip
    /// to the retained subtree, so text baselines and child geometry stay fixed for the animation.
    pub fn vertical_reveal(
        edge: crate::VerticalRevealEdge,
        from_fraction: f32,
        to_fraction: f32,
    ) -> Self {
        let edge = match edge {
            crate::VerticalRevealEdge::Top => 0.0,
            crate::VerticalRevealEdge::Bottom => 1.0,
        };
        Self {
            property: TransitionProperty::ClipReveal,
            // [fraction, fixed-edge, axis, reserved], where axis 0 is vertical.
            from: [from_fraction.clamp(0.0, 1.0), edge, 0.0, 0.0],
            to: [to_fraction.clamp(0.0, 1.0), edge, 0.0, 0.0],
            capture_subtree: false,
        }
    }

    /// Animate a horizontal reveal from one fixed edge without changing child layout.
    ///
    /// Fractions are relative to the element's final width. The retained subtree is laid out and
    /// shaped once; subsequent animation samples only tighten its renderer-owned content mask.
    pub fn horizontal_reveal(
        edge: HorizontalRevealEdge,
        from_fraction: f32,
        to_fraction: f32,
    ) -> Self {
        let edge = match edge {
            HorizontalRevealEdge::Left => 0.0,
            HorizontalRevealEdge::Right => 1.0,
        };
        Self {
            property: TransitionProperty::ClipReveal,
            // [fraction, fixed-edge, axis, reserved], where axis 1 is horizontal.
            from: [from_fraction.clamp(0.0, 1.0), edge, 1.0, 0.0],
            to: [to_fraction.clamp(0.0, 1.0), edge, 1.0, 0.0],
            capture_subtree: false,
        }
    }

    /// Animate scale and opacity around one normalized transform origin.
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
            capture_subtree: false,
        }
    }

    fn resolved_values(
        self,
        bounds: Bounds<Pixels>,
        scale_factor: f32,
        visual_scale: f32,
    ) -> ([f32; 4], [f32; 4]) {
        match self.property {
            TransitionProperty::HorizontalEdgeFirst | TransitionProperty::HorizontalEdgeSecond => {
                let mut from = self.from;
                let mut to = self.to;
                from[0] *= scale_factor;
                to[0] *= scale_factor;
                (from, to)
            }
            TransitionProperty::Translation => {
                // Scene primitive bounds are already converted to device-scaled `ScaledPixels`.
                // Translation is declared in logical `Pixels`, so resolve it into the same device
                // coordinate space before the GPU adds it to those bounds.
                let (mut from, mut to) = self.translation_values_for_bounds(bounds);
                from[0] *= scale_factor;
                from[1] *= scale_factor;
                to[0] *= scale_factor;
                to[1] *= scale_factor;
                (from, to)
            }
            TransitionProperty::FilterBlur => {
                let scale = if scale_factor.is_finite() && visual_scale.is_finite() {
                    (scale_factor * visual_scale).abs()
                } else {
                    1.0
                };
                let mut from = self.from;
                let mut to = self.to;
                from[0] = from[0].max(0.0) * scale;
                to[0] = to[0].max(0.0) * scale;
                (from, to)
            }
            TransitionProperty::Transform => {
                let origin = TransformOrigin::new(self.from[2], self.from[3]).resolve(bounds);
                let mut from = self.from;
                let mut to = self.to;
                from[2] = origin.x.0 * scale_factor;
                from[3] = origin.y.0 * scale_factor;
                to[2] = from[2];
                to[3] = from[3];
                (from, to)
            }
            TransitionProperty::Rotation => {
                let center = bounds.center();
                let mut from = self.from;
                let mut to = self.to;
                from[1] = center.x.0 * scale_factor;
                from[2] = center.y.0 * scale_factor;
                to[1] = from[1];
                to[2] = from[2];
                (from, to)
            }
            TransitionProperty::ClipReveal => {
                let left = bounds.origin.x.0 * scale_factor;
                let right = bounds.right().0 * scale_factor;
                let top = bounds.origin.y.0 * scale_factor;
                let bottom = bounds.bottom().0 * scale_factor;
                let width = right - left;
                let height = bottom - top;
                let resolve = |value: [f32; 4]| {
                    let fraction = value[0].clamp(0.0, 1.0);
                    let fixed_end = value[1] >= 0.5;
                    let horizontal = value[2] >= 0.5;
                    if horizontal {
                        let visible_width = width * fraction;
                        if fixed_end {
                            [right - visible_width, right, top, bottom]
                        } else {
                            [left, left + visible_width, top, bottom]
                        }
                    } else {
                        let visible_height = height * fraction;
                        if fixed_end {
                            [left, right, bottom - visible_height, bottom]
                        } else {
                            [left, right, top, top + visible_height]
                        }
                    }
                };
                (resolve(self.from), resolve(self.to))
            }
            _ => (self.from, self.to),
        }
    }

    fn translation_values_for_bounds(self, bounds: Bounds<Pixels>) -> ([f32; 4], [f32; 4]) {
        if self.property != TransitionProperty::Translation
            || self.from[3] != 2.0
            || self.to[3] != 2.0
        {
            return (self.from, self.to);
        }

        let width = f32::from(bounds.size.width);
        let height = f32::from(bounds.size.height);
        let mut from = self.from;
        let mut to = self.to;
        from[0] *= width;
        from[1] *= height;
        to[0] *= width;
        to[1] *= height;
        from[3] = 0.0;
        to[3] = 0.0;
        (from, to)
    }

    fn text_raster_scale(self) -> f32 {
        crate::animation::scene_text_raster_scale(self.property, self.from, self.to)
    }

    fn dirty_bounds(self, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        self.dirty_bounds_with_visual_scale(bounds, 1.0)
    }

    fn dirty_bounds_with_visual_scale(
        self,
        bounds: Bounds<Pixels>,
        visual_scale: f32,
    ) -> Bounds<Pixels> {
        match self.property {
            TransitionProperty::Translation
            | TransitionProperty::HorizontalEdgeFirst
            | TransitionProperty::HorizontalEdgeSecond => {
                let (from, to) = self.translation_values_for_bounds(bounds);
                translated_bounds(bounds, from).union(&translated_bounds(bounds, to))
            }
            TransitionProperty::Rotation => rotation_bounds(bounds),
            TransitionProperty::FilterBlur => {
                let visual_scale = if visual_scale.is_finite() {
                    visual_scale.abs()
                } else {
                    1.0
                };
                let radius = self.from[0].abs().max(self.to[0].abs()) * visual_scale;
                if radius.is_finite() && radius > 0.0 {
                    bounds.dilate(crate::px(radius * 3.0 + 0.5))
                } else {
                    bounds
                }
            }
            _ => bounds,
        }
    }

    fn spring_translation_dirty_bounds(self, bounds: Bounds<Pixels>) -> Option<Bounds<Pixels>> {
        if !matches!(
            self.property,
            TransitionProperty::Translation
                | TransitionProperty::HorizontalEdgeFirst
                | TransitionProperty::HorizontalEdgeSecond
        ) {
            return None;
        }
        let (from, to) = self.translation_values_for_bounds(bounds);
        let first =
            translated_bounds_at_progress(bounds, from, to, SPRING_TRANSLATION_PROGRESS_MIN);
        let last = translated_bounds_at_progress(bounds, from, to, SPRING_TRANSLATION_PROGRESS_MAX);
        Some(first.union(&last))
    }
}

fn paint_scene_animation<R>(
    window: &mut Window,
    animation_id: SceneAnimationId,
    property: AnimationProperty,
    bounds: Bounds<Pixels>,
    from: [f32; 4],
    to: [f32; 4],
    paint: impl FnOnce(&mut Window) -> R,
) -> R {
    if property.property == TransitionProperty::FilterBlur {
        let max_radius_device = from[0].abs().max(to[0].abs());
        window.with_scene_blur_animation(
            animation_id,
            property.text_raster_scale(),
            bounds,
            max_radius_device,
            paint,
        )
    } else if property.capture_subtree
        || matches!(
            property.property,
            TransitionProperty::Transform
                | TransitionProperty::Rotation
                | TransitionProperty::ClipReveal
        )
    {
        // These properties capture a complete retained subtree. Use this animation element's own
        // stable layout box, not the inherited content mask (which may be the entire viewport).
        window.with_scene_composite_animation(
            animation_id,
            property.property,
            property.text_raster_scale(),
            bounds,
            paint,
        )
    } else {
        window.with_scene_animation(
            animation_id,
            property.property,
            property.text_raster_scale(),
            paint,
        )
    }
}

impl Animation {
    /// Create a new animation from the given duration.
    /// By default the animation will only run once and will use a linear easing function.
    pub fn new(duration: Duration) -> Self {
        Self::from_spec(AnimationSpec::new(duration))
    }

    /// Animate a spring in physical seconds until both position and velocity settle.
    /// Unlike a duration-based easing curve, this does not truncate the spring's tail.
    pub fn spring(spring: crate::Spring) -> Self {
        let mut animation =
            Self::from_spec(AnimationSpec::new(Duration::ZERO).driver(AnimationDriver::Paint));
        animation.spring = Some(spring);
        animation
    }

    /// Create an element animation from an engine timing specification.
    pub fn from_spec(spec: AnimationSpec) -> Self {
        Self {
            spec,
            property: None,
            spring: None,
        }
    }

    /// Delay the start of this animation.
    ///
    /// This applies to both duration-based animations and physical springs. Use a backwards fill
    /// mode when the animated property must remain at its declared start value during the delay.
    pub fn delay(mut self, delay: Duration) -> Self {
        self.spec.delay = delay;
        self
    }

    /// Set how the animation behaves before its delay and after completion.
    pub fn fill_mode(mut self, fill_mode: crate::FillMode) -> Self {
        self.spec.fill_mode = fill_mode;
        self
    }

    /// Set the animation to loop when it finishes.
    pub fn repeat(mut self) -> Self {
        self.spec.repeat = RepeatMode::Forever;
        self
    }

    /// Set the easing function to use for this animation.
    /// The easing function will take a time delta between 0 and 1 and return a new delta
    /// that may overshoot the 0 to 1 range.
    /// The closure must be thread-safe so retained visual timelines can be sampled by an
    /// independent presentation owner.
    pub fn with_easing(mut self, easing: impl Fn(f32) -> f32 + Send + Sync + 'static) -> Self {
        self.spring = None;
        self.spec.easing = crate::Easing::Custom(Arc::new(easing));
        self
    }

    /// Declare a visual property that GPUI can animate without relayout.
    /// The animator callback is evaluated only for the initial scene, so it must
    /// not animate additional properties. Leave such combined animations on the
    /// layout path, or declare only the property owned by the renderer. Custom
    /// easing closures are sampled by the CPU animation engine with the paint
    /// driver while the retained scene still owns the visual property; only an
    /// explicit layout driver forces a declared visual property back through
    /// layout. Opacity, scale and translation bind directly to supported
    /// primitives; wrap a mixed subtree in
    /// [`crate::CompositeLayerExt::composite_layer`] when it can contain paths,
    /// underlines or platform surfaces.
    pub fn with_property(mut self, property: AnimationProperty) -> Self {
        self.property = Some(property);
        self
    }

    /// Animate opacity as a renderer-owned property without changing layout.
    pub fn with_opacity(self, from: f32, to: f32) -> Self {
        self.with_property(AnimationProperty::opacity(from, to))
    }

    /// Animate translation as a renderer-owned property without changing layout.
    pub fn with_translation(self, from: Point<Pixels>, to: Point<Pixels>) -> Self {
        self.with_property(AnimationProperty::translation(from, to))
    }

    /// Animate scale around each primitive's static center without changing layout.
    pub fn with_scale(self, from: f32, to: f32) -> Self {
        self.with_property(AnimationProperty::uniform_scale(from, to))
    }

    fn scene_animation(&self) -> Option<(AnimationProperty, &AnimationSpec)> {
        let property = self.property?;
        (!matches!(self.spec.driver, AnimationDriver::Layout)).then_some((property, &self.spec))
    }
}

/// An extension trait for adding the animation wrapper to both Elements and Components
pub trait AnimationExt {
    /// Render this component or element with an animation
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
            animator: Some(Box::new(move |this, _, value| animator(this, value))),
            animations: smallvec::smallvec![animation],
            parallel_group: false,
        }
    }

    /// Render this component or element with a chain of animations
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
            animator: Some(Box::new(animator)),
            animations: animations.into(),
            parallel_group: false,
        }
    }

    /// Animate a declared visual property on the presentation lane without an animator callback.
    ///
    /// The element's layout and paint are committed once; subsequent visual samples are applied
    /// to the retained scene. Use [`AnimationExt::with_animation`] when each sample must change
    /// layout or content.
    ///
    /// # Errors
    ///
    /// Returns [`VisualAnimationError::MissingProperty`] when `animation` does not declare an
    /// [`AnimationProperty`], or [`VisualAnimationError::LayoutDrivenProperty`] when its driver is
    /// `Layout`.
    fn with_visual_animation(
        self,
        id: impl Into<ElementId>,
        animation: Animation,
    ) -> Result<AnimationElement<Self>, VisualAnimationError>
    where
        Self: Sized,
    {
        let Some(property) = animation.property else {
            return Err(VisualAnimationError::MissingProperty);
        };
        if matches!(animation.spec.driver, AnimationDriver::Layout) {
            return Err(VisualAnimationError::LayoutDrivenProperty(
                property.property,
            ));
        }

        Ok(AnimationElement {
            id: id.into(),
            element: Some(self),
            animator: None,
            animations: smallvec::smallvec![animation],
            parallel_group: false,
        })
    }

    /// Animate up to three visual properties in parallel without rebuilding or relaying out the
    /// retained element on each sample. Every track keeps its own easing and spring.
    fn with_animation_group(
        self,
        id: impl Into<ElementId>,
        group: AnimationGroup,
    ) -> AnimationElement<Self>
    where
        Self: Sized,
    {
        AnimationElement {
            id: id.into(),
            element: Some(self),
            animator: None,
            animations: group.tracks,
            parallel_group: true,
        }
    }

    /// Paint this element into a retained scene animation using a caller-sampled progress value.
    ///
    /// This legacy form allocates a frame-local animation id and therefore requires descendants to
    /// repaint whenever the sample changes. Use [`AnimationExt::with_stable_sampled_animation`] for
    /// recurring caller-sampled motion whose static descendants should remain replayable.
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

    /// Paint caller-sampled motion with one persistent scene-animation identity.
    ///
    /// `id` is retained with the element state. When `animating` is true this wrapper schedules the
    /// next compositor-paced sample by invalidating only its own paint context; descendants keep
    /// their previous primitive ranges and continue referring to the same animation id. Use this
    /// only when layout is already at final geometry and the sampled property is visual-only.
    fn with_stable_sampled_animation(
        self,
        id: impl Into<ElementId>,
        property: AnimationProperty,
        progress: f32,
        animating: bool,
    ) -> StableSampledAnimationElement<Self>
    where
        Self: Sized,
    {
        StableSampledAnimationElement {
            id: id.into(),
            element: Some(self),
            property,
            progress,
            animating,
        }
    }

    /// Attach an automatically identified retained invalidation target to a caller-sampled layout animation.
    ///
    /// The owning view is still rerendered on every sample so callers may recompute arbitrary
    /// layout/style values. Only this retained subtree is marked dirty, allowing unrelated siblings
    /// to replay their previous prepaint/paint ranges instead of repainting with the entire view.
    /// The target identity comes from the parent mount path, this wrapper's call site, and its type;
    /// application code does not need to invent a rendering-only element ID.
    #[track_caller]
    fn with_layout_animation_target(self, animating: bool) -> LayoutAnimationTargetElement<Self>
    where
        Self: Sized,
    {
        LayoutAnimationTargetElement {
            source: core::panic::Location::caller(),
            element: Some(self),
            animating,
        }
    }
}

impl<E: IntoElement + 'static> AnimationExt for E {}

/// A retained boundary for manually sampled layout animations.
pub struct LayoutAnimationTargetElement<E> {
    source: &'static core::panic::Location<'static>,
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

/// An element whose static primitives are retained while the caller supplies motion progress.
pub struct SampledAnimationElement<E> {
    element: Option<E>,
    property: AnimationProperty,
    progress: f32,
}

/// Opaque prepaint state for [`SampledAnimationElement`].
#[doc(hidden)]
pub struct SampledAnimationPrepaintState {
    animation_id: SceneAnimationId,
}

impl<E: IntoElement + 'static> IntoElement for SampledAnimationElement<E> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<E: IntoElement + 'static> Element for SampledAnimationElement<E> {
    type RequestLayoutState = AnyElement;
    type PrepaintState = SampledAnimationPrepaintState;
    // The animation id above is scene-local and is allocated during prepaint. If an ancestor
    // replays this wrapper, the current frame never records a matching SceneAnimationValue and
    // retained primitives keep a stale frame-local binding. Always execute this boundary so its
    // descendants are rebound to the current scene id before paint.
    const RETAINED_REPLAY_CAPABILITY: crate::RetainedReplayCapability =
        crate::RetainedReplayCapability::OwnsFrameLocalCacheBoundary;

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
    ) -> Self::PrepaintState {
        let animation_id = window.next_frame.scene.allocate_animation_id();
        window.with_retained_replay_barrier(true, |window| element.prepaint(window, cx));
        SampledAnimationPrepaintState { animation_id }
    }

    fn paint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        state: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (from, to) =
            self.property
                .resolved_values(bounds, window.scale_factor(), window.visual_scale());
        window
            .next_frame
            .scene
            .push_animation_value(crate::SceneAnimationValue {
                animation_id: state.animation_id,
                property: self.property.property,
                progress: if self.progress.is_finite() {
                    self.progress
                } else {
                    0.0
                },
                from,
                to,
            });
        paint_scene_animation(
            window,
            state.animation_id,
            self.property,
            bounds,
            from,
            to,
            |window| element.paint(window, cx),
        );
    }
}

#[derive(Clone)]
#[doc(hidden)]
pub struct StableSampledAnimationState {
    animation_id: SceneAnimationId,
    property: AnimationProperty,
    frame_pending: Rc<Cell<bool>>,
    bound: bool,
}

/// A caller-sampled scene animation whose primitive ownership survives retained replay.
pub struct StableSampledAnimationElement<E> {
    id: ElementId,
    element: Option<E>,
    property: AnimationProperty,
    progress: f32,
    animating: bool,
}

impl<E: IntoElement + 'static> IntoElement for StableSampledAnimationElement<E> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<E: IntoElement + 'static> Element for StableSampledAnimationElement<E> {
    type RequestLayoutState = AnyElement;
    type PrepaintState = Option<StableSampledAnimationState>;

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
        let mut element = self
            .element
            .take()
            .expect("stable sampled animation element should only be laid out once")
            .into_any_element();
        (element.request_layout(window, cx), element)
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let global_id = global_id.expect(
            "StableSampledAnimationElement always supplies an element id for state tracking",
        );
        let (state, binding_changed) = window.with_element_state(
            global_id,
            |state: Option<StableSampledAnimationState>, _window| {
                let (mut state, property_changed) = match state {
                    Some(state) if state.property == self.property => (state, false),
                    Some(state) => (
                        StableSampledAnimationState {
                            animation_id: allocate_stable_sampled_animation_id(),
                            property: self.property,
                            frame_pending: state.frame_pending,
                            bound: false,
                        },
                        true,
                    ),
                    None => (
                        StableSampledAnimationState {
                            animation_id: allocate_stable_sampled_animation_id(),
                            property: self.property,
                            frame_pending: Rc::new(Cell::new(false)),
                            bound: false,
                        },
                        true,
                    ),
                };
                let binding_changed = property_changed || state.bound != self.animating;
                state.bound = self.animating;
                ((state.clone(), binding_changed), state)
            },
        );

        // Entering or leaving renderer ownership must repaint descendants once. Without this
        // boundary, retained rows may keep a stale SceneAnimationId after the transition ends.
        window.with_retained_replay_barrier(binding_changed, |window| element.prepaint(window, cx));

        self.animating.then_some(state)
    }

    fn paint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        element: &mut Self::RequestLayoutState,
        state: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(state) = state.as_ref() else {
            element.paint(window, cx);
            return;
        };

        let (from, to) =
            self.property
                .resolved_values(bounds, window.scale_factor(), window.visual_scale());
        window
            .next_frame
            .scene
            .push_animation_value(crate::SceneAnimationValue {
                animation_id: state.animation_id,
                property: self.property.property,
                progress: if self.progress.is_finite() {
                    self.progress
                } else {
                    0.0
                },
                from,
                to,
            });
        paint_scene_animation(
            window,
            state.animation_id,
            self.property,
            bounds,
            from,
            to,
            |window| element.paint(window, cx),
        );

        if !state.frame_pending.replace(true) {
            let frame_pending = state.frame_pending.clone();
            let view_id = window.current_view();
            let retained_id = window
                .current_retained_element_id()
                .expect("stable sampled animation must have a retained identity");
            let dirty_bounds = if self.property.property == TransitionProperty::FilterBlur {
                self.property.dirty_bounds_with_visual_scale(
                    window.visual_bounds(bounds),
                    window.visual_scale(),
                )
            } else {
                self.property.dirty_bounds(bounds)
            };
            window.on_next_presentation_frame(move |window, cx| {
                frame_pending.set(false);
                window.notify_interactive_region_scoped_for_current_frame(
                    view_id,
                    Some(&retained_id),
                    dirty_bounds,
                    false,
                    cx,
                );
            });
        }
    }
}

/// A GPUI element that applies an animation to another element
pub struct AnimationElement<E> {
    id: ElementId,
    element: Option<E>,
    animations: SmallVec<[Animation; 3]>,
    animator: Option<Box<dyn Fn(E, usize, f32) -> E + 'static>>,
    parallel_group: bool,
}

impl<E> AnimationElement<E> {
    /// Returns a new [`AnimationElement<E>`] after applying the given function
    /// to the element being animated.
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

struct AnimationState(ElementAnimationTimeline);

#[derive(Clone, Debug, PartialEq)]
#[doc(hidden)]
pub struct SceneAnimationState {
    animation_id: SceneAnimationId,
    property: AnimationProperty,
    spec: AnimationSpec,
    spring: Option<crate::Spring>,
    from: [f32; 4],
    to: [f32; 4],
    bounds: Bounds<Pixels>,
    bound: bool,
}

impl<E: IntoElement + 'static> Element for AnimationElement<E> {
    type RequestLayoutState = AnyElement;
    type PrepaintState = Option<SmallVec<[SceneAnimationState; 3]>>;

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
            let mut element = match progress {
                Some(progress) => match self.animator.as_ref() {
                    Some(animator) => {
                        animator(element, animation_index, progress).into_any_element()
                    }
                    None => element.into_any_element(),
                },
                None => element.into_any_element(),
            };
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
                state.unwrap_or_else(|| AnimationState(ElementAnimationTimeline::new(now)));
            let (animation_ix, delta, done) =
                sample_element_animation(&mut state.0, &self.animations, now);

            debug_assert!(
                delta.is_none_or(f32::is_finite),
                "eased progress must be finite"
            );

            let element = self.element.take().expect("should only be called once");
            let mut element = match delta {
                Some(delta) => match self.animator.as_ref() {
                    Some(animator) => animator(element, animation_ix, delta).into_any_element(),
                    None => element.into_any_element(),
                },
                None => element.into_any_element(),
            };

            let repeats = self
                .animations
                .get(animation_ix)
                .is_some_and(|animation| matches!(animation.spec.repeat, RepeatMode::Forever));
            schedule_next_animation_frame(window, cx, now, done, repeats, &retained_id);

            ((element.request_layout(window, cx), element), state)
        })
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: crate::Bounds<crate::Pixels>,
        element: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let Some(animations) = self.scene_animations() else {
            element.prepaint(window, cx);
            return None;
        };
        let global_id =
            global_id.expect("AnimationElement always supplies an element id for state tracking");
        let mut prepared = SmallVec::<
            [(
                AnimationProperty,
                AnimationSpec,
                Option<crate::Spring>,
                [f32; 4],
                [f32; 4],
                Bounds<Pixels>,
            ); 3],
        >::new();
        for animation in animations {
            let Some((property, spec)) = animation.scene_animation() else {
                element.prepaint(window, cx);
                return None;
            };
            let (from, to) =
                property.resolved_values(bounds, window.scale_factor(), window.visual_scale());
            // Custom curves can overshoot. Translation uses the conservative viewport until paint
            // has registered the exact retained subtree bounds.
            let dirty_bounds = if property.property == TransitionProperty::Translation {
                Bounds::new(Point::default(), window.viewport_size())
            } else if property.property == TransitionProperty::FilterBlur {
                property.dirty_bounds_with_visual_scale(
                    window.visual_bounds(bounds),
                    window.visual_scale(),
                )
            } else {
                property.dirty_bounds(bounds)
            };
            prepared.push((
                property,
                spec.clone(),
                animation.spring,
                from,
                to,
                dirty_bounds,
            ));
        }

        let parallel_group = self.parallel_group;
        let (states, binding_changed, bound) = window.with_element_state(
            global_id,
            |state: Option<SmallVec<[SceneAnimationState; 3]>>, window| {
                let previous = state.unwrap_or_default();
                let mut animation_id = previous.first().map(|state| state.animation_id);
                let mut states = SmallVec::<[SceneAnimationState; 3]>::new();
                let mut binding_changed = false;
                let mut bound = false;

                for old_state in previous.iter().filter(|old_state| {
                    !prepared
                        .iter()
                        .any(|track| track.0.property == old_state.property.property)
                }) {
                    window.cancel_scene_animation_track(
                        global_id,
                        old_state.property.property,
                        old_state.animation_id,
                    );
                    if !parallel_group {
                        binding_changed |= old_state.bound;
                    }
                }

                for (property, spec, spring, from, to, dirty_bounds) in &prepared {
                    let previous_state = previous
                        .iter()
                        .find(|state| state.property.property == property.property);
                    let next_state = match previous_state {
                        Some(state)
                            if state.property == *property
                                && state.spec == *spec
                                && state.spring == *spring
                                && (!matches!(
                                    property.property,
                                    TransitionProperty::HorizontalEdgeFirst
                                        | TransitionProperty::HorizontalEdgeSecond
                                ) || state.bounds.origin == bounds.origin)
                                && ((state.from == *from && state.to == *to)
                                    || (state.bounds.size == bounds.size
                                        && state.property.resolved_values(
                                            state.bounds,
                                            window.scale_factor(),
                                            window.visual_scale(),
                                        ) == (state.from, state.to)
                                        && matches!(
                                            property.property,
                                            TransitionProperty::Transform
                                                | TransitionProperty::Rotation
                                                | TransitionProperty::ClipReveal
                                        ))) =>
                        {
                            if state.from != *from || state.to != *to {
                                let delta = bounds.origin - state.bounds.origin;
                                let scale = window.scale_factor();
                                window.translate_scene_animation_origin(
                                    global_id,
                                    property.property,
                                    [delta.x.0 * scale, delta.y.0 * scale],
                                    *dirty_bounds,
                                );
                            }
                            let track_bound = window.scene_animation_track_is_bound(
                                state.animation_id,
                                property.property,
                            );
                            let mut state = state.clone();
                            state.from = *from;
                            state.to = *to;
                            state.bounds = bounds;
                            state.bound = track_bound;
                            if !parallel_group {
                                binding_changed |= previous_state
                                    .is_some_and(|previous| previous.bound != track_bound);
                            }
                            bound |= track_bound;
                            state
                        }
                        Some(state)
                            if window.scene_animation_track_is_active(
                                state.animation_id,
                                property.property,
                            ) && window.retarget_scene_animation(
                                global_id,
                                property.property,
                                state.animation_id,
                                spec.clone(),
                                *spring,
                                state.bounds,
                                bounds,
                                *dirty_bounds,
                                *to,
                            ) =>
                        {
                            let mut state = state.clone();
                            state.spec = spec.clone();
                            state.spring = *spring;
                            state.from = *from;
                            state.to = *to;
                            state.bounds = bounds;
                            state.bound = true;
                            bound = true;
                            binding_changed = true;
                            state
                        }
                        _ => {
                            let track_id = if parallel_group {
                                window.start_grouped_scene_animation(
                                    global_id,
                                    property.property,
                                    spec.clone(),
                                    *dirty_bounds,
                                    *from,
                                    *to,
                                    animation_id,
                                )
                            } else if let Some(animation_id) = animation_id {
                                window.start_scene_animation_with_id(
                                    global_id,
                                    property.property,
                                    spec.clone(),
                                    *dirty_bounds,
                                    *from,
                                    *to,
                                    animation_id,
                                    false,
                                );
                                animation_id
                            } else {
                                window.start_scene_animation(
                                    global_id,
                                    property.property,
                                    spec.clone(),
                                    *dirty_bounds,
                                    *from,
                                    *to,
                                )
                            };
                            if let Some(spring) = spring {
                                window.set_scene_animation_spring(
                                    global_id,
                                    property.property,
                                    *spring,
                                );
                            }
                            animation_id = Some(track_id);
                            bound = true;
                            binding_changed = true;
                            SceneAnimationState {
                                animation_id: track_id,
                                property: *property,
                                spec: spec.clone(),
                                spring: *spring,
                                from: *from,
                                to: *to,
                                bounds,
                                bound: true,
                            }
                        }
                    };
                    window.set_grouped_visual_scene_animation(
                        global_id,
                        property.property,
                        next_state.animation_id,
                        parallel_group,
                    );
                    states.push(next_state);
                }

                if parallel_group {
                    let group_bound = animation_id
                        .is_some_and(|animation_id| window.scene_animation_is_bound(animation_id));
                    binding_changed |= previous
                        .first()
                        .is_some_and(|previous| previous.bound != group_bound);
                    for state in &mut states {
                        state.bound = group_bound;
                    }
                    bound = group_bound;
                }

                ((states.clone(), binding_changed, bound), states)
            },
        );

        // The renderer owns this subtree only while the timeline is active. Completion schedules
        // one targeted repaint, and this barrier guarantees descendants drop the old animation id
        // instead of replaying it forever into later hover/scroll frames.
        window.with_retained_replay_barrier(binding_changed, |window| element.prepaint(window, cx));

        bound.then_some(states)
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: crate::Bounds<crate::Pixels>,
        element: &mut Self::RequestLayoutState,
        state: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(states) = state.as_ref() else {
            element.paint(window, cx);
            return;
        };
        let global_id =
            global_id.expect("AnimationElement always supplies an element id for state tracking");
        // The scope must retain geometry semantics even when opacity is the group's first track.
        // In particular, glyph rasterization needs to know that the shared binding moves/scales.
        let representative = states
            .iter()
            .find(|state| {
                self.parallel_group
                    && matches!(
                        state.property.property,
                        TransitionProperty::Scale
                            | TransitionProperty::Transform
                            | TransitionProperty::Translation
                            | TransitionProperty::Rotation
                            | TransitionProperty::ClipReveal
                    )
            })
            .or_else(|| states.first())
            .expect("active scene group has a track");
        let paint = |window: &mut Window| {
            element.paint(window, cx);
            for state in states {
                if state.spring.is_some()
                    && matches!(
                        state.property.property,
                        TransitionProperty::Translation
                            | TransitionProperty::HorizontalEdgeFirst
                            | TransitionProperty::HorizontalEdgeSecond
                    )
                    && let Some(visual_bounds) =
                        window.scene_animation_visual_bounds(state.animation_id)
                    && let Some(dirty_bounds) = state
                        .property
                        .spring_translation_dirty_bounds(visual_bounds)
                {
                    window.set_scene_animation_dirty_bounds(
                        global_id,
                        state.property.property,
                        dirty_bounds,
                    );
                }
            }
        };
        if self.parallel_group {
            let text_raster_scale = states
                .iter()
                .map(|state| state.property.text_raster_scale())
                .fold(1.0_f32, f32::max);
            window.with_scene_animation(
                representative.animation_id,
                representative.property.property,
                text_raster_scale,
                paint,
            );
        } else {
            paint_scene_animation(
                window,
                representative.animation_id,
                representative.property,
                bounds,
                representative.from,
                representative.to,
                paint,
            );
        }
    }
}

impl<E> AnimationElement<E> {
    fn scene_animation(&self) -> Option<(AnimationProperty, &AnimationSpec)> {
        (self.animations.len() == 1)
            .then(|| self.animations.first()?.scene_animation())
            .flatten()
    }

    fn scene_animations(&self) -> Option<&[Animation]> {
        if self.parallel_group {
            Some(&self.animations)
        } else {
            self.scene_animation().map(|_| self.animations.as_slice())
        }
    }

    fn initial_scene_animation_sample(&self) -> Option<(usize, Option<f32>)> {
        if self.parallel_group {
            return Some((0, None));
        }
        let (_, spec) = self.scene_animation()?;
        let sample = spec.sample_elapsed(Duration::ZERO);
        Some((
            0,
            if self.animations[0].spring.is_some() {
                Some(0.0)
            } else {
                sample.applies.then_some(sample.eased_progress)
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
    } else {
        // A platform frame is already paced by the compositor. Repeating animations must join
        // that same latest-wins frame instead of spawning an independent 3 ms timer per target;
        // those timers can wake faster than the display and create foreground executor pressure.
        Some(Duration::ZERO)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_free_visual_animation_requires_a_presentation_property() {
        let element = crate::div()
            .with_visual_animation(
                "fade",
                Animation::new(Duration::from_millis(180)).with_opacity(0.0, 1.0),
            )
            .expect("opacity is a presentation property");
        assert!(element.animator.is_none());
        assert!(element.scene_animation().is_some());

        let missing_property = crate::div().with_visual_animation(
            "missing-property",
            Animation::new(Duration::from_millis(180)),
        );
        assert!(matches!(
            missing_property,
            Err(VisualAnimationError::MissingProperty)
        ));

        let layout_driven = crate::div().with_visual_animation(
            "layout-driven",
            Animation::from_spec(
                AnimationSpec::new(Duration::from_millis(180)).driver(AnimationDriver::Layout),
            )
            .with_opacity(0.0, 1.0),
        );
        assert!(matches!(
            layout_driven,
            Err(VisualAnimationError::LayoutDrivenProperty(
                TransitionProperty::Opacity
            ))
        ));
    }

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
    fn visual_animation_group_keeps_independent_timing_and_rejects_capture() {
        let group = AnimationGroup::parallel([
            Animation::new(Duration::from_millis(180)).with_opacity(0.0, 1.0),
            Animation::spring(crate::Spring::default()).with_translation(
                Point::default(),
                Point::new(crate::px(24.0), crate::px(0.0)),
            ),
            Animation::new(Duration::from_millis(320)).with_scale(0.9, 1.0),
        ])
        .expect("the group uses three distinct compositor properties");

        assert_eq!(group.tracks.len(), 3);
        assert_eq!(
            group.tracks[0].property.expect("opacity track").property,
            TransitionProperty::Opacity
        );
        assert_eq!(
            group.tracks[1]
                .property
                .expect("translation track")
                .property,
            TransitionProperty::Translation
        );
        assert_eq!(
            group.tracks[2].property.expect("scale track").property,
            TransitionProperty::Scale
        );
        assert!(group.tracks[1].spring.is_some());

        let captured = Animation::new(Duration::from_millis(180)).with_property(
            AnimationProperty::clipped_translation(
                Point::default(),
                Point::new(crate::px(4.0), crate::px(0.0)),
            ),
        );
        assert!(matches!(
            AnimationGroup::parallel([captured]),
            Err(AnimationGroupError::CapturedSubtreeProperty(
                TransitionProperty::Translation
            ))
        ));
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
    fn declared_blur_uses_renderer_gpu_driver_and_device_radius() {
        let property = AnimationProperty::filter_blur(crate::px(2.0), crate::px(12.0));
        let animation = Animation::new(Duration::from_millis(240)).with_property(property);
        let (declared, spec) = animation.scene_animation().expect("scene animation");
        let bounds = Bounds::new(
            Point::new(crate::px(0.0), crate::px(0.0)),
            crate::size(crate::px(100.0), crate::px(60.0)),
        );

        assert_eq!(declared.property, TransitionProperty::FilterBlur);
        assert_eq!(
            declared.resolved_values(bounds, 2.0, 1.5),
            ([6.0, 0.0, 0.0, 0.0], [36.0, 0.0, 0.0, 0.0])
        );
        assert!(TransitionProperty::FilterBlur.supports_gpu_driver());
        assert_eq!(
            TransitionProperty::FilterBlur.preferred_driver(),
            AnimationDriver::Gpu
        );
        assert_eq!(spec.driver, AnimationDriver::Auto);
    }

    #[test]
    fn resolved_translation_uses_device_pixel_distance() {
        let property = AnimationProperty::translation(
            Point::new(crate::px(10.0), crate::px(5.0)),
            Point::new(crate::px(40.0), crate::px(15.0)),
        );
        let bounds = Bounds::new(
            Point::new(crate::px(10.0), crate::px(20.0)),
            crate::size(crate::px(30.0), crate::px(40.0)),
        );

        assert_eq!(
            property.resolved_values(bounds, 2.0, 1.0),
            ([20.0, 10.0, 0.0, 0.0], [80.0, 30.0, 0.0, 0.0])
        );
    }

    #[test]
    fn relative_translation_resolves_against_final_bounds() {
        let property =
            AnimationProperty::relative_translation(Point::new(-0.75, 0.25), Point::new(0.0, 0.0));
        let bounds = Bounds::new(
            Point::new(crate::px(10.0), crate::px(20.0)),
            crate::size(crate::px(200.0), crate::px(40.0)),
        );

        assert_eq!(
            property.resolved_values(bounds, 2.0, 1.0),
            ([-300.0, 20.0, 0.0, 0.0], [0.0, 0.0, 0.0, 0.0])
        );
        assert_eq!(
            property.dirty_bounds(bounds),
            Bounds::new(
                Point::new(crate::px(-140.0), crate::px(20.0)),
                crate::size(crate::px(350.0), crate::px(50.0)),
            )
        );
    }

    #[test]
    fn resolved_translation_opacity_preserves_alpha_lanes() {
        let property = AnimationProperty::translation_opacity(
            Point::new(crate::px(10.0), crate::px(5.0)),
            Point::new(crate::px(0.0), crate::px(0.0)),
            0.88,
            1.0,
        );
        let bounds = Bounds::new(
            Point::new(crate::px(10.0), crate::px(20.0)),
            crate::size(crate::px(30.0), crate::px(40.0)),
        );

        assert_eq!(
            property.resolved_values(bounds, 2.0, 1.0),
            ([20.0, 10.0, 0.88, 1.0], [0.0, 0.0, 1.0, 1.0])
        );
        assert!(!property.capture_subtree);
    }

    #[test]
    fn clipped_translation_opacity_keeps_one_subtree_transform() {
        let property = AnimationProperty::clipped_translation_opacity(
            Point::new(crate::px(-18.0), crate::px(0.0)),
            Point::default(),
            0.94,
            1.0,
        );
        let bounds = Bounds::new(
            Point::new(crate::px(10.0), crate::px(20.0)),
            crate::size(crate::px(300.0), crate::px(180.0)),
        );

        assert_eq!(property.property, TransitionProperty::Translation);
        assert!(property.capture_subtree);
        assert_eq!(
            property.resolved_values(bounds, 2.0, 1.0),
            ([-36.0, 0.0, 0.94, 1.0], [0.0, 0.0, 1.0, 1.0])
        );
    }

    #[test]
    fn resolved_rotation_uses_shared_element_center() {
        let property = AnimationProperty::rotation(crate::radians(0.0), crate::radians(1.0));
        let bounds = Bounds::new(
            Point::new(crate::px(10.0), crate::px(20.0)),
            crate::size(crate::px(30.0), crate::px(40.0)),
        );
        let (from, to) = property.resolved_values(bounds, 2.0, 1.0);

        assert_eq!(from, [0.0, 50.0, 80.0, 0.0]);
        assert_eq!(to, [1.0, 50.0, 80.0, 0.0]);
    }

    #[test]
    fn resolved_vertical_reveal_uses_shared_device_pixel_edges() {
        let bounds = Bounds::new(
            Point::new(crate::px(10.0), crate::px(20.0)),
            crate::size(crate::px(30.0), crate::px(40.0)),
        );

        let top = AnimationProperty::vertical_reveal(crate::VerticalRevealEdge::Top, 0.0, 1.0);
        assert_eq!(
            top.resolved_values(bounds, 2.0, 1.0),
            ([20.0, 80.0, 40.0, 40.0], [20.0, 80.0, 40.0, 120.0])
        );

        let bottom =
            AnimationProperty::vertical_reveal(crate::VerticalRevealEdge::Bottom, 0.25, 1.0);
        assert_eq!(
            bottom.resolved_values(bounds, 2.0, 1.0),
            ([20.0, 80.0, 100.0, 120.0], [20.0, 80.0, 40.0, 120.0])
        );
    }

    #[test]
    fn resolved_horizontal_reveal_uses_shared_device_pixel_edges() {
        let bounds = Bounds::new(
            Point::new(crate::px(10.0), crate::px(20.0)),
            crate::size(crate::px(30.0), crate::px(40.0)),
        );

        let left = AnimationProperty::horizontal_reveal(HorizontalRevealEdge::Left, 0.25, 1.0);
        assert_eq!(
            left.resolved_values(bounds, 2.0, 1.0),
            ([20.0, 35.0, 40.0, 120.0], [20.0, 80.0, 40.0, 120.0])
        );

        let right = AnimationProperty::horizontal_reveal(HorizontalRevealEdge::Right, 0.0, 0.5);
        assert_eq!(
            right.resolved_values(bounds, 2.0, 1.0),
            ([80.0, 80.0, 40.0, 120.0], [50.0, 80.0, 40.0, 120.0])
        );
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
    fn blur_dirty_bounds_include_outer_visual_scale() {
        let property = AnimationProperty::filter_blur(crate::px(2.0), crate::px(10.0));
        let visual_bounds = Bounds::new(
            Point::new(crate::px(20.0), crate::px(30.0)),
            crate::size(crate::px(150.0), crate::px(90.0)),
        );

        assert_eq!(
            property.dirty_bounds_with_visual_scale(visual_bounds, 1.5),
            visual_bounds.dilate(crate::px(45.5))
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
    fn stable_sampled_animation_ids_use_reserved_non_engine_range() {
        let first = allocate_stable_sampled_animation_id();
        let second = allocate_stable_sampled_animation_id();
        assert!(first.0 >= STABLE_SAMPLED_ANIMATION_ID_START);
        assert!(first.0 < ENGINE_ANIMATION_ID_START);
        assert!(second.0 > first.0);
        assert!(second.0 < ENGINE_ANIMATION_ID_START);
    }

    #[test]
    fn sampled_animation_owns_frame_local_retained_boundary() {
        assert_eq!(
            <SampledAnimationElement<crate::Div> as Element>::RETAINED_REPLAY_CAPABILITY,
            crate::RetainedReplayCapability::OwnsFrameLocalCacheBoundary
        );
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
    fn custom_easing_with_visual_property_uses_scene_animation_path() {
        let animation = Animation::new(Duration::from_millis(100))
            .with_easing(|progress| progress * progress)
            .with_property(AnimationProperty::opacity(0.0, 1.0));

        let (property, spec) = animation.scene_animation().expect("scene animation");
        assert_eq!(property.property, TransitionProperty::Opacity);
        assert!(matches!(spec.easing, crate::Easing::Custom(_)));
    }

    #[test]
    fn repeating_animation_uses_gpui_frame_cadence() {
        assert_eq!(
            next_animation_frame_delay(false, true, true),
            Some(Duration::ZERO)
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

    /// The linear easing function, or delta itself
    pub fn linear(delta: f32) -> f32 {
        delta
    }

    /// The quadratic easing function, delta * delta
    pub fn quadratic(delta: f32) -> f32 {
        delta * delta
    }

    /// The quadratic ease-in-out function, which starts and ends slowly but speeds up in the middle
    pub fn ease_in_out(delta: f32) -> f32 {
        if delta < 0.5 {
            2.0 * delta * delta
        } else {
            let x = -2.0 * delta + 2.0;
            1.0 - x * x / 2.0
        }
    }

    /// The Quint ease-out function, which starts quickly and decelerates to a stop
    pub fn ease_out_quint() -> impl Fn(f32) -> f32 {
        move |delta| 1.0 - (1.0 - delta).powi(5)
    }

    /// Apply the given easing function first in the forward direction and then in reverse.
    pub fn bounce(easing: impl Fn(f32) -> f32) -> impl Fn(f32) -> f32 {
        move |delta| {
            if delta < 0.5 {
                easing(delta * 2.0)
            } else {
                easing((1.0 - delta) * 2.0)
            }
        }
    }

    /// A custom easing function for pulsating alpha that slows down as it approaches 0.1
    pub fn pulsating_between(min: f32, max: f32) -> impl Fn(f32) -> f32 {
        let range = max - min;

        move |delta| {
            // Use a combination of sine and cubic functions for a more natural breathing rhythm
            let t = (delta * 2.0 * PI).sin();
            let breath = (t * t * t + t) / 2.0;

            // Map the breath to our desired alpha range
            let normalized_alpha = (breath + 1.0) / 2.0;

            min + (normalized_alpha * range)
        }
    }
}
