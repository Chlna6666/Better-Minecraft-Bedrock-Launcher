use crate::ui::animation::{spring_motion, spring_smooth};
use crate::ui::state::i18n::I18n;
use crate::ui::state::update::UpdateState;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::settings::layout::SettingsLayout;
use crate::ui::views::settings::state::{SettingsPageState, SettingsTab};
use gpui::StatefulInteractiveElement as _;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use std::time::Duration;

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
    let compact_plugins = window_width < px(980.);

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
            .flex()
            .flex_col()
            .child(
                div()
                    .id("settings-plugins-viewport")
                    .flex_1()
                    .min_h(px(0.))
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
                            .pb(px(16.))
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
        SettingsTab::ProtonGdk => proton_gdk::render(colors).into_any_element(),
        SettingsTab::Customization => {
            customization::render_customization_tab(colors, i18n, state, system_font_names)
                .into_any_element()
        }
        SettingsTab::Plugins => {
            plugins::render_plugins_tab(colors, i18n, state, plugin_model, compact_plugins)
                .into_any_element()
        }
        SettingsTab::About => {
            about::render_about_tab(colors, window_width, render_engine, i18n, state, update)
                .into_any_element()
        }
    };

    let scroll_area = div()
        .id("settings-content-scroll")
        .flex_1()
        .min_h(px(0.))
        .overflow_y_scroll()
        .scrollbar_width(px(0.))
        .flex()
        .flex_col()
        .child(
            div().w_full().flex().justify_center().child(
                div()
                    .w_full()
                    .max_w(layout.content_max_width)
                    .pt(px(6.))
                    .pb(px(24.))
                    .child(animated_settings_panel(panel, state.tab, false)),
            ),
        );

    div()
        .relative()
        .flex_1()
        .min_h(px(0.))
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
    let translation = AnimationProperty::translation(point(px(10.), px(0.)), point(px(0.), px(0.)));

    div()
        .w_full()
        .min_w(px(0.))
        .when(fill_height, |this| this.h_full().min_h(px(0.)))
        .child(panel)
        .with_animation(
            key,
            spring_motion(spring_smooth(), Duration::from_millis(520)).with_property(translation),
            |panel, progress| panel.opacity(0.76 + progress * 0.24),
        )
        .into_any_element()
}
