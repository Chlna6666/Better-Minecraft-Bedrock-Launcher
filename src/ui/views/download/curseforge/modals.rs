use super::*;
use std::sync::Arc;

fn curseforge_mod_entry_to_query_data(
    mod_entry: crate::ui::views::download::state::CurseForgeModEntry,
) -> crate::core::curseforge::queries::CurseForgeModSummaryData {
    crate::core::curseforge::queries::CurseForgeModSummaryData {
        id: mod_entry.id,
        name: mod_entry.name.to_string(),
        summary: mod_entry.summary.map(|value| value.to_string()),
        author_names: mod_entry
            .author_names
            .into_iter()
            .map(|value| value.to_string())
            .collect(),
        logo_url: mod_entry.logo_url.map(|value| value.to_string()),
        download_count: mod_entry.download_count,
        date_modified: mod_entry.date_modified.to_string(),
        class_id: mod_entry.class_id,
        category_ids: mod_entry.category_ids,
    }
}

fn curseforge_mod_entry_from_query_data(
    mod_entry: crate::core::curseforge::queries::CurseForgeModSummaryData,
) -> crate::ui::views::download::state::CurseForgeModEntry {
    crate::ui::views::download::state::CurseForgeModEntry {
        id: mod_entry.id,
        name: SharedString::from(mod_entry.name),
        summary: mod_entry.summary.map(SharedString::from),
        author_names: mod_entry
            .author_names
            .into_iter()
            .map(SharedString::from)
            .collect(),
        logo_url: mod_entry.logo_url.map(SharedString::from),
        download_count: mod_entry.download_count,
        date_modified: SharedString::from(mod_entry.date_modified),
        class_id: mod_entry.class_id,
        category_ids: mod_entry.category_ids,
    }
}

fn curseforge_file_entries_from_query_data(
    files: Vec<crate::core::curseforge::queries::CurseForgeFileData>,
) -> Vec<crate::ui::views::download::state::CurseForgeFileEntry> {
    files
        .into_iter()
        .map(
            |file| crate::ui::views::download::state::CurseForgeFileEntry {
                id: file.id,
                display_name: SharedString::from(file.display_name),
                file_name: SharedString::from(file.file_name),
                file_length: file.file_length,
                download_url: file.download_url.map(SharedString::from),
                game_versions: file
                    .game_versions
                    .into_iter()
                    .map(SharedString::from)
                    .collect(),
                file_date: SharedString::from(file.file_date),
            },
        )
        .collect()
}

pub(super) fn render_curseforge_install_modal(
    colors: &ThemeColors,
    state: &DownloadPageState,
    selected_folder: Option<SharedString>,
    local_versions: &crate::ui::hooks::use_local_versions::LocalVersionsSnapshot,
    tasks: &HashMap<Arc<str>, Arc<TaskSnapshot>>,
) -> Div {
    super::render_curseforge_install_modal(colors, state, selected_folder, local_versions, tasks)
}

pub(super) fn render_curseforge_mod_page_modal(
    colors: &ThemeColors,
    state: &DownloadPageState,
    detail_image_cache: &Entity<BoundedImageCache>,
    selected_folder: Option<SharedString>,
    local_versions: &crate::ui::hooks::use_local_versions::LocalVersionsSnapshot,
    tasks: &HashMap<Arc<str>, Arc<TaskSnapshot>>,
) -> Div {
    super::render_curseforge_mod_page_modal(
        colors,
        state,
        detail_image_cache,
        selected_folder,
        local_versions,
        tasks,
    )
}

fn clone_curseforge_mod_entry_by_id(
    mod_id: i32,
    cx: &App,
) -> Option<crate::ui::views::download::state::CurseForgeModEntry> {
    cx.read_global(|state: &DownloadPageState, _cx| {
        state
            .curseforge_mods
            .iter()
            .find(|mod_entry| mod_entry.id == mod_id)
            .cloned()
    })
}

pub(super) fn close_curseforge_install_modal(cx: &mut App) {
    cx.update_global(|state: &mut DownloadPageState, _cx| {
        state.curseforge_install_open = false;
        state.curseforge_install_stage =
            crate::ui::views::download::state::CurseForgeInstallStage::Idle;
    });
}

pub(super) fn close_curseforge_mod_page(cx: &mut App) {
    cx.update_global(|state: &mut DownloadPageState, _cx| {
        state.curseforge_mod_page_open = false;
        state.curseforge_mod_page_loading = false;
        state.curseforge_mod_page_error = None;
        state.curseforge_mod_page_mod_id = None;
        state.curseforge_mod_page_mod = None;
        state.set_curseforge_mod_page_description(SharedString::from(""));
        state
            .curseforge_mod_page_scroll
            .set_offset(point(px(0.), px(0.)));
    });
}

pub(crate) fn handle_close_overlay(cx: &mut App) {
    let (install_open, mod_page_open) = cx.read_global(|state: &DownloadPageState, _cx| {
        (
            state.curseforge_install_open,
            state.curseforge_mod_page_open,
        )
    });

    if install_open {
        close_curseforge_install_modal(cx);
    } else if mod_page_open {
        close_curseforge_mod_page(cx);
    }
}

pub(super) fn default_install_target(
    selected_folder: Option<SharedString>,
    local_versions: &crate::ui::hooks::use_local_versions::LocalVersionsSnapshot,
) -> Option<SharedString> {
    selected_folder.or_else(|| {
        local_versions
            .versions
            .first()
            .map(|version| SharedString::from(version.folder.clone()))
    })
}

pub(super) fn default_install_target_for_file(
    file: &crate::ui::views::download::state::CurseForgeFileEntry,
    selected_folder: Option<SharedString>,
    local_versions: &crate::ui::hooks::use_local_versions::LocalVersionsSnapshot,
) -> Option<SharedString> {
    if let Some(selected_folder) = selected_folder
        && local_version_supports_file(selected_folder.as_ref(), file, local_versions)
    {
        return Some(selected_folder);
    }

    local_versions
        .versions
        .iter()
        .find(|version| launch_version_supports_file(version, file))
        .map(|version| SharedString::from(version.folder.clone()))
        .or_else(|| default_install_target(None, local_versions))
}

fn local_version_supports_file(
    folder: &str,
    file: &crate::ui::views::download::state::CurseForgeFileEntry,
    local_versions: &crate::ui::hooks::use_local_versions::LocalVersionsSnapshot,
) -> bool {
    local_versions.versions.iter().any(|version| {
        version.folder.as_ref() == folder && launch_version_supports_file(version, file)
    })
}

fn launch_version_supports_file(
    version: &crate::core::version::launch_versions::LaunchVersionEntry,
    file: &crate::ui::views::download::state::CurseForgeFileEntry,
) -> bool {
    file.game_versions.iter().any(|game_version| {
        let game_version = game_version.trim();
        !game_version.is_empty()
            && (version_text_matches_game_version(version.version.as_ref(), game_version)
                || version_text_matches_game_version(
                    version.manifest_version.as_ref(),
                    game_version,
                )
                || version_text_matches_game_version(version.folder.as_ref(), game_version)
                || version_text_matches_game_version(version.name.as_ref(), game_version))
    })
}

fn version_text_matches_game_version(version_text: &str, game_version: &str) -> bool {
    let version_text = version_text.trim();
    if version_text == game_version {
        return true;
    }

    let version_parts = numeric_version_parts(version_text);
    let game_version_parts = numeric_version_parts(game_version);
    !game_version_parts.is_empty()
        && version_parts.len() >= game_version_parts.len()
        && game_version_parts
            .iter()
            .zip(version_parts.iter())
            .all(|(game_part, version_part)| game_part == version_part)
}

fn numeric_version_parts(text: &str) -> Vec<u64> {
    let mut parts = Vec::new();
    let mut current = String::new();

    for character in text.chars() {
        if character.is_ascii_digit() {
            current.push(character);
        } else if !current.is_empty() {
            if let Ok(value) = current.parse() {
                parts.push(value);
            }
            current.clear();
        }
    }

    if !current.is_empty()
        && let Ok(value) = current.parse()
    {
        parts.push(value);
    }

    parts
}

pub(super) fn open_curseforge_mod_page(mod_id: i32, cx: &mut App) {
    let found = clone_curseforge_mod_entry_by_id(mod_id, cx);

    cx.update_global(|state: &mut DownloadPageState, _cx| {
        state.curseforge_mod_page_open = true;
        state.curseforge_mod_page_error = None;
        state.curseforge_mod_page_mod_id = Some(mod_id);
        state.curseforge_mod_page_mod = found.clone();
        state.set_curseforge_mod_page_description(SharedString::from(""));
        state.curseforge_mod_page_loading = true;
        state
            .curseforge_mod_page_scroll
            .set_offset(point(px(0.), px(0.)));
        state.curseforge_install_open = false;
        state.curseforge_install_stage =
            crate::ui::views::download::state::CurseForgeInstallStage::LoadingFiles;
        state.curseforge_install_error = None;
        state.curseforge_install_mod = found.clone();
        state.curseforge_install_files.clear();
        state.curseforge_install_selected_file_id = None;
    });

    let cached_mod_entry = found.map(curseforge_mod_entry_to_query_data);
    cx.spawn(async move |cx| {
        let result =
            crate::core::curseforge::queries::load_mod_page(mod_id, cached_mod_entry).await;
        match result {
            Ok(page_data) => {
                let crate::core::curseforge::queries::CurseForgeModPageData {
                    mod_entry,
                    description_html,
                } = page_data;
                let mod_entry = curseforge_mod_entry_from_query_data(mod_entry);
                if let Err(error) = cx.update_global(|state: &mut DownloadPageState, _cx| {
                    if state.curseforge_mod_page_mod_id != Some(mod_id) {
                        return;
                    }
                    state.curseforge_mod_page_loading = false;
                    state.curseforge_mod_page_error = None;
                    state.curseforge_mod_page_mod = Some(mod_entry);
                    state.set_curseforge_mod_page_description(SharedString::from(description_html));
                }) {
                    tracing::debug!("skip curseforge mod page update: {error}");
                }
            }
            Err(error) => {
                if let Err(update_error) = cx.update_global(|state: &mut DownloadPageState, _cx| {
                    if state.curseforge_mod_page_mod_id != Some(mod_id) {
                        return;
                    }
                    state.curseforge_mod_page_loading = false;
                    state.curseforge_mod_page_error = Some(SharedString::from(error.to_string()));
                }) {
                    tracing::debug!("skip curseforge mod page error update: {update_error}");
                }
            }
        }
        Ok::<(), ()>(())
    })
    .detach();

    spawn_load_curseforge_install_files(mod_id, cx);
}

pub(super) fn open_curseforge_install_modal_for_mod_id(
    mod_id: i32,
    default_target: Option<SharedString>,
    cx: &mut App,
) {
    let Some(mod_entry) = clone_curseforge_mod_entry_by_id(mod_id, cx) else {
        tracing::debug!("skip curseforge install modal open for missing mod_id={mod_id}");
        return;
    };

    open_curseforge_install_modal(mod_entry, default_target, cx);
}

pub(super) fn open_curseforge_install_modal(
    mod_entry: crate::ui::views::download::state::CurseForgeModEntry,
    default_target: Option<SharedString>,
    cx: &mut App,
) {
    cx.update_global(|state: &mut DownloadPageState, _cx| {
        state.curseforge_install_open = true;
        state.curseforge_install_stage =
            crate::ui::views::download::state::CurseForgeInstallStage::LoadingFiles;
        state.curseforge_install_error = None;
        state.curseforge_install_mod = Some(mod_entry.clone());
        state.curseforge_install_files.clear();
        state.curseforge_install_selected_file_id = None;
        state.curseforge_install_task_id = None;
        state.curseforge_install_downloaded_path = None;
        state.curseforge_install_conflict_message = None;
        state.curseforge_install_target_folder = default_target;
    });

    spawn_load_curseforge_install_files(mod_entry.id, cx);
}

pub(super) fn open_curseforge_install_modal_for_file(
    mod_entry: crate::ui::views::download::state::CurseForgeModEntry,
    file_id: i32,
    default_target: Option<SharedString>,
    cx: &mut App,
) {
    cx.update_global(|state: &mut DownloadPageState, _cx| {
        state.curseforge_install_open = true;
        state.curseforge_install_stage =
            crate::ui::views::download::state::CurseForgeInstallStage::Idle;
        state.curseforge_install_error = None;
        state.curseforge_install_mod = Some(mod_entry);
        state.curseforge_install_selected_file_id = Some(file_id);
        state.curseforge_install_task_id = None;
        state.curseforge_install_downloaded_path = None;
        state.curseforge_install_conflict_message = None;
        state.curseforge_install_target_folder = default_target;
    });
}

fn spawn_load_curseforge_install_files(mod_id: i32, cx: &mut App) {
    let game_version = cx.read_global(|state: &DownloadPageState, _cx| {
        Some(state.curseforge_selected_game_version.to_string())
    });

    cx.spawn(async move |cx| {
        let result = crate::core::curseforge::queries::load_mod_files(mod_id, game_version).await;
        match result {
            Ok(files) => {
                let files = curseforge_file_entries_from_query_data(files);
                if let Err(error) = cx.update_global(|state: &mut DownloadPageState, _cx| {
                    let current_mod_id = state
                        .curseforge_install_mod
                        .as_ref()
                        .map(|mod_entry| mod_entry.id);
                    if current_mod_id != Some(mod_id) {
                        return;
                    }
                    let previous_selected_id = state.curseforge_install_selected_file_id;
                    let selected_id =
                        previous_selected_id.filter(|id| files.iter().any(|file| file.id == *id));
                    state.curseforge_install_files = files;
                    state.curseforge_install_selected_file_id = selected_id;
                    state.curseforge_install_stage =
                        crate::ui::views::download::state::CurseForgeInstallStage::Idle;
                }) {
                    tracing::debug!("skip curseforge install files update: {error}");
                }
            }
            Err(error) => {
                if let Err(update_error) = cx.update_global(|state: &mut DownloadPageState, _cx| {
                    let current_mod_id = state
                        .curseforge_install_mod
                        .as_ref()
                        .map(|mod_entry| mod_entry.id);
                    if current_mod_id != Some(mod_id) {
                        return;
                    }
                    state.curseforge_install_stage =
                        crate::ui::views::download::state::CurseForgeInstallStage::Error;
                    state.curseforge_install_error = Some(SharedString::from(error.to_string()));
                }) {
                    tracing::debug!("skip curseforge install files error update: {update_error}");
                }
            }
        }
        Ok::<(), ()>(())
    })
    .detach();
}
