use super::*;

/// Virtual mods rendered inline in the normal Mod list.
///
/// Kept as a separate row renderer so plugin-owned entries never enter the
/// filesystem mod batch-selection/deletion path.
pub(super) fn virtual_mods_for_list(
    state: &ManagePageState,
    cx: &mut Context<ManagePageView>,
) -> Vec<crate::plugins::game_mods::GameMod> {
    let query = state
        .asset_search_query
        .as_ref()
        .trim()
        .to_ascii_lowercase();
    crate::plugins::runtime::game_mods(cx)
        .into_iter()
        .filter(|game_mod| {
            query.is_empty()
                || game_mod.declaration.name.to_ascii_lowercase().contains(&query)
                || game_mod.declaration.id.to_ascii_lowercase().contains(&query)
                || game_mod.plugin.to_ascii_lowercase().contains(&query)
        })
        .collect()
}

fn support_version(version: &ManagedVersionEntry) -> &str {
    if version.manifest_version.is_empty() {
        version.version.as_ref()
    } else {
        version.manifest_version.as_ref()
    }
}

pub(super) fn render_virtual_mod_row(
    colors: &ThemeColors,
    version: &ManagedVersionEntry,
    state: &ManagePageState,
    game_mod: &crate::plugins::game_mods::GameMod,
    cx: &mut Context<ManagePageView>,
) -> Stateful<Div> {
    let enabled = game_mod.enabled(&state.version_config.virtual_mods);
    let mod_loading = !state.version_config.disable_mod_loading;
    let supported = game_mod.supports(support_version(version));
    let usable = supported && mod_loading;

    let status = if !supported {
        t!("ManagePage.plugin_game_mod_unavailable")
    } else if !mod_loading {
        t!("ManagePage.plugin_game_mod_disabled_by_version")
    } else if enabled {
        t!("AssetManager.status_enabled")
    } else {
        t!("AssetManager.status_disabled")
    };
    let status_color = if usable && enabled {
        colors.stat_green_text
    } else {
        colors.danger
    };
    let detail = format!(
        "{} · {}",
        game_mod.plugin,
        t!("ManagePage.plugin_game_mods").as_ref()
    );
    let open_dir = game_mod
        .payload
        .parent()
        .map(|path| SharedString::from(path.to_string_lossy().into_owned()));

    // Same 18px leading slot as the filesystem rows, left empty like an
    // unchecked selection box (plugin mods are not batch-selectable).
    let marker = div()
        .w(px(18.))
        .h(px(18.))
        .rounded(px(crate::ui::theme::tokens::radius::MD))
        .border_1()
        .border_color(colors.border)
        .bg(colors.surface);

    let icon = if let Some(icon_path) = game_mod.icon_path.as_ref() {
        let path = SharedString::from(icon_path.to_string_lossy().into_owned());
        rounded_manage_thumbnail(
            colors,
            &path,
            lucide_gpui::icon!(layers).into(),
            colors.surface,
        )
    } else {
        div()
            .w(px(32.))
            .h(px(32.))
            .rounded(px(MANAGE_LIST_THUMBNAIL_RADIUS_PX))
            .bg(Hsla {
                a: 0.10,
                ..colors.surface
            })
            .border_1()
            .border_color(colors.border)
            .flex()
            .items_center()
            .justify_center()
            .child(
                svg()
                    .path(lucide_gpui::icon!(layers))
                    .w(px(16.))
                    .h(px(16.))
                    .text_color(colors.text_secondary),
            )
            .into_any_element()
    };

    let title = formatted_single_line(
        game_mod.declaration.name.clone(),
        colors,
        px(13.),
        colors.text_primary,
    );

    let mut actions = div().flex().items_center().justify_end().gap(px(6.));
    if let Some(open_dir) = open_dir {
        actions = actions.child(
            compact_icon_button(
                colors,
                SharedString::from(format!("manage-virtual-mod-open-{}", game_mod.key)),
                lucide_gpui::icon!(folder_open),
            )
            .on_mouse_down(MouseButton::Left, {
                cx.listener(move |this, _, _, cx| {
                    this.open_asset_folder(open_dir.clone(), cx);
                })
            }),
        );
    }
    let toggle = compact_icon_button(
        colors,
        SharedString::from(format!("manage-virtual-mod-toggle-{}", game_mod.key)),
        if enabled {
            lucide_gpui::icon!(toggle_right)
        } else {
            lucide_gpui::icon!(toggle_left)
        },
    );
    let toggle = if usable {
        toggle.on_mouse_down(MouseButton::Left, {
            let folder = version.folder.clone();
            let game_mod = game_mod.clone();
            cx.listener(move |_this, _, _, cx| {
                save_virtual_mod(folder.clone(), game_mod.clone(), !enabled, cx);
            })
        })
    } else {
        toggle.opacity(0.45)
    };
    actions = actions.child(toggle);

    div()
        .id(SharedString::from(format!(
            "manage-virtual-mod-row-{}",
            game_mod.key
        )))
        .w_full()
        .h(px(MANAGE_ASSET_ROW_HEIGHT_PX))
        .flex_none()
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(Hsla {
            a: 0.16,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.55,
            ..colors.surface
        })
        .px(px(10.))
        .py(px(8.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(10.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .flex()
                        .gap(px(12.))
                        .items_center()
                        .child(marker)
                        .child(icon)
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .gap(px(3.))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .min_w(px(0.))
                                        .child(div().flex_1().min_w(px(0.)).child(title))
                                        .child(
                                            div()
                                                .w(px(6.))
                                                .h(px(6.))
                                                .rounded_full()
                                                .bg(status_color),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(8.))
                                        .overflow_hidden()
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .text_color(colors.text_secondary)
                                                .child(SharedString::from(detail)),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.))
                                                .text_color(status_color)
                                                .child(status),
                                        ),
                                ),
                        ),
                )
                .child(div().flex_shrink_0().child(actions)),
        )
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
