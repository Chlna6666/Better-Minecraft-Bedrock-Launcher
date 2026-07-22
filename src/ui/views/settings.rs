use crate::plugins::runtime::PluginRegistry;
use crate::ui::components::button::Button;
use crate::ui::components::modal;
use crate::ui::state::i18n::I18n;
use crate::ui::state::theme::ThemeState;
use crate::ui::state::update::UpdateState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};
use crate::ui::views::settings::state::{SettingsPageState, SettingsTab};
use gpui::*;
use std::rc::Rc;
use std::sync::Arc;

mod about;
mod common;
mod content;
mod customization;
mod game;
mod launcher;
mod plugins;
#[cfg(target_os = "linux")]
mod proton_gdk;
mod rows;
pub mod state;
mod tabs;

const STATIC_ASSET_PRELOAD_LIMIT: usize = 4;

pub(crate) fn preload_static_assets(cx: &mut App) -> usize {
    cx.preload_image_resources(
        about::ABOUT_INTERACTION_PRELOAD_RESOURCES
            .iter()
            .copied()
            .take(STATIC_ASSET_PRELOAD_LIMIT)
            .map(|path| Resource::Embedded(SharedString::from(path))),
    )
    .len()
}

pub struct SettingsPageView {
    _subscriptions: Vec<Subscription>,
    last_update_checking: bool,
}

impl SettingsPageView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let last_update_checking = cx.global::<UpdateState>().checking;
        let subscriptions = vec![
            cx.observe_global::<SettingsPageState>(|_, cx| {
                let tab = cx.global::<SettingsPageState>().tab;
                tracing::trace!(?tab, "settings view notify source=SettingsPageState");
                cx.notify();
            }),
            cx.observe_global::<ThemeState>(|_, cx| {
                tracing::trace!("settings view notify source=ThemeState");
                cx.notify();
            }),
            cx.observe_global::<I18n>(|_, cx| {
                tracing::trace!("settings view notify source=I18n");
                cx.notify();
            }),
            cx.observe_global::<UpdateState>(|this, cx| {
                let route = crate::ui::navigation::current_route(cx);
                let settings_state = cx.global::<SettingsPageState>();
                let update_state = cx.global::<UpdateState>();
                let checking_changed = this.last_update_checking != update_state.checking;
                this.last_update_checking = update_state.checking;
                if route == crate::ui::navigation::AppRoute::Settings
                    && settings_state.tab == SettingsTab::About
                    && checking_changed
                {
                    tracing::trace!(
                        checking = update_state.checking,
                        "settings view notify source=UpdateState"
                    );
                    cx.notify();
                }
            }),
            cx.observe_global::<PluginRegistry>(|_, cx| {
                let route = crate::ui::navigation::current_route(cx);
                let tab = cx.global::<SettingsPageState>().tab;
                if route == crate::ui::navigation::AppRoute::Settings && tab == SettingsTab::Plugins
                {
                    tracing::trace!(?tab, "settings view notify source=PluginRegistry");
                    cx.notify();
                }
            }),
        ];
        Self {
            _subscriptions: subscriptions,
            last_update_checking,
        }
    }
}

impl Render for SettingsPageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = std::time::Instant::now();
        let theme = cx.global::<ThemeState>();
        let colors = lerp_theme_colors(
            &LightColors::colors(),
            &DarkColors::colors(),
            theme.factor(now),
            theme.accent,
        );
        let window_size = window.bounds().size;
        let render_engine = about::render_engine_label(window);
        plugins::ensure_plugin_resources(window, cx);
        let system_font_names = cx.text_system().font_names();
        let plugin_model =
            plugins::PluginSettingsModel::snapshot(cx, cx.global::<SettingsPageState>());
        render_settings_page(
            colors,
            window_size.width,
            window_size.height,
            render_engine,
            cx.global::<I18n>(),
            cx.global::<SettingsPageState>(),
            plugin_model,
            cx.global::<UpdateState>(),
            system_font_names.as_ref(),
        )
    }
}

pub fn render_settings_page(
    colors: ThemeColors,
    window_width: Pixels,
    _window_height: Pixels,
    render_engine: SharedString,
    i18n: &I18n,
    state: &SettingsPageState,
    plugin_model: plugins::PluginSettingsModel,
    update: &UpdateState,
    system_font_names: &[String],
) -> impl IntoElement + use<> {
    common::page_shell(
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(tabs::render_tabs(&colors, i18n, state.tab))
            .child(content::render_settings_content(
                &colors,
                window_width,
                render_engine,
                i18n,
                state,
                &plugin_model,
                update,
                system_font_names,
            )),
        &colors,
    )
}

pub fn render_settings_overlay(
    colors: &ThemeColors,
    window_width: Pixels,
    window_height: Pixels,
    theme_factor: f32,
    accent_override: Option<Hsla>,
    i18n: &I18n,
    state: &SettingsPageState,
    agreement_document: Arc<crate::ui::components::markdown_renderer::MarkdownDocument>,
) -> Option<AnyElement> {
    if state.font_restart_confirm_open {
        return Some(render_font_restart_modal(colors, i18n));
    }

    if state.tab == crate::ui::views::settings::state::SettingsTab::Launcher {
        if let Some(modal) =
            launcher::render_connectivity_modal(colors, window_width, window_height, i18n, state)
        {
            return Some(modal);
        }
    }

    if state.tab == crate::ui::views::settings::state::SettingsTab::About
        && state.about_sponsors_open
    {
        return Some(about::render_sponsors_modal(colors, i18n, state).into_any_element());
    }

    if state.tab == crate::ui::views::settings::state::SettingsTab::About
        && state.about_dependencies_open
    {
        return Some(
            about::render_dependencies_modal(colors, i18n, state, window_width, window_height)
                .into_any_element(),
        );
    }

    if state.tab == crate::ui::views::settings::state::SettingsTab::About
        && state.about_agreement_open
    {
        let title = i18n.t("UserAgreement.title");
        return Some(
            crate::ui::overlays::user_agreement::render_user_agreement_modal(
                agreement_document,
                window_width,
                window_height,
                theme_factor,
                accent_override,
                title,
                SharedString::from(""),
                state.about_agreement_scroll_handle.clone(),
                true,
                crate::ui::overlays::user_agreement::UserAgreementModalOptions::read_only(
                    std::rc::Rc::new(|cx: &mut App| {
                        cx.update_global(|state: &mut SettingsPageState, cx| {
                            state.about_agreement_open = false;
                        });
                    }),
                ),
            )
            .into_any_element(),
        );
    }

    None
}

fn render_font_restart_modal(colors: &ThemeColors, i18n: &I18n) -> AnyElement {
    let dismiss = Rc::new(|cx: &mut App| {
        cx.update_global(|state: &mut SettingsPageState, _cx| {
            state.close_font_restart_confirm();
        });
    });

    let cancel_dismiss = dismiss.clone();
    modal::modal_layer_dismissible(
        div()
            .w(px(460.))
            .max_w(px(460.))
            .rounded(px(22.))
            .border_1()
            .border_color(Hsla {
                a: 0.18,
                ..colors.border
            })
            .shadow(vec![BoxShadow {
                color: Hsla {
                    a: 0.24,
                    ..rgb(0x000000).into()
                },
                blur_radius: px(34.),
                spread_radius: px(-8.),
                offset: point(px(0.), px(18.)),
            }])
            .flex()
            .flex_col()
            .m(px(16.))
            .min_w(px(0.))
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
                cx.stop_propagation();
            })
            .child(
                div()
                    .p(px(24.))
                    .flex()
                    .items_start()
                    .gap(px(14.))
                    .child(
                        div()
                            .w(px(42.))
                            .h(px(42.))
                            .rounded(px(14.))
                            .bg(Hsla {
                                a: 0.14,
                                ..colors.accent
                            })
                            .border_1()
                            .border_color(Hsla {
                                a: 0.22,
                                ..colors.accent
                            })
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                div()
                                    .text_size(px(16.))
                                    .line_height(px(16.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(colors.accent)
                                    .child("Aa"),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(
                                div()
                                    .text_size(px(18.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(colors.text_primary)
                                    .child(i18n.t("CustomizationSettings.font_restart_title")),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .line_height(px(20.))
                                    .text_color(colors.text_secondary)
                                    .child(i18n.t("CustomizationSettings.font_restart_desc")),
                            ),
                    ),
            )
            .child(
                div()
                    .px(px(24.))
                    .pb(px(24.))
                    .flex()
                    .justify_end()
                    .gap(px(10.))
                    .child(
                        Button::new("settings-font-restart-cancel")
                            .h(px(38.))
                            .px(px(16.))
                            .rounded(px(11.))
                            .border_1()
                            .border_color(Hsla {
                                a: 0.20,
                                ..colors.border
                            })
                            .bg(Hsla {
                                a: 0.08,
                                ..colors.text_secondary
                            })
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.text_secondary)
                            .label(i18n.t("CustomizationSettings.font_restart_later"))
                            .on_click(move |_event, _window, cx| {
                                cancel_dismiss(cx);
                            }),
                    )
                    .child(
                        Button::new("settings-font-restart-confirm")
                            .h(px(38.))
                            .px(px(16.))
                            .rounded(px(11.))
                            .border_0()
                            .bg(colors.accent)
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.btn_primary_text)
                            .label(i18n.t("CustomizationSettings.font_restart_now"))
                            .on_click(|_event, _window, cx| {
                                cx.update_global(|state: &mut SettingsPageState, _cx| {
                                    state.close_font_restart_confirm();
                                });
                                cx.restart();
                            }),
                    ),
            ),
        colors.backdrop,
        dismiss,
    )
    .into_any_element()
}
