use super::*;
use crate::ui::state::local_versions::LocalVersionsState;
use crate::ui::views::download::state::{DownloadPageState, DownloadTab};
use crate::ui::views::settings::state::{SettingsPageState, SettingsTab};

pub(super) fn settings_visible(state: &SettingsPageState) -> bool {
    state.font_restart_confirm_open
        || (state.tab == SettingsTab::Launcher && state.launcher_connectivity_open)
        || (state.tab == SettingsTab::About
            && (state.about_sponsors_open
                || state.about_dependencies_open
                || state.about_agreement_open))
}

pub(super) fn download_visible(state: &DownloadPageState) -> bool {
    state.game_dialog.is_some()
        || (state.levilauncher_modal_open && state.levilauncher_selected_mod.is_some())
        || (state.native_mod_modal_open && state.native_mod_selected.is_some())
        || (state.tab == DownloadTab::ResourcePack
            && (state.curseforge_install_open || state.curseforge_mod_page_open))
}

pub(super) struct PageOverlayView {
    route: AppRoute,
    was_visible: bool,
    _subscriptions: Vec<Subscription>,
}

impl PageOverlayView {
    pub(super) fn new(route: AppRoute, cx: &mut Context<Self>) -> Self {
        Self {
            route,
            was_visible: false,
            _subscriptions: vec![
                cx.observe_global::<SettingsPageState>(|this, cx| {
                    if this.route == AppRoute::Settings {
                        this.notify_if_visible(cx);
                    }
                }),
                cx.observe_global::<DownloadPageState>(|this, cx| {
                    if this.route == AppRoute::Download {
                        this.notify_if_visible(cx);
                    }
                }),
                cx.observe_global::<crate::ui::views::manage::state::ManagePageState>(
                    |this, cx| {
                        if this.route == AppRoute::Download {
                            this.notify_if_visible(cx);
                        }
                    },
                ),
                cx.observe_global::<LocalVersionsState>(|this, cx| {
                    if this.route == AppRoute::Download {
                        this.notify_if_visible(cx);
                    }
                }),
                cx.observe_global::<AgreementState>(|this, cx| {
                    if this.route == AppRoute::Settings {
                        this.notify_if_visible(cx);
                    }
                }),
                cx.observe_global::<ThemeState>(Self::notify_if_visible),
                cx.observe_global::<I18n>(Self::notify_if_visible),
                cx.observe_global::<gpui_router::RouterState>(Self::notify_if_visible),
            ],
        }
    }

    fn visible(&self, cx: &App) -> bool {
        crate::ui::navigation::current_route(cx) == self.route
            && match self.route {
                AppRoute::Settings => settings_visible(cx.global::<SettingsPageState>()),
                AppRoute::Download => download_visible(cx.global::<DownloadPageState>()),
                _ => false,
            }
    }

    fn notify_if_visible(&mut self, cx: &mut Context<Self>) {
        let visible = self.visible(cx);
        if visible || self.was_visible {
            cx.notify();
        }
        self.was_visible = visible;
    }
}

impl Render for PageOverlayView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.was_visible = self.visible(cx);
        if !self.was_visible {
            return Empty {}.into_any_element();
        }
        let now = window.animation_time();
        let theme = cx.global::<ThemeState>();
        let factor = theme.factor(now);
        let accent = theme.accent;
        let colors = lerp_theme_colors(
            &LightColors::colors(),
            &DarkColors::colors(),
            factor,
            accent,
        );
        crate::ui::animation::request_layout_animation_frame_if(window, theme.is_animating(now));
        let overlay = match self.route {
            AppRoute::Settings => crate::ui::views::settings::render_settings_overlay(
                &colors,
                window.bounds().size.width,
                window.bounds().size.height,
                factor,
                accent,
                cx.global::<I18n>(),
                cx.global::<SettingsPageState>(),
                cx.global::<AgreementState>().cached_document(),
            ),
            AppRoute::Download => {
                crate::ui::views::download::render_download_overlay(&colors, window, cx)
            }
            _ => None,
        };
        div()
            .absolute()
            .inset_0()
            .children(overlay)
            .into_any_element()
    }
}
