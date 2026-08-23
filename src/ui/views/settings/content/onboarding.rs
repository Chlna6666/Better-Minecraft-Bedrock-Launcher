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
    let description = "重新进入完整交互式功能导览：依次展示游戏下载、CurseForge 资源、模组、导入、任务、版本管理、设置和工具。UWP 数据保护不属于首次导览，也不保存独立配置；下载或导入 UWP 时只会根据当前 Windows 是否存在 Microsoft Store/系统安装的 Minecraft 实时判断是否需要提示。";
    #[cfg(target_os = "linux")]
    let description = "重新进入完整交互式功能导览：会依次展示游戏下载、CurseForge 资源、模组、导入、任务、版本管理、设置、工具和 Proton-GDK。Linux 导览不会执行 UWP 检查。";

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
                        .child("完整交互式功能导览"),
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
