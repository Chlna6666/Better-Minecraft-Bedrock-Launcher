use super::{
    scheduler::AnimationDriver,
    timeline::{
        AnimationParallel, AnimationSequence, AnimationSpec, AnimationStagger,
        ParallelTimelineSample, SequencedTimelineSample, StaggerTimelineSample, TimelineSample,
    },
    transition::{TransitionProperty, resolve_driver_with_cpu_policy},
};
use crate::{Bounds, GlobalElementId, Pixels};
use collections::{FxHashMap, FxHashSet};
use smallvec::SmallVec;
use std::{fmt, rc::Rc, time::Instant};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct AnimationTimelineKey {
    element_id: Rc<GlobalElementId>,
    property: TransitionProperty,
}

#[derive(Clone, Debug)]
struct AnimationTimeline {
    spec: AnimationSpec,
    started_at: Instant,
    driver: AnimationDriver,
    bounds: Option<Bounds<Pixels>>,
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
    pub fn done(&self) -> bool {
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
    /// Whether this tick involved GPU/paint timelines.
    pub has_gpu_or_paint: bool,
    /// Whether this tick involved layout timelines.
    pub has_layout: bool,
    /// Dirty visual bounds touched by sampled paint/GPU timelines.
    pub dirty_bounds: SmallVec<[Bounds<Pixels>; 4]>,
}

/// Per-window animation timeline engine.
#[derive(Default)]
pub struct AnimationEngine {
    timelines: FxHashMap<AnimationTimelineKey, AnimationTimeline>,
    timelines_by_element: FxHashMap<Rc<GlobalElementId>, SmallVec<[TransitionProperty; 4]>>,
    visual_timeline_keys: FxHashSet<AnimationTimelineKey>,
    layout_timeline_keys: FxHashSet<AnimationTimelineKey>,
    group_timelines: FxHashMap<AnimationGroupId, AnimationGroupTimeline>,
    visual_group_ids: FxHashSet<AnimationGroupId>,
    layout_group_ids: FxHashSet<AnimationGroupId>,
    next_group_id: u64,
    frame_pending: bool,
}

impl fmt::Debug for AnimationEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnimationEngine")
            .field("timelines", &self.timelines.len())
            .field("indexed_elements", &self.timelines_by_element.len())
            .field("visual_timelines", &self.visual_timeline_keys.len())
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
        self.remove_driver_index(&key);
        let started_at = self
            .timelines
            .get(&key)
            .and_then(|timeline| {
                let elapsed = now.saturating_duration_since(timeline.started_at);
                let sample = timeline.spec.sample_elapsed(elapsed);
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
                started_at,
                driver,
                bounds,
            },
        );
        self.insert_driver_index(key, driver);
        let indexed_properties = self.timelines_by_element.entry(element_id).or_default();
        if !indexed_properties.contains(&property) {
            indexed_properties.push(property);
        }
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
            .map(|timeline| {
                timeline
                    .spec
                    .sample_elapsed(now.saturating_duration_since(timeline.started_at))
            })
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
        let Some(timeline) = self.timelines.get_mut(&AnimationTimelineKey {
            element_id: indexed_element_id,
            property,
        }) else {
            return false;
        };
        timeline.bounds = Some(bounds);
        true
    }

    /// Returns true when there are active timelines.
    pub fn has_active_timelines(&self) -> bool {
        !self.timelines.is_empty() || !self.group_timelines.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn test_index_counts(&self) -> (usize, usize, usize) {
        (
            self.timelines_by_element.len(),
            self.visual_timeline_keys.len(),
            self.layout_timeline_keys.len(),
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
        for key in keys {
            let Some(timeline) = self.timelines.get(&key) else {
                self.remove_driver_index(&key);
                continue;
            };
            let sample = timeline
                .spec
                .sample_elapsed(now.saturating_duration_since(timeline.started_at));

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
                self.remove_timeline(&key);
                continue;
            }
        }

        for group_id in group_ids {
            let Some(timeline) = self.group_timelines.get(&group_id) else {
                self.remove_group_driver_index(group_id);
                continue;
            };
            let sample = timeline
                .kind
                .sample_elapsed(now.saturating_duration_since(timeline.started_at));

            match timeline.driver {
                AnimationDriver::Gpu | AnimationDriver::Paint | AnimationDriver::Auto => {
                    has_gpu_or_paint = true;
                    if let Some(bounds) = timeline.bounds {
                        dirty_bounds.push(bounds);
                    }
                }
                AnimationDriver::Layout => has_layout = true,
            }

            if sample.done() {
                self.remove_group(group_id);
            }
        }

        AnimationTick {
            active_count: self.active_count(),
            has_gpu_or_paint,
            has_layout,
            dirty_bounds,
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

    fn group_ids_for_driver(&self, driver: AnimationDriver) -> SmallVec<[AnimationGroupId; 8]> {
        match driver {
            AnimationDriver::Auto => self.group_timelines.keys().copied().collect(),
            AnimationDriver::Layout => self.layout_group_ids.iter().copied().collect(),
            AnimationDriver::Gpu | AnimationDriver::Paint => {
                self.visual_group_ids.iter().copied().collect()
            }
        }
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
            self.visual_timeline_keys.insert(key);
        }
    }

    fn remove_driver_index(&mut self, key: &AnimationTimelineKey) {
        self.visual_timeline_keys.remove(key);
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
        element_id: &Rc<GlobalElementId>,
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

    fn indexed_element_id(&self, element_id: &GlobalElementId) -> Option<&Rc<GlobalElementId>> {
        self.timelines_by_element
            .get_key_value(element_id)
            .map(|(indexed_element_id, _)| indexed_element_id)
    }

    fn shared_element_id(&mut self, element_id: &GlobalElementId) -> Rc<GlobalElementId> {
        self.indexed_element_id(element_id)
            .cloned()
            .unwrap_or_else(|| Rc::new(GlobalElementId(element_id.0.clone())))
    }
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
