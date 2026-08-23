use crate::ui::state::i18n::I18n;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::settings::layout::SettingsLayout;
use crate::ui::views::settings::state::{SettingsPageState, SettingsTab};
use gpui::*;
use lucide_gpui::icons as lucide_icons;

pub(super) fn render_tabs(
    colors: &ThemeColors,
    i18n: &I18n,
    active: SettingsTab,
    layout: &SettingsLayout,
) -> Div {
    let tab = |id: &'static str,
               icon: &'static str,
               label: SharedString,
               tab: SettingsTab,
               active: SettingsTab| {
        let is_active = tab == active;
        let fg = if is_active {
            colors.text_primary
        } else {
            colors.text_secondary
        };

        div()
            .id(id)
            .flex_none()
            .min_h(px(40.))
            .px(px(12.))
            .py(px(9.))
            .rounded(px(crate::ui::theme::tokens::radius::SM))
            .bg(if is_active {
                Hsla {
                    a: 0.16,
                    ..colors.accent
                }
            } else {
                Hsla {
                    a: 0.20,
                    ..colors.surface
                }
            })
            .border_1()
            .border_color(if is_active {
                Hsla {
                    a: 0.30,
                    ..colors.accent
                }
            } else {
                Hsla {
                    a: 0.10,
                    ..colors.border
                }
            })
            .cursor_pointer()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                svg()
                    .path(icon)
                    .w(px(15.))
                    .h(px(15.))
                    .text_color(fg)
                    .opacity(if is_active { 1.0 } else { 0.74 }),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(if is_active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::NORMAL
                    })
                    .text_color(fg)
                    .child(label),
            )
            .hover(|this| {
                this.bg(Hsla {
                    a: 0.46,
                    ..colors.surface_hover
                })
            })
            .active(|this| this.scale(0.97))
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                let committed_blur = cx.update_global(|s: &mut SettingsPageState, _cx| {
                    if s.commit_background_blur_preview() {
                        Some(s.background_blur)
                    } else {
                        None
                    }
                });
                if let Some(blur) = committed_blur {
                    crate::ui::views::settings::common::spawn_persist_background_blur(blur, cx);
                }
                cx.update_global(|s: &mut SettingsPageState, _cx| {
                    s.tab = tab;
                });
                if tab == SettingsTab::Launcher {
                    super::tabs::refresh_gpu_adapters_if_needed(cx);
                    super::launcher::logs::refresh_log_stats(cx);
                }
            })
    };

    let container = div()
        .relative()
        .w_full()
        .min_w(px(0.))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .bg(Hsla {
            a: 0.22,
            ..colors.surface
        })
        .border_1()
        .border_color(Hsla {
            a: 0.14,
            ..colors.border
        })
        .p(if layout.scroll_tabs { px(5.) } else { px(6.) })
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(6.))
        .child(tab(
            "settings-tab-game",
            lucide_icons::icon_gamepad_2(),
            i18n.t("Settings.tabs.game"),
            SettingsTab::Game,
            active,
        ))
        .child(tab(
            "settings-tab-launcher",
            lucide_icons::icon_rocket(),
            i18n.t("Settings.tabs.launcher"),
            SettingsTab::Launcher,
            active,
        ));

    #[cfg(target_os = "linux")]
    let container = container.child(tab(
        "settings-tab-proton-gdk",
        lucide_icons::icon_box(),
        SharedString::from("Proton-GDK"),
        SettingsTab::ProtonGdk,
        active,
    ));

    container
        .child(tab(
            "settings-tab-customize",
            lucide_icons::icon_palette(),
            i18n.t("Settings.tabs.customization"),
            SettingsTab::Customization,
            active,
        ))
        .child(tab(
            "settings-tab-plugins",
            lucide_icons::icon_plug(),
            i18n.t("Settings.tabs.plugins"),
            SettingsTab::Plugins,
            active,
        ))
        .child(tab(
            "settings-tab-about",
            lucide_icons::icon_info(),
            i18n.t("Settings.tabs.about"),
            SettingsTab::About,
            active,
        ))
        .child(crate::ui::onboarding::anchor::observe(
            crate::ui::onboarding::state::OnboardingAnchor::SettingsTabs,
        ))
}
