#![cfg(target_os = "windows")]

use gpui::*;
use lucide_gpui::icons as lucide_icons;

use crate::ui::state::launch_prereq::{LaunchPrereqMode, LaunchPrereqState, OnboardingStep};
use crate::ui::theme::colors::ThemeColors;

pub(super) fn render_onboarding_card(colors: &ThemeColors) -> Div {
    let action = div()
        .id("settings-reopen-onboarding")
        .h(px(36.))
        .px(px(14.))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla {
            a: 0.24,
            ..colors.border
        })
        .bg(colors.surface)
        .text_color(colors.text_primary)
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .hover(|this| this.bg(colors.surface_hover))
        .flex()
        .items_center()
        .justify_center()
        .child("重新打开引导")
        .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            if let Err(error) = crate::config::onboarding::reset_onboarding() {
                crate::ui::components::toast::error(
                    cx,
                    SharedString::from(format!("无法重置首次运行引导: {error}")),
                );
                return;
            }

            cx.update_global(|state: &mut LaunchPrereqState, _cx| {
                state.visible = true;
                state.mode = LaunchPrereqMode::Onboarding;
                state.onboarding_step = OnboardingStep::Welcome;
                state.onboarding_scanning = false;
                state.onboarding_environment = None;
                state.onboarding_error = None;
            });
        });

    crate::ui::components::page_shell::glass_card(colors)
        .shadow(Vec::new())
        .bg(Hsla {
            a: 0.94,
            ..colors.settings_card_bg
        })
        .border_color(Hsla {
            a: 0.28,
            ..colors.border
        })
        .w_full()
        .p(px(18.))
        .flex()
        .items_center()
        .gap(px(14.))
        .child(
            div()
                .w(px(42.))
                .h(px(42.))
                .rounded(px(crate::ui::theme::tokens::radius::MD))
                .bg(Hsla {
                    a: 0.12,
                    ..colors.accent
                })
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path(lucide_icons::icon_route())
                        .w(px(19.))
                        .h(px(19.))
                        .text_color(colors.accent),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child("首次运行设置向导"),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(colors.text_secondary)
                        .child("重新查看版本下载、导入、散装 UWP 多版本切换和数据保护说明。"),
                ),
        )
        .child(action)
}
