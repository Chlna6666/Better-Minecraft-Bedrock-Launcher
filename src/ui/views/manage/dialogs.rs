use super::*;

#[derive(Clone)]
pub(super) enum ConfirmAction {
    DeleteVersion {
        version: ManagedVersionEntry,
    },
    DeleteAssets {
        version: ManagedVersionEntry,
        config: ManageVersionConfig,
        tab: ManageTab,
        pack_subtype: ManagePackSubtype,
        selected_gdk_user: Option<SharedString>,
        folder_names: Vec<String>,
    },
    DeleteScreenshot {
        entry: ManageScreenshotEntry,
    },
    DeleteServer {
        version: ManagedVersionEntry,
        config: ManageVersionConfig,
        selected_gdk_user: Option<SharedString>,
        entry: ManageServerEntry,
    },
}

#[derive(Clone)]
pub(super) struct ConfirmDialogState {
    pub(super) title: SharedString,
    pub(super) description: SharedString,
    pub(super) confirm_label: SharedString,
    pub(super) danger: bool,
    pub(super) pending: bool,
    pub(super) action: ConfirmAction,
}

#[derive(Clone)]
pub(super) enum ValuePromptTarget {
    VersionReducePixels,
    ModInjectDelay {
        version: ManagedVersionEntry,
        asset: ManageAssetEntry,
    },
    LevelDat(level_dat_editor::ValueFieldSpec),
}

#[derive(Clone)]
pub(super) struct ValuePromptDialogState {
    pub(super) title: SharedString,
    pub(super) description: SharedString,
    pub(super) confirm_label: SharedString,
    pub(super) input: Entity<InputState>,
    pub(super) target: ValuePromptTarget,
    pub(super) pending: bool,
}

#[derive(Clone)]
pub(super) struct ModTypeDialogState {
    pub(super) version: ManagedVersionEntry,
    pub(super) asset: ManageAssetEntry,
    pub(super) selected_mod_type: SharedString,
    pub(super) delay_input: Entity<InputState>,
    pub(super) pending: bool,
}

impl ManagePageView {
    pub(super) fn confirm_dialog_close(&mut self, cx: &mut Context<Self>) {
        self.confirm_dialog = None;
        cx.notify();
    }

    pub(super) fn save_confirm_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.confirm_dialog.as_mut() else {
            return;
        };
        if dialog.pending {
            return;
        }
        dialog.pending = true;
        let action = dialog.action.clone();

        match action {
            ConfirmAction::DeleteVersion { version } => {
                let folder = version.folder.to_string();
                self.confirm_dialog = None;
                toast::push(cx, SharedString::from("正在删除版本"));
                cx.spawn(async move |handle, cx| {
                    let result = delete_version(&folder)
                        .await
                        .map_err(|error| error.to_string());

                    let _ = handle.update(cx, |_this, cx| {
                        match result {
                            Ok(()) => {
                                remove_local_version(&folder, cx);
                                toast::success(cx, SharedString::from("版本已删除"));
                                ensure_local_versions_loaded(true, cx);
                            }
                            Err(error) => {
                                toast::error(cx, SharedString::from(error));
                                ensure_local_versions_loaded(true, cx);
                            }
                        }
                        cx.notify();
                    });
                    Ok::<(), anyhow::Error>(())
                })
                .detach();
            }
            ConfirmAction::DeleteAssets {
                version,
                config,
                tab,
                pack_subtype,
                selected_gdk_user,
                folder_names,
            } => {
                cx.spawn(async move |handle, cx| {
                    let result = data::delete_assets(
                        &version,
                        &config,
                        tab,
                        pack_subtype,
                        selected_gdk_user.as_ref().map(SharedString::as_ref),
                        &folder_names,
                    )
                    .await;
                    let _ = handle.update(cx, |this, cx| {
                        match result {
                            Ok(()) => {
                                toast::success(cx, SharedString::from("资源已删除"));
                                this.confirm_dialog = None;
                                cx.update_global(|state: &mut ManagePageState, _cx| {
                                    state.selected_asset_keys.clear();
                                    state.assets_loaded = false;
                                });
                            }
                            Err(error) => {
                                if let Some(dialog) = this.confirm_dialog.as_mut() {
                                    dialog.pending = false;
                                }
                                toast::error(cx, SharedString::from(error));
                            }
                        }
                        cx.notify();
                    });
                    Ok::<(), anyhow::Error>(())
                })
                .detach();
            }
            ConfirmAction::DeleteScreenshot { entry } => {
                cx.spawn(async move |handle, cx| {
                    let result = data::delete_screenshot(&entry).await;
                    let _ = handle.update(cx, |this, cx| {
                        match result {
                            Ok(()) => {
                                toast::success(cx, SharedString::from("截图已删除"));
                                this.confirm_dialog = None;
                                this.last_screenshots_signature = None;
                                cx.update_global(|state: &mut ManagePageState, _cx| {
                                    state.screenshots_loaded = false;
                                    state.screenshots_loading = false;
                                });
                            }
                            Err(error) => {
                                if let Some(dialog) = this.confirm_dialog.as_mut() {
                                    dialog.pending = false;
                                }
                                toast::error(cx, SharedString::from(error));
                            }
                        }
                        cx.notify();
                    });
                    Ok::<(), anyhow::Error>(())
                })
                .detach();
            }
            ConfirmAction::DeleteServer {
                version,
                config,
                selected_gdk_user,
                entry,
            } => {
                cx.spawn(async move |handle, cx| {
                    let result = data::delete_external_server(
                        &version,
                        &config,
                        selected_gdk_user.as_ref().map(SharedString::as_ref),
                        entry.key.as_ref(),
                    )
                    .await;
                    let _ = handle.update(cx, |this, cx| {
                        match result {
                            Ok(()) => {
                                toast::success(cx, SharedString::from("服务器已删除"));
                                this.confirm_dialog = None;
                                this.last_servers_signature = None;
                                cx.update_global(|state: &mut ManagePageState, _cx| {
                                    state.servers_loaded = false;
                                    state.servers_loading = false;
                                    let mut motd = (*state.server_motd).clone();
                                    motd.remove(&entry.key);
                                    state.server_motd = Arc::new(motd);
                                });
                            }
                            Err(error) => {
                                if let Some(dialog) = this.confirm_dialog.as_mut() {
                                    dialog.pending = false;
                                }
                                toast::error(cx, SharedString::from(error));
                            }
                        }
                        cx.notify();
                    });
                    Ok::<(), anyhow::Error>(())
                })
                .detach();
            }
        }
    }

    pub(super) fn close_value_prompt(&mut self, cx: &mut Context<Self>) {
        self.value_prompt = None;
        cx.notify();
    }

    pub(super) fn save_value_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(prompt) = self.value_prompt.as_mut() else {
            return;
        };
        if prompt.pending {
            return;
        }
        let value = prompt.input.read(cx).value().to_string();

        match &prompt.target {
            ValuePromptTarget::VersionReducePixels => {
                let parsed = match value.trim().parse::<i32>() {
                    Ok(value) => value.max(0),
                    Err(error) => {
                        toast::error(cx, SharedString::from(format!("输入无效: {error}")));
                        return;
                    }
                };
                if let Some(modal) = self.version_settings_modal.as_mut() {
                    modal.config.reduce_pixels = parsed;
                }
                self.value_prompt = None;
                cx.notify();
            }
            ValuePromptTarget::LevelDat(field) => {
                let Some(editor) = self.level_dat_editor.as_mut() else {
                    return;
                };
                match level_dat_editor::apply_value_text(&mut editor.document, *field, &value) {
                    Ok(()) => {
                        if let Err(error) = self.sync_level_dat_json_from_document(cx) {
                            toast::error(cx, SharedString::from(error));
                            return;
                        }
                        self.value_prompt = None;
                        cx.notify();
                    }
                    Err(error) => {
                        toast::error(cx, SharedString::from(error));
                    }
                }
            }
            ValuePromptTarget::ModInjectDelay { version, asset } => {
                let delay = match value.trim().parse::<u64>() {
                    Ok(value) => value,
                    Err(error) => {
                        toast::error(cx, SharedString::from(format!("输入无效: {error}")));
                        return;
                    }
                };
                prompt.pending = true;
                let version = version.clone();
                let asset = asset.clone();
                cx.spawn(async move |handle, cx| {
                    let result = data::set_mod_inject_delay(
                        version.folder.as_ref(),
                        asset.folder_name.as_ref(),
                        delay,
                    )
                    .await;
                    let _ = handle.update(cx, |this, cx| {
                        match result {
                            Ok(()) => {
                                this.value_prompt = None;
                                cx.update_global(|state: &mut ManagePageState, _cx| {
                                    state.assets_loaded = false;
                                });
                                toast::success(cx, SharedString::from("注入延迟已更新"));
                            }
                            Err(error) => {
                                if let Some(prompt) = this.value_prompt.as_mut() {
                                    prompt.pending = false;
                                }
                                toast::error(cx, SharedString::from(error));
                            }
                        }
                        cx.notify();
                    });
                    Ok::<(), anyhow::Error>(())
                })
                .detach();
            }
        }
    }

    pub(super) fn close_mod_type_dialog(&mut self, cx: &mut Context<Self>) {
        self.mod_type_dialog = None;
        cx.notify();
    }

    pub(super) fn save_mod_type_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.mod_type_dialog.as_mut() else {
            return;
        };
        if dialog.pending {
            return;
        }
        dialog.pending = true;
        let version = dialog.version.clone();
        let asset = dialog.asset.clone();
        let mod_type = dialog.selected_mod_type.to_string();
        let delay = dialog.delay_input.read(cx).value().to_string();
        let delay = match delay.trim().parse::<u64>() {
            Ok(value) => value,
            Err(error) => {
                dialog.pending = false;
                toast::error(cx, SharedString::from(format!("输入无效: {error}")));
                return;
            }
        };

        cx.spawn(async move |handle, cx| {
            let result = async {
                data::set_mod_type(
                    version.folder.as_ref(),
                    asset.folder_name.as_ref(),
                    &mod_type,
                )
                .await?;
                if mod_type == "hot-inject" {
                    data::set_mod_inject_delay(
                        version.folder.as_ref(),
                        asset.folder_name.as_ref(),
                        delay,
                    )
                    .await?;
                }
                Ok::<(), String>(())
            }
            .await;

            let _ = handle.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.mod_type_dialog = None;
                        cx.update_global(|state: &mut ManagePageState, _cx| {
                            state.assets_loaded = false;
                        });
                        toast::success(cx, SharedString::from("Mod 类型已更新"));
                    }
                    Err(error) => {
                        if let Some(dialog) = this.mod_type_dialog.as_mut() {
                            dialog.pending = false;
                        }
                        toast::error(cx, SharedString::from(error));
                    }
                }
                cx.notify();
            });
            Ok::<(), anyhow::Error>(())
        })
        .detach();
    }
}

pub(super) fn render_confirm_dialog(
    dialog: &ConfirmDialogState,
    colors: &ThemeColors,
    view_handle: WeakEntity<ManagePageView>,
) -> AnyElement {
    let dismiss_handle = view_handle.clone();
    let dismiss = Rc::new(move |cx: &mut App| {
        let _ = dismiss_handle.update(cx, |this, cx| {
            if this
                .confirm_dialog
                .as_ref()
                .is_some_and(|dialog| dialog.pending)
            {
                return;
            }
            this.confirm_dialog_close(cx);
        });
    });

    modal::modal_layer_dismissible(
        div()
            .w_full()
            .max_w(px(480.))
            .rounded(px(22.))
            .border_1()
            .border_color(Hsla {
                a: 0.18,
                ..colors.border
            })
            .bg(colors.settings_panel_bg)
            .flex()
            .flex_col()
            .child(
                div()
                    .p(px(22.))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(colors.text_primary)
                            .child(dialog.title.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .line_height(relative(1.5))
                            .text_color(colors.text_secondary)
                            .child(dialog.description.clone()),
                    ),
            )
            .child(
                div()
                    .px(px(22.))
                    .pb(px(22.))
                    .flex()
                    .justify_end()
                    .gap(px(10.))
                    .child({
                        let view_handle = view_handle.clone();
                        ghost_button(colors, "manage-confirm-cancel", "取消").on_mouse_down(
                            MouseButton::Left,
                            move |_, _, cx| {
                                let _ = view_handle.update(cx, |this, cx| {
                                    this.confirm_dialog_close(cx);
                                });
                            },
                        )
                    })
                    .child({
                        let view_handle = view_handle.clone();
                        primary_button(
                            colors,
                            "manage-confirm-save",
                            if dialog.pending {
                                SharedString::from("处理中...")
                            } else {
                                dialog.confirm_label.clone()
                            },
                        )
                        .bg(if dialog.danger {
                            colors.danger
                        } else {
                            colors.accent
                        })
                        .opacity(if dialog.pending { 0.72 } else { 1.0 })
                        .on_mouse_down(
                            MouseButton::Left,
                            move |_, _, cx| {
                                let _ = view_handle.update(cx, |this, cx| {
                                    this.save_confirm_dialog(cx);
                                });
                            },
                        )
                    }),
            ),
        colors.backdrop,
        dismiss,
    )
    .into_any_element()
}

pub(super) fn render_value_prompt(
    dialog: &ValuePromptDialogState,
    colors: &ThemeColors,
    view_handle: WeakEntity<ManagePageView>,
) -> AnyElement {
    let dismiss_handle = view_handle.clone();
    let dismiss = Rc::new(move |cx: &mut App| {
        let _ = dismiss_handle.update(cx, |this, cx| {
            if this
                .value_prompt
                .as_ref()
                .is_some_and(|dialog| dialog.pending)
            {
                return;
            }
            this.close_value_prompt(cx);
        });
    });

    modal::modal_layer_dismissible(
        div()
            .w_full()
            .max_w(px(520.))
            .rounded(px(22.))
            .border_1()
            .border_color(Hsla {
                a: 0.18,
                ..colors.border
            })
            .bg(colors.settings_panel_bg)
            .flex()
            .flex_col()
            .child(
                div()
                    .p(px(22.))
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(colors.text_primary)
                            .child(dialog.title.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(relative(1.45))
                            .text_color(colors.text_secondary)
                            .child(dialog.description.clone()),
                    )
                    .child(
                        Input::new(&dialog.input)
                            .with_size(InputSize::Medium)
                            .w_full(),
                    ),
            )
            .child(
                div()
                    .px(px(22.))
                    .pb(px(22.))
                    .flex()
                    .justify_end()
                    .gap(px(10.))
                    .child({
                        let view_handle = view_handle.clone();
                        ghost_button(colors, "manage-prompt-cancel", "取消").on_mouse_down(
                            MouseButton::Left,
                            move |_, _, cx| {
                                let _ = view_handle.update(cx, |this, cx| {
                                    this.close_value_prompt(cx);
                                });
                            },
                        )
                    })
                    .child({
                        let view_handle = view_handle.clone();
                        primary_button(
                            colors,
                            "manage-prompt-save",
                            if dialog.pending {
                                SharedString::from("处理中...")
                            } else {
                                dialog.confirm_label.clone()
                            },
                        )
                        .opacity(if dialog.pending { 0.72 } else { 1.0 })
                        .on_mouse_down(
                            MouseButton::Left,
                            move |_, _, cx| {
                                let _ = view_handle.update(cx, |this, cx| {
                                    this.save_value_prompt(cx);
                                });
                            },
                        )
                    }),
            ),
        colors.backdrop,
        dismiss,
    )
    .into_any_element()
}
pub(super) fn render_mod_type_dialog(
    dialog: &ModTypeDialogState,
    colors: &ThemeColors,
    view_handle: WeakEntity<ManagePageView>,
) -> AnyElement {
    let options = vec![
        (
            SharedString::from("preload-native"),
            DropdownOption::from(SharedString::from("Preload Native")),
        ),
        (
            SharedString::from("hot-inject"),
            DropdownOption::from(SharedString::from("Hot Inject")),
        ),
        (
            SharedString::from("native"),
            DropdownOption::from(SharedString::from("Native")),
        ),
        (
            SharedString::from("lse-quickjs"),
            DropdownOption::from(SharedString::from("LSE QuickJS")),
        ),
    ];
    let selected_index = options
        .iter()
        .position(|(value, _)| *value == dialog.selected_mod_type)
        .unwrap_or(0);
    let label = options
        .get(selected_index)
        .map(|(_, option)| option.label.clone())
        .unwrap_or_else(|| SharedString::from("Preload Native"));

    let dropdown = Dropdown::new(
        SharedString::from("manage-mod-type-dropdown"),
        colors,
        px(240.),
        label,
        options
            .iter()
            .map(|(_, option)| option.clone())
            .collect::<Vec<_>>(),
        selected_index,
        !dialog.pending,
        {
            let values = options
                .iter()
                .map(|(value, _)| value.clone())
                .collect::<Vec<_>>();
            let view_handle = view_handle.clone();
            move |index, _window, cx| {
                let selected = values
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| SharedString::from("preload-native"));
                let _ = view_handle.update(cx, |this, cx| {
                    this.set_mod_type_selection(selected, cx);
                });
            }
        },
    );

    let dismiss_handle = view_handle.clone();
    let dismiss = Rc::new(move |cx: &mut App| {
        let _ = dismiss_handle.update(cx, |this, cx| {
            if this
                .mod_type_dialog
                .as_ref()
                .is_some_and(|dialog| dialog.pending)
            {
                return;
            }
            this.close_mod_type_dialog(cx);
        });
    });

    modal::modal_layer_dismissible(
        div()
            .w_full()
            .max_w(px(540.))
            .rounded(px(22.))
            .border_1()
            .border_color(Hsla {
                a: 0.18,
                ..colors.border
            })
            .bg(colors.settings_panel_bg)
            .flex()
            .flex_col()
            .child(
                div()
                    .p(px(22.))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(colors.text_primary)
                            .child("Mod 设置"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(colors.text_secondary)
                            .child(dialog.asset.display_name.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(12.))
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(colors.text_primary)
                                    .child("注入方式"),
                            )
                            .child(dropdown),
                    )
                    .when(
                        dialog.selected_mod_type.as_ref() == "hot-inject",
                        |this: Div| {
                            this.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(8.))
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(colors.text_primary)
                                            .child("注入延迟"),
                                    )
                                    .child(
                                        Input::new(&dialog.delay_input)
                                            .with_size(InputSize::Medium)
                                            .w_full(),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(colors.text_muted)
                                            .child("单位为毫秒，适用于 hot-inject。"),
                                    ),
                            )
                        },
                    ),
            )
            .child(
                div()
                    .px(px(22.))
                    .pb(px(22.))
                    .flex()
                    .justify_end()
                    .gap(px(10.))
                    .child({
                        let view_handle = view_handle.clone();
                        ghost_button(colors, "manage-mod-type-cancel", "取消").on_mouse_down(
                            MouseButton::Left,
                            move |_, _, cx| {
                                let _ = view_handle.update(cx, |this, cx| {
                                    this.close_mod_type_dialog(cx);
                                });
                            },
                        )
                    })
                    .child({
                        let view_handle = view_handle.clone();
                        primary_button(
                            colors,
                            "manage-mod-type-save",
                            if dialog.pending {
                                SharedString::from("保存中...")
                            } else {
                                SharedString::from("保存 Mod 设置")
                            },
                        )
                        .opacity(if dialog.pending { 0.72 } else { 1.0 })
                        .on_mouse_down(
                            MouseButton::Left,
                            move |_, _, cx| {
                                let _ = view_handle.update(cx, |this, cx| {
                                    this.save_mod_type_dialog(cx);
                                });
                            },
                        )
                    }),
            ),
        colors.backdrop,
        dismiss,
    )
    .into_any_element()
}

pub fn render_manage_overlay(
    colors: &ThemeColors,
    i18n: &I18n,
    view: &Entity<ManagePageView>,
    cx: &App,
) -> Option<AnyElement> {
    let (
        version_settings_modal,
        confirm_dialog,
        value_prompt,
        mod_type_dialog,
        server_editor_dialog,
    ) = view.read_with(cx, |this, _| {
        (
            this.version_settings_modal.clone(),
            this.confirm_dialog.clone(),
            this.value_prompt.clone(),
            this.mod_type_dialog.clone(),
            this.server_editor_dialog.clone(),
        )
    });

    if version_settings_modal.is_none()
        && confirm_dialog.is_none()
        && value_prompt.is_none()
        && mod_type_dialog.is_none()
        && server_editor_dialog.is_none()
    {
        return None;
    }

    let view_handle = view.downgrade();
    let mut root = div().absolute().inset_0();

    if let Some(modal) = version_settings_modal.as_ref() {
        root = root.child(version_settings::render(
            modal,
            colors,
            i18n,
            view_handle.clone(),
        ));
    }
    if let Some(dialog) = confirm_dialog.as_ref() {
        root = root.child(render_confirm_dialog(dialog, colors, view_handle.clone()));
    }
    if let Some(dialog) = value_prompt.as_ref() {
        root = root.child(render_value_prompt(dialog, colors, view_handle.clone()));
    }
    if let Some(dialog) = mod_type_dialog.as_ref() {
        root = root.child(render_mod_type_dialog(dialog, colors, view_handle.clone()));
    }
    if let Some(dialog) = server_editor_dialog.as_ref() {
        root = root.child(render_server_editor_dialog(dialog, colors, view_handle));
    }

    Some(root.into_any_element())
}
