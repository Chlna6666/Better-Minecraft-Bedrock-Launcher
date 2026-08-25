use super::retained::ImageElementState;
use crate::{AnimatedFrame, App, BackgroundExecutor, RenderImage, Window};
use std::time::Duration;

fn frame_duration(delay: image::Delay, config: crate::AnimatedImageConfig) -> Duration {
    let duration = Duration::from(delay);
    let minimum = config.minimum_frame_duration();
    if duration.is_zero() {
        minimum
    } else {
        duration.max(minimum)
    }
}

fn frame_advance_budget(config: crate::AnimatedImageConfig) -> usize {
    config.prefetch_frames.clamp(1, 4)
}

fn next_animation_frame(
    render_image: &RenderImage,
    current_sequence: usize,
) -> Option<AnimatedFrame> {
    if render_image.frame_count() == usize::MAX {
        render_image.next_streaming_frame(current_sequence)
    } else {
        let frame_count = render_image.frame_count();
        if frame_count == 0 {
            return None;
        }
        render_image.frame((current_sequence + 1) % frame_count)
    }
}

pub(super) fn select_animation_frame(
    state: &mut ImageElementState,
    render_image: &RenderImage,
    animation_config: crate::AnimatedImageConfig,
    executor: &BackgroundExecutor,
) -> Option<AnimatedFrame> {
    let animation_config = animation_config.clamped();
    let current_time = executor.now();
    let mut current_frame = state
        .current_frame
        .clone()
        .or_else(|| render_image.frame(0))?;

    if !render_image.is_animated() || !animation_config.play {
        let first_frame = render_image.frame(0)?;
        state.current_frame = Some(first_frame.clone());
        state.next_frame_at = None;
        return Some(first_frame);
    }

    let mut next_frame_at = state
        .next_frame_at
        .unwrap_or_else(|| current_time + frame_duration(current_frame.delay(), animation_config));

    if current_time < next_frame_at {
        state.current_frame = Some(current_frame.clone());
        state.next_frame_at = Some(next_frame_at);
        return Some(current_frame);
    }

    let mut advanced_frame = false;
    for _ in 0..frame_advance_budget(animation_config) {
        if current_time < next_frame_at {
            break;
        }

        let Some(next_frame) = next_animation_frame(render_image, current_frame.sequence()) else {
            next_frame_at = current_time + animation_config.minimum_frame_duration();
            break;
        };
        next_frame_at += frame_duration(next_frame.delay(), animation_config);
        current_frame = next_frame;
        advanced_frame = true;
    }

    if advanced_frame && current_time >= next_frame_at {
        next_frame_at = current_time + frame_duration(current_frame.delay(), animation_config);
    }

    state.next_frame_at = Some(next_frame_at);
    state.current_frame = Some(current_frame.clone());
    Some(current_frame)
}

pub(super) fn request_next_image_animation_frame(
    state: &ImageElementState,
    window: &mut Window,
    cx: &App,
    animation_config: crate::AnimatedImageConfig,
) {
    let deadline = state.next_frame_at.unwrap_or_else(|| {
        cx.background_executor().now() + animation_config.minimum_frame_duration()
    });
    window.request_image_animation_frame_at(deadline, cx, animation_config);
}

pub(super) fn should_request_image_animation_frame(
    render_image: &RenderImage,
    animation_config: crate::AnimatedImageConfig,
) -> bool {
    render_image.is_animated() && animation_config.play
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_frame_delay_is_preserved_below_the_global_rate_limit() {
        let delay = image::Delay::from_saturating_duration(Duration::from_millis(40));
        assert_eq!(
            frame_duration(delay, crate::AnimatedImageConfig::default()),
            Duration::from_millis(40)
        );
    }

    #[test]
    fn all_animated_formats_share_the_ninety_fps_ceiling() {
        let delay = image::Delay::from_saturating_duration(Duration::from_millis(1));
        let duration = frame_duration(delay, crate::AnimatedImageConfig::default());
        let expected = Duration::from_secs_f32(1.0 / 90.0);
        assert_eq!(duration, expected);
    }

    #[test]
    fn applications_can_raise_the_playback_ceiling() {
        let delay = image::Delay::from_saturating_duration(Duration::from_millis(1));
        let config = crate::AnimatedImageConfig {
            max_fps: 240.0,
            ..crate::AnimatedImageConfig::default()
        };
        assert_eq!(
            frame_duration(delay, config),
            Duration::from_secs_f32(1.0 / 240.0)
        );
    }
}
