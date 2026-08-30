use super::*;

impl ManagePageView {
    pub fn close_version_settings(&mut self, cx: &mut Context<Self>) {
        self.version_settings_modal = None;
        cx.notify();
    }

    pub fn toggle_version_setting(
        &mut self,
        field: version_settings::VersionSettingsToggle,
        cx: &mut Context<Self>,
    ) {
        let Some(state) = self.version_settings_modal.as_mut() else {
            return;
        };
        match field {
            version_settings::VersionSettingsToggle::DebugConsole => {
                state.config.enable_debug_console = !state.config.enable_debug_console;
            }
            version_settings::VersionSettingsToggle::Redirection => {
                state.config.enable_redirection = !state.config.enable_redirection;
            }
            version_settings::VersionSettingsToggle::EditorMode => {
                state.config.editor_mode = !state.config.editor_mode;
            }
            version_settings::VersionSettingsToggle::DisableModLoading => {
                state.config.disable_mod_loading = !state.config.disable_mod_loading;
            }
            version_settings::VersionSettingsToggle::LockMouseOnLaunch => {
                state.config.lock_mouse_on_launch = !state.config.lock_mouse_on_launch;
            }
            version_settings::VersionSettingsToggle::ShortcutSilentLaunch => {
                state.config.shortcut_silent_launch = !state.config.shortcut_silent_launch;
            }
        }
        cx.notify();
    }

    pub fn set_version_hotkey(&mut self, hotkey: SharedString, cx: &mut Context<Self>) {
        if let Some(state) = self.version_settings_modal.as_mut() {
            state.config.unlock_mouse_hotkey = hotkey;
            cx.notify();
        }
    }

    pub fn open_reduce_pixels_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.version_settings_modal.as_ref() else {
            return;
        };
        let Some(input) = create_text_input(
            window,
            cx,
            t!("ManagePage.input_pixels").as_ref(),
            &state.config.reduce_pixels.to_string(),
        ) else {
            return;
        };
        self.value_prompt = Some(ValuePromptDialogState {
            title: t!("ManagePage.reduce_pixels"),
            description: t!("ManagePage.reduce_pixels_desc"),
            confirm_label: t!("common.apply"),
            input,
            target: ValuePromptTarget::VersionReducePixels,
            pending: false,
        });
        cx.notify();
    }

    pub fn save_version_settings(&mut self, cx: &mut Context<Self>) {
        let Some(modal_state) = self.version_settings_modal.as_mut() else {
            return;
        };
        if modal_state.saving {
            return;
        }
        modal_state.saving = true;
        let version = modal_state.version.clone();
        let config = modal_state.config.clone();
        let icon_source_path = modal_state.icon_source_path.clone();
        let i18n = cx.global::<I18n>().clone();
        cx.spawn(async move |handle, cx| {
            let version_for_save = version.clone();
            let config_for_save = config.clone();
            let result = gpui_tokio::Tokio::spawn_result(cx, async move {
                data::save_manage_version_config(&version_for_save, &config_for_save)
                    .await
                    .map_err(anyhow::Error::msg)?;
                if let Some(icon_source_path) = icon_source_path {
                    crate::core::version::icons::copy_version_icon(
                        std::path::Path::new(icon_source_path.as_ref()),
                        std::path::Path::new(version_for_save.path.as_ref()),
                    )
                    .await
                    .map_err(|error| anyhow::anyhow!("failed to save version icon: {error}"))?;
                }
                Ok::<(), anyhow::Error>(())
            })
            .await;
            let _ = handle.update(cx, |this, cx| {
                if let Some(modal) = this.version_settings_modal.as_mut() {
                    modal.saving = false;
                }

                match result {
                    Ok(()) => {
                        cx.update_global(|state: &mut ManagePageState, _cx| {
                            state.version_config = config.clone();
                            state.version_config_error = None;
                        });
                        ensure_local_versions_loaded(true, cx);
                        let message = t!("ManagePage.settings_saved");
                        toast::success(cx, message);
                        this.version_settings_modal = None;
                        this.invalidate_version_dependent_data(cx);
                    }
                    Err(error) => {
                        let message = error.to_string();
                        let localized_message =
                            t!("ManagePage.settings_save_failed", message = &message);
                        toast::error(cx, localized_message);
                    }
                }
                cx.notify();
            });
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }
    pub(super) fn refresh_versions(&mut self, cx: &mut Context<Self>) {
        self.invalidate_version_dependent_data(cx);
        ensure_local_versions_loaded(true, cx);
        let message = t!("ManagePage.refreshing_versions");
        toast::push(cx, message);
    }
    pub(super) fn open_version_settings(&mut self, cx: &mut Context<Self>) {
        let state = cx.global::<ManagePageState>();
        let Some(version) = self.selected_version(state).cloned() else {
            return;
        };
        let supports_levilamina = version.is_gdk();
        self.version_settings_modal = Some(version_settings::VersionSettingsModalState {
            version: version.clone(),
            config: state.version_config.clone(),
            icon_source_path: None,
            saving: false,
            levilamina_loading: supports_levilamina,
            levilamina_busy: false,
            levilamina_error: None,
            levilamina_versions: Vec::new(),
            selected_levilamina_version: SharedString::from(""),
            levilamina_installation: crate::core::levilamina::LeviLaminaInstallation::default(),
        });
        cx.notify();
        if supports_levilamina {
            self.load_levilamina_settings(version, cx);
        }
    }

    pub fn select_version_icon(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(icon_source_path) =
            pick_file_path_with_filter_for_window(window, "PNG Image", &["png"])
        else {
            return;
        };
        let Some(modal_state) = self.version_settings_modal.as_mut() else {
            return;
        };
        modal_state.icon_source_path = Some(SharedString::from(icon_source_path));
        cx.notify();
    }

    pub(super) fn select_version(&mut self, folder: SharedString, cx: &mut Context<Self>) {
        cx.update_global(|state: &mut ManagePageState, _cx| {
            state.selected_folder = Some(folder);
        });
        cx.notify();
    }

    pub(super) fn set_tab(&mut self, tab: ManageTab, cx: &mut Context<Self>) {
        cx.update_global(|state: &mut ManagePageState, _cx| {
            state.tab = tab;
            state.selected_asset_keys.clear();
            match tab {
                ManageTab::Map | ManageTab::Screenshot => {
                    state.asset_sort_key = ManageAssetSortKey::Date;
                    state.asset_sort_desc = true;
                }
                ManageTab::Statistics
                | ManageTab::Mod
                | ManageTab::ResourcePack
                | ManageTab::SkinPack
                | ManageTab::Server => {
                    state.asset_sort_key = ManageAssetSortKey::Name;
                    state.asset_sort_desc = false;
                }
            }
        });
        self.last_assets_signature = None;
        self.last_screenshots_signature = None;
        self.last_servers_signature = None;
        self.reset_asset_list_view();
        self.reset_screenshot_list_view();
        self.reset_server_list_view();
        cx.notify();
    }

    pub(super) fn open_selected_version_folder(&mut self, cx: &mut Context<Self>) {
        let path = cx
            .global::<ManagePageState>()
            .selected_folder
            .as_ref()
            .and_then(|folder| {
                cx.global::<ManagePageState>()
                    .versions
                    .iter()
                    .find(|version| version.folder.as_ref() == folder.as_ref())
                    .map(|version| version.path.clone())
            });
        let Some(path) = path else {
            return;
        };

        cx.spawn(async move |_handle, cx| {
            if let Err(error) = crate::utils::open_path::open_path(path.to_string()).await {
                let _ = cx.update(|cx| {
                    toast::error(cx, SharedString::from(error));
                });
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn request_delete_version(&mut self, cx: &mut Context<Self>) {
        let state = cx.global::<ManagePageState>();
        let Some(version) = self.selected_version(state).cloned() else {
            return;
        };
        let i18n = cx.global::<I18n>().clone();
        let version_name = version.display_name().to_string();
        self.confirm_dialog = Some(ConfirmDialogState {
            title: t!("ManagePage.delete_version_title"),
            description: t!(
                "ManagePage.delete_version_confirm_named",
                name = &version_name
            ),
            confirm_label: t!("ManagePage.delete_version"),
            danger: true,
            pending: false,
            action: ConfirmAction::DeleteVersion { version },
        });
        cx.notify();
    }

    pub(super) fn launch_selected_version(&mut self, cx: &mut Context<Self>) {
        let state = cx.global::<ManagePageState>();
        let Some(version) = self.selected_version(state) else {
            return;
        };
        let descriptor = LaunchVersionDescriptor {
            folder: version.folder.clone(),
            name: version.name.clone(),
            version: version.version.clone(),
            kind: version.kind.clone(),
            path: version.path.clone(),
            launch_args: None,
        };
        let _ = start_launcher(descriptor, cx);
    }

    pub(super) fn create_selected_version_shortcut(&mut self, cx: &mut Context<Self>) {
        let state = cx.global::<ManagePageState>();
        let Some(version) = self.selected_version(state) else {
            return;
        };
        let folder = version.folder.to_string();
        let i18n = cx.global::<I18n>().clone();
        match crate::utils::shortcut::create_desktop_shortcut(&folder, &folder) {
            Ok(path) => {
                let message = t!(
                    "ManagePage.shortcut_created",
                    path = &path.display().to_string()
                );
                toast::success(cx, message);
            }
            Err(error) => {
                let message = t!("ManagePage.shortcut_failed", message = &error.to_string());
                toast::error(cx, message);
            }
        }
    }
    pub(super) fn open_path_background(&mut self, path: SharedString, cx: &mut Context<Self>) {
        cx.spawn(async move |_handle, cx| {
            if let Err(error) = crate::utils::open_path::open_path(path.to_string()).await {
                let _ = cx.update(|cx| {
                    toast::error(cx, SharedString::from(error));
                });
            }
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }
}
