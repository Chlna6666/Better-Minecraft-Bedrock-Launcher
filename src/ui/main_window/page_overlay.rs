use super::*;
use crate::ui::state::local_versions::LocalVersionsState;
use crate::ui::views::download::state::{DownloadPageState, DownloadTab};
use crate::ui::views::settings::state::{SettingsPageState, SettingsTab};
use std::hash::{Hash, Hasher};

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
    settings_projection: u64,
    download_projection: u64,
    _subscriptions: Vec<Subscription>,
}

impl PageOverlayView {
    pub(super) fn new(route: AppRoute, cx: &mut Context<Self>) -> Self {
        Self {
            route,
            was_visible: false,
            settings_projection: settings_projection(
                cx.global::<SettingsPageState>(),
                cx.global::<AgreementState>(),
            ),
            download_projection: download_projection(
                cx.global::<DownloadPageState>(),
                cx.global::<crate::ui::views::manage::state::ManagePageState>(),
                cx.global::<LocalVersionsState>(),
            ),
            _subscriptions: vec![
                cx.observe_global::<SettingsPageState>(|this, cx| {
                    if this.route == AppRoute::Settings {
                        let next = settings_projection(
                            cx.global::<SettingsPageState>(),
                            cx.global::<AgreementState>(),
                        );
                        if this.settings_projection != next {
                            this.settings_projection = next;
                            this.notify_if_visible(cx);
                        }
                    }
                }),
                cx.observe_global::<DownloadPageState>(|this, cx| {
                    if this.route == AppRoute::Download {
                        this.notify_download_if_changed(cx);
                    }
                }),
                cx.observe_global::<crate::ui::views::manage::state::ManagePageState>(
                    |this, cx| {
                        if this.route == AppRoute::Download {
                            this.notify_download_if_changed(cx);
                        }
                    },
                ),
                cx.observe_global::<LocalVersionsState>(|this, cx| {
                    if this.route == AppRoute::Download {
                        this.notify_download_if_changed(cx);
                    }
                }),
                cx.observe_global::<AgreementState>(|this, cx| {
                    if this.route == AppRoute::Settings {
                        let next = settings_projection(
                            cx.global::<SettingsPageState>(),
                            cx.global::<AgreementState>(),
                        );
                        if this.settings_projection != next {
                            this.settings_projection = next;
                            this.notify_if_visible(cx);
                        }
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

    fn notify_download_if_changed(&mut self, cx: &mut Context<Self>) {
        let next = download_projection(
            cx.global::<DownloadPageState>(),
            cx.global::<crate::ui::views::manage::state::ManagePageState>(),
            cx.global::<LocalVersionsState>(),
        );
        if self.download_projection != next {
            self.download_projection = next;
            self.notify_if_visible(cx);
        }
    }
}

fn settings_projection(state: &SettingsPageState, agreement: &AgreementState) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    (state.tab as u8).hash(&mut hasher);
    state.font_restart_confirm_open.hash(&mut hasher);
    state.launcher_connectivity_open.hash(&mut hasher);
    if state.launcher_connectivity_open && state.tab == SettingsTab::Launcher {
        state.launcher_connectivity_running.hash(&mut hasher);
        state.launcher_connectivity_req_id.hash(&mut hasher);
        for item in &state.launcher_connectivity_items {
            item.group_index.hash(&mut hasher);
            item.item_index.hash(&mut hasher);
            item.name.hash(&mut hasher);
            item.url.hash(&mut hasher);
            (item.status as u8).hash(&mut hasher);
            item.latency_ms.hash(&mut hasher);
            item.error.hash(&mut hasher);
        }
    }
    state.about_sponsors_open.hash(&mut hasher);
    if state.about_sponsors_open && state.tab == SettingsTab::About {
        state.about_sponsors_loading.hash(&mut hasher);
        state.about_sponsors_error.hash(&mut hasher);
        state.about_sponsors_req_id.hash(&mut hasher);
        state.about_sponsors_page.hash(&mut hasher);
        state.about_sponsors_page_size.hash(&mut hasher);
        state.about_sponsors_skeleton_phase.hash(&mut hasher);
        for sponsor in &state.about_sponsors {
            sponsor.user_id.hash(&mut hasher);
            sponsor.name.hash(&mut hasher);
            sponsor.avatar_url.hash(&mut hasher);
            sponsor.all_sum_amount.hash(&mut hasher);
        }
    }
    state.about_dependencies_open.hash(&mut hasher);
    state.about_agreement_open.hash(&mut hasher);
    if state.about_agreement_open && state.tab == SettingsTab::About {
        agreement.accepted.hash(&mut hasher);
    }
    hasher.finish()
}

fn download_projection(
    state: &DownloadPageState,
    manage: &crate::ui::views::manage::state::ManagePageState,
    local_versions: &LocalVersionsState,
) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    if let Some(dialog) = &state.game_dialog {
        1_u8.hash(&mut hasher);
        (dialog.kind as u8).hash(&mut hasher);
        dialog.version.hash(&mut hasher);
        dialog.package_id.hash(&mut hasher);
        dialog.version_type.hash(&mut hasher);
        dialog.md5.hash(&mut hasher);
        dialog.is_gdk.hash(&mut hasher);
        dialog.file_name.hash(&mut hasher);
        dialog.local_path.hash(&mut hasher);
        state.game_dialog_cdn_loading.hash(&mut hasher);
        state.game_dialog_cdn_request_id.hash(&mut hasher);
        state.game_dialog_cdn_error.hash(&mut hasher);
        state.game_dialog_selected_cdn_base.hash(&mut hasher);
        state.game_dialog_cdn_expanded.hash(&mut hasher);
        state.game_dialog_install_levilamina.hash(&mut hasher);
        state
            .game_dialog_selected_levilamina_version
            .hash(&mut hasher);
        for result in &state.game_dialog_cdn_results {
            result.base.hash(&mut hasher);
            result.url.hash(&mut hasher);
            result.latency_ms.hash(&mut hasher);
            result.error.hash(&mut hasher);
        }
    } else if state.levilauncher_modal_open {
        2_u8.hash(&mut hasher);
        if let Some(mod_entry) = &state.levilauncher_selected_mod {
            mod_entry.package_id.hash(&mut hasher);
            mod_entry.name.hash(&mut hasher);
            mod_entry.description.hash(&mut hasher);
        }
        state.levilauncher_selected_version.hash(&mut hasher);
        state.levilauncher_install_target_path.hash(&mut hasher);
        state.levilauncher_install_busy.hash(&mut hasher);
        state.levilauncher_install_targets_loading.hash(&mut hasher);
        state
            .levilauncher_install_targets_request_id
            .hash(&mut hasher);
        state.levilauncher_install_error.hash(&mut hasher);
    } else if state.native_mod_modal_open {
        3_u8.hash(&mut hasher);
        if let Some(mod_entry) = &state.native_mod_selected {
            mod_entry.id.hash(&mut hasher);
            mod_entry.name.hash(&mut hasher);
            mod_entry.description.hash(&mut hasher);
        }
        state.native_mod_selected_file.hash(&mut hasher);
        state.native_mod_target_path.hash(&mut hasher);
        state.native_mod_install_busy.hash(&mut hasher);
        state.native_mod_install_error.hash(&mut hasher);
    } else if state.tab == DownloadTab::ResourcePack
        && (state.curseforge_install_open || state.curseforge_mod_page_open)
    {
        4_u8.hash(&mut hasher);
        state.curseforge_install_open.hash(&mut hasher);
        (state.curseforge_install_stage as u8).hash(&mut hasher);
        state.curseforge_install_error.hash(&mut hasher);
        state.curseforge_install_files_request_id.hash(&mut hasher);
        state.curseforge_install_selected_file_id.hash(&mut hasher);
        state.curseforge_install_target_folder.hash(&mut hasher);
        for file in &state.curseforge_install_files {
            file.id.hash(&mut hasher);
            file.display_name.hash(&mut hasher);
            file.file_name.hash(&mut hasher);
        }
        state.curseforge_mod_page_open.hash(&mut hasher);
        state.curseforge_mod_page_loading.hash(&mut hasher);
        state.curseforge_mod_page_request_id.hash(&mut hasher);
        state.curseforge_mod_page_mod_id.hash(&mut hasher);
        state.curseforge_mod_page_error.hash(&mut hasher);
        state.curseforge_mod_page_description_len.hash(&mut hasher);
        if let Some(mod_entry) = &state.curseforge_mod_page_mod {
            mod_entry.id.hash(&mut hasher);
            mod_entry.name.hash(&mut hasher);
            mod_entry.summary.hash(&mut hasher);
        }
        manage.selected_folder.hash(&mut hasher);
        manage.versions_revision.hash(&mut hasher);
        local_versions.snapshot_revision.hash(&mut hasher);
    }
    hasher.finish()
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
