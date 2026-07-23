use super::*;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct ManageRenderSignature {
    pub(super) tab: ManageTab,
    pub(super) loaded: bool,
    pub(super) loading: bool,
    pub(super) error: Option<SharedString>,
    pub(super) versions_ptr: usize,
    pub(super) versions_len: usize,
    pub(super) selected_folder: Option<SharedString>,
    pub(super) search_query: SharedString,
    pub(super) asset_search_query: SharedString,
    pub(super) pack_subtype: ManagePackSubtype,
    pub(super) asset_sort_key: ManageAssetSortKey,
    pub(super) asset_sort_desc: bool,
    pub(super) selected_asset_count: usize,
    pub(super) version_config: ManageVersionConfig,
    pub(super) version_config_loading: bool,
    pub(super) version_config_error: Option<SharedString>,
    pub(super) version_config_request_id: u64,
    pub(super) gdk_users_ptr: usize,
    pub(super) gdk_users_len: usize,
    pub(super) selected_gdk_user: Option<SharedString>,
    pub(super) gdk_users_loading: bool,
    pub(super) gdk_users_error: Option<SharedString>,
    pub(super) gdk_users_request_id: u64,
    pub(super) assets_ptr: usize,
    pub(super) assets_len: usize,
    pub(super) assets_loaded: bool,
    pub(super) assets_loading: bool,
    pub(super) assets_error: Option<SharedString>,
    pub(super) assets_request_id: u64,
    pub(super) screenshot_search_query: SharedString,
    pub(super) screenshots_ptr: usize,
    pub(super) screenshots_len: usize,
    pub(super) screenshots_loaded: bool,
    pub(super) screenshots_loading: bool,
    pub(super) screenshots_error: Option<SharedString>,
    pub(super) screenshots_request_id: u64,
    pub(super) server_search_query: SharedString,
    pub(super) servers_ptr: usize,
    pub(super) servers_len: usize,
    pub(super) servers_loaded: bool,
    pub(super) servers_loading: bool,
    pub(super) servers_error: Option<SharedString>,
    pub(super) servers_request_id: u64,
    pub(super) server_motd_ptr: usize,
    pub(super) server_motd_len: usize,
    pub(super) server_motd_loading: bool,
    pub(super) server_motd_request_id: u64,
}

impl ManageRenderSignature {
    pub(super) fn from_state(state: &ManagePageState) -> Self {
        Self {
            tab: state.tab,
            loaded: state.loaded,
            loading: state.loading,
            error: state.error.clone(),
            versions_ptr: state.versions.as_ptr() as usize,
            versions_len: state.versions.len(),
            selected_folder: state.selected_folder.clone(),
            search_query: state.search_query.clone(),
            asset_search_query: state.asset_search_query.clone(),
            pack_subtype: state.pack_subtype,
            asset_sort_key: state.asset_sort_key,
            asset_sort_desc: state.asset_sort_desc,
            selected_asset_count: state.selected_asset_keys.len(),
            version_config: state.version_config.clone(),
            version_config_loading: state.version_config_loading,
            version_config_error: state.version_config_error.clone(),
            version_config_request_id: state.version_config_request_id,
            gdk_users_ptr: state.gdk_users.as_ptr() as usize,
            gdk_users_len: state.gdk_users.len(),
            selected_gdk_user: state.selected_gdk_user.clone(),
            gdk_users_loading: state.gdk_users_loading,
            gdk_users_error: state.gdk_users_error.clone(),
            gdk_users_request_id: state.gdk_users_request_id,
            assets_ptr: state.assets.as_ptr() as usize,
            assets_len: state.assets.len(),
            assets_loaded: state.assets_loaded,
            assets_loading: state.assets_loading,
            assets_error: state.assets_error.clone(),
            assets_request_id: state.assets_request_id,
            screenshot_search_query: state.screenshot_search_query.clone(),
            screenshots_ptr: state.screenshots.as_ptr() as usize,
            screenshots_len: state.screenshots.len(),
            screenshots_loaded: state.screenshots_loaded,
            screenshots_loading: state.screenshots_loading,
            screenshots_error: state.screenshots_error.clone(),
            screenshots_request_id: state.screenshots_request_id,
            server_search_query: state.server_search_query.clone(),
            servers_ptr: state.servers.as_ptr() as usize,
            servers_len: state.servers.len(),
            servers_loaded: state.servers_loaded,
            servers_loading: state.servers_loading,
            servers_error: state.servers_error.clone(),
            servers_request_id: state.servers_request_id,
            server_motd_ptr: Arc::as_ptr(&state.server_motd) as usize,
            server_motd_len: state.server_motd.len(),
            server_motd_loading: state.server_motd_loading,
            server_motd_request_id: state.server_motd_request_id,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct VersionConfigLoadSignature {
    pub(super) folder: SharedString,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct GdkUsersLoadSignature {
    pub(super) folder: SharedString,
    pub(super) enable_redirection: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct AssetsLoadSignature {
    pub(super) folder: SharedString,
    pub(super) tab: ManageTab,
    pub(super) pack_subtype: ManagePackSubtype,
    pub(super) selected_gdk_user: Option<SharedString>,
    pub(super) enable_redirection: bool,
    pub(super) locale_code: SharedString,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct ScreenshotsLoadSignature {
    pub(super) folder: SharedString,
    pub(super) selected_gdk_user: Option<SharedString>,
    pub(super) enable_redirection: bool,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct ServersLoadSignature {
    pub(super) folder: SharedString,
    pub(super) selected_gdk_user: Option<SharedString>,
    pub(super) enable_redirection: bool,
}

impl ManagePageView {
    pub(super) fn ensure_asset_search_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.asset_search_input.is_some() {
            return;
        }

        let input = cx.update_global(|state: &mut ManagePageState, cx| {
            let initial = state.asset_search_query.to_string();
            cx.new(|cx| {
                let mut input_state = InputState::new(window, cx);
                input_state.set_placeholder(SharedString::from("搜索资源..."), window, cx);
                if !initial.trim().is_empty() {
                    input_state.set_value(SharedString::from(initial), window, cx);
                }
                input_state
            })
        });

        let subscription = cx.subscribe(&input, |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let value = input.read(cx).value();
                let changed = cx.update_global(|state: &mut ManagePageState, _cx| {
                    if state.asset_search_query == value {
                        return false;
                    }
                    state.asset_search_query = value;
                    true
                });
                if changed {
                    this.reset_asset_list_view();
                    cx.notify();
                }
            }
        });
        self._subscriptions.push(subscription);
        self.asset_search_input = Some(input);
    }

    pub(super) fn ensure_screenshot_search_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.screenshot_search_input.is_some() {
            return;
        }

        let input = cx.update_global(|state: &mut ManagePageState, cx| {
            let initial = state.screenshot_search_query.to_string();
            cx.new(|cx| {
                let mut input_state = InputState::new(window, cx);
                input_state.set_placeholder(SharedString::from("搜索截图..."), window, cx);
                if !initial.trim().is_empty() {
                    input_state.set_value(SharedString::from(initial), window, cx);
                }
                input_state
            })
        });

        let subscription = cx.subscribe(&input, |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let value = input.read(cx).value();
                let changed = cx.update_global(|state: &mut ManagePageState, _cx| {
                    if state.screenshot_search_query == value {
                        return false;
                    }
                    state.screenshot_search_query = value;
                    true
                });
                if changed {
                    this.reset_screenshot_list_view();
                    cx.notify();
                }
            }
        });
        self._subscriptions.push(subscription);
        self.screenshot_search_input = Some(input);
    }

    pub(super) fn ensure_server_search_input(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.server_search_input.is_some() {
            return;
        }

        let input = cx.update_global(|state: &mut ManagePageState, cx| {
            let initial = state.server_search_query.to_string();
            cx.new(|cx| {
                let mut input_state = InputState::new(window, cx);
                input_state.set_placeholder(SharedString::from("搜索服务器..."), window, cx);
                if !initial.trim().is_empty() {
                    input_state.set_value(SharedString::from(initial), window, cx);
                }
                input_state
            })
        });

        let subscription = cx.subscribe(&input, |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let value = input.read(cx).value();
                let changed = cx.update_global(|state: &mut ManagePageState, _cx| {
                    if state.server_search_query == value {
                        return false;
                    }
                    state.server_search_query = value;
                    true
                });
                if changed {
                    this.reset_server_list_view();
                    cx.notify();
                }
            }
        });
        self._subscriptions.push(subscription);
        self.server_search_input = Some(input);
    }

    pub(super) fn sync_selected_version(&mut self, cx: &mut Context<Self>) {
        let selected_folder = cx.global::<ManagePageState>().selected_folder.clone();
        if self.last_selected_folder == selected_folder {
            return;
        }

        self.last_selected_folder = selected_folder;
        self.reset_asset_list_view();
        self.reset_screenshot_list_view();
        self.reset_server_list_view();
        self.version_settings_modal = None;
        self.confirm_dialog = None;
        self.value_prompt = None;
        self.mod_type_dialog = None;
        self.server_editor_dialog = None;
        self.level_dat_editor = None;
        self.last_version_config_signature = None;
        self.last_gdk_users_signature = None;
        self.last_assets_signature = None;
        self.last_screenshots_signature = None;
        self.last_servers_signature = None;

        cx.update_global(|state: &mut ManagePageState, _cx| {
            state.selected_asset_keys.clear();
            state.version_config = ManageVersionConfig::default();
            state.version_config_loading = false;
            state.version_config_error = None;
            state.version_config_request_id = state.version_config_request_id.wrapping_add(1);
            state.gdk_users = Arc::from(Vec::new());
            state.selected_gdk_user = None;
            state.gdk_users_loading = false;
            state.gdk_users_error = None;
            state.gdk_users_request_id = state.gdk_users_request_id.wrapping_add(1);
            state.assets = Arc::from(Vec::new());
            state.assets_loaded = false;
            state.assets_loading = false;
            state.assets_error = None;
            state.assets_request_id = state.assets_request_id.wrapping_add(1);
            state.screenshots = Arc::from(Vec::new());
            state.screenshots_loaded = false;
            state.screenshots_loading = false;
            state.screenshots_error = None;
            state.screenshots_request_id = state.screenshots_request_id.wrapping_add(1);
            state.servers = Arc::from(Vec::new());
            state.servers_loaded = false;
            state.servers_loading = false;
            state.servers_error = None;
            state.servers_request_id = state.servers_request_id.wrapping_add(1);
            state.server_motd = Arc::new(HashMap::new());
            state.server_motd_loading = false;
            state.server_motd_request_id = state.server_motd_request_id.wrapping_add(1);
        });
    }

    pub(super) fn invalidate_version_dependent_data(&mut self, cx: &mut Context<Self>) {
        self.last_version_config_signature = None;
        self.last_gdk_users_signature = None;
        self.last_assets_signature = None;
        self.last_screenshots_signature = None;
        self.last_servers_signature = None;
        self.reset_asset_list_view();
        self.reset_screenshot_list_view();
        self.reset_server_list_view();
        cx.update_global(|state: &mut ManagePageState, _cx| {
            state.selected_asset_keys.clear();
            state.assets_loaded = false;
            state.assets_loading = false;
            state.assets_error = None;
            state.screenshots_loaded = false;
            state.screenshots_loading = false;
            state.screenshots_error = None;
            state.servers_loaded = false;
            state.servers_loading = false;
            state.servers_error = None;
            state.server_motd = Arc::new(HashMap::new());
            state.server_motd_loading = false;
            state.server_motd_request_id = state.server_motd_request_id.wrapping_add(1);
            state.gdk_users_loading = false;
            state.gdk_users_error = None;
        });
    }

    pub(super) fn selected_version<'a>(
        &self,
        state: &'a ManagePageState,
    ) -> Option<&'a ManagedVersionEntry> {
        let selected = state.selected_folder.as_ref()?;
        state
            .versions
            .iter()
            .find(|version| version.folder.as_ref() == selected.as_ref())
    }

    pub(super) fn sync_data_requests(&mut self, cx: &mut Context<Self>) {
        let locale_code = SharedString::from(cx.global::<I18n>().locale().code());
        let (
            selected_version,
            version_config_loading,
            version_config,
            tab,
            pack_subtype,
            selected_gdk_user,
            assets_loaded,
            assets_loading,
            screenshots_loaded,
            screenshots_loading,
            servers_loaded,
            servers_loading,
        ) = {
            let state = cx.global::<ManagePageState>();
            (
                self.selected_version(state).cloned(),
                state.version_config_loading,
                state.version_config.clone(),
                state.tab,
                state.pack_subtype,
                state.selected_gdk_user.clone(),
                state.assets_loaded,
                state.assets_loading,
                state.screenshots_loaded,
                state.screenshots_loading,
                state.servers_loaded,
                state.servers_loading,
            )
        };

        let Some(selected_version) = selected_version else {
            return;
        };

        let config_signature = VersionConfigLoadSignature {
            folder: selected_version.folder.clone(),
        };
        if self.last_version_config_signature.as_ref() != Some(&config_signature) {
            self.last_version_config_signature = Some(config_signature);
            self.request_version_config(selected_version.clone(), cx);
            return;
        }

        if version_config_loading {
            return;
        }

        if selected_version.is_gdk() && is_gdk_user_scoped_tab(tab) {
            let gdk_signature = GdkUsersLoadSignature {
                folder: selected_version.folder.clone(),
                enable_redirection: version_config.enable_redirection,
            };
            if self.last_gdk_users_signature.as_ref() != Some(&gdk_signature) {
                self.last_gdk_users_signature = Some(gdk_signature);
                self.request_gdk_users(selected_version.clone(), version_config.clone(), cx);
            }
        }

        if selected_version.is_gdk() && is_gdk_user_scoped_tab(tab) && selected_gdk_user.is_none() {
            self.clear_active_user_scoped_data(tab, cx);
            return;
        }

        match tab {
            ManageTab::Mod | ManageTab::ResourcePack | ManageTab::SkinPack | ManageTab::Map => {
                let assets_signature = AssetsLoadSignature {
                    folder: selected_version.folder.clone(),
                    tab,
                    pack_subtype,
                    selected_gdk_user: selected_gdk_user.clone(),
                    enable_redirection: version_config.enable_redirection,
                    locale_code,
                };

                if self.last_assets_signature.as_ref() != Some(&assets_signature)
                    || (!assets_loaded && !assets_loading)
                {
                    self.last_assets_signature = Some(assets_signature.clone());
                    self.request_assets(
                        selected_version,
                        version_config,
                        tab,
                        pack_subtype,
                        selected_gdk_user,
                        assets_signature.locale_code,
                        cx,
                    );
                }
            }
            ManageTab::Screenshot => {
                let screenshots_signature = ScreenshotsLoadSignature {
                    folder: selected_version.folder.clone(),
                    selected_gdk_user: selected_gdk_user.clone(),
                    enable_redirection: version_config.enable_redirection,
                };
                if self.last_screenshots_signature.as_ref() != Some(&screenshots_signature)
                    || (!screenshots_loaded && !screenshots_loading)
                {
                    self.last_screenshots_signature = Some(screenshots_signature);
                    self.request_screenshots(
                        selected_version,
                        version_config,
                        selected_gdk_user,
                        cx,
                    );
                }
            }
            ManageTab::Server => {
                let servers_signature = ServersLoadSignature {
                    folder: selected_version.folder.clone(),
                    selected_gdk_user: selected_gdk_user.clone(),
                    enable_redirection: version_config.enable_redirection,
                };
                if self.last_servers_signature.as_ref() != Some(&servers_signature)
                    || (!servers_loaded && !servers_loading)
                {
                    self.last_servers_signature = Some(servers_signature);
                    self.request_servers(selected_version, version_config, selected_gdk_user, cx);
                }
            }
        }
    }

    pub(super) fn request_version_config(
        &mut self,
        version: ManagedVersionEntry,
        cx: &mut Context<Self>,
    ) {
        let request_id = cx.update_global(|state: &mut ManagePageState, _cx| {
            state.version_config_loading = true;
            state.version_config_error = None;
            state.version_config_request_id = state.version_config_request_id.wrapping_add(1);
            state.version_config_request_id
        });

        cx.spawn(async move |handle, cx| {
            let result = data::load_version_config(&version).await;
            let applied = cx.update_global(|state: &mut ManagePageState, _cx| {
                if state.version_config_request_id != request_id
                    || state.selected_folder.as_ref() != Some(&version.folder)
                {
                    state.version_config_loading = false;
                    return false;
                }
                state.version_config_loading = false;
                match result {
                    Ok(ref config) => {
                        state.version_config = config.clone();
                        state.version_config_error = None;
                    }
                    Err(ref error) => {
                        state.version_config_error = Some(SharedString::from(error.clone()));
                    }
                }
                true
            })?;
            if applied
                && let Err(error) = handle.update(cx, |this, cx| {
                    this.sync_data_requests(cx);
                    cx.notify();
                })
            {
                tracing::debug!("manage view was released after version config loaded: {error:?}");
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn request_gdk_users(
        &mut self,
        version: ManagedVersionEntry,
        config: ManageVersionConfig,
        cx: &mut Context<Self>,
    ) {
        let request_id = cx.update_global(|state: &mut ManagePageState, _cx| {
            state.gdk_users_loading = true;
            state.gdk_users_error = None;
            state.gdk_users_request_id = state.gdk_users_request_id.wrapping_add(1);
            state.gdk_users_request_id
        });

        cx.spawn(async move |handle, cx| {
            let result = data::load_gdk_users(&version, &config).await;

            let applied = cx.update_global(|state: &mut ManagePageState, _cx| {
                if state.gdk_users_request_id != request_id
                    || state.selected_folder.as_ref() != Some(&version.folder)
                {
                    state.gdk_users_loading = false;
                    return false;
                }
                state.gdk_users_loading = false;
                match result {
                    Ok(users) => {
                        let preferred_user = Self::preferred_gdk_user(&users);
                        let has_selected =
                            state.selected_gdk_user.as_ref().is_some_and(|selected| {
                                users
                                    .iter()
                                    .any(|user| user.folder_name.as_ref() == selected.as_ref())
                            });
                        let selected_is_shared =
                            state.selected_gdk_user.as_ref().is_some_and(|selected| {
                                selected.as_ref().eq_ignore_ascii_case("shared")
                            });
                        if !has_selected || selected_is_shared {
                            state.selected_gdk_user = preferred_user;
                        }
                        state.gdk_users = Arc::from(users);
                        state.gdk_users_error = None;
                    }
                    Err(error) => {
                        state.gdk_users_error = Some(SharedString::from(error));
                    }
                }
                true
            })?;
            if applied
                && let Err(error) = handle.update(cx, |this, cx| {
                    this.sync_data_requests(cx);
                    cx.notify();
                })
            {
                tracing::debug!("manage view was released after GDK users loaded: {error:?}");
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn preferred_gdk_user(users: &[ManageGdkUser]) -> Option<SharedString> {
        users
            .iter()
            .find(|user| !user.folder_name.as_ref().eq_ignore_ascii_case("shared"))
            .or_else(|| users.first())
            .map(|user| user.folder_name.clone())
    }

    pub(super) fn clear_active_user_scoped_data(&mut self, tab: ManageTab, cx: &mut Context<Self>) {
        match tab {
            ManageTab::Map => {
                self.last_assets_signature = None;
                self.reset_asset_list_view();
                cx.update_global(|state: &mut ManagePageState, _cx| {
                    state.assets = Arc::from(Vec::new());
                    state.assets_loading = false;
                    state.assets_loaded = true;
                    state.assets_error = None;
                    state.selected_asset_keys.clear();
                });
            }
            ManageTab::Screenshot => {
                self.last_screenshots_signature = None;
                self.reset_screenshot_list_view();
                cx.update_global(|state: &mut ManagePageState, _cx| {
                    state.screenshots = Arc::from(Vec::new());
                    state.screenshots_loading = false;
                    state.screenshots_loaded = true;
                    state.screenshots_error = None;
                });
            }
            ManageTab::Server => {
                self.last_servers_signature = None;
                self.reset_server_list_view();
                cx.update_global(|state: &mut ManagePageState, _cx| {
                    state.servers = Arc::from(Vec::new());
                    state.servers_loading = false;
                    state.servers_loaded = true;
                    state.servers_error = None;
                    state.server_motd = Arc::new(HashMap::new());
                    state.server_motd_loading = false;
                    state.server_motd_request_id = state.server_motd_request_id.wrapping_add(1);
                });
            }
            ManageTab::Mod | ManageTab::ResourcePack | ManageTab::SkinPack => {}
        }
        cx.notify();
    }

    pub(super) fn request_assets(
        &mut self,
        version: ManagedVersionEntry,
        config: ManageVersionConfig,
        tab: ManageTab,
        pack_subtype: ManagePackSubtype,
        selected_gdk_user: Option<SharedString>,
        locale_code: SharedString,
        cx: &mut Context<Self>,
    ) {
        let request_id = cx.update_global(|state: &mut ManagePageState, _cx| {
            state.assets_loading = true;
            state.assets_error = None;
            state.assets_request_id = state.assets_request_id.wrapping_add(1);
            state.assets_request_id
        });

        cx.spawn(async move |handle, cx| {
            let result = data::load_assets(
                &version,
                &config,
                tab,
                pack_subtype,
                selected_gdk_user.as_ref().map(SharedString::as_ref),
                locale_code.as_ref(),
            )
            .await;

            let applied = cx
                .update_global(|state: &mut ManagePageState, _cx| {
                    if state.assets_request_id != request_id
                        || state.selected_folder.as_ref() != Some(&version.folder)
                    {
                        state.assets_loading = false;
                        return false;
                    }
                    state.assets_loading = false;
                    state.assets_loaded = true;
                    match result {
                        Ok(assets) => {
                            state.assets = Arc::from(assets);
                            state.assets_error = None;
                            state
                                .selected_asset_keys
                                .retain(|key| state.assets.iter().any(|asset| asset.key == *key));
                        }
                        Err(error) => {
                            state.assets_error = Some(SharedString::from(error));
                            state.assets = Arc::from(Vec::new());
                            state.selected_asset_keys.clear();
                        }
                    }
                    true
                })
                .unwrap_or_else(|error| {
                    tracing::warn!("failed to apply loaded manage assets: {error:?}");
                    false
                });

            if applied
                && let Err(error) = handle.update(cx, |this, cx| {
                    this.reset_asset_list_view();
                    cx.notify();
                })
            {
                tracing::debug!("manage view was released after assets loaded: {error:?}");
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn request_screenshots(
        &mut self,
        version: ManagedVersionEntry,
        config: ManageVersionConfig,
        selected_gdk_user: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let request_id = cx.update_global(|state: &mut ManagePageState, _cx| {
            state.screenshots_loading = true;
            state.screenshots_error = None;
            state.screenshots_request_id = state.screenshots_request_id.wrapping_add(1);
            state.screenshots_request_id
        });

        cx.spawn(async move |handle, cx| {
            let result = data::load_screenshots(
                &version,
                &config,
                selected_gdk_user.as_ref().map(SharedString::as_ref),
            )
            .await;

            let applied = cx
                .update_global(|state: &mut ManagePageState, _cx| {
                    if state.screenshots_request_id != request_id
                        || state.selected_folder.as_ref() != Some(&version.folder)
                    {
                        state.screenshots_loading = false;
                        return false;
                    }
                    state.screenshots_loading = false;
                    state.screenshots_loaded = true;
                    match result {
                        Ok(screenshots) => {
                            state.screenshots = Arc::from(screenshots);
                            state.screenshots_error = None;
                        }
                        Err(error) => {
                            state.screenshots_error = Some(SharedString::from(error));
                            state.screenshots = Arc::from(Vec::new());
                        }
                    }
                    true
                })
                .unwrap_or_else(|error| {
                    tracing::warn!("failed to apply loaded screenshots: {error:?}");
                    false
                });

            if applied
                && let Err(error) = handle.update(cx, |this, cx| {
                    this.reset_screenshot_list_view();
                    cx.notify();
                })
            {
                tracing::debug!("manage view was released after screenshots loaded: {error:?}");
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn request_servers(
        &mut self,
        version: ManagedVersionEntry,
        config: ManageVersionConfig,
        selected_gdk_user: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let request_id = cx.update_global(|state: &mut ManagePageState, _cx| {
            state.servers_loading = true;
            state.servers_error = None;
            state.servers_request_id = state.servers_request_id.wrapping_add(1);
            state.servers_request_id
        });

        cx.spawn(async move |handle, cx| {
            let result = data::load_external_servers(
                &version,
                &config,
                selected_gdk_user.as_ref().map(SharedString::as_ref),
            )
            .await;

            let mut servers_for_motd = Vec::new();
            let applied = cx
                .update_global(|state: &mut ManagePageState, _cx| {
                    if state.servers_request_id != request_id
                        || state.selected_folder.as_ref() != Some(&version.folder)
                    {
                        state.servers_loading = false;
                        return false;
                    }
                    state.servers_loading = false;
                    state.servers_loaded = true;
                    match result {
                        Ok(servers) => {
                            servers_for_motd =
                                servers.iter().map(ManageServerMotdTarget::from).collect();
                            state.servers = Arc::from(servers);
                            state.servers_error = None;
                            state.server_motd = Arc::new(HashMap::new());
                        }
                        Err(error) => {
                            state.servers_error = Some(SharedString::from(error));
                            state.servers = Arc::from(Vec::new());
                            state.server_motd = Arc::new(HashMap::new());
                        }
                    }
                    true
                })
                .unwrap_or_else(|error| {
                    tracing::warn!("failed to apply loaded servers: {error:?}");
                    false
                });

            if applied
                && let Err(error) = handle.update(cx, |this, cx| {
                    this.reset_server_list_view();
                    if !servers_for_motd.is_empty() {
                        this.request_server_motds(servers_for_motd, cx);
                    }
                    cx.notify();
                })
            {
                tracing::debug!("manage view was released after servers loaded: {error:?}");
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn refresh_server_motds(&mut self, cx: &mut Context<Self>) {
        let servers = cx
            .global::<ManagePageState>()
            .servers
            .iter()
            .map(ManageServerMotdTarget::from)
            .collect();
        self.request_server_motds(servers, cx);
    }

    pub(super) fn request_server_motds(
        &mut self,
        servers: Vec<ManageServerMotdTarget>,
        cx: &mut Context<Self>,
    ) {
        if servers.is_empty() {
            cx.update_global(|state: &mut ManagePageState, _cx| {
                state.server_motd = Arc::new(HashMap::new());
                state.server_motd_loading = false;
                state.server_motd_request_id = state.server_motd_request_id.wrapping_add(1);
            });
            return;
        }

        let keys: Vec<_> = servers.iter().map(|server| server.key.clone()).collect();
        let request_id = cx.update_global(|state: &mut ManagePageState, _cx| {
            state.server_motd_loading = true;
            state.server_motd_request_id = state.server_motd_request_id.wrapping_add(1);
            let mut motd = (*state.server_motd).clone();
            for key in &keys {
                motd.insert(key.clone(), ManageServerMotdStatus::Loading);
            }
            state.server_motd = Arc::new(motd);
            state.server_motd_request_id
        });

        cx.spawn(async move |_handle, cx| {
            let result = data::query_server_motd_batch(servers).await;
            cx.update_global(|state: &mut ManagePageState, _cx| {
                if state.server_motd_request_id != request_id {
                    state.server_motd_loading = false;
                    return;
                }
                let mut motd = (*state.server_motd).clone();
                for (key, status) in result {
                    motd.insert(key, status);
                }
                state.server_motd = Arc::new(motd);
                state.server_motd_loading = false;
            })?;
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }
}
