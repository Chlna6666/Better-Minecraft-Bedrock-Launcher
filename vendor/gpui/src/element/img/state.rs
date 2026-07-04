use super::loader::TargetSizeImageSource;
use crate::{AnimatedFrame, AnyElement, App, BackgroundExecutor, RenderImage, Task, Window};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

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
    config.decode_ahead_frames.clamp(1, 4)
}

fn next_animation_frame(
    data: &RenderImage,
    current_sequence: usize,
    executor: &BackgroundExecutor,
) -> Option<AnimatedFrame> {
    if data.frame_count() == usize::MAX {
        data.next_streaming_frame(current_sequence, executor)
    } else {
        let frame_count = data.frame_count();
        if frame_count == 0 {
            return None;
        }
        data.frame((current_sequence + 1) % frame_count)
    }
}

pub(super) fn select_animation_frame(
    state: &mut ImgState,
    data: &RenderImage,
    animation_config: crate::AnimatedImageConfig,
    executor: &BackgroundExecutor,
) -> Option<AnimatedFrame> {
    let animation_config = animation_config.clamped();
    let current_time = executor.now();
    let mut current_frame = state.current_frame.clone().or_else(|| data.frame(0))?;

    if !data.is_animated() || !animation_config.play {
        let first_frame = data.frame(0)?;
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

        let Some(next_frame) = next_animation_frame(data, current_frame.sequence(), executor)
        else {
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
    state: &ImgState,
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
    data: &RenderImage,
    animation_config: crate::AnimatedImageConfig,
) -> bool {
    data.is_animated() && animation_config.play
}

/// The image state between frames
pub(super) struct ImgState {
    pub(super) current_image: Option<Arc<RenderImage>>,
    pub(super) current_frame: Option<AnimatedFrame>,
    pub(super) next_frame_at: Option<Instant>,
    pub(super) started_loading: Option<(Instant, Task<()>)>,
    pub(super) target_size_asset: Option<TargetSizeImageSource>,
    pub(super) pending_target_drop: Option<TargetSizeImageSource>,
}

/// The image layout state between frames
pub struct ImgLayoutState {
    pub(super) frame: Option<AnimatedFrame>,
    pub(super) replacement: Option<AnyElement>,
}
