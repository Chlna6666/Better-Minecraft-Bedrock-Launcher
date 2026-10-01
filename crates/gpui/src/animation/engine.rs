use super::{
    scheduler::AnimationDriver,
    timeline::{
        AnimationParallel, AnimationSequence, AnimationSpec, AnimationStagger,
        ParallelTimelineSample, SequencedTimelineSample, StaggerTimelineSample, TimelineSample,
    },
    transition::{TransitionProperty, resolve_driver_with_cpu_policy},
};
use crate::{Bounds, GlobalElementId, Pixels, SceneAnimationId, SceneAnimationValue};
use collections::{FxHashMap, FxHashSet};
use smallvec::SmallVec;
use std::{
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};

const MIN_COMPLETED_SCENE_TEXT_RASTER_SCALE: f32 = 1.0 / 4096.0;
const SCENE_TEXT_RASTER_SCALE_EPSILON: f32 = 0.0001;
const MAX_SCENE_RETARGET_NORMALIZED_VELOCITY: f32 = 24.0;

fn translate_scene_geometry(property: TransitionProperty, value: &mut [f32; 4], delta: [f32; 2]) {
    match property {
        TransitionProperty::Transform => {
            value[2] += delta[0];
            value[3] += delta[1];
        }
        TransitionProperty::Rotation => {
            value[1] += delta[0];
            value[2] += delta[1];
        }
        TransitionProperty::ClipReveal => {
            value[0] += delta[0];
            value[1] += delta[0];
            value[2] += delta[1];
            value[3] += delta[1];
        }
        _ => {}
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct AnimationTimelineKey {
    element_id: Arc<GlobalElementId>,
    property: TransitionProperty,
}

#[derive(Clone)]
struct AnimationTimeline {
    spec: AnimationSpec,
    spring: Option<super::Spring>,
    spring_initial_velocity: f32,
    started_at: Instant,
    driver: AnimationDriver,
    bounds: Option<Bounds<Pixels>>,
    scene_animation: Option<SceneAnimation>,
    grouped_visual_scene_animation: bool,
    scene_animation_generation: u64,
    endpoint_text_raster_scale: Option<f32>,
    needs_endpoint_reraster: bool,
    completion_invalidation: Option<CompletionInvalidationTarget>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CompletionInvalidationTarget {
    view_id: crate::EntityId,
    retained_id: GlobalElementId,
}

/// UI invalidation to process after a renderer-owned scene animation reaches its endpoint.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SceneAnimationCompletion {
    /// Scene animation whose endpoint needs a retained reraster or equivalent UI update.
    pub(crate) animation_id: SceneAnimationId,
    /// Property identifies one track when several compositor tracks share a scene binding.
    pub(crate) property: TransitionProperty,
    /// Timeline generation that produced this event, so a queued completion cannot finish a retarget.
    pub(crate) generation: u64,
    /// Owning view identified when the animation was committed.
    pub(crate) view_id: Option<crate::EntityId>,
    /// Retained element path that must be refreshed on the UI owner.
    pub(crate) retained_id: Option<GlobalElementId>,
}

impl AnimationTimeline {
    fn sample_with_velocity(&self, now: Instant) -> (TimelineSample, f32) {
        let elapsed = now.saturating_duration_since(self.started_at);
        if let Some(spring) = self.spring {
            if elapsed < self.spec.delay {
                return (
                    TimelineSample {
                        raw_progress: 0.0,
                        eased_progress: 0.0,
                        done: false,
                        applies: self.spec.fill_mode.fills_backwards(),
                    },
                    0.0,
                );
            }

            let active_elapsed = elapsed.saturating_sub(self.spec.delay);
            let sample = spring
                .sample_with_velocity(active_elapsed.as_secs_f32(), self.spring_initial_velocity);
            return (
                TimelineSample {
                    raw_progress: sample.progress,
                    eased_progress: if sample.done { 1.0 } else { sample.progress },
                    done: sample.done,
                    applies: !sample.done || self.spec.fill_mode.fills_forwards(),
                },
                if sample.done { 0.0 } else { sample.velocity },
            );
        }

        (self.spec.sample_elapsed(elapsed), 0.0)
    }

    fn sample(&self, now: Instant) -> TimelineSample {
        self.sample_with_velocity(now).0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SceneAnimation {
    id: SceneAnimationId,
    from: [f32; 4],
    to: [f32; 4],
}

/// Immutable scene-animation input that can be sampled by a presentation owner.
#[derive(Clone)]
pub(crate) struct SceneAnimationTimeline {
    timeline: AnimationTimeline,
    animation: SceneAnimation,
    property: TransitionProperty,
    completion: Option<SceneAnimationCompletion>,
}

pub(crate) struct SceneAnimationSample {
    pub(crate) value: Option<SceneAnimationValue>,
    pub(crate) done: bool,
    pub(crate) completion: Option<SceneAnimationCompletion>,
}

impl SceneAnimationTimeline {
    pub(crate) fn sample_at(&mut self, now: Instant) -> SceneAnimationSample {
        let mut sample = self.timeline.sample(now);
        let repeats = self.timeline.spring.is_some()
            && matches!(self.timeline.spec.repeat, super::RepeatMode::Forever);
        if sample.done && repeats {
            // Match the UI driver's repeating-spring behavior: present the settled endpoint for
            // this sample, then begin a fresh physical cycle from the next compositor frame.
            self.timeline.started_at = now;
            sample.done = false;
            sample.applies = true;
        }
        SceneAnimationSample {
            value: sample.applies.then_some(SceneAnimationValue {
                animation_id: self.animation.id,
                property: self.property,
                progress: sample.eased_progress,
                from: self.animation.from,
                to: self.animation.to,
            }),
            done: sample.done,
            completion: (sample.done && !repeats)
                .then(|| self.completion.clone())
                .flatten(),
        }
    }

    pub(crate) fn animation_id(&self) -> SceneAnimationId {
        self.animation.id
    }

    pub(crate) fn track_key(&self) -> (SceneAnimationId, TransitionProperty) {
        (self.animation.id, self.property)
    }

    pub(crate) fn is_grouped_visual(&self) -> bool {
        self.timeline.grouped_visual_scene_animation
    }

    pub(crate) fn bounds(&self) -> Option<Bounds<Pixels>> {
        self.timeline.bounds
    }
}

#[derive(Clone, Copy, Debug)]
struct CompletedSceneAnimation {
    value: SceneAnimationValue,
    endpoint_text_raster_scale: Option<f32>,
}

/// Identifier for an animation group owned by an [`AnimationEngine`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AnimationGroupId(u64);

impl AnimationGroupId {
    /// Return the numeric identifier backing this group id.
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug)]
enum AnimationGroupTimelineKind {
    Sequence(AnimationSequence),
    Parallel(AnimationParallel),
    Stagger(AnimationStagger),
}

impl AnimationGroupTimelineKind {
    fn sample_elapsed(&self, elapsed: std::time::Duration) -> AnimationGroupSample {
        match self {
            Self::Sequence(sequence) => {
                AnimationGroupSample::Sequence(sequence.sample_elapsed(elapsed))
            }
            Self::Parallel(parallel) => {
                AnimationGroupSample::Parallel(parallel.sample_elapsed(elapsed))
            }
            Self::Stagger(stagger) => {
                AnimationGroupSample::Stagger(stagger.sample_elapsed(elapsed))
            }
        }
    }

    fn is_done_at(&self, elapsed: std::time::Duration) -> bool {
        match self {
            Self::Sequence(sequence) => sequence.is_done_at(elapsed),
            Self::Parallel(parallel) => parallel.is_done_at(elapsed),
            Self::Stagger(stagger) => stagger.is_done_at(elapsed),
        }
    }
}

#[derive(Clone, Debug)]
struct AnimationGroupTimeline {
    kind: AnimationGroupTimelineKind,
    started_at: Instant,
    driver: AnimationDriver,
    bounds: Option<Bounds<Pixels>>,
}

/// Sample returned for an engine-owned animation group.
#[derive(Clone, Debug, PartialEq)]
pub enum AnimationGroupSample {
    /// Sample for a sequence group.
    Sequence(SequencedTimelineSample),
    /// Sample for a parallel group.
    Parallel(ParallelTimelineSample),
    /// Sample for a stagger group.
    Stagger(StaggerTimelineSample),
}

impl AnimationGroupSample {
    /// Returns true when the group has completed.
    pub fn is_done(&self) -> bool {
        match self {
            Self::Sequence(sample) => sample.done,
            Self::Parallel(sample) => sample.done,
            Self::Stagger(sample) => sample.done,
        }
    }
}

/// Summary returned after sampling a window animation engine.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnimationTick {
    /// Remaining active timeline count.
    pub active_count: usize,
    /// Remaining active GPU/paint timeline count.
    pub active_visual_count: usize,
    /// Remaining visual timelines that still need another UI-owned sample.
    pub(crate) ui_active_visual_count: usize,
    /// Whether this tick involved GPU/paint timelines.
    pub has_gpu_or_paint: bool,
    /// Whether this tick involved layout timelines.
    pub has_layout: bool,
    /// Dirty visual bounds touched by sampled paint/GPU timelines.
    pub dirty_bounds: SmallVec<[Bounds<Pixels>; 4]>,
    pub(crate) scene_values: SmallVec<[SceneAnimationValue; 4]>,
    pub(crate) completion_events: SmallVec<[SceneAnimationCompletion; 4]>,
}

/// Per-window animation timeline engine.
#[derive(Default)]
pub struct AnimationEngine {
    timelines: FxHashMap<AnimationTimelineKey, AnimationTimeline>,
    completed_scene_values: FxHashMap<AnimationTimelineKey, CompletedSceneAnimation>,
    timelines_by_element: FxHashMap<Arc<GlobalElementId>, SmallVec<[TransitionProperty; 4]>>,
    visual_timeline_keys: FxHashSet<AnimationTimelineKey>,
    ui_visual_timeline_keys: FxHashSet<AnimationTimelineKey>,
    layout_timeline_keys: FxHashSet<AnimationTimelineKey>,
    group_timelines: FxHashMap<AnimationGroupId, AnimationGroupTimeline>,
    visual_group_ids: FxHashSet<AnimationGroupId>,
    layout_group_ids: FxHashSet<AnimationGroupId>,
    live_scene_animation_ids_scratch: FxHashSet<SceneAnimationId>,
    next_scene_animation_generation: u64,
    next_group_id: u64,
    frame_pending: bool,
}

impl fmt::Debug for AnimationEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnimationEngine")
            .field("timelines", &self.timelines.len())
            .field("indexed_elements", &self.timelines_by_element.len())
            .field("visual_timelines", &self.visual_timeline_keys.len())
            .field("ui_visual_timelines", &self.ui_visual_timeline_keys.len())
            .field("layout_timelines", &self.layout_timeline_keys.len())
            .field("groups", &self.group_timelines.len())
            .field("frame_pending", &self.frame_pending)
            .finish()
    }
}

impl AnimationEngine {
    /// Create a new empty animation engine.
    pub fn new() -> Self {
        Self::default()
    }

    /// Start or replace a transition timeline for an element property.
    pub fn start_transition(
        &mut self,
        element_id: &GlobalElementId,
        property: TransitionProperty,
        spec: AnimationSpec,
        now: Instant,
    ) {
        let driver = resolve_driver_with_cpu_policy(
            spec.driver,
            [property],
            spec.easing.requires_cpu_driver(),
        );
        let element_id = self.shared_element_id(element_id);
        let key = AnimationTimelineKey {
            element_id: element_id.clone(),
            property,
        };
        self.completed_scene_values.remove(&key);
        self.remove_driver_index(&key);
        let started_at = self
            .timelines
            .get(&key)
            .and_then(|timeline| {
                let elapsed = now.saturating_duration_since(timeline.started_at);
                let sample = timeline.sample(now);
                let reference_iteration = timeline.spec.active_iteration_at_elapsed(elapsed);
                now.checked_sub(
                    spec.elapsed_for_raw_progress(sample.raw_progress, reference_iteration),
                )
            })
            .unwrap_or(now);
        let bounds = self
            .timelines
            .get(&key)
            .and_then(|timeline| timeline.bounds);
        self.timelines.insert(
            key.clone(),
            AnimationTimeline {
                spec,
                spring: None,
                spring_initial_velocity: 0.0,
                started_at,
                driver,
                bounds,
                scene_animation: None,
                grouped_visual_scene_animation: false,
                scene_animation_generation: 0,
                endpoint_text_raster_scale: None,
                needs_endpoint_reraster: false,
                completion_invalidation: None,
            },
        );
        self.insert_driver_index(key, driver);
        let indexed_properties = self.timelines_by_element.entry(element_id).or_default();
        if !indexed_properties.contains(&property) {
            indexed_properties.push(property);
        }
    }

    /// Retarget an active renderer-owned scene animation from its current presented value.
    ///
    /// Application state lays the element out at its new final geometry once. The engine samples
    /// the old presentation, converts translation into the new base coordinate system and then
    /// continues entirely in the presentation/compositor lane.
    pub(crate) fn retarget_scene_animation(
        &mut self,
        element_id: &GlobalElementId,
        property: TransitionProperty,
        animation_id: SceneAnimationId,
        spec: AnimationSpec,
        spring: Option<super::Spring>,
        now: Instant,
        dirty_bounds: Bounds<Pixels>,
        base_translation_delta: [f32; 2],
        requested_to: [f32; 4],
    ) -> bool {
        let Some(indexed_element_id) = self.indexed_element_id(element_id).cloned() else {
            return false;
        };
        let key = AnimationTimelineKey {
            element_id: indexed_element_id,
            property,
        };
        let Some(previous) = self.timelines.get(&key) else {
            return false;
        };
        let Some(scene) = previous.scene_animation else {
            return false;
        };
        if scene.id != animation_id {
            return false;
        }

        let (sample, normalized_velocity) = previous.sample_with_velocity(now);
        let mut current = interpolate_scene_value(scene.from, scene.to, sample.eased_progress);
        let current_velocity = scene_property_velocity(scene.from, scene.to, normalized_velocity);

        if matches!(
            property,
            TransitionProperty::HorizontalEdgeFirst | TransitionProperty::HorizontalEdgeSecond
        ) {
            current[0] += base_translation_delta[0];
        } else if property == TransitionProperty::Translation {
            current[0] += base_translation_delta[0];
            current[1] += base_translation_delta[1];
            current[3] = requested_to[3];
        } else if property == TransitionProperty::Rotation {
            current[1] = requested_to[1];
            current[2] = requested_to[2];
        } else if property == TransitionProperty::Transform {
            current[2] = requested_to[2];
            current[3] = requested_to[3];
        }

        let delta = subtract_scene_values(requested_to, current);
        let spring_initial_velocity = if spring.is_some() {
            responsive_scene_retarget_velocity(current_velocity, delta)
        } else {
            0.0
        };
        let driver = resolve_driver_with_cpu_policy(
            spec.driver,
            [property],
            spec.easing.requires_cpu_driver(),
        );

        self.completed_scene_values.remove(&key);
        self.remove_driver_index(&key);
        let Some(timeline) = self.timelines.get_mut(&key) else {
            return false;
        };
        timeline.spec = spec;
        timeline.spring = spring;
        timeline.spring_initial_velocity = spring_initial_velocity;
        timeline.started_at = now;
        timeline.driver = driver;
        timeline.bounds = Some(dirty_bounds);
        self.next_scene_animation_generation =
            self.next_scene_animation_generation.wrapping_add(1).max(1);
        timeline.scene_animation_generation = self.next_scene_animation_generation;
        timeline.scene_animation = Some(SceneAnimation {
            id: animation_id,
            from: current,
            to: requested_to,
        });
        timeline.completion_invalidation = None;
        self.insert_driver_index(key.clone(), driver);

        self.bind_scene_animation(element_id, property, animation_id, current, requested_to);
        true
    }

    /// Start a sequence group and return its engine-owned id.
    pub fn start_sequence(
        &mut self,
        sequence: AnimationSequence,
        now: Instant,
    ) -> AnimationGroupId {
        let driver = resolve_specs_driver(sequence.specs());
        self.start_group(AnimationGroupTimelineKind::Sequence(sequence), driver, now)
    }

    /// Start a parallel group and return its engine-owned id.
    pub fn start_parallel(
        &mut self,
        parallel: AnimationParallel,
        now: Instant,
    ) -> AnimationGroupId {
        let driver = resolve_specs_driver(parallel.specs());
        self.start_group(AnimationGroupTimelineKind::Parallel(parallel), driver, now)
    }

    /// Start a stagger group and return its engine-owned id.
    pub fn start_stagger(&mut self, stagger: AnimationStagger, now: Instant) -> AnimationGroupId {
        let driver = resolve_specs_driver([stagger.spec()]);
        self.start_group(AnimationGroupTimelineKind::Stagger(stagger), driver, now)
    }

    /// Cancel an engine-owned animation group.
    pub fn cancel_group(&mut self, group_id: AnimationGroupId) -> bool {
        self.remove_group(group_id).is_some()
    }

    /// Sample an engine-owned animation group without mutating engine state.
    pub fn sample_group(
        &self,
        group_id: AnimationGroupId,
        now: Instant,
    ) -> Option<AnimationGroupSample> {
        self.group_timelines.get(&group_id).map(|timeline| {
            timeline
                .kind
                .sample_elapsed(now.saturating_duration_since(timeline.started_at))
        })
    }

    /// Update the visual bounds associated with an engine-owned animation group.
    pub fn set_group_bounds(&mut self, group_id: AnimationGroupId, bounds: Bounds<Pixels>) -> bool {
        let Some(timeline) = self.group_timelines.get_mut(&group_id) else {
            return false;
        };
        timeline.bounds = Some(bounds);
        true
    }

    /// Driver selected for an engine-owned animation group.
    pub fn group_driver(&self, group_id: AnimationGroupId) -> Option<AnimationDriver> {
        self.group_timelines
            .get(&group_id)
            .map(|timeline| timeline.driver)
    }

    /// Cancel all timelines for an element.
    pub fn cancel_element(&mut self, element_id: &GlobalElementId) {
        self.completed_scene_values
            .retain(|key, _| key.element_id.as_ref() != element_id);
        let Some(indexed_element_id) = self.indexed_element_id(element_id).cloned() else {
            return;
        };
        let Some(properties) = self.timelines_by_element.remove(&indexed_element_id) else {
            return;
        };
        for property in properties {
            let key = AnimationTimelineKey {
                element_id: indexed_element_id.clone(),
                property,
            };
            self.timelines.remove(&key);
            self.remove_driver_index(&key);
        }
    }

    /// Returns the fastest presentation interval requested by active visual timelines.
    ///
    /// None means no visual timeline is active. A zero duration means at least one active visual
    /// timeline requests the raw platform cadence. Visual groups remain uncapped.
    pub(crate) fn visual_presentation_interval(&self, driver: AnimationDriver) -> Option<Duration> {
        if matches!(driver, AnimationDriver::Layout) {
            return None;
        }
        if !self.visual_group_ids.is_empty() {
            return Some(Duration::ZERO);
        }

        let mut fastest = None::<Duration>;
        for key in &self.visual_timeline_keys {
            let Some(timeline) = self.timelines.get(key) else {
                continue;
            };
            let Some(interval) = timeline.spec.presentation_interval else {
                return Some(Duration::ZERO);
            };
            fastest = Some(fastest.map_or(interval, |current| current.min(interval)));
        }
        fastest
    }

    /// Number of active timelines.
    pub fn active_count(&self) -> usize {
        self.timelines.len() + self.group_timelines.len()
    }

    /// Sample a specific element property timeline without mutating engine state.
    pub fn sample_transition(
        &self,
        element_id: &GlobalElementId,
        property: TransitionProperty,
        now: Instant,
    ) -> Option<TimelineSample> {
        let indexed_element_id = self.indexed_element_id(element_id)?;
        self.timelines
            .get(&AnimationTimelineKey {
                element_id: indexed_element_id.clone(),
                property,
            })
            .map(|timeline| timeline.sample(now))
    }

    /// Update the visual bounds associated with an element property timeline.
    pub fn set_transition_bounds(
        &mut self,
        element_id: &GlobalElementId,
        property: TransitionProperty,
        bounds: Bounds<Pixels>,
    ) -> bool {
        let Some(indexed_element_id) = self.indexed_element_id(element_id).cloned() else {
            return false;
        };
        let key = AnimationTimelineKey {
            element_id: indexed_element_id,
            property,
        };
        let Some(timeline) = self.timelines.get_mut(&key) else {
            return false;
        };
        timeline.bounds = Some(bounds);
        true
    }

    /// Move screen-space origin/clip coordinates without restarting the logical timeline.
    pub(crate) fn translate_scene_animation_origin(
        &mut self,
        element_id: &GlobalElementId,
        property: TransitionProperty,
        delta: [f32; 2],
    ) -> bool {
        let Some(indexed_element_id) = self.indexed_element_id(element_id).cloned() else {
            return false;
        };
        let key = AnimationTimelineKey {
            element_id: indexed_element_id,
            property,
        };
        if let Some(animation) = self
            .timelines
            .get_mut(&key)
            .and_then(|timeline| timeline.scene_animation.as_mut())
        {
            translate_scene_geometry(property, &mut animation.from, delta);
            translate_scene_geometry(property, &mut animation.to, delta);
            return true;
        }
        if let Some(completed) = self.completed_scene_values.get_mut(&key) {
            translate_scene_geometry(property, &mut completed.value.from, delta);
            translate_scene_geometry(property, &mut completed.value.to, delta);
            return true;
        }
        false
    }

    pub(crate) fn bind_scene_animation(
        &mut self,
        element_id: &GlobalElementId,
        property: TransitionProperty,
        id: SceneAnimationId,
        from: [f32; 4],
        to: [f32; 4],
    ) -> bool {
        let Some(indexed_element_id) = self.indexed_element_id(element_id).cloned() else {
            return false;
        };
        let key = AnimationTimelineKey {
            element_id: indexed_element_id,
            property,
        };
        let Some(timeline) = self.timelines.get_mut(&key) else {
            return false;
        };
        let scene_animation = SceneAnimation { id, from, to };
        if timeline.scene_animation != Some(scene_animation) {
            self.next_scene_animation_generation =
                self.next_scene_animation_generation.wrapping_add(1).max(1);
            timeline.scene_animation_generation = self.next_scene_animation_generation;
        }
        timeline.scene_animation = Some(scene_animation);

        let endpoint_text_raster_scale = if matches!(
            property,
            TransitionProperty::Scale | TransitionProperty::Transform
        ) {
            let scale = to[0];
            (scale.is_finite() && scale > 0.0)
                .then_some(scale.max(MIN_COMPLETED_SCENE_TEXT_RASTER_SCALE))
        } else {
            None
        };
        let active_text_raster_scale = super::scene_text_raster_scale(property, from, to);
        timeline.needs_endpoint_reraster = endpoint_text_raster_scale.is_some_and(|endpoint| {
            (active_text_raster_scale - endpoint).abs() > SCENE_TEXT_RASTER_SCALE_EPSILON
        });
        timeline.endpoint_text_raster_scale = endpoint_text_raster_scale;
        timeline.completion_invalidation = None;
        self.ui_visual_timeline_keys.remove(&key);
        true
    }

    /// Returns whether the matching track's presentation grouping changed.
    pub(crate) fn set_grouped_visual_scene_animation(
        &mut self,
        element_id: &GlobalElementId,
        property: TransitionProperty,
        animation_id: SceneAnimationId,
        grouped: bool,
    ) -> bool {
        let Some(indexed_element_id) = self.indexed_element_id(element_id).cloned() else {
            return false;
        };
        let Some(timeline) = self.timelines.get_mut(&AnimationTimelineKey {
            element_id: indexed_element_id,
            property,
        }) else {
            return false;
        };
        if !timeline
            .scene_animation
            .is_some_and(|animation| animation.id == animation_id)
        {
            return false;
        }
        if timeline.grouped_visual_scene_animation == grouped {
            return false;
        }
        timeline.grouped_visual_scene_animation = grouped;
        true
    }

    pub(crate) fn scene_animation_needs_completion_invalidation(
        &self,
        animation_id: SceneAnimationId,
    ) -> bool {
        self.timelines.values().any(|timeline| {
            timeline
                .scene_animation
                .is_some_and(|animation| animation.id == animation_id)
                && timeline.completion_invalidation.is_none()
        })
    }

    pub(crate) fn set_scene_animation_completion_invalidation(
        &mut self,
        animation_id: SceneAnimationId,
        view_id: crate::EntityId,
        retained_id: GlobalElementId,
    ) -> bool {
        let mut updated = false;
        for timeline in self.timelines.values_mut().filter(|timeline| {
            timeline
                .scene_animation
                .is_some_and(|animation| animation.id == animation_id)
        }) {
            if timeline.completion_invalidation.is_none() {
                timeline.completion_invalidation = Some(CompletionInvalidationTarget {
                    view_id,
                    retained_id: retained_id.clone(),
                });
                updated = true;
            }
        }
        updated
    }

    /// Transfer a compositor completion back into UI-owned animation state.
    pub(crate) fn complete_scene_animation(
        &mut self,
        completion: &SceneAnimationCompletion,
    ) -> Option<SceneAnimationCompletion> {
        let (key, timeline) = self.timelines.iter().find_map(|(key, timeline)| {
            timeline
                .scene_animation
                .is_some_and(|animation| {
                    animation.id == completion.animation_id && key.property == completion.property
                })
                .then(|| (key.clone(), timeline.clone()))
        })?;
        let animation = timeline.scene_animation?;
        let target = timeline.completion_invalidation.as_ref();
        let current_completion = SceneAnimationCompletion {
            animation_id: animation.id,
            property: key.property,
            generation: timeline.scene_animation_generation,
            view_id: target.map(|target| target.view_id),
            retained_id: target.map(|target| target.retained_id.clone()),
        };
        if &current_completion != completion {
            return None;
        }

        if timeline.spec.fill_mode.fills_forwards() {
            self.completed_scene_values.insert(
                key.clone(),
                CompletedSceneAnimation {
                    value: SceneAnimationValue {
                        animation_id: animation.id,
                        property: key.property,
                        progress: 1.0,
                        from: animation.from,
                        to: animation.to,
                    },
                    endpoint_text_raster_scale: timeline.endpoint_text_raster_scale,
                },
            );
        } else {
            self.completed_scene_values.remove(&key);
        }
        self.remove_timeline(&key);
        if !self.has_active_timelines() {
            self.frame_pending = false;
        }

        Some(current_completion)
    }

    pub(crate) fn scene_animation_is_active(&self, animation_id: SceneAnimationId) -> bool {
        self.timelines.values().any(|timeline| {
            timeline
                .scene_animation
                .is_some_and(|animation| animation.id == animation_id)
        })
    }

    pub(crate) fn scene_animation_track_is_active(
        &self,
        animation_id: SceneAnimationId,
        property: TransitionProperty,
    ) -> bool {
        self.timelines.iter().any(|(key, timeline)| {
            key.property == property
                && timeline
                    .scene_animation
                    .is_some_and(|animation| animation.id == animation_id)
        })
    }

    pub(crate) fn scene_animation_track_is_bound(
        &self,
        animation_id: SceneAnimationId,
        property: TransitionProperty,
    ) -> bool {
        self.scene_animation_track_is_active(animation_id, property)
            || self.completed_scene_values.values().any(|completed| {
                completed.value.animation_id == animation_id && completed.value.property == property
            })
    }

    pub(crate) fn scene_animation_is_bound(&self, animation_id: SceneAnimationId) -> bool {
        self.scene_animation_is_active(animation_id)
            || self
                .completed_scene_values
                .values()
                .any(|completed| completed.value.animation_id == animation_id)
    }

    pub(crate) fn cancel_scene_animation_track(
        &mut self,
        element_id: &GlobalElementId,
        property: TransitionProperty,
        animation_id: SceneAnimationId,
    ) -> bool {
        let Some(indexed_element_id) = self.indexed_element_id(element_id).cloned() else {
            return false;
        };
        let key = AnimationTimelineKey {
            element_id: indexed_element_id,
            property,
        };
        if !self
            .timelines
            .get(&key)
            .and_then(|timeline| timeline.scene_animation)
            .is_some_and(|animation| animation.id == animation_id)
        {
            return false;
        }
        self.remove_timeline(&key);
        self.completed_scene_values.remove(&key);
        if !self.has_active_timelines() {
            self.frame_pending = false;
        }
        true
    }

    pub(crate) fn completed_scene_text_raster_scale(
        &self,
        animation_id: SceneAnimationId,
    ) -> Option<f32> {
        self.completed_scene_values
            .values()
            .filter(|completed| completed.value.animation_id == animation_id)
            .find_map(|completed| completed.endpoint_text_raster_scale)
    }

    pub(crate) fn set_transition_spring(
        &mut self,
        element_id: &GlobalElementId,
        property: TransitionProperty,
        spring: super::Spring,
    ) {
        let Some(element_id) = self.indexed_element_id(element_id).cloned() else {
            return;
        };
        if let Some(timeline) = self.timelines.get_mut(&AnimationTimelineKey {
            element_id,
            property,
        }) {
            timeline.spring = Some(spring);
            timeline.spring_initial_velocity = 0.0;
        }
    }

    pub(crate) fn transition_driver(
        &self,
        element_id: &GlobalElementId,
        property: TransitionProperty,
    ) -> Option<AnimationDriver> {
        let indexed_element_id = self.indexed_element_id(element_id)?;
        self.timelines
            .get(&AnimationTimelineKey {
                element_id: indexed_element_id.clone(),
                property,
            })
            .map(|timeline| timeline.driver)
    }

    /// Returns whether pruning removed timeline or endpoint metadata from the presentation packet.
    pub(crate) fn retain_scene_animations_for_scene(
        &mut self,
        scene: &crate::scene::Scene,
    ) -> bool {
        let previous_counts = (self.timelines.len(), self.completed_scene_values.len());
        let mut live_ids = std::mem::take(&mut self.live_scene_animation_ids_scratch);
        live_ids.clear();
        scene.collect_animation_ids_into(&mut live_ids);
        let live_count = live_ids.len();

        self.retain_scene_animations(&live_ids);

        let target = 16usize.max(live_count);
        if live_ids.capacity() > target.saturating_mul(4) {
            live_ids.shrink_to(target);
        }
        live_ids.clear();
        self.live_scene_animation_ids_scratch = live_ids;
        previous_counts != (self.timelines.len(), self.completed_scene_values.len())
    }

    pub(crate) fn retain_scene_animations(&mut self, live_ids: &FxHashSet<SceneAnimationId>) {
        self.completed_scene_values
            .retain(|_, completed| live_ids.contains(&completed.value.animation_id));
        let stale_keys = self
            .timelines
            .iter()
            .filter_map(|(key, timeline)| {
                timeline
                    .scene_animation
                    .is_some_and(|animation| !live_ids.contains(&animation.id))
                    .then(|| key.clone())
            })
            .collect::<SmallVec<[_; 8]>>();
        for key in stale_keys {
            self.remove_timeline(&key);
        }
    }

    pub(crate) fn scene_values(&self, now: Instant) -> SmallVec<[SceneAnimationValue; 4]> {
        self.timelines
            .iter()
            .filter_map(|(key, timeline)| {
                let animation = timeline.scene_animation?;
                let sample = timeline.sample(now);
                sample.applies.then_some(SceneAnimationValue {
                    animation_id: animation.id,
                    property: key.property,
                    progress: sample.eased_progress,
                    from: animation.from,
                    to: animation.to,
                })
            })
            .chain(
                self.completed_scene_values
                    .values()
                    .map(|completed| completed.value),
            )
            .collect()
    }

    /// Snapshot active scene timelines for sampling outside the UI frame owner.
    pub(crate) fn presentation_timelines(&self) -> SmallVec<[SceneAnimationTimeline; 4]> {
        self.timelines
            .iter()
            .filter_map(|(key, timeline)| {
                timeline
                    .scene_animation
                    .map(|animation| SceneAnimationTimeline {
                        timeline: timeline.clone(),
                        animation,
                        property: key.property,
                        completion: Some(SceneAnimationCompletion {
                            animation_id: animation.id,
                            property: key.property,
                            generation: timeline.scene_animation_generation,
                            view_id: timeline
                                .completion_invalidation
                                .as_ref()
                                .map(|target| target.view_id),
                            retained_id: timeline
                                .completion_invalidation
                                .as_ref()
                                .map(|target| target.retained_id.clone()),
                        }),
                    })
            })
            .collect()
    }

    /// Returns true when there are active timelines.
    pub fn has_active_timelines(&self) -> bool {
        !self.timelines.is_empty() || !self.group_timelines.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn test_index_counts(&self) -> (usize, usize, usize, usize) {
        (
            self.timelines_by_element.len(),
            self.visual_timeline_keys.len(),
            self.layout_timeline_keys.len(),
            self.ui_visual_timeline_keys.len(),
        )
    }

    /// Mark a driver frame as pending. Returns false when one was already pending.
    pub fn mark_frame_pending(&mut self) -> bool {
        if self.frame_pending {
            false
        } else {
            self.frame_pending = true;
            true
        }
    }

    /// Clear the pending-frame marker.
    pub fn clear_frame_pending(&mut self) {
        self.frame_pending = false;
    }

    /// Sample active timelines for the requested driver and remove finite
    /// completed timelines.
    pub fn tick_driver(&mut self, driver: AnimationDriver, now: Instant) -> AnimationTick {
        let keys = self.timeline_keys_for_driver(driver);
        let group_ids = self.group_ids_for_driver(driver);
        self.tick_keys(now, keys, group_ids)
    }

    /// Tick visual work that is not already sampled from the committed scene by the platform.
    pub(crate) fn tick_driver_with_compositor(
        &mut self,
        driver: AnimationDriver,
        now: Instant,
    ) -> AnimationTick {
        let keys = self.ui_timeline_keys_for_driver(driver);
        let group_ids = self.group_ids_for_driver(driver);
        let mut tick = self.tick_keys(now, keys, group_ids);
        tick.ui_active_visual_count = self.ui_active_visual_count_for(driver);
        tick
    }

    /// Sample all active timelines once and remove finite completed timelines.
    pub fn tick(&mut self, now: Instant) -> AnimationTick {
        let keys = self.timelines.keys().cloned().collect();
        let group_ids = self.group_timelines.keys().copied().collect();
        self.tick_keys(now, keys, group_ids)
    }

    fn tick_keys(
        &mut self,
        now: Instant,
        keys: SmallVec<[AnimationTimelineKey; 16]>,
        group_ids: SmallVec<[AnimationGroupId; 8]>,
    ) -> AnimationTick {
        self.frame_pending = false;

        let mut has_gpu_or_paint = false;
        let mut has_layout = false;
        let mut dirty_bounds = SmallVec::new();
        let mut completion_events = SmallVec::new();
        let mut scene_values: SmallVec<[SceneAnimationValue; 4]> = self
            .completed_scene_values
            .values()
            .map(|completed| completed.value)
            .collect();
        for key in keys {
            let Some(timeline) = self.timelines.get(&key) else {
                self.remove_driver_index(&key);
                continue;
            };
            let sample = timeline.sample(now);
            let repeats = timeline.spring.is_some()
                && matches!(timeline.spec.repeat, super::RepeatMode::Forever);

            if let Some(animation) = timeline.scene_animation {
                let value = SceneAnimationValue {
                    animation_id: animation.id,
                    property: key.property,
                    progress: sample.eased_progress,
                    from: animation.from,
                    to: animation.to,
                };
                if sample.applies {
                    scene_values.push(value);
                } else {
                    self.completed_scene_values.remove(&key);
                }

                if sample.done && !repeats {
                    if sample.applies && timeline.spec.fill_mode.fills_forwards() {
                        self.completed_scene_values.insert(
                            key.clone(),
                            CompletedSceneAnimation {
                                value,
                                endpoint_text_raster_scale: timeline.endpoint_text_raster_scale,
                            },
                        );
                    } else {
                        self.completed_scene_values.remove(&key);
                    }
                    completion_events.push(SceneAnimationCompletion {
                        animation_id: animation.id,
                        property: key.property,
                        generation: timeline.scene_animation_generation,
                        view_id: timeline
                            .completion_invalidation
                            .as_ref()
                            .map(|target| target.view_id),
                        retained_id: timeline
                            .completion_invalidation
                            .as_ref()
                            .map(|target| target.retained_id.clone()),
                    });
                }
            }

            match timeline.driver {
                AnimationDriver::Gpu | AnimationDriver::Paint | AnimationDriver::Auto => {
                    has_gpu_or_paint = true;
                    if let Some(bounds) = timeline.bounds {
                        dirty_bounds.push(bounds);
                    }
                }
                AnimationDriver::Layout => has_layout = true,
            }

            if sample.done {
                if repeats {
                    if let Some(timeline) = self.timelines.get_mut(&key) {
                        timeline.started_at = now;
                    }
                } else {
                    self.remove_timeline(&key);
                }
                continue;
            }
        }

        for group_id in group_ids {
            let Some(timeline) = self.group_timelines.get(&group_id) else {
                self.remove_group_driver_index(group_id);
                continue;
            };
            let done = timeline
                .kind
                .is_done_at(now.saturating_duration_since(timeline.started_at));

            match timeline.driver {
                AnimationDriver::Gpu | AnimationDriver::Paint | AnimationDriver::Auto => {
                    has_gpu_or_paint = true;
                    if let Some(bounds) = timeline.bounds {
                        dirty_bounds.push(bounds);
                    }
                }
                AnimationDriver::Layout => has_layout = true,
            }

            if done {
                self.remove_group(group_id);
            }
        }

        AnimationTick {
            active_count: self.active_count(),
            active_visual_count: self.visual_timeline_keys.len() + self.visual_group_ids.len(),
            ui_active_visual_count: self.visual_timeline_keys.len() + self.visual_group_ids.len(),
            has_gpu_or_paint,
            has_layout,
            dirty_bounds,
            scene_values,
            completion_events,
        }
    }

    fn timeline_keys_for_driver(
        &self,
        driver: AnimationDriver,
    ) -> SmallVec<[AnimationTimelineKey; 16]> {
        match driver {
            AnimationDriver::Auto => self.timelines.keys().cloned().collect(),
            AnimationDriver::Layout => self.layout_timeline_keys.iter().cloned().collect(),
            AnimationDriver::Gpu | AnimationDriver::Paint => {
                self.visual_timeline_keys.iter().cloned().collect()
            }
        }
    }

    fn ui_timeline_keys_for_driver(
        &self,
        driver: AnimationDriver,
    ) -> SmallVec<[AnimationTimelineKey; 16]> {
        match driver {
            AnimationDriver::Auto => self
                .ui_visual_timeline_keys
                .iter()
                .chain(self.layout_timeline_keys.iter())
                .cloned()
                .collect(),
            AnimationDriver::Layout => self.layout_timeline_keys.iter().cloned().collect(),
            AnimationDriver::Gpu | AnimationDriver::Paint => {
                self.ui_visual_timeline_keys.iter().cloned().collect()
            }
        }
    }

    fn group_ids_for_driver(&self, driver: AnimationDriver) -> SmallVec<[AnimationGroupId; 8]> {
        match driver {
            AnimationDriver::Auto => self.group_timelines.keys().copied().collect(),
            AnimationDriver::Layout => self.layout_group_ids.iter().copied().collect(),
            AnimationDriver::Gpu | AnimationDriver::Paint => {
                self.visual_group_ids.iter().copied().collect()
            }
        }
    }

    fn ui_active_visual_count_for(&self, driver: AnimationDriver) -> usize {
        if matches!(driver, AnimationDriver::Layout) {
            return 0;
        }

        self.ui_visual_timeline_keys.len() + self.visual_group_ids.len()
    }

    fn remove_timeline(&mut self, key: &AnimationTimelineKey) -> Option<AnimationTimeline> {
        let timeline = self.timelines.remove(key)?;
        self.remove_driver_index(key);
        self.remove_indexed_property(&key.element_id, key.property);
        Some(timeline)
    }

    fn start_group(
        &mut self,
        kind: AnimationGroupTimelineKind,
        driver: AnimationDriver,
        now: Instant,
    ) -> AnimationGroupId {
        let group_id = self.next_group_id();
        self.group_timelines.insert(
            group_id,
            AnimationGroupTimeline {
                kind,
                started_at: now,
                driver,
                bounds: None,
            },
        );
        self.insert_group_driver_index(group_id, driver);
        group_id
    }

    fn remove_group(&mut self, group_id: AnimationGroupId) -> Option<AnimationGroupTimeline> {
        let timeline = self.group_timelines.remove(&group_id)?;
        self.remove_group_driver_index(group_id);
        Some(timeline)
    }

    fn insert_driver_index(&mut self, key: AnimationTimelineKey, driver: AnimationDriver) {
        if matches!(driver, AnimationDriver::Layout) {
            self.layout_timeline_keys.insert(key);
        } else {
            if self
                .timelines
                .get(&key)
                .is_some_and(|timeline| timeline.scene_animation.is_none())
            {
                self.ui_visual_timeline_keys.insert(key.clone());
            }
            self.visual_timeline_keys.insert(key);
        }
    }

    fn remove_driver_index(&mut self, key: &AnimationTimelineKey) {
        self.visual_timeline_keys.remove(key);
        self.ui_visual_timeline_keys.remove(key);
        self.layout_timeline_keys.remove(key);
    }

    fn insert_group_driver_index(&mut self, group_id: AnimationGroupId, driver: AnimationDriver) {
        if matches!(driver, AnimationDriver::Layout) {
            self.layout_group_ids.insert(group_id);
        } else {
            self.visual_group_ids.insert(group_id);
        }
    }

    fn remove_group_driver_index(&mut self, group_id: AnimationGroupId) {
        self.visual_group_ids.remove(&group_id);
        self.layout_group_ids.remove(&group_id);
    }

    fn next_group_id(&mut self) -> AnimationGroupId {
        loop {
            let group_id = AnimationGroupId(self.next_group_id);
            self.next_group_id = self.next_group_id.wrapping_add(1);
            if !self.group_timelines.contains_key(&group_id) {
                return group_id;
            }
        }
    }

    fn remove_indexed_property(
        &mut self,
        element_id: &Arc<GlobalElementId>,
        property: TransitionProperty,
    ) {
        let remove_element = if let Some(properties) = self.timelines_by_element.get_mut(element_id)
        {
            properties.retain(|indexed_property| *indexed_property != property);
            properties.is_empty()
        } else {
            false
        };
        if remove_element {
            self.timelines_by_element.remove(element_id);
        }
    }

    fn indexed_element_id(&self, element_id: &GlobalElementId) -> Option<&Arc<GlobalElementId>> {
        self.timelines_by_element
            .get_key_value(element_id)
            .map(|(indexed_element_id, _)| indexed_element_id)
    }

    fn shared_element_id(&mut self, element_id: &GlobalElementId) -> Arc<GlobalElementId> {
        self.indexed_element_id(element_id)
            .cloned()
            .unwrap_or_else(|| Arc::new(element_id.clone()))
    }
}

fn interpolate_scene_value(from: [f32; 4], to: [f32; 4], progress: f32) -> [f32; 4] {
    [
        from[0] + (to[0] - from[0]) * progress,
        from[1] + (to[1] - from[1]) * progress,
        from[2] + (to[2] - from[2]) * progress,
        from[3] + (to[3] - from[3]) * progress,
    ]
}

fn scene_property_velocity(from: [f32; 4], to: [f32; 4], normalized_velocity: f32) -> [f32; 4] {
    [
        (to[0] - from[0]) * normalized_velocity,
        (to[1] - from[1]) * normalized_velocity,
        (to[2] - from[2]) * normalized_velocity,
        (to[3] - from[3]) * normalized_velocity,
    ]
}

fn subtract_scene_values(to: [f32; 4], from: [f32; 4]) -> [f32; 4] {
    [
        to[0] - from[0],
        to[1] - from[1],
        to[2] - from[2],
        to[3] - from[3],
    ]
}

fn responsive_scene_retarget_velocity(velocity: [f32; 4], delta: [f32; 4]) -> f32 {
    let dot = velocity
        .iter()
        .zip(delta.iter())
        .map(|(velocity, delta)| velocity * delta)
        .sum::<f32>();
    let magnitude_squared = delta.iter().map(|delta| delta * delta).sum::<f32>();

    if !dot.is_finite() || !magnitude_squared.is_finite() || magnitude_squared <= 1e-8 || dot <= 0.0
    {
        return 0.0;
    }

    (dot / magnitude_squared).clamp(0.0, MAX_SCENE_RETARGET_NORMALIZED_VELOCITY)
}

fn resolve_specs_driver<'a>(specs: impl IntoIterator<Item = &'a AnimationSpec>) -> AnimationDriver {
    let mut has_gpu_driver = false;
    let mut requires_cpu_driver = false;

    for spec in specs {
        if matches!(spec.driver, AnimationDriver::Layout) {
            return AnimationDriver::Layout;
        }
        has_gpu_driver |= matches!(spec.driver, AnimationDriver::Gpu);
        requires_cpu_driver |= spec.easing.requires_cpu_driver();
    }

    if requires_cpu_driver {
        AnimationDriver::Paint
    } else if has_gpu_driver {
        AnimationDriver::Gpu
    } else {
        AnimationDriver::Paint
    }
}

#[cfg(test)]
mod geometry_tests {
    use super::*;

    #[test]
    fn scrolling_origin_preserves_animation_phase_and_clip_relative_to_content() {
        let now = Instant::now();
        let element_id = GlobalElementId::from_path(&[crate::ElementId::from("scrolling")]);
        for (property, from, to, expected_from, expected_to) in [
            (
                TransitionProperty::Transform,
                [0.7, 1.0, 100.0, 200.0],
                [1.0, 1.0, 100.0, 200.0],
                [0.7, 1.0, 100.0, 120.0],
                [1.0, 1.0, 100.0, 120.0],
            ),
            (
                TransitionProperty::Rotation,
                [0.0, 100.0, 200.0, 0.0],
                [0.3, 100.0, 200.0, 0.0],
                [0.0, 100.0, 120.0, 0.0],
                [0.3, 100.0, 120.0, 0.0],
            ),
            (
                TransitionProperty::ClipReveal,
                [50.0, 150.0, 200.0, 200.0],
                [50.0, 150.0, 200.0, 300.0],
                [50.0, 150.0, 120.0, 120.0],
                [50.0, 150.0, 120.0, 220.0],
            ),
        ] {
            let mut engine = AnimationEngine::new();
            engine.start_transition(
                &element_id,
                property,
                AnimationSpec::new(Duration::from_secs(1))
                    .ease(super::super::Easing::Linear)
                    .driver(AnimationDriver::Gpu),
                now,
            );
            assert!(engine.bind_scene_animation(
                &element_id,
                property,
                SceneAnimationId(77),
                from,
                to
            ));
            let before = engine.scene_values(now + Duration::from_millis(250))[0];
            assert!(engine.translate_scene_animation_origin(&element_id, property, [0.0, -80.0]));
            let after = engine.scene_values(now + Duration::from_millis(250))[0];
            assert_eq!(after.progress, before.progress);
            assert_eq!(after.from, expected_from);
            assert_eq!(after.to, expected_to);
            assert_eq!(
                engine.scene_values(now + Duration::from_millis(500))[0].progress,
                0.5
            );
        }
    }
}
