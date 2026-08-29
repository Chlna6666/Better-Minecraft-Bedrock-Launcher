use crate::ui::animation::{spring_motion, spring_smooth};
use crate::ui::state::i18n::I18n;
use crate::ui::state::update::UpdateState;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::settings::layout::SettingsLayout;
use crate::ui::views::settings::state::{SettingsPageState, SettingsTab};
use gpui::StatefulInteractiveElement as _;
use gpui::prelude::FluentBuilder as _;
use gpui::*;

#[cfg(any(target_os = "windows", target_os = "linux"))]
mod onboarding;

#[cfg(target_os = "linux")]
use super::proton_gdk;
use super::{about, customization, game, launcher, plugins};

pub(super) fn render_settings_content(
    colors: &ThemeColors,
    window_width: Pixels,
    render_engine: SharedString,
    i18n: &I18n,
    state: &SettingsPageState,
    plugin_model: &plugins::PluginSettingsModel,
    update: &UpdateState,
    system_font_names: &[String],
    layout: &SettingsLayout,
) -> impl IntoElement {
    // 桌面插件管理始终保持 master-detail 左右结构。默认窗口仍有足够的详情宽度，
    // 不应因为高度或中等窗口宽度切成上下堆叠，破坏桌面工作流的空间连续性。
    let compact_plugins = false;

    if state.tab == SettingsTab::Plugins {
        let panel = animated_settings_panel(
            plugins::render_plugins_tab(colors, i18n, state, plugin_model, compact_plugins),
            state.tab,
            true,
        );
        return div()
            .relative()
            .flex_1()
            .min_h(px(0.))
            .min_w(px(0.))
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(
                div()
                    .id("settings-plugins-viewport")
                    .flex_1()
                    .min_h(px(0.))
                    .min_w(px(0.))
                    .w_full()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .w_full()
                            .max_w(layout.plugin_max_width)
                            .h_full()
                            .min_w(px(0.))
                            .min_h(px(0.))
                            .pb(px(10.))
                            .child(panel),
                    ),
            )
            .into_any_element();
    }

    let panel: AnyElement = match state.tab {
        SettingsTab::Game => game::render_game_tab(colors, i18n, state).into_any_element(),
        SettingsTab::Launcher => {
            launcher::render_launcher_tab(colors, i18n, state).into_any_element()
        }
        #[cfg(target_os = "linux")]
        SettingsTab::ProtonGdk => proton_gdk::render(colors, i18n).into_any_element(),
        SettingsTab::Customization => {
            customization::render_customization_tab(colors, i18n, state, system_font_names)
                .into_any_element()
        }
        SettingsTab::Plugins => {
            plugins::render_plugins_tab(colors, i18n, state, plugin_model, compact_plugins)
                .into_any_element()
        }
        SettingsTab::About => {
            let about_panel =
                about::render_about_tab(colors, window_width, render_engine, i18n, state, update);
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            {
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(about_panel)
                    .child(onboarding::render_onboarding_card(colors, i18n))
                    .into_any_element()
            }
            #[cfg(not(any(target_os = "windows", target_os = "linux")))]
            {
                about_panel.into_any_element()
            }
        }
    };

    let scroll_area = div()
        .id("settings-content-scroll")
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .overflow_y_scroll()
        .scrollbar_width(px(0.))
        .flex()
        .flex_col()
        .child(
            div().w_full().flex().justify_center().child(
                div()
                    .w_full()
                    .max_w(layout.content_max_width)
                    .pt(px(4.))
                    .pb(px(18.))
                    .child(animated_settings_panel(panel, state.tab, false)),
            ),
        );

    div()
        .relative()
        .flex_1()
        .min_h(px(0.))
        .min_w(px(0.))
        .flex()
        .flex_col()
        .child(scroll_area)
        .into_any_element()
}

fn animated_settings_panel(
    panel: impl IntoElement,
    tab: SettingsTab,
    fill_height: bool,
) -> AnyElement {
    let key = match tab {
        SettingsTab::Game => "settings-content-game",
        SettingsTab::Launcher => "settings-content-launcher",
        #[cfg(target_os = "linux")]
        SettingsTab::ProtonGdk => "settings-content-proton-gdk",
        SettingsTab::Customization => "settings-content-customization",
        SettingsTab::Plugins => "settings-content-plugins",
        SettingsTab::About => "settings-content-about",
    };

    // 整页位移必须包含文字、路径和嵌套裁剪；局部图元的 retained 动画尚不覆盖这些语义。

    div()
        .w_full()
        .min_w(px(0.))
        .when(fill_height, |this| this.h_full().min_h(px(0.)))
        .child(panel)
        .with_animation(key, spring_motion(spring_smooth()), |panel, progress| {
            panel.relative().left(px(10.0 * (1.0 - progress)))
        })
        .into_any_element()
}
