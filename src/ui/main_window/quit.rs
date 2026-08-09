use crate::ui::animation::ease_out_cubic;
use crate::ui::state::quit::QuitState;
use gpui::{App, AppContext as _, BorrowAppContext as _, Pixels, Window, px};
use std::time::Instant;

const EXIT_SCALE_DISTANCE: f32 = 0.50;
const EXIT_OFFSET_DISTANCE: f32 = 24.0;
const EXIT_FADE_START: f32 = 0.30;
const EXIT_MIN_OPACITY: f32 = 0.08;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct QuitVisualState {
    pub(super) opacity: f32,
    pub(super) scale: f32,
    pub(super) offset_y: Pixels,
}

pub(super) fn visual_state(progress: f32) -> QuitVisualState {
    let progress = progress.clamp(0.0, 1.0);
    let motion = ease_out_cubic(progress);
    let fade_progress = ((progress - EXIT_FADE_START) / (1.0 - EXIT_FADE_START)).clamp(0.0, 1.0);
    QuitVisualState {
        opacity: 1.0 - (1.0 - EXIT_MIN_OPACITY) * fade_progress.powf(1.7),
        scale: 1.0 - EXIT_SCALE_DISTANCE * motion,
        offset_y: px(EXIT_OFFSET_DISTANCE * motion),
    }
}

pub(super) fn install_window_close_interceptor(window: &Window, cx: &App) {
    window.on_window_should_close(cx, |window, cx| {
        request(window, cx);
        false
    });
}

pub(super) fn request(window: &mut Window, cx: &mut App) {
    let now = Instant::now();
    let (started, duration) =
        cx.update_global(|state: &mut QuitState, _cx| (state.request_quit(now), state.duration()));
    if !started {
        return;
    }

    let window_handle = window.window_handle();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(duration).await;
        if let Err(error) = cx.update_window(window_handle, |_root, window, _cx| {
            window.remove_window();
        }) {
            tracing::debug!(?error, "main window closed before quit animation completed");
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visual_state_recedes_before_fading_out() {
        let halfway = visual_state(0.5);

        assert!(halfway.opacity > 0.85);
        assert!(halfway.scale < 0.60);
        assert!(halfway.offset_y > px(20.0));
    }

    #[test]
    fn visual_state_keeps_a_visible_final_frame_until_window_removal() {
        let finished = visual_state(1.0);

        assert!((finished.opacity - EXIT_MIN_OPACITY).abs() < f32::EPSILON);
        assert!((finished.scale - 0.5).abs() < f32::EPSILON);
        assert_eq!(finished.offset_y, px(24.0));
    }
}
