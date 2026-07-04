use crate::ui::state::i18n::I18n;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::settings::state::{SettingsPageState, SettingsTab};
use gpui::*;
use lucide_gpui::icons as lucide_icons;

pub(super) fn refresh_gpu_adapters_if_needed(cx: &mut App) {
    let should_refresh = cx.read_global(|state: &SettingsPageState, _cx| {
        state.tab == SettingsTab::Launcher && state.gpu_adapter_options.is_empty()
    });
    if !should_refresh {
        return;
    }

    let renderer_backend = cx.read_global(|state: &SettingsPageState, _cx| {
        crate::config::config::normalize_renderer_backend(state.renderer_backend.as_ref())
    });
    refresh_gpu_adapters_for_backend(renderer_backend, cx);
}

pub(super) fn refresh_gpu_adapters_for_backend(renderer_backend: String, cx: &mut App) {
    cx.spawn(async move |cx| {
        let requested_renderer_backend = renderer_backend.clone();
        let adapters = tokio::task::spawn_blocking(move || {
            let backend = renderer_backend
                .parse::<gpui::RendererBackend>()
                .unwrap_or_default();
            gpui::enumerate_gpu_adapters(backend)
                .into_iter()
                .map(|adapter| SharedString::from(adapter.name))
                .collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();

        let _ = cx.update_global(|state: &mut SettingsPageState, cx| {
            let current_renderer_backend =
                crate::config::config::normalize_renderer_backend(state.renderer_backend.as_ref());
            if current_renderer_backend != requested_renderer_backend {
                return;
            }
            state.gpu_adapter_options = adapters;
        });
    })
    .detach();
}

pub(super) fn render_tabs(colors: &ThemeColors, i18n: &I18n, active: SettingsTab) -> Div {
    let tab = |id: &'static str,
               icon: &'static str,
               label: SharedString,
               tab: SettingsTab,
               active: SettingsTab| {
        let is_active = tab == active;
        let bg = if is_active {
            Hsla {
                a: 0.20,
                ..colors.accent
            }
        } else {
            Hsla {
                a: 0.44,
                ..colors.surface
            }
        };
        let border = if is_active {
            Hsla {
                a: 0.34,
                ..colors.accent
            }
        } else {
            Hsla {
                a: 0.14,
                ..colors.border
            }
        };
        let fg = if is_active {
            colors.text_primary
        } else {
            colors.text_secondary
        };

        let mut tab_button = div()
            .id(id)
            .min_h(px(42.))
            .flex()
            .items_center()
            .gap(px(9.))
            .px(px(14.))
            .py(px(10.))
            .rounded(px(12.))
            .bg(bg)
            .border_1()
            .border_color(border)
            .cursor_pointer()
            .child(
                div().text_color(fg).child(
                    svg()
                        .path(icon)
                        .w(px(15.))
                        .h(px(15.))
                        .text_color(fg)
                        .opacity(if is_active { 1.0 } else { 0.72 }),
                ),
            )
            .child(
                div()
                    .text_size(px(13.5))
                    .font_weight(if is_active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::default()
                    })
                    .text_color(fg)
                    .child(label),
            )
            .hover(|this| {
                this.bg(Hsla {
                    a: 0.58,
                    ..colors.surface_hover
                })
            })
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
                    refresh_gpu_adapters_if_needed(cx);
                }
            });

        if is_active {
            tab_button = tab_button.shadow(vec![BoxShadow {
                color: Hsla {
                    a: 0.14,
                    ..rgb(0x000000).into()
                },
                blur_radius: px(12.),
                spread_radius: px(-6.),
                offset: point(px(0.), px(4.)),
            }]);
        }

        tab_button
    };

    div()
        .w_full()
        .rounded(px(16.))
        .bg(Hsla {
            a: 0.42,
            ..colors.surface
        })
        .border_1()
        .border_color(Hsla {
            a: 0.16,
            ..colors.border
        })
        .p(px(7.))
        .flex()
        .items_center()
        .gap(px(8.))
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
        ))
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
}
