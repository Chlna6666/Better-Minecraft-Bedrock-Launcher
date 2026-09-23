use std::path::{Path, PathBuf};

use super::*;

const PACK_EXTENSIONS: &[&str] = &["mcpack", "mcaddon", "mctemplate", "zip"];
const SKIN_PACK_EXTENSIONS: &[&str] = &["mcpack", "mcaddon", "zip"];
const MAP_EXTENSIONS: &[&str] = &["mcworld", "mctemplate", "zip"];
const MOD_EXTENSIONS: &[&str] = &["dll"];

#[derive(Clone)]
struct AssetImportContext {
    version: ManagedVersionEntry,
    tab: ManageTab,
    selected_gdk_user: Option<SharedString>,
}

struct ImportPickerSpec {
    filter_name: &'static str,
    extensions: &'static [&'static str],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ManageDropTarget {
    Versions,
    Assets(ManageTab),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ManageDropHoverState {
    pub(super) target: ManageDropTarget,
    pub(super) accepted_count: usize,
    pub(super) rejected_count: usize,
    pub(super) file_summary: SharedString,
}

impl ManagePageView {
    pub(super) fn import_version_package(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.defer(cx, move |window, cx| {
            let Some(path) = pick_file_path_with_filter_for_window(
                window,
                "Packages",
                LOCAL_GAME_PACKAGE_EXTENSIONS,
            ) else {
                return;
            };
            start_version_imports(vec![path], cx);
        });
    }

    pub(super) fn import_dropped_versions(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        self.clear_drop_hover(cx);
        let paths = supported_paths(paths, LOCAL_GAME_PACKAGE_EXTENSIONS);
        if paths.is_empty() {
            let _i18n = cx.global::<I18n>().clone();
            toast::error(cx, t!("Manage.drop_version_package"));
            return;
        }

        let _i18n = cx.global::<I18n>().clone();
        let files = summarize_string_paths(&paths);
        self.confirm_dialog = Some(ConfirmDialogState {
            title: t!("Manage.drop_version_confirm_title"),
            description: t!(
                "Manage.drop_version_confirm_desc",
                count = &paths.len().to_string(),
                files = &files
            ),
            confirm_label: t!("Manage.drop_version_confirm_action"),
            danger: false,
            pending: false,
            action: ConfirmAction::ImportVersions { paths },
        });
        cx.notify();
    }

    pub(super) fn import_assets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(import_context) = self.asset_import_context(cx) else {
            let _i18n = cx.global::<I18n>().clone();
            toast::error(cx, t!("Manage.select_import_version"));
            return;
        };
        let Some(picker) = asset_picker_spec(import_context.tab) else {
            let _i18n = cx.global::<I18n>().clone();
            toast::error(cx, t!("Manage.import_not_supported"));
            return;
        };

        let view_handle = cx.entity().downgrade();
        window.defer(cx, move |window, cx| {
            if import_context.tab == ManageTab::Mod {
                let paths = pick_file_paths_with_filter_for_window(
                    window,
                    picker.filter_name,
                    picker.extensions,
                );
                if !paths.is_empty() {
                    let paths = paths.into_iter().map(PathBuf::from).collect::<Vec<_>>();
                    let version = import_context.version.clone();
                    let _ = view_handle.update(cx, |this, cx| {
                        this.begin_mod_import_confirmation(version, paths, window, cx);
                    });
                }
            } else {
                crate::ui::window::import::pick_and_open_import_window(
                    window,
                    picker.filter_name,
                    picker.extensions,
                    import_context.import_target(),
                    cx,
                );
            }
        });
    }

    pub(super) fn import_dropped_assets(
        &mut self,
        paths: &[PathBuf],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clear_drop_hover(cx);
        let Some(import_context) = self.asset_import_context(cx) else {
            let _i18n = cx.global::<I18n>().clone();
            toast::error(cx, t!("Manage.select_import_version"));
            return;
        };
        let Some(picker) = asset_picker_spec(import_context.tab) else {
            let _i18n = cx.global::<I18n>().clone();
            toast::error(cx, t!("Manage.drop_import_not_supported"));
            return;
        };

        let supported = paths
            .iter()
            .filter(|path| has_supported_extension(path, picker.extensions))
            .cloned()
            .collect::<Vec<_>>();
        if supported.is_empty() {
            let _i18n = cx.global::<I18n>().clone();
            let message = if import_context.tab == ManageTab::Mod {
                t!("Manage.drop_mod_file")
            } else {
                t!(
                    "Import.supported_types",
                    extensions = &picker.extensions.join(", ")
                )
            };
            toast::error(cx, message);
            return;
        }

        if import_context.tab == ManageTab::Mod {
            self.begin_mod_import_confirmation(import_context.version, supported, window, cx);
        } else {
            crate::ui::window::import::open_import_overlay_batch(
                supported,
                import_context.import_target(),
                window,
                cx,
            );
        }
    }

    fn begin_mod_import_confirmation(
        &mut self,
        version: ManagedVersionEntry,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_mod_import_dialogs.clear();
        self.pending_mod_import_items.clear();

        let total = paths.len();
        for (index, path) in paths.into_iter().enumerate() {
            let display_name = file_display_name(&path);
            let Some(delay_input) = create_text_input(window, cx, "输入毫秒延迟", "0") else {
                self.pending_mod_import_dialogs.clear();
                self.pending_mod_import_items.clear();
                return;
            };
            self.pending_mod_import_dialogs.push_back(ModTypeDialogState {
                version: version.clone(),
                target: ModTypeDialogTarget::ImportFile {
                    path,
                    display_name,
                    current: index + 1,
                    total,
                },
                selected_mod_type: SharedString::from("preload-native"),
                delay_input,
                pending: false,
            });
        }

        self.mod_type_dialog = self.pending_mod_import_dialogs.pop_front();
        cx.notify();
    }

    pub(super) fn update_drop_hover(
        &mut self,
        target: ManageDropTarget,
        paths: &[PathBuf],
        cx: &mut Context<Self>,
    ) {
        let extensions = supported_extensions_for_target(target);
        let accepted = extensions
            .map(|extensions| {
                paths
                    .iter()
                    .filter(|path| has_supported_extension(path, extensions))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let next = ManageDropHoverState {
            target,
            accepted_count: accepted.len(),
            rejected_count: paths.len().saturating_sub(accepted.len()),
            file_summary: summarize_paths(accepted.into_iter().map(PathBuf::as_path)),
        };
        if self.drop_hover.as_ref() == Some(&next) {
            return;
        }
        self.drop_hover = Some(next);
        cx.notify();
    }

    pub(super) fn clear_drop_hover(&mut self, cx: &mut Context<Self>) {
        if self.drop_hover.take().is_some() {
            cx.notify();
        }
    }

    pub(super) fn render_drop_hover_overlay(
        &self,
        target: ManageDropTarget,
        colors: &ThemeColors,
        cx: &App,
    ) -> Option<AnyElement> {
        let preview = self.drop_hover.as_ref().filter(|preview| preview.target == target)?;
        let i18n = cx.global::<I18n>();
        let has_selected_version = cx
            .global::<ManagePageState>()
            .selected_folder
            .as_ref()
            .is_some();
        let target_ready = match target {
            ManageDropTarget::Versions => true,
            ManageDropTarget::Assets(
                ManageTab::Mod | ManageTab::ResourcePack | ManageTab::SkinPack | ManageTab::Map,
            ) => has_selected_version,
            ManageDropTarget::Assets(_) => false,
        };
        let accepted = preview.accepted_count > 0 && target_ready;
        let accent = if accepted { colors.accent } else { colors.danger };
        let target_label = drop_target_label(target, i18n);
        let hint = if matches!(target, ManageDropTarget::Assets(_)) && !has_selected_version {
            i18n
                .lookup("Manage.select_import_version")
                .unwrap_or_else(|| SharedString::from("Select a target game version first"))
        } else {
            match target {
            ManageDropTarget::Versions => i18n
                .lookup("Manage.drop_preview_version_hint")
                .unwrap_or_else(|| SharedString::from("Confirm version packages before installation")),
            ManageDropTarget::Assets(ManageTab::Mod) => i18n
                .lookup("Manage.drop_preview_mod_hint")
                .unwrap_or_else(|| SharedString::from("Configure every Mod before import")),
            ManageDropTarget::Assets(
                ManageTab::ResourcePack | ManageTab::SkinPack | ManageTab::Map,
            ) => i18n
                .lookup("Manage.drop_preview_asset_hint")
                .unwrap_or_else(|| SharedString::from("All packages will be parsed in one import preview")),
            ManageDropTarget::Assets(_) => i18n
                .lookup("Manage.drop_import_not_supported")
                .unwrap_or_else(|| SharedString::from("This category does not support import")),
            }
        };
        let count_text = t!(
            "Manage.drop_preview_counts",
            accepted = &preview.accepted_count.to_string(),
            rejected = &preview.rejected_count.to_string()
        );

        Some(
            div()
                .absolute()
                .inset_0()
                .bg(Hsla {
                    a: 0.90,
                    ..colors.settings_panel_bg
                })
                .border_2()
                .border_color(Hsla { a: 0.70, ..accent })
                .rounded(px(crate::ui::theme::tokens::radius::MD))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .max_w(px(460.))
                        .px(px(24.))
                        .py(px(20.))
                        .rounded(px(crate::ui::theme::tokens::radius::MD))
                        .bg(Hsla {
                            a: 0.96,
                            ..colors.surface
                        })
                        .border_1()
                        .border_color(Hsla { a: 0.35, ..accent })
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            svg()
                                .path(lucide_gpui::icon!(download))
                                .w(px(28.))
                                .h(px(28.))
                                .text_color(accent),
                        )
                        .child(
                            div()
                                .text_size(px(16.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(colors.text_primary)
                                .child(target_label),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(accent)
                                .child(count_text),
                        )
                        .when(!preview.file_summary.is_empty(), |this| {
                            this.child(
                                div()
                                    .max_w(px(420.))
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .text_size(px(11.))
                                    .text_color(colors.text_secondary)
                                    .child(preview.file_summary.clone()),
                            )
                        })
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(colors.text_muted)
                                .child(hint),
                        ),
                )
                .into_any_element(),
        )
    }

    fn asset_import_context(&self, cx: &App) -> Option<AssetImportContext> {
        let state = cx.global::<ManagePageState>();
        Some(AssetImportContext {
            version: self.selected_version(state)?.clone(),
            tab: state.tab,
            selected_gdk_user: state.selected_gdk_user.clone(),
        })
    }
}

impl AssetImportContext {
    fn import_target(&self) -> crate::ui::window::import::ImportWindowTarget {
        crate::ui::window::import::ImportWindowTarget::locked(
            self.version.folder.clone(),
            self.version.folder.clone(),
            self.version.version.clone(),
            self.selected_gdk_user.clone(),
        )
    }
}

fn asset_picker_spec(tab: ManageTab) -> Option<ImportPickerSpec> {
    match tab {
        ManageTab::Mod => Some(ImportPickerSpec {
            filter_name: "DLL",
            extensions: MOD_EXTENSIONS,
        }),
        ManageTab::ResourcePack => Some(ImportPickerSpec {
            filter_name: "Packs",
            extensions: PACK_EXTENSIONS,
        }),
        ManageTab::SkinPack => Some(ImportPickerSpec {
            filter_name: "Skin Packs",
            extensions: SKIN_PACK_EXTENSIONS,
        }),
        ManageTab::Map => Some(ImportPickerSpec {
            filter_name: "Maps",
            extensions: MAP_EXTENSIONS,
        }),
        ManageTab::Statistics | ManageTab::Screenshot | ManageTab::Server => None,
    }
}

fn supported_extensions_for_target(target: ManageDropTarget) -> Option<&'static [&'static str]> {
    match target {
        ManageDropTarget::Versions => Some(LOCAL_GAME_PACKAGE_EXTENSIONS),
        ManageDropTarget::Assets(tab) => asset_picker_spec(tab).map(|picker| picker.extensions),
    }
}

fn supported_paths(paths: &[PathBuf], extensions: &[&str]) -> Vec<String> {
    paths
        .iter()
        .filter(|path| has_supported_extension(path, extensions))
        .map(|path| path.to_string_lossy().into_owned())
        .collect()
}

fn has_supported_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extensions
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
}

fn summarize_paths<'a>(paths: impl IntoIterator<Item = &'a Path>) -> SharedString {
    let mut names = paths
        .into_iter()
        .filter_map(|path| path.file_name().and_then(|name| name.to_str()))
        .take(4)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if names.is_empty() {
        return SharedString::from("");
    }
    SharedString::from(names.drain(..).collect::<Vec<_>>().join(" · "))
}

fn summarize_string_paths(paths: &[String]) -> String {
    paths
        .iter()
        .filter_map(|path| {
            Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .take(4)
        .collect::<Vec<_>>()
        .join(" · ")
}

fn file_display_name(path: &Path) -> SharedString {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| SharedString::from(name.to_string()))
        .unwrap_or_else(|| SharedString::from(path.display().to_string()))
}

fn drop_target_label(target: ManageDropTarget, i18n: &I18n) -> SharedString {
    let key = match target {
        ManageDropTarget::Versions => "Manage.drop_target_versions",
        ManageDropTarget::Assets(ManageTab::Mod) => "ManagePage.tabs.mods",
        ManageDropTarget::Assets(ManageTab::ResourcePack) => "ManagePage.tabs.resource",
        ManageDropTarget::Assets(ManageTab::SkinPack) => "ManagePage.tabs.skins",
        ManageDropTarget::Assets(ManageTab::Map) => "ManagePage.tabs.maps",
        ManageDropTarget::Assets(ManageTab::Statistics) => "ManagePage.tabs.statistics",
        ManageDropTarget::Assets(ManageTab::Screenshot) => "ManagePage.tabs.screenshots",
        ManageDropTarget::Assets(ManageTab::Server) => "ManagePage.tabs.servers",
    };
    i18n
        .lookup(key)
        .unwrap_or_else(|| SharedString::from(key.to_string()))
}

pub(super) fn start_version_imports(paths: Vec<String>, cx: &mut App) {
    let _i18n = cx.global::<I18n>().clone();
    cx.spawn(async move |cx| {
        for path in paths {
            let task_id = start_local_game_package_import(path).await;

            cx.update(|cx| match task_id {
                Ok(task_id) => {
                    toast::push(cx, t!("Manage.import_task_started"));
                    watch_import_task(task_id, cx);
                }
                Err(error) => {
                    toast::error(cx, SharedString::from(error));
                }
            })?;
        }
        Ok::<(), anyhow::Error>(())
    })
    .detach();
}

pub(super) fn start_mod_import(
    version: ManagedVersionEntry,
    items: Vec<crate::core::native_mods::NativeModImportItem>,
    cx: &mut App,
) {
    let request = crate::core::native_mods::NativeModImportRequest {
        version_folder: version.folder.to_string(),
        items,
    };
    match crate::core::native_mods::start_import(request) {
        Ok(task_id) => {
            let _i18n = cx.global::<I18n>().clone();
            toast::push(cx, t!("Manage.import_task_started"));
            watch_import_task(task_id, cx);
        }
        Err(error) => {
            toast::error(cx, SharedString::from(error));
        }
    }
}
