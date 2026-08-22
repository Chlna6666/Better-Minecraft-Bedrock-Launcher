#![cfg(target_os = "windows")]

use gpui::{AnyElement, App, Window};

use crate::ui::state::launch_prereq::LaunchPrereqState;

pub fn render_launch_prereq_overlay(
    state: &LaunchPrereqState,
    window: &mut Window,
    cx: &App,
) -> AnyElement {
    if state.is_onboarding() {
        return super::onboarding::render_onboarding_overlay(state, window, cx);
    }

    super::launch_prereq_legacy::render_launch_prereq_overlay(state, window, cx)
}
