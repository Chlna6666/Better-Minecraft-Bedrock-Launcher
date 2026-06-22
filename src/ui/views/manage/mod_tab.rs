use super::*;

impl ManagePageView {
    pub(super) fn toggle_mod_enabled(&mut self, asset: ManageAssetEntry, cx: &mut Context<Self>) {
        let state = cx.global::<ManagePageState>();
        let Some(version) = self.selected_version(state).cloned() else {
            return;
        };
        let Some(enabled) = asset.enabled else {
            return;
        };

        cx.spawn(async move |_handle, cx| {
            let result = data::set_mod_enabled(
                version.folder.as_ref(),
                asset.folder_name.as_ref(),
                !enabled,
            )
            .await;
            let _ = cx.update(|cx| match result {
                Ok(()) => {
                    toast::success(cx, SharedString::from("Mod 状态已更新"));
                    cx.update_global(|state: &mut ManagePageState, _cx| {
                        state.assets_loaded = false;
                        state.assets_loading = false;
                        state.assets_error = None;
                    });
                }
                Err(error) => {
                    toast::error(cx, SharedString::from(error));
                }
            });
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }

    pub(super) fn open_mod_type_dialog(
        &mut self,
        asset: ManageAssetEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (version, selected_mod_type) = {
            let state = cx.global::<ManagePageState>();
            let Some(version) = self.selected_version(state).cloned() else {
                return;
            };
            let asset_key = asset.key.clone();
            let selected_mod_type = state
                .assets
                .iter()
                .find(|entry| entry.key == asset_key)
                .and_then(|entry| entry.mod_type.clone())
                .unwrap_or_else(|| SharedString::from("preload-native"));
            (version, selected_mod_type)
        };
        let delay_value = asset.inject_delay_ms.unwrap_or(0).to_string();
        let Some(delay_input) = create_text_input(window, cx, "输入毫秒延迟", &delay_value)
        else {
            return;
        };
        self.mod_type_dialog = Some(ModTypeDialogState {
            version,
            asset,
            selected_mod_type,
            delay_input,
            pending: false,
        });
        cx.notify();
    }

    pub(super) fn open_mod_delay_prompt(
        &mut self,
        asset: ManageAssetEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = cx.global::<ManagePageState>();
        let Some(version) = self.selected_version(state).cloned() else {
            return;
        };
        let Some(input) = create_text_input(
            window,
            cx,
            "输入毫秒值",
            &asset.inject_delay_ms.unwrap_or(0).to_string(),
        ) else {
            return;
        };
        self.value_prompt = Some(ValuePromptDialogState {
            title: SharedString::from("注入延迟"),
            description: SharedString::from(asset.display_name.clone()),
            confirm_label: SharedString::from("保存"),
            input,
            target: ValuePromptTarget::ModInjectDelay { version, asset },
            pending: false,
        });
        cx.notify();
    }

    pub(super) fn set_mod_type_selection(
        &mut self,
        mod_type: SharedString,
        cx: &mut Context<Self>,
    ) {
        if let Some(dialog) = self.mod_type_dialog.as_mut() {
            dialog.selected_mod_type = mod_type;
            cx.notify();
        }
    }
}

pub(super) fn append_mod_asset_actions(
    actions: Div,
    colors: &ThemeColors,
    asset: &ManageAssetEntry,
    action_key: &SharedString,
    cx: &mut Context<ManagePageView>,
) -> Div {
    actions
        .child(
            compact_icon_button(
                colors,
                SharedString::from(format!("manage-mod-toggle-{}", asset.key)),
                if asset.enabled.unwrap_or(true) {
                    lucide_icons::icon_toggle_right()
                } else {
                    lucide_icons::icon_toggle_left()
                },
            )
            .on_mouse_down(MouseButton::Left, {
                let key = action_key.clone();
                cx.listener(move |this, _, _, cx| {
                    let asset = resolve_asset_by_key(cx.global::<ManagePageState>(), &key);
                    if let Some(asset) = asset {
                        this.toggle_mod_enabled(asset, cx);
                    }
                })
            }),
        )
        .child(
            compact_icon_button(
                colors,
                SharedString::from(format!("manage-mod-delay-{}", asset.key)),
                lucide_icons::icon_clock_3(),
            )
            .on_mouse_down(MouseButton::Left, {
                let key = action_key.clone();
                cx.listener(move |this, _, window, cx| {
                    let asset = resolve_asset_by_key(cx.global::<ManagePageState>(), &key);
                    if let Some(asset) = asset {
                        this.open_mod_delay_prompt(asset, window, cx);
                    }
                })
            }),
        )
        .child(
            compact_icon_button(
                colors,
                SharedString::from(format!("manage-mod-settings-{}", asset.key)),
                lucide_icons::icon_settings_2(),
            )
            .on_mouse_down(MouseButton::Left, {
                let key = action_key.clone();
                cx.listener(move |this, _, window, cx| {
                    let asset = resolve_asset_by_key(cx.global::<ManagePageState>(), &key);
                    if let Some(asset) = asset {
                        this.open_mod_type_dialog(asset, window, cx);
                    }
                })
            }),
        )
}
