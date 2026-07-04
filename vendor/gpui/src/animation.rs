//! Animation primitives shared by GPUI elements, styles, and window scheduling.

mod animatable;
mod easing;
mod keyframe;
mod manager;
mod physics;
mod scheduler;
mod spring;
mod timeline;
mod transition;
mod tween;

pub use animatable::Animatable;
pub use easing::{Easing, StepPosition, TransitionEasing};
pub use keyframe::{Keyframe, KeyframeTrack};
pub use manager::{AnimationEngine, AnimationGroupId, AnimationGroupSample, AnimationTick};
pub use physics::{PhysicsConfig, SpringMotion};
pub use scheduler::AnimationDriver;
pub use spring::{Spring, SpringSample};
pub use timeline::{
    AnimationDirection, AnimationParallel, AnimationSequence, AnimationSpec, AnimationStagger,
    FillMode, LegacyAnimationRawSample, LegacyAnimationSample, LegacyAnimationSpec,
    LegacyAnimationTimeline, LegacyAnimationTiming, ParallelTimelineSample, RepeatMode,
    SequencedTimelineSample, StaggerTimelineSample, TimelineSample, TransitionSpec,
};
pub use transition::{Transition, TransitionProperty, TransitionStyle};
pub use tween::Tween;

pub(crate) use easing::sample_legacy_easing_bounded;
pub(crate) use scheduler::merge_requested_drivers;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ElementId, GlobalElementId, Radians, TransformationMatrix, bounds, hsla, point, px, size,
    };
    use smallvec::smallvec;
    use std::{rc::Rc, time::Duration, time::Instant};

    fn test_global_element_id(name: &'static str) -> GlobalElementId {
        GlobalElementId(smallvec![ElementId::from(name)])
    }

    #[test]
    fn easing_clamps_input_and_preserves_finite_output() {
        assert_eq!(Easing::Linear.sample(-1.0), 0.0);
        assert_eq!(Easing::Linear.sample(2.0), 1.0);
        assert!(Easing::OutBack.sample(0.8) > 1.0);
        assert!(
            (Easing::CubicBezier {
                x1: 0.4,
                y1: 0.0,
                x2: 0.2,
                y2: 1.0,
            }
            .sample(0.5)
                - 0.775)
                .abs()
                < 0.02
        );
        assert_eq!(
            Easing::Steps {
                count: 4,
                position: StepPosition::End,
            }
            .sample(0.74),
            0.5
        );
        assert_eq!(
            Easing::Steps {
                count: 4,
                position: StepPosition::Start,
            }
            .sample(0.0),
            0.25
        );
        assert_eq!(
            Easing::Steps {
                count: 4,
                position: StepPosition::Start,
            }
            .sample(1.0),
            1.0
        );
        assert_eq!(Easing::Custom(Rc::new(|_| 3.0)).sample(0.5), 3.0);
        assert_eq!(Easing::Custom(Rc::new(|_| f32::NAN)).sample(0.5), 0.5);
    }

    #[test]
    fn zero_duration_finishes_finite_animation() {
        let sample = AnimationSpec::new(Duration::ZERO).sample_elapsed(Duration::ZERO);
        assert_eq!(sample.raw_progress, 1.0);
        assert!(sample.done);
    }

    #[test]
    fn animation_spec_reports_finite_total_duration() {
        assert_eq!(
            AnimationSpec::new(Duration::from_millis(100))
                .delay(Duration::from_millis(25))
                .repeat(RepeatMode::Count(2))
                .finite_total_duration(),
            Some(Duration::from_millis(325))
        );
        assert_eq!(
            AnimationSpec::new(Duration::from_millis(100))
                .repeat(RepeatMode::Forever)
                .finite_total_duration(),
            None
        );
    }

    #[test]
    fn animation_sequence_samples_active_segment() {
        let sequence = AnimationSequence::new(vec![
            AnimationSpec::new(Duration::from_millis(100)),
            AnimationSpec::new(Duration::from_millis(200)),
        ]);

        assert_eq!(
            sequence.finite_total_duration(),
            Some(Duration::from_millis(300))
        );
        let sample = sequence.sample_elapsed(Duration::from_millis(150));
        assert_eq!(sample.animation_index, 1);
        assert!((sample.sample.raw_progress - 0.25).abs() < 0.001);
        assert!(!sample.done);

        let sample = sequence.sample_elapsed(Duration::from_millis(350));
        assert_eq!(sample.animation_index, 1);
        assert_eq!(sample.sample.raw_progress, 1.0);
        assert!(sample.done);
    }

    #[test]
    fn animation_parallel_finishes_when_all_children_finish() {
        let parallel = AnimationParallel::new(vec![
            AnimationSpec::new(Duration::from_millis(100)),
            AnimationSpec::new(Duration::from_millis(200)),
        ]);

        assert_eq!(
            parallel.finite_total_duration(),
            Some(Duration::from_millis(200))
        );
        let sample = parallel.sample_elapsed(Duration::from_millis(150));
        assert_eq!(sample.samples.len(), 2);
        assert!(sample.samples[0].done);
        assert!(!sample.samples[1].done);
        assert!(!sample.done);

        assert!(parallel.sample_elapsed(Duration::from_millis(250)).done);
    }

    #[test]
    fn animation_stagger_offsets_each_child() {
        let stagger = AnimationStagger::new(
            AnimationSpec::new(Duration::from_millis(100)),
            3,
            Duration::from_millis(50),
        );

        assert_eq!(
            stagger.finite_total_duration(),
            Some(Duration::from_millis(200))
        );
        let sample = stagger.sample_elapsed(Duration::from_millis(75));
        assert_eq!(sample.samples.len(), 3);
        assert!((sample.samples[0].raw_progress - 0.75).abs() < 0.001);
        assert!((sample.samples[1].raw_progress - 0.25).abs() < 0.001);
        assert_eq!(sample.samples[2].raw_progress, 0.0);
        assert!(!sample.done);
    }

    #[test]
    fn zero_duration_forever_animation_finishes_without_frame_loop() {
        let sample = AnimationSpec::new(Duration::ZERO)
            .repeat(RepeatMode::Forever)
            .sample_elapsed(Duration::ZERO);
        assert_eq!(sample.raw_progress, 1.0);
        assert!(sample.done);
    }

    #[test]
    fn delay_holds_backwards_fill_at_start() {
        let sample = AnimationSpec::new(Duration::from_millis(100))
            .delay(Duration::from_millis(50))
            .fill_mode(FillMode::Backwards)
            .sample_elapsed(Duration::from_millis(25));
        assert_eq!(sample.raw_progress, 0.0);
        assert!(!sample.done);
    }

    #[test]
    fn alternate_direction_reverses_odd_iterations() {
        let sample = AnimationSpec::new(Duration::from_millis(100))
            .repeat(RepeatMode::Forever)
            .direction(AnimationDirection::Alternate)
            .sample_elapsed(Duration::from_millis(125));
        assert!((sample.raw_progress - 0.75).abs() < 0.001);
    }

    #[test]
    fn elapsed_for_raw_progress_preserves_alternate_iteration_direction() {
        let spec = AnimationSpec::new(Duration::from_millis(100))
            .repeat(RepeatMode::Forever)
            .direction(AnimationDirection::Alternate);
        let elapsed = Duration::from_millis(125);
        let sample = spec.sample_elapsed(elapsed);
        let iteration = spec.active_iteration_at_elapsed(elapsed);
        let resumed_elapsed = spec.elapsed_for_raw_progress(sample.raw_progress, iteration);

        assert_eq!(iteration, 1);
        assert!((resumed_elapsed.as_secs_f32() - elapsed.as_secs_f32()).abs() < 0.001);
        assert!((spec.elapsed_for_raw_progress(0.3, 1).as_secs_f32() - 0.17).abs() < 0.001);

        let spec = AnimationSpec::new(Duration::from_millis(100))
            .repeat(RepeatMode::Forever)
            .direction(AnimationDirection::AlternateReverse);
        let elapsed = Duration::from_millis(125);
        let sample = spec.sample_elapsed(elapsed);
        let iteration = spec.active_iteration_at_elapsed(elapsed);
        let resumed_elapsed = spec.elapsed_for_raw_progress(sample.raw_progress, iteration);

        assert_eq!(iteration, 1);
        assert!((resumed_elapsed.as_secs_f32() - elapsed.as_secs_f32()).abs() < 0.001);
    }

    #[test]
    fn interpolation_handles_core_values() {
        assert_eq!(f32::interpolate(&0.0, &10.0, 0.5), 5.0);
        assert_eq!(f32::interpolate(&0.0, &10.0, 1.2), 12.0);
        assert_eq!(
            crate::Pixels::interpolate(&px(0.0), &px(10.0), 0.5),
            px(5.0)
        );
        assert_eq!(
            crate::Point::<crate::Pixels>::interpolate(
                &point(px(0.0), px(0.0)),
                &point(px(4.0), px(8.0)),
                0.5
            ),
            point(px(2.0), px(4.0))
        );
        assert_eq!(
            crate::Size::<crate::Pixels>::interpolate(
                &size(px(0.0), px(0.0)),
                &size(px(4.0), px(8.0)),
                0.5
            ),
            size(px(2.0), px(4.0))
        );
        assert_eq!(
            crate::Hsla::interpolate(&hsla(0.0, 0.0, 0.0, 0.0), &hsla(1.0, 1.0, 1.0, 1.0), 0.5),
            hsla(0.0, 0.5, 0.5, 0.5)
        );
    }

    #[test]
    fn hsla_hue_interpolates_shortest_arc() {
        let color =
            crate::Hsla::interpolate(&hsla(0.9, 1.0, 0.5, 1.0), &hsla(0.1, 1.0, 0.5, 1.0), 0.5);
        assert!(color.h < 0.001 || color.h > 0.999);
    }

    #[test]
    fn vec_interpolates_equal_length_tracks() {
        assert_eq!(
            Vec::<f32>::interpolate(&vec![0.0, 10.0], &vec![10.0, 20.0], 0.5),
            vec![5.0, 15.0]
        );
        assert_eq!(
            Vec::<f32>::interpolate(&vec![0.0], &vec![10.0, 20.0], 0.5),
            vec![0.0]
        );
        assert_eq!(
            Vec::<f32>::interpolate(&vec![0.0], &vec![10.0, 20.0], 1.0),
            vec![10.0, 20.0]
        );
    }

    #[test]
    fn transformation_matrix_interpolates_rotation_without_squash() {
        let from = TransformationMatrix::unit();
        let to = TransformationMatrix::unit().rotate(Radians(std::f32::consts::FRAC_PI_2));
        let midpoint = TransformationMatrix::interpolate(&from, &to, 0.5);
        let expected = std::f32::consts::FRAC_1_SQRT_2;

        assert!((midpoint.rotation_scale[0][0] - expected).abs() < 0.001);
        assert!((midpoint.rotation_scale[0][1] + expected).abs() < 0.001);
        assert!((midpoint.rotation_scale[1][0] - expected).abs() < 0.001);
        assert!((midpoint.rotation_scale[1][1] - expected).abs() < 0.001);
    }

    #[test]
    fn transformation_matrix_tolerates_small_rotation_scale_drift() {
        let from = TransformationMatrix::unit();
        let mut to = TransformationMatrix::unit()
            .rotate(Radians(std::f32::consts::FRAC_PI_2))
            .scale(size(1.5, 0.75));
        to.rotation_scale[1][1] = 0.002;

        let midpoint = TransformationMatrix::interpolate(&from, &to, 0.5);
        let scale_x = midpoint.rotation_scale[0][0].hypot(midpoint.rotation_scale[1][0]);
        let scale_y = midpoint.rotation_scale[0][1].hypot(midpoint.rotation_scale[1][1]);
        let column_dot = midpoint.rotation_scale[0][0] * midpoint.rotation_scale[0][1]
            + midpoint.rotation_scale[1][0] * midpoint.rotation_scale[1][1];
        let normalized_dot = column_dot / (scale_x * scale_y).max(f32::MIN_POSITIVE);

        assert!(normalized_dot.abs() < 0.02);
    }

    #[test]
    fn layout_properties_force_layout_driver() {
        let transition = Transition::new(Duration::from_millis(100))
            .properties([TransitionProperty::Opacity, TransitionProperty::Width])
            .driver(AnimationDriver::Auto);
        assert_eq!(transition.resolved_driver(), AnimationDriver::Layout);
    }

    #[test]
    fn visual_properties_use_gpu_for_auto_driver() {
        let transition = Transition::new(Duration::from_millis(100))
            .properties([TransitionProperty::Opacity, TransitionProperty::Transform])
            .driver(AnimationDriver::Auto);
        assert_eq!(transition.resolved_driver(), AnimationDriver::Gpu);
    }

    #[test]
    fn paint_only_visual_properties_fallback_to_paint_driver() {
        let transition = Transition::new(Duration::from_millis(100))
            .properties([TransitionProperty::Opacity, TransitionProperty::Color])
            .driver(AnimationDriver::Auto);
        assert_eq!(transition.resolved_driver(), AnimationDriver::Paint);

        let transition = Transition::new(Duration::from_millis(100))
            .properties([TransitionProperty::Color])
            .driver(AnimationDriver::Gpu);
        assert_eq!(transition.resolved_driver(), AnimationDriver::Paint);
    }

    #[test]
    fn custom_easing_fallbacks_to_paint_for_visual_properties() {
        let transition = Transition::new(Duration::from_millis(100))
            .ease(Easing::Custom(Rc::new(|progress| progress)))
            .properties([TransitionProperty::Opacity])
            .driver(AnimationDriver::Auto);
        assert_eq!(transition.resolved_driver(), AnimationDriver::Paint);

        let style = transition.into_style();
        assert_eq!(style.spec.driver, AnimationDriver::Paint);
        assert_eq!(style.resolved_driver(), AnimationDriver::Paint);

        let restored = Transition::from(style);
        assert_eq!(restored.spec.driver, AnimationDriver::Paint);
        assert_eq!(restored.resolved_driver(), AnimationDriver::Paint);
    }

    #[test]
    fn custom_easing_keeps_layout_driver_for_layout_properties() {
        let transition = Transition::new(Duration::from_millis(100))
            .ease(Easing::Custom(Rc::new(|progress| progress)))
            .properties([TransitionProperty::Width])
            .driver(AnimationDriver::Auto);
        assert_eq!(transition.resolved_driver(), AnimationDriver::Layout);
    }

    #[test]
    fn spring_easing_integrates_with_transition_driver() {
        let spring = Spring::default();
        assert_eq!(Easing::Spring(spring).sample(0.25), spring.sample(0.25));
        assert!(Easing::OutElastic.sample(0.5) > 1.0);

        let transition = Transition::new(Duration::from_millis(100))
            .ease(Easing::Spring(spring))
            .properties([TransitionProperty::Opacity])
            .driver(AnimationDriver::Auto);
        assert_eq!(transition.resolved_driver(), AnimationDriver::Paint);

        let style = transition.into_style();
        assert_eq!(style.spec.easing, TransitionEasing::Spring(spring));
        assert_eq!(style.resolved_driver(), AnimationDriver::Paint);
    }

    #[test]
    fn engine_tick_reports_only_remaining_drivers() {
        let now = Instant::now();
        let mut engine = AnimationEngine::new();
        let layout_element = test_global_element_id("layout-finished");
        let paint_element = test_global_element_id("paint-active");

        engine.start_transition(
            &layout_element,
            TransitionProperty::Width,
            AnimationSpec::new(Duration::ZERO).driver(AnimationDriver::Layout),
            now,
        );
        engine.start_transition(
            &paint_element,
            TransitionProperty::Opacity,
            AnimationSpec::new(Duration::from_millis(100))
                .repeat(RepeatMode::Forever)
                .driver(AnimationDriver::Paint),
            now,
        );

        let tick = engine.tick(now);
        assert_eq!(tick.active_count, 1);
        assert!(tick.has_gpu_or_paint);
        assert!(tick.has_layout);
    }

    #[test]
    fn engine_visual_tick_does_not_sample_layout_timelines() {
        let now = Instant::now();
        let mut engine = AnimationEngine::new();
        let layout_element = test_global_element_id("layout-active");
        let paint_element = test_global_element_id("paint-active");

        engine.start_transition(
            &layout_element,
            TransitionProperty::Width,
            AnimationSpec::new(Duration::ZERO).driver(AnimationDriver::Layout),
            now,
        );
        engine.start_transition(
            &paint_element,
            TransitionProperty::Opacity,
            AnimationSpec::new(Duration::from_millis(100))
                .repeat(RepeatMode::Forever)
                .driver(AnimationDriver::Paint),
            now,
        );

        let tick = engine.tick_driver(AnimationDriver::Paint, now);
        assert_eq!(tick.active_count, 2);
        assert!(tick.has_gpu_or_paint);
        assert!(!tick.has_layout);
        assert!(
            engine
                .sample_transition(&layout_element, TransitionProperty::Width, now)
                .is_some()
        );

        let tick = engine.tick_driver(AnimationDriver::Layout, now);
        assert_eq!(tick.active_count, 1);
        assert!(!tick.has_gpu_or_paint);
        assert!(tick.has_layout);
    }

    #[test]
    fn animation_driver_requests_merge_without_dropping_layout_work() {
        assert_eq!(
            merge_requested_drivers(None, AnimationDriver::Auto),
            AnimationDriver::Paint
        );
        assert_eq!(
            merge_requested_drivers(Some(AnimationDriver::Paint), AnimationDriver::Gpu),
            AnimationDriver::Gpu
        );
        assert_eq!(
            merge_requested_drivers(Some(AnimationDriver::Paint), AnimationDriver::Layout),
            AnimationDriver::Auto
        );
    }

    #[test]
    fn legacy_timeline_preserves_chained_oneshot_semantics() {
        let now = Instant::now();
        let mut timeline = LegacyAnimationTimeline::new(now);
        let specs = [
            LegacyAnimationSpec {
                duration: Duration::from_millis(100),
                oneshot: true,
                easing: Easing::Linear,
            },
            LegacyAnimationSpec {
                duration: Duration::from_millis(100),
                oneshot: false,
                easing: Easing::Linear,
            },
        ];

        let sample = timeline.sample(&specs, now + Duration::from_millis(120));
        assert_eq!(sample.animation_index, 0);
        assert!(!sample.done);
        assert_eq!(timeline.animation_index, 1);

        let sample = timeline.sample(&specs, now + Duration::from_millis(170));
        assert_eq!(sample.animation_index, 1);
        assert!(!sample.done);
    }

    #[test]
    fn legacy_zero_duration_repeat_finishes_without_frame_loop() {
        let now = Instant::now();
        let mut timeline = LegacyAnimationTimeline::new(now);
        let specs = [LegacyAnimationSpec {
            duration: Duration::ZERO,
            oneshot: false,
            easing: Easing::Linear,
        }];

        let sample = timeline.sample(&specs, now);
        assert_eq!(sample.animation_index, 0);
        assert_eq!(sample.progress, 1.0);
        assert!(sample.done);
    }

    #[test]
    fn legacy_empty_chain_is_done() {
        let now = Instant::now();
        let mut timeline = LegacyAnimationTimeline::new(now);

        let sample = timeline.sample(&[], now);
        assert_eq!(sample.animation_index, 0);
        assert_eq!(sample.progress, 1.0);
        assert!(sample.done);
    }

    #[test]
    fn keyframe_track_interpolates_between_keyframes() {
        let track = KeyframeTrack::new(vec![Keyframe::new(0.0, 0.0), Keyframe::new(1.0, 10.0)]);
        assert_eq!(track.sample(0.25), Some(2.5));
    }

    #[test]
    fn keyframe_track_normalizes_offsets_and_progress() {
        let track = KeyframeTrack::new(vec![
            Keyframe {
                offset: f32::NAN,
                value: 0.0,
                easing: None,
            },
            Keyframe {
                offset: 2.0,
                value: 10.0,
                easing: None,
            },
        ]);
        assert_eq!(track.sample(f32::NAN), Some(0.0));
        assert_eq!(track.sample(2.0), Some(10.0));
    }

    #[test]
    fn keyframe_track_uses_segment_easing() {
        let track = KeyframeTrack::new(vec![
            Keyframe::new(0.0, 0.0).ease(Easing::Steps {
                count: 2,
                position: StepPosition::End,
            }),
            Keyframe::new(1.0, 10.0),
        ]);
        assert_eq!(track.sample(0.49), Some(0.0));
        assert_eq!(track.sample(0.51), Some(5.0));
    }

    #[test]
    fn tween_samples_between_values() {
        let tween = Tween::new(0.0, 10.0);
        assert_eq!(tween.sample(0.4), 4.0);
    }

    #[test]
    fn tween_preserves_overshooting_easing() {
        let tween = Tween::new(0.0, 10.0).ease(Easing::OutBack);
        assert!(tween.sample(0.8) > 10.0);
    }

    #[test]
    fn spring_progress_moves_toward_one() {
        let spring = Spring {
            physics: PhysicsConfig {
                stiffness: 60.0,
                damping: 8.0,
                mass: 1.0,
            },
            ..Spring::default()
        };
        assert!(spring.sample(0.5) > spring.sample(0.1));
        assert_eq!(spring.sample(-1.0), 0.0);
        assert_eq!(spring.sample(f32::NAN), 0.0);
        assert_ne!(
            PhysicsConfig {
                stiffness: 10.0,
                damping: 8.0,
                mass: 1.0,
            }
            .position_velocity(0.25, 0.0)
            .displacement,
            PhysicsConfig {
                stiffness: 100.0,
                damping: 8.0,
                mass: 1.0,
            }
            .position_velocity(0.25, 0.0)
            .displacement
        );
        assert_ne!(
            spring.sample_with_velocity(0.2, 0.0).progress,
            spring.sample_with_velocity(0.2, 4.0).progress
        );
    }

    #[test]
    fn animation_engine_cancels_element_by_index() {
        let now = Instant::now();
        let mut engine = AnimationEngine::new();
        let first = test_global_element_id("first");
        let second = test_global_element_id("second");

        engine.start_transition(
            &first,
            TransitionProperty::Opacity,
            AnimationSpec::new(Duration::from_millis(100)).repeat(RepeatMode::Forever),
            now,
        );
        engine.start_transition(
            &first,
            TransitionProperty::Transform,
            AnimationSpec::new(Duration::from_millis(100)).repeat(RepeatMode::Forever),
            now,
        );
        engine.start_transition(
            &second,
            TransitionProperty::Opacity,
            AnimationSpec::new(Duration::from_millis(100)).repeat(RepeatMode::Forever),
            now,
        );

        engine.cancel_element(&first);
        assert_eq!(engine.active_count(), 1);

        let tick = engine.tick(now + Duration::from_millis(16));
        assert_eq!(tick.active_count, 1);
    }

    #[test]
    fn animation_engine_replacement_continues_from_current_progress() {
        let now = Instant::now();
        let mut engine = AnimationEngine::new();
        let element = test_global_element_id("interrupted");
        let replacement_time = now + Duration::from_millis(500);

        engine.start_transition(
            &element,
            TransitionProperty::Opacity,
            AnimationSpec::new(Duration::from_millis(1000)),
            now,
        );
        engine.start_transition(
            &element,
            TransitionProperty::Opacity,
            AnimationSpec::new(Duration::from_millis(1000)),
            replacement_time,
        );

        let sample = engine
            .sample_transition(&element, TransitionProperty::Opacity, replacement_time)
            .expect("replacement timeline should exist");
        assert!((sample.raw_progress - 0.5).abs() < 0.001);
    }

    #[test]
    fn animation_engine_tick_reports_visual_dirty_bounds() {
        let now = Instant::now();
        let mut engine = AnimationEngine::new();
        let element = test_global_element_id("dirty-bounds");
        let dirty_bounds = bounds(point(px(2.0), px(4.0)), size(px(8.0), px(16.0)));

        engine.start_transition(
            &element,
            TransitionProperty::Opacity,
            AnimationSpec::new(Duration::from_millis(100))
                .repeat(RepeatMode::Forever)
                .driver(AnimationDriver::Paint),
            now,
        );
        assert!(engine.set_transition_bounds(&element, TransitionProperty::Opacity, dirty_bounds));

        let tick = engine.tick_driver(AnimationDriver::Paint, now + Duration::from_millis(16));
        assert_eq!(tick.dirty_bounds.as_slice(), &[dirty_bounds]);
        assert!(tick.has_gpu_or_paint);
    }

    #[test]
    fn animation_engine_ticks_sequence_group_with_dirty_bounds() {
        let now = Instant::now();
        let mut engine = AnimationEngine::new();
        let dirty_bounds = bounds(point(px(2.0), px(4.0)), size(px(8.0), px(16.0)));
        let group_id = engine.start_sequence(
            AnimationSequence::new(vec![
                AnimationSpec::new(Duration::from_millis(100)).driver(AnimationDriver::Paint),
                AnimationSpec::new(Duration::from_millis(100)).driver(AnimationDriver::Paint),
            ]),
            now,
        );

        assert_eq!(engine.active_count(), 1);
        assert!(engine.set_group_bounds(group_id, dirty_bounds));
        let Some(AnimationGroupSample::Sequence(sample)) =
            engine.sample_group(group_id, now + Duration::from_millis(50))
        else {
            panic!("sequence group should produce a sequence sample");
        };
        assert_eq!(sample.animation_index, 0);

        let tick = engine.tick_driver(AnimationDriver::Paint, now + Duration::from_millis(50));
        assert_eq!(tick.active_count, 1);
        assert!(tick.has_gpu_or_paint);
        assert_eq!(tick.dirty_bounds.as_slice(), &[dirty_bounds]);

        let tick = engine.tick_driver(AnimationDriver::Paint, now + Duration::from_millis(250));
        assert_eq!(tick.active_count, 0);
        assert!(tick.has_gpu_or_paint);
    }

    #[test]
    fn animation_engine_ticks_parallel_group_by_layout_driver() {
        let now = Instant::now();
        let mut engine = AnimationEngine::new();
        let group_id = engine.start_parallel(
            AnimationParallel::new(vec![
                AnimationSpec::new(Duration::from_millis(100)).driver(AnimationDriver::Paint),
                AnimationSpec::new(Duration::from_millis(100)).driver(AnimationDriver::Layout),
            ]),
            now,
        );

        let tick = engine.tick_driver(AnimationDriver::Paint, now + Duration::from_millis(50));
        assert_eq!(tick.active_count, 1);
        assert!(!tick.has_gpu_or_paint);
        assert!(engine.sample_group(group_id, now).is_some());

        let tick = engine.tick_driver(AnimationDriver::Layout, now + Duration::from_millis(150));
        assert_eq!(tick.active_count, 0);
        assert!(tick.has_layout);
    }

    #[test]
    fn animation_engine_samples_and_cancels_stagger_group() {
        let now = Instant::now();
        let mut engine = AnimationEngine::new();
        let group_id = engine.start_stagger(
            AnimationStagger::new(
                AnimationSpec::new(Duration::from_millis(100)).driver(AnimationDriver::Paint),
                2,
                Duration::from_millis(50),
            ),
            now,
        );

        let Some(AnimationGroupSample::Stagger(sample)) =
            engine.sample_group(group_id, now + Duration::from_millis(75))
        else {
            panic!("stagger group should produce a stagger sample");
        };
        assert_eq!(sample.samples.len(), 2);
        assert!((sample.samples[0].raw_progress - 0.75).abs() < 0.001);
        assert!((sample.samples[1].raw_progress - 0.25).abs() < 0.001);

        assert!(engine.cancel_group(group_id));
        assert_eq!(engine.active_count(), 0);
    }

    #[test]
    fn animation_engine_completion_tick_reports_finished_driver() {
        let now = Instant::now();
        let mut engine = AnimationEngine::new();
        let element = test_global_element_id("visual-completion");
        let dirty_bounds = bounds(point(px(2.0), px(4.0)), size(px(8.0), px(16.0)));

        engine.start_transition(
            &element,
            TransitionProperty::Opacity,
            AnimationSpec::new(Duration::from_millis(100)).driver(AnimationDriver::Paint),
            now,
        );
        assert!(engine.set_transition_bounds(&element, TransitionProperty::Opacity, dirty_bounds));

        let tick = engine.tick_driver(AnimationDriver::Paint, now + Duration::from_millis(200));
        assert_eq!(tick.active_count, 0);
        assert!(tick.has_gpu_or_paint);
        assert!(!tick.has_layout);
        assert_eq!(tick.dirty_bounds.as_slice(), &[dirty_bounds]);

        let mut engine = AnimationEngine::new();
        let element = test_global_element_id("layout-completion");
        engine.start_transition(
            &element,
            TransitionProperty::Width,
            AnimationSpec::new(Duration::from_millis(100)).driver(AnimationDriver::Layout),
            now,
        );
        assert!(engine.set_transition_bounds(&element, TransitionProperty::Width, dirty_bounds));

        let tick = engine.tick_driver(AnimationDriver::Layout, now + Duration::from_millis(200));
        assert_eq!(tick.active_count, 0);
        assert!(!tick.has_gpu_or_paint);
        assert!(tick.has_layout);
        assert!(tick.dirty_bounds.is_empty());
    }

    #[test]
    fn animation_engine_driver_indexes_follow_replace_cancel_and_completion() {
        let now = Instant::now();
        let mut engine = AnimationEngine::new();
        let element = test_global_element_id("indexed");

        engine.start_transition(
            &element,
            TransitionProperty::Opacity,
            AnimationSpec::new(Duration::from_millis(100))
                .repeat(RepeatMode::Forever)
                .driver(AnimationDriver::Paint),
            now,
        );
        engine.start_transition(
            &element,
            TransitionProperty::Width,
            AnimationSpec::new(Duration::from_millis(100))
                .repeat(RepeatMode::Forever)
                .driver(AnimationDriver::Layout),
            now,
        );
        assert_eq!(engine.test_index_counts(), (1, 1, 1));

        engine.start_transition(
            &element,
            TransitionProperty::Opacity,
            AnimationSpec::new(Duration::from_millis(100))
                .repeat(RepeatMode::Forever)
                .driver(AnimationDriver::Layout),
            now,
        );
        assert_eq!(engine.test_index_counts(), (1, 0, 2));

        engine.start_transition(
            &element,
            TransitionProperty::Width,
            AnimationSpec::new(Duration::ZERO).driver(AnimationDriver::Layout),
            now,
        );
        let tick = engine.tick_driver(AnimationDriver::Layout, now);
        assert_eq!(tick.active_count, 1);
        assert_eq!(engine.test_index_counts(), (1, 0, 1));

        engine.cancel_element(&element);
        assert_eq!(engine.test_index_counts(), (0, 0, 0));
        assert_eq!(engine.active_count(), 0);
    }
}
