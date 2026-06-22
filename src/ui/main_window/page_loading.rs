use super::*;

impl MainWindowView {
    pub(super) fn ensure_route_page_data_loaded(
        &mut self,
        route: AppRoute,
        cx: &mut Context<Self>,
    ) {
        match route {
            AppRoute::Download => self.ensure_download_route_data_loaded(cx),
            AppRoute::Manage => self.ensure_manage_page_loaded(cx),
            AppRoute::Home | AppRoute::Tools | AppRoute::Tasks | AppRoute::Settings => {}
        }
    }

    fn ensure_download_route_data_loaded(&mut self, cx: &mut Context<Self>) {
        let download_state: &crate::ui::views::download::state::DownloadPageState =
            cx.global::<crate::ui::views::download::state::DownloadPageState>();
        let force_refresh = download_state.force_refresh_next;
        let tab = download_state.tab;

        self.ensure_download_page_loaded(force_refresh, cx);

        if tab == crate::ui::views::download::state::DownloadTab::ResourcePack {
            self.ensure_manage_page_loaded(cx);
            self.ensure_curseforge_loaded(cx);
            self.ensure_curseforge_results_loaded(false, cx);
        }
    }

    pub(super) fn ensure_manage_page_loaded(&mut self, cx: &mut Context<Self>) {
        crate::ui::hooks::use_local_versions::ensure_local_versions_loaded(false, cx);
        self.notify_manage_page(cx);
    }

    pub(super) fn ensure_settings_loaded(&mut self, cx: &mut Context<Self>) {
        if self.settings_load_started {
            return;
        }
        self.settings_load_started = true;

        let settings_ready = cx.update_global(
            |state: &mut crate::ui::views::settings::state::SettingsPageState, _cx| {
                if state.loaded {
                    return true;
                }
                tracing::warn!("settings state was not initialized from startup configuration");
                state.loaded = true;
                true
            },
        );

        if settings_ready {
            if let Some(view) = &self.settings_page_view {
                notify_view(view, cx);
            }
            cx.notify();
        }
    }
}
