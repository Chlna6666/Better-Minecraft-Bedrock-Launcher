use gpui::{App, AppContext, BorrowAppContext, Context, ImageSource, SharedString, Subscription};
use gpui_hooks::hooks::{UseRefHook, UseStateHook};
use std::path::PathBuf;
use std::sync::Arc;

use crate::core::minecraft::paths::{BuildType, Edition, GamePathOptions, get_game_root};
use crate::core::version::launch_versions::{LaunchVersionEntry, sort_launch_versions};
use crate::ui::state::local_versions::LocalVersionsState;
use crate::ui::views::manage::state::{ManagePageState, ManagedVersionEntry};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalVersionsSnapshot {
    pub loaded: bool,
    pub loading: bool,
    pub error: Option<SharedString>,
    pub versions: Arc<[LaunchVersionEntry]>,
}

#[derive(Debug, Eq, PartialEq)]
pub enum LaunchVersionIcon {
    Local(PathBuf),
    Embedded(&'static str),
}

impl From<LaunchVersionIcon> for ImageSource {
    fn from(value: LaunchVersionIcon) -> Self {
        match value {
            LaunchVersionIcon::Local(path) => Self::from(path),
            LaunchVersionIcon::Embedded(path) => Self::from(path),
        }
    }
}

pub fn launch_version_icon_path(custom_icon_path: Option<&str>, name: &str) -> LaunchVersionIcon {
    if let Some(custom_icon_path) = custom_icon_path {
        return LaunchVersionIcon::Local(PathBuf::from(custom_icon_path));
    }

    LaunchVersionIcon::Embedded(if name.contains("EducationPreview") {
        "images/minecraft/EducationEditionPreview.png"
    } else if name.contains("Education") {
        "images/minecraft/EducationEdition.png"
    } else if name.contains("Preview") || name.contains("Beta") {
        "images/minecraft/Preview.png"
    } else {
        "images/minecraft/Release.png"
    })
}

pub fn read_local_versions_snapshot(cx: &App) -> LocalVersionsSnapshot {
    cx.read_global(|state: &LocalVersionsState, _cx| LocalVersionsSnapshot {
        loaded: state.loaded,
        loading: state.loading,
        error: state.error.clone(),
        versions: state.versions.clone(),
    })
}

fn managed_versions_from_local_versions(state: &LocalVersionsSnapshot) -> Vec<ManagedVersionEntry> {
    state
        .versions
        .iter()
        .map(|version| ManagedVersionEntry {
            folder: SharedString::from(version.folder.clone()),
            name: SharedString::from(version.name.clone()),
            version: SharedString::from(version.version.clone()),
            manifest_version: SharedString::from(version.manifest_version.clone()),
            path: SharedString::from(version.path.clone()),
            kind: SharedString::from(version.kind.clone()),
            icon_path: version
                .custom_icon_path
                .as_ref()
                .map(|icon_path| SharedString::from(icon_path.clone())),
        })
        .collect()
}

pub fn use_local_versions<Hooks, View>(
    hooks: &Hooks,
    cx: &mut Context<View>,
) -> LocalVersionsSnapshot
where
    Hooks: UseStateHook + UseRefHook,
    View: 'static,
{
    let snapshot = hooks.use_state(|| read_local_versions_snapshot(cx));
    let subscription = hooks.use_ref(|| None::<Subscription>);

    if subscription.borrow().is_none() {
        let snapshot = snapshot.clone();
        let observer = cx.observe_global::<LocalVersionsState>(move |_, cx| {
            snapshot.set(read_local_versions_snapshot(cx));
            cx.notify();
        });
        *subscription.borrow_mut() = Some(observer);
    }

    let current = read_local_versions_snapshot(cx);
    if snapshot.with(|value| value != &current) {
        snapshot.set(current.clone());
    }

    if !current.loaded && !current.loading && current.versions.is_empty() {
        ensure_local_versions_loaded(false, cx);
    }

    current
}

fn sync_manage_page_state_from_local_versions(cx: &mut App) {
    let snapshot = read_local_versions_snapshot(cx);
    let versions = managed_versions_from_local_versions(&snapshot);

    cx.update_global(|state: &mut ManagePageState, _cx| {
        state.loaded = snapshot.loaded;
        state.loading = snapshot.loading;
        state.error = snapshot.error.clone();
        state.versions = versions;

        if let Some(selected) = state.selected_folder.clone() {
            let exists = state
                .versions
                .iter()
                .any(|version| version.folder == selected);
            if !exists {
                state.selected_folder =
                    state.versions.first().map(|version| version.folder.clone());
            }
        } else {
            state.selected_folder = state.versions.first().map(|version| version.folder.clone());
        }
    });
}

pub fn seed_local_versions(versions: &[LaunchVersionEntry], cx: &mut App) {
    let mut versions = versions.to_vec();
    sort_launch_versions(&mut versions);

    cx.update_global(|state: &mut LocalVersionsState, _cx| {
        if state.loaded || state.loading || !state.versions.is_empty() {
            return;
        }
        state.versions = Arc::from(versions);
        state.loaded = true;
        state.loading = false;
        state.error = None;
    });
    sync_manage_page_state_from_local_versions(cx);
}

pub fn remove_local_version(folder_name: &str, cx: &mut App) {
    let folder_name = folder_name.to_string();

    cx.update_global(|state: &mut LocalVersionsState, _cx| {
        let mut versions = state
            .versions
            .iter()
            .filter(|version| version.folder.as_ref() != folder_name)
            .cloned()
            .collect::<Vec<_>>();
        sort_launch_versions(&mut versions);
        state.versions = Arc::from(versions);
        state.loaded = true;
        state.loading = false;
        state.error = None;
    });

    sync_manage_page_state_from_local_versions(cx);
}

fn request_local_versions_refresh(state: &mut LocalVersionsState, force_refresh: bool) -> bool {
    if state.loading {
        if force_refresh && !state.loading_force_refresh {
            state.refresh_pending = true;
        }
        return false;
    }
    if state.loaded && !force_refresh {
        return false;
    }

    state.loading = true;
    state.loading_force_refresh = force_refresh;
    state.error = None;
    true
}

fn take_pending_local_versions_refresh(state: &mut LocalVersionsState) -> bool {
    std::mem::take(&mut state.refresh_pending)
}

pub fn ensure_local_versions_loaded(force_refresh: bool, cx: &mut App) {
    let should_spawn = cx.update_global(|state: &mut LocalVersionsState, _cx| {
        request_local_versions_refresh(state, force_refresh)
    });

    if !should_spawn {
        return;
    }

    sync_manage_page_state_from_local_versions(cx);
    cx.spawn(async move |cx| {
        let result = async {
            let mut versions = crate::core::version::api::get_version_list().await?;
            sort_launch_versions(&mut versions);
            Ok::<_, anyhow::Error>(versions)
        }
        .await;

        let refresh_again = match result {
            Ok(versions) => {
                match cx.update_global(|state: &mut LocalVersionsState, _cx| {
                    state.versions = Arc::from(versions);
                    state.loaded = true;
                    state.loading = false;
                    state.loading_force_refresh = false;
                    state.error = None;
                    take_pending_local_versions_refresh(state)
                }) {
                    Ok(refresh_again) => refresh_again,
                    Err(error) => {
                        tracing::warn!("update local versions failed: {error:?}");
                        false
                    }
                }
            }
            Err(error) => {
                match cx.update_global(|state: &mut LocalVersionsState, _cx| {
                    state.loaded = !state.versions.is_empty();
                    state.loading = false;
                    state.loading_force_refresh = false;
                    state.error = Some(SharedString::from(error.to_string()));
                    take_pending_local_versions_refresh(state)
                }) {
                    Ok(refresh_again) => refresh_again,
                    Err(update_error) => {
                        tracing::warn!("update local versions error failed: {update_error:?}");
                        false
                    }
                }
            }
        };

        cx.update(sync_manage_page_state_from_local_versions)?;
        if refresh_again {
            cx.update(|cx| ensure_local_versions_loaded(true, cx))?;
        }
        Ok::<(), anyhow::Error>(())
    })
    .detach();
}

pub fn version_build_type(version: &LaunchVersionEntry) -> BuildType {
    if version.kind.eq_ignore_ascii_case("gdk") {
        BuildType::Gdk
    } else {
        BuildType::Uwp
    }
}

pub fn version_edition(version: &LaunchVersionEntry) -> Edition {
    if version.name.contains("Preview") || version.name.contains("Beta") {
        Edition::Preview
    } else {
        Edition::Release
    }
}

pub fn version_enable_isolation(version: &LaunchVersionEntry) -> bool {
    let config_path = std::path::Path::new(&*version.path).join("config.json");
    let content = match std::fs::read_to_string(config_path) {
        Ok(content) => content,
        Err(_) => return false,
    };
    let value = match serde_json::from_str::<serde_json::Value>(&content) {
        Ok(value) => value,
        Err(_) => return false,
    };
    value
        .get("enable_redirection")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

pub fn version_target_root_path(version: &LaunchVersionEntry) -> Option<SharedString> {
    let build_type = version_build_type(version);
    let edition = version_edition(version);
    let enable_isolation = version_enable_isolation(version);
    let options = GamePathOptions {
        build_type,
        edition,
        version_name: version.folder.to_string(),
        enable_isolation,
        user_id: None,
        allow_shared_fallback: false,
    };
    get_game_root(&options).map(|path| SharedString::from(path.to_string_lossy().to_string()))
}

pub fn version_isolation_label(version: &LaunchVersionEntry) -> SharedString {
    if version_enable_isolation(version) {
        SharedString::from("版本隔离")
    } else {
        SharedString::from("系统路径")
    }
}

pub fn version_type_summary_label(version: &LaunchVersionEntry) -> SharedString {
    let platform = if version.kind.eq_ignore_ascii_case("gdk") {
        "GDK"
    } else {
        "UWP"
    };
    let edition = if version.name.contains("Preview") || version.name.contains("Beta") {
        "预览版"
    } else {
        "正式版"
    };
    SharedString::from(format!("{platform} · {edition}"))
}

pub fn launch_version_display_name(version: &LaunchVersionEntry) -> SharedString {
    let ver = version.version.trim();
    if !ver.is_empty() {
        SharedString::from(ver.to_string())
    } else {
        let name = version.name.trim();
        if !name.is_empty() {
            SharedString::from(name.to_string())
        } else {
            SharedString::from(version.folder.to_string())
        }
    }
}

pub fn launch_version_display_type(version: &LaunchVersionEntry) -> &'static str {
    if version.name.contains("Preview")
        || version.name.contains("Beta")
        || version.folder.contains("Preview")
        || version.folder.contains("Beta")
    {
        "预览版"
    } else {
        "正式版"
    }
}

pub fn launch_version_dropdown_label(version: &LaunchVersionEntry) -> SharedString {
    let name = launch_version_display_name(version);
    let type_str = launch_version_display_type(version);
    SharedString::from(format!("{} ({})", name, type_str))
}

pub fn version_primary_label(version: &LaunchVersionEntry) -> SharedString {
    if !version.folder.is_empty() {
        return SharedString::from(version.folder.clone());
    }
    if !version.version.is_empty() {
        return SharedString::from(version.version.clone());
    }
    if !version.manifest_version.is_empty() {
        return SharedString::from(version.manifest_version.clone());
    }
    SharedString::from("Unknown")
}

pub fn version_detail_label(version: &LaunchVersionEntry) -> SharedString {
    let mut parts = Vec::new();
    if !version.version.is_empty() && version.version.as_ref() != version.folder.as_ref() {
        parts.push(version.version.as_ref());
    }
    if !version.manifest_version.is_empty()
        && version.manifest_version.as_ref() != version.version.as_ref()
    {
        parts.push(version.manifest_version.as_ref());
    }
    if parts.is_empty() {
        SharedString::from(version.folder.clone())
    } else {
        SharedString::from(parts.join(" / "))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LaunchVersionIcon, launch_version_icon_path, request_local_versions_refresh,
        take_pending_local_versions_refresh,
    };
    use crate::ui::state::local_versions::LocalVersionsState;
    use std::path::PathBuf;

    #[test]
    fn launch_version_icon_path_prefers_saved_custom_icon() {
        assert_eq!(
            launch_version_icon_path(Some("C:\\Game\\icon.png"), "Minecraft"),
            LaunchVersionIcon::Local(PathBuf::from("C:\\Game\\icon.png"))
        );
    }

    #[test]
    fn launch_version_icon_path_falls_back_to_preview_asset_without_custom_icon() {
        assert_eq!(
            launch_version_icon_path(None, "Minecraft Preview"),
            LaunchVersionIcon::Embedded("images/minecraft/Preview.png")
        );
    }

    #[test]
    fn request_local_versions_refresh_queues_forced_refresh_after_initial_load() {
        let mut state = LocalVersionsState {
            loading: true,
            ..Default::default()
        };

        assert!(!request_local_versions_refresh(&mut state, true));
        assert!(state.refresh_pending);
        assert!(take_pending_local_versions_refresh(&mut state));
        assert!(!take_pending_local_versions_refresh(&mut state));
    }

    #[test]
    fn request_local_versions_refresh_coalesces_duplicate_forced_refreshes() {
        let mut state = LocalVersionsState {
            loading: true,
            loading_force_refresh: true,
            ..Default::default()
        };

        assert!(!request_local_versions_refresh(&mut state, true));
        assert!(!state.refresh_pending);
    }
}
