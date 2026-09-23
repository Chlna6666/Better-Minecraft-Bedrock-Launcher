use super::*;
use crate::ui::components::{dialog, scroll::ScrollableElement};
use std::path::PathBuf;

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
    ImportVersions {
        paths: Vec<String>,
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
pub(super) enum ModTypeDialogTarget {
    ExistingAsset {
        asset: ManageAssetEntry,
    },
    ImportFile {
        path: PathBuf,
        display_name: SharedString,
        current: usize,
        total: usize,
    },
}

#[derive(Clone)]
pub(super) struct ModTypeDialogState {
    pub(super) version: ManagedVersionEntry,
    pub(super) target: ModTypeDialogTarget,
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
                let view_handle = cx.entity().downgrade();
                match start_delete_version_task(folder.clone()) {
                    Ok(task_id) => {
                        self.confirm_dialog = None;
                        let _i18n = cx.global::<I18n>().clone();
                        toast::push(cx, t!("ManagePage.deleting_version"));
                        watch_version_mutation_task(
                            task_id,
                            VersionMutationUiCompletion::Deleted { folder },
                            t!("ManagePage.version_deleted"),
                            view_handle,
                            cx,
                        );
                    }
                    Err(error) => {
                        if let Some(dialog) = self.confirm_dialog.as_mut() {
                            dialog.pending = false;
                        }
                        toast::error(cx, SharedString::from(error));
                    }
                }
                cx.notify();
            }
            ConfirmAction::DeleteAssets {
                version,
                config,
                tab,
                pack_subtype,
                selected_gdk_user,
                folder_names,
            } => {
                match data::start_delete_assets_task(
                    &version,
                    &config,
                    tab,
                    pack_subtype,
                    selected_gdk_user.as_ref().map(SharedString::as_ref),
                    folder_names,
                ) {
                    Ok(task_id) => {
                        self.confirm_dialog = None;
                        let _i18n = cx.global::<I18n>().clone();
                        watch_manage_asset_mutation_task(
                            task_id,
                            t!("ManagePage.asset_deleted"),
                            cx,
                        );
                    }
                    Err(error) => {
                        if let Some(dialog) = self.confirm_dialog.as_mut() {
                            dialog.pending = false;
                        }
                        toast::error(cx, SharedString::from(error));
                    }
                }
                cx.notify();
            }
            ConfirmAction::DeleteScreenshot { entry } => {
                let view_handle = cx.entity().downgrade();
                match data::start_delete_screenshot_task(&entry) {
                    Ok(task_id) => {
                        self.confirm_dialog = None;
                        let _i18n = cx.global::<I18n>().clone();
                        watch_screenshot_mutation_task(
                            task_id,
                            t!("ManagePage.screenshot_deleted"),
                            view_handle,
                            cx,
                        );
                    }
                    Err(error) => {
                        if let Some(dialog) = self.confirm_dialog.as_mut() {
                            dialog.pending = false;
                        }
                        toast::error(cx, SharedString::from(error));
                    }
                }
                cx.notify();
            }
            ConfirmAction::DeleteServer {
                version,
                config,
                selected_gdk_user,
                entry,
            } => {
                let view_handle = cx.entity().downgrade();
                match data::start_delete_external_server_task(
                    &version,
                    &config,
                    selected_gdk_user.as_ref().map(SharedString::as_ref),
                    &entry,
                ) {
                    Ok(task_id) => {
                        self.confirm_dialog = None;
                        let _i18n = cx.global::<I18n>().clone();
                        watch_server_mutation_task(
                            task_id,
                            t!("ManagePage.server_deleted"),
                            view_handle,
                            cx,
                        );
                    }
                    Err(error) => {
                        if let Some(dialog) = self.confirm_dialog.as_mut() {
                            dialog.pending = false;
                        }
                        toast::error(cx, SharedString::from(error));
                    }
                }
                cx.notify();
            }
            ConfirmAction::ImportVersions { paths } => {
                self.confirm_dialog = None;
                start_version_imports(paths, cx);
                cx.notify();
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
        let _i18n = cx.global::<I18n>();
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
        let _i18n = cx.global::<I18n>().clone();
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

                let view_handle = cx.entity().downgrade();
                match start_rename_version_task(old_name, new_name.clone()) {
                    Ok(task_id) => {
                        self.value_prompt = None;
                        watch_version_mutation_task(
                            task_id,
                            VersionMutationUiCompletion::Renamed {
                                new_name: new_name.clone(),
                            },
                            t!("ManagePage.rename_success"),
                            view_handle,
                            cx,
                        );
                    }
                    Err(error) => {
                        let msg = t!("ManagePage.rename_failed", message = &error);
                        toast::error(cx, msg);
                    }
                }
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
                        toast::error(
                            cx,
                            t!("ManagePage.invalid_input", message = &error.to_string()),
                        );
                        return;
                    }
                };
                match crate::tasks::manage_service::start_set_mod_inject_delay(
                    version.folder.to_string(),
                    asset.folder_name.to_string(),
                    delay,
                ) {
                    Ok(task_id) => {
                        self.value_prompt = None;
                        watch_manage_asset_mutation_task(
                            task_id,
                            t!("ManagePage.inject_delay_updated"),
                            cx,
                        );
                        cx.notify();
                    }
                    Err(error) => {
                        toast::error(cx, SharedString::from(error));
                    }
                }
            }
        }
    }

    pub(super) fn close_mod_type_dialog(&mut self, cx: &mut Context<Self>) {
        let importing = self
            .mod_type_dialog
            .as_ref()
            .is_some_and(|dialog| matches!(&dialog.target, ModTypeDialogTarget::ImportFile { .. }));
        self.mod_type_dialog = None;
        if importing {
            self.pending_mod_import_dialogs.clear();
            self.pending_mod_import_items.clear();
        }
        cx.notify();
    }

    pub(super) fn save_mod_type_dialog(&mut self, cx: &mut Context<Self>) {
        let _i18n = cx.global::<I18n>().clone();
        let Some(dialog) = self.mod_type_dialog.as_ref() else {
            return;
        };
        if dialog.pending {
            return;
        }

        let version = dialog.version.clone();
        let target = dialog.target.clone();
        let mod_type = dialog.selected_mod_type.to_string();
        let delay_text = dialog.delay_input.read(cx).value().to_string();
        let delay = match delay_text.trim().parse::<u64>() {
            Ok(value) => value,
            Err(error) => {
                toast::error(
                    cx,
                    t!("ManagePage.invalid_input", message = &error.to_string()),
                );
                return;
            }
        };
        let is_hot_inject = mod_type == "hot-inject";
        let inject_delay_ms = if is_hot_inject { delay } else { 0 };

        match target {
            ModTypeDialogTarget::ExistingAsset { asset } => {
                match crate::tasks::manage_service::start_update_mod_settings(
                    version.folder.to_string(),
                    asset.folder_name.to_string(),
                    mod_type,
                    is_hot_inject.then_some(inject_delay_ms),
                ) {
                    Ok(task_id) => {
                        self.mod_type_dialog = None;
                        watch_manage_asset_mutation_task(
                            task_id,
                            t!("ManagePage.mod_type_updated"),
                            cx,
                        );
                        cx.notify();
                    }
                    Err(error) => {
                        toast::error(cx, SharedString::from(error));
                    }
                }
            }
            ModTypeDialogTarget::ImportFile { path, .. } => {
                self.pending_mod_import_items
                    .push(crate::core::native_mods::NativeModImportItem {
                        path,
                        mod_type,
                        inject_delay_ms,
                    });

                if let Some(next) = self.pending_mod_import_dialogs.pop_front() {
                    self.mod_type_dialog = Some(next);
                    cx.notify();
                    return;
                }

                self.mod_type_dialog = None;
                let items = std::mem::take(&mut self.pending_mod_import_items);
                start_mod_import(version, items, cx);
                cx.notify();
            }
        }
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
    _i18n: &I18n,
    view_handle: WeakEntity<ManagePageView>,
) -> AnyElement {
    let (title, display_name, progress, import_mode) = match &dialog.target {
        ModTypeDialogTarget::ExistingAsset { asset } => (
            t!("ManagePage.mod_settings"),
            asset.display_name.clone(),
            None,
            false,
        ),
        ModTypeDialogTarget::ImportFile {
            display_name,
            current,
            total,
            ..
        } => (
            t!("Manage.mod_import_settings_title"),
            display_name.clone(),
            Some(t!(
                "Manage.mod_import_progress",
                current = &current.to_string(),
                total = &total.to_string()
            )),
            true,
        ),
    };
    let confirm_label = match &dialog.target {
        ModTypeDialogTarget::ImportFile { current, total, .. } if current < total => {
            t!("Manage.mod_import_continue")
        }
        ModTypeDialogTarget::ImportFile { .. } => t!("Manage.mod_import_start"),
        ModTypeDialogTarget::ExistingAsset { .. } => t!("ManagePage.save_mod_settings"),
    };

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
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.text_secondary)
                        .child(display_name),
                )
                .when_some(progress, |this, progress| {
                    this.child(
                        div()
                            .text_size(px(11.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.accent)
                            .child(progress),
                    )
                })
                .when(import_mode, |this| {
                    this.child(
                        div()
                            .px(px(10.))
                            .py(px(8.))
                            .rounded(px(crate::ui::theme::tokens::radius::SM))
                            .bg(Hsla {
                                a: 0.08,
                                ..colors.accent
                            })
                            .text_size(px(11.))
                            .text_color(colors.text_secondary)
                            .child(t!("Manage.mod_import_batch_hint")),
                    )
                })
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
                    confirm_label
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
