#![cfg(target_os = "windows")]

use gpui::*;

use crate::ui::state::launch_prereq::LaunchPrereqState;

pub fn render_launch_prereq_overlay(
    state: &LaunchPrereqState,
    window: &mut Window,
    cx: &App,
) -> AnyElement {
    if state.is_onboarding() {
        let current_window_id = window.window_handle().window_id().as_u64();
        let main_window_id = cx.global::<crate::ui::window::debug::DebugState>().main_window_id;
        if main_window_id == Some(current_window_id) {
            return super::onboarding::render_onboarding_overlay(state, window, cx);
        }

        // 文件关联/独立导入窗口共享 LaunchPrereqState，但不应展示主窗口首次引导。
        return div().into_any_element();
    }

    super::launch_prereq_legacy::render_launch_prereq_overlay(state, window, cx)
}
