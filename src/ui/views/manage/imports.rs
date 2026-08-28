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
        let paths = supported_paths(paths, LOCAL_GAME_PACKAGE_EXTENSIONS);
        if paths.is_empty() {
            let message = t!("Manage.drop_version_package");
            toast::error(cx, message);
            return;
        }
        start_version_imports(paths, cx);
    }

    pub(super) fn import_assets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(import_context) = self.asset_import_context(cx) else {
            let message = t!("Manage.select_import_version");
            toast::error(cx, message);
            return;
        };
        let Some(picker) = asset_picker_spec(import_context.tab) else {
            let message = t!("Manage.import_not_supported");
            toast::error(cx, message);
            return;
        };

        window.defer(cx, move |window, cx| {
            if import_context.tab == ManageTab::Mod {
                let paths = pick_file_paths_with_filter_for_window(
                    window,
                    picker.filter_name,
                    picker.extensions,
                );
                if !paths.is_empty() {
                    start_mod_import(import_context.version, paths, cx);
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
        let Some(import_context) = self.asset_import_context(cx) else {
            let message = t!("Manage.select_import_version");
            toast::error(cx, message);
            return;
        };
        let Some(picker) = asset_picker_spec(import_context.tab) else {
            let message = t!("Manage.drop_import_not_supported");
            toast::error(cx, message);
            return;
        };
        if import_context.tab == ManageTab::Mod {
            let paths = supported_paths(paths, picker.extensions);
            if paths.is_empty() {
                let message = t!("Manage.drop_mod_file");
                toast::error(cx, message);
                return;
            }
            start_mod_import(import_context.version, paths, cx);
        } else {
            crate::ui::window::import::open_dropped_import(
                paths,
                picker.extensions,
                import_context.import_target(),
                window,
                cx,
            );
        }
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

fn start_version_imports(paths: Vec<String>, cx: &mut App) {
    let i18n = cx.global::<I18n>().clone();
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

fn start_mod_import(version: ManagedVersionEntry, paths: Vec<String>, cx: &mut App) {
    let i18n = cx.global::<I18n>().clone();
    cx.spawn(async move |cx| {
        let result = gpui_tokio::Tokio::spawn_result(cx, async move {
            data::import_mod_files(version.folder.as_ref(), &paths)
                .await
                .map(|()| t!("Manage.mods_imported", count = paths.len()))
                .map_err(anyhow::Error::msg)
        })
        .await;

        cx.update(|cx| match result {
            Ok(message) => {
                toast::success(cx, message);
                cx.update_global(|state: &mut ManagePageState, _cx| {
                    state.selected_asset_keys.clear();
                    state.assets_loaded = false;
                    state.assets_loading = false;
                    state.assets_error = None;
                });
            }
            Err(error) => {
                toast::error(cx, SharedString::from(error.to_string()));
            }
        })?;
        Ok::<(), anyhow::Error>(())
    })
    .detach();
}
