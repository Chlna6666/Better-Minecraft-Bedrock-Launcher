use super::*;

pub(super) fn sample_element_animation(
    timeline: &mut LegacyAnimationTimeline,
    animations: &[Animation],
    now: Instant,
) -> (usize, f32, bool) {
    if let Some(animation) = animations.get(timeline.animation_index)
        && let Some(spring) = animation.spring
    {
        let sample = spring.sample_with_velocity(
            now.saturating_duration_since(timeline.started_at)
                .as_secs_f32(),
            0.0,
        );
        let index = timeline.animation_index;
        if sample.done && (!animation.oneshot || index + 1 < animations.len()) {
            timeline.started_at = now;
            if animation.oneshot {
                timeline.animation_index += 1;
            }
            return (index, 1.0, false);
        }
        return (
            index,
            if sample.done { 1.0 } else { sample.progress },
            sample.done,
        );
    }
    let sample = timeline.sample_raw_with(animations.len(), now, |index| {
        let animation = &animations[index];
        LegacyAnimationTiming {
            duration: animation.duration,
            oneshot: animation.oneshot,
        }
    });
    let progress = animations
        .get(sample.animation_index)
        .map_or(1.0, |animation| {
            sample_legacy_easing(animation.easing.as_ref(), sample.raw_progress)
        });
    (sample.animation_index, progress, sample.done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_spring_keeps_overshoot_and_runs_until_settled() {
        let start = Instant::now();
        let spring = crate::Spring::default();
        let animations = [Animation::spring(spring)];
        let mut timeline = LegacyAnimationTimeline::new(start);
        for milliseconds in [0, 100, 200, 520, 800, 2000, 3500] {
            let elapsed = Duration::from_millis(milliseconds);
            let expected = spring.sample_with_velocity(elapsed.as_secs_f32(), 0.0);
            let (_, progress, done) =
                sample_element_animation(&mut timeline, &animations, start + elapsed);
            assert_eq!(done, expected.done);
            assert_eq!(
                progress,
                if expected.done {
                    1.0
                } else {
                    expected.progress
                }
            );
        }
        assert!(
            sample_element_animation(
                &mut timeline,
                &animations,
                start + Duration::from_millis(3500)
            )
            .1 > 1.0
        );
        assert!(
            sample_element_animation(&mut timeline, &animations, start + Duration::from_secs(30)).2
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
        let mut timeline = LegacyAnimationTimeline::new(start);
        assert_eq!(
            sample_element_animation(
                &mut timeline,
                &animations,
                start + Duration::from_millis(100)
            ),
            (0, 1.0, false)
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
        assert_eq!((index, progress, done), (1, 1.0, false));
        assert_eq!(
            sample_element_animation(
                &mut timeline,
                &animations,
                start + Duration::from_millis(30200)
            ),
            (2, 1.0, true)
        );
    }
}
