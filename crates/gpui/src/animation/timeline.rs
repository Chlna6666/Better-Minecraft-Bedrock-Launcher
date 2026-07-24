use super::{easing::Easing, scheduler::AnimationDriver};
use std::time::{Duration, Instant};

/// How an animation repeats after one iteration.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    PartialEq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
pub enum RepeatMode {
    /// Run the animation once.
    #[default]
    Once,
    /// Repeat forever.
    Forever,
    /// Repeat the specified number of extra iterations.
    Count(u32),
}

/// Direction used when sampling each animation iteration.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    PartialEq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
pub enum AnimationDirection {
    /// Sample from start to end.
    #[default]
    Normal,
    /// Sample from end to start.
    Reverse,
    /// Alternate normal and reverse iterations.
    Alternate,
    /// Alternate reverse and normal iterations.
    AlternateReverse,
}

/// Fill behavior outside the active interval.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    PartialEq,
    serde::Serialize,
    serde::Deserialize,
    schemars::JsonSchema,
)]
pub enum FillMode {
    /// Use the underlying style before and after the active interval.
    #[default]
    None,
    /// Hold the first keyframe during delay.
    Backwards,
    /// Hold the last sampled keyframe after completion.
    Forwards,
    /// Hold both start and end keyframes outside the active interval.
    Both,
}

impl FillMode {
    pub(crate) fn fills_backwards(self) -> bool {
        matches!(self, Self::Backwards | Self::Both)
    }

    pub(crate) fn fills_forwards(self) -> bool {
        matches!(self, Self::Forwards | Self::Both)
    }
}

/// Full timing description for an animation.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationSpec {
    /// Active duration of one iteration.
    pub duration: Duration,
    /// Delay before the active interval begins.
    pub delay: Duration,
    /// Repeat behavior.
    pub repeat: RepeatMode,
    /// Direction behavior per iteration.
    pub direction: AnimationDirection,
    /// Fill behavior before/after the active interval.
    pub fill_mode: FillMode,
    /// Easing curve.
    pub easing: Easing,
    /// Preferred driver.
    pub driver: AnimationDriver,
}

impl AnimationSpec {
    /// Create a new animation spec with the given duration.
    pub fn new(duration: Duration) -> Self {
        Self {
            duration,
            delay: Duration::ZERO,
            repeat: RepeatMode::Once,
            direction: AnimationDirection::Normal,
            fill_mode: FillMode::Forwards,
            easing: Easing::Linear,
            driver: AnimationDriver::Auto,
        }
    }

    /// Set the start delay.
    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// Set the easing curve.
    pub fn ease(mut self, easing: Easing) -> Self {
        self.easing = easing;
        self
    }

    /// Set repeat behavior.
    pub fn repeat(mut self, repeat: RepeatMode) -> Self {
        self.repeat = repeat;
        self
    }

    /// Set direction behavior.
    pub fn direction(mut self, direction: AnimationDirection) -> Self {
        self.direction = direction;
        self
    }

    /// Set fill behavior.
    pub fn fill_mode(mut self, fill_mode: FillMode) -> Self {
        self.fill_mode = fill_mode;
        self
    }

    /// Set preferred driver.
    pub fn driver(mut self, driver: AnimationDriver) -> Self {
        self.driver = driver;
        self
    }

    /// Sample this spec relative to the supplied elapsed time.
    pub fn sample_elapsed(&self, elapsed: Duration) -> TimelineSample {
        if elapsed < self.delay {
            let progress = if self.fill_mode.fills_backwards() {
                self.direction_progress(0, 0.0)
            } else {
                0.0
            };
            return TimelineSample {
                raw_progress: progress,
                eased_progress: self.easing.sample(progress),
                done: false,
            };
        }

        if self.duration.is_zero() {
            let progress = self.direction_progress(0, 1.0);
            return TimelineSample {
                raw_progress: progress,
                eased_progress: self.easing.sample(progress),
                done: true,
            };
        }

        let active_elapsed = elapsed.saturating_sub(self.delay);
        let duration = self.duration.as_secs_f64();
        let active = active_elapsed.as_secs_f64();
        let iteration = (active / duration).floor() as u64;
        let mut iteration_progress = (active % duration) / duration;
        let final_iteration = self.final_iteration();

        let done = final_iteration.is_some_and(|final_iteration| iteration > final_iteration);
        let iteration = if let Some(final_iteration) = final_iteration {
            iteration.min(final_iteration)
        } else {
            iteration
        };

        if done {
            iteration_progress = if self.fill_mode.fills_forwards() {
                1.0
            } else {
                0.0
            };
        }

        let raw_progress = self.direction_progress(iteration, iteration_progress as f32);
        TimelineSample {
            raw_progress,
            eased_progress: self.easing.sample(raw_progress),
            done,
        }
    }

    fn direction_progress(&self, iteration: u64, progress: f32) -> f32 {
        match self.direction {
            AnimationDirection::Normal => progress,
            AnimationDirection::Reverse => 1.0 - progress,
            AnimationDirection::Alternate => {
                if iteration % 2 == 0 {
                    progress
                } else {
                    1.0 - progress
                }
            }
            AnimationDirection::AlternateReverse => {
                if iteration % 2 == 0 {
                    1.0 - progress
                } else {
                    progress
                }
            }
        }
    }

    pub(crate) fn active_iteration_at_elapsed(&self, elapsed: Duration) -> u64 {
        if self.duration.is_zero() {
            return 0;
        }

        let active_elapsed = elapsed.saturating_sub(self.delay);
        let iteration = (active_elapsed.as_secs_f64() / self.duration.as_secs_f64()).floor() as u64;
        self.final_iteration()
            .map_or(iteration, |final_iteration| iteration.min(final_iteration))
    }

    pub(crate) fn elapsed_for_raw_progress(
        &self,
        progress: f32,
        reference_iteration: u64,
    ) -> Duration {
        if self.duration.is_zero() {
            return self.delay;
        }

        let iteration = self
            .final_iteration()
            .map_or(reference_iteration, |final_iteration| {
                reference_iteration.min(final_iteration)
            });
        let progress = if progress.is_nan() {
            0.0
        } else {
            progress.clamp(0.0, 1.0)
        };
        let iteration_progress = self.iteration_progress_for_raw_progress(iteration, progress);
        let active_elapsed = duration_mul_u64(self.duration, iteration).saturating_add(
            Duration::from_secs_f64(self.duration.as_secs_f64() * f64::from(iteration_progress)),
        );
        self.delay.saturating_add(active_elapsed)
    }

    fn iteration_progress_for_raw_progress(&self, iteration: u64, progress: f32) -> f32 {
        match self.direction {
            AnimationDirection::Normal => progress,
            AnimationDirection::Reverse => 1.0 - progress,
            AnimationDirection::Alternate => {
                if iteration % 2 == 0 {
                    progress
                } else {
                    1.0 - progress
                }
            }
            AnimationDirection::AlternateReverse => {
                if iteration % 2 == 0 {
                    1.0 - progress
                } else {
                    progress
                }
            }
        }
    }

    fn final_iteration(&self) -> Option<u64> {
        match self.repeat {
            RepeatMode::Once => Some(0),
            RepeatMode::Count(count) => Some(u64::from(count)),
            RepeatMode::Forever => None,
        }
    }

    /// Total elapsed time needed for a finite animation to complete, including
    /// delay and counted repeats. Returns `None` for infinite animations.
    pub fn finite_total_duration(&self) -> Option<Duration> {
        let iterations = match self.repeat {
            RepeatMode::Once => 1,
            RepeatMode::Count(count) => count.saturating_add(1),
            RepeatMode::Forever => return None,
        };
        Some(
            self.delay
                .saturating_add(duration_mul(self.duration, iterations)),
        )
    }

    pub(crate) fn to_style_spec(&self) -> TransitionSpec {
        TransitionSpec {
            duration: self.duration,
            delay: self.delay,
            repeat: self.repeat,
            direction: self.direction,
            fill_mode: self.fill_mode,
            easing: self.easing.to_style_easing(),
            driver: self.driver,
        }
    }
}

impl Default for AnimationSpec {
    fn default() -> Self {
        Self::new(Duration::ZERO)
    }
}

/// Serializable timing metadata stored in styles.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct TransitionSpec {
    /// Active duration of one iteration.
    pub duration: Duration,
    /// Delay before the active interval begins.
    pub delay: Duration,
    /// Repeat behavior.
    pub repeat: RepeatMode,
    /// Direction behavior per iteration.
    pub direction: AnimationDirection,
    /// Fill behavior before/after the active interval.
    pub fill_mode: FillMode,
    /// Easing metadata.
    pub easing: super::easing::TransitionEasing,
    /// Preferred driver.
    pub driver: AnimationDriver,
}

impl Default for TransitionSpec {
    fn default() -> Self {
        AnimationSpec::default().to_style_spec()
    }
}

impl From<TransitionSpec> for AnimationSpec {
    fn from(spec: TransitionSpec) -> Self {
        let requires_cpu_driver = spec.easing.requires_cpu_driver();
        Self {
            duration: spec.duration,
            delay: spec.delay,
            repeat: spec.repeat,
            direction: spec.direction,
            fill_mode: spec.fill_mode,
            easing: spec.easing.into(),
            driver: if requires_cpu_driver && !matches!(spec.driver, AnimationDriver::Layout) {
                AnimationDriver::Paint
            } else {
                spec.driver
            },
        }
    }
}

/// A sampled point on a timeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimelineSample {
    /// Direction-adjusted progress before easing.
    pub raw_progress: f32,
    /// Direction-adjusted progress after easing.
    pub eased_progress: f32,
    /// True after a finite animation has completed.
    pub done: bool,
}

/// A sequence of animation specs sampled one after another.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnimationSequence {
    specs: Vec<AnimationSpec>,
}

impl AnimationSequence {
    /// Create a sequence from the supplied specs.
    pub fn new(specs: Vec<AnimationSpec>) -> Self {
        Self { specs }
    }

    /// Returns the specs in sequence order.
    pub fn specs(&self) -> &[AnimationSpec] {
        &self.specs
    }

    /// Total elapsed time for a finite sequence. Returns `None` when any
    /// segment repeats forever.
    pub fn finite_total_duration(&self) -> Option<Duration> {
        self.specs
            .iter()
            .try_fold(Duration::ZERO, |duration, spec| {
                Some(duration.saturating_add(spec.finite_total_duration()?))
            })
    }

    /// Sample the active segment at `elapsed`.
    pub fn sample_elapsed(&self, elapsed: Duration) -> SequencedTimelineSample {
        let Some((last_index, last_spec)) = self.specs.iter().enumerate().last() else {
            return SequencedTimelineSample {
                animation_index: 0,
                sample: completed_sample(),
                done: true,
            };
        };

        let mut segment_elapsed = elapsed;
        for (animation_index, spec) in self.specs.iter().enumerate() {
            let Some(segment_duration) = spec.finite_total_duration() else {
                return SequencedTimelineSample {
                    animation_index,
                    sample: spec.sample_elapsed(segment_elapsed),
                    done: false,
                };
            };

            if segment_elapsed < segment_duration || animation_index == last_index {
                let sample = spec.sample_elapsed(segment_elapsed);
                return SequencedTimelineSample {
                    animation_index,
                    done: animation_index == last_index && sample.done,
                    sample,
                };
            }

            segment_elapsed = segment_elapsed.saturating_sub(segment_duration);
        }

        SequencedTimelineSample {
            animation_index: last_index,
            sample: last_spec
                .sample_elapsed(last_spec.finite_total_duration().unwrap_or(Duration::ZERO)),
            done: true,
        }
    }
}

/// Sampled state for an [`AnimationSequence`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SequencedTimelineSample {
    /// Active animation index.
    pub animation_index: usize,
    /// Sample for the active animation.
    pub sample: TimelineSample,
    /// True when the full sequence is complete.
    pub done: bool,
}

/// A set of animation specs sampled at the same elapsed time.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnimationParallel {
    specs: Vec<AnimationSpec>,
}

impl AnimationParallel {
    /// Create a parallel group from the supplied specs.
    pub fn new(specs: Vec<AnimationSpec>) -> Self {
        Self { specs }
    }

    /// Returns the specs in group order.
    pub fn specs(&self) -> &[AnimationSpec] {
        &self.specs
    }

    /// Total elapsed time for a finite parallel group. Returns `None` when any
    /// child repeats forever.
    pub fn finite_total_duration(&self) -> Option<Duration> {
        let mut total = Duration::ZERO;
        for spec in &self.specs {
            total = total.max(spec.finite_total_duration()?);
        }
        Some(total)
    }

    /// Sample every child at `elapsed`.
    pub fn sample_elapsed(&self, elapsed: Duration) -> ParallelTimelineSample {
        let samples = self
            .specs
            .iter()
            .map(|spec| spec.sample_elapsed(elapsed))
            .collect::<Vec<_>>();
        let done = samples.iter().all(|sample| sample.done);
        ParallelTimelineSample { samples, done }
    }
}

/// Sampled state for an [`AnimationParallel`].
#[derive(Clone, Debug, PartialEq)]
pub struct ParallelTimelineSample {
    /// Samples in the same order as the input specs.
    pub samples: Vec<TimelineSample>,
    /// True when every child animation is complete.
    pub done: bool,
}

/// Repeats one animation spec across `count` targets with a fixed delay offset.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationStagger {
    spec: AnimationSpec,
    count: usize,
    interval: Duration,
}

impl AnimationStagger {
    /// Create a staggered group from one spec, target count, and interval.
    pub fn new(spec: AnimationSpec, count: usize, interval: Duration) -> Self {
        Self {
            spec,
            count,
            interval,
        }
    }

    /// The child animation spec applied to each target.
    pub fn spec(&self) -> &AnimationSpec {
        &self.spec
    }

    /// Number of staggered targets.
    pub fn count(&self) -> usize {
        self.count
    }

    /// Delay between adjacent target starts.
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Total elapsed time for a finite stagger. Returns `None` when the child
    /// animation repeats forever.
    pub fn finite_total_duration(&self) -> Option<Duration> {
        if self.count == 0 {
            return Some(Duration::ZERO);
        }
        let last_offset = duration_mul(self.interval, usize_to_u32_saturating(self.count - 1));
        Some(last_offset.saturating_add(self.spec.finite_total_duration()?))
    }

    /// Sample every staggered target at `elapsed`.
    pub fn sample_elapsed(&self, elapsed: Duration) -> StaggerTimelineSample {
        let samples = (0..self.count)
            .map(|index| {
                let offset = duration_mul(self.interval, usize_to_u32_saturating(index));
                if elapsed < offset {
                    self.spec.sample_elapsed(Duration::ZERO)
                } else {
                    self.spec.sample_elapsed(elapsed.saturating_sub(offset))
                }
            })
            .collect::<Vec<_>>();
        let done = samples.iter().all(|sample| sample.done);
        StaggerTimelineSample { samples, done }
    }
}

/// Sampled state for an [`AnimationStagger`].
#[derive(Clone, Debug, PartialEq)]
pub struct StaggerTimelineSample {
    /// Samples in target order.
    pub samples: Vec<TimelineSample>,
    /// True when every target animation is complete.
    pub done: bool,
}

fn completed_sample() -> TimelineSample {
    TimelineSample {
        raw_progress: 1.0,
        eased_progress: 1.0,
        done: true,
    }
}

fn duration_mul(duration: Duration, multiplier: u32) -> Duration {
    duration.checked_mul(multiplier).unwrap_or(Duration::MAX)
}

fn duration_mul_u64(duration: Duration, multiplier: u64) -> Duration {
    duration
        .checked_mul(u32::try_from(multiplier).unwrap_or(u32::MAX))
        .unwrap_or(Duration::MAX)
}

fn usize_to_u32_saturating(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// Compatibility timeline for closure-based element animations.
#[derive(Clone, Debug)]
pub struct LegacyAnimationTimeline {
    /// Start instant for the active animation in a chain.
    pub started_at: Instant,
    /// Active animation index in a chain.
    pub animation_index: usize,
}

impl LegacyAnimationTimeline {
    /// Create a new legacy timeline starting now.
    pub fn new(now: Instant) -> Self {
        Self {
            started_at: now,
            animation_index: 0,
        }
    }

    /// Sample a chain of legacy specs, preserving one-shot/repeat semantics.
    pub fn sample(&mut self, specs: &[LegacyAnimationSpec], now: Instant) -> LegacyAnimationSample {
        let sample = self.sample_raw_with(specs.len(), now, |animation_index| {
            specs[animation_index].timing()
        });
        let progress = specs
            .get(sample.animation_index)
            .map_or(1.0, |spec| spec.easing.sample_bounded(sample.raw_progress));
        LegacyAnimationSample {
            animation_index: sample.animation_index,
            progress,
            done: sample.done,
        }
    }

    /// Sample a chain of legacy specs without requiring callers to allocate a
    /// temporary spec collection.
    pub fn sample_with(
        &mut self,
        animation_count: usize,
        now: Instant,
        mut animation_at: impl FnMut(usize) -> LegacyAnimationSpec,
    ) -> LegacyAnimationSample {
        let mut easing = None;
        let sample = self.sample_raw_with(animation_count, now, |animation_index| {
            let spec = animation_at(animation_index);
            let timing = spec.timing();
            easing = Some(spec.easing);
            timing
        });
        let progress = easing
            .as_ref()
            .map_or(1.0, |easing| easing.sample_bounded(sample.raw_progress));
        LegacyAnimationSample {
            animation_index: sample.animation_index,
            progress,
            done: sample.done,
        }
    }

    /// Sample a chain of legacy timing data and return raw progress. Callers
    /// that already own easing closures can apply easing without creating a
    /// temporary [`Easing::Custom`] wrapper every frame.
    pub fn sample_raw_with(
        &mut self,
        animation_count: usize,
        now: Instant,
        mut timing_at: impl FnMut(usize) -> LegacyAnimationTiming,
    ) -> LegacyAnimationRawSample {
        if self.animation_index >= animation_count {
            return LegacyAnimationRawSample {
                animation_index: self.animation_index,
                raw_progress: 1.0,
                done: true,
            };
        }

        let timing = timing_at(self.animation_index);
        let elapsed = now.saturating_duration_since(self.started_at);
        if timing.duration.is_zero() {
            if timing.oneshot {
                self.animation_index += 1;
                self.started_at = now;
                let done = self.animation_index >= animation_count;
                return LegacyAnimationRawSample {
                    animation_index: self.animation_index.saturating_sub(1),
                    raw_progress: 1.0,
                    done,
                };
            }

            return LegacyAnimationRawSample {
                animation_index: self.animation_index,
                raw_progress: 1.0,
                done: true,
            };
        }

        let raw_progress = elapsed.as_secs_f32() / timing.duration.as_secs_f32();
        if raw_progress >= 1.0 && timing.oneshot {
            self.animation_index += 1;
            self.started_at = now;
            let done = self.animation_index >= animation_count;
            LegacyAnimationRawSample {
                animation_index: self.animation_index.saturating_sub(1),
                raw_progress: 1.0,
                done,
            }
        } else {
            let raw_progress = if timing.oneshot {
                raw_progress.clamp(0.0, 1.0)
            } else {
                raw_progress.rem_euclid(1.0)
            };
            LegacyAnimationRawSample {
                animation_index: self.animation_index,
                raw_progress,
                done: false,
            }
        }
    }
}

/// Timing data used by compatibility wrappers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LegacyAnimationTiming {
    /// Duration of the animation.
    pub duration: Duration,
    /// True when the animation should advance after one iteration.
    pub oneshot: bool,
}

/// Raw sampled state for compatibility wrappers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LegacyAnimationRawSample {
    /// Active animation index.
    pub animation_index: usize,
    /// Uneased progress.
    pub raw_progress: f32,
    /// True when the full chain is complete.
    pub done: bool,
}

/// Timing data used by compatibility wrappers.
#[derive(Clone)]
pub struct LegacyAnimationSpec {
    /// Duration of the animation.
    pub duration: Duration,
    /// True when the animation should advance after one iteration.
    pub oneshot: bool,
    /// Easing curve.
    pub easing: Easing,
}

impl LegacyAnimationSpec {
    pub(crate) fn timing(&self) -> LegacyAnimationTiming {
        LegacyAnimationTiming {
            duration: self.duration,
            oneshot: self.oneshot,
        }
    }
}

/// Sampled state for compatibility wrappers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LegacyAnimationSample {
    /// Active animation index.
    pub animation_index: usize,
    /// Eased progress.
    pub progress: f32,
    /// True when the full chain is complete.
    pub done: bool,
}
