use super::*;

pub(super) fn render_virtual_mods(
    colors: &ThemeColors,
    version: &ManagedVersionEntry,
    state: &ManagePageState,
    cx: &mut Context<ManagePageView>,
) -> Option<AnyElement> {
    let mods = crate::plugins::runtime::game_mods(cx);
    if mods.is_empty() {
        return None;
    }
    let mut rows = div().id("plugin-game-mods").max_h(px(218.)).overflow_y_scroll()
        .flex().flex_col().gap(px(6.)).p(px(8.))
        .child(div().text_sm().text_color(colors.text_secondary).child(t!("ManagePage.plugin_game_mods")));
    for game_mod in mods {
        let enabled = game_mod.enabled(&state.version_config.virtual_mods);
        let mod_loading = !state.version_config.disable_mod_loading;
        let supported = game_mod.supports(&version.version);
        let label = match (supported, mod_loading, enabled) {
            (false, _, _) => t!("ManagePage.plugin_game_mod_unavailable"),
            (_, false, _) => t!("ManagePage.plugin_game_mod_disabled_by_version"),
            (_, _, true) => t!("ManagePage.plugin_game_mod_disable"),
            _ => t!("ManagePage.plugin_game_mod_enable"),
        };
        let folder = version.folder.clone();
        let option = div()
            .id(SharedString::from(format!("virtual-mod-{}", game_mod.key)))
            .px(px(10.)).py(px(6.)).rounded(px(6.))
            .bg(colors.surface_hover)
            .text_sm().text_color(colors.text_primary)
            .child(label);
        let option = if supported && mod_loading {
            option.cursor_pointer().on_click(cx.listener({
                let game_mod = game_mod.clone();
                move |_this, _, _, cx| save_virtual_mod(folder.clone(), game_mod.clone(), !enabled, cx)
            }))
        } else { option };
        rows = rows.child(div().flex().justify_between().items_center().gap(px(8.))
            .child(div().flex().flex_col()
                .child(game_mod.declaration.name)
                .child(div().text_xs().text_color(colors.text_muted).child(game_mod.plugin)))
            .child(option));
    }
    Some(rows.into_any_element())
}

fn save_virtual_mod(
    version: SharedString,
    game_mod: crate::plugins::game_mods::GameMod,
    enabled: bool,
    cx: &mut Context<ManagePageView>,
) {
    let folder = version.to_string();
    cx.spawn(async move |handle, cx| {
        let result = crate::tasks::runtime::blocking::run_io(move || {
            crate::plugins::game_mods::set_enabled(&folder, &game_mod, enabled)
        }).await.and_then(|result| result);
        let _ = handle.update(cx, |this, cx| match result {
            Ok(approvals) => {
                let selected = this.selected_version(cx.global::<ManagePageState>())
                    .is_some_and(|entry| entry.folder == version);
                if selected {
                    cx.update_global(|state: &mut ManagePageState, _| state.version_config.virtual_mods = approvals);
                    cx.notify();
                }
            }
            Err(error) => {
                toast::error(cx, SharedString::from(error));
            }
        });
    }).detach();
}

impl ManagePageView {
    pub(super) fn toggle_mod_enabled(&mut self, asset: ManageAssetEntry, cx: &mut Context<Self>) {
        let state = cx.global::<ManagePageState>();
        let Some(version) = self.selected_version(state).cloned() else {
            return;
        };
        let Some(enabled) = asset.enabled else {
            return;
        };

        match crate::tasks::manage_service::start_set_mod_enabled(
            version.folder.to_string(),
            asset.folder_name.to_string(),
            !enabled,
        ) {
            Ok(task_id) => {
                let _i18n = cx.global::<I18n>();
                watch_asset_mutation(task_id, t!("ManagePage.mod_status_updated"), cx);
            }
            Err(error) => {
                toast::error(cx, SharedString::from(error));
            }
        }
    }

    pub(super) fn open_mod_type_dialog(
        &mut self,
        asset: ManageAssetEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if asset.folder_name.to_string().starts_with("levilamina:") {
            toast::error(cx, t!("AssetManager.levilamina_mod_type_locked"));
            return;
        }
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
                .unwrap_or_else(|| SharedString::from("mod-native"));
            (version, selected_mod_type)
        };
        let delay_value = asset.inject_delay_ms.unwrap_or(0).to_string();
        let Some(delay_input) = create_text_input(
            window,
            cx,
            t!("AssetManager.inject_delay_placeholder").as_ref(),
            &delay_value,
        ) else {
            return;
        };
        self.mod_type_dialog = Some(ModTypeDialogState {
            version,
            target: ModTypeDialogTarget::ExistingAsset { asset },
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
            t!("AssetManager.inject_delay_value_placeholder").as_ref(),
            &asset.inject_delay_ms.unwrap_or(0).to_string(),
        ) else {
            return;
        };
        self.value_prompt = Some(ValuePromptDialogState {
            title: t!("AssetManager.inject_delay_title"),
            description: SharedString::from(asset.display_name.clone()),
            confirm_label: t!("common.save"),
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
                    lucide_gpui::icon!(toggle_right)
                } else {
                    lucide_gpui::icon!(toggle_left)
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
                lucide_gpui::icon!(clock_3),
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
                lucide_gpui::icon!(settings_2),
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
