#![cfg(target_os = "windows")]

use gpui::*;

use crate::ui::state::launch_prereq::LaunchPrereqState;

pub fn render_launch_prereq_overlay(
    state: &LaunchPrereqState,
    window: &mut Window,
    cx: &App,
) -> AnyElement {
    if state.is_onboarding() {
        // 首次运行导览已经迁移到 RootView 上的独立 Guided Tour。
        // LaunchPrereqState 继续保留旧字段只用于兼容现有启动前置状态机，
        // 这里不再把 onboarding 当作 modal 渲染。
        return div().into_any_element();
    }

    super::launch_prereq_legacy::render_launch_prereq_overlay(state, window, cx)
}
