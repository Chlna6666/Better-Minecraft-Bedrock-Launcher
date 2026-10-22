//! Application-wide UI frame pacing. Display VSync remains authoritative in auto
//! mode; explicit custom/unlimited modes opt in to Immediate presentation.
use gpui::{App, AppContext as _, FrameClock, PresentModePreference};

pub(crate) const DEFAULT_FPS: u32 = 120;
pub(crate) const MAX_FPS: u32 = 1000;

pub(crate) fn normalized_mode(mode: &str) -> &'static str {
    match mode {
        "vsync" => "vsync",
        "custom" => "custom",
        "unlimited" => "unlimited",
        _ => "auto",
    }
}

pub(crate) fn frame_clock(mode: &str, fps: u32) -> FrameClock {
    match normalized_mode(mode) {
        "unlimited" | "vsync" => FrameClock::System,
        "custom" => FrameClock::fixed_fps(fps.clamp(1, MAX_FPS))
            .expect("validated nonzero custom FPS"),
        _ => FrameClock::fixed_fps(DEFAULT_FPS).expect("nonzero default FPS"),
    }
}

pub(crate) fn present_mode(mode: &str) -> PresentModePreference {
    if matches!(normalized_mode(mode), "auto" | "vsync") {
        PresentModePreference::AutoVsync
    } else {
        PresentModePreference::Immediate
    }
}

/// Launch flags affect only this process. The user's persisted preference is never overwritten.
pub(crate) fn effective_mode<'a>(
    persisted_mode: &'a str,
    override_fps: Option<u32>,
) -> &'a str {
    match override_fps {
        Some(0) => "unlimited",
        Some(_) => "custom",
        None => normalized_mode(persisted_mode),
    }
}

pub(crate) fn effective_fps(saved: u32, override_fps: Option<u32>) -> u32 {
    override_fps.filter(|fps| *fps != 0).unwrap_or(saved).clamp(1, MAX_FPS)
}

pub(crate) fn apply_window_clock(mode: &str, fps: u32, cx: &mut App) {
    let clock = frame_clock(mode, fps);
    gpui::set_default_frame_clock(clock);
    // Use a snapshot; closing a secondary window can mutate the registry while
    // it is being iterated. A closed handle is harmless.
    for handle in cx.windows() {
        let _ = cx.update_window(handle, |_, window, _| {
            window.set_frame_clock(clock);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn auto_is_always_120_ceiling_without_disabling_vsync() {
        assert_eq!(frame_clock("auto", 240).fps().unwrap().get(), 120);
        assert_eq!(present_mode("auto"), PresentModePreference::AutoVsync);
    }
    #[test]
    fn native_vsync_has_no_120_fps_software_cap() {
        assert_eq!(normalized_mode("vsync"), "vsync");
        assert!(frame_clock("vsync", 300).fps().is_none());
        assert_eq!(present_mode("vsync"), PresentModePreference::AutoVsync);
        // Original default policy is unchanged.
        assert_eq!(frame_clock("auto", 300).fps().unwrap().get(), 120);
    }

    #[test]
    fn custom_bypasses_vsync_and_obeys_explicit_cap() {
        assert_eq!(frame_clock("custom", 360).fps().unwrap().get(), 360);
        assert_eq!(present_mode("custom"), PresentModePreference::Immediate);
        assert_eq!(frame_clock("custom", 5000).fps().unwrap().get(), 1000);
        assert_eq!(frame_clock("custom", 0).fps().unwrap().get(), 1);
    }
    #[test]
    fn command_line_override_is_not_persistent_and_can_unlock() {
        assert_eq!(effective_mode("auto", Some(240)), "custom");
        assert_eq!(effective_fps(120, Some(240)), 240);
        assert_eq!(effective_mode("custom", Some(0)), "unlimited");
        assert!(frame_clock("unlimited", 120).fps().is_none());
    }
}
