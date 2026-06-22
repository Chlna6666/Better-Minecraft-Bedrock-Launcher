use gpui::{App, AppContext, BorrowAppContext, Context, SharedString, Subscription};
use gpui_hooks::hooks::{UseRefHook, UseStateHook};
use std::sync::Arc;

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

pub fn launch_version_icon_path(name: &str) -> &'static str {
    if name.contains("EducationPreview") {
        "images/minecraft/EducationEditionPreview.png"
    } else if name.contains("Education") {
        "images/minecraft/EducationEdition.png"
    } else if name.contains("Preview") || name.contains("Beta") {
        "images/minecraft/Preview.png"
    } else {
        "images/minecraft/Release.png"
    }
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

pub fn ensure_local_versions_loaded(force_refresh: bool, cx: &mut App) {
    let should_spawn = cx.update_global(|state: &mut LocalVersionsState, _cx| {
        if state.loading || (state.loaded && !force_refresh) {
            return false;
        }
        state.loading = true;
        state.error = None;
        true
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

        match result {
            Ok(versions) => {
                if let Err(error) = cx.update_global(|state: &mut LocalVersionsState, _cx| {
                    state.versions = Arc::from(versions);
                    state.loaded = true;
                    state.loading = false;
                    state.error = None;
                }) {
                    tracing::warn!("update local versions failed: {error:?}");
                }
            }
            Err(error) => {
                if let Err(update_error) =
                    cx.update_global(|state: &mut LocalVersionsState, _cx| {
                        state.loaded = !state.versions.is_empty();
                        state.loading = false;
                        state.error = Some(SharedString::from(error.to_string()));
                    })
                {
                    tracing::warn!("update local versions error failed: {update_error:?}");
                }
            }
        }

        cx.update(sync_manage_page_state_from_local_versions)?;
        Ok::<(), anyhow::Error>(())
    })
    .detach();
}
