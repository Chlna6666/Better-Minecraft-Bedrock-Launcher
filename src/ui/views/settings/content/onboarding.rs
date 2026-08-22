#![cfg(any(target_os = "windows", target_os = "linux"))]

use gpui::*;
use lucide_gpui::icons as lucide_icons;

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
            #[cfg(target_os = "windows")]
            cx.update_global(
                |state: &mut crate::ui::state::launch_prereq::LaunchPrereqState, _cx| {
                    state.reopen_onboarding();
                },
            );

            #[cfg(target_os = "linux")]
            cx.update_global(
                |state: &mut crate::ui::state::linux_onboarding::LinuxOnboardingState, _cx| {
                    state.reopen();
                },
            );
        });

    #[cfg(target_os = "windows")]
    let description = "重新查看版本下载、导入、散装 UWP 多版本切换和数据保护说明。不会重置已经完成的首次运行状态。";
    #[cfg(target_os = "linux")]
    let description = "重新查看版本下载、导入、Linux 运行环境和 Proton-GDK 配置说明。Linux 引导不会执行 UWP 检查，也不会重置已经完成的首次运行状态。";

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
                        .child(description),
                ),
        )
        .child(action)
}
