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
mod layout;
mod navigation;
mod plugins;
#[cfg(target_os = "linux")]
mod proton_gdk;
mod rows;
pub mod state;
mod surface;
mod tabs;

const STATIC_ASSET_PRELOAD_LIMIT: usize = 4;

pub(crate) fn preload_static_assets(cx: &mut App) -> usize {
    cx.preload_image_resources(
        about::ABOUT_INTERACTION_PRELOAD_RESOURCES
            .iter()
            .copied()
            .take(STATIC_ASSET_PRELOAD_LIMIT)
            .map(|path| AssetLocation::Embedded(SharedString::from(path))),
    )
    .len()
}

pub struct SettingsPageView {
    _subscriptions: Vec<Subscription>,
    content_view: Entity<SettingsContentView>,
    last_tab: SettingsTab,
    last_content_window_size: Option<(Pixels, Pixels)>,
    active: bool,
    background_prepare_scheduled: bool,
    prepared_tab: Option<SettingsTab>,
}

impl SettingsPageView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let last_tab = cx.global::<SettingsPageState>().tab;
        let content_view = cx.new(SettingsContentView::new);
        let subscriptions = vec![
            cx.observe_global::<SettingsPageState>(|this, cx| {
                let tab = cx.global::<SettingsPageState>().tab;
                if this.last_tab != tab {
                    this.last_tab = tab;
                    if this.active {
                        tracing::trace!(?tab, "settings shell notify source=tab");
                        cx.notify();
                    }
                }
            }),
            cx.observe_global::<ThemeState>(|this, cx| {
                if this.active {
                    cx.notify();
                }
            }),
            cx.observe_global::<I18n>(|this, cx| {
                if this.active {
                    cx.notify();
                }
            }),
        ];
        Self {
            _subscriptions: subscriptions,
            content_view,
            last_tab,
            last_content_window_size: None,
            active: false,
            background_prepare_scheduled: false,
            prepared_tab: None,
        }
    }

    pub(crate) fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        self.content_view
            .update(cx, |content, cx| content.set_active(active, cx));
        if active {
            cx.notify();
        }
    }
}

// Only retain fields read by the corresponding tab's content tree. Other
// tabs remain conservative until their nested dependencies have been audited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SettingsContentProjection {
    Game {
        launcher_display_mode: state::LauncherDisplayMode,
        fix_uwp_minimize: bool,
        keep_downloaded_packages: bool,
        modify_appx_manifest: bool,
    },
    About,
    Conservative(SettingsTab),
}

impl SettingsContentProjection {
    fn from_state(state: &SettingsPageState) -> Self {
        match state.tab {
            SettingsTab::Game => Self::Game {
                launcher_display_mode: state.launcher_display_mode,
                fix_uwp_minimize: state.fix_uwp_minimize,
                keep_downloaded_packages: state.keep_downloaded_packages,
                modify_appx_manifest: state.modify_appx_manifest,
            },
            // The About main content depends on UpdateState, ThemeState and I18n.
            // Sponsors/dependencies/agreement dialogs have a separate overlay View.
            SettingsTab::About => Self::About,
            tab => Self::Conservative(tab),
        }
    }

    fn should_notify(self, next: Self) -> bool {
        self != next || matches!(next, Self::Conservative(_))
    }
}

struct SettingsContentView {
    _subscriptions: Vec<Subscription>,
    last_update_checking: bool,
    last_projection: SettingsContentProjection,
    active: bool,
}

impl SettingsContentView {
    fn new(cx: &mut Context<Self>) -> Self {
        let last_update_checking = cx.global::<UpdateState>().checking;
        let last_projection =
            SettingsContentProjection::from_state(cx.global::<SettingsPageState>());
        let subscriptions = vec![
            cx.observe_global::<SettingsPageState>(|this, cx| {
                if !this.active {
                    return;
                }
                let next = SettingsContentProjection::from_state(cx.global::<SettingsPageState>());
                let changed = this.last_projection.should_notify(next);
                this.last_projection = next;
                if changed {
                    tracing::trace!(?next, "settings content notify source=tab-projection");
                    cx.notify();
                }
            }),
            cx.observe_global::<ThemeState>(|this, cx| {
                if this.active {
                    cx.notify();
                }
            }),
            cx.observe_global::<I18n>(|this, cx| {
                if this.active {
                    cx.notify();
                }
            }),
            cx.observe_global::<UpdateState>(|this, cx| {
                let route = crate::ui::navigation::current_route(cx);
                let settings_state = cx.global::<SettingsPageState>();
                let update_state = cx.global::<UpdateState>();
                let checking_changed = this.last_update_checking != update_state.checking;
                this.last_update_checking = update_state.checking;
                if this.active
                    && route == crate::ui::navigation::AppRoute::Settings
                    && settings_state.tab == SettingsTab::About
                    && checking_changed
                {
                    cx.notify();
                }
            }),
            cx.observe_global::<PluginRegistry>(|this, cx| {
                let route = crate::ui::navigation::current_route(cx);
                let tab = cx.global::<SettingsPageState>().tab;
                if this.active
                    && route == crate::ui::navigation::AppRoute::Settings
                    && tab == SettingsTab::Plugins
                {
                    cx.notify();
                }
            }),
        ];
        Self {
            _subscriptions: subscriptions,
            last_update_checking,
            last_projection,
            active: false,
        }
    }

    fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active != active {
            self.active = active;
            if active {
                self.last_projection =
                    SettingsContentProjection::from_state(cx.global::<SettingsPageState>());
                cx.notify();
            }
        }
    }
}

impl Render for SettingsPageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.active && !self.background_prepare_scheduled {
            self.background_prepare_scheduled = true;
            window.on_next_frame(|_window, cx| {
                // Prepare plugin metadata after Settings has presented once.
                crate::plugins::runtime::ensure_manifest_index(cx);
            });
        }

        let active_tab = cx.global::<SettingsPageState>().tab;
        tracing::trace!(
            target: "bmcbl::ui::render_trace",
            component = "settings-shell",
            ?active_tab,
            "settings view render"
        );
        if self.active && self.prepared_tab != Some(active_tab) {
            self.prepared_tab = Some(active_tab);
            window.on_next_frame(move |_window, cx| match active_tab {
                SettingsTab::Launcher => {
                    tabs::refresh_gpu_adapters_if_needed(cx);
                    launcher::logs::refresh_log_stats(cx);
                }
                SettingsTab::Plugins => {}
                #[cfg(target_os = "linux")]
                SettingsTab::ProtonGdk => {
                    proton_gdk::ensure_runner_snapshot(cx);
                }
                SettingsTab::Game | SettingsTab::Customization | SettingsTab::About => {}
            });
        }

        let theme = cx.global::<ThemeState>();
        let colors = lerp_theme_colors(
            &LightColors::colors(),
            &DarkColors::colors(),
            theme.factor(window.animation_time()),
            theme.accent,
        );
        let window_size = window.bounds().size;
        let content_size = (window_size.width, window_size.height);
        if self.last_content_window_size != Some(content_size) {
            self.last_content_window_size = Some(content_size);
            self.content_view.update(cx, |_, cx| cx.notify());
        }
        render_settings_page(
            colors,
            window_size.width,
            window_size.height,
            cx.global::<I18n>(),
            active_tab,
            self.content_view.clone(),
        )
    }
}

impl Render for SettingsContentView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = window.animation_time();
        let theme = cx.global::<ThemeState>();
        let colors = lerp_theme_colors(
            &LightColors::colors(),
            &DarkColors::colors(),
            theme.factor(now),
            theme.accent,
        );
        let window_size = window.bounds().size;
        let active_tab = cx.global::<SettingsPageState>().tab;
        tracing::trace!(
            target: "bmcbl::ui::render_trace",
            component = "settings-content",
            ?active_tab,
            "settings view render"
        );
        let render_engine = if active_tab == SettingsTab::About {
            about::render_engine_label(window)
        } else {
            SharedString::default()
        };
        let plugin_statuses = if active_tab == SettingsTab::Plugins {
            plugins::ensure_plugin_resources(window, cx)
        } else {
            Vec::new()
        };
        let system_font_names = if active_tab == SettingsTab::Customization {
            cx.text_system().font_names()
        } else {
            Arc::from(Vec::<String>::new())
        };
        let plugin_model = plugins::PluginSettingsModel::snapshot(
            cx,
            cx.global::<SettingsPageState>(),
            plugin_statuses,
            colors.bg.l < 0.5,
        );
        let settings_layout = layout::SettingsLayout::new(window_size.width, window_size.height);
        content::render_settings_content(
            &colors,
            window_size.width,
            render_engine,
            cx.global::<I18n>(),
            cx.global::<SettingsPageState>(),
            &plugin_model,
            cx.global::<UpdateState>(),
            system_font_names.as_ref(),
            &settings_layout,
        )
        .into_any_element()
    }
}

fn render_settings_page(
    colors: ThemeColors,
    window_width: Pixels,
    window_height: Pixels,
    i18n: &I18n,
    active_tab: SettingsTab,
    content_view: Entity<SettingsContentView>,
) -> impl IntoElement + use<> {
    let settings_layout = layout::SettingsLayout::new(window_width, window_height);
    surface::page_shell(
        div()
            .size_full()
            .min_w(px(0.))
            .min_h(px(0.))
            .flex()
            .flex_col()
            .gap(settings_layout.content_gap)
            .child(navigation::render_tabs(
                &colors,
                i18n,
                active_tab,
                &settings_layout,
            ))
            .child(
                // A retained child View still needs a definite flex viewport.
                // Without a flex parent its own flex_1 expands with the rows,
                // so the outer clipped page hides content instead of scrolling it.
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .min_h(px(0.))
                    .flex()
                    .flex_col()
                    .child(content_view),
            ),
        &colors,
        &settings_layout,
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
        let title = t!("UserAgreement.title");
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
                        cx.update_global(|state: &mut SettingsPageState, _cx| {
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

fn render_font_restart_modal(colors: &ThemeColors, _i18n: &I18n) -> AnyElement {
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
            .rounded(px(crate::ui::theme::tokens::radius::MD))
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
                            .rounded(px(crate::ui::theme::tokens::radius::MD))
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
                                    .child(t!("CustomizationSettings.font_restart_title")),
                            )
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .line_height(px(20.))
                                    .text_color(colors.text_secondary)
                                    .child(t!("CustomizationSettings.font_restart_desc")),
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
                            .rounded(px(crate::ui::theme::tokens::radius::SM))
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
                            .label(t!("CustomizationSettings.font_restart_later"))
                            .on_click(move |_event, _window, cx| {
                                cancel_dismiss(cx);
                            }),
                    )
                    .child(
                        Button::new("settings-font-restart-confirm")
                            .h(px(38.))
                            .px(px(16.))
                            .rounded(px(crate::ui::theme::tokens::radius::SM))
                            .border_0()
                            .bg(colors.accent)
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.btn_primary_text)
                            .label(t!("CustomizationSettings.font_restart_now"))
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

#[cfg(test)]
mod content_projection_tests {
    use super::*;

    #[test]
    fn game_tab_ignores_unrelated_settings() {
        let mut settings = SettingsPageState::default();
        settings.tab = SettingsTab::Game;
        let previous = SettingsContentProjection::from_state(&settings);
        settings.gpui_trace = !settings.gpui_trace;
        assert!(!previous.should_notify(SettingsContentProjection::from_state(&settings)));
        settings.fix_uwp_minimize = !settings.fix_uwp_minimize;
        assert!(previous.should_notify(SettingsContentProjection::from_state(&settings)));
    }

    #[test]
    fn tab_transition_and_about_overlay_updates() {
        let mut settings = SettingsPageState::default();
        settings.tab = SettingsTab::Game;
        let before = SettingsContentProjection::from_state(&settings);
        settings.tab = SettingsTab::About;
        let about = SettingsContentProjection::from_state(&settings);
        assert!(before.should_notify(about));
        settings.about_sponsors_open = true;
        assert!(!about.should_notify(SettingsContentProjection::from_state(&settings)));
        settings.tab = SettingsTab::Launcher;
        let launcher = SettingsContentProjection::from_state(&settings);
        assert!(about.should_notify(launcher));
        assert!(launcher.should_notify(launcher));
    }
}
