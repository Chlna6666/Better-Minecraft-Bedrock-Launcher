use super::*;
use crate::ui::components::{dialog, scroll::ScrollableElement};

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
    RenameVersion {
        version: ManagedVersionEntry,
    },
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
                let i18n = cx.global::<I18n>().clone();
                let deleting_message = t!("ManagePage.deleting_version");
                toast::push(cx, deleting_message);
                cx.spawn(async move |handle, cx| {
                    let folder_for_delete = folder.clone();
                    let result = gpui_tokio::Tokio::spawn_result(cx, async move {
                        delete_version(&folder_for_delete).await
                    })
                    .await
                    .map_err(|error| error.to_string());

                    let _ = handle.update(cx, |this, cx| {
                        match result {
                            Ok(()) => {
                                remove_local_version(&folder, cx);
                                this.invalidate_version_dependent_data(cx);
                                let message = t!("ManagePage.version_deleted");
                                toast::success(cx, message);
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
                let i18n = cx.global::<I18n>().clone();
                cx.spawn(async move |handle, cx| {
                    let result = gpui_tokio::Tokio::spawn_result(cx, async move {
                        data::delete_assets(
                            &version,
                            &config,
                            tab,
                            pack_subtype,
                            selected_gdk_user.as_ref().map(SharedString::as_ref),
                            &folder_names,
                        )
                        .await
                        .map_err(anyhow::Error::msg)
                    })
                    .await;
                    let _ = handle.update(cx, |this, cx| {
                        match result {
                            Ok(()) => {
                                let message = t!("ManagePage.asset_deleted");
                                toast::success(cx, message);
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
                                toast::error(cx, SharedString::from(error.to_string()));
                            }
                        }
                        cx.notify();
                    });
                    Ok::<(), anyhow::Error>(())
                })
                .detach();
            }
            ConfirmAction::DeleteScreenshot { entry } => {
                let i18n = cx.global::<I18n>().clone();
                cx.spawn(async move |handle, cx| {
                    let result = crate::tasks::runtime::run_blocking(
                        crate::tasks::runtime::BlockingTaskOptions::hidden("Deleting screenshot"),
                        {
                            let entry = entry.clone();
                            move || data::delete_screenshot(&entry)
                        },
                    )
                    .await;
                    let _ = handle.update(cx, |this, cx| {
                        match result {
                            Ok(()) => {
                                let message = t!("ManagePage.screenshot_deleted");
                                toast::success(cx, message);
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
                let i18n = cx.global::<I18n>().clone();
                cx.spawn(async move |handle, cx| {
                    let result = crate::tasks::runtime::run_blocking(
                        crate::tasks::runtime::BlockingTaskOptions::hidden("Deleting server"),
                        {
                            let entry = entry.clone();
                            move || {
                                data::delete_external_server(
                                    &version,
                                    &config,
                                    selected_gdk_user.as_ref().map(SharedString::as_ref),
                                    entry.key.as_ref(),
                                )
                            }
                        },
                    )
                    .await;
                    let _ = handle.update(cx, |this, cx| {
                        match result {
                            Ok(()) => {
                                let message = t!("ManagePage.server_deleted");
                                toast::success(cx, message);
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

    pub(super) fn open_rename_version_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = cx.global::<ManagePageState>();
        let Some(version) = self.selected_version(state).cloned() else {
            return;
        };
        let input = cx.new(|cx| {
            let mut input_state = InputState::new(window, cx);
            input_state.set_value(version.folder.clone(), window, cx);
            input_state
        });
        let i18n = cx.global::<I18n>();
        self.value_prompt = Some(ValuePromptDialogState {
            title: t!("ManagePage.rename_title"),
            description: t!("ManagePage.rename_desc"),
            confirm_label: t!("common.confirm"),
            input,
            target: ValuePromptTarget::RenameVersion { version },
            pending: false,
        });
        cx.notify();
    }

    pub(super) fn close_value_prompt(&mut self, cx: &mut Context<Self>) {
        self.value_prompt = None;
        cx.notify();
    }

    pub(super) fn save_value_prompt(&mut self, cx: &mut Context<Self>) {
        let i18n = cx.global::<I18n>().clone();
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
                        toast::error(
                            cx,
                            t!("ManagePage.invalid_input", message = &error.to_string()),
                        );
                        return;
                    }
                };
                if let Some(modal) = self.version_settings_modal.as_mut() {
                    modal.config.reduce_pixels = parsed;
                }
                self.value_prompt = None;
                cx.notify();
            }
            ValuePromptTarget::RenameVersion { version } => {
                let new_name = value.trim().to_string();
                let old_name = version.folder.to_string();
                if new_name == old_name {
                    self.value_prompt = None;
                    cx.notify();
                    return;
                }
                prompt.pending = true;
                let old_name_clone = old_name.clone();
                let new_name_clone = new_name.clone();
                cx.spawn(async move |handle, cx| {
                    let old_name_for_rename = old_name_clone.clone();
                    let new_name_for_rename = new_name_clone.clone();
                    let result = gpui_tokio::Tokio::spawn_result(cx, async move {
                        data::rename_version_instance(&old_name_for_rename, &new_name_for_rename)
                            .await
                            .map_err(anyhow::Error::msg)
                    })
                    .await;
                    let _ = handle.update(cx, |this, cx| {
                        match result {
                            Ok(()) => {
                                this.value_prompt = None;
                                cx.update_global(|state: &mut ManagePageState, _cx| {
                                    state.selected_folder =
                                        Some(SharedString::from(new_name_clone));
                                });
                                crate::ui::hooks::use_local_versions::ensure_local_versions_loaded(
                                    true, cx,
                                );
                                let msg = t!("ManagePage.rename_success");
                                toast::success(cx, msg);
                            }
                            Err(error) => {
                                if let Some(prompt) = this.value_prompt.as_mut() {
                                    prompt.pending = false;
                                }
                                let msg =
                                    t!("ManagePage.rename_failed", message = &error.to_string());
                                toast::error(cx, msg);
                            }
                        }
                        cx.notify();
                    });
                    Ok::<(), anyhow::Error>(())
                })
                .detach();
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
                        toast::error(
                            cx,
                            t!("ManagePage.invalid_input", message = &error.to_string()),
                        );
                        return;
                    }
                };
                prompt.pending = true;
                let version = version.clone();
                let asset = asset.clone();
                cx.spawn(async move |handle, cx| {
                    let result = gpui_tokio::Tokio::spawn_result(cx, async move {
                        data::set_mod_inject_delay(
                            version.folder.as_ref(),
                            asset.folder_name.as_ref(),
                            delay,
                        )
                        .await
                        .map_err(anyhow::Error::msg)
                    })
                    .await;
                    let _ = handle.update(cx, |this, cx| {
                        match result {
                            Ok(()) => {
                                this.value_prompt = None;
                                cx.update_global(|state: &mut ManagePageState, _cx| {
                                    state.assets_loaded = false;
                                });
                                toast::success(cx, t!("ManagePage.inject_delay_updated"));
                            }
                            Err(error) => {
                                if let Some(prompt) = this.value_prompt.as_mut() {
                                    prompt.pending = false;
                                }
                                toast::error(cx, SharedString::from(error.to_string()));
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
        let i18n = cx.global::<I18n>().clone();
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
                toast::error(
                    cx,
                    t!("ManagePage.invalid_input", message = &error.to_string()),
                );
                return;
            }
        };

        cx.spawn(async move |handle, cx| {
            let result = gpui_tokio::Tokio::spawn_result(cx, async move {
                data::set_mod_type(
                    version.folder.as_ref(),
                    asset.folder_name.as_ref(),
                    &mod_type,
                )
                .await
                .map_err(anyhow::Error::msg)?;
                if mod_type == "hot-inject" {
                    data::set_mod_inject_delay(
                        version.folder.as_ref(),
                        asset.folder_name.as_ref(),
                        delay,
                    )
                    .await
                    .map_err(anyhow::Error::msg)?;
                }
                Ok::<(), anyhow::Error>(())
            })
            .await;

            let _ = handle.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.mod_type_dialog = None;
                        cx.update_global(|state: &mut ManagePageState, _cx| {
                            state.assets_loaded = false;
                        });
                        toast::success(cx, t!("ManagePage.mod_type_updated"));
                    }
                    Err(error) => {
                        if let Some(dialog) = this.mod_type_dialog.as_mut() {
                            dialog.pending = false;
                        }
                        toast::error(cx, SharedString::from(error.to_string()));
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
    i18n: &I18n,
    view_handle: WeakEntity<ManagePageView>,
) -> AnyElement {
    let modal_dismiss_handle = modal::ModalDismissHandle::new();
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

    let confirm_view_handle = view_handle.clone();
    dialog::confirm_dialog(
        i18n,
        colors,
        dialog.title.clone(),
        dialog.description.clone(),
        dialog.confirm_label.clone(),
        dialog.danger,
        dialog.pending,
        modal_dismiss_handle,
        dismiss,
        move |_, _, cx| {
            let _ = confirm_view_handle.update(cx, |this, cx| {
                this.save_confirm_dialog(cx);
            });
        },
    )
}

pub(super) fn render_value_prompt(
    dialog: &ValuePromptDialogState,
    colors: &ThemeColors,
    i18n: &I18n,
    view_handle: WeakEntity<ManagePageView>,
) -> AnyElement {
    let modal_dismiss_handle = modal::ModalDismissHandle::new();
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

    let save_view_handle = view_handle.clone();
    dialog::prompt_dialog(
        i18n,
        colors,
        dialog.title.clone(),
        dialog.description.clone(),
        Input::new(&dialog.input)
            .with_size(InputSize::Medium)
            .w_full(),
        dialog.confirm_label.clone(),
        dialog.pending,
        modal_dismiss_handle,
        dismiss,
        move |_, _, cx| {
            let _ = save_view_handle.update(cx, |this, cx| {
                this.save_value_prompt(cx);
            });
        },
    )
}
pub(super) fn render_mod_type_dialog(
    dialog: &ModTypeDialogState,
    colors: &ThemeColors,
    i18n: &I18n,
    view_handle: WeakEntity<ManagePageView>,
) -> AnyElement {
    let options = vec![
        (
            SharedString::from("preload-native"),
            DropdownOption::from(t!("AssetManager.mod_type_preload_native")),
        ),
        (
            SharedString::from("hot-inject"),
            DropdownOption::from(t!("AssetManager.mod_type_hot_inject")),
        ),
        (
            SharedString::from("native"),
            DropdownOption::from(t!("AssetManager.mod_type_native")),
        ),
        (
            SharedString::from("lse-quickjs"),
            DropdownOption::from(t!("AssetManager.mod_type_lse_quickjs")),
        ),
    ];
    let selected_index = options
        .iter()
        .position(|(value, _)| *value == dialog.selected_mod_type)
        .unwrap_or(0);
    let label = options
        .get(selected_index)
        .map(|(_, option)| option.label.clone())
        .unwrap_or_else(|| t!("AssetManager.mod_type_preload_native"));

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

    let modal_dismiss_handle = modal::ModalDismissHandle::new();
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

    let cancel_dismiss = modal_dismiss_handle.clone();
    let save_view_handle = view_handle.clone();

    let content = dialog::dialog_container(colors, px(540.))
        .child(
            div()
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scrollbar()
                .p(px(22.))
                .flex()
                .flex_col()
                .gap(px(12.))
                .child(
                    div()
                        .text_size(px(18.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .child(t!("ManagePage.mod_settings")),
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
                                .child(t!("ManagePage.injection_method")),
                        )
                        .child(dropdown),
                )
                .when(dialog.selected_mod_type.as_ref() == "hot-inject", |this| {
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
                                    .child(t!("ManagePage.inject_delay")),
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
                                    .child(t!("ManagePage.inject_delay_hint")),
                            ),
                    )
                }),
        )
        .child(dialog::dialog_actions(
            colors,
            ghost_button(colors, "manage-mod-type-cancel", t!("common.cancel")).on_mouse_down(
                MouseButton::Left,
                move |_, _, cx| {
                    cancel_dismiss.dismiss(cx);
                },
            ),
            primary_button(
                colors,
                "manage-mod-type-save",
                if dialog.pending {
                    t!("common.saving")
                } else {
                    t!("ManagePage.save_mod_settings")
                },
            )
            .opacity(if dialog.pending { 0.72 } else { 1.0 })
            .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                let _ = save_view_handle.update(cx, |this, cx| {
                    this.save_mod_type_dialog(cx);
                });
            }),
        ));

    modal::modal_layer_dismissible_with_handle(
        modal_dismiss_handle,
        content,
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
        root = root.child(render_confirm_dialog(
            dialog,
            colors,
            i18n,
            view_handle.clone(),
        ));
    }
    if let Some(dialog) = value_prompt.as_ref() {
        root = root.child(render_value_prompt(
            dialog,
            colors,
            i18n,
            view_handle.clone(),
        ));
    }
    if let Some(dialog) = mod_type_dialog.as_ref() {
        root = root.child(render_mod_type_dialog(
            dialog,
            colors,
            i18n,
            view_handle.clone(),
        ));
    }
    if let Some(dialog) = server_editor_dialog.as_ref() {
        root = root.child(render_server_editor_dialog(
            dialog,
            colors,
            i18n,
            view_handle,
        ));
    }

    Some(root.into_any_element())
}
