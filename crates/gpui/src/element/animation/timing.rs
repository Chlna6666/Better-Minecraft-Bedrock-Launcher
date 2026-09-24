use super::*;

pub(super) struct ElementAnimationTimeline {
    animation_index: usize,
    started_at: Instant,
}

impl ElementAnimationTimeline {
    pub(super) fn new(started_at: Instant) -> Self {
        Self {
            animation_index: 0,
            started_at,
        }
    }
}

pub(super) fn sample_element_animation(
    timeline: &mut ElementAnimationTimeline,
    animations: &[Animation],
    now: Instant,
) -> (usize, Option<f32>, bool) {
    loop {
        let Some(animation) = animations.get(timeline.animation_index) else {
            return (timeline.animation_index.saturating_sub(1), None, true);
        };
        let elapsed = now.saturating_duration_since(timeline.started_at);
        if let Some(spring) = animation.spring {
            let index = timeline.animation_index;
            if elapsed < animation.spec.delay {
                return (
                    index,
                    animation
                        .spec
                        .fill_mode
                        .fills_backwards()
                        .then_some(0.0),
                    false,
                );
            }

            let active_elapsed = elapsed.saturating_sub(animation.spec.delay);
            let sample = spring.sample_with_velocity(active_elapsed.as_secs_f32(), 0.0);
            let repeats = matches!(animation.spec.repeat, RepeatMode::Forever);
            if sample.done && !repeats && index + 1 < animations.len() {
                timeline.animation_index += 1;
                timeline.started_at = now;
                continue;
            }
            if sample.done && repeats {
                timeline.started_at = now;
                return (index, Some(1.0), false);
            }

            let applies = !sample.done || animation.spec.fill_mode.fills_forwards();
            return (
                index,
                applies.then_some(if sample.done { 1.0 } else { sample.progress }),
                sample.done,
            );
        }

        let sample = animation.spec.sample_elapsed(elapsed);
        if sample.done && timeline.animation_index + 1 < animations.len() {
            let Some(segment_duration) = animation.spec.finite_total_duration() else {
                return (
                    timeline.animation_index,
                    sample.applies.then_some(sample.eased_progress),
                    sample.done,
                );
            };
            let remainder = elapsed.saturating_sub(segment_duration);
            timeline.animation_index += 1;
            timeline.started_at = now.checked_sub(remainder).unwrap_or(now);
            continue;
        }
        return (
            timeline.animation_index,
            sample.applies.then_some(sample.eased_progress),
            sample.done,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_spring_keeps_overshoot_and_runs_until_settled() {
        let start = Instant::now();
        let spring = crate::Spring::default();
        let animations = [Animation::spring(spring)];
        let mut timeline = ElementAnimationTimeline::new(start);
        for milliseconds in [0, 100, 200, 520, 800, 2000, 3500] {
            let elapsed = Duration::from_millis(milliseconds);
            let expected = spring.sample_with_velocity(elapsed.as_secs_f32(), 0.0);
            let (_, progress, done) =
                sample_element_animation(&mut timeline, &animations, start + elapsed);
            assert_eq!(done, expected.done);
            assert_eq!(
                progress,
                Some(if expected.done {
                    1.0
                } else {
                    expected.progress
                })
            );
        }
        assert!(
            sample_element_animation(
                &mut timeline,
                &animations,
                start + Duration::from_millis(3500)
            )
            .1
            .is_some_and(|progress| progress > 1.0)
        );
        assert!(
            sample_element_animation(&mut timeline, &animations, start + Duration::from_secs(30)).2
        );
    }

    #[test]
    fn physical_spring_respects_delay_and_backwards_fill() {
        let start = Instant::now();
        let spring = crate::Spring::default();
        let animations = [Animation::spring(spring)
            .delay(Duration::from_millis(60))
            .fill_mode(crate::FillMode::Both)];
        let mut timeline = ElementAnimationTimeline::new(start);

        assert_eq!(
            sample_element_animation(
                &mut timeline,
                &animations,
                start + Duration::from_millis(30)
            ),
            (0, Some(0.0), false)
        );

        let active_elapsed = Duration::from_millis(90);
        let expected = spring.sample_with_velocity(active_elapsed.as_secs_f32(), 0.0);
        let (_, progress, done) = sample_element_animation(
            &mut timeline,
            &animations,
            start + Duration::from_millis(150),
        );
        assert_eq!(done, expected.done);
        assert_eq!(
            progress,
            Some(if expected.done {
                1.0
            } else {
                expected.progress
            })
        );
    }

    #[test]
    fn physical_spring_can_follow_and_precede_duration_animation() {
        let start = Instant::now();
        let animations = [
            Animation::new(Duration::from_millis(100)),
            Animation::spring(crate::Spring::default()),
            Animation::new(Duration::from_millis(100)),
        ];
        let mut timeline = ElementAnimationTimeline::new(start);
        assert_eq!(
            sample_element_animation(
                &mut timeline,
                &animations,
                start + Duration::from_millis(100)
            ),
            (1, Some(0.0), false)
        );
        let (index, _, done) = sample_element_animation(
            &mut timeline,
            &animations,
            start + Duration::from_millis(150),
        );
        assert_eq!(index, 1);
        assert!(!done);
        let (index, progress, done) =
            sample_element_animation(&mut timeline, &animations, start + Duration::from_secs(30));
        assert_eq!((index, progress, done), (2, Some(0.0), false));
        assert_eq!(
            sample_element_animation(
                &mut timeline,
                &animations,
                start + Duration::from_millis(30200)
            ),
            (2, Some(1.0), true)
        );
    }

    #[test]
    fn element_animation_preserves_spec_timing() {
        let start = Instant::now();

        let delayed = [Animation::from_spec(
            AnimationSpec::new(Duration::from_millis(100)).delay(Duration::from_millis(50)),
        )];
        let mut timeline = ElementAnimationTimeline::new(start);
        assert_eq!(
            sample_element_animation(&mut timeline, &delayed, start + Duration::from_millis(25)),
            (0, None, false)
        );

        let reversed = [Animation::from_spec(
            AnimationSpec::new(Duration::from_millis(100))
                .direction(crate::AnimationDirection::Reverse),
        )];
        let mut timeline = ElementAnimationTimeline::new(start);
        assert_eq!(
            sample_element_animation(&mut timeline, &reversed, start + Duration::from_millis(25)),
            (0, Some(0.75), false)
        );

        let repeated = [Animation::from_spec(
            AnimationSpec::new(Duration::from_millis(100)).repeat(RepeatMode::Count(2)),
        )];
        let mut timeline = ElementAnimationTimeline::new(start);
        assert_eq!(
            sample_element_animation(&mut timeline, &repeated, start + Duration::from_millis(250)),
            (0, Some(0.5), false)
        );
    }

    #[test]
    fn element_animation_chain_preserves_long_frame_remainder() {
        let start = Instant::now();
        let animations = [
            Animation::new(Duration::from_millis(100)),
            Animation::new(Duration::from_millis(100)),
        ];
        let mut timeline = ElementAnimationTimeline::new(start);

        assert_eq!(
            sample_element_animation(
                &mut timeline,
                &animations,
                start + Duration::from_millis(150)
            ),
            (1, Some(0.5), false)
        );
    }
}
