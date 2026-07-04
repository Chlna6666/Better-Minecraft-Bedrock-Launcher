use super::*;

impl MainWindowView {
    pub(super) fn sync_page_active_flags(&mut self, route: &RouteTarget, cx: &mut Context<Self>) {
        let home_page_active = matches!(route, RouteTarget::Builtin(AppRoute::Home));
        if let Some(view) = &self.home_page_view {
            let _ = view.update(cx, |view, cx| {
                view.set_active(home_page_active, cx);
                Ok::<(), anyhow::Error>(())
            });
        }

        let download_page_active = matches!(route, RouteTarget::Builtin(AppRoute::Download));
        if let Some(view) = &self.download_page_view {
            let _ = view.update(cx, |view, cx| {
                view.set_active(download_page_active, cx);
                Ok::<(), anyhow::Error>(())
            });
        }

        let tasks_page_active = matches!(route, RouteTarget::Builtin(AppRoute::Tasks));
        if let Some(view) = &self.tasks_page_view {
            let _ = view.update(cx, |view, cx| {
                view.set_active(tasks_page_active, cx);
                Ok::<(), anyhow::Error>(())
            });
        }
    }

    pub(super) fn ensure_page_view_for_route(&mut self, route: AppRoute, cx: &mut Context<Self>) {
        match route {
            AppRoute::Home => {
                let _ = get_or_create_page_view(&mut self.home_page_view, cx, |cx| {
                    cx.new(|cx| crate::ui::views::home::HomePageView::new(None, cx))
                });
            }
            AppRoute::Download => {
                let _ = get_or_create_page_view(&mut self.download_page_view, cx, |cx| {
                    cx.new(crate::ui::views::download::DownloadPageView::new)
                });
            }
            AppRoute::Manage => {
                let _ = get_or_create_page_view(&mut self.manage_page_view, cx, |cx| {
                    cx.new(crate::ui::views::manage::ManagePageView::new)
                });
            }
            AppRoute::Tools => {
                let _ = get_or_create_page_view(&mut self.tools_page_view, cx, |cx| {
                    cx.new(crate::ui::views::tools::ToolsPageView::new)
                });
            }
            AppRoute::Tasks => {
                let _ = get_or_create_page_view(&mut self.tasks_page_view, cx, |cx| {
                    cx.new(crate::ui::views::tasks::TasksPageView::new)
                });
            }
            AppRoute::Settings => {
                let _ = get_or_create_page_view(&mut self.settings_page_view, cx, |cx| {
                    cx.new(crate::ui::views::settings::SettingsPageView::new)
                });
            }
        }
    }

    pub(super) fn prewarm_page_view_for_route(&mut self, route: AppRoute, cx: &mut Context<Self>) {
        self.ensure_page_view_for_route(route, cx);
        let current_route = crate::ui::navigation::current_route_target(cx);
        self.sync_page_active_flags(&current_route, cx);
    }

    pub(super) fn ensure_page_view_for_target(
        &mut self,
        target: &RouteTarget,
        cx: &mut Context<Self>,
    ) {
        match target {
            RouteTarget::Builtin(route) => {
                self.plugin_page_view = None;
                self.plugin_page_key = None;
                self.ensure_page_view_for_route(*route, cx);
            }
            RouteTarget::Plugin { plugin_id, page_id } => {
                let key = (plugin_id.clone(), page_id.clone());
                if self.plugin_page_key.as_ref() != Some(&key) {
                    self.plugin_page_view = Some(cx.new(|cx| {
                        crate::ui::views::plugin::PluginPageView::new(
                            plugin_id.clone(),
                            page_id.clone(),
                            cx,
                        )
                    }));
                    self.plugin_page_key = Some(key);
                }
            }
        }
    }

    pub(super) fn install_reactors(&mut self, cx: &mut Context<Self>) {
        self._route_subscriptions
            .push(cx.observe_global::<gpui_router::RouterState>(|this, cx| {
                let route = crate::ui::navigation::current_route_target(cx);
                this.handle_route_change_without_window(route, cx);
                cx.notify();
            }));

        self._reactor_subscriptions
            .push(cx.observe_global::<UpdateState>(|this, cx| {
                let now = Instant::now();
                let changed = this.sync_update_state(now, cx)
                    | this.ensure_update_download_listener(cx)
                    | this.sync_current_background_animation_policy(now, cx);
                let update_state = cx.global::<UpdateState>();
                let visible_update_ui_changed = update_state.available.is_some()
                    || update_state.checking
                    || update_state.last_error.is_some()
                    || update_state.downloading
                    || update_state.show_modal
                    || update_state.modal_pending_open
                    || update_state.is_modal_animating(now);
                if changed || visible_update_ui_changed {
                    cx.notify();
                }
            }));
        self._reactor_subscriptions
            .push(cx.observe_global::<gpui_router::RouterState>(|this, cx| {
                if this.sync_current_background_animation_policy(Instant::now(), cx) {
                    cx.notify();
                }
            }));
        self._reactor_subscriptions
            .push(cx.observe_global::<DebugState>(|this, cx| {
                if this.sync_current_background_animation_policy(Instant::now(), cx) {
                    cx.notify();
                }
            }));
        self._reactor_subscriptions
            .push(cx.observe_global::<ThemeState>(|_this, cx| {
                cx.notify();
            }));
        self._reactor_subscriptions
            .push(cx.observe_global::<LauncherState>(|_this, cx| {
                let now = Instant::now();
                let state = cx.global::<LauncherState>();
                if state.show_modal || state.modal_visible || state.is_modal_animating(now) {
                    cx.notify();
                }
            }));
        self._reactor_subscriptions.push(
            cx.observe_global::<crate::ui::state::launch_prereq::LaunchPrereqState>(|_this, cx| {
                let state = cx.global::<crate::ui::state::launch_prereq::LaunchPrereqState>();
                if state.visible || state.is_busy() {
                    cx.notify();
                }
            }),
        );
        self._reactor_subscriptions
            .push(
                cx.observe_global::<crate::ui::state::quit::QuitState>(|_this, cx| {
                    cx.notify();
                }),
            );
        self._reactor_subscriptions.push(
            cx.observe_global::<crate::ui::state::diagnostics::DiagnosticsState>(|_this, cx| {
                cx.notify();
            }),
        );
        self._reactor_subscriptions.push(
            cx.observe_global::<crate::ui::components::toast::ToastState>(|_this, cx| {
                let now = Instant::now();
                let state = cx.global::<crate::ui::components::toast::ToastState>();
                if crate::ui::components::toast::has_visible_toasts(now, state)
                    || crate::ui::components::toast::has_visible_breadcrumb(now, state)
                {
                    cx.notify();
                }
            }),
        );
        self._reactor_subscriptions.push(
            cx.observe_global::<crate::ui::components::dropdown::DropdownOverlayState>(
                |_this, cx| {
                    let now = Instant::now();
                    let state =
                        cx.global::<crate::ui::components::dropdown::DropdownOverlayState>();
                    if crate::ui::components::dropdown::has_visible_overlay(now, state) {
                        cx.notify();
                    }
                },
            ),
        );
        self._reactor_subscriptions.push(
            cx.observe_global::<crate::plugins::runtime::PluginRegistry>(|_this, cx| {
                let route = crate::ui::navigation::current_route_target(cx);
                let plugin_page_active = matches!(route, RouteTarget::Plugin { .. });
                if plugin_page_active || crate::plugins::runtime::active_modal(cx).is_some() {
                    cx.notify();
                }
            }),
        );
        self._reactor_subscriptions.push(
            cx.observe_global::<crate::ui::views::settings::state::SettingsPageState>(
                |_this, cx| {
                    if crate::ui::navigation::current_route(cx) == AppRoute::Settings {
                        cx.notify();
                    }
                },
            ),
        );
        self._reactor_subscriptions
            .push(cx.observe_global::<AgreementState>(|_this, cx| {
                if cx.global::<AgreementState>().is_visible()
                    || crate::ui::navigation::current_route(cx) == AppRoute::Settings
                {
                    cx.notify();
                }
            }));
        self._reactor_subscriptions
            .push(cx.observe_global::<I18n>(|_this, cx| {
                let locale_code = cx.global::<I18n>().locale().code().to_string();
                let agreement_visible = cx.update_global(|agreement: &mut AgreementState, _cx| {
                    let _ = agreement.get_or_cache_document(&locale_code);
                    agreement.is_visible()
                });
                if agreement_visible
                    || crate::ui::navigation::current_route(cx) == AppRoute::Settings
                {
                    cx.notify();
                }
            }));
    }

    pub(super) fn install_window_observers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._window_subscriptions
            .push(cx.observe_window_bounds(window, |this, window, _cx| {
                this.maybe_trim_working_set_on_minimize(window);
            }));
        self._window_subscriptions
            .push(cx.observe_window_activation(window, |this, window, cx| {
                this.maybe_trim_working_set_on_minimize(window);
                if this.sync_current_background_animation_policy(Instant::now(), cx) {
                    cx.notify();
                }
            }));
        self.maybe_trim_working_set_on_minimize(window);
    }

    pub(super) fn release_download_page(&mut self, cx: &mut Context<Self>) {
        let has_page_resources = self.download_page_view.is_some()
            || self.download_controls_initialized
            || !self.download_controls_subscriptions.is_empty()
            || self.download_prefs_last_save.is_some();
        let has_route_state = cx.read_global(
            |state: &crate::ui::views::download::state::DownloadPageState, _cx| {
                state.has_releasable_route_state()
            },
        );
        if !has_page_resources && !has_route_state {
            return;
        }

        clear_optional_page_view(&mut self.download_page_view);
        self.download_controls_initialized = false;
        self.download_controls_subscriptions.clear();
        self.download_prefs_last_save = None;
        if !has_route_state {
            return;
        }

        cx.update_global(
            |state: &mut crate::ui::views::download::state::DownloadPageState, cx| {
                let mods_before = state.curseforge_mods.len();
                let categories_before = state.curseforge_categories.len();
                let versions_before = state.curseforge_versions.len();
                let game_versions_before = state.versions.len();
                state.release_curseforge_tab_state(cx);
                state.search_input = None;
                state.page_jump_input = None;
                state.search_query = SharedString::from("");
                state.page_index = 0;
                state.tab = crate::ui::views::download::state::DownloadTab::Game;
                state.tab_anim_at = None;
                state.force_refresh_next = false;
                trace!(
                    "release download page game_versions={} mods={} categories={} versions={}",
                    game_versions_before, mods_before, categories_before, versions_before
                );
            },
        );
    }

    pub(super) fn release_manage_page(&mut self, cx: &mut Context<Self>) {
        let has_page_resources = self.manage_page_view.is_some()
            || self.manage_controls_initialized
            || !self.manage_controls_subscriptions.is_empty();
        let has_route_state = cx.read_global(
            |state: &crate::ui::views::manage::state::ManagePageState, _cx| {
                state.search_input.is_some()
            },
        );
        if !has_page_resources && !has_route_state {
            return;
        }

        clear_optional_page_view(&mut self.manage_page_view);
        self.manage_controls_initialized = false;
        self.manage_controls_subscriptions.clear();
        if !has_route_state {
            return;
        }

        cx.update_global(
            |state: &mut crate::ui::views::manage::state::ManagePageState, _cx| {
                state.search_input = None;
            },
        );
    }

    pub(super) fn release_tools_page(&mut self, cx: &mut Context<Self>) {
        let has_page_resources = self.tools_page_view.is_some()
            || self.tools_controls_initialized
            || !self.tools_controls_subscriptions.is_empty();
        let has_route_state = cx.read_global(
            |state: &crate::ui::views::tools::state::ToolsPageState, _cx| {
                state.room_code_input.is_some()
                    || state.bootstrap_peers_input.is_some()
                    || state.player_name_input.is_some()
                    || state.game_ports_input.is_some()
            },
        );
        if !has_page_resources && !has_route_state {
            return;
        }

        clear_optional_page_view(&mut self.tools_page_view);
        self.tools_controls_initialized = false;
        self.tools_controls_subscriptions.clear();
        if !has_route_state {
            return;
        }

        cx.update_global(
            |state: &mut crate::ui::views::tools::state::ToolsPageState, _cx| {
                state.room_code_input = None;
                state.bootstrap_peers_input = None;
                state.player_name_input = None;
                state.game_ports_input = None;
            },
        );
    }

    pub(super) fn release_settings_page(&mut self, cx: &mut Context<Self>) {
        let has_page_resources = self.settings_page_view.is_some()
            || self.settings_controls_initialized
            || !self.settings_controls_subscriptions.is_empty()
            || self.settings_load_started;
        let has_route_state = cx.read_global(
            |state: &crate::ui::views::settings::state::SettingsPageState, _cx| {
                state.has_releasable_route_state()
            },
        );
        if !has_page_resources && !has_route_state {
            return;
        }

        clear_optional_page_view(&mut self.settings_page_view);
        self.settings_controls_initialized = false;
        self.settings_controls_subscriptions.clear();
        self.settings_load_started = false;
        if !has_route_state {
            return;
        }

        let custom_style = cx.update_global(
            |state: &mut crate::ui::views::settings::state::SettingsPageState, _cx| {
                let custom_style = if state.commit_background_blur_preview() {
                    Some((
                        state.theme_color.to_string(),
                        state.background_option.to_string(),
                        state.local_image_path.to_string(),
                        state.network_image_url.to_string(),
                        state.background_blur,
                        state.show_launch_animation,
                        state.font_source.to_string(),
                        state.local_font_path.to_string(),
                        state.local_font_family.to_string(),
                        state.system_font_family.to_string(),
                    ))
                } else {
                    None
                };
                state.release_route_state();
                custom_style
            },
        );
        if let Some((
            theme,
            option,
            local,
            network,
            blur,
            anim,
            font_source,
            local_font_path,
            local_font_family,
            system_font_family,
        )) = custom_style
        {
            self.persist_settings_custom_style(
                theme,
                option,
                local,
                network,
                blur,
                anim,
                font_source,
                local_font_path,
                local_font_family,
                system_font_family,
                cx,
            );
        }
    }

    pub(super) fn release_home_page(&mut self) {
        clear_optional_page_view(&mut self.home_page_view);
    }

    pub(super) fn release_tasks_page(&mut self) {
        clear_optional_page_view(&mut self.tasks_page_view);
    }

    pub(super) fn release_plugin_page(&mut self) {
        clear_optional_page_view(&mut self.plugin_page_view);
        self.plugin_page_key = None;
    }

    pub(super) fn release_inactive_route_resources(
        &mut self,
        active_route: AppRoute,
        cx: &mut Context<Self>,
    ) {
        cx.update_global(
            |state: &mut crate::ui::components::dropdown::DropdownOverlayState, _cx| {
                state.clear();
            },
        );

        match active_route {
            AppRoute::Home => {
                self.release_download_page(cx);
                self.release_manage_page(cx);
                self.release_tools_page(cx);
                self.release_settings_page(cx);
                self.release_tasks_page();
                self.release_plugin_page();
            }
            AppRoute::Download => {
                self.release_home_page();
                self.release_manage_page(cx);
                self.release_tools_page(cx);
                self.release_settings_page(cx);
                self.release_tasks_page();
                self.release_plugin_page();
            }
            AppRoute::Manage => {
                self.release_home_page();
                self.release_download_page(cx);
                self.release_tools_page(cx);
                self.release_settings_page(cx);
                self.release_tasks_page();
                self.release_plugin_page();
            }
            AppRoute::Tools => {
                self.release_home_page();
                self.release_download_page(cx);
                self.release_manage_page(cx);
                self.release_settings_page(cx);
                self.release_tasks_page();
                self.release_plugin_page();
            }
            AppRoute::Tasks => {
                self.release_home_page();
                self.release_manage_page(cx);
                self.release_tools_page(cx);
                self.release_settings_page(cx);
                self.release_plugin_page();
            }
            AppRoute::Settings => {
                self.release_home_page();
                self.release_download_page(cx);
                self.release_manage_page(cx);
                self.release_tools_page(cx);
                self.release_tasks_page();
                self.release_plugin_page();
            }
        }
    }

    pub(super) fn release_inactive_target_resources(
        &mut self,
        active_target: &RouteTarget,
        cx: &mut Context<Self>,
    ) {
        match active_target {
            RouteTarget::Builtin(route) => self.release_inactive_route_resources(*route, cx),
            RouteTarget::Plugin { .. } => {
                cx.update_global(
                    |state: &mut crate::ui::components::dropdown::DropdownOverlayState, _cx| {
                        state.clear();
                    },
                );
                self.release_home_page();
                self.release_download_page(cx);
                self.release_manage_page(cx);
                self.release_tools_page(cx);
                self.release_settings_page(cx);
                self.release_tasks_page();
            }
        }
    }

    fn cache_agreement_document(cx: &mut Context<Self>) {
        let locale_code = cx.global::<I18n>().locale().code().to_string();
        cx.update_global(|agreement: &mut AgreementState, _cx| {
            let _document = agreement.get_or_cache_document(&locale_code);
        });
    }

    fn schedule_startup_deferred_work(
        &mut self,
        startup_check_updates: bool,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |handle, cx| {
            Timer::after(STARTUP_ROUTE_BOOTSTRAP_DELAY).await;

            match handle.update(cx, |this, cx| {
                this.startup_deferred_ready = true;
                info!(
                    "startup_trace: deferred_bootstrap t={:.3}ms phase=main_window",
                    startup_trace_elapsed_ms()
                );
                this.ensure_startup_route_bootstrapped(cx);
                this.ensure_music_library_load_started(cx);
                Self::cache_agreement_document(cx);

                let now = Instant::now();
                let changed = this.sync_update_state(now, cx)
                    | this.ensure_update_download_listener(cx)
                    | this.sync_current_background_animation_policy(now, cx);
                if changed {
                    tracing::debug!("startup deferred work changed render state");
                }
                cx.notify();
            }) {
                Ok(()) => {}
                Err(error) => {
                    warn!("startup deferred work failed: {error:?}");
                    return Ok::<(), anyhow::Error>(());
                }
            }

            Timer::after(STARTUP_INTERACTION_WARMUP_DELAY).await;
            if let Err(error) = handle.update(cx, |this, cx| {
                let preloaded = crate::ui::views::settings::preload_static_assets(cx);
                this.prewarm_page_view_for_route(AppRoute::Settings, cx);
                tracing::debug!(
                    "startup interaction warmup scheduled: settings_images={} settings_view=true",
                    preloaded
                );
            }) {
                warn!("startup interaction warmup failed: {error:?}");
            }

            Timer::after(STARTUP_INTERACTION_WARMUP_STEP_DELAY).await;
            if let Err(error) = handle.update(cx, |this, cx| {
                this.prewarm_page_view_for_route(AppRoute::Tasks, cx);
                tracing::debug!("startup interaction warmup scheduled: tasks_view=true");
            }) {
                warn!("startup tasks warmup failed: {error:?}");
            }

            if startup_check_updates {
                Timer::after(STARTUP_UPDATE_CHECK_DELAY).await;
                if let Err(error) = handle.update(cx, |_this, cx| {
                    MainWindowView::spawn_startup_update_check(true, cx);
                }) {
                    warn!("startup update check scheduling failed: {error:?}");
                }
            }

            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub fn new(startup_check_updates: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = cx.global::<crate::ui::views::settings::state::SettingsPageState>();
        let background_option = settings.background_option.clone();
        let local_image_path = settings.local_image_path.clone();
        let network_image_url = settings.network_image_url.clone();

        let background_view = cx.new(|cx| {
            background::AppBackgroundView::new(
                background_option,
                local_image_path,
                network_image_url,
                cx,
            )
        });

        let mut this = Self {
            background_view,
            chrome_view: None,
            music_library_load_started: false,
            home_page_view: None,
            download_page_view: None,
            manage_page_view: None,
            tools_page_view: None,
            settings_page_view: None,
            tasks_page_view: None,
            plugin_page_view: None,
            plugin_page_key: None,
            download_controls_initialized: false,
            download_controls_subscriptions: Vec::new(),
            download_prefs_last_save: None,
            download_curseforge_invalidate_seq_seen: 0,
            download_curseforge_invalidate_pending_seen: false,
            manage_controls_initialized: false,
            manage_controls_subscriptions: Vec::new(),
            tools_controls_initialized: false,
            tools_controls_subscriptions: Vec::new(),
            settings_controls_initialized: false,
            settings_controls_subscriptions: Vec::new(),
            _route_subscriptions: Vec::new(),
            _reactor_subscriptions: Vec::new(),
            _window_subscriptions: Vec::new(),
            update_download_listener_running: false,
            update_markdown_view: None,
            settings_load_started: false,
            background_animation_suppressed: false,
            last_route_for_side_effects: None,
            runtime_font_logged: false,
            startup_route_bootstrapped: false,
            startup_deferred_ready: false,
            was_window_minimized: false,
        };

        this.install_reactors(cx);
        this.install_window_observers(window, cx);

        if cx.global::<AgreementState>().is_visible() {
            Self::cache_agreement_document(cx);
        }
        this.schedule_startup_deferred_work(startup_check_updates, cx);

        this
    }

    pub(super) fn notify_download_page(&self, cx: &mut App) {
        if let Some(view) = &self.download_page_view {
            notify_view(view, cx);
        }
    }

    pub(super) fn notify_manage_page(&self, cx: &mut App) {
        if let Some(view) = &self.manage_page_view {
            notify_view(view, cx);
        }
    }

    pub(super) fn notify_tools_page(&self, cx: &mut App) {
        if let Some(view) = &self.tools_page_view {
            notify_view(view, cx);
        }
    }

    pub(super) fn notify_settings_page(&self, cx: &mut App) {
        if let Some(view) = &self.settings_page_view {
            notify_view(view, cx);
        }
    }

    pub(super) fn notify_home_page(&self, cx: &mut App) {
        if let Some(view) = &self.home_page_view {
            notify_view(view, cx);
        }
    }

    pub(super) fn notify_tasks_page(&self, cx: &mut App) {
        if let Some(view) = &self.tasks_page_view {
            notify_view(view, cx);
        }
    }

    pub(super) fn notify_all_page_views(&self, cx: &mut App) {
        self.notify_home_page(cx);
        self.notify_download_page(cx);
        self.notify_manage_page(cx);
        self.notify_tools_page(cx);
        self.notify_settings_page(cx);
        self.notify_tasks_page(cx);
        if let Some(view) = &self.plugin_page_view {
            notify_view(view, cx);
        }
    }
}
