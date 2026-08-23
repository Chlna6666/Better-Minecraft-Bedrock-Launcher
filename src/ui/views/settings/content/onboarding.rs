#![cfg(any(target_os = "windows", target_os = "linux"))]

use gpui::*;
use lucide_gpui::icons as lucide_icons;

use crate::ui::theme::colors::ThemeColors;

pub(super) fn render_onboarding_card(colors: &ThemeColors) -> Div {
    let action = div()
        .id("settings-reopen-onboarding")
        .flex_none()
        .min_h(px(36.))
        .px(px(14.))
        .py(px(8.))
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
        .child("重新打开导览")
        .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            crate::ui::onboarding::reopen(cx);
        });

    #[cfg(target_os = "windows")]
    let description = "重新进入交互式功能导览：会实际切换下载、导入和版本管理页面，并重新说明 Windows UWP 数据保护。不会重置游戏或配置。";
    #[cfg(target_os = "linux")]
    let description = "重新进入交互式功能导览：会实际切换下载、导入、版本管理和 Proton-GDK 页面。Linux 导览不会执行 UWP 检查。";

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
                .flex_none()
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
                        .w_full()
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child("交互式首次运行导览"),
                )
                .child(
                    div()
                        .w_full()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(colors.text_secondary)
                        .child(description),
                ),
        )
        .child(action)
}
