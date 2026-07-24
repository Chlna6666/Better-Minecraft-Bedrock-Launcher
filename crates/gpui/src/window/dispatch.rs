use super::*;

const SLOW_INPUT_DISPATCH: Duration = Duration::from_millis(16);

pub(super) fn platform_input_name(event: &PlatformInput) -> &'static str {
    match event {
        PlatformInput::KeyDown(_) => "key_down",
        PlatformInput::KeyUp(_) => "key_up",
        PlatformInput::ModifiersChanged(_) => "modifiers_changed",
        PlatformInput::MouseDown(_) => "mouse_down",
        PlatformInput::MouseUp(_) => "mouse_up",
        PlatformInput::MouseMove(event) if event.pressed_button.is_some() => "mouse_drag",
        PlatformInput::MouseMove(_) => "mouse_move",
        PlatformInput::MouseExited(_) => "mouse_exited",
        PlatformInput::ScrollWheel(_) => "scroll_wheel",
        PlatformInput::FileDrop(_) => "file_drop",
    }
}

pub(super) fn log_timed_gpui_event(
    message: &'static str,
    elapsed: Duration,
    log_fields: impl FnOnce() -> String,
) {
    if elapsed >= SLOW_INPUT_DISPATCH {
        if log::log_enabled!(log::Level::Warn) {
            log::warn!("{} elapsed={:?} {}", message, elapsed, log_fields());
        }
    } else if log::log_enabled!(log::Level::Trace) {
        log::trace!("{} elapsed={:?} {}", message, elapsed, log_fields());
    }
}
