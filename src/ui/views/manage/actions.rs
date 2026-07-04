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
            "输入整数像素值",
            &state.config.reduce_pixels.to_string(),
        ) else {
            return;
        };
        self.value_prompt = Some(ValuePromptDialogState {
            title: SharedString::from("缩减像素"),
            description: SharedString::from("输入鼠标锁定时缩减的像素值。"),
            confirm_label: SharedString::from("应用"),
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
        cx.spawn(async move |handle, cx| {
            let result = data::save_manage_version_config(&version, &config).await;
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
                        toast::success(cx, SharedString::from("版本设置已保存"));
                        this.version_settings_modal = None;
                        this.invalidate_version_dependent_data(cx);
                    }
                    Err(error) => {
                        toast::error(cx, SharedString::from(error));
                    }
                }
                cx.notify();
            });
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }
    pub(super) fn refresh_versions(&mut self, cx: &mut Context<Self>) {
        ensure_local_versions_loaded(true, cx);
        toast::push(cx, SharedString::from("正在刷新版本列表"));
    }
    pub(super) fn open_version_settings(&mut self, cx: &mut Context<Self>) {
        let state = cx.global::<ManagePageState>();
        let Some(version) = self.selected_version(state).cloned() else {
            return;
        };
        self.version_settings_modal = Some(version_settings::VersionSettingsModalState {
            version,
            config: state.version_config.clone(),
            saving: false,
        });
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
                ManageTab::Mod
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

    pub(super) fn import_version_package(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.defer(cx, move |_window, cx| {
            cx.spawn(async move |cx| {
                let selected = tokio::task::spawn_blocking(|| {
                    pick_file_path_with_filter("Packages", &["appx", "zip", "msixvc"])
                })
                .await
                .ok()
                .flatten();

                let Some(path) = selected else {
                    return Ok::<(), anyhow::Error>(());
                };

                let task_id = if path.to_ascii_lowercase().ends_with(".msixvc") {
                    let folder_name = std::path::Path::new(&path)
                        .file_stem()
                        .and_then(|value| value.to_str())
                        .filter(|value| !value.trim().is_empty())
                        .unwrap_or("ImportedGDK");
                    start_unpack_gdk_task(&path, folder_name)
                } else {
                    import_appx(path.clone(), None).await
                };

                cx.update(|cx| match task_id {
                    Ok(task_id) => {
                        toast::push(cx, SharedString::from("安装任务已开始"));
                        watch_import_task(task_id, cx);
                    }
                    Err(error) => {
                        toast::error(cx, SharedString::from(error));
                    }
                })?;

                Ok::<(), anyhow::Error>(())
            })
            .detach();
        });
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
        self.confirm_dialog = Some(ConfirmDialogState {
            title: SharedString::from("删除版本"),
            description: SharedString::from(format!(
                "确定删除 {} 吗？此操作不可撤销。",
                version.display_name()
            )),
            confirm_label: SharedString::from("删除版本"),
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
